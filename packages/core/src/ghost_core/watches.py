"""Watches (ADR 0028): a department observes an outside system, deterministically first.

A watch is declared by the operator in ``department.json`` under ``watches`` and never
written by the model. A polled watch calls one of a connected app's read-only tools, on an
interval, from the daemon (ADR 0034); no model is involved in that. What comes back is untrusted
data: the watch keeps only the fields it names (a path, a type, a constraint) and drops
the rest, then decides in declared steps, cheapest first:

1. ``when``: a deterministic check over the kept fields. Most watches stop here.
2. ``jev``: a typed question to Jev over the fields the watch lets leave the machine;
   the answer must be one of the watch's declared choices, else the declared default.
3. ``model``: the same, asked of a model, only when Jev isn't declared or couldn't say.

A choice is a response: ``tell`` (the waiting list), ``record`` (kept, not alerted) or
``drop`` (not an item). No step can pick anything else or grant anything. Every item has
a stable key from a template over the fields, so one failure is one item.

Example::

    "watches": {
      "ci": {
        "app": "github", "read": "list_workflow_runs",
        "args": {"owner": "heka-vault", "repo": "ghost", "status": "failure"},
        "every": 60, "items": "workflow_runs",
        "fields": {
          "id": {"at": "id", "type": "int"},
          "branch": {"at": "head_branch", "type": "str", "pattern": "^[\\\\w./-]+$"},
          "name": {"at": "name", "type": "str", "max": 120},
          "url": {"at": "html_url", "type": "str", "pattern": "^https://github\\\\.com/"}
        },
        "when": [{"field": "branch", "op": "==", "value": "main"}],
        "key": "ci:{id}", "title": "CI failed on {branch}: {name}", "link": "{url}",
        "response": "tell"
      }
    }
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import re
import time
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Any, Literal

from loguru import logger

from ghost_core.models import utcnow
from ghost_core.waiting import Item, ItemDraft, Items, Scalar

Choice = Literal["tell", "record", "drop"]
CHOICES: tuple[Choice, ...] = ("tell", "record", "drop")
OPS = ("==", "!=", "in", "not_in", ">", ">=", "<", "<=", "exists")
TYPES = ("str", "int", "float", "bool")
MIN_EVERY = 15  # seconds between polls, at the least
MAX_ITEMS = 100  # a watch looks at this many results per poll, at most
MAX_ASKED = 10  # Jev or model questions per poll; the rest take the default
STR_MAX = 200
_PLACEHOLDER = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")
_PATH = re.compile(r"^[A-Za-z0-9_\-]+(\.[A-Za-z0-9_\-]+)*$")
_WATCH_KEYS = {
    "app",
    "server",  # the older word for app (ADR 0034), still read
    "read",
    "args",
    "every",
    "items",
    "fields",
    "when",
    "for_minutes",
    "key",
    "title",
    "link",
    "response",
    "urgency",
    "jev",
    "model",
}


class WatchError(ValueError):
    """A watch declaration that does not read. Reported on the department."""


# --- the declaration ----------------------------------------------------------------


@dataclass(frozen=True)
class FieldSpec:
    at: str  # a dotted path into one result; a number is a list index
    type: str = "str"
    pattern: re.Pattern[str] | None = None
    max: int = STR_MAX
    enum: tuple[Scalar, ...] | None = None

    def take(self, obj: Any) -> Scalar:
        """The value at ``at``, typed and constrained; ``None`` when missing. A value that
        breaks a constraint raises, and the whole result is dropped."""
        v = _at(obj, self.at)
        if v is None:
            return None
        if self.type == "str":
            if isinstance(v, bool | dict | list):
                raise WatchError(f"{self.at}: not text")
            out: Scalar = " ".join(str(v).split())[: self.max]
            if self.pattern is not None and not self.pattern.fullmatch(str(out)):
                raise WatchError(f"{self.at}: does not match its pattern")
        elif self.type == "int":
            if isinstance(v, bool) or not isinstance(v, int | str):
                raise WatchError(f"{self.at}: not a whole number")
            try:
                out = int(v)
            except ValueError:
                raise WatchError(f"{self.at}: not a whole number") from None
        elif self.type == "float":
            if isinstance(v, bool) or not isinstance(v, int | float | str):
                raise WatchError(f"{self.at}: not a number")
            try:
                out = float(v)
            except ValueError:
                raise WatchError(f"{self.at}: not a number") from None
        else:
            if not isinstance(v, bool):
                raise WatchError(f"{self.at}: not true or false")
            out = v
        if self.enum is not None and out not in self.enum:
            raise WatchError(f"{self.at}: not one of its values")
        return out


@dataclass(frozen=True)
class Check:
    field: str
    op: str
    value: Any = None

    def holds(self, fields: Mapping[str, Scalar]) -> bool:
        v = fields.get(self.field)
        if self.op == "exists":
            return (v is not None) is (self.value is not False)
        if v is None:
            return False
        try:
            if self.op == "==":
                return bool(v == self.value)
            if self.op == "!=":
                return bool(v != self.value)
            if self.op == "in":
                return v in self.value
            if self.op == "not_in":
                return v not in self.value
            if self.op == ">":
                return bool(v > self.value)
            if self.op == ">=":
                return bool(v >= self.value)
            if self.op == "<":
                return bool(v < self.value)
            if self.op == "<=":
                return bool(v <= self.value)
        except TypeError:
            return False
        return False


@dataclass(frozen=True)
class Ask:
    """A Jev or model step: a question, the fields that may leave the machine for it, the
    choices it may give (each with what it means) and the default when it can't say."""

    question: str
    fields: tuple[str, ...]
    choices: Mapping[Choice, str]
    default: Choice


