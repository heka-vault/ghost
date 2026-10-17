#!/usr/bin/env python3
"""Build a labeled set of short operator turns as Parakeet actually hears them.

Kokoro speaks each phrase (two voices, plus a noisy and a fast variant), Parakeet
transcribes it, and the result is written as JSONL with the intended label. This
gives the classifier the real distribution of mis-hearings without a microphone
or a week of sessions. Output stays in .scratch/ (ignored); nothing leaves the
machine.

    uv run python docs/benchmarks/scripts/jev_make_utterances.py --out .scratch/jev/synthetic.jsonl
"""

from __future__ import annotations

import argparse
import json
import random
import sys
import time
from pathlib import Path

import numpy as np

CACHE = Path.home() / ".cache" / "ghost"

PENDING = {
    "pending_action": 'On ghost-mock/app#151, "Add dark mode toggle to settings": closed the issue.',
    "question": "Close one fifty one, the dark mode one?",
}
NO_PENDING = {
    "pending_action": "",
    "question": "Issue one forty two is waiting on the Redis or Postgres decision.",
}

# label -> phrases. Spoken, so punctuation is only for the TTS.
PHRASES: dict[str, list[str]] = {
    "affirm": [
        "yes", "yeah", "yep", "confirm", "go ahead", "do it", "sure, go ahead", "yes please",
        "yeah close it", "okay do it", "ghost, confirm", "that's right", "approved", "ship it",
        "mm hm, yes", "yes, close it", "go for it", "yeah that's fine", "correct", "proceed",
        "okay yes", "yes do that", "sounds good, do it", "yep go ahead",
    ],
    "deny": [
        "no", "nope", "no don't", "wait", "hold on", "not yet", "no, cancel that", "don't close it",
        "no leave it open", "hmm, no", "actually no", "no wait", "stop", "nah", "no not that one",
        "hold off", "no, the other one", "don't", "no no no", "let's not", "not now",
        "wait, which one", "no leave it", "cancel",
    ],
    "sleep": [
        "go to sleep", "ghost go to sleep", "that's all", "that's all for now", "thanks ghost",
        "thank you ghost", "that's it", "we're done", "goodnight ghost", "okay that's all",
        "sleep", "that'll do", "all done", "thanks, that's everything", "you can go to sleep",
        "we're good", "that's enough for now", "later ghost", "bye ghost", "done for now",
        "okay go to sleep", "thanks that's all", "I'm done", "that's it thanks",
    ],
    "cancel": [
        "never mind", "forget it", "scratch that", "forget that", "skip it", "leave it",
        "never mind that", "drop it", "forget about it", "skip that one", "nah forget it",
        "never mind, skip it", "scrap that", "let's skip it", "leave that one", "forget the comment",
        "moving on", "skip", "next one", "never mind the label", "ignore that", "drop that",
        "let it go", "forget I asked",
    ],
    "repeat": [
        "say that again", "what", "sorry what", "repeat that", "come again", "say again",
        "what was that", "one more time", "pardon", "huh", "what did you say", "repeat",
        "sorry, say that again", "didn't catch that", "I missed that", "again please",
        "can you repeat that", "what was the title", "read that again", "sorry", "hmm what",
        "once more", "what did you say the number was", "say the last part again",
    ],
    "ask": [
        "what needs my sign off", "read me the next one", "what are the labels", "who opened it",
        "what's issue ninety eight about", "show me the infra repo", "any comments on it",
        "what's the oldest one", "label it bug", "add a comment saying looks good",
        "how many are waiting", "what did dev say", "what time is it", "reopen seventeen",
        "what's blocked", "tell me about the redis one", "approve it with a comment",
        "what's left", "which repo is that in", "anything from yesterday", "mark it approved",
        "request changes, the tests are flaky", "what did I decide on the cache",
        "read the first comment",
    ],
    "noise": [
        "hey, you want coffee", "I'll be right back", "where did I put my keys",
        "and I will always love you", "take me down to the paradise city", "the",
        "um", "oh", "so anyway she said", "turn it up", "is that the door", "hello", "okay so",
        "one two three", "we're gonna need a bigger boat", "hey siri", "alexa stop", "ugh",
        "it's raining again", "did you feed the cat", "yeah so like I was saying to him",
        "sweet dreams are made of this", "the quick brown fox", "ah",
    ],
}


def resample(x: np.ndarray, sr_in: int, sr_out: int) -> np.ndarray:
    if sr_in == sr_out:
        return x
    n = int(round(len(x) * sr_out / sr_in))
    return np.interp(np.linspace(0, len(x), n, endpoint=False), np.arange(len(x)), x).astype(
        np.float32
    )


def add_noise(x: np.ndarray, snr_db: float, rng: np.random.Generator) -> np.ndarray:
    p = float(np.mean(x**2)) or 1e-6
    noise = rng.normal(0, 1, len(x)).astype(np.float32)
    noise *= np.sqrt(p / (10 ** (snr_db / 10)) / (np.mean(noise**2) or 1e-6))
    return np.clip(x + noise, -1, 1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=Path(".scratch/jev/synthetic.jsonl"))
    ap.add_argument("--voices", default="af_heart,am_adam")
    ap.add_argument("--snr", type=float, default=8.0, help="dB for the noisy variant")
    ap.add_argument("--fast", type=float, default=1.35, help="speed for the fast variant")
    ap.add_argument("--limit", type=int, default=0, help="phrases per label (0 = all)")
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()

    from kokoro_onnx import Kokoro
    import onnx_asr
    import onnxruntime as ort

    rng = np.random.default_rng(a.seed)
    print("loading kokoro + parakeet", file=sys.stderr)
    kokoro = Kokoro(str(CACHE / "kokoro/kokoro-v1.0.onnx"), str(CACHE / "kokoro/voices-v1.0.bin"))
    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    stt = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", quantization="int8", sess_options=so)

    a.out.parent.mkdir(parents=True, exist_ok=True)
    n = 0
    t0 = time.time()
    with a.out.open("w") as f:
        for label, phrases in PHRASES.items():
            if a.limit:
                phrases = phrases[: a.limit]
            for i, phrase in enumerate(phrases):
                # affirm/deny always answer a question; others alternate so the
                # classifier sees both situations.
                ctx = PENDING if label in {"affirm", "deny"} or i % 2 == 0 else NO_PENDING
                for voice in a.voices.split(","):
                    samples, sr = kokoro.create(phrase, voice=voice, speed=1.0, lang="en-us")
                    clean = resample(np.asarray(samples, dtype=np.float32), sr, 16000)
                    variants = [("clean", clean)]
                    # Degrade a third of the clips: noisy room, or spoken quickly.
                    if i % 3 == 1:
                        variants.append(("noisy", add_noise(clean, a.snr, rng)))
                    elif i % 3 == 2:
                        fast, sr2 = kokoro.create(phrase, voice=voice, speed=a.fast, lang="en-us")
                        variants.append(("fast", resample(np.asarray(fast, dtype=np.float32), sr2, 16000)))
                    for kind, pcm in variants:
                        heard = (stt.recognize(pcm, sample_rate=16000) or "").strip()
                        f.write(
                            json.dumps(
                                {
                                    "text": heard,
                                    "label": label,
                                    "intended": phrase,
                                    "voice": voice,
                                    "variant": kind,
                                    "context": ctx,
                                }
                            )
                            + "\n"
                        )
                        n += 1
            print(f"  {label}: done ({n} so far, {time.time() - t0:.0f}s)", file=sys.stderr)
    print(f"wrote {n} utterances to {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
