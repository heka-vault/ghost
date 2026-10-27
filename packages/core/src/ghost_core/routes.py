"""How Ghost reaches the operator (ADR 0031): the route policy and the route engine.

The operator's ``[routes]`` in ``policy.toml`` says which routes are on, how loud each
watch may get, when to escalate, quiet hours, batching and a cap on calls. The model sets
none of it; do-not-disturb is a fixed intent the operator says.

Routes, quietest first: ``digest`` (held until the next wake), ``desk`` (Ghost greets at
the desk and waits for a reply, ADR 0032), ``desktop`` (a notification), ``phone`` (the
phone app's waiting list and its notification while it runs), ``call`` (the desk rings
the phone), then the carriers (``sms``, ``whatsapp``). The digest and the phone are on
unless the file says otherwise; every other route is off until enabled.

**Delivered is not acknowledged.** A route reports what it did; only the operator
acknowledging an item (a reply, opening it, pressing acknowledge) stops the alerting. An
item delivered and not acknowledged moves up the ladder after ``escalate_after`` minutes,
or at once when the route knows nobody heard (desk voice with no reply), never past its
watch's urgency.
"""

from __future__ import annotations

import re
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, field
from datetime import datetime, time, timedelta
from typing import Any, Literal, Protocol

from loguru import logger

from ghost_core.models import utcnow
from ghost_core.waiting import Item, Items

RouteName = Literal["digest", "desk", "desktop", "phone", "call", "sms", "whatsapp"]
LADDER: tuple[RouteName, ...] = ("digest", "desk", "desktop", "phone", "call", "sms", "whatsapp")
CEILING = {"digest": "digest", "notify": "phone", "call": "whatsapp"}  # the loudest per urgency
AT_DESK = ("desk", "desktop")
DEFAULT_ENABLED: tuple[RouteName, ...] = ("digest", "phone")
_KEYS = {
    "enabled",
    "escalate_after",
    "quiet_hours",
    "batch_secs",
    "calls_per_hour",
    "calls_per_day",
    "carry",
    "greeting",
    "urgency",
}


class RouteError(ValueError):
    pass


@dataclass(frozen=True)
class RoutePolicy:
    enabled: tuple[RouteName, ...] = DEFAULT_ENABLED
    escalate_after: int = 15  # minutes a delivery waits for an acknowledgment
    quiet_hours: tuple[time, time] | None = None  # local times; nothing but the digest
    batch_secs: int = 60  # a new item waits this long so others can go with it
    calls_per_hour: int = 2
    calls_per_day: int = 6
    carry: Literal["count", "title"] = "count"  # what an outside route may say
    greeting: str = "Got a minute?"  # what desk voice says before anything else
    urgency: Mapping[str, str] = field(default_factory=dict)  # "eng/ci" -> digest|notify|call

    @classmethod
    def parse(cls, raw: object, where: str = "[routes]") -> RoutePolicy:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping):
            raise RouteError(f"{where} must be a table")
        unknown = sorted(set(raw) - _KEYS)
        if unknown:
            raise RouteError(f"{where}: unknown key(s) {', '.join(unknown)}")
        enabled = raw.get("enabled", list(DEFAULT_ENABLED))
        if not isinstance(enabled, list) or any(r not in LADDER for r in enabled):
            raise RouteError(f"{where}.enabled lists routes from: {', '.join(LADDER)}")
        out = cls(
            enabled=tuple(r for r in LADDER if r in enabled),  # quietest first, whatever order
            escalate_after=_whole(
                raw.get("escalate_after", 15), 1, 24 * 60, f"{where}.escalate_after"
            ),
            quiet_hours=_hours(raw.get("quiet_hours"), f"{where}.quiet_hours"),
            batch_secs=_whole(raw.get("batch_secs", 60), 0, 3600, f"{where}.batch_secs"),
            calls_per_hour=_whole(raw.get("calls_per_hour", 2), 0, 60, f"{where}.calls_per_hour"),
            calls_per_day=_whole(raw.get("calls_per_day", 6), 0, 200, f"{where}.calls_per_day"),
            carry=_one(raw.get("carry", "count"), ("count", "title"), f"{where}.carry"),
            greeting=_greeting(raw.get("greeting", "Got a minute?"), f"{where}.greeting"),
            urgency=_urgency(raw.get("urgency", {}), f"{where}.urgency"),
        )
        return out

    def to_toml(self) -> list[str]:
        """``[routes]`` as the file says it, for a policy file the API rewrites."""
        lines = ["[routes]"]
        lines.append("enabled = [" + ", ".join(f'"{r}"' for r in self.enabled) + "]")
        lines.append(f"escalate_after = {self.escalate_after}")
        if self.quiet_hours:
            a, b = self.quiet_hours
            lines.append(f'quiet_hours = "{a:%H:%M}-{b:%H:%M}"')
        lines.append(f"batch_secs = {self.batch_secs}")
        lines.append(f"calls_per_hour = {self.calls_per_hour}")
        lines.append(f"calls_per_day = {self.calls_per_day}")
        lines.append(f'carry = "{self.carry}"')
        lines.append("greeting = " + _quote(self.greeting))
        if self.urgency:
            lines += ["", "[routes.urgency]"]
            lines += [f'"{k}" = "{v}"' for k, v in self.urgency.items()]
        return lines

    def is_quiet(self, local: datetime) -> bool:
        if self.quiet_hours is None:
            return False
        a, b = self.quiet_hours
        t = local.time()
        return a <= t < b if a <= b else (t >= a or t < b)