@dataclass(frozen=True)
class Watch:
    name: str
    department: str
    app: str
    read: str
    args: Mapping[str, Any]
    every: int
    items: str
    fields: Mapping[str, FieldSpec]
    key: str
    title: str
    link: str | None = None
    when: tuple[Check, ...] = ()
    for_minutes: int = 0
    response: Choice = "tell"
    urgency: str = "notify"  # for the route policy (ADR 0031)
    jev: Ask | None = None
    model: Ask | None = None

    @property
    def source(self) -> str:
        """Its items' source, as the waiting list knows it."""
        return f"watch:{self.department}/{self.name}"

    def look(self, result: Any) -> tuple[list[dict[str, Scalar]], bool]:
        """The kept fields of each result, and whether that is every result the app sent.
        A result that breaks a constraint, or lacks a field the key or title names, is
        dropped whole, as is anything past ``MAX_ITEMS``; the rest of what the app sent is
        never kept. A result with no list at ``items`` is a read that failed (an error
        that doesn't say so): ``WatchError``."""
        rows = result if not self.items else _at(result, self.items)
        if isinstance(rows, Mapping):
            rows = [rows]
        if not isinstance(rows, list):
            raise WatchError(f"the result has no {self.items or 'items'}")
        named = set(_PLACEHOLDER.findall(self.key)) | set(_PLACEHOLDER.findall(self.title))
        out: list[dict[str, Scalar]] = []
        whole = len(rows) <= MAX_ITEMS
        for row in rows[:MAX_ITEMS]:
            try:
                fields = {n: f.take(row) for n, f in self.fields.items()}
            except WatchError:
                whole = False
                continue
            if any(fields[n] is None for n in named):
                whole = False
                continue
            out.append(fields)
        return out, whole

    def observe(self, result: Any) -> list[dict[str, Scalar]]:
        """The kept fields of each result (``look`` without the rest)."""
        try:
            return self.look(result)[0]
        except WatchError:
            return []

    def checked(self, fields: Mapping[str, Scalar]) -> bool:
        return all(c.holds(fields) for c in self.when)

    def draft(self, fields: Mapping[str, Scalar], response: Choice) -> ItemDraft | None:
        key = render(self.key, fields)
        title = render(self.title, fields)
        if not key.strip() or not title.strip():
            return None
        link = render(self.link, fields) if self.link else None
        return ItemDraft(
            key=f"{self.department}:{key}"[:300],
            title=title,
            department=self.department,
            link=link if link and link.startswith(("https://", "http://")) else None,
            fields=dict(fields),
            response="tell" if response == "tell" else "record",
        )


