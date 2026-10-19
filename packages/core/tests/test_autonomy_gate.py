"""submit paths of the gate (ADR 0011)."""

import pytest
from fakes import AddComment, Issue, SetLabels, SetState, changeset, register_fake
from ghost_core import (
    Autonomy,
    AutonomyPolicy,
    PolicyError,
    ProposalStatus,
    Store,
    Verdict,
    WriteGate,
)

LABEL = changeset(SetLabels(add=("bug",), remove=("needs-signoff",)))
COMMENT = changeset(AddComment(body="Looks fine."))
CLOSE = changeset(AddComment(body="dup"), SetState(state="closed"))


def make(writer, text="", **kw):
    g = WriteGate(Store(), autonomy=AutonomyPolicy.parse(text), **kw)
    register_fake(g, writer)
    return g


async def submit(gate, target, changes, op="set_labels", risk=None, session="s1"):
    return await gate.submit(
        session_id=session,
        ability="github_issues",
        operation=op,
        target=target,
        changes=changes,
        rationale="asked",
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
    assert gate.level("github_issues", "close", target) is Autonomy.DENY


async def test_allowlist_still_wins(writer):
    gate = make(writer, '[github_issues]\n"*" = "quiet"\n')
    other = Issue(owner="acme", repo="other", number=1)
    writer.seed(other)
    with pytest.raises(PolicyError):
        await submit(gate, other, LABEL)


async def test_reversing_is_an_ordinary_change(writer, target):
    """There is no undo path: taking a change back is the opposite change, submitted
    like any other and resolved against the operator's policy."""
    gate = make(writer, '[github_issues]\nclose = "announce"\nreopen = "confirm"\n')
    first = await submit(gate, target, LABEL)
    back = await submit(gate, target, changeset(SetLabels(add=("needs-signoff",), remove=("bug",))))
    assert back.executed and writer.issues[target]["labels"] == ["needs-signoff"]
    store = gate._store
    assert store.get_proposal(first.proposal.id).status is ProposalStatus.EXECUTED
    assert store.get_proposal(back.proposal.id).status is ProposalStatus.EXECUTED

    await submit(gate, target, CLOSE, op="close", risk="high")
    reopen = await submit(gate, target, changeset(SetState(state="open")), op="reopen")
    assert reopen.autonomy is Autonomy.CONFIRM and not reopen.executed
    assert writer.issues[target]["state"] == "closed"


async def test_audit_records_autonomy(writer, target):
    gate = make(writer)
    sub = await submit(gate, target, LABEL)
    events = gate._store.events(sub.proposal.id)
    assert events[0][1] == "prepared" and events[0][2]["autonomy"] == "announce"
    assert events[-1][1] == "finished"


async def test_custom_classifier_is_used_with_context(writer, target):
    seen = []

    class Always:
        name = "always"

        async def classify(self, utterance, ctx):
            seen.append((utterance, ctx))
            return Verdict.AFFIRMATIVE

    gate = make(writer, classifier=Always())
    await gate.prepare(
        session_id="s1",
        ability="github_issues",
        operation="close",
        target=target,
        changes=CLOSE,
        rationale="",
    )
    verdict, outcome = await gate.on_user_turn("s1", "mm hm", question="Close it?")
    assert verdict is Verdict.AFFIRMATIVE and outcome.status == "success"
    assert seen[0][1].question == "Close it?" and "closed the issue" in seen[0][1].pending_action
    assert gate._store.events(outcome.proposal_id)[1][2]["classifier"] == "always"
