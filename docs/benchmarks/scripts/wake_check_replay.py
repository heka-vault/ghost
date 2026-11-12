"""Replay the "hey ghost" detector and its check of a weak hit over recorded streams, under
the rules before and after the 2026-11-12 wake fix (ADR 0025, issues #8 and #13).

    make wake-check-replay ARGS="--stream take.aec.wav:pos@12.3,40.1 --stream room.wav:neg"
    uv run --project tools/oracle python -I docs/benchmarks/scripts/wake_check_replay.py \\
        --stream PATH[#CHANNEL]:LABEL ... [--threshold 0.6] [--floor 0.3] [--cache DIR]

A stream is a 16 kHz WAV (``#1`` picks a channel of a multi-channel one) and a label:

- ``neg``: nothing in it should wake Ghost (room talk, music, Ghost's own voice);
- ``pos@T1,T2,…``: the operator said "hey ghost" with "ghost" ending near those seconds;
  every other hit in it counts as a false wake, but ``~T`` marks another wake phrasing
  ("Ghost, are you there?") whose hit counts neither way;
- ``count=N``: the operator said "hey ghost" N times and nothing else, so every hit is a
  take (``make record NAME=… SECS=30`` at the desk, then the replay on both files);
- ``program=PATH.json``: a replay-rig program (``.scratch/wake-review``), its clips' times
  and texts; the room recordings are aligned on the program's 1 kHz marker.

The detector scores the last 2 s every 80 ms after 2 s of silence, as ``ghost_voice::
wakeword`` does live. A score at the threshold wakes Ghost (GHOST_WAKE_VERIFY=false). A
score from the floor up is in the checked band, and the first frame in it is a weak hit,
though the score may still be rising. The "hold" rows instead wait ``HOLD_FRAMES`` for a
score at the threshold (the alternative measured and turned down on 2026-11-12). Parakeet
reads a weak hit twice as the processor does, to just after it and again 0.8 s later, from
2 s before it, and the rule checks each reading: ``match_wake`` "hey ghost" at 0.8, and
after the fix also ``match_heys_spelling``. A hit holds the detector for the 2 s cooldown.

Each row is one setup: before the fix, after it, the two rejected hold variants, and the
rejected "the name anywhere in the reading".
Nothing is written but the score cache (numbers only). Transcripts are printed only with
``--show-text`` and only to the terminal, so keep that output local (CLAUDE.md).
"""

from __future__ import annotations

import argparse
import json
import re
import wave
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from oracle.wake import match_heys_spelling, match_wake

RATE = 16000
STEP = 1280
WINDOW = 32000
COOLDOWN = 2.0
READ_BEFORE = 2.0
READS_AFTER = (0.2, 0.8)  # the first check, then the final read when they finish
NEAR = 1.2  # a hit this close to a labelled "ghost" is that take
CACHE = Path.home() / ".cache" / "ghost"
MODEL = CACHE / "wake" / "hey_ghost.onnx"
PARAKEET = CACHE / "models" / "parakeet-tdt-0.6b-v3"
NAME = re.compile(r"\bghost\b", re.IGNORECASE)
HOLD_FRAMES = 3  # the rejected alternative: wait for the peak (2026-11-12)

Span = tuple[float, float]


@dataclass(frozen=True)
class Setup:
    name: str
    hold: int
    heys_spelling: bool
    name_anywhere: bool = False  # the word "ghost" anywhere in the reading


SETUPS = (
    Setup("before: match_wake at 0.8", 0, False),
    Setup("after: or match_heys_spelling (shipped)", 0, True),
    Setup("rejected: hold for the peak", HOLD_FRAMES, False),
    Setup("rejected: hold, and heys spelling", HOLD_FRAMES, True),
    Setup("rejected: the name anywhere in the reading", 0, True, True),
)


def read(spec: str) -> np.ndarray:
    path, _, ch = spec.partition("#")
    w = wave.open(path)
    assert w.getframerate() == RATE and w.getsampwidth() == 2, f"{path}: 16 kHz 16-bit only"
    a = np.frombuffer(w.readframes(w.getnframes()), np.int16)
    n = w.getnchannels()
    return a.reshape(-1, n)[:, int(ch or 0)].copy() if n > 1 else a.copy()


def scores(pcm: np.ndarray, cache: Path | None, key: str) -> np.ndarray:
    """The detector's score for each window ending at ``i * STEP`` samples into ``pcm``."""
    f = cache / f"{re.sub(r'[^A-Za-z0-9._-]', '_', key)}.npz" if cache else None
    if f and f.exists():
        return np.load(f)["score"]
    from oracle.wakeword import WakeWordScorer

    s = WakeWordScorer([MODEL])
    padded = np.concatenate([np.zeros(WINDOW, np.int16), pcm])
    out = np.array(
        [s.score(padded[e - WINDOW : e])["hey_ghost"] for e in range(WINDOW, len(padded) + 1, STEP)]
    )
    if f:
        f.parent.mkdir(parents=True, exist_ok=True)
        np.savez(f, score=out)
    return out


def hits(sc: np.ndarray, threshold: float, floor: float, hold: int) -> list[tuple[float, float]]:
    """(seconds, score) of each hit, as ``DetectorWakeProcessor::score`` decides them."""
    out, last, i = [], -1e9, 0
    while i < len(sc):
        t = i * STEP / RATE
        if sc[i] < floor or t - last < COOLDOWN:
            i += 1
            continue
        at = i
        if sc[i] < threshold and hold:
            for j in range(i + 1, min(len(sc), i + hold + 1)):
                if sc[j] >= threshold:
                    at = j
                    break
                if sc[j] > sc[at]:
                    at = j
        out.append((at * STEP / RATE, float(sc[at])))
        last = at * STEP / RATE
        i = at + 1
    return out