def render(template: str, fields: Mapping[str, Scalar]) -> str:
    """``{name}`` placeholders from the kept fields, nothing else: no attribute access,
    no format specs, so a value can never reach into anything."""
    return _PLACEHOLDER.sub(
        lambda m: "" if fields.get(m[1]) is None else str(fields[m[1]]), template
    )


def _at(obj: Any, path: str) -> Any:
    if not path:
        return obj
    for part in path.split("."):
        if isinstance(obj, Mapping):
            obj = obj.get(part)
        elif isinstance(obj, list) and part.isdigit():
            i = int(part)
            obj = obj[i] if i < len(obj) else None
        else:
            return None
        if obj is None:
            return None
    return obj


def parse_watches(body: object, department: str, where: str) -> tuple[dict[str, Watch], list[str]]:
    """``watches`` from ``department.json``: the ones that read, and a problem for each
    that doesn't (that one is off)."""
    if body is None:
        return {}, []
    if not isinstance(body, Mapping):
        return {}, [f"{where}: watches must be an object keyed by watch name"]
    out: dict[str, Watch] = {}
    problems: list[str] = []
    for name, spec in body.items():
        try:
            out[str(name)] = parse_watch(str(name), spec, department, f"{where}: watch {name}")
        except WatchError as e:
            problems.append(f"{e}; the watch is off")
    return out, problems


def parse_watch(name: str, body: object, department: str, where: str) -> Watch:
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,40}", name):
        raise WatchError(f"{where}: a watch name is lowercase letters, digits, - and _")
    if not isinstance(body, Mapping):
        raise WatchError(f"{where}: must be an object")
    unknown = sorted(set(body) - _WATCH_KEYS)
    if unknown:
        raise WatchError(f"{where}: unknown key(s) {', '.join(unknown)}")
    app, read = body.get("app", body.get("server")), body.get("read")
    if not isinstance(app, str) or not app or not isinstance(read, str) or not read:
        raise WatchError(f"{where}: app and read are required")
    args = body.get("args", {})
    if not isinstance(args, Mapping):
        raise WatchError(f"{where}: args must be an object")
    every = body.get("every", 60)
    if isinstance(every, bool) or not isinstance(every, int) or not MIN_EVERY <= every <= 86400:
        raise WatchError(f"{where}: every is whole seconds from {MIN_EVERY} to 86400")
    items = body.get("items", "")
    if not isinstance(items, str) or (items and not _PATH.match(items)):
        raise WatchError(f"{where}: items is a dotted path")
    fields = _parse_fields(body.get("fields"), where)
    when = tuple(_parse_check(c, fields, f"{where}: when") for c in _list(body.get("when"), where))
    for_minutes = body.get("for_minutes", 0)
    if isinstance(for_minutes, bool) or not isinstance(for_minutes, int) or for_minutes < 0:
        raise WatchError(f"{where}: for_minutes is a whole number of minutes")
    key, title, link = body.get("key"), body.get("title"), body.get("link")
    for label, t in (("key", key), ("title", title)):
        if not isinstance(t, str) or not t.strip():
            raise WatchError(f"{where}: {label} is required")
    if link is not None and not isinstance(link, str):
        raise WatchError(f"{where}: link is a template")
    for label, t in (("key", key), ("title", title), ("link", link or "")):
        for ref in _PLACEHOLDER.findall(str(t)):
            if ref not in fields:
                raise WatchError(f"{where}: {label} names {{{ref}}}, which is not a field")
    if not _PLACEHOLDER.search(str(key)):
        raise WatchError(f"{where}: key must name at least one field, or every result is one item")
    response = body.get("response", "tell")
    if response not in ("tell", "record"):
        raise WatchError(f"{where}: response is tell or record")
    urgency = body.get("urgency", "notify")
    if urgency not in ("digest", "notify", "call"):
        raise WatchError(f"{where}: urgency is digest, notify or call")
    return Watch(
        name=name,
        department=department,
        app=app,
        read=read,
        args=dict(args),
        every=every,
        items=items,
        fields=fields,
        key=str(key),
        title=str(title),
        link=link,
        when=when,
        for_minutes=for_minutes,
        response=response,
        urgency=urgency,
        jev=_parse_ask(body.get("jev"), fields, f"{where}: jev"),
        model=_parse_ask(body.get("model"), fields, f"{where}: model"),
    )


