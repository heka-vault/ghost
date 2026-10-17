from datetime import timedelta

import pytest
from ghost_domain import (
    AddComment,
    ChangeSet,
    ProposalStatus,
    RepoPolicy,
    SetLabels,
    SetState,
    Store,
    Target,
    Verdict,
    WriteGate,
)
from ghost_domain.policy import PolicyError

APPROVE = ChangeSet(
    steps=(
        SetLabels(add=("approved",), remove=("needs-signoff",)),
        AddComment(body="Approved by voice."),
    )
)


async def prepare(gate, target, changes=APPROVE, session="s1"):
    return await gate.prepare(
        session_id=session,
        skill="github_issues",
        operation="approve",
        target=target,
        changes=changes,
        rationale="operator asked",
    )


async def test_prepare_writes_nothing(gate, writer, target):
    p = await prepare(gate, target)
    assert p.status is ProposalStatus.PENDING
    assert "add approved" in p.preview and p.autonomy == "confirm"
    assert [c[0] for c in writer.calls] == ["fetch"]
    assert writer.issues[target]["labels"] == ["needs-signoff"]


async def test_confirm_executes_and_reads_back(gate, writer, target):
    p = await prepare(gate, target)
    verdict, outcome = await gate.on_user_turn("s1", "Confirm.")
    assert verdict is Verdict.AFFIRMATIVE
    assert outcome is not None and outcome.status == "success"
    assert outcome.readback is not None and outcome.readback.labels == ("approved",)
    assert writer.comments[target][0][1] == p.idempotency_key
    assert gate.pending("s1") is None
    assert "Done" in outcome.spoken()


async def test_negative_cancels(gate, writer, target):
    await prepare(gate, target)
    verdict, outcome = await gate.on_user_turn("s1", "no, cancel")
    assert verdict is Verdict.NEGATIVE and outcome is None
    assert gate.pending("s1") is None
    assert not any(c[0] != "fetch" for c in writer.calls)


async def test_unrelated_turn_cancels_instead_of_confirming(gate, writer, target):
    await prepare(gate, target)
    verdict, outcome = await gate.on_user_turn("s1", "what's the next issue")
    assert verdict is Verdict.UNCLEAR and outcome is None
    assert gate.pending("s1") is None


async def test_no_pending_is_a_noop(gate):
    assert await gate.on_user_turn("s1", "confirm") == (Verdict.UNCLEAR, None)


async def test_expired_proposal_cannot_execute(gate, writer, target):
    p = await prepare(gate, target)
    later = p.expires_at + timedelta(seconds=1)
    verdict, outcome = await gate.on_user_turn("s1", "confirm", now=later)
    assert outcome is None
    assert gate._store.get_proposal(p.id).status is ProposalStatus.EXPIRED
    assert not any(c[0] != "fetch" for c in writer.calls)


async def test_stale_issue_is_not_written(gate, writer, target):
    await prepare(gate, target)
    writer.issues[target]["updated_at"] = "2026-10-16T00:00:05Z"  # someone edited it
    _, outcome = await gate.on_user_turn("s1", "confirm")
    assert outcome is not None and outcome.status == "failed"
    assert "changed since" in (outcome.error or "")
    assert not any(c[0] != "fetch" for c in writer.calls)


async def test_partial_failure_is_reported_honestly(gate, writer, target):
    await prepare(gate, target)
    writer.fail_on = "add_comment"
    _, outcome = await gate.on_user_turn("s1", "confirm")
    assert outcome.status == "partial"
    assert outcome.applied == ("labels: add approved; remove needs-signoff",)
    assert outcome.failed_step.startswith("comment:")
    assert "Partly done" in outcome.spoken()


async def test_first_step_failure_is_failed(gate, writer, target):
    await prepare(gate, target)
    writer.fail_on = "set_labels"
    _, outcome = await gate.on_user_turn("s1", "confirm")
    assert outcome.status == "failed" and outcome.applied == ()


async def test_new_prepare_replaces_pending(gate, target):
    p1 = await prepare(gate, target)
    p2 = await prepare(gate, target, ChangeSet(steps=(SetState(state="closed"),)))
    assert gate.pending("s1").id == p2.id
    assert gate._store.get_proposal(p1.id).status is ProposalStatus.CANCELLED


async def test_sessions_are_isolated(gate, target):
    await prepare(gate, target, session="a")
    assert await gate.on_user_turn("b", "confirm") == (Verdict.UNCLEAR, None)
    assert gate.pending("a") is not None


async def test_policy_blocks_other_repos(gate):
    with pytest.raises(PolicyError):
        await prepare(gate, Target(owner="acme", repo="other", number=1))


async def test_claim_is_atomic(writer, target):
    store = Store()
    gate = WriteGate(store, writer, RepoPolicy.from_csv("acme/app"))
    p = await prepare(gate, target)
    assert store.claim(p.id) is True
    assert store.claim(p.id) is False


async def test_audit_trail(gate, target):
    p = await prepare(gate, target)
    await gate.on_user_turn("s1", "confirm")
    names = [e[1] for e in gate._store.events(p.id)]
    assert names[:3] == ["prepared", "user_turn", "claimed"]
    assert names[-1] == "finished"


def test_changeset_is_bounded():
    with pytest.raises(ValueError):
        ChangeSet(steps=tuple(AddComment(body="x") for _ in range(5)))
    with pytest.raises(ValueError):
        ChangeSet(steps=())
