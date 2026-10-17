import pytest
from ghost_domain import Verdict, classify_confirmation


@pytest.mark.parametrize(
    "text",
    ["Confirm.", "yes", "Yes, confirm.", "Go ahead", "ghost confirm", "Okay, do it", "approved"],
)
def test_affirmatives(text):
    assert classify_confirmation(text) is Verdict.AFFIRMATIVE


@pytest.mark.parametrize(
    "text",
    ["no", "Cancel that", "wait", "never mind", "no, don't", "Ghost, cancel", "yes but wait"],
)
def test_negatives(text):
    assert classify_confirmation(text) is Verdict.NEGATIVE


@pytest.mark.parametrize(
    "text",
    [
        "",
        "yes I think the login page should also get a dark mode toggle before confirm",
        "what are the labels again",
        "confirm the second one and skip the first",
        "sure",
    ],
)
def test_unclear(text):
    assert classify_confirmation(text) is Verdict.UNCLEAR
