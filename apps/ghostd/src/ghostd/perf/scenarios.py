"""Scripted conversations. Each opens like a real wake (Ghost greets), then plays the
operator's lines one at a time, waiting for Ghost to finish each answer.

A line with ``" ... "`` has a pause in the middle (ghostd.perf.audio). The checks are
soft: a run that drifts (the model asks instead of acting) is reported, not failed.
They read the mock GitHub state and the latency records, never transcripts."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

Check = Callable[[dict[str, Any]], str | None]  # mock issues -> problem, or None


@dataclass(frozen=True)
class Line:
    text: str
    tools: frozenset[str] = frozenset()  # tools the answer should use
    status: str | None = None  # a change's expected status, e.g. "done:announce"
    ends: bool = False  # the session should end after this line
    intercepted: bool = False


@dataclass(frozen=True)
class Scenario:
    name: str
    about: str
    lines: tuple[Line, ...]
    check: Check | None = None
    tags: frozenset[str] = field(default_factory=frozenset)


def _issue(issues: dict[str, Any], key: str) -> dict[str, Any]:
    return issues.get(key) or {}


def _closed(key: str) -> Check:
    def check(issues: dict[str, Any]) -> str | None:
        state = _issue(issues, key).get("state")
        return None if state == "closed" else f"{key} is {state}, expected closed"

    return check


def _labeled(key: str, label: str) -> Check:
    def check(issues: dict[str, Any]) -> str | None:
        labels = {lb["name"] for lb in _issue(issues, key).get("labels", [])}
        return None if label in labels else f"{key} lacks {label!r}"

    return check


SCENARIOS: dict[str, Scenario] = {
    s.name: s
    for s in (
        Scenario(
            "greeting",
            "the wake greeting, then a hello",
            (Line("Hi Ghost, how's it going?"),),
        ),
        Scenario(
            "small_talk",
            "a question with no tools",
            (Line("What's a good name for a gray cat?"),),
        ),
        Scenario(
            "lookup",
            "one read tool",
            (Line("What needs my sign-off?", tools=frozenset({"list_awaiting_signoff"})),),
        ),
        Scenario(
            "lookup_pause",
            "one read tool, a pause mid-sentence (the turn hold)",
            (
                Line(
                    "What needs my sign-off ... in the app repo?",
                    tools=frozenset({"list_awaiting_signoff"}),
                ),
            ),
            tags=frozenset({"pause"}),
        ),
        Scenario(
            "chain",
            "an issue and what people said: one get_issue now, two reads before",
            (
                Line(
                    "What's going on with app 142, and what have people said about it?",
                    tools=frozenset({"get_issue"}),  # with its latest comments
                ),
            ),
        ),
        Scenario(
            "announce",
            "a change at announce: approve",
            (Line("Approve app 151.", tools=frozenset({"change"}), status="done:announce"),),
            check=_labeled("ghost-mock/app#151", "approved"),
        ),
        Scenario(
            "confirm_plain",
            "a confirm change asked plainly: the request is the yes (ADR 0017)",
            (Line("Close infra 17.", tools=frozenset({"change"}), status="done:request"),),
            check=_closed("ghost-mock/infra#17"),
        ),
        Scenario(
            "confirm_yes",
            "a confirm change that needs a spoken yes (Jev or regex)",
            (
                Line(
                    "I think infra 17 can probably be closed now.",
                    tools=frozenset({"change"}),
                    status="needs_confirmation",
                ),
                Line("Yes, go ahead."),
            ),
            check=_closed("ghost-mock/infra#17"),
        ),
        Scenario(
            "bye_shortcut",
            "a goodbye the quick intents take without the model",
            (Line("That's all for now, bye.", ends=True, intercepted=True),),
        ),
        Scenario(
            "bye_model",
            "a goodbye only the model recognizes (hang_up)",
            (
                Line(
                    "Okay, I'm heading out to lunch now, we can stop here.",
                    tools=frozenset({"hang_up"}),
                    ends=True,
                ),
            ),
        ),
        Scenario(
            "session",
            "a realistic session: lookups, a change, goodbye (cache growth across turns)",
            (
                Line("What needs my sign-off?", tools=frozenset({"list_awaiting_signoff"})),
                Line("Tell me more about the dark mode one.", tools=frozenset({"get_issue"})),
                Line("Approve it.", tools=frozenset({"change"}), status="done:announce"),
                Line("Thanks, that's everything.", ends=True, intercepted=True),
            ),
            check=_labeled("ghost-mock/app#151", "approved"),
            tags=frozenset({"long"}),
        ),
    )
}

DEFAULT = (
    "greeting",
    "small_talk",
    "lookup",
    "lookup_pause",
    "chain",
    "announce",
    "confirm_plain",
    "confirm_yes",
    "bye_shortcut",
    "bye_model",
    "session",
)
