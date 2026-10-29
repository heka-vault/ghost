#!/usr/bin/env python3
"""Record the operator's own wake takes: the wake test set (ADR 0025, issue #8).

    make record-takes [BLOCKS=A,C,F]
    uv run --project tools/oracle python -I scripts/record-wake-takes.py \
        [--blocks A,C,F] [--session DIR] [--dry-run]

A guided recorder. Each block says once where to stand and how to speak; each take shows
the exact line, counts down 3-2-1 on screen and records it from ghost.mic (what Ghost
hears) and from the raw mic array ghost.mic is built on, at the same time. Nothing is
played: no beeps, nothing through the speakers. Takes go to .scratch/wake-takes/<date>/
(git-ignored) with a takes.jsonl manifest, and a rerun resumes where the last one stopped.
Score them with `make wake-takes-eval`. The how-to is docs/wake-recording.md.

The plan, the names and the manifest are plain functions (``build_plan``, ``Take.stem``,
``pending``, ``capture_target``, ``levels``) so they can be tested without a microphone.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import re
import signal
import subprocess
import sys
import threading
import time
import wave
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

import numpy as np

RATE = 16000
AEC_DEVICE = "ghost.mic"  # Ghost's own echo-cancelled source (infra/pipewire), not hardware
RAW_CHANNELS = 2
MANIFEST = "takes.jsonl"
REPO = Path(__file__).resolve().parents[1]
TAKES_ROOT = REPO / ".scratch" / "wake-takes"
ECHO_CANCEL_CONF = Path.home() / ".config" / "ghost" / "echo-cancel.conf"

SECS_BARE = 3.0
SECS_REQUEST = 5.0
SECS_NEGATIVE = 4.0
COUNTDOWN = 3
WALK_SECS = 5  # extra countdown when the operator has to walk away from the keyboard
QUIET_PEAK_DBFS = -35.0
CLIP_PEAK_DBFS = -0.5

WAKE = "Hey Ghost."
REQUESTS = (
    "Hey Ghost, what needs my sign-off?",
    "Hey Ghost, any new issues today?",
    "Hey Ghost, read me the latest bug.",
    "Hey Ghost, what's on my plate?",
)
NEGATIVES = (
    "Hey coach.",
    "Hey Gus, how have you been?",
    "He goes to the gym every morning.",
    "Hey guys, what's up?",
    "Hey, post that.",
    "Hey, gross.",
    "The ghost of Christmas past.",
    "Okay, coast is clear.",
)
LABELS = ("wake", "negative", "other")

PRIVACY = (
    "Privacy: these takes stay on this machine, under .scratch/wake-takes (git-ignored). "
    "They are recorded here, on purpose, and never taken from Ghost's passive audio."
)


@dataclass(frozen=True)
class Condition:
    """Where the operator is and how they speak, shown once when it starts."""

    block: str
    key: str  # the manifest's ``condition``
    title: str
    instruction: str
    away: bool = False  # away from the keyboard: the countdown gets WALK_SECS more


@dataclass(frozen=True)
class Take:
    index: int  # 1-based over the whole plan, so a --blocks subset keeps its names
    cond: Condition
    label: str
    text: str
    seconds: float

    @property
    def block(self) -> str:
        return self.cond.block

    @property
    def stem(self) -> str:
        return f"{self.index:03d}-{self.block}-{self.label}"

    @property
    def aec_name(self) -> str:
        return f"{self.stem}.aec.wav"

    @property
    def raw_name(self) -> str:
        return f"{self.stem}.raw.wav"


def _rotate(lines: tuple[str, ...], n: int, start: int = 0) -> list[str]:
    return [lines[(start + i) % len(lines)] for i in range(n)]


def _bare(n: int) -> list[tuple[str, str, float]]:
    return [("wake", WAKE, SECS_BARE)] * n


def _requests(n: int) -> list[tuple[str, str, float]]:
    return [("wake", t, SECS_REQUEST) for t in _rotate(REQUESTS, n)]


def _segments() -> list[tuple[Condition, list[tuple[str, str, float]]]]:
    negatives = [("negative", t, SECS_NEGATIVE) for t in _rotate(NEGATIVES, 16)]
    return [
        (
            Condition(
                "A",
                "desk",
                "At the desk, normal voice",
                "Sit where you usually sit and speak the way you would talk to Ghost on an "
                "ordinary day.",
            ),
            _bare(10) + _requests(5),
        ),
        (
            Condition(
                "B",
                "desk-soft",
                "At the desk, soft or tired voice",
                "Still at the desk. Speak softly, the way you might at the end of a long "
                "day. Not a whisper.",
            ),
            _bare(5),
        ),
        (
            Condition(
                "C",
                "across-room",
                "Across the room, 3 m or more",
                "Press Enter, then walk to a spot 3 m or more from the mic and face it. "
                f"The countdown runs {COUNTDOWN + WALK_SECS} s so you have time; count it "
                "in your head if you can't read the screen. Normal voice, no shouting.",
                away=True,
            ),
            _bare(8) + _requests(4),
        ),
        (
            Condition(
                "D",
                "turned-away",
                "At the desk, turned away from the mic",
                "Stay near the desk but turn so the mic is behind you or to your side. "
                "Normal voice.",
            ),
            _bare(6),
        ),
        (
            Condition(
                "E",
                "music",
                "At the desk, with music playing",
                "Start some music yourself on the desk speakers, at the level you usually "
                "listen at (this script plays nothing). Stay at the desk, normal voice. "
                "Stop the music when the block ends.",
            ),
            _bare(8) + _requests(4),
        ),
        (
            Condition(
                "F",
                "desk",
                "Near misses, at the desk",
                "Music off. These lines sound a little like the wake phrase but are not it; "
                "Ghost should not wake on any of them. Say them naturally, as if to "
                "someone in the room.",
            ),
            negatives[:12],
        ),
        (
            Condition(
                "F",
                "across-room",
                "Near misses, across the room",
                "The same kind of lines from 3 m or more away. Press Enter, then walk "
                f"over; the countdown runs {COUNTDOWN + WALK_SECS} s.",
                away=True,
            ),
            negatives[12:],
        ),
        (
            Condition(
                "G",
                "desk",
                "Optional: other wake phrases",
                "Other phrases Ghost accepts today. These are optional: press q to finish "
                "here if you'd rather not.",
            ),
            [("other", "Okay Ghost.", SECS_BARE)] * 3
            + [("other", "Ghost, are you there?", SECS_NEGATIVE)] * 3,
        ),
    ]


BLOCKS = tuple(dict.fromkeys(c.block for c, _ in _segments()))


def parse_blocks(spec: str | None) -> tuple[str, ...] | None:
    """``"A,c, F"`` -> ``("A", "C", "F")``; None or empty -> None (every block)."""
    if not spec or not spec.strip():
        return None
    out = tuple(b.strip().upper() for b in spec.split(",") if b.strip())
    unknown = [b for b in out if b not in BLOCKS]
    if unknown:
        raise ValueError(f"unknown block {', '.join(unknown)}; the blocks are {', '.join(BLOCKS)}")
    return out


def build_plan(blocks: Iterable[str] | None = None) -> list[Take]:
    """Every take in order, numbered over the whole plan, then filtered to ``blocks``."""
    wanted = set(blocks) if blocks is not None else None
    plan: list[Take] = []
    index = 0
    for cond, takes in _segments():
        for label, text, seconds in takes:
            index += 1
            if wanted is None or cond.block in wanted:
                plan.append(Take(index, cond, label, text, seconds))
    return plan


# --- manifest and resume ----------------------------------------------------------


def load_manifest(path: Path) -> dict[int, dict]:
    """Entries by take number; the last line for a take wins, a torn line is ignored."""
    entries: dict[int, dict] = {}
    if not path.exists():
        return entries
    for line in path.read_text().splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(e, dict) and isinstance(e.get("take"), int):
            entries[e["take"]] = e
    return entries


def save_manifest(path: Path, entries: dict[int, dict]) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text("".join(json.dumps(entries[k]) + "\n" for k in sorted(entries)))
    os.replace(tmp, path)


def pending(
    plan: list[Take], entries: dict[int, dict], *, retry_skipped: bool = False
) -> list[Take]:
    """The takes still to record: not done, and not skipped unless ``retry_skipped``."""
    out = []
    for t in plan:
        e = entries.get(t.index)
        if e is None or (retry_skipped and e.get("skipped")):
            out.append(t)
    return out


def entry(take: Take, *, raw: bool, rms: float, peak: float) -> dict:
    return {
        "take": take.index,
        "file": take.aec_name,
        "raw": take.raw_name if raw else None,
        "block": take.block,
        "condition": take.cond.key,
        "label": take.label,
        "text": take.text,
        "seconds": take.seconds,
        "rms": round(rms, 1),
        "peak": round(peak, 1),
        "time": dt.datetime.now().astimezone().isoformat(timespec="seconds"),
    }


def skipped_entry(take: Take) -> dict:
    return {
        "take": take.index,
        "file": None,
        "raw": None,
        "block": take.block,
        "condition": take.cond.key,
        "label": take.label,
        "text": take.text,
        "seconds": take.seconds,
        "skipped": True,
        "time": dt.datetime.now().astimezone().isoformat(timespec="seconds"),
    }


def default_session(today: dt.date | None = None) -> Path:
    return TAKES_ROOT / (today or dt.date.today()).isoformat()


# --- devices, levels ----------------------------------------------------------------


def capture_target(conf: str) -> str | None:
    """The ``node.target`` inside ``capture.props`` of the echo-cancel config: the mic
    ghost.mic is built on. The playback block has a ``node.target`` too (the speakers)."""
    text = re.sub(r"#[^\n]*", "", conf)
    block = re.search(r"capture\.props\s*=\s*\{([^}]*)\}", text)
    if not block:
        return None
    m = re.search(r'node\.target\s*=\s*"?([^"\s}]+)"?', block.group(1))
    return m.group(1) if m else None


def raw_device() -> tuple[str | None, str]:
    """The raw mic and where it came from: GHOST_RAW_MIC, else the echo-cancel config."""
    env = os.environ.get("GHOST_RAW_MIC", "").strip()
    if env:
        return env, "GHOST_RAW_MIC"
    try:
        found = capture_target(ECHO_CANCEL_CONF.read_text())
    except OSError:
        found = None
    if found:
        return found, str(ECHO_CANCEL_CONF).replace(str(Path.home()), "~")
    return None, "not found (no GHOST_RAW_MIC, no capture target in the echo-cancel config)"


def levels(pcm: bytes) -> tuple[float, float]:
    """RMS and peak of s16le audio, in dBFS (floor -120)."""
    x = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64) / 32768.0
    if not x.size:
        return -120.0, -120.0
    rms = math.sqrt(float(np.mean(x * x)))
    peak = float(np.max(np.abs(x)))
    return (
        max(-120.0, 20 * math.log10(max(rms, 1e-12))),
        max(-120.0, 20 * math.log10(max(peak, 1e-12))),
    )


def level_warning(peak: float) -> str | None:
    if peak < QUIET_PEAK_DBFS:
        return f"too quiet (peak below {QUIET_PEAK_DBFS:g} dBFS): is the mic muted or unplugged?"
    if peak >= CLIP_PEAK_DBFS:
        return "clipping (peak at full scale): back off a little, or turn the mic gain down"
    return None


def last_seconds(data: bytes, seconds: float, channels: int) -> bytes:
    """The last ``seconds`` of an s16le stream, cut on whole frames."""
    frame = 2 * channels
    data = data[: len(data) // frame * frame]
    n = int(round(seconds * RATE)) * frame
    return data[-n:] if len(data) > n else data


# --- recording ----------------------------------------------------------------------


class Capture:
    """One ``parecord --raw`` stream, read on a thread until stopped.

    parecord's own WAV writer, stopped by a signal, left empty headers; raw PCM is read
    here and the WAV is written with the ``wave`` module.
    """

    def __init__(self, device: str, channels: int) -> None:
        self.device = device
        self.channels = channels
        self._buf = bytearray()
        self._lock = threading.Lock()
        self._proc = subprocess.Popen(
            [
                "parecord",
                "--raw",
                f"--device={device}",
                f"--rate={RATE}",
                f"--channels={channels}",
                "--format=s16le",
                "--latency-msec=20",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self._thread = threading.Thread(target=self._pump, daemon=True)
        self._thread.start()

    def _pump(self) -> None:
        assert self._proc.stdout is not None
        fd = self._proc.stdout.fileno()
        while chunk := os.read(fd, 65536):
            with self._lock:
                self._buf += chunk

    def started(self) -> bool:
        with self._lock:
            return bool(self._buf) or self._proc.poll() is not None

    def stop(self) -> tuple[bytes, str]:
        """The audio so far and parecord's error output, if it had any."""
        if self._proc.poll() is None:
            self._proc.terminate()
        try:
            self._proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            self._proc.wait()
        self._thread.join(timeout=2)
        err = self._proc.stderr.read().decode(errors="replace").strip() if self._proc.stderr else ""
        with self._lock:
            return bytes(self._buf), err


