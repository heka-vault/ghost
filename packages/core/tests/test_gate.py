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


async def test_new_prepare_replaces_pending(gate, target):
    p1 = await prepare(gate, target)
    p2 = await prepare(gate, target, changeset(SetState(state="closed")))
    assert gate.pending("s1").id == p2.id
    assert gate._store.get_proposal(p1.id).status is ProposalStatus.CANCELLED


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


# --- scope (ADR 0019) ---------------------------------------------------------------


async def test_scope_refuses_a_write_outside_the_thread_before_policy(gate, writer, target):
    from ghost_core import ActiveScope, OutOfScope

    other = Issue(owner="acme", repo="app", number=43)
    writer.seed(other)
    inside = ActiveScope("github_issues", "App", items=("acme/app",))
    sub = await gate.submit(
        session_id="s1",
        ability="github_issues",
        operation="set_labels",
        target=target,
        changes=changeset(SetLabels(add=("bug",))),
        rationale="",
        scope=inside,
    )
    assert sub.executed

    outside = ActiveScope(
        "github_issues",
        "Infra",
        items=("acme/infra",),
        reads_wider=True,  # cross-department answers never loosen writes
        owners=lambda item: ["App"] if item == "acme/app" else [],
    )
    with pytest.raises(OutOfScope) as e:
        await gate.submit(
            session_id="s1",
            ability="github_issues",
            operation="set_labels",
            target=other,
            changes=changeset(SetLabels(add=("bug",))),
            rationale="",
            scope=outside,
        )
    assert str(e.value) == "acme/app is outside this department; it belongs to the App department"
    assert e.value.owners == ("App",)
    assert not any(c[0] == "set_labels" and c[1] == other for c in writer.calls)
    assert gate.pending("s1") is None  # nothing was even proposed

    nobody = ActiveScope("github_issues", "Infra", items=("acme/infra",))
    with pytest.raises(PolicyError, match="acme/app is outside this department$"):
        await gate.prepare(
            session_id="s1",
            ability="github_issues",
            operation="set_labels",
            target=other,
            changes=changeset(SetLabels(add=("bug",))),
            rationale="",
            scope=nobody,
        )

    # unscoped (Ghost): everything the ability allows
    sub = await gate.submit(
        session_id="s1",
        ability="github_issues",
        operation="set_labels",
        target=other,
        changes=changeset(SetLabels(add=("bug",))),
        rationale="",
        scope=ActiveScope("github_issues", "Ghost", items=None),
    )
    assert sub.executed


def test_active_scope_helpers():
    from ghost_core import ActiveScope, OutOfScope, scope_of

    s = ActiveScope("github_issues", "App", items=("acme/app",), reads_wider=True)
    assert s.scoped and s.only == "acme/app"
    assert s.covers("ACME/App") and not s.covers("acme/infra")
    assert s.narrow(["acme/infra", "acme/app"]) == ["acme/app"]
    s.check_read("acme/infra")  # reads may go wider
    with pytest.raises(OutOfScope):
        s.check_write("acme/infra")
    two = ActiveScope("github_issues", "App", items=("a/b", "c/d"))
    assert two.only is None
    with pytest.raises(OutOfScope):
        two.check_read("x/y")
    assert scope_of({"_scope": s}) is s and scope_of({"_scope": "junk"}) is None
    assert scope_of({}) is None
