#!/usr/bin/env python3
"""Record the operator's hear takes: the test set `make perf-hear` scores (issue #60, #52).

    make record-hear [BLOCKS=A,B] [ARGS=--dry-run]
    uv run --project tools/oracle python -I scripts/record-hear-takes.py \
        [--blocks A,B] [--session DIR] [--retry-skipped] [--dry-run]

A guided recorder, like the wake one (scripts/record-wake-takes.py, whose capture code this
reuses). The lines are docs/benchmarks/hear-plan.json, the same list the harness renders
with Kokoro. Each block says once where to be and how to speak; each take shows the exact
line, counts down 3-2-1 and records it from ghost.mic (what Ghost hears) and from the raw
mic array under it, at the same time. Nothing is played. Takes go to
.scratch/hear-takes/<date>/ (git-ignored) with a takes.jsonl manifest that carries each
take's line, terms and tags, and a rerun resumes where the last one stopped. The how-to is
docs/hear-recording.md.

The plan, the names, the lengths and the manifest are plain functions so they can be
tested without a microphone (tools/tests/test_record_hear_takes.py); the capture code is
loaded only when recording.
"""

from __future__ import annotations

import argparse
import datetime as dt
import importlib.util
import json
import math
import os
import signal
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType

REPO = Path(__file__).resolve().parents[1]
PLAN = REPO / "docs" / "benchmarks" / "hear-plan.json"
WAKE_RECORDER = REPO / "scripts" / "record-wake-takes.py"
TAKES_ROOT = REPO / ".scratch" / "hear-takes"
MANIFEST = "takes.jsonl"
PAUSE = " ... "

COUNTDOWN = 3
WALK_SECS = 5  # extra countdown when the operator walks away from the keyboard
SECS_REPLY = 3.0
SECS_MIN = 3.5
SECS_PER_WORD = 0.42  # a relaxed request, with room to start late
SECS_PER_PAUSE = 1.4
SECS_SLACK = 2.0

PRIVACY = (
    "Privacy: these takes stay on this machine, under .scratch/hear-takes (git-ignored). "
    "They are recorded here, on purpose, and never taken from Ghost's passive audio."
)


@dataclass(frozen=True)
class Condition:
    block: str
    key: str
    title: str
    instruction: str
    away: bool = False
    music: bool = False


@dataclass(frozen=True)
class Take:
    id: str
    cond: Condition
    kind: str
    text: str
    seconds: float
    say: str = ""
    tags: tuple[str, ...] = field(default_factory=tuple)
    terms: tuple[str, ...] = field(default_factory=tuple)

    @property
    def block(self) -> str:
        return self.cond.block

    @property
    def aec_name(self) -> str:
        return f"{self.id}-{self.kind}.aec.wav"

    @property
    def raw_name(self) -> str:
        return f"{self.id}-{self.kind}.raw.wav"

    @property
    def shown(self) -> str:
        """The line as the operator reads it: the pause marked, a silent take's task."""
        if self.kind == "silence":
            return self.say or "(say nothing)"
        return self.text.replace(PAUSE, " ... (pause) ... ")


def take_seconds(kind: str, text: str, seconds: float | None = None) -> float:
    """How long to record: given for a silent take; for a line, its words and pauses."""
    if seconds is not None:
        return float(seconds)
    if kind == "reply":
        return SECS_REPLY
    words = len(text.replace(PAUSE, " ").split())
    pauses = text.count(PAUSE)
    secs = SECS_SLACK + SECS_PER_WORD * words + SECS_PER_PAUSE * pauses
    return max(SECS_MIN, math.ceil(secs * 2) / 2)


def load_plan(text: str | None = None) -> list[Take]:
    data = json.loads(text if text is not None else PLAN.read_text())
    takes: list[Take] = []
    for b in data["blocks"]:
        cond = Condition(
            b["block"],
            b["condition"],
            b["title"],
            b["instruction"],
            bool(b.get("away")),
            bool(b.get("music")),
        )
        for t in b["takes"]:
            takes.append(
                Take(
                    t["id"],
                    cond,
                    t["kind"],
                    t.get("text", ""),
                    take_seconds(t["kind"], t.get("text", ""), t.get("seconds")),
                    t.get("say", ""),
                    tuple(t.get("tags", ())),
                    tuple(t.get("terms", ())),
                )
            )
    return takes


