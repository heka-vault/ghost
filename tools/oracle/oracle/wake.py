"""The transcript wake matcher, as ``ghost_voice.wake`` matched a phrase (ADR 0010).

The Rust port is ``crates/ghost-infer/src/wake/matcher.rs``; ``docs/benchmarks/scripts/
wake_eval.py`` and ``wake_takes_eval.py`` score recorded takes with this one. A match
returns the words that followed the phrase, so "hey ghost, what needs me?" becomes the
first turn.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from difflib import SequenceMatcher

DEFAULT_WAKE_PHRASES = (
    "hey ghost",
    "yo ghost",
    "okay ghost",
    "ok ghost",
    "ghost you there",
    "ghost are you there",
    "ghost wake up",
)


@dataclass(frozen=True)
class WakeMatch:
    phrase: str
    remainder: str  # what was said after the phrase, "" if nothing


def _words(text: str) -> list[str]:
    return re.sub(r"[^\w\s]", " ", text.lower()).split()


def _after(text: str, k: int) -> str:
    """``text`` from its ``k``-th word (as ``_words`` counts them) on, as it was written:
    the request keeps its case and punctuation ("let's", not "let s")."""
    starts = [m.start() for m in re.finditer(r"\w+", text)]
    return text[starts[k] :].strip() if k < len(starts) else ""


def parse_phrases(spec: str | None) -> tuple[str, ...]:
    """Comma-separated phrases from the environment, normalised; empty -> defaults."""
    if not spec or not spec.strip():
        return DEFAULT_WAKE_PHRASES
    out = tuple(" ".join(_words(p)) for p in spec.split(",") if _words(p))
    return out or DEFAULT_WAKE_PHRASES


_PHONETIC_SUBS = (
    ("ph", "f"),
    ("gh", "g"),
    ("ck", "k"),
    ("wh", "w"),
    ("th", "t"),
    ("dg", "j"),
    ("c", "k"),
    ("q", "k"),
    ("x", "ks"),
    ("z", "s"),
)


def _phonetic(word: str) -> str:
    """A crude phonetic key: consonant skeleton after the first letter, common English
    confusions folded. "ghost" -> "gst", "goes" and "guys" -> "gs", "hey" and "he" -> "h"."""
    w = word.lower()
    for a, b in _PHONETIC_SUBS:
        w = w.replace(a, b)
    if not w:
        return ""
    out = w[0]
    for ch in w[1:]:
        if ch in "aeiouyh" or ch == out[-1]:
            continue
        out += ch
    return out


def _similar(a: str, b: str) -> float:
    return SequenceMatcher(None, a, b).ratio()


def match_wake(
    text: str, phrases: tuple[str, ...] | list[str], *, fuzzy: float = 0.0
) -> WakeMatch | None:
    """Find the first wake phrase as a contiguous word sequence in ``text``.

    Case and punctuation are ignored, so "Ghost, you there?" matches
    "ghost you there". Longer phrases are tried first so "hey ghost" wins over
    a plain "ghost" if both are configured.

    With ``fuzzy`` > 0, a window of the same word count whose phonetic key is at
    least that similar to the phrase's also matches. 0 disables it. 0.9 (ghostd's
    default) takes spellings with the phrase's own sounds, "he ghost" for "hey ghost";
    0.8 also takes "hey guys" and "he goes", which a webcam mic needed, but they are
    ordinary speech too: 2.1 false wakes an hour of read speech, none at 0.9
    (docs/benchmarks/2026-10-21-wake.md).
    """
    words = _words(text)
    if not words:
        return None
    ordered = sorted(phrases, key=lambda p: -len(p.split()))
    for phrase in ordered:
        pw = phrase.split()
        n = len(pw)
        for i in range(len(words) - n + 1):
            if words[i : i + n] == pw:
                return WakeMatch(phrase=phrase, remainder=_after(text, i + n))
    if fuzzy <= 0:
        return None
    best: tuple[float, WakeMatch] | None = None
    for phrase in ordered:
        pw = phrase.split()
        n = len(pw)
        keys = [_phonetic(w) for w in pw]
        key = "".join(keys)
        anchor = max(keys, key=len)  # the distinctive word ("ghost"), which must be present
        for i in range(len(words) - n + 1):
            window = [_phonetic(w) for w in words[i : i + n]]
            if max(_similar(w, anchor) for w in window) < 0.66:
                continue  # "see you there" is not "ghost you there"
            score = _similar("".join(window), key)
            if score >= fuzzy and (best is None or score > best[0]):
                best = (score, WakeMatch(phrase=phrase, remainder=_after(text, i + n)))
    if best:
        return best[1]
    return None


# Words that may come before the name when the operator addresses Ghost ("so, Ghost, …").
# Not "the" or "a": "the ghost story" isn't talking to Ghost.
OPENING_LEAD = frozenset(
    {"so", "okay", "ok", "hey", "and", "um", "uh", "well", "oh", "alright", "right", "yo"}
)


# What Parakeet writes for "Ghost" as the first word of an utterance, in the operator's
# voice: "Coach" (2026-10-24), "goes", "coast". The g/k onset and the o vowel come through;
# the end doesn't. A word of that shape counts as the name only when an opener follows it.
NAME_SHAPE = re.compile(r"^[gkc]h?o[a-z]{0,4}$")


def _opener_follows(words: list[str], openers: Sequence[str]) -> bool:
    """``words`` begin with one of ``openers``, each word exact or a phonetic near-miss."""
    for opener in openers:
        ow = _words(opener)
        if len(words) >= len(ow) and all(
            w == o or _similar(_phonetic(w), _phonetic(o)) >= 0.8
            for w, o in zip(words[: len(ow)], ow, strict=True)
        ):
            return True
    return False


def match_opening(
    text: str, phrase: str, *, fuzzy: float = 0.0, openers: Sequence[str] = ()
) -> WakeMatch | None:
    """``phrase`` opening a sentence, or after one interjection ("so, Ghost, …"): the
    operator addressing Ghost by name. The words after it are the request. Any sentence
    counts, since what was heard can start a sentence before they turned to Ghost ("Let
    me try again. Ghost, I need you …").

    Unlike ``match_wake`` the name inside a sentence ("the ghost story") is no match.
    ``fuzzy`` compares phonetic keys as ``match_wake`` does: at 0.8 "goes" is "ghost".
    With ``openers``, a first word merely shaped like the name (``NAME_SHAPE``, "Coach")
    also counts when one of them follows it: the detector heard the name, and the opener's
    words are the part Parakeet gets right.
    """
    words = _words(text)
    pw = phrase.split()
    n = len(pw)
    key = "".join(_phonetic(w) for w in pw)
    sentences = [0] + [len(_words(text[: m.end()])) for m in re.finditer(r"[.!?]+\s+", text)]
    for s in dict.fromkeys(sentences):
        for i in [s, s + 1] if s < len(words) and words[s] in OPENING_LEAD else [s]:
            window = words[i : i + n]
            if len(window) < n:
                break
            if window == pw or (
                fuzzy > 0 and _similar("".join(_phonetic(w) for w in window), key) >= fuzzy
            ):
                return WakeMatch(phrase=phrase, remainder=_after(text, i + n))
            shaped = n == 1 and openers and NAME_SHAPE.match(window[0])
            if shaped and _opener_follows(words[i + 1 :], openers):
                return WakeMatch(phrase=phrase, remainder=_after(text, i + 1))
    return None