def _list(v: object, where: str) -> list[Any]:
    if v is None:
        return []
    if not isinstance(v, list):
        raise WatchError(f"{where}: when is a list of checks")
    return v


def _parse_fields(body: object, where: str) -> dict[str, FieldSpec]:
    if not isinstance(body, Mapping) or not body:
        raise WatchError(f"{where}: fields names what to keep (at least one)")
    out: dict[str, FieldSpec] = {}
    for name, spec in body.items():
        w = f"{where}: field {name}"
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,40}", str(name)):
            raise WatchError(f"{w}: a field name is a word")
        if isinstance(spec, str):
            spec = {"at": spec}
        if not isinstance(spec, Mapping):
            raise WatchError(f"{w}: must be a path or an object")
        at, typ = spec.get("at"), spec.get("type", "str")
        if not isinstance(at, str) or not _PATH.match(at):
            raise WatchError(f"{w}: at is a dotted path")
        if typ not in TYPES:
            raise WatchError(f"{w}: type is one of {', '.join(TYPES)}")
        pattern = spec.get("pattern")
        try:
            compiled = re.compile(pattern) if isinstance(pattern, str) else None
        except re.error:
            raise WatchError(f"{w}: the pattern does not compile") from None
        mx = spec.get("max", STR_MAX)
        if isinstance(mx, bool) or not isinstance(mx, int) or not 1 <= mx <= 1000:
            raise WatchError(f"{w}: max is 1 to 1000")
        enum = spec.get("enum")
        if enum is not None and not isinstance(enum, list):
            raise WatchError(f"{w}: enum is a list")
        out[str(name)] = FieldSpec(
            at=at, type=typ, pattern=compiled, max=mx, enum=tuple(enum) if enum else None
        )
    return out


def _parse_check(body: object, fields: Mapping[str, FieldSpec], where: str) -> Check:
    if not isinstance(body, Mapping):
        raise WatchError(f"{where}: a check is an object")
    f, op = body.get("field"), body.get("op", "==")
    if f not in fields:
        raise WatchError(f"{where}: {f!r} is not a field")
    if op not in OPS:
        raise WatchError(f"{where}: op is one of {' '.join(OPS)}")
    value = body.get("value")
    if op in ("in", "not_in") and not isinstance(value, list):
        raise WatchError(f"{where}: {op} takes a list")
    return Check(str(f), str(op), value)


def _parse_ask(body: object, fields: Mapping[str, FieldSpec], where: str) -> Ask | None:
    if body is None:
        return None
    if not isinstance(body, Mapping):
        raise WatchError(f"{where}: must be an object")
    q = body.get("question")
    if not isinstance(q, str) or not q.strip():
        raise WatchError(f"{where}: question is required")
    send = body.get("fields")
    if not isinstance(send, list) or not send or any(s not in fields for s in send):
        raise WatchError(f"{where}: fields lists which kept fields may leave the machine")
    choices = body.get("choices")
    if (
        not isinstance(choices, Mapping)
        or len(choices) < 2
        or any(c not in CHOICES for c in choices)
        or any(not isinstance(v, str) or not v.strip() for v in choices.values())
    ):
        raise WatchError(
            f"{where}: choices maps two or more of tell, record, drop to what each means"
        )
    default = body.get("default")
    if default not in choices:
        raise WatchError(f"{where}: default is one of its choices")
    return Ask(
        question=q.strip(),
        fields=tuple(str(s) for s in send),
        choices={c: str(v) for c, v in choices.items()},  # type: ignore[misc]
        default=default,
    )


# --- deciding ----------------------------------------------------------------------

# (question, fields that may leave, choices with their meaning) -> a choice, or None when
# it can't say. Wired by the daemon: Jev's client, a model call.
Decider = Callable[[str, Mapping[str, Scalar], Mapping[str, str]], Awaitable[str | None]]