def _whole(v: object, lo: int, hi: int, where: str) -> int:
    if isinstance(v, bool) or not isinstance(v, int) or not lo <= v <= hi:
        raise RouteError(f"{where} is a whole number from {lo} to {hi}")
    return v


def _one(v: object, allowed: Sequence[str], where: str) -> Any:
    if v not in allowed:
        raise RouteError(f"{where} is one of {', '.join(allowed)}")
    return v


def _greeting(v: object, where: str) -> str:
    if not isinstance(v, str) or not 1 <= len(v.strip()) <= 80:
        raise RouteError(f"{where} is a short phrase")
    return v.strip()


def _hours(v: object, where: str) -> tuple[time, time] | None:
    if v in (None, ""):
        return None
    m = re.fullmatch(r"(\d{1,2}):(\d{2})-(\d{1,2}):(\d{2})", str(v).replace(" ", ""))
    if not m:
        raise RouteError(f'{where} is "HH:MM-HH:MM"')
    h1, m1, h2, m2 = (int(x) for x in m.groups())
    try:
        return time(h1, m1), time(h2, m2)
    except ValueError:
        raise RouteError(f"{where} has a time that isn't one") from None


def _urgency(v: object, where: str) -> dict[str, str]:
    if not isinstance(v, Mapping) or any(u not in CEILING for u in v.values()):
        raise RouteError(f"{where} maps a department/watch to digest, notify or call")
    return {str(k): str(u) for k, u in v.items()}


