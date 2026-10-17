from __future__ import annotations

import pytest
from ghost_voice.wake import DEFAULT_WAKE_PHRASES, match_wake, parse_phrases

PHRASES = ("hey ghost", "ghost you there", "ghost are you there", "okay ghost")


@pytest.mark.parametrize(
    "text, phrase, remainder",
    [
        ("Hey Ghost.", "hey ghost", ""),
        ("hey, ghost!", "hey ghost", ""),
        ("Ghost, you there?", "ghost you there", ""),
        ("Ghost, are you there?", "ghost are you there", ""),
        ("Okay Ghost, what needs my sign-off?", "okay ghost", "what needs my sign off"),
        ("um hey ghost what's pending", "hey ghost", "what s pending"),
    ],
)
def test_matches(text, phrase, remainder):
    m = match_wake(text, PHRASES)
    assert m is not None
    assert m.phrase == phrase
    assert m.remainder == remainder


@pytest.mark.parametrize(
    "text",
    [
        "",
        "the ghost of christmas past",
        "hey, is the ghost build green",
        "you there ghost",
        "ghostly",
        "hey ghosts",
    ],
)
def test_non_matches(text):
    assert match_wake(text, PHRASES) is None


def test_longest_phrase_wins():
    m = match_wake("hey ghost", ("ghost", "hey ghost"))
    assert m is not None and m.phrase == "hey ghost"


def test_parse_phrases_defaults_and_normalises():
    assert parse_phrases(None) == DEFAULT_WAKE_PHRASES
    assert parse_phrases("  ,  ") == DEFAULT_WAKE_PHRASES
    assert parse_phrases("Hey Ghost, Ghost: you there?") == ("hey ghost", "ghost you there")