def blocks_of(plan: list[Take]) -> tuple[str, ...]:
    return tuple(dict.fromkeys(t.block for t in plan))


def parse_blocks(spec: str | None, plan: list[Take]) -> tuple[str, ...] | None:
    """``"a, D"`` -> ``("A", "D")``; None or empty -> None (every block)."""
    if not spec or not spec.strip():
        return None
    known = blocks_of(plan)
    out = tuple(b.strip().upper() for b in spec.split(",") if b.strip())
    unknown = [b for b in out if b not in known]
    if unknown:
        raise ValueError(f"unknown block {', '.join(unknown)}; the blocks are {', '.join(known)}")
    return out


def select(plan: list[Take], blocks: Iterable[str] | None) -> list[Take]:
    wanted = set(blocks) if blocks is not None else None
    return [t for t in plan if wanted is None or t.block in wanted]


# --- manifest and resume ----------------------------------------------------------


def load_manifest(path: Path) -> dict[str, dict]:
    """Entries by take id; the last line for a take wins, a torn line is ignored."""
    entries: dict[str, dict] = {}
    if not path.exists():
        return entries
    for line in path.read_text().splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(e, dict) and isinstance(e.get("id"), str):
            entries[e["id"]] = e
    return entries


def save_manifest(path: Path, entries: dict[str, dict]) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text("".join(json.dumps(entries[k]) + "\n" for k in sorted(entries)))
    os.replace(tmp, path)


def pending(
    plan: list[Take], entries: dict[str, dict], *, retry_skipped: bool = False
) -> list[Take]:
    out = []
    for t in plan:
        e = entries.get(t.id)
        if e is None or (retry_skipped and e.get("skipped")):
            out.append(t)
    return out


def _now() -> str:
    return dt.datetime.now().astimezone().isoformat(timespec="seconds")


def entry(take: Take, *, raw: bool, rms: float, peak: float) -> dict:
    return {
        "id": take.id,
        "file": take.aec_name,
        "raw": take.raw_name if raw else None,
        "block": take.block,
        "condition": take.cond.key,
        "kind": take.kind,
        "text": take.text,
        "tags": list(take.tags),
        "terms": list(take.terms),
        "seconds": take.seconds,
        "rms": round(rms, 1),
        "peak": round(peak, 1),
        "time": _now(),
    }


def skipped_entry(take: Take) -> dict:
    return {
        "id": take.id,
        "file": None,
        "raw": None,
        "block": take.block,
        "condition": take.cond.key,
        "kind": take.kind,
        "text": take.text,
        "tags": list(take.tags),
        "terms": list(take.terms),
        "seconds": take.seconds,
        "skipped": True,
        "time": _now(),
    }


def default_session(today: dt.date | None = None) -> Path:
    return TAKES_ROOT / (today or dt.date.today()).isoformat()


def countdown_secs(take: Take) -> int:
    return COUNTDOWN + (WALK_SECS if take.cond.away else 0)


def plan_minutes(plan: list[Take]) -> float:
    return sum(t.seconds + countdown_secs(t) + 2 for t in plan) / 60


# --- recording (the wake recorder's capture, loaded on demand) ------------------------


def wake_recorder() -> ModuleType:
    """scripts/record-wake-takes.py as a module: its parecord capture, levels and keys."""
    spec = importlib.util.spec_from_file_location("record_wake_takes", WAKE_RECORDER)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod  # its dataclasses look their module up
    spec.loader.exec_module(mod)
    return mod


