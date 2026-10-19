"""A request that plainly asks for a confirm-level change is its yes (ghost_core.request)."""

from __future__ import annotations

import pytest
from fakes import SetState, changeset
from ghost_core import ProposalStatus, asked_for, number_mentions

CLOSE = ("close", "shut")
N17 = number_mentions(17)
N142 = number_mentions(142)


def test_number_mentions():
    assert {"17", "seventeen"} <= N17
    assert {"142", "one forty two", "one hundred forty two"} <= N142


@pytest.mark.parametrize(
    "request_,context,expect",
    [
        ("Close infra 17.", "", True),
        ("close seventeen with a note saying we won't do it", "", True),
        ("go ahead and close one forty two", "", False),  # wrong number for 17
        ("can you close 17", "", True),
        ("close it", "Infra 17 is the Discord webhook rotation.", True),
        ("yeah close that one", "Infra 17 is the Discord webhook rotation.", True),
        ("close it", "App 151 is the dark mode toggle.", False),  # "it" isn't 17
        ("close it", "", False),
        # hedged, negated, asked: Ghost asks
        ("should we close 17", "", False),
        ("maybe close 17", "", False),
        ("don't close 17", "", False),
        ("Not closes, we're not gonna do it.", "Close infra 17?", False),
        ("close 17?", "", False),
        ("Hey, can you um reopen infra number seventeen?", "", False),  # reopen isn't close
        ("Hey, can you um close infra number seventeen?", "", True),
        ("could you close 17?", "", True),
        ("Okay, that's fine. Can we close number seventeen, please?", "", True),
        ("That's fine. Is 17 closed?", "", False),
        ("is 17 closed", "", False),
        ("is 17 closed?", "", False),
        ("what happens if you close 17?", "", False),
        ("I think we close 17", "", False),
        # no operation word: it was inferred
        ("we're not gonna do infra 17", "", False),
        ("label 17 bug", "", False),
    ],
)
def test_asked_for(request_, context, expect):
    assert asked_for(request_, verbs=CLOSE, mentions=N17, context=context) is expect


async def prepare_close(gate, target, session="s1"):
    return await gate.prepare(
        session_id=session,
        ability="github_issues",
        operation="close",
        target=target,
        changes=changeset(SetState(state="closed")),
        rationale="operator asked",
    )


async def test_request_confirms_and_records_it(gate, writer, target):
    p = await prepare_close(gate, target)
    out = await gate.confirm_by_request("s1", "close 42 please")
    assert out is not None and out.status == "success"
    assert writer.issues[target]["state"] == "closed"
    assert gate.pending("s1") is None
    turns = [e for e in gate._store.events(p.id) if e[1] == "user_turn"]  # noqa: SLF001
    assert turns[-1][2]["classifier"] == "request"


async def test_vague_request_leaves_it_pending(gate, writer, target):
    await prepare_close(gate, target)
    assert await gate.confirm_by_request("s1", "we're not gonna do that one") is None
    p = gate.pending("s1")
    assert p is not None and p.status is ProposalStatus.PENDING
    assert writer.issues[target]["state"] != "closed"


async def test_operation_without_words_always_asks(gate, writer, target):
    from fakes import SetLabels

    await gate.prepare(
        session_id="s1",
        ability="github_issues",
        operation="approve",
        target=target,
        changes=changeset(SetLabels(add=("approved",), remove=("needs-signoff",))),
        rationale="",
    )
    assert await gate.confirm_by_request("s1", "approve 42") is None
    assert gate.pending("s1") is not None
