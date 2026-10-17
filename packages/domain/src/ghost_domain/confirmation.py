"""Classify a spoken user turn as a clear yes, a clear no, or neither.

Deliberately conservative: only a short, unambiguous utterance confirms. A long
sentence that happens to contain "yes" does not. The text comes from the
pipeline's own transcript (ADR 0007), never from the model.
"""

from __future__ import annotations

import enum
import re

_AFFIRMATIVE = {
    "confirm",
    "confirmed",
    "yes",
    "yes confirm",
    "yes do it",
    "yes please",
    "yep",
    "yeah",
    "do it",
    "go ahead",
    "approve",
    "approved",
    "ship it",
    "send it",
    "okay do it",
    "ok do it",
    "okay confirm",
    "ok confirm",
    "that's right",
    "correct",
    "proceed",
}

_NEGATIVE = {
    "no",
    "nope",
    "cancel",
    "cancel that",
    "never mind",
    "nevermind",
    "stop",
    "don't",
    "do not",
    "abort",
    "wait",
    "hold on",
    "not yet",
    "no cancel",
    "no don't",
    "scratch that",
}

_MAX_WORDS = 5


class Verdict(enum.StrEnum):
    AFFIRMATIVE = "affirmative"
    NEGATIVE = "negative"
    UNCLEAR = "unclear"


def normalize(text: str) -> str:
    text = text.lower().strip()
    text = re.sub(r"[^a-z' ]+", " ", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text


def classify_confirmation(text: str) -> Verdict:
    norm = normalize(text)
    if not norm:
        return Verdict.UNCLEAR
    words = norm.split()
    if len(words) > _MAX_WORDS:
        return Verdict.UNCLEAR
    # A single wake-word or filler prefix is tolerated: "ghost, confirm".
    if words and words[0] in {"ghost", "okay", "ok"} and len(words) > 1:
        rest = " ".join(words[1:])
        if rest in _NEGATIVE:
            return Verdict.NEGATIVE
        if rest in _AFFIRMATIVE:
            return Verdict.AFFIRMATIVE
    if norm in _NEGATIVE:
        return Verdict.NEGATIVE
    if norm in _AFFIRMATIVE:
        return Verdict.AFFIRMATIVE
    # Any negation word anywhere in a short utterance means no.
    if any(w in {"no", "don't", "cancel", "stop", "wait", "abort"} for w in words):
        return Verdict.NEGATIVE
    return Verdict.UNCLEAR