@dataclass
class _Memo:
    choice: Choice
    by: str


class Decisions:
    """What Jev or a model said about an item, remembered while its fields stay the same,
    so a watch doesn't ask again every poll."""

    def __init__(self) -> None:
        self._memo: dict[tuple[str, str, str], _Memo] = {}

    def get(self, watch: Watch, key: str, fields: Mapping[str, Scalar]) -> _Memo | None:
        return self._memo.get((watch.source, key, _digest(fields)))

    def put(self, watch: Watch, key: str, fields: Mapping[str, Scalar], memo: _Memo) -> None:
        if len(self._memo) > 5000:
            self._memo.clear()
        self._memo[(watch.source, key, _digest(fields))] = memo


def _digest(fields: Mapping[str, Scalar]) -> str:
    raw = json.dumps(fields, sort_keys=True, default=str)
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


async def decide(
    watch: Watch,
    fields: Mapping[str, Scalar],
    *,
    jev: Decider | None,
    model: Decider | None,
    budget: list[int],
) -> tuple[Choice, str]:
    """The response for one observation that passed the check, and which step chose it.
    ``budget`` is how many questions this poll may still ask (shared, decremented).
    ``unjudged`` is the default taken because a step that could be asked wasn't (the
    budget ran out) or didn't answer; ``default`` is the default with no step to ask."""
    unjudged = False
    for step, ask, decider in (("jev", watch.jev, jev), ("model", watch.model, model)):
        if ask is None or decider is None:
            continue
        if budget[0] <= 0:
            unjudged = True
            continue
        budget[0] -= 1
        sent = {f: fields.get(f) for f in ask.fields}
        try:
            answer = await asyncio.wait_for(decider(ask.question, sent, dict(ask.choices)), 10)
        except Exception as e:  # noqa: BLE001 - a step that fails falls through
            logger.info(f"watch {watch.source}: {step} failed ({type(e).__name__})")
            answer = None
        if answer in ask.choices:
            return answer, step  # type: ignore[return-value]
        unjudged = True
    for ask in (watch.jev, watch.model):
        if ask is not None:
            return ask.default, "unjudged" if unjudged else "default"
    return watch.response, "check"


# --- running --------------------------------------------------------------------------

Reader = Callable[[Watch], Awaitable[Any]]  # the declared read, untrimmed, scope-checked


@dataclass
class WatchState:
    """What the operator sees per watch: ``ghostd check``, ``GET /departments``."""

    last_run: datetime | None = None
    last_ok: datetime | None = None
    error: str | None = None
    seconds: float | None = None
    seen: int = 0
    open: int = 0
    first_match: dict[str, datetime] = field(default_factory=dict)


