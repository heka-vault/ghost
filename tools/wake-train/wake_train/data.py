"""What the retraining reads, and how it splits it (ADR 0064). Standard library only, so the
tools' own tests cover it without the training's dependencies.

Three kinds of audio, all read in place and never copied, sent or rewritten:

- **The labelled streams** of the wake check (``sets.txt``, docs/benchmarks/2026-11-12-wake-
  check.md): the operator's recorded takes of 2026-10-30 with where each "ghost" ends, their
  negatives, and the replay rig's Kokoro programs. ``read_sets`` parses the file.
- **The kept wake takes** (``<data>/wake/takes/<id>.wav`` and ``<id>.json``), the operator's
  own wake attempts kept by Ghost as it ran, once they chose to keep them. Only what the
  operator labelled counts, and an unlabelled accepted wake counts as a positive only when
  they said so (``label_of``).
- **Synthetic audio** made on this machine (``synth``) and the music of ``noise``.

``split`` decides which groups of takes are held out for testing. It hashes each group's
name, so a take held out today is held out tomorrow, when more takes have been kept, and
the numbers of two runs stay comparable.
"""

from __future__ import annotations

import glob
import hashlib
import json
import os
import re
import wave
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path

RATE = 16000

# Where in a kept take the frame that scored ended, when its metadata does not say
# (``ghost_core::wake_takes``: 12 s into an accepted take, 4 s into a refused one).
TAKE_PRE_SECS = 4.0


@dataclass(frozen=True)
class Stream:
    """One labelled recording: ``kind`` is ``neg``, ``pos``, ``count`` or ``program``."""

    path: str
    channel: int
    kind: str
    times: tuple[float, ...] = ()  # pos: where each "ghost" ends
    others: tuple[float, ...] = ()  # pos: other wake phrasings, counted neither way
    count: int = 0
    program: str = ""  # program: the replay rig's clip list

    @property
    def name(self) -> str:
        n = Path(self.path).name
        return f"{n}#{self.channel}" if self.channel else n

    @property
    def group(self) -> str:
        """The recording session a stream belongs to: its file's name without the stream
        suffix, so a take's ``ghost.mic`` and array streams are held out together."""
        stem = Path(self.path).stem
        return re.sub(r"[-.](aec|raw)$", "", stem)


@dataclass
class StreamSet:
    title: str
    streams: list[Stream] = field(default_factory=list)


def expand(text: str, env: dict[str, str]) -> str:
    return re.sub(r"\$([A-Z]+)", lambda m: env.get(m.group(1), m.group(0)), text)


def parse_stream(spec: str) -> Stream:
    """``PATH[#CHANNEL]:LABEL``, as ``wake_check_replay.py`` takes it."""
    path, sep, label = spec.rpartition(":")
    if not sep or not path:
        raise ValueError(f"no label in {spec!r}")
    path, _, ch = path.partition("#")
    channel = int(ch) if ch else 0
    if label == "neg":
        return Stream(path, channel, "neg")
    if label.startswith("pos@"):
        times, others = [], []
        for t in label[4:].split(","):
            t = t.strip()
            if not t:
                continue
            (others if t.startswith("~") else times).append(float(t.lstrip("~")))
        return Stream(path, channel, "pos", tuple(times), tuple(others))
    if label.startswith("count="):
        return Stream(path, channel, "count", count=int(label[6:]))
    if label.startswith("program="):
        return Stream(path, channel, "program", program=label[8:])
    raise ValueError(f"unknown label {label!r} in {spec!r}")


def read_sets(text: str, env: dict[str, str]) -> list[StreamSet]:
    """The set file: ``[title]`` starts a set; each line is a stream; a line starting with
    ``#`` is a comment (a ``#`` later in a line is a channel). A ``*`` in a path is kept as
    it is (``matches`` lists its files, read as one stream)."""
    sets: list[StreamSet] = []
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            sets.append(StreamSet(line[1:-1]))
            continue
        if not sets:
            raise ValueError("a stream before the first [set]")
        sets[-1].streams.append(parse_stream(expand(line, env)))
    return sets


def default_env() -> dict[str, str]:
    home = Path.home()
    return {
        "SCRATCH": os.environ.get("SCRATCH", str(home / "projects/active/ghost/.scratch")),
        "HEAR": os.environ.get("HEAR", str(home / ".local/share/ghost/hear/sessions")),
    }