def write_wav(path: Path, pcm: bytes, channels: int) -> None:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(channels)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(pcm)


def record_take(take: Take, raw: str | None) -> tuple[bytes, bytes | None] | None:
    """Count down, record, and return (ghost.mic, raw) PCM; None if ghost.mic failed."""
    caps = [Capture(AEC_DEVICE, 1)]
    if raw:
        caps.append(Capture(raw, RAW_CHANNELS))
    results: list[tuple[bytes, str]] = []
    try:
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline and not all(c.started() for c in caps):
            time.sleep(0.02)
        count = COUNTDOWN + (WALK_SECS if take.cond.away else 0)
        for i in range(count, 0, -1):
            print(f"\r    {i}...  ", end="", flush=True)
            time.sleep(1)
        print(f"\r    speak now ({take.seconds:g} s)", end="", flush=True)
        time.sleep(take.seconds)
        print("\r    done                 ")
    finally:
        results = [c.stop() for c in caps]
    want = int(take.seconds * RATE)
    aec_data, aec_err = results[0]
    aec = last_seconds(aec_data, take.seconds, 1)
    if len(aec) // 2 < 0.8 * want:
        print(f"  ghost.mic gave {len(aec) / 2 / RATE:.1f} s of audio, not {take.seconds:g} s.")
        if aec_err:
            print(f"  parecord: {aec_err.splitlines()[-1]}")
        return None
    raw_pcm = None
    if raw:
        raw_data, raw_err = results[1]
        raw_pcm = last_seconds(raw_data, take.seconds, RAW_CHANNELS)
        if len(raw_pcm) // (2 * RAW_CHANNELS) < 0.8 * want:
            print(f"  the raw mic ({raw}) gave too little audio; this take has no .raw.wav.")
            if raw_err:
                print(f"  parecord: {raw_err.splitlines()[-1]}")
            raw_pcm = None
    return aec, raw_pcm


