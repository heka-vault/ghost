"""submit/undo paths of the gate (ADR 0011)."""

from datetime import timedelta

import pytest
from ghost_domain import (
    AddComment,
    Autonomy,
    AutonomyPolicy,
    ChangeSet,
    GateError,
    PolicyError,
    ProposalStatus,
    RepoPolicy,
    SetLabels,
    SetState,
    Store,
    Verdict,
    WriteGate,
)
from ghost_domain.models import utcnow

LABEL = ChangeSet(steps=(SetLabels(add=("bug",), remove=("needs-signoff",)),))
COMMENT = ChangeSet(steps=(AddComment(body="Looks fine."),))
CLOSE = ChangeSet(steps=(AddComment(body="dup"), SetState(state="closed")))


def make(writer, text=""):
    return WriteGate(
        Store(), writer, RepoPolicy.from_csv("acme/app"), autonomy=AutonomyPolicy.parse(text)
    )


async def submit(gate, target, changes, op="set_labels", risk="low", session="s1"):
    return await gate.submit(
        session_id=session,
        skill="github_issues",
        operation=op,
        target=target,
        changes=changes,
        rationale="asked",
        risk=risk,
    )


async def test_low_risk_announces_and_executes_at_once(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, LABEL)
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.executed
    assert sub.outcome.status == "success"
    assert writer.issues[target]["labels"] == ["bug"]
    assert gate.pending("s1") is None
    assert "Applied on acme/app#42" in sub.outcome.summary()


async def test_high_risk_waits_for_confirmation(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, CLOSE, op="close", risk="high")
    assert sub.autonomy is Autonomy.CONFIRM and not sub.executed
    assert gate.pending("s1").id == sub.proposal.id
    assert writer.issues[target]["state"] == "open"
    verdict, outcome = await gate.on_user_turn("s1", "yeah go ahead")
    assert verdict is Verdict.AFFIRMATIVE and outcome.status == "success"
    assert writer.issues[target]["state"] == "closed"


async def test_policy_file_overrides_risk(writer, target):
    gate = make(writer, '[github_issues]\nset_labels = "confirm"\nclose = "quiet"\n')
    assert not (await submit(gate, target, LABEL)).executed
    sub = await submit(gate, target, CLOSE, op="close", risk="high")
    assert sub.autonomy is Autonomy.QUIET and sub.executed


async def test_deny_raises_and_writes_nothing(writer, target):
    gate = make(writer, '[github_issues.targets."acme/app"]\nclose = "deny"\n')
    with pytest.raises(PolicyError):
        await submit(gate, target, CLOSE, op="close", risk="high")
    assert writer.issues[target]["state"] == "open"
    assert gate.level("github_issues", "close", target, "high") is Autonomy.DENY


async def test_allowlist_still_wins(writer):
    from ghost_domain import Target

    gate = make(writer, '[github_issues]\n"*" = "quiet"\n')
    other = Target(owner="acme", repo="other", number=1)
    writer.seed(other)
    with pytest.raises(PolicyError):
        await submit(gate, other, LABEL)


async def test_undo_labels_restores_snapshot(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, LABEL)
    out = await gate.undo("s1")
    assert out.status == "success"
    assert writer.issues[target]["labels"] == ["needs-signoff"]
    assert gate._store.get_proposal(sub.proposal.id).status is ProposalStatus.UNDONE
    assert "add needs-signoff; remove bug" in out.applied[0]


async def test_undo_comment_deletes_by_marker(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, COMMENT, op="comment")
    assert writer.comments[target] == [("Looks fine.", sub.proposal.idempotency_key)]
    out = await gate.undo("s1")
    assert out.status == "success" and writer.comments[target] == []


async def test_undo_close_reopens_in_reverse_order(writer, target):
    gate = make(writer, '[github_issues]\nclose = "announce"\n')
    await submit(gate, target, CLOSE, op="close", risk="high")
    assert writer.issues[target]["state"] == "closed"
    out = await gate.undo("s1")
    assert out.status == "success"
    assert out.applied == ("open the issue", "delete the comment I added")
    assert writer.issues[target]["state"] == "open" and writer.comments[target] == []


async def test_undo_twice_has_nothing_left(writer, target):
    gate = make(writer)
    await submit(gate, target, LABEL)
    await gate.undo("s1")
    with pytest.raises(GateError, match="nothing recent"):
        await gate.undo("s1")


async def test_undo_respects_window(writer, target):
    gate = WriteGate(
        Store(),
        writer,
        RepoPolicy.from_csv("acme/app"),
        undo_window=timedelta(minutes=1),
    )
    await submit(gate, target, LABEL)
    with pytest.raises(GateError):
        await gate.undo("s1", now=utcnow() + timedelta(minutes=2))


async def test_undo_is_per_session(writer, target):
    gate = make(writer)
    await submit(gate, target, LABEL, session="a")
    with pytest.raises(GateError):
        await gate.undo("b")


async def test_undo_noop_when_nothing_changed(writer, target):
    gate = make(writer)
    # Adding a label the issue already has, then removing one it lacks: nothing to invert.
    await submit(
        gate, target, ChangeSet(steps=(SetLabels(add=("needs-signoff",), remove=("ghost",)),))
    )
    with pytest.raises(GateError, match="cannot be reversed"):
        await gate.undo("s1")


async def test_audit_records_autonomy_and_undo(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, LABEL)
    await gate.undo("s1")
    events = gate._store.events(sub.proposal.id)
    assert events[0][1] == "prepared" and events[0][2]["autonomy"] == "announce"
    assert events[-1][1] == "undone"


async def test_custom_classifier_is_used_with_context(writer, target):
    seen = []

    class Always:
        name = "always"

        async def classify(self, utterance, ctx):
            seen.append((utterance, ctx))
            return Verdict.AFFIRMATIVE

    gate = WriteGate(Store(), writer, RepoPolicy.from_csv("acme/app"), classifier=Always())
    await gate.prepare(
        session_id="s1",
        skill="github_issues",
        operation="close",
        target=target,
        changes=CLOSE,
        rationale="",
    )
    verdict, outcome = await gate.on_user_turn("s1", "mm hm", question="Close it?")
    assert verdict is Verdict.AFFIRMATIVE and outcome.status == "success"
    assert seen[0][1].question == "Close it?" and "closed the issue" in seen[0][1].pending_action
    assert gate._store.events(outcome.proposal_id)[1][2]["classifier"] == "always"
