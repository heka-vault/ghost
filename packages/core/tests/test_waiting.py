"""The waiting list (ADR 0029) and changes that outlive the session (ADR 0030)."""

from datetime import timedelta

from fakes import SetState, changeset
from ghost_core import AutonomyPolicy, ProposalStatus, Store, Verdict
from ghost_core.models import utcnow
from ghost_core.waiting import (
    GATE,
    ItemDraft,
    Items,
    WaitingList,
    change_key,
    waiting_source,
)
from test_gate import prepare


def draft(key="ci:1", title="CI failed on main", **kw):
    return ItemDraft(key=key, title=title, **kw)


# --- items ---------------------------------------------------------------------------


def test_one_key_is_one_item_however_often_it_is_seen():
    items = Items(Store())
    first = items.sync("watch", [draft()])
    again = items.sync("watch", [draft()])
    assert first.new == ("ci:1",) and not again.any
    assert [i.key for i in items.open()] == ["ci:1"]


def test_a_key_that_disappears_is_resolved_and_one_that_returns_is_new():
    items = Items(Store())
    items.sync("watch", [draft()])
    gone = items.sync("watch", [])
    assert gone.resolved == ("ci:1",) and items.open() == []
    back = items.sync("watch", [draft()])
    assert back.new == ("ci:1",)


def test_acknowledged_stops_alerting_until_the_item_changes():
    items = Items(Store())
    t0 = utcnow()
    items.sync("watch", [draft()], now=t0)
    items.delivered("ci:1", "digest", now=t0 + timedelta(seconds=1))
    assert items.get("ci:1").state(t0 + timedelta(seconds=2)) == "delivered"
    assert items.get("ci:1").needs_alerting(t0 + timedelta(seconds=2))  # delivered isn't heard
    items.acknowledge("ci:1", now=t0 + timedelta(seconds=3))
    assert not items.get("ci:1").needs_alerting(t0 + timedelta(seconds=4))
    changed = items.sync("watch", [draft(title="CI failed twice")], now=t0 + timedelta(seconds=5))
    assert changed.changed == ("ci:1",)
    assert items.get("ci:1").needs_alerting(t0 + timedelta(seconds=6))


def test_snooze_hides_until_its_time():
    items = Items(Store())
    now = utcnow()
    items.sync("watch", [draft()], now=now)
    items.snooze("ci:1", now + timedelta(minutes=30))
    assert items.get("ci:1").state(now + timedelta(minutes=1)) == "snoozed"
    assert items.get("ci:1").state(now + timedelta(minutes=31)) == "new"


def test_acknowledge_and_snooze_survive_a_restart(tmp_path):
    db = tmp_path / "ghost.db"
    store = Store(db)
    Items(store).sync("watch", [draft()])
    Items(store).acknowledge("ci:1")
    store.close()
    assert Items(Store(db)).get("ci:1").state() == "acknowledged"


def test_a_key_open_under_another_source_is_left_to_it():
    items = Items(Store())
    items.sync("a", [draft()])
    other = items.sync("b", [draft(title="something else")])
    assert not other.any and items.get("ci:1").source == "a"


def test_a_title_is_flattened_and_clipped():
    items = Items(Store())
    items.sync("watch", [draft(title="a\n\nb " + "x" * 400)])
    t = items.get("ci:1").title
    assert "\n" not in t and len(t) <= 200


# --- changes that outlive the session (ADR 0030) ---------------------------------------


async def test_an_unrelated_turn_sets_the_change_aside(gate, target):
    p = await prepare(gate, target)
    verdict, outcome = await gate.on_user_turn("s1", "what's the next issue")
    assert verdict is Verdict.UNCLEAR and outcome is None
    assert gate.pending("s1") is None  # the session's next "yes" answers nothing
    assert gate._store.get_proposal(p.id).status is ProposalStatus.PENDING
    assert [w.id for w in gate.waiting()] == [p.id]