# --- the session --------------------------------------------------------------------


def read_key(prompt: str) -> str:
    """One of "\\n", "r", "s", "q", without waiting for Enter when stdin is a terminal."""
    print(prompt, end="", flush=True)
    if not sys.stdin.isatty():
        line = sys.stdin.readline()
        print()
        if not line:
            return "q"
        key = line.strip().lower()[:1]
        return key if key in ("r", "s", "q") else "\n"
    import termios
    import tty

    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    try:
        termios.tcflush(fd, termios.TCIFLUSH)  # keys pressed while recording don't count
        tty.setcbreak(fd)
        while True:
            ch = os.read(fd, 1).decode(errors="ignore").lower()
            if ch in ("\n", "\r"):
                key = "\n"
                break
            if ch in ("r", "s", "q"):
                key = ch
                break
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old)
    print("" if key == "\n" else key)
    return key


def show_condition(cond: Condition) -> None:
    print(f"\n== Block {cond.block}: {cond.title} ==")
    print(f"   {cond.instruction}")


def show_take(take: Take, number: int, total: int) -> None:
    print(
        f"\n  take {take.index:03d} ({number} of {total}, {take.label}, {take.seconds:g} s):"
        f'  "{take.text}"'
    )


def record_and_save(take: Take, session: Path, raw: str | None, entries: dict[int, dict]) -> bool:
    got = record_take(take, raw)
    if got is None:
        return False
    aec, raw_pcm = got
    write_wav(session / take.aec_name, aec, 1)
    if raw_pcm is not None:
        write_wav(session / take.raw_name, raw_pcm, RAW_CHANNELS)
    else:
        (session / take.raw_name).unlink(missing_ok=True)  # a redo must not keep an old one
    rms, peak = levels(aec)
    entries[take.index] = entry(take, raw=raw_pcm is not None, rms=rms, peak=peak)
    save_manifest(session / MANIFEST, entries)
    print(f"  ghost.mic level: rms {rms:.1f} dBFS, peak {peak:.1f} dBFS")
    if warning := level_warning(peak):
        print(f"  warning: {warning}")
    return True


