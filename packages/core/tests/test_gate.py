import asyncio
from datetime import timedelta

import pytest
from fakes import AddComment, FakeWriter, Issue, SetLabels, SetState, changeset, register_fake
from ghost_core import ChangeSet, ProposalStatus, Store, Verdict, WriteGate
from ghost_core.policy import PolicyError

APPROVE = changeset(
    SetLabels(add=("approved",), remove=("needs-signoff",)), AddComment(body="Approved by voice.")
)


async def prepare(gate, target, changes=APPROVE, session="s1"):
    return await gate.prepare(
        session_id=session,
        ability="github_issues",
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


async def test_stale_target_is_not_written(gate, writer, target):
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


async def test_new_prepare_sets_the_old_aside(gate, target):
    # ADR 0030: the older change is not cancelled; it waits on the waiting list.
    p1 = await prepare(gate, target)
    p2 = await prepare(gate, target, changeset(SetState(state="closed")))
    assert gate.pending("s1").id == p2.id
    assert gate._store.get_proposal(p1.id).status is ProposalStatus.PENDING
    assert [p.id for p in gate.waiting()] == [p1.id, p2.id]


async def test_sessions_are_isolated(gate, target):
    await prepare(gate, target, session="a")
    assert await gate.on_user_turn("b", "confirm") == (Verdict.UNCLEAR, None)
    assert gate.pending("a") is not None


async def test_ability_allowlist_blocks_other_targets(gate):
    with pytest.raises(PolicyError):
        await prepare(gate, Issue(owner="acme", repo="other", number=1))


async def test_unknown_operation_is_a_gate_error(gate, target):
    from ghost_core import GateError

    with pytest.raises(GateError):
        await gate.submit(
            session_id="s",
            ability="nope",
            operation="x",
            target=target,
            changes=APPROVE,
            rationale="",
        )


async def test_claim_is_atomic(writer, target):
    store = Store()
    gate = WriteGate(store)
    register_fake(gate, writer)
    p = await prepare(gate, target)
    assert store.claim(p.id) is True
    assert store.claim(p.id) is False


async def test_audit_trail(gate, target):
    p = await prepare(gate, target)
    await gate.on_user_turn("s1", "confirm")
    names = [e[1] for e in gate._store.events(p.id)]
    assert names[:3] == ["prepared", "user_turn", "claimed"]
    assert names[-1] == "finished"
    assert gate._store.events(p.id)[0][2]["target"] == "fake:acme/app#42"


async def test_proposal_round_trips_through_sqlite(gate, target):
    p = await prepare(gate, target)
    back = gate._store.get_proposal(p.id)
    assert back.target == target and isinstance(back.target, Issue)
    assert back.changes == APPROVE and back.snapshot.labels == ("needs-signoff",)


async def test_legacy_undo_rows_still_read(tmp_path, target):
    """Databases from before undo was removed hold proposals marked 'undone' and
    reversal rows named ``undo:<op>`` whose steps no longer exist. They still read."""
    import json
    import sqlite3

    from ghost_core.codec import RetiredChange

    path = tmp_path / "ghost.db"
    store = Store(path)
    gate = WriteGate(store)
    w = FakeWriter()
    w.seed(target)
    register_fake(gate, w)
    p = await prepare(gate, target)
    body = json.loads(store._conn.execute("SELECT body FROM proposals").fetchone()[0])
    body |= {
        "id": "u1",
        "operation": "undo:approve",
        "undo_of": p.id,
        "changes": [{"$type": "fakes:DeleteComment", "data": {"marker": f"ghost:{p.id}"}}],
    }
    store.close()
    conn = sqlite3.connect(path)
    conn.execute("UPDATE proposals SET status = 'undone' WHERE id = ?", (p.id,))
    conn.execute(
        "INSERT INTO proposals (id, session_id, status, created_at, expires_at, body)"
        " VALUES ('u1', 's1', 'executed', ?, ?, ?)",
        (body["created_at"], body["expires_at"], json.dumps(body)),
    )
    conn.commit()
    conn.close()

    store = Store(path)
    assert store.get_proposal(p.id).status is ProposalStatus.EXECUTED
    rows, total = store.list_proposals()
    assert total == 2 and {r.id for r in rows} == {p.id, "u1"}
    legacy = next(r for r in rows if r.id == "u1")
    assert legacy.operation == "undo:approve"
    assert isinstance(legacy.changes.steps[0], RetiredChange)
    rows, total = store.list_proposals(status="executed")
    assert total == 2 and len(rows) == 2


def test_changeset_is_bounded():
    with pytest.raises(ValueError):
        ChangeSet(steps=tuple(AddComment(body="x") for _ in range(5)))
    with pytest.raises(ValueError):
        ChangeSet(steps=())


def test_fresh_writer_fixture_is_independent():
    assert FakeWriter().issues == {}


# --- two yeses at once -------------------------------------------------------------


class SlowWriter(FakeWriter):
    """Every read takes a moment, as a real app's does. ``versioned`` False is an app's
    tool, whose snapshot has no version (ADR 0034)."""

    def __init__(self, versioned: bool = True) -> None:
        super().__init__()
        self.versioned = versioned

    async def fetch(self, target):
        await asyncio.sleep(0.01)
        snap = await super().fetch(target)
        return snap if self.versioned else snap.model_copy(update={"version": ""})


async def _later(coro):
    await asyncio.sleep(0.005)
    return await coro


@pytest.mark.parametrize("versioned", [True, False])
async def test_two_confirms_at_once_run_the_change_once(target, versioned):
    # A Confirm click and a spoken yes at the same moment: one runs it, the other finds
    # it no longer waiting, and nothing raises or writes over the executed row.
    writer = SlowWriter(versioned)
    writer.seed(target)
    gate = WriteGate(Store())
    register_fake(gate, writer)
    p = await prepare(gate, target)
    gate.set_aside("s1", "hung up")
    first, second = await asyncio.gather(gate.confirm_id(p.id), _later(gate.confirm_id(p.id)))
    assert first is not None and first.status == "success"
    assert second is None
    assert gate._store.get_proposal(p.id).status is ProposalStatus.EXECUTED
    assert [c[0] for c in writer.calls].count("set_labels") == 1
    assert "stale" not in [e for _, e, _ in gate._store.events(p.id)]


async def test_a_spoken_yes_and_a_click_at_once_run_the_change_once(target):
    writer = SlowWriter()
    writer.seed(target)
    gate = WriteGate(Store())
    register_fake(gate, writer)
    p = await prepare(gate, target)
    (_, spoken), clicked = await asyncio.gather(
        gate.on_user_turn("s1", "yes"), _later(gate.confirm_id(p.id))
    )
    assert [o is not None for o in (spoken, clicked)].count(True) == 1
    assert gate._store.get_proposal(p.id).status is ProposalStatus.EXECUTED
    assert [c[0] for c in writer.calls].count("set_labels") == 1


async def test_a_finished_proposal_keeps_its_status(gate, target):
    p = await prepare(gate, target)
    await gate.on_user_turn("s1", "yes")
    store = gate._store
    for status in (ProposalStatus.STALE, ProposalStatus.CANCELLED, ProposalStatus.EXPIRED):
        assert store.set_status(p.id, status) is False
    assert store.get_proposal(p.id).status is ProposalStatus.EXECUTED
    assert gate.cancel_id(p.id) is None


# --- scope (ADR 0019) ---------------------------------------------------------------