def _quote(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


# --- routes --------------------------------------------------------------------------

Delivery = Literal["delivered", "not_heard", "heard", "unavailable"]


class Route(Protocol):
    """One way to reach the operator. ``deliver`` passes the items on and says what it
    knows: ``delivered`` (sent; nobody has said they heard), ``heard`` (the operator
    answered, which is the acknowledgment), ``not_heard`` (nobody answered: go on at once)
    or ``unavailable`` (it can't now: skip it this time)."""

    name: RouteName

    async def deliver(self, items: Sequence[Item], policy: RoutePolicy) -> Delivery: ...


# --- the engine ------------------------------------------------------------------------


@dataclass
class Plan:
    route: RouteName
    items: list[Item]


class RouteEngine:
    """Decides, each tick, which unacknowledged items go out by which route, and records
    every delivery. ``urgency_of`` gives an item's declared urgency (its watch's or hook's,
    else ``notify``); ``at_desk`` says whether the operator is at the desk now; ``local``
    turns a time into the operator's local time for quiet hours."""

    def __init__(
        self,
        items: Items,
        *,
        policy: RoutePolicy | None = None,
        routes: Mapping[str, Route] | None = None,
        urgency_of: Callable[[Item], str] = lambda _i: "notify",
        at_desk: Callable[[], bool] = lambda: False,
        local: Callable[[datetime], datetime] = lambda t: t.astimezone(),
        on_change: Callable[[], None] | None = None,
    ) -> None:
        self.items = items
        self.policy = policy or RoutePolicy()
        self.routes: dict[str, Route] = dict(routes or {})
        self._urgency_of = urgency_of
        self._at_desk = at_desk
        self._local = local
        self._on_change = on_change or (lambda: None)
        self.dnd_until: datetime | None = None
        self.calls: list[datetime] = []  # when calls went out, for the cap
        self.log: list[tuple[datetime, str, str, str]] = []  # (at, key, route, what)
        self._not_heard: set[str] = set()  # delivered, and the route knows nobody heard

    def add_route(self, route: Route) -> None:
        self.routes[route.name] = route

    def set_policy(self, policy: RoutePolicy) -> None:
        self.policy = policy

    def quiet_until(self, until: datetime | None) -> None:
        """Do-not-disturb, from the operator's own words (a fixed intent)."""
        self.dnd_until = until

    # --- what is due --------------------------------------------------------------------

    def ladder(self, item: Item, now: datetime) -> list[RouteName]:
        """The enabled routes this item may use now, quietest first, without the digest."""
        urgency = self.policy.urgency.get(_short(item.source), self._urgency_of(item))
        top = LADDER.index(CEILING.get(urgency, "phone"))  # type: ignore[arg-type]
        out: list[RouteName] = []
        for r in LADDER[1 : top + 1]:
            if r not in self.policy.enabled or (r != "digest" and r not in self.routes):
                continue
            if r == "call" and not self._calls_left(now):
                continue
            out.append(r)
        if not self._at_desk():
            out = [r for r in out if r not in AT_DESK]
        return out

    def _calls_left(self, now: datetime) -> bool:
        self.calls = [t for t in self.calls if now - t < timedelta(days=1)]
        hour = sum(1 for t in self.calls if now - t < timedelta(hours=1))
        return hour < self.policy.calls_per_hour and len(self.calls) < self.policy.calls_per_day

    def quiet(self, now: datetime) -> bool:
        if self.dnd_until is not None and now < self.dnd_until:
            return True
        return self.policy.is_quiet(self._local(now))

    def due(self, now: datetime | None = None) -> list[Plan]:
        """Which items go out by which route now. Held for the digest when it is quiet,
        when no louder route is on, or while a new item waits out the batch window."""
        now = now or utcnow()
        if self.quiet(now):
            return []
        plans: dict[RouteName, list[Item]] = {}
        for item in self.items.open(response="tell"):
            if not item.needs_alerting(now):
                continue
            if (now - item.changed_at).total_seconds() < self.policy.batch_secs:
                continue
            ladder = self.ladder(item, now)
            if not ladder:
                continue
            nxt = self._next(item, ladder, now)
            if nxt is not None:
                plans.setdefault(nxt, []).append(item)
        return [Plan(r, plans[r]) for r in LADDER if r in plans]

    def _next(self, item: Item, ladder: list[RouteName], now: datetime) -> RouteName | None:
        last = item.delivered_via if item.state(now) == "delivered" else None
        if last is None or last == "digest":
            return ladder[0]
        waited = item.delivered_at is not None and now - item.delivered_at >= timedelta(
            minutes=self.policy.escalate_after
        )
        if not waited and item.key not in self._not_heard:
            return None  # still waiting for the operator to acknowledge it
        louder = [r for r in ladder if LADDER.index(r) > LADDER.index(last)]  # type: ignore[arg-type]
        return louder[0] if louder else None  # the top of the ladder: no more

    async def tick(self, now: datetime | None = None) -> list[tuple[RouteName, Delivery]]:
        """Send what is due. A route that says nobody heard hands its items on at once."""
        now = now or utcnow()
        done: list[tuple[RouteName, Delivery]] = []
        for _ in range(len(LADDER)):  # at most one pass per route: "not heard" moves up
            plans = self.due(now)
            if not plans:
                break
            moved = False
            for plan in plans:
                route = self.routes.get(plan.route)
                if route is None:
                    continue
                try:
                    what = await route.deliver(plan.items, self.policy)
                except Exception as e:  # noqa: BLE001 - one route failing is not the end
                    logger.warning(f"routes: {plan.route} failed: {type(e).__name__}")
                    what = "unavailable"
                done.append((plan.route, what))
                if plan.route == "call" and what != "unavailable":
                    self.calls.append(now)
                for item in plan.items:
                    self.log.append((now, item.key, plan.route, what))
                    if what == "unavailable":
                        continue
                    self.items.delivered(item.key, plan.route, now)
                    self._not_heard.discard(item.key)
                    if what == "heard":
                        self.items.acknowledge(item.key, now)
                    elif what == "not_heard":
                        self._not_heard.add(item.key)  # nobody heard: move on now
                        moved = True
            self._on_change()
            if not moved:
                break
        return done

    # --- the digest ---------------------------------------------------------------------

    def digest(self, now: datetime | None = None) -> list[Item]:
        """What the next wake should say before anything else: every open item that still
        needs the operator to hear it, oldest first."""
        now = now or utcnow()
        return [i for i in self.items.open(response="tell") if i.needs_alerting(now)]

    def heard_in_digest(self, items: Sequence[Item], now: datetime | None = None) -> None:
        """Ghost said them at the wake: delivered by the digest and heard."""
        now = now or utcnow()
        for i in items:
            self.items.delivered(i.key, "digest", now)
            self.items.acknowledge(i.key, now)
            self.log.append((now, i.key, "digest", "heard"))
        if items:
            self._on_change()


def _short(source: str) -> str:
    """``watch:eng/ci`` and ``hook:eng/ci`` are ``eng/ci`` in ``[routes.urgency]``."""
    return source.split(":", 1)[1] if ":" in source else source


def spoken_digest(items: Sequence[Item], *, most: int = 3) -> str:
    """The digest as Ghost says it at a bare wake: a count, then up to three titles. The
    titles come from outside systems; they are read as they are, never acted on."""
    if not items:
        return ""
    titles = [_clip(i.title) for i in items[:most]]
    head = "One thing is waiting" if len(items) == 1 else f"{_count(len(items))} things are waiting"
    rest = "" if len(items) <= most else f", and {_count(len(items) - most).lower()} more"
    return f"Hi. {head}: " + "; ".join(titles) + rest + "."


def _clip(t: str, n: int = 90) -> str:
    t = t.removeprefix("Waiting for your yes: ").rstrip(".")
    return t if len(t) <= n else t[: n - 1] + "…"


def _count(n: int) -> str:
    words = ["No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten"]
    return words[n] if n < len(words) else str(n)


# --- do not disturb, from the operator's words ------------------------------------------

_NUM = {
    "one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7, "eight": 8,
    "nine": 9, "ten": 10, "eleven": 11, "twelve": 12, "an": 1, "a": 1, "half": 0,
}  # fmt: skip
_QUIET = re.compile(
    r"^(?:ghost )?(?:nothing|no interruptions|no notifications|don'?t disturb me|do not disturb"
    r"(?: me)?|quiet|hold everything|no calls)\s+(until|till|til|for)\s+(.+)$"
)


def parse_quiet(text: str, now: datetime) -> datetime | None:
    """ "Nothing until two", "do not disturb for an hour", "quiet till 3:30 pm": when the
    quiet ends, in ``now``'s timezone, or None when the words aren't that."""
    t = re.sub(r"[^a-z0-9: ']", " ", text.lower()).strip()
    t = re.sub(r"\s+", " ", t).removeprefix("okay ").removeprefix("ok ")
    m = _QUIET.match(t)
    if not m:
        return None
    how, rest = m.groups()
    if how == "for":
        d = re.fullmatch(r"(\d+|\w+) (hour|hours|minute|minutes)", rest)
        if rest in ("an hour", "a hour", "one hour"):
            return now + timedelta(hours=1)
        if rest in ("half an hour", "a half hour"):
            return now + timedelta(minutes=30)
        if not d:
            return None
        n = int(d[1]) if d[1].isdigit() else _NUM.get(d[1])
        if not n:
            return None
        return now + (timedelta(hours=n) if d[2].startswith("hour") else timedelta(minutes=n))
    return _next_clock(rest, now)


def _next_clock(rest: str, now: datetime) -> datetime | None:
    rest = rest.replace("o'clock", "").replace("oclock", "").strip()
    if rest in ("noon", "midday"):
        h, mins, half = 12, 0, "pm"
    elif rest == "midnight":
        h, mins, half = 0, 0, "am"
    else:
        m = re.fullmatch(r"(\d{1,2})(?::(\d{2}))?\s*(am|pm|a m|p m)?", rest) or re.fullmatch(
            r"(\w+)(?: (thirty|fifteen|forty five|forty-five))?\s*(am|pm|a m|p m)?", rest
        )
        if not m:
            return None
        hour_s, min_s, half_s = m.groups()
        h = int(hour_s) if hour_s.isdigit() else _NUM.get(hour_s, -1)
        if not 0 <= h <= 23:
            return None
        if min_s is None:
            mins = 0
        elif min_s.isdigit():
            mins = int(min_s)
        else:
            mins = {"thirty": 30, "fifteen": 15, "forty five": 45, "forty-five": 45}[min_s]
        half = (half_s or "").replace(" ", "")
    candidates = []
    if half == "pm":
        candidates = [h if h == 12 else h + 12]
    elif half == "am":
        candidates = [0 if h == 12 else h]
    elif h <= 12:
        candidates = [h % 12, h % 12 + 12]
    else:
        candidates = [h]
    best = None
    for hh in candidates:
        at = now.replace(hour=hh, minute=mins, second=0, microsecond=0)
        if at <= now:
            at += timedelta(days=1)
        if best is None or at < best:
            best = at
    return best


QuietSetter = Callable[[datetime], Awaitable[None]]
