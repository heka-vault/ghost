#!/usr/bin/env python
"""Record the Python daemon's model outputs as parity fixtures for ``crates/ghost-infer``.

The Python daemon is the oracle (ADR 0039): the Rust crate must match these files word for
word, verdict for verdict, and within 1e-3 where numbers are compared. Run it through the
main checkout's venv, which has every model and its Python wrapper installed::

    /home/detroitpro/projects/active/ghost/.venv/bin/python -I scripts/fixtures/record.py

The fixtures land under ``<ghost>/.scratch/spike-fixtures/`` and are never committed. The
clips are the operator's deliberately recorded takes and the synthetic wake-review corpus
(ADR 0025): test data, never passive audio.

Per clip (``clips.json`` lists them):

- ``parakeet/<id>.json``: the transcript as ``ghost_voice.parakeet`` returns it, its tokens.
- ``silero/<id>.json``: the raw model's probability per 512-sample chunk (fresh state, no
  reset), and Pipecat's analyzer over the same chunks with its clock driven by the audio:
  confidence, smoothed volume and VAD state per chunk.
- ``smart_turn/<id>.npy``: the 80x800 log-mel features; ``<id>.json``: probability, verdict.
- ``wake/<id>.json``: each detector's score per 80 ms frame once 2 s are in, and the first hit
  at the production thresholds.

For 50 sentences (``kokoro/sentences.json``): ``kokoro/<n>.json`` with the phoneme string,
the token ids and the trim interval; ``<n>.npy`` the 24 kHz PCM Ghost would play; ``<n>.raw.npy``
the model's output before trimming.

``meta.json`` records the library versions, the model files and the thresholds;
``timings.json`` the Python side's p50/p95 per model on this CPU.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import statistics
import sys
import time
import wave
from pathlib import Path

import numpy as np

SENTENCES = [
    "Done.",
    "Yes.",
    "No, nothing is waiting.",
    "Three issues need you.",
    "Issue 142 is ready for a human.",
    "I commented on heka-vault/ghost#39.",
    "The build is green.",
    "Closing it now.",
    "That's 2,500 lines across 14 files.",
    "It was opened on October 16th, 2026.",
    "Eric asked for the wake word at 0.9.",
    "Parakeet took 0.16 seconds.",
    "Hang on, Ghost.",
    "Okay, the deploy finished at 4:32 p.m.",
    "Dr. Smith replied: not yet.",
    "Redis and Postgres are both up.",
    "There are 12 open pull requests, 3 of them red.",
    "The first one is from Sonia Kowalski.",
    "Labels: needs-triage, needs-info, wontfix.",
    "I'll ask before changing anything.",
    "Version 2026.10.0 is running.",
    "The phone is on WireGuard; the desktop isn't.",
    "Nothing heard on hold is kept.",
    "Should I merge it?",
    "Confirm.",
    "Next issue.",
    "That one is blocked by permissions.",
    "The vault has 7 passwords.",
    "I can't reach GitHub right now.",
    "Kokoro, Parakeet, Silero and Smart Turn are loaded.",
    "Temperature is 71 degrees, humidity 40%.",
    "The meeting moved to 9:15 tomorrow.",
    "Owner slash app colon 142.",
    "Pipecat's aggregator held the sentence.",
    "She said, \"ship it\" and left.",
    "Wait... are you sure?",
    "Right, let's work on the wake word.",
    "One, two, three, four, five.",
    "$29.99 a month, billed yearly.",
    "The ADR is 0039; it supersedes 0003.",
    "Ghost heard you.",
    "Jev scored it 0.97, above the 0.95 bar.",
    "Mobbin's billing notice was dropped.",
    "I need you to look at the build.",
    "What's the status of the infra repo?",
    "Elapsed: 1 hour 12 minutes.",
    "Reading issue #16 (ready-for-agent).",
    "No undo: I'll propose the opposite change.",
    "Tauri built; WebKitGTK is fine.",
    "Good night, Eric.",
]

RATE = 16000
STEP = 1280  # 80 ms, ghost_voice.wakeword.STEP
WINDOW = 32000  # 2 s, ghost_voice.wakeword.WINDOW


def ghost_root() -> Path:
    # The venv is <ghost>/.venv; its python may be a symlink elsewhere, so go by the prefix.
    return Path(sys.prefix).parent


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path)) as w:
        if (w.getframerate(), w.getnchannels(), w.getsampwidth()) != (RATE, 1, 2):
            raise ValueError(f"{path}: not 16 kHz mono int16")
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)


def clip_id(ghost: Path, path: Path) -> str:
    return str(path.relative_to(ghost / ".scratch")).replace("/", "__").removesuffix(".wav")


def find_clips(ghost: Path) -> list[Path]:
    # The 200 labelled corpus clips (labels.jsonl beside the corpus) and the operator's
    # perf utterances; not the minutes-long room recordings under wake-review/rec.
    review = ghost / ".scratch" / "wake-review"
    clips = sorted((review / "corpus").glob("*.wav"))
    clips += sorted((ghost / ".scratch" / "perf" / "utterances").glob("*.wav"))
    return clips


def pct(values: list[float]) -> dict[str, float]:
    if not values:
        return {}
    s = sorted(values)
    return {
        "n": len(s),
        "p50": statistics.median(s),
        "p95": s[min(len(s) - 1, int(round(0.95 * (len(s) - 1))))],
        "max": s[-1],
    }


def dump(path: Path, obj: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, ensure_ascii=False, indent=None) + "\n", encoding="utf-8")


# --- models ----------------------------------------------------------------------------


def record_parakeet(out: Path, clips: list[tuple[str, np.ndarray]], timings: dict) -> None:
    from ghost_voice.parakeet import ParakeetSTTService

    svc = ParakeetSTTService(threads=4)
    svc.load()
    assert svc._model is not None
    timed = svc._model.with_timestamps()
    secs: list[float] = []
    for cid, pcm in clips:
        audio = pcm.astype(np.float32) / 32768.0
        t0 = time.perf_counter()
        text = svc.recognize(audio)
        secs.append(time.perf_counter() - t0)
        res = timed.recognize(audio, sample_rate=RATE)
        dump(
            out / "parakeet" / f"{cid}.json",
            {"text": text, "tokens": res.tokens or [], "secs": secs[-1]},
        )
    timings["parakeet"] = pct(secs)
    print(f"parakeet: {len(clips)} clips, p50 {timings['parakeet']['p50']:.3f}s")


def record_silero(out: Path, clips: list[tuple[str, np.ndarray]], timings: dict) -> None:
    import pipecat.audio.vad.silero as silero_mod
    from pipecat.audio.vad.silero import SileroOnnxModel, SileroVADAnalyzer
    from pipecat.audio.vad.vad_analyzer import VADParams

    model_path = str(Path(silero_mod.__file__).parent / "data" / "silero_vad.onnx")
    params = VADParams(confidence=0.7, start_secs=0.2, stop_secs=0.2, min_volume=0.6)
    secs: list[float] = []
    for cid, pcm in clips:
        n = len(pcm) // 512
        chunks = [pcm[i * 512 : (i + 1) * 512] for i in range(n)]
        raw = SileroOnnxModel(model_path)
        probs = []
        t0 = time.perf_counter()
        for c in chunks:
            probs.append(float(raw(c.astype(np.float32) / 32768.0, RATE)[0][0]))
        secs.append((time.perf_counter() - t0) / max(1, n))

        # The analyzer, its wall clock replaced by the audio clock so the 5 s state resets
        # land at the same chunks every run.
        clock = {"now": 0.0}
        silero_mod.time = type("T", (), {"time": staticmethod(lambda: clock["now"])})()
        an = SileroVADAnalyzer(sample_rate=RATE, params=params)
        an.set_sample_rate(RATE)
        confidences: list[float] = []
        volumes: list[float] = []
        states: list[int] = []
        real_conf = an.voice_confidence
        real_vol = an._get_smoothed_volume

        def conf(buf, _real=real_conf, _c=confidences):
            v = _real(buf)  # a 1-element array, which Pipecat compares as is
            _c.append(float(np.asarray(v).reshape(-1)[0]))
            return v

        def vol(buf, _real=real_vol, _v=volumes):
            v = float(_real(buf))
            _v.append(v)
            return v

        an.voice_confidence = conf  # type: ignore[method-assign]
        an._get_smoothed_volume = vol  # type: ignore[method-assign]
        for i, c in enumerate(chunks):
            clock["now"] = (i + 1) * 512 / RATE
            states.append(an._run_analyzer(c.tobytes()).value)
        silero_mod.time = time
        dump(
            out / "silero" / f"{cid}.json",
            {
                "chunk": 512,
                "params": params.model_dump(),
                "probs": probs,
                "confidences": confidences,
                "volumes": volumes,
                "states": states,
            },
        )
    timings["silero_per_chunk"] = pct(secs)
    print(f"silero: {len(clips)} clips, p50 {timings['silero_per_chunk']['p50'] * 1000:.2f}ms/chunk")


def record_smart_turn(out: Path, clips: list[tuple[str, np.ndarray]], timings: dict) -> None:
    from pipecat.audio.turn.smart_turn._whisper_features import compute_whisper_log_mel_features
    from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import LocalSmartTurnAnalyzerV3

    an = LocalSmartTurnAnalyzerV3(cpu_count=2)
    secs: list[float] = []
    for cid, pcm in clips:
        audio = pcm.astype(np.float32) / 32768.0
        t0 = time.perf_counter()
        res = an._predict_endpoint(audio)
        secs.append(time.perf_counter() - t0)
        n = 8 * RATE
        padded = audio[-n:] if len(audio) > n else np.pad(audio, (n - len(audio), 0))
        feats = compute_whisper_log_mel_features(padded, do_normalize=True)
        (out / "smart_turn").mkdir(parents=True, exist_ok=True)
        np.save(out / "smart_turn" / f"{cid}.npy", feats.astype(np.float32))
        dump(
            out / "smart_turn" / f"{cid}.json",
            {"probability": res["probability"], "complete": res["prediction"] == 1, "secs": secs[-1]},
        )
    timings["smart_turn"] = pct(secs)
    print(f"smart_turn: {len(clips)} clips, p50 {timings['smart_turn']['p50']:.3f}s")


def record_wake(
    out: Path, clips: list[tuple[str, np.ndarray]], timings: dict, meta: dict
) -> None:
    from ghost_voice.wakeword import WakeModel, WakeWordScorer

    cache = Path.home() / ".cache" / "ghost" / "wake"
    paths = [cache / "hey_ghost.onnx", *sorted(cache.glob("ghost_*.onnx"))]
    scorer = WakeWordScorer(paths)
    # ghostd's defaults (apps/ghostd/src/ghostd/config.py): 0.3 with Parakeet's check on,
    # 0.7 with it off, for "hey ghost" and for the ghost_* models alike.
    checked = tuple(WakeModel.known(p, 0.3) for p in paths)
    unchecked = tuple(WakeModel.known(p, 0.7) for p in paths)
    meta["wake"] = {
        "models": [str(p) for p in paths],
        "thresholds": {"checked": 0.3, "unchecked": 0.7},
        "step": STEP,
        "window": WINDOW,
    }

    def first_hit(frames: list[dict[str, float]], models) -> dict | None:
        for k, scores in enumerate(frames):
            over = [(scores.get(m.name, 0.0), m) for m in models if scores.get(m.name, 0.0) >= m.threshold]
            if over:
                score, m = max(over, key=lambda h: h[0])
                return {"model": m.name, "frame": WINDOW // STEP + k, "score": score}
        return None

    secs: list[float] = []
    for cid, clip in clips:
        # Live, the detector's 2 s window slides over a continuous stream; a clip on its own
        # is that clip after silence, so 2 s of silence go in front and every 80 ms step of
        # the clip gets a score.
        pcm = np.concatenate((np.zeros(WINDOW, dtype=np.int16), clip))
        frames: list[dict[str, float]] = []
        k = WINDOW // STEP
        while k * STEP <= len(pcm):
            window = pcm[k * STEP - WINDOW : k * STEP]
            t0 = time.perf_counter()
            frames.append(scorer.score(window))
            secs.append(time.perf_counter() - t0)
            k += 1
        dump(
            out / "wake" / f"{cid}.json",
            {
                "padded": WINDOW,
                "first_frame": WINDOW // STEP,
                "scores": {p.stem: [f.get(p.stem, 0.0) for f in frames] for p in paths},
                "hit_checked": first_hit(frames, checked),
                "hit_unchecked": first_hit(frames, unchecked),
            },
        )
    timings["wake_per_frame"] = pct(secs)
    print(f"wake: {len(clips)} clips, p50 {timings['wake_per_frame'].get('p50', 0) * 1000:.1f}ms/frame")


def record_kokoro(out: Path, timings: dict, meta: dict) -> None:
    from kokoro_onnx import Kokoro
    from kokoro_onnx.chunker import split_phonemes
    from kokoro_onnx.trim import trim as trim_audio
    from phonemizer.backend.espeak.wrapper import EspeakWrapper

    cache = Path.home() / ".cache" / "ghost" / "kokoro"
    model, voices = cache / "kokoro-v1.0.onnx", cache / "voices-v1.0.bin"
    kokoro = Kokoro(str(model), str(voices))
    w = EspeakWrapper()
    meta["kokoro"] = {
        "model": str(model),
        "voices": str(voices),
        "voice": "af_heart",
        "speed": 1.0,
        "lang": "en-us",
        "espeak_version": ".".join(str(v) for v in w.version),
        "espeak_data_path": str(w.data_path),
        "espeak_library": str(w.library_path),
    }
    (out / "kokoro").mkdir(parents=True, exist_ok=True)
    dump(out / "kokoro" / "sentences.json", SENTENCES)
    voice = kokoro.get_voice_style("af_heart")
    secs: list[float] = []
    for i, text in enumerate(SENTENCES):
        phonemes = kokoro.tokenizer.phonemize(text, "en-us")
        batches = split_phonemes(phonemes)
        tokens = kokoro.tokenizer.tokenize(batches[0]) if batches else []
        t0 = time.perf_counter()
        audio, sr = kokoro.create(text, voice="af_heart", speed=1.0, lang="en-us")
        secs.append(time.perf_counter() - t0)
        assert sr == 24000
        raw, _ = kokoro._infer(tokens, kokoro._style_for(voice, len(tokens)), 1.0)
        _, (start, end) = trim_audio(raw)
        np.save(out / "kokoro" / f"{i}.npy", audio.astype(np.float32))
        np.save(out / "kokoro" / f"{i}.raw.npy", raw.astype(np.float32))
        dump(
            out / "kokoro" / f"{i}.json",
            {
                "text": text,
                "phonemes": phonemes,
                "batches": batches,
                "tokens": tokens,
                "trim": [int(start), int(end)],
                "secs": secs[-1],
            },
        )
    timings["kokoro_sentence"] = pct(secs)
    print(f"kokoro: {len(SENTENCES)} sentences, p50 {timings['kokoro_sentence']['p50']:.3f}s")


def record_tables(out: Path) -> None:
    """The filterbanks the Rust crate computes for itself, to check them against."""
    import onnx_asr.preprocessors
    from pipecat.audio.turn.smart_turn import _whisper_features as wf

    with np.load(Path(onnx_asr.preprocessors.__file__).parent / "data" / "fbanks.npz") as data:
        np.save(out / "nemo128_fbanks.npy", data["nemo128"])
    np.save(out / "whisper80_fbanks.npy", wf._MEL_FILTERS.astype(np.float64))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--ghost", type=Path, default=None, help="the main checkout (default: the venv's parent)")
    ap.add_argument("--only", default="", help="comma-separated: parakeet,silero,smart_turn,wake,kokoro")
    ap.add_argument("--limit", type=int, default=0, help="first N clips only (for a quick run)")
    args = ap.parse_args()
    ghost = args.ghost or ghost_root()
    out = args.out or ghost / ".scratch" / "spike-fixtures"
    out.mkdir(parents=True, exist_ok=True)
    only = {s.strip() for s in args.only.split(",") if s.strip()}

    def want(name: str) -> bool:
        return not only or name in only

    paths = find_clips(ghost)
    if args.limit:
        paths = paths[: args.limit]
    clips: list[tuple[str, np.ndarray]] = []
    index = []
    for p in paths:
        pcm = read_wav(p)
        cid = clip_id(ghost, p)
        clips.append((cid, pcm))
        index.append({"id": cid, "wav": str(p), "samples": int(len(pcm))})
    dump(out / "clips.json", index)
    print(f"{len(clips)} clips -> {out}")

    import importlib.metadata as md

    import onnxruntime

    meta: dict = {
        "onnxruntime": onnxruntime.__version__,
        "numpy": np.__version__,
        "onnx_asr": md.version("onnx-asr"),
        "kokoro_onnx": md.version("kokoro-onnx"),
        "pipecat": md.version("pipecat-ai"),
        "livekit_wakeword": md.version("livekit-wakeword"),
        "parakeet_dir": glob.glob(
            os.path.expanduser(
                "~/.cache/huggingface/hub/models--istupakov--parakeet-tdt-0.6b-v3-onnx/snapshots/*/"
            )
        ),
        "cpu": os.cpu_count(),
    }
    timings: dict = {}
    record_tables(out)
    if want("parakeet"):
        record_parakeet(out, clips, timings)
    if want("silero"):
        record_silero(out, clips, timings)
    if want("smart_turn"):
        record_smart_turn(out, clips, timings)
    if want("wake"):
        record_wake(out, clips, timings, meta)
    if want("kokoro"):
        record_kokoro(out, timings, meta)
    dump(out / "meta.json", meta)
    if not only:
        dump(out / "timings.json", timings)
    else:
        existing = json.loads((out / "timings.json").read_text()) if (out / "timings.json").exists() else {}
        existing.update(timings)
        dump(out / "timings.json", existing)
    print(json.dumps(timings, indent=1))


if __name__ == "__main__":
    main()