def record_take(w: ModuleType, take: Take, raw: str | None) -> tuple[bytes, bytes | None] | None:
    """Count down, record, and return (ghost.mic, raw) PCM; None if ghost.mic failed."""
    import time

    caps = [w.Capture(w.AEC_DEVICE, 1)]
    if raw:
        caps.append(w.Capture(raw, w.RAW_CHANNELS))
    try:
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline and not all(c.started() for c in caps):
            time.sleep(0.02)
        for i in range(countdown_secs(take), 0, -1):
            print(f"\r    {i}...  ", end="", flush=True)
            time.sleep(1)
        cue = "say nothing" if take.kind == "silence" else "speak now"
        print(f"\r    {cue} ({take.seconds:g} s)", end="", flush=True)
        time.sleep(take.seconds)
        print("\r    done                       ")
    finally:
        results = [c.stop() for c in caps]
    want = int(take.seconds * w.RATE)
    aec_data, aec_err = results[0]
    aec = w.last_seconds(aec_data, take.seconds, 1)
    if len(aec) // 2 < 0.8 * want:
        print(f"  ghost.mic gave {len(aec) / 2 / w.RATE:.1f} s of audio, not {take.seconds:g} s.")
        if aec_err:
            print(f"  parecord: {aec_err.splitlines()[-1]}")
        return None
    raw_pcm = None
    if raw:
        raw_data, raw_err = results[1]
        raw_pcm = w.last_seconds(raw_data, take.seconds, w.RAW_CHANNELS)
        if len(raw_pcm) // (2 * w.RAW_CHANNELS) < 0.8 * want:
            print(f"  the raw mic ({raw}) gave too little audio; this take has no .raw.wav.")
            if raw_err:
                print(f"  parecord: {raw_err.splitlines()[-1]}")
            raw_pcm = None
    return aec, raw_pcm


def record_and_save(
    w: ModuleType, take: Take, session: Path, raw: str | None, entries: dict[str, dict]
) -> bool:
    got = record_take(w, take, raw)
    if got is None:
        return False
    aec, raw_pcm = got
    w.write_wav(session / take.aec_name, aec, 1)
    if raw_pcm is not None:
        w.write_wav(session / take.raw_name, raw_pcm, w.RAW_CHANNELS)
    else:
        (session / take.raw_name).unlink(missing_ok=True)  # a redo must not keep an old one
    rms, peak = w.levels(aec)
    entries[take.id] = entry(take, raw=raw_pcm is not None, rms=rms, peak=peak)
    save_manifest(session / MANIFEST, entries)
    print(f"  ghost.mic level: rms {rms:.1f} dBFS, peak {peak:.1f} dBFS")
    # a silent take is meant to be quiet; only clipping is worth a word there
    if take.kind != "silence" and (warning := w.level_warning(peak)):
        print(f"  warning: {warning}")
    elif take.kind == "silence" and peak >= w.CLIP_PEAK_DBFS:
        print("  warning: clipping in a silent take: was something loud near the mic?")
    return True


def show_condition(cond: Condition) -> None:
    print(f"\n== Block {cond.block}: {cond.title} ==")
    print(f"   {cond.instruction}")


def show_take(take: Take, number: int, total: int) -> None:
    head = f"take {take.id} ({number} of {total}, {take.kind}, {take.seconds:g} s)"
    print(f'\n  {head}:  "{take.shown}"')


def run_session(
    w: ModuleType,
    plan: list[Take],
    todo: list[Take],
    entries: dict[str, dict],
    session: Path,
    raw: str | None,
) -> None:
    by_id = {t.id: t for t in load_plan()}
    done = [i for i in entries if i in by_id]
    last: Take | None = by_id[max(done)] if done else None
    shown: Condition | None = None
    i = 0
    while i < len(todo):
        take = todo[i]
        if take.cond != shown:
            show_condition(take.cond)
            shown = take.cond
        number = sum(1 for t in plan if t.id in entries) + 1
        show_take(take, number, len(plan))
        key = w.read_key("  Enter: record it   r: redo the last one   s: skip   q: quit  ")
        if key == "q":
            return
        if key == "s":
            entries[take.id] = skipped_entry(take)
            save_manifest(session / MANIFEST, entries)
            print("  skipped")
            last = take
            i += 1
        elif key == "r":
            if last is None:
                print("  nothing to redo yet")
                continue
            print(
                f'\n  redo take {last.id} (block {last.block}, {last.cond.title}):  "{last.shown}"'
            )
            if record_and_save(w, last, session, raw, entries) and last.cond != take.cond:
                shown = None
        elif record_and_save(w, take, session, raw, entries):
            last = take
            i += 1


def ghost_running() -> str | None:
    """A Ghost that would hear the takes and answer: the service, `make dev`, or the app."""
    try:
        if (
            subprocess.run(
                ["systemctl", "--user", "is-active", "ghostd"], capture_output=True, text=True
            ).stdout.strip()
            == "active"
        ):
            return "service"
        if subprocess.run(["pgrep", "-f", r"ghostd run( |$)"], capture_output=True).returncode == 0:
            return "process"
    except OSError:
        return None
    return None