class WatchRunner:
    """Runs every declared watch on its own interval, separate from the voice loop. Each
    poll: the read (no model), the kept fields, the check, the declared steps, then one
    ``Items.sync`` for the watch's source, so a key that is gone is resolved."""

    def __init__(
        self,
        items: Items,
        reader: Reader,
        *,
        jev: Decider | None = None,
        model: Decider | None = None,
        on_change: Callable[[], None] | None = None,
    ) -> None:
        self._items = items
        self._read = reader
        self._jev = jev
        self._model = model
        self._on_change = on_change or (lambda: None)
        self._decisions = Decisions()
        self._tasks: dict[str, asyncio.Task[None]] = {}
        self.watches: dict[str, Watch] = {}
        self.state: dict[str, WatchState] = {}

    def set_watches(self, watches: Sequence[Watch]) -> None:
        """The declared watches, again (a folder was re-read): new ones start, gone ones
        stop and their open items resolve, changed ones restart."""
        wanted = {w.source: w for w in watches}
        for source in set(self._tasks) | set(self.watches):
            if source in wanted and wanted[source] == self.watches.get(source):
                continue
            task = self._tasks.pop(source, None)
            if task is not None:
                task.cancel()
            if source not in wanted:
                self.watches.pop(source, None)
                self.state.pop(source, None)
                if self._items.sync(source, []).any:
                    self._on_change()
        for source, w in wanted.items():
            self.watches[source] = w
            self.state.setdefault(source, WatchState())
            if source not in self._tasks:
                self._tasks[source] = asyncio.ensure_future(self._loop(w))

    async def close(self) -> None:
        for t in self._tasks.values():
            t.cancel()
        await asyncio.gather(*self._tasks.values(), return_exceptions=True)
        self._tasks.clear()

    async def _loop(self, w: Watch) -> None:
        backoff = w.every
        while True:
            try:
                ok = await self.poll(w)
            except Exception as e:  # noqa: BLE001 - a watch that breaks says so and goes on
                st = self.state.setdefault(w.source, WatchState())
                st.error = f"{type(e).__name__}: {str(e)[:160]}"
                logger.warning(f"watch {w.source}: poll failed: {st.error}")
                ok = False
            backoff = w.every if ok else min(backoff * 2, max(w.every, 900))
            await asyncio.sleep(backoff)

    def _held(self, w: Watch, key: str) -> Item | None:
        """This watch's open item under ``key``, if it has one."""
        item = self._items.get(f"{w.department}:{key}"[:300])
        if item is None or item.source != w.source or item.resolved_at is not None:
            return None
        return item

    async def poll(self, w: Watch, now: datetime | None = None) -> bool:
        """One look. True when the read worked (the backoff resets)."""
        st = self.state.setdefault(w.source, WatchState())
        began = time.perf_counter()
        st.last_run = now or utcnow()
        try:
            result = await self._read(w)
        except Exception as e:  # noqa: BLE001 - a watch that can't read says so and waits
            st.error = str(e)[:200] or type(e).__name__
            st.seconds = time.perf_counter() - began
            logger.info(f"watch {w.source}: read failed: {st.error}")
            return False
        if isinstance(result, Mapping) and set(result) == {"error"}:
            st.error = str(result["error"])[:200]
            st.seconds = time.perf_counter() - began
            return False
        try:
            rows, whole = w.look(result)
        except WatchError as e:  # no list where the watch looks: not "nothing is wrong"
            st.error = str(e)[:200]
            st.seconds = time.perf_counter() - began
            logger.info(f"watch {w.source}: read failed: {st.error}")
            return False
        at = now or utcnow()
        drafts: list[ItemDraft] = []
        budget = [MAX_ASKED]
        matched: set[str] = set()
        for fields in rows:
            if not w.checked(fields):
                continue
            key = render(w.key, fields)
            matched.add(key)
            # An item already open that still matches stays: neither the wait (which a
            # restart begins again) nor a question that wasn't asked or answered this
            # poll is a reason to resolve it and tell the operator again later.
            held = self._held(w, key)
            first = st.first_match.setdefault(key, at)
            if w.for_minutes and at - first < timedelta(minutes=w.for_minutes):
                if held is not None and (d := w.draft(fields, held.response)) is not None:
                    drafts.append(d)
                continue
            memo = self._decisions.get(w, key, fields)
            if memo is None:
                choice, by = await decide(
                    w, fields, jev=self._jev, model=self._model, budget=budget
                )
                if by in ("jev", "model"):
                    self._decisions.put(w, key, fields, _Memo(choice, by))
                elif by == "unjudged" and held is not None:
                    choice = held.response
            else:
                choice = memo.choice
            if choice == "drop":
                continue
            d = w.draft(fields, choice)
            if d is not None:
                drafts.append(d)
        st.first_match = {k: v for k, v in st.first_match.items() if k in matched}
        # Rows dropped (past the cap, or breaking a constraint) may be the open ones: a
        # partial look adds and updates but resolves nothing.
        found = self._items.sync(w.source, drafts, at, resolve=whole)
        st.error = None
        st.last_ok = at
        st.seen = len(rows)
        st.open = len(drafts)
        st.seconds = time.perf_counter() - began
        if found.any:
            logger.info(
                f"watch {w.source}: {len(found.new)} new, {len(found.changed)} changed, "
                f"{len(found.resolved)} resolved"
            )
            self._on_change()
        return True
