"""Classify a spoken user turn as a clear yes, a clear no, or neither.

The text comes from the pipeline's own transcript (ADR 0007), never from the
model. Two classifiers implement the same protocol: ``RegexClassifier`` (the
allowlist below, always available) and ``ghost_domain.jev.JevClassifier``
(when a key is configured; falls back to the regex). Both apply
``has_negation`` first: a "no", "wait" or "cancel" anywhere in a short utterance
is a no, whatever else is said.
"""

from __future__ import annotations

import enum
import re
from dataclasses import dataclass
from typing import Protocol

_AFFIRMATIVE = {
    "confirm",
    "confirmed",
    "yes",
    "yes confirm",
    "yes do it",
    "yes please",
    "yep",
    "yeah",
    "yeah do it",
    "yeah go ahead",
    "sure go ahead",
    "do it",
    "go ahead",
    "go for it",
    "approve",
    "approved",
    "ship it",
    "send it",
    "okay do it",
    "ok do it",
    "okay confirm",
    "ok confirm",
    "okay go ahead",
    "ok go ahead",
    "that's right",
    "correct",
    "proceed",
    "yes go ahead",
    "yes close it",
    "yeah close it",
    "close it",
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
    "leave it",
}

_NEGATION_WORDS = {"no", "don't", "dont", "cancel", "stop", "wait", "abort", "nope", "not"}

_MAX_WORDS = 5


class Verdict(enum.StrEnum):
    AFFIRMATIVE = "affirmative"
    NEGATIVE = "negative"
    UNCLEAR = "unclear"


@dataclass(frozen=True)
class ConfirmationContext:
    """What the operator is answering. Lets a classifier tell "yeah, close it" from
    "yeah, the other one"."""

    pending_action: str  # the proposal's summary, e.g. 'On acme/app#7 "Login": close the issue'
    question: str = ""  # what Ghost asked, when known


class ConfirmationClassifier(Protocol):
    async def classify(self, utterance: str, context: ConfirmationContext) -> Verdict: ...


def normalize(text: str) -> str:
    text = text.lower().strip()
    text = re.sub(r"[^a-z' ]+", " ", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text


def has_negation(text: str) -> bool:
    """True when a short utterance contains a negation word anywhere. The guard that
    bounds a confident wrong yes from any classifier."""
    words = normalize(text).split()
    return 0 < len(words) <= _MAX_WORDS and any(w in _NEGATION_WORDS for w in words)


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
    if has_negation(norm):
        return Verdict.NEGATIVE
    if norm in _AFFIRMATIVE:
        return Verdict.AFFIRMATIVE
    return Verdict.UNCLEAR


class RegexClassifier:
    """The allowlist above. Deliberately conservative: only a short, unambiguous
    utterance confirms. A long sentence that happens to contain "yes" does not."""

    name = "regex"

    async def classify(self, utterance: str, context: ConfirmationContext) -> Verdict:
        return classify_confirmation(utterance)
