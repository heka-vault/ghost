"""Score the operator's wake takes offline: transcript wake against the trained detector,
alone and with Parakeet confirming it (ADR 0025, issue #8).

    make wake-takes-eval [TAKES=.scratch/wake-takes/2026-10-22] [ARGS="--show-text"]
    uv run python docs/benchmarks/scripts/wake_takes_eval.py [--takes DIR] [--model PATH]

Reads the ``.aec.wav`` takes (what Ghost hears) listed in a session's takes.jsonl, as
scripts/record-wake-takes.py writes them; the default session is the newest under
.scratch/wake-takes. For each take:

- transcript wake (today): Parakeet over the whole take, then ``match_wake`` with the
  default phrases at fuzzy 0.9;
- the detector (livekit-wakeword): 2 s of silence before the take, then every 80 ms frame
  scored on the last 2 s, as its listener does. The max score, and the first frame at or
  above each threshold;
- two-stage, as ghostd will run it: the detector's first hit at 0.1 and at 0.3, confirmed
  when Parakeet on 2 s before to 1 s after it matches "hey ghost" at 0.9;
- end of speech from Silero VAD (an energy endpoint if Silero won't load), so the report
  can say when the detector fires.

The report is printed and written beside the takes as ``eval-<model stem>.md``.
Transcripts are never written anywhere; ``--show-text`` prints them to the terminal only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import statistics
import sys
import time
import wave
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

RATE = 16000
FRAME = 1280  # the listener's 80 ms frame
WINDOW = 2 * RATE  # and the 2 s it scores
PAD_SECS = 2.0
THRESHOLDS = (0.1, 0.3, 0.5, 0.7)
TWO_STAGE = (0.1, 0.3)
VERIFY_BEFORE, VERIFY_AFTER = 2.0, 1.0
FUZZY = 0.9
VERIFY_PHRASES = ("hey ghost",)
SPEECH_PROB = 0.5  # Silero probability that counts as speech for the end-of-speech estimate

REPO = Path(__file__).resolve().parents[3]
TAKES_ROOT = REPO / ".scratch" / "wake-takes"
CACHE = Path.home() / ".cache" / "ghost"
MODEL_CANDIDATES = (
    CACHE / "wake" / "hey_ghost.onnx",
    CACHE / "wake-train" / "models" / "hey_ghost.onnx",
    CACHE / "wake-train" / "models" / "pilot" / "hey_ghost_pilot.onnx",
)


def words(text: str) -> list[str]:
    return re.sub(r"[^\w\s]", " ", text.lower()).split()


def load(path: Path) -> np.ndarray:
    """16 kHz mono float32; other rates and channel counts are converted."""
    with wave.open(str(path)) as w:
        rate, ch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if width != 2:
        raise SystemExit(f"{path}: need 16-bit PCM")
    pcm = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768
    if ch > 1:
        pcm = pcm.reshape(-1, ch).mean(axis=1)
    if rate != RATE:
        n = int(len(pcm) * RATE / rate)
        pcm = np.interp(np.linspace(0, len(pcm) - 1, n), np.arange(len(pcm)), pcm)
    return pcm.astype(np.float32)


def newest_session() -> Path | None:
    sessions = [d for d in TAKES_ROOT.glob("*") if (d / "takes.jsonl").is_file()]
    return max(sessions, key=lambda d: (d / "takes.jsonl").stat().st_mtime, default=None)


def load_takes(session: Path) -> list[dict]:
    """Recorded takes whose file exists, by take number; the last line for a take wins."""
    by_take: dict[int, dict] = {}
    for line in (session / "takes.jsonl").read_text().splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(e, dict) and isinstance(e.get("take"), int):
            by_take[e["take"]] = e
    return [
        e
        for _, e in sorted(by_take.items())
        if e.get("file") and not e.get("skipped") and (session / e["file"]).is_file()
    ]


def default_model() -> Path | None:
    return next((p for p in MODEL_CANDIDATES if p.is_file()), None)


# --- the parts under test -----------------------------------------------------------


class Parakeet:
    """Parakeet TDT 0.6B v3 int8 through onnx-asr, as ghost_voice.parakeet loads it."""

    def __init__(self) -> None:
        import onnx_asr

        self._model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", quantization="int8")

    def __call__(self, pcm: np.ndarray) -> str:
        if len(pcm) < RATE // 10:
            return ""
        return (self._model.recognize(pcm, sample_rate=RATE) or "").strip()


def detector_scores(model, pcm: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Each frame's score and its time in the take (the end of the frame, in seconds).

    The listener keeps the last 25 frames (2 s) and scores them on every new frame once
    the buffer is full; 2 s of silence ahead of the take stands in for the room before it.
    """
    x = np.concatenate([np.zeros(int(PAD_SECS * RATE), np.float32), pcm])
    scores, times = [], []
    for k in range(WINDOW // FRAME, len(x) // FRAME + 1):
        end = k * FRAME
        s = model.predict(x[end - WINDOW : end])
        scores.append(max(s.values()) if s else 0.0)
        times.append(end / RATE - PAD_SECS)
    return np.asarray(scores), np.asarray(times)


def first_hit(scores: np.ndarray, times: np.ndarray, threshold: float) -> float | None:
    above = np.flatnonzero(scores >= threshold)
    return float(times[above[0]]) if above.size else None


class EndOfSpeech:
    """Where speech ends in a take: Silero VAD as Ghost runs it, or an energy endpoint."""

    def __init__(self) -> None:
        self.method = "energy"
        self._silero = None
        try:
            from pipecat.audio.vad.silero import SileroVADAnalyzer

            # The bare model, reset per take: the analyzer's own reset runs on wall time.
            self._silero = SileroVADAnalyzer(sample_rate=RATE)._model
            self.method = "Silero VAD"
        except Exception as e:  # noqa: BLE001 - any failure falls back to energy
            print(f"Silero VAD unavailable ({e}); using an energy endpoint", file=sys.stderr)

    def __call__(self, pcm: np.ndarray) -> float | None:
        if self._silero is not None:
            self._silero.reset_states()
            last = None
            for i in range(0, len(pcm) - 511, 512):
                if float(np.ravel(self._silero(pcm[i : i + 512], RATE))[0]) >= SPEECH_PROB:
                    last = i + 512
            return last / RATE if last is not None else None
        hop = 320  # 20 ms
        n = len(pcm) // hop
        if n == 0:
            return None
        frames = pcm[: n * hop].reshape(n, hop)
        db = 20 * np.log10(np.sqrt(np.mean(frames**2, axis=1)) + 1e-9)
        threshold = max(np.percentile(db, 10) + 10, db.max() - 30)
        speech = np.flatnonzero(db >= threshold)
        return (speech[-1] + 1) * hop / RATE if speech.size else None


@dataclass
class Result:
    take: dict
    transcript_woke: bool
    max_score: float
    first: dict[float, float | None]
    confirmed: dict[float, bool]
    eos: float | None
    texts: list[tuple[str, str]] = field(default_factory=list)  # terminal only

    @property
    def bare(self) -> bool:
        return words(self.take.get("text", "")) == ["hey", "ghost"]


def score_take(path: Path, take: dict, stt: Parakeet, model, eos: EndOfSpeech) -> Result:
    from ghost_voice.wake import DEFAULT_WAKE_PHRASES, match_wake

    pcm = load(path)
    whole = stt(pcm)
    texts = [("whole take", whole)]
    transcript_woke = match_wake(whole, DEFAULT_WAKE_PHRASES, fuzzy=FUZZY) is not None
    scores, times = detector_scores(model, pcm)
    first = {t: first_hit(scores, times, t) for t in THRESHOLDS}
    confirmed: dict[float, bool] = {}
    heard: dict[tuple[int, int], str] = {}
    for t in TWO_STAGE:
        hit = first[t]
        if hit is None:
            confirmed[t] = False
            continue
        span = (
            max(0, int((hit - VERIFY_BEFORE) * RATE)),
            min(len(pcm), int((hit + VERIFY_AFTER) * RATE)),
        )
        if span not in heard:
            heard[span] = stt(pcm[span[0] : span[1]])
            texts.append((f"verify {hit:.2f} s", heard[span]))
        confirmed[t] = match_wake(heard[span], VERIFY_PHRASES, fuzzy=FUZZY) is not None
    return Result(
        take=take,
        transcript_woke=transcript_woke,
        max_score=float(scores.max()) if scores.size else 0.0,
        first=first,
        confirmed=confirmed,
        eos=eos(pcm),
        texts=texts,
    )


# --- the report ---------------------------------------------------------------------


def _cols() -> list[str]:
    return (
        ["Transcript wake"]
        + [f"Detector {t:g}" for t in THRESHOLDS]
        + [f"Two-stage {t:g}" for t in TWO_STAGE]
    )


def _counts(rs: list[Result]) -> list[int]:
    return (
        [sum(r.transcript_woke for r in rs)]
        + [sum(r.first[t] is not None for r in rs) for t in THRESHOLDS]
        + [sum(r.confirmed[t] for r in rs) for t in TWO_STAGE]
    )


def _table(
    title: str, groups: list[tuple[str, str, list[Result]]], total: bool, second: str = "Condition"
) -> list[str]:
    head = ["Block", second, "Takes", *_cols()]
    out = [f"| {' | '.join(head)} |", "|" + "---|" * len(head)]
    rows = list(groups)
    if total and len(groups) > 1:
        rows.append(("**All**", "", [r for _, _, rs in groups for r in rs]))
    for block, cond, rs in rows:
        cells = [f"{c}/{len(rs)}" for c in _counts(rs)]
        out.append(f"| {block} | {cond} | {len(rs)} | {' | '.join(cells)} |")
    return [f"## {title}", "", *out, ""]


def _groups(rs: list[Result], key) -> list[tuple[str, str, list[Result]]]:
    seen: dict[tuple[str, str], list[Result]] = {}
    for r in rs:
        seen.setdefault(key(r), []).append(r)
    return [(a, b, v) for (a, b), v in seen.items()]


def _p50(values: list[float]) -> str:
    return f"{statistics.median(values):+.2f} s" if values else "n/a"


def block_cond(r: Result) -> tuple[str, str]:
    return r.take.get("block", "?"), r.take.get("condition", "")


def block_line(r: Result) -> tuple[str, str]:
    return r.take.get("block", "?"), r.take.get("text", "")


def report(session: Path, model_path: Path, results: list[Result], eos_method: str) -> str:
    by = {lab: [r for r in results if r.take.get("label") == lab] for lab in ("wake", "negative")}
    other = [r for r in results if r.take.get("label") not in ("wake", "negative")]
    lines = [
        f"# Wake takes: {session.name}, {model_path.stem}",
        "",
        f"{len(by['wake'])} wake takes, {len(by['negative'])} near misses and {len(other)} "
        f"other phrases from `{session.name}`, as ghost.mic heard them (`.aec.wav`).",
        "",
        "- **Transcript wake** is today's path: Parakeet TDT 0.6B v3 int8 over the whole take, "
        f"then `match_wake` with the default phrases at fuzzy {FUZZY}. Live, Parakeet sees "
        "VAD segments rather than whole takes.",
        f"- **Detector** is `{model_path.name}` through livekit-wakeword: 2 s of silence before "
        "the take, every 80 ms frame scored on the last 2 s. A take counts as heard at a "
        "threshold when any frame reaches it.",
        "- **Two-stage** is the detector's first hit at that threshold, confirmed when Parakeet "
        f'on {VERIFY_BEFORE:g} s before to {VERIFY_AFTER:g} s after it matches "hey ghost" '
        f"at {FUZZY}. A rejected first hit is not retried.",
        "",
    ]
    if by["wake"]:
        lines += _table("Wake takes heard", _groups(by["wake"], block_cond), total=True)
    if by["negative"]:
        lines += _table("False wakes on near misses", _groups(by["negative"], block_cond), True)
    if other:
        lines += _table(
            'Other phrases (reported apart: the detector and the verifier know only "hey ghost")',
            _groups(other, block_line),
            total=False,
            second="Line",
        )
    if by["wake"]:
        lines += [
            "## When the detector fires",
            "",
            f"Detector fire time minus end of speech ({eos_method}), p50, over the wake takes "
            "it heard. Negative means it fired before the operator finished speaking.",
            "",
            '| Threshold | "Hey Ghost." alone | n | With a request | n |',
            "|---|---|---|---|---|",
        ]
        for t in THRESHOLDS:
            row = []
            for bare in (True, False):
                d = [
                    r.first[t] - r.eos
                    for r in by["wake"]
                    if r.bare == bare and r.first[t] is not None and r.eos is not None
                ]
                row += [_p50(d), str(len(d))]
            lines.append(f"| {t:g} | {' | '.join(row)} |")
        lines.append("")
    lines += [
        "## Per take",
        "",
        "| Take | Label | Line | Transcript | Max score | "
        + " | ".join(f"≥{t:g} at" for t in THRESHOLDS)
        + " | "
        + " | ".join(f"Two-stage {t:g}" for t in TWO_STAGE)
        + " |",
        "|" + "---|" * (5 + len(THRESHOLDS) + len(TWO_STAGE)),
    ]
    for r in results:
        at = [f"{r.first[t]:.2f} s" if r.first[t] is not None else "" for t in THRESHOLDS]
        two = ["yes" if r.confirmed[t] else "" for t in TWO_STAGE]
        lines.append(
            f"| {Path(r.take['file']).name.removesuffix('.aec.wav')} | {r.take.get('label', '')} "
            f"| {r.take.get('text', '')} | {'wake' if r.transcript_woke else ''} "
            f"| {r.max_score:.2f} | {' | '.join(at)} | {' | '.join(two)} |"
        )
    lines += [
        "",
        f"Scored {dt.date.today().isoformat()} with `{model_path}`; end of speech by "
        f"{eos_method}. Transcripts are not kept.",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description="Score wake takes (docs/wake-recording.md).")
    ap.add_argument("--takes", type=Path, help="session directory (default: the newest)")
    ap.add_argument(
        "--model",
        type=Path,
        help="detector model (default: the first that exists of "
        + ", ".join(str(p).replace(str(Path.home()), "~") for p in MODEL_CANDIDATES)
        + ")",
    )
    ap.add_argument("--show-text", action="store_true", help="print transcripts (terminal only)")
    a = ap.parse_args()

    session = a.takes.expanduser() if a.takes else newest_session()
    if session is None:
        raise SystemExit(f"no takes under {TAKES_ROOT}; record some with `make record-takes`")
    if not (session / "takes.jsonl").is_file():
        raise SystemExit(f"{session}: no takes.jsonl")
    takes = load_takes(session)
    if not takes:
        raise SystemExit(f"{session}: no recorded takes")
    model_path = a.model.expanduser() if a.model else default_model()
    if model_path is None or not model_path.is_file():
        raise SystemExit(
            f"no detector model at {model_path or ', '.join(map(str, MODEL_CANDIDATES))}"
        )

    from loguru import logger

    logger.remove()
    logger.add(sys.stderr, level="WARNING")
    from livekit.wakeword import WakeWordModel

    print(f"{len(takes)} takes in {session}; model {model_path}")
    detector = WakeWordModel(models=[model_path])
    stt = Parakeet()
    eos = EndOfSpeech()
    results = []
    started = time.monotonic()
    for take in takes:
        r = score_take(session / take["file"], take, stt, detector, eos)
        results.append(r)
        fires = " ".join(f"{t:g}@{r.first[t]:.2f}" for t in THRESHOLDS if r.first[t] is not None)
        two = "/".join("yes" if r.confirmed[t] else "no" for t in TWO_STAGE)
        print(
            f"  {Path(take['file']).name.removesuffix('.aec.wav'):<16} "
            f"transcript {'WAKE' if r.transcript_woke else '----'}  max {r.max_score:.2f}  "
            f"two-stage {two}  {fires}"
        )
        if a.show_text:
            for where, text in r.texts:
                print(f"      {where}: {text!r}")
    text = report(session, model_path, results, eos.method)
    out = session / f"eval-{model_path.stem}.md"
    out.write_text(text)
    print(f"\n{text}")
    print(f"Wrote {out} ({time.monotonic() - started:.0f} s)")


if __name__ == "__main__":
    main()