def print_plan(plan: list[Take], entries: dict[str, dict], todo: list[Take], session: Path) -> None:
    print(f"\nSession: {session}")
    done = sum(1 for t in plan if t.id in entries)
    print(
        f"Done: {done} of {len(plan)}"
        + (f"; next is take {todo[0].id}" if todo else "; nothing left")
    )
    shown: Condition | None = None
    for t in plan:
        if t.cond != shown:
            show_condition(t.cond)
            shown = t.cond
        e = entries.get(t.id)
        state = "" if e is None else (" skipped" if e.get("skipped") else " done")
        print(f'  {t.id}  {t.kind:<7} {t.seconds:>4g} s  "{t.shown}"{state}')
    kinds = {k: sum(1 for t in plan if t.kind == k) for k in ("speech", "reply", "silence")}
    print(
        f"\n{len(plan)} takes: "
        + ", ".join(f"{n} {k}" for k, n in kinds.items() if n)
        + f"; about {plan_minutes(plan):.0f} min of countdowns and takes"
    )


def main(argv: list[str] | None = None) -> int:
    full = load_plan()
    ap = argparse.ArgumentParser(description="Record the hear takes (docs/hear-recording.md).")
    ap.add_argument(
        "--blocks", help=f"record only these blocks, e.g. A,D (all: {','.join(blocks_of(full))})"
    )
    ap.add_argument(
        "--session", type=Path, help="session directory (default .scratch/hear-takes/<today>)"
    )
    ap.add_argument("--retry-skipped", action="store_true", help="ask again for skipped takes")
    ap.add_argument("--dry-run", action="store_true", help="print the plan; record nothing")
    a = ap.parse_args(argv)
    try:
        blocks = parse_blocks(a.blocks, full)
    except ValueError as e:
        ap.error(str(e))
    plan = select(full, blocks)
    session = (a.session or default_session()).expanduser().resolve()
    entries = load_manifest(session / MANIFEST)
    todo = pending(plan, entries, retry_skipped=a.retry_skipped)
    print(PRIVACY)
    if a.dry_run:
        print_plan(plan, entries, todo, session)
        return 0
    if not todo:
        print(f"Every take is done in {session}.")
        print("Score them with: make perf-hear TAKES=latest")
        return 0
    w = wake_recorder()
    if not w.service_active("ghost-echo-cancel"):
        print("ghost-echo-cancel isn't running, so ghost.mic doesn't exist; nothing to record.")
        print("Start it with `systemctl --user start ghost-echo-cancel`.")
        return 1
    raw, raw_from = w.raw_device()
    print(f"Records: {w.AEC_DEVICE} (mono, {w.RATE} Hz)")
    print(f"    and: {raw or 'no raw mic; ghost.mic only'} (from {raw_from})")

    def interrupted(signum, frame):  # noqa: ARG001 - a closed terminal still restarts Ghost
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    stopped = False
    try:
        running = ghost_running()
        if running == "service":
            print("Ghost (ghostd) is running and would hear these takes and answer.")
            if w.ask_yes("Stop it for this session? It starts again when you finish. [Y/n] "):
                subprocess.run(["systemctl", "--user", "stop", "ghostd"], check=True)
                stopped = True
                print("ghostd stopped.")
        elif running == "process":
            print("A ghostd is running (`make dev`, `make start` or the desktop app) and would")
            print("hear these takes. Stop it (`make stop`, or quit the app) and run this again.")
            return 1
        session.mkdir(parents=True, exist_ok=True)
        done = len(plan) - len(todo)
        if done:
            print(f"Resuming {session}: {done} of {len(plan)} done, next is take {todo[0].id}.")
        else:
            print(f"Recording into {session}.")
        run_session(w, plan, todo, entries, session, raw)
    except KeyboardInterrupt:
        print("\nStopped. The takes so far are saved; run it again to carry on.")
        return 130
    finally:
        if stopped:
            subprocess.run(["systemctl", "--user", "start", "ghostd"], check=False)
            print("ghostd started again.")
    recorded = sum(1 for t in plan if entries.get(t.id, {}).get("file"))
    skipped = sum(1 for t in plan if entries.get(t.id, {}).get("skipped"))
    print(
        f"\n{recorded} of {len(plan)} takes recorded in {session}"
        + (f", {skipped} skipped." if skipped else ".")
    )
    print("Score them with: make perf-hear TAKES=latest")
    return 0


if __name__ == "__main__":
    sys.exit(main())