async def test_a_no_still_cancels(gate, target):
    p = await prepare(gate, target)
    await gate.on_user_turn("s1", "no")
    assert gate._store.get_proposal(p.id).status is ProposalStatus.CANCELLED
    assert gate.waiting() == []


async def test_a_change_set_aside_is_confirmed_later_by_id(gate, writer, target):
    p = await prepare(gate, target)
    gate.set_aside("s1", "hung up")
    assert await gate.on_user_turn("s1", "yes") == (Verdict.UNCLEAR, None)
    outcome = await gate.confirm_id(p.id, source="ui")
    assert outcome is not None and outcome.status == "success"
    assert writer.issues[target]["labels"] == ["approved"]
    events = [e for _, e, _ in gate._store.events(p.id)]
    assert "set_aside" in events and events[-1] == "finished"


async def test_a_change_that_went_stale_is_refused_when_confirmed_later(gate, writer, target):
    p = await prepare(gate, target)
    gate.set_aside("s1", "hung up")
    writer.issues[target]["updated_at"] = "later"  # someone edited it meanwhile
    outcome = await gate.confirm_id(p.id)
    assert outcome is not None and outcome.status == "failed"
    assert gate._store.get_proposal(p.id).status is ProposalStatus.STALE


async def test_bring_back_attaches_it_to_another_session(gate, writer, target):
    p = await prepare(gate, target, session="old")
    gate.set_aside("old", "hung up")
    assert gate.bring_back(p.id, "new", None).id == p.id
    verdict, outcome = await gate.on_user_turn("new", "yes")
    assert verdict is Verdict.AFFIRMATIVE and outcome.status == "success"


async def test_the_wait_comes_from_the_policy_file(writer, target):
    from fakes import register_fake
    from ghost_core import WriteGate

    policy = AutonomyPolicy.parse("[defaults]\npending_ttl = 600\n")
    gate = WriteGate(Store(), autonomy=policy)
    register_fake(gate, writer)
    p = await prepare(gate, target)
    assert timedelta(seconds=590) < p.expires_at - p.created_at <= timedelta(seconds=600)
    assert gate.waiting(now=utcnow() + timedelta(seconds=601)) == []
    assert gate._store.get_proposal(p.id).status is ProposalStatus.EXPIRED


ASK = '[permissions]\nmode = "ask"\n'
BLOCKED = ASK + 'block = ["github_issues.approve"]\n'


def _asking_gate(writer):
    from fakes import register_fake
    from ghost_core import WriteGate

    gate = WriteGate(Store(), autonomy=AutonomyPolicy.parse(ASK))
    register_fake(gate, writer)
    return gate


async def _submit(gate, target, department=None, session="s1"):
    from test_gate import APPROVE

    sub = await gate.submit(
        session_id=session,
        ability="github_issues",
        operation="approve",
        target=target,
        changes=APPROVE,
        rationale="operator asked",
        department=department,
    )
    assert sub.outcome is None  # it waits for a yes
    return sub.proposal


def _assert_refused_as_blocked(gate, writer, p, outcome):
    assert outcome is not None and outcome.status == "failed"
    assert "block" in (outcome.error or "")
    assert not any(c[0] != "fetch" for c in writer.calls)
    assert gate._store.get_proposal(p.id).status is ProposalStatus.CANCELLED
    assert "blocked" in [e for _, e, _ in gate._store.events(p.id)]
    assert gate.waiting() == []


async def test_a_change_blocked_while_it_waited_is_refused_when_clicked(writer, target):
    # The operator blocked it during the hour it waited: the yes no longer runs it.
    gate = _asking_gate(writer)
    p = await _submit(gate, target)
    gate.set_aside("s1", "hung up")
    gate.set_autonomy(AutonomyPolicy.parse(BLOCKED))
    _assert_refused_as_blocked(gate, writer, p, await gate.confirm_id(p.id))


