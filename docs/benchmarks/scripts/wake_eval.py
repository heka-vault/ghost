"""Offline wake-phrase check over recorded clips: what Parakeet hears, what the matcher does.

    uv run python docs/benchmarks/scripts/wake_eval.py [.scratch/clips/*.wav] [--fuzzy 0.8]

Each clip is transcribed whole and in 2 s windows (the passive listener sees short VAD
segments, so the windows are the realistic case). Clips named ``*-neg*`` are expected
not to wake. Record clips with scripts/record-clip.sh.
"""

from __future__ import annotations

import argparse
import glob
import wave
from pathlib import Path

import numpy as np
from ghost_voice.wake import DEFAULT_WAKE_PHRASES, match_wake, parse_phrases


def load(path: Path) -> np.ndarray:
    """16 kHz mono float32; other rates and stereo are converted (linear resample is
    fine for an evaluation)."""
    with wave.open(str(path)) as w:
        rate, ch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    assert width == 2, f"{path}: need 16-bit PCM"
    pcm = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768
    if ch > 1:
        pcm = pcm.reshape(-1, ch).mean(axis=1)
    if rate != 16000:
        n = int(len(pcm) * 16000 / rate)
        pcm = np.interp(np.linspace(0, len(pcm) - 1, n), np.arange(len(pcm)), pcm)
    return pcm.astype(np.float32)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("clips", nargs="*", default=sorted(glob.glob(".scratch/clips/*.wav")))
    ap.add_argument("--fuzzy", type=float, default=0.8)
    ap.add_argument("--phrases", default="")
    ap.add_argument("--window", type=float, default=2.0)
    a = ap.parse_args()
    if not a.clips:
        raise SystemExit("no clips; record some with scripts/record-clip.sh")
    import onnx_asr

    model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", quantization="int8")
    phrases = parse_phrases(a.phrases) or DEFAULT_WAKE_PHRASES
    sr, win, hop = 16000, int(16000 * a.window), int(16000 * a.window / 2)
    hits = misses = false_wakes = 0
    for clip in a.clips:
        path = Path(clip)
        pcm = load(path)
        expect_wake = "-neg" not in path.stem
        texts = [model.recognize(pcm, sample_rate=sr).strip()]
        for i in range(0, max(1, len(pcm) - win + 1), hop):
            t = model.recognize(pcm[i : i + win], sample_rate=sr).strip()
            if t and t not in texts:
                texts.append(t)
        woke = [(t, match_wake(t, phrases, fuzzy=a.fuzzy)) for t in texts]
        any_wake = any(m for _, m in woke)
        if expect_wake:
            hits += any_wake
            misses += not any_wake
        else:
            false_wakes += any_wake
        mark = "WAKE" if any_wake else "----"
        flag = "" if any_wake == expect_wake else "   <-- unexpected"
        print(f"{mark} {path.name}{flag}")
        for t, m in woke:
            print(f"       {t!r}" + (f"  => {m.phrase!r} + {m.remainder!r}" if m else ""))
    print(f"\nwake clips: {hits} woke, {misses} missed; negative clips: {false_wakes} false wakes")


if __name__ == "__main__":
    main()