def run_session(
    plan: list[Take], todo: list[Take], entries: dict[int, dict], session: Path, raw: str | None
) -> None:
    by_index = {t.index: t for t in build_plan()}
    total = len(plan)
    done = [i for i in entries if i in by_index]
    last: Take | None = by_index[max(done)] if done else None
    shown: Condition | None = None
    i = 0
    while i < len(todo):
        take = todo[i]
        if take.cond != shown:
            show_condition(take.cond)
            shown = take.cond
        number = sum(1 for t in plan if t.index in entries) + 1
        show_take(take, number, total)
        key = read_key("  Enter: record it   r: redo the last one   s: skip   q: quit  ")
        if key == "q":
            return
        if key == "s":
            entries[take.index] = skipped_entry(take)
            save_manifest(session / MANIFEST, entries)
            print("  skipped")
            last = take
            i += 1
        elif key == "r":
            if last is None:
                print("  nothing to redo yet")
                continue
            print(
                f"\n  redo take {last.index:03d} (block {last.block}, {last.cond.title}):"
                f'  "{last.text}"'
            )
            if record_and_save(last, session, raw, entries) and last.cond != take.cond:
                shown = None  # back to the current block: show its instruction again
        elif record_and_save(take, session, raw, entries):
            last = take
            i += 1
        # a failed take stays next; the prompt comes back for it


