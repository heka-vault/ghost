"""Routes (ADR 0031): the operator's policy decides how loud; delivered is not acknowledged."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta

import pytest
from ghost_core import AutonomyPolicy, Store
from ghost_core.routes import (
    UNCLEAR,
    RouteEngine,
    RouteError,
    RoutePolicy,
    SpokenDigest,
    parse_quiet,
    spoken_digest,
)
from ghost_core.waiting import ItemDraft, Items

T0 = datetime(2026, 10, 27, 15, 0, tzinfo=UTC)


class Fake:
    def __init__(self, name, answer="delivered"):
        self.name, self.answer, self.got = name, answer, []

    async def deliver(self, items, policy):
        self.got.append([i.key for i in items])
        return self.answer


def engine(enabled=("digest", "desk", "desktop", "phone", "call"), at_desk=False, **policy):
    items = Items(Store())
    items.sync("watch:eng/ci", [ItemDraft(key="eng:ci:1", title="CI failed on main")], T0)
    routes = {r: Fake(r) for r in ("desk", "desktop", "phone", "call", "sms")}
    e = RouteEngine(
        items,
        policy=RoutePolicy(enabled=enabled, batch_secs=0, **policy),
        routes=routes,
        at_desk=lambda: at_desk,
        local=lambda t: t,
    )
    return e, routes, items


async def test_away_from_the_desk_it_goes_to_the_phone_then_escalates_to_a_call():
    e, routes, items = engine(urgency={"eng/ci": "call"})
    await e.tick(T0)
    assert routes["phone"].got == [["eng:ci:1"]] and routes["desk"].got == []
    await e.tick(T0 + timedelta(minutes=5))  # still waiting for an acknowledgment
    assert routes["call"].got == []
    await e.tick(T0 + timedelta(minutes=16))
    assert routes["call"].got == [["eng:ci:1"]]


async def test_notify_never_goes_past_the_phone():
    e, routes, _ = engine()
    for m in (0, 16, 40):
        await e.tick(T0 + timedelta(minutes=m))
    assert routes["phone"].got == [["eng:ci:1"]] and routes["call"].got == []


async def test_at_the_desk_nobody_answering_goes_on_at_once():
    e, routes, items = engine(at_desk=True)
    routes["desk"].answer = "not_heard"
    await e.tick(T0)
    assert routes["desk"].got == [["eng:ci:1"]] and routes["desktop"].got == [["eng:ci:1"]]
    assert items.get("eng:ci:1").delivered_via == "desktop"


async def test_heard_is_the_acknowledgment():
    e, routes, items = engine(at_desk=True)
    routes["desk"].answer = "heard"
    await e.tick(T0)
    await e.tick(T0 + timedelta(minutes=30))
    assert routes["desktop"].got == [] and items.get("eng:ci:1").state(T0) == "acknowledged"


async def test_an_acknowledged_item_stops_until_it_changes():
    e, routes, items = engine()
    await e.tick(T0)
    items.acknowledge("eng:ci:1", T0 + timedelta(minutes=1))
    await e.tick(T0 + timedelta(minutes=20))
    assert routes["phone"].got == [["eng:ci:1"]]
    items.sync(
        "watch:eng/ci",
        [ItemDraft(key="eng:ci:1", title="CI failed again")],
        T0 + timedelta(minutes=21),
    )
    await e.tick(T0 + timedelta(minutes=22))
    assert routes["phone"].got == [["eng:ci:1"], ["eng:ci:1"]]


async def test_only_enabled_routes_with_a_route_behind_them_are_used():
    e, routes, _ = engine(enabled=("digest",))
    await e.tick(T0)
    assert all(r.got == [] for r in routes.values())
    assert [i.key for i in e.digest(T0)] == ["eng:ci:1"]


async def test_quiet_hours_and_do_not_disturb_hold_everything_for_the_digest():
    from datetime import time

    e, routes, _ = engine(quiet_hours=(time(14, 0), time(16, 0)))
    await e.tick(T0)
    assert routes["phone"].got == []
    e2, routes2, _ = engine()
    e2.quiet_until(T0 + timedelta(hours=1))
    await e2.tick(T0)
    assert routes2["phone"].got == []
    await e2.tick(T0 + timedelta(hours=1, minutes=1))
    assert routes2["phone"].got == [["eng:ci:1"]]


async def test_a_new_item_waits_out_the_batch_window():
    e, routes, items = engine()
    e.policy = RoutePolicy(enabled=e.policy.enabled, batch_secs=120)
    await e.tick(T0 + timedelta(seconds=30))
    assert routes["phone"].got == []
    items.sync(
        "watch:eng/ci",
        [
            ItemDraft(key="eng:ci:1", title="CI failed on main"),
            ItemDraft(key="eng:ci:2", title="again"),
        ],
        T0 + timedelta(seconds=10),
    )
    await e.tick(T0 + timedelta(minutes=3))
    assert routes["phone"].got == [["eng:ci:1", "eng:ci:2"]]  # together


async def test_the_call_cap_drops_to_the_route_below():
    e, routes, _ = engine(enabled=("digest", "call"), urgency={"eng/ci": "call"}, calls_per_hour=1)
    e.calls = [T0 - timedelta(minutes=10)]
    await e.tick(T0)
    assert routes["call"].got == []


async def test_a_route_that_fails_is_skipped_not_fatal():
    e, routes, items = engine()

    async def broken(_items, _policy):
        raise RuntimeError("no signal")

    routes["phone"].deliver = broken
    assert await e.tick(T0) == [("phone", "unavailable")]
    assert items.get("eng:ci:1").state(T0) == "new"


async def test_an_unavailable_route_hands_the_item_on():
    # notify-send missing and the desk busy: the phone still hears of it (QA, 2026-10-27)
    e, routes, items = engine(at_desk=True)
    routes["desk"].answer = routes["desktop"].answer = "unavailable"
    for m in range(0, 105, 15):
        await e.tick(T0 + timedelta(minutes=m))
    assert routes["phone"].got == [["eng:ci:1"]]
    assert items.get("eng:ci:1").delivered_via == "phone"


async def test_a_snooze_ends_and_the_item_alerts_again_from_the_start():
    e, routes, items = engine()  # away from the desk: the phone is the whole ladder
    await e.tick(T0)
    items.snooze("eng:ci:1", T0 + timedelta(minutes=30))
    await e.tick(T0 + timedelta(minutes=20))
    assert routes["phone"].got == [["eng:ci:1"]]  # snoozed: nothing
    await e.tick(T0 + timedelta(minutes=31))
    assert routes["phone"].got == [["eng:ci:1"], ["eng:ci:1"]]


async def test_after_a_snooze_at_the_desk_it_starts_at_the_desk_again():
    e, routes, items = engine(at_desk=True)
    routes["desk"].answer = "not_heard"
    await e.tick(T0)  # the desk, then the desktop at once
    items.snooze("eng:ci:1", T0 + timedelta(minutes=30))
    routes["desk"].answer = "delivered"
    await e.tick(T0 + timedelta(minutes=31))
    assert routes["desk"].got == [["eng:ci:1"], ["eng:ci:1"]]


def many(e, items, n=5):
    items.sync("watch:eng/ci", [ItemDraft(key=f"w{i}", title=f"Item {i}") for i in range(n)], T0)
    return e.digest(T0)


def test_the_digest_acknowledges_only_the_titles_it_said_once_the_operator_replies():
    e, _, items = engine()
    d = SpokenDigest(e, many(e, items))
    assert d.text == "Hi. Five things are waiting: Item 0; Item 1; Item 2, and two more."
    d.on_turn("user", "hello?", T0)  # before the greeting: not a reply to it
    assert all(i.state(T0) == "new" for i in items.open())
    d.on_turn("assistant", d.text, T0)  # the greeting reached the voice: delivered
    assert all(i.state(T0) == "delivered" and i.delivered_via == "digest" for i in items.open())
    d.on_turn("user", "okay, thanks", T0)  # the operator answered: the three said are heard
    left = [i.key for i in e.digest(T0)]
    assert left == ["w3", "w4"]  # "and two more" was not hearing them
    d.on_turn("user", "and the rest?", T0)
    assert [i.key for i in e.digest(T0)] == left  # only the first reply counts


def test_a_digest_that_was_never_spoken_marks_nothing():
    e, _, items = engine()
    d = SpokenDigest(e, many(e, items))
    d.on_turn("user", "hello?", T0)  # the voice failed: the operator never heard it
    d.on_turn("assistant", "Sure, what do you need?", T0)
    d.on_turn("user", "nothing", T0)
    assert all(i.state(T0) == "new" for i in items.open())


async def test_desk_voice_acknowledges_only_the_titles_it_could_give():
    e, routes, items = engine(at_desk=True)
    routes["desk"].answer = "heard"
    routes["desk"].names = 5  # what the session's note carries
    many(e, items, 7)
    await e.tick(T0)
    left = {i.key for i in items.open() if i.needs_alerting(T0)}
    assert left == {"w5", "w6"}
    assert all(items.get(k).delivered_via == "desk" for k in left)


def test_spoken_digest_counts_and_clips():
    many = [ItemDraft(key=f"k{i}", title=f"Item {i}") for i in range(5)]
    items = Items(Store())
    items.sync("s", many, T0)
    out = spoken_digest(items.open())
    assert out.startswith("Hi. Five things are waiting: Item 0; Item 1; Item 2, and two more")


# --- the policy file --------------------------------------------------------------------


def test_routes_live_in_the_policy_file_and_round_trip():
    text = (
        '[routes]\nenabled = ["phone", "digest", "desk"]\nquiet_hours = "22:00-07:00"\n'
        '[routes.urgency]\n"eng/ci" = "call"\n[conversations]\ndelete = "confirm"\n'
    )
    p = AutonomyPolicy.parse(text)
    assert p.routes.enabled == ("digest", "desk", "phone")  # quietest first
    assert [str(r) for r in p.permissions.ask] == ["conversations.delete"]
    again = AutonomyPolicy.parse(p.to_toml())
    assert again.routes == p.routes and again.permissions == p.permissions


@pytest.mark.parametrize(
    "body",
    [
        '[routes]\nenabled = ["pager"]',
        "[routes]\nescalate_after = 0",
        '[routes]\nquiet_hours = "late"',
        '[routes]\ncarry = "everything"',
        '[routes.urgency]\n"eng/ci" = "scream"',
        "[routes]\nsurprise = 1",
    ],
)
def test_a_bad_routes_section_is_refused(body):
    with pytest.raises(RouteError):
        AutonomyPolicy.parse(body)


def test_quiet_hours_across_midnight():
    from datetime import time

    p = RoutePolicy(quiet_hours=(time(22, 0), time(7, 0)))
    assert p.is_quiet(T0.replace(hour=23)) and p.is_quiet(T0.replace(hour=6))
    assert not p.is_quiet(T0.replace(hour=12))


# --- do not disturb, in the operator's words -----------------------------------------------


@pytest.mark.parametrize(
    "said,hours,minutes",
    [
        ("Nothing until two", 11, 0),  # 15:00 now: the next two o'clock is 02:00
        ("Ghost, do not disturb for an hour", 1, 0),
        ("no interruptions for 45 minutes", 0, 45),
        ("quiet till 5:30 pm", 2, 30),
        ("nothing until noon", 21, 0),
        ("don't disturb me for half an hour", 0, 30),
    ],
)
def test_parse_quiet(said, hours, minutes):
    assert parse_quiet(said, T0) == T0 + timedelta(hours=hours, minutes=minutes)


@pytest.mark.parametrize(
    "said", ["nothing", "what's until two", "quiet please", "nothing until later"]
)
def test_other_words_are_not_quiet(said):
    assert parse_quiet(said, T0) is None


@pytest.mark.parametrize(
    "said",
    ["quiet until 3:75", "nothing until 14 pm", "nothing until 25", "quiet for 9999999 hours"],
)
def test_a_time_that_isnt_one_is_not_understood(said):
    assert parse_quiet(said, T0) == UNCLEAR


def test_do_not_disturb_outlives_a_restart(tmp_path):
    store = Store(tmp_path / "ghost.db")
    RouteEngine(Items(store)).quiet_until(T0 + timedelta(hours=1))
    again = RouteEngine(Items(store))
    assert again.dnd_until == T0 + timedelta(hours=1) and again.quiet(T0)
    again.quiet_until(None)
    assert RouteEngine(Items(store)).dnd_until is None