def program_positives(path: Path, pcm: np.ndarray, room: bool) -> tuple[list[Span], list[Span]]:
    """The spans of the "hey ghost" clips and of the other wake phrasings, in the stream's
    seconds: a hit anywhere in a clip, or just after it, is that clip."""
    prog = json.loads(path.read_text())
    lag = 0.0
    if room:
        seg = pcm[: RATE * 15].astype(np.float32)
        tone = np.sin(2 * np.pi * 1000 * np.arange(int(RATE * 0.3)) / RATE).astype(np.float32)
        lag = float(np.argmax(np.abs(np.convolve(seg, tone[::-1], mode="valid")))) / RATE
        lag -= prog["marker_at"]
    pos, other = [], []
    for c in prog["clips"]:
        if c["kind"] != "pos":
            continue
        said = re.sub(r"[^a-z ]", "", c["text"].lower())
        (pos if "hey ghost" in said else other).append((c["start"] + lag, c["end"] + lag + 0.6))
    return pos, other


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--stream", action="append", required=True, metavar="PATH[#CH]:LABEL")
    ap.add_argument("--threshold", type=float, default=0.6)
    ap.add_argument("--floor", type=float, default=0.3)
    ap.add_argument("--fuzzy", type=float, default=0.8, help="the weak hit's match_wake fuzz")
    ap.add_argument("--cache", type=Path, default=None, help="keep the scores (numbers) here")
    ap.add_argument("--show-text", action="store_true", help="print Parakeet's readings")
    a = ap.parse_args()

    from oracle.parakeet import ParakeetSTT

    stt = ParakeetSTT(quantization=None)
    if PARAKEET.is_dir():  # the daemon's fp32 copy; else onnx-asr's download
        import onnx_asr
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = 4
        so.inter_op_num_threads = 1
        stt.model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", PARAKEET, sess_options=so)

    # per setup: takes, heard, false wakes, Parakeet runs
    total = {s.name: [0, 0, 0, 0] for s in SETUPS}
    for spec in a.stream:
        src, _, label = spec.rpartition(":")
        pcm = read(src)
        sc = scores(pcm, a.cache, Path(src).name)
        if label.startswith("program="):
            pos, ignore = program_positives(Path(label[8:]), pcm, "room" in label or "rec/" in src)
        elif label.startswith("pos@"):
            marks = [x for x in label[4:].split(",") if x]
            pos = [(float(x) - NEAR, float(x) + NEAR) for x in marks if not x.startswith("~")]
            ignore = [
                (float(x[1:]) - NEAR, float(x[1:]) + NEAR) for x in marks if x.startswith("~")
            ]
        elif label == "neg":
            pos, ignore = [], []
        elif label.startswith("count="):
            # every hit is one of the takes, so it can only be heard or missed
            pos, ignore = [(0.0, len(pcm) / RATE)] * int(label[6:]), []
        else:
            raise SystemExit(f"{spec}: label is neg, pos@T1,T2, count=N or program=PATH")
        readings: dict[float, list[str]] = {}
        line = []
        for setup in SETUPS:
            heard: set[int] = set()
            false = reads = 0
            for at, s in hits(sc, a.threshold, a.floor, setup.hold):
                take = next(
                    (k for k, (lo, hi) in enumerate(pos) if lo <= at <= hi and k not in heard), None
                )
                if take is None and any(lo <= at <= hi for lo, hi in ignore):
                    continue  # "okay ghost" and the like: not this model's phrase
                woke = s >= a.threshold
                if not woke:
                    reads += 1
                    if at not in readings:
                        lo = max(0, int((at - READ_BEFORE) * RATE))
                        readings[at] = [
                            stt.recognize(
                                pcm[lo : min(len(pcm), int((at + x) * RATE))].astype(np.float32)
                                / 32768
                            )
                            for x in READS_AFTER
                        ]
                    texts = readings[at]
                    woke = (
                        any(match_wake(x, ("hey ghost",), fuzzy=a.fuzzy) for x in texts)
                        or (setup.heys_spelling and any(match_heys_spelling(x) for x in texts))
                        or (setup.name_anywhere and any(NAME.search(x) for x in texts))
                    )
                    if a.show_text:
                        print(
                            f"  [{setup.name}] {Path(src).name} {at:7.2f}s {s:.2f}"
                            f" {'take' if take is not None else 'neg '} {texts!r} woke={woke}"
                        )
                if take is not None:
                    heard.add(take) if woke else None
                else:
                    false += woke
            t = total[setup.name]
            t[0] += len(pos)
            t[1] += len(heard)
            t[2] += false
            t[3] += reads
            line.append(f"{len(heard)}/{len(pos)} heard, {false} false, {reads} read")
        print(f"{Path(src).name} [{label.split('=')[0].split('@')[0]}]: " + " | ".join(line))
    print("\n| setup | takes heard | false wakes | Parakeet runs |\n|---|---|---|---|")
    for name, (n, h, f, r) in total.items():
        print(f"| {name} | {h}/{n} | {f} | {r} |")


if __name__ == "__main__":
    main()