def service_active(unit: str) -> bool:
    try:
        out = subprocess.run(
            ["systemctl", "--user", "is-active", unit], capture_output=True, text=True, timeout=10
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return out.stdout.strip() == "active"


def foreground_ghostd() -> bool:
    """A ghostd from `make dev`, `make daemon` or `make start`: this script can't stop it."""
    try:
        return (
            subprocess.run(["pgrep", "-f", "bin/ghostd run$"], capture_output=True).returncode == 0
        )
    except OSError:
        return False


def ask_yes(prompt: str) -> bool:
    try:
        answer = input(prompt)
    except EOFError:
        return False
    return answer.strip().lower() in ("", "y", "yes")


def print_plan(
    plan: list[Take],
    entries: dict[int, dict],
    todo: list[Take],
    session: Path,
    raw: str | None,
    raw_from: str,
) -> None:
    print(f"\nSession: {session}")
    done = sum(1 for t in plan if t.index in entries)
    print(
        f"Done: {done} of {len(plan)}"
        + (f"; next is take {todo[0].index:03d}" if todo else "; nothing left")
    )
    print(f"Records: {AEC_DEVICE} (mono, {RATE} Hz)")
    print(
        f"     and: {raw or 'no raw mic, skipped'} ({RAW_CHANNELS} ch, {RATE} Hz; from {raw_from})"
    )
    print(f"ghost-echo-cancel: {'active' if service_active('ghost-echo-cancel') else 'NOT active'}")
    ghostd = "active (the recorder offers to stop it)" if service_active("ghostd") else "not active"
    print(f"ghostd: {ghostd}")
    shown: Condition | None = None
    for t in plan:
        if t.cond != shown:
            show_condition(t.cond)
            shown = t.cond
        state = (
            " done"
            if t.index in entries and not entries[t.index].get("skipped")
            else (" skipped" if t.index in entries else "")
        )
        print(
            f'  {t.index:03d}  {t.label:<8} {t.seconds:g} s  "{t.text}"'
            f"  -> {t.aec_name}, {t.raw_name}{state}"
        )
    counts = {label: sum(1 for t in plan if t.label == label) for label in LABELS}
    secs = sum(t.seconds + COUNTDOWN + (WALK_SECS if t.cond.away else 0) for t in plan)
    print(
        f"\n{len(plan)} takes: "
        + ", ".join(f"{n} {k}" for k, n in counts.items() if n)
        + f"; about {secs / 60:.0f} min of countdowns and takes"
    )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description="Record the operator's wake takes (docs/wake-recording.md)."
    )
    ap.add_argument(
        "--blocks", help=f"record only these blocks, e.g. A,C,F (all: {','.join(BLOCKS)})"
    )
    ap.add_argument(
        "--session", type=Path, help="session directory (default .scratch/wake-takes/<today>)"
    )
    ap.add_argument(
        "--retry-skipped", action="store_true", help="ask again for takes skipped before"
    )
    ap.add_argument(
        "--dry-run", action="store_true", help="print the plan and the file names; record nothing"
    )
    a = ap.parse_args(argv)
    try:
        blocks = parse_blocks(a.blocks)
    except ValueError as e:
        ap.error(str(e))
    plan = build_plan(blocks)
    session = (a.session or default_session()).expanduser().resolve()
    entries = load_manifest(session / MANIFEST)
    todo = pending(plan, entries, retry_skipped=a.retry_skipped)
    raw, raw_from = raw_device()
    print(PRIVACY)
    if a.dry_run:
        print_plan(plan, entries, todo, session, raw, raw_from)
        return 0
    if not todo:
        print(
            f"Every take in {'blocks ' + ','.join(blocks) if blocks else 'the plan'}"
            f" is done in {session}."
        )
        print("Score them with: make wake-takes-eval")
        return 0
    if not service_active("ghost-echo-cancel"):
        print(
            "ghost-echo-cancel isn't running, so ghost.mic doesn't exist"
            " and there is nothing to record."
        )
        print(
            "Start it with `systemctl --user start ghost-echo-cancel`"
            " (or install it with `make echo-cancel`)."
        )
        return 1
    if raw:
        print(f"Raw mic: {raw} (from {raw_from})")
    else:
        print(f"Raw mic: {raw_from}; recording ghost.mic only.")

    def interrupted(signum, frame):  # noqa: ARG001 - a closed terminal or a kill still restarts Ghost
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    stopped = False
    try:
        if service_active("ghostd"):
            print("Ghost (ghostd) is running and would wake on these takes.")
            if ask_yes("Stop it for this session? It starts again when you finish. [Y/n] "):
                subprocess.run(["systemctl", "--user", "stop", "ghostd"], check=True)
                stopped = True
                print("ghostd stopped.")
            else:
                print("Carrying on with Ghost running; it may wake and answer during the takes.")
        elif foreground_ghostd():
            print(
                "A ghostd from `make dev`, `make daemon` or `make start` is running "
                "and would wake on the takes."
            )
            print("Stop it with `make stop` in another shell before you record.")
        session.mkdir(parents=True, exist_ok=True)
        done = len(plan) - len(todo)
        if done:
            print(
                f"Resuming {session}: {done} of {len(plan)} done, next is take {todo[0].index:03d}."
            )
        else:
            print(f"Recording into {session}.")
        run_session(plan, todo, entries, session, raw)
    except KeyboardInterrupt:
        print("\nStopped. The takes so far are saved; run it again to carry on.")
        return 130
    finally:
        if stopped:
            subprocess.run(["systemctl", "--user", "start", "ghostd"], check=False)
            print("ghostd started again.")
    recorded = sum(1 for t in plan if entries.get(t.index, {}).get("file"))
    skipped = sum(1 for t in plan if entries.get(t.index, {}).get("skipped"))
    print(
        f"\n{recorded} of {len(plan)} takes recorded in {session}"
        + (f", {skipped} skipped." if skipped else ".")
    )
    print("Score them with: make wake-takes-eval")
    return 0


if __name__ == "__main__":
    sys.exit(main())
