"""Scoring a detector over labelled streams as the daemon decides (ADR 0025, the 2026-11-12
check): every 80 ms, the first frame at the floor is the hit, and a hit holds the detector
for the 2 s cooldown. A hit at the threshold wakes Ghost at once; a hit under it, in the
checked band, goes to Parakeet (not run here: the counts say how often it would be asked).

A hit within 1.2 s of a labelled "ghost" is that take; any other hit in a stream is a false
one, except near another wake phrasing (``~T``), which counts neither way.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from wake_train.features import STEP

RATE = 16000
COOLDOWN = 2.0
NEAR = 1.2


def hits(sc: np.ndarray, threshold: float, floor: float) -> list[tuple[float, float]]:
    """(seconds, score) of each hit: the first frame at the floor, then the cooldown."""
    out: list[tuple[float, float]] = []
    last = -1e9
    for i in np.flatnonzero(sc >= min(floor, threshold)):
        t = i * STEP / RATE
        if t - last < COOLDOWN:
            continue
        out.append((t, float(sc[i])))
        last = t
    return out


def rule_counts(negs: list[np.ndarray], threshold: float, floor: float) -> tuple[int, int]:
    """(false wakes, Parakeet asked) over negative score streams, as the daemon decides:
    a hit whose first frame at the floor is already at the threshold wakes Ghost unchecked;
    one under it is read by Parakeet."""
    fw = asked = 0
    for sc in negs:
        for _t, s in hits(sc, threshold, floor):
            if s >= threshold:
                fw += 1
            else:
                asked += 1
    return fw, asked


def operating_point(
    old_negs: list[np.ndarray],
    new_negs: list[np.ndarray],
    heard,
    shipped: tuple[float, float],
) -> tuple[float, float, dict] | None:
    """The new detector's threshold and floor, from its own curve under the daemon's rule:
    of the points that wake no more often and ask Parakeet no more often than the shipped
    detector does on the training negatives (issue #13), the one that hears most of the
    training takes (``heard(thr, floor)`` -> (woke at once, checked)). None when no point
    keeps both."""
    f0, a0 = rule_counts(old_negs, *shipped)
    best = None
    for thr in np.round(np.arange(0.50, 0.995, 0.01), 2):
        for floor in np.round(np.arange(0.20, thr + 1e-9, 0.05), 2):
            fw, asked = rule_counts(new_negs, float(thr), float(floor))
            if fw > f0 or asked > a0:
                continue
            woke, checked = heard(float(thr), float(floor))
            key = (woke, woke + checked, -fw, -asked)
            if best is None or key > best[0]:
                best = (key, float(thr), float(floor), {"false_wakes": fw, "asked": asked})
    if best is None:
        return None
    _k, thr, floor, at = best
    return thr, floor, {**at, "shipped_false_wakes": f0, "shipped_asked": a0}


@dataclass
class Count:
    takes: int = 0
    woke: int = 0  # the first frame at the floor was already at the threshold
    checked: int = 0  # a weak hit: Parakeet decides
    false_wakes: int = 0  # a hit at the threshold away from any take
    false_checked: int = 0  # a weak hit away from any take: Parakeet runs
    hours: float = 0.0

    def add(self, o: Count) -> None:
        for k in self.__dataclass_fields__:
            setattr(self, k, getattr(self, k) + getattr(o, k))

    def row(self) -> dict:
        return {k: round(v, 4) if isinstance(v, float) else v for k, v in self.__dict__.items()}


def count(
    sc: np.ndarray,
    threshold: float,
    floor: float,
    takes: list[float],
    others: list[float] | None = None,
    spans: list[tuple[float, float]] | None = None,
) -> Count:
    """``takes``: where each "ghost" ends; ``spans``: a program's clips, a hit anywhere in a
    span (or ``NEAR`` after it) being that clip."""
    c = Count(takes=len(takes) + len(spans or []), hours=len(sc) * STEP / RATE / 3600)
    seen: set[int] = set()
    for t, s in hits(sc, threshold, floor):
        strong = s >= threshold
        j = next(
            (k for k, g in enumerate(takes) if abs(t - g) <= NEAR and k not in seen),
            None,
        )
        if j is None and spans:
            j = next(
                (
                    len(takes) + k
                    for k, (a, b) in enumerate(spans)
                    if a - 0.2 <= t <= b + NEAR and len(takes) + k not in seen
                ),
                None,
            )
        if j is not None:
            seen.add(j)
            if strong:
                c.woke += 1
            else:
                c.checked += 1
            continue
        if others and any(abs(t - o) <= NEAR + 0.8 for o in others):
            continue
        if strong:
            c.false_wakes += 1
        else:
            c.false_checked += 1
    return c


def program_spans(path: Path, pcm: np.ndarray, room: bool) -> tuple[list, list, list]:
    """A replay-rig program's "hey ghost" clips, its other wake phrasings and its negatives,
    in the stream's seconds; a room recording is aligned on the program's 1 kHz marker
    (as ``wake_check_replay.py`` does)."""
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
        span = (c["start"] + lag, c["end"] + lag)
        text = c["text"].lower()
        (pos if text.startswith("hey ghost") or text.startswith("hey, ghost") else other).append(
            span
        )
    return pos, other, []


@dataclass
class Report:
    rows: list[dict] = field(default_factory=list)

    def add(self, title: str, stream: str, seen: str, before: Count, after: Count) -> None:
        self.rows.append(
            {
                "set": title,
                "stream": stream,
                "seen_in_training": seen,
                "before": before.row(),
                "after": after.row(),
            }
        )


def curve(negs: list[np.ndarray], grid: np.ndarray, floor_of: float | None = None) -> np.ndarray:
    """Hits at each threshold of ``grid`` over negative streams (every hit is false)."""
    out = np.zeros(len(grid), int)
    for k, t in enumerate(grid):
        f = t if floor_of is None else floor_of
        out[k] = sum(len(hits(sc, t, min(f, t))) for sc in negs)
    return out