async def test_a_change_blocked_while_it_waited_is_refused_when_spoken(writer, target):
    gate = _asking_gate(writer)
    p = await _submit(gate, target)
    gate.set_autonomy(AutonomyPolicy.parse(BLOCKED))
    verdict, outcome = await gate.on_user_turn("s1", "yes")
    assert verdict is Verdict.AFFIRMATIVE
    _assert_refused_as_blocked(gate, writer, p, outcome)


async def test_a_change_its_department_blocked_while_it_waited_is_refused(writer, target):
    from ghost_core.permissions import Permissions

    gate = _asking_gate(writer)
    p = await _submit(gate, target, department="ops")
    block = Permissions.parse({"block": ["github_issues"]}, "ops", narrow_only=True)
    gate.set_department_policies({"ops": gate.autonomy.with_rules(block, "ops")})
    _assert_refused_as_blocked(gate, writer, p, await gate.confirm_pending("s1"))


async def test_a_session_brings_up_only_its_own_departments_changes(writer, target):
    gate = _asking_gate(writer)
    p = await _submit(gate, target, department="sales", session="old")
    gate.set_aside("old", "hung up")
    assert gate.bring_back(p.id, "support-session", department="support") is None
    assert gate.pending("support-session") is None
    assert await gate.confirm_pending("support-session") is None
    assert gate._store.get_proposal(p.id).status is ProposalStatus.PENDING
    assert gate.bring_back(p.id, "sales-session", department="sales").id == p.id


async def test_ghost_brings_up_any_departments_change(writer, target):
    gate = _asking_gate(writer)
    p = await _submit(gate, target, department="sales", session="old")
    gate.set_aside("old", "hung up")
    assert gate.bring_back(p.id, "ghost-session", department="ghost").id == p.id
    verdict, outcome = await gate.on_user_turn("ghost-session", "yes")
    assert verdict is Verdict.AFFIRMATIVE and outcome.status == "success"
    # a change made outside any department is Ghost's alone to bring up
    q = await _submit(gate, target, session="older")
    gate.set_aside("older", "hung up")
    assert gate.bring_back(q.id, "sales-session", department="sales") is None
    assert gate.bring_back(q.id, "ghost-session", department="ghost").id == q.id


def test_pending_ttl_round_trips_and_is_bounded():
    p = AutonomyPolicy.parse("[defaults]\npending_ttl = 900\n")
    assert AutonomyPolicy.parse(p.to_toml()).pending_ttl == 900
    for bad in ("5", '"an hour"', "true"):
        try:
            AutonomyPolicy.parse(f"[defaults]\npending_ttl = {bad}\n")
        except ValueError:
            continue
        raise AssertionError(bad)


# --- the waiting list ----------------------------------------------------------------


async def test_the_list_has_the_waiting_changes_and_ghosts_problems(gate, target):
    p = await prepare(gate, target, changeset(SetState(state="closed")))
    problems = [("server:github", "The github server is down", None)]
    calls = []
    waiting = WaitingList(
        Items(gate._store),
        changes=gate.waiting,
        problems=lambda: problems,
        on_change=lambda: calls.append(1),
    )
    keys = [i.key for i in waiting.refresh()]
    assert keys == [change_key(p.id), "server:github"] and calls == [1]
    assert waiting.count() == 2
    gate.cancel_id(p.id)
    problems.clear()
    assert waiting.refresh() == [] and calls == [1, 1]


async def test_the_model_reads_typed_fields_only(gate, target):
    p = await prepare(gate, target)
    waiting = WaitingList(Items(gate._store), changes=gate.waiting)
    tool = waiting_source(waiting).read_tools[0]
    out = await tool.handler({})
    (item,) = out["items"]
    assert set(item) == {"key", "source", "department", "title", "age_minutes", "state", "change"}
    assert item["source"] == GATE and item["change"] == p.id