def matches(path: str) -> list[str]:
    return sorted(glob.glob(path)) if "*" in path else [path]


# --- reading audio ---------------------------------------------------------------------------


def read_wav(path: str | Path, channel: int = 0) -> list[int]:
    """16 kHz 16-bit samples of one channel, as a list (``numpy.asarray`` it)."""
    import array

    with wave.open(str(path)) as w:
        if w.getframerate() != RATE or w.getsampwidth() != 2:
            raise ValueError(f"{path}: 16 kHz 16-bit only")
        n = w.getnchannels()
        a = array.array("h", w.readframes(w.getnframes()))
    if n == 1:
        return a.tolist()
    if channel >= n:
        raise ValueError(f"{path} has {n} channels, not {channel + 1}")
    return a[channel::n].tolist()


def write_wav(path: str | Path, samples: Sequence[int]) -> None:
    import array

    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(array.array("h", samples).tobytes())


# --- the kept takes --------------------------------------------------------------------------


@dataclass(frozen=True)
class KeptTake:
    id: str
    wav: Path
    outcome: str  # accepted | refused
    label: str | None  # me | not | me_missed_before | None
    score: float
    input: str
    at: str
    hit_at: float = TAKE_PRE_SECS  # seconds into the take where the scored frame ended
    array: Path | None = None  # the array's own stream, when kept

    @property
    def group(self) -> str:
        return f"kept:{self.id}"


def read_kept(folder: Path) -> list[KeptTake]:
    """Every take with both its files. A take whose metadata does not read is skipped, never
    guessed at."""
    out: list[KeptTake] = []
    if not folder.is_dir():
        return out
    for meta in sorted(folder.glob("*.json")):
        wav = meta.with_suffix(".wav")
        if not wav.is_file():
            continue
        try:
            m = json.loads(meta.read_text())
            out.append(
                KeptTake(
                    id=str(m["id"]),
                    wav=wav,
                    outcome=str(m["outcome"]),
                    label=m.get("label"),
                    score=float(m.get("score", 0.0)),
                    input=str(m.get("input", "")),
                    at=str(m.get("at", "")),
                    hit_at=float(m.get("hit_at", TAKE_PRE_SECS)),
                    array=meta.with_suffix("").with_suffix(".array.wav")
                    if meta.with_suffix("").with_suffix(".array.wav").is_file()
                    else None,
                )
            )
        except (ValueError, KeyError, TypeError):
            continue
    return out


def label_of(t: KeptTake, unlabelled_accepted_are_positive: bool) -> str | None:
    """``pos``, ``neg`` or None (not used). The operator's label decides; an accepted wake
    with no label is a positive only when the operator said so; a refused hit with no
    label is never used, since nobody confirmed what it was."""
    if t.label in ("me", "me_missed_before"):
        return "pos"
    if t.label == "not":
        return "neg"
    if t.label is None and t.outcome == "accepted" and unlabelled_accepted_are_positive:
        return "pos"
    return None


# --- holding out ------------------------------------------------------------------------------


def bucket(name: str) -> float:
    """A stable number in [0, 1) for a group's name."""
    h = hashlib.sha256(name.encode()).digest()
    return int.from_bytes(h[:8], "big") / 2**64


def split(groups: Iterable[str], share: float) -> tuple[set[str], set[str]]:
    """(train, held out). Each group lands by its hash, so the split of a group never
    changes; at least one group is held out and one trained on when there are two."""
    gs = sorted(set(groups))
    held = {g for g in gs if bucket(g) < share}
    if len(gs) >= 2 and not held:
        held = {min(gs, key=bucket)}
    if len(gs) >= 2 and held == set(gs):
        held.discard(max(gs, key=bucket))
    return set(gs) - held, held


def folds(groups: Iterable[str], k: int, weight: dict[str, int] | None = None) -> list[set[str]]:
    """``k`` folds of whole groups, balanced by weight (takes per group), in hash order."""
    gs = sorted(set(groups), key=bucket)
    w = weight or {}
    out: list[set[str]] = [set() for _ in range(max(1, k))]
    load = [0] * len(out)
    for g in sorted(gs, key=lambda g: (-w.get(g, 1), bucket(g))):
        i = load.index(min(load))
        out[i].add(g)
        load[i] += w.get(g, 1)
    return out
