"""Pushed watches: signed webhooks, parsed by declaration, checked at the source (ADR 0027).

A department declares each webhook it takes in ``department.json`` under ``hooks``. Every
step between the request and an item is deterministic and tested with recorded deliveries;
the model never sees a body:

1. **Signature first.** ``github`` checks ``X-Hub-Signature-256`` over the raw bytes;
   ``hmac`` (Ghost's own, for local senders) checks ``X-Ghost-Signature`` over
   ``<timestamp>.<delivery id>.<body>`` inside a five-minute window. The secret is
   ``${NAME}`` from the environment. A delivery id seen before is a replay.
2. **Parsed by declaration.** The event (``X-GitHub-Event`` or ``X-Ghost-Event``) must be
   declared; its fields are typed and constrained paths into the JSON body, as a watch's
   are (``ghost_core.watches``). A field that breaks its constraint rejects the delivery
   whole; everything not declared is dropped at the door. ``resolve`` checks close an
   item; ``when`` checks open one; anything else is ignored.
3. **Checked at the source.** With ``check``, the reference is read again through the
   department's declared read (its allowlist applies) and that read is what is believed;
   a read that fails or doesn't hold rejects the delivery. Without ``check`` the item is
   marked unchecked.

What is kept is the parsed fields, the delivery id and the outcome, never the body. A hook
with no fixtures, or one whose fixtures fail, stays off.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
from collections import Counter
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Literal

from loguru import logger

from ghost_core.models import utcnow
from ghost_core.store import Store
from ghost_core.waiting import ItemDraft, Items, Scalar
from ghost_core.watches import (
    _PLACEHOLDER,
    Check,
    FieldSpec,
    WatchError,
    _parse_check,
    _parse_fields,
    render,
)

Scheme = Literal["github", "hmac"]
MAX_BODY = 256 * 1024
WINDOW = timedelta(minutes=5)
DEDUPE_FOR = timedelta(days=7)
HOOKS_DIR = "hooks"  # <department>/hooks/<name>/*.json: the fixtures
H_GH_SIG, H_GH_ID, H_GH_EVENT = "x-hub-signature-256", "x-github-delivery", "x-github-event"
H_SIG, H_ID, H_TIME, H_EVENT = (
    "x-ghost-signature",
    "x-ghost-delivery",
    "x-ghost-timestamp",
    "x-ghost-event",
)
_HOOK_KEYS = {"verify", "secret", "events", "response", "urgency"}
_EVENT_KEYS = {"fields", "when", "resolve", "key", "title", "link", "check"}


class Rejected(Exception):
    """The delivery is refused; the reason is for the count and the log, never the body."""


@dataclass(frozen=True)
class ReadBack:
    server: str
    read: str
    args: Mapping[str, Any]  # values may be "{field}" templates
    fields: Mapping[str, FieldSpec]
    when: tuple[Check, ...] = ()


@dataclass(frozen=True)
class HookEvent:
    name: str
    fields: Mapping[str, FieldSpec]
    key: str
    title: str
    link: str | None = None
    when: tuple[Check, ...] = ()
    resolve: tuple[Check, ...] = ()
    check: ReadBack | None = None


@dataclass(frozen=True)
class Hook:
    name: str
    department: str
    verify: Scheme
    secret: str  # "${NAME}"
    events: Mapping[str, HookEvent]
    response: Literal["tell", "record"] = "tell"
    urgency: str = "notify"
    folder: Path | None = None  # where its fixtures are

    @property
    def source(self) -> str:
        return f"hook:{self.department}/{self.name}"

    @property
    def path(self) -> str:
        return f"/hooks/{self.department}/{self.name}"

    def secret_name(self) -> str:
        return self.secret[2:-1]


# --- the declaration --------------------------------------------------------------------


def parse_hooks(
    body: object, department: str, where: str, folder: Path | None = None
) -> tuple[dict[str, Hook], list[str]]:
    if body is None:
        return {}, []
    if not isinstance(body, Mapping):
        return {}, [f"{where}: hooks must be an object keyed by hook name"]
    out: dict[str, Hook] = {}
    problems: list[str] = []
    for name, spec in body.items():
        try:
            out[str(name)] = parse_hook(
                str(name),
                spec,
                department,
                f"{where}: hook {name}",
                folder / HOOKS_DIR / str(name) if folder else None,
            )
        except WatchError as e:
            problems.append(f"{e}; the hook is off")
    return out, problems


def parse_hook(
    name: str, body: object, department: str, where: str, folder: Path | None = None
) -> Hook:
    import re

    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,40}", name):
        raise WatchError(f"{where}: a hook name is lowercase letters, digits, - and _")
    if not isinstance(body, Mapping):
        raise WatchError(f"{where}: must be an object")
    unknown = sorted(set(body) - _HOOK_KEYS)
    if unknown:
        raise WatchError(f"{where}: unknown key(s) {', '.join(unknown)}")
    verify = body.get("verify")
    if verify not in ("github", "hmac"):
        raise WatchError(f"{where}: verify is github or hmac")
    secret = body.get("secret")
    if not isinstance(secret, str) or not re.fullmatch(r"\$\{[A-Z][A-Z0-9_]*\}", secret):
        raise WatchError(f"{where}: secret is ${{NAME}}, from the environment, never the value")
    events_raw = body.get("events")
    if not isinstance(events_raw, Mapping) or not events_raw:
        raise WatchError(f"{where}: events names at least one event it takes")
    events = {
        str(e): _parse_event(str(e), spec, f"{where}: event {e}") for e, spec in events_raw.items()
    }
    response = body.get("response", "tell")
    if response not in ("tell", "record"):
        raise WatchError(f"{where}: response is tell or record")
    urgency = body.get("urgency", "notify")
    if urgency not in ("digest", "notify", "call"):
        raise WatchError(f"{where}: urgency is digest, notify or call")
    return Hook(name, department, verify, secret, events, response, urgency, folder)


def _parse_event(name: str, body: object, where: str) -> HookEvent:
    if not isinstance(body, Mapping):
        raise WatchError(f"{where}: must be an object")
    unknown = sorted(set(body) - _EVENT_KEYS)
    if unknown:
        raise WatchError(f"{where}: unknown key(s) {', '.join(unknown)}")
    fields = _parse_fields(body.get("fields"), where)
    when = tuple(_parse_check(c, fields, f"{where}: when") for c in body.get("when") or [])
    resolve = tuple(_parse_check(c, fields, f"{where}: resolve") for c in body.get("resolve") or [])
    if not when and not resolve:
        raise WatchError(f"{where}: when or resolve says which deliveries count")
    key, title, link = body.get("key"), body.get("title"), body.get("link")
    if not isinstance(key, str) or not _PLACEHOLDER.search(key):
        raise WatchError(f"{where}: key must name at least one field")
    if not isinstance(title, str) or not title.strip():
        raise WatchError(f"{where}: title is required")
    for label, t in (("key", key), ("title", title), ("link", link or "")):
        for ref in _PLACEHOLDER.findall(str(t)):
            if ref not in fields:
                raise WatchError(f"{where}: {label} names {{{ref}}}, which is not a field")
    check = None
    raw = body.get("check")
    if raw is not None:
        if not isinstance(raw, Mapping) or not raw.get("server") or not raw.get("read"):
            raise WatchError(f"{where}: check names the server and the read")
        args = raw.get("args", {})
        if not isinstance(args, Mapping):
            raise WatchError(f"{where}: check.args must be an object")
        for v in args.values():
            for ref in _PLACEHOLDER.findall(str(v)):
                if ref not in fields:
                    raise WatchError(f"{where}: check.args names {{{ref}}}, not a field")
        rb_fields = _parse_fields(raw.get("fields"), f"{where}: check")
        rb_when = tuple(
            _parse_check(c, rb_fields, f"{where}: check.when") for c in raw.get("when") or []
        )
        check = ReadBack(str(raw["server"]), str(raw["read"]), dict(args), rb_fields, rb_when)
    return HookEvent(name, fields, key, title, link, when, resolve, check)


# --- verifying ----------------------------------------------------------------------------


@dataclass(frozen=True)
class Delivery:
    id: str
    event: str
    payload: Mapping[str, Any]


def sign_hmac(secret: str, ts: str, delivery: str, body: bytes) -> str:
    mac = hmac.new(secret.encode(), f"{ts}.{delivery}.".encode() + body, hashlib.sha256)
    return "sha256=" + mac.hexdigest()


def sign_github(secret: str, body: bytes) -> str:
    return "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()


def verify(
    hook: Hook, headers: Mapping[str, str], body: bytes, secret: str, now: datetime | None = None
) -> Delivery:
    """The signature over the raw bytes, then the JSON. Raises ``Rejected``."""
    if len(body) > MAX_BODY:
        raise Rejected("too large")
    h = {k.lower(): v for k, v in headers.items()}
    if hook.verify == "github":
        given, delivery, event = h.get(H_GH_SIG, ""), h.get(H_GH_ID, ""), h.get(H_GH_EVENT, "")
        expected = sign_github(secret, body)
    else:
        given, delivery, event = h.get(H_SIG, ""), h.get(H_ID, ""), h.get(H_EVENT, "")
        ts = h.get(H_TIME, "")
        try:
            at = datetime.fromtimestamp(int(ts), tz=(now or utcnow()).tzinfo)
        except (ValueError, OverflowError):
            raise Rejected("no timestamp") from None
        if abs((now or utcnow()) - at) > WINDOW:
            raise Rejected("outside the clock window")
        expected = sign_hmac(secret, ts, delivery, body)
    if not given or not hmac.compare_digest(given, expected):
        raise Rejected("bad signature")
    if not delivery or len(delivery) > 100:
        raise Rejected("no delivery id")
    try:
        payload = json.loads(body)
    except ValueError:
        raise Rejected("not JSON") from None
    if not isinstance(payload, Mapping):
        raise Rejected("not a JSON object")
    return Delivery(delivery, event or "event", payload)


# --- parsing and checking -----------------------------------------------------------------

Outcome = Literal["item", "resolved", "ignored"]
# (department, server, read, args) -> the read's result; raises when it can't, or when the
# reference is outside the department's allowlist.
Reader = Callable[[str, str, str, dict[str, Any]], Awaitable[Any]]


@dataclass
class Parsed:
    outcome: Outcome
    key: str = ""
    draft: ItemDraft | None = None
    checked: bool = False


def _fill(args: Mapping[str, Any], fields: Mapping[str, Scalar]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for k, v in args.items():
        if isinstance(v, str):
            whole = _PLACEHOLDER.fullmatch(v)
            out[k] = fields.get(whole[1]) if whole else render(v, fields)
        else:
            out[k] = v
    return out


def _take(specs: Mapping[str, FieldSpec], obj: Any) -> dict[str, Scalar]:
    try:
        return {n: f.take(obj) for n, f in specs.items()}
    except WatchError as e:
        raise Rejected(f"field {e}") from None


async def parse(hook: Hook, d: Delivery, read: Reader | None) -> Parsed:
    """The delivery's typed fields and what they mean. Raises ``Rejected``."""
    ev = hook.events.get(d.event)
    if ev is None:
        return Parsed("ignored")
    fields = _take(ev.fields, d.payload)
    key = f"{hook.department}:{render(ev.key, fields)}"[:300]
    if ev.resolve and all(c.holds(fields) for c in ev.resolve):
        return Parsed("resolved", key)
    if not ev.when or not all(c.holds(fields) for c in ev.when):
        return Parsed("ignored", key)
    checked = False
    if ev.check is not None:
        if read is None:
            raise Rejected("not confirmed at the source")
        try:
            result = await read(
                hook.department, ev.check.server, ev.check.read, _fill(ev.check.args, fields)
            )
        except Exception as e:  # noqa: BLE001 - outside the allowlist, a failed read
            raise Rejected(f"not confirmed at the source ({type(e).__name__})") from None
        if isinstance(result, Mapping) and set(result) == {"error"}:
            raise Rejected("not confirmed at the source")
        truth = _take(ev.check.fields, result)
        if not all(c.holds(truth) for c in ev.check.when):
            raise Rejected("the source does not say so")
        fields = {**fields, **truth}
        checked = True
    title = render(ev.title, fields)
    link = render(ev.link, fields) if ev.link else None
    draft = ItemDraft(
        key=key,
        title=title or ev.name,
        department=hook.department,
        link=link if link and link.startswith(("https://", "http://")) else None,
        fields={**fields, "checked": checked},
        response=hook.response,
    )
    return Parsed("item", key, draft, checked)


# --- the intake ---------------------------------------------------------------------------


@dataclass
class Result:
    status: int
    outcome: str  # item, resolved, ignored, or the rejection's reason


@dataclass
class HookState:
    accepted: Counter[str] = field(default_factory=Counter)
    rejected: Counter[str] = field(default_factory=Counter)
    last: datetime | None = None


class HookIntake:
    """Takes a request for ``/hooks/<department>/<name>`` and turns it into an item, or
    refuses it. The listener in ``ghostd`` hands it the path, the headers and the raw body
    and nothing else reaches the daemon."""

    def __init__(
        self,
        items: Items,
        store: Store,
        *,
        read: Reader | None = None,
        environ: Mapping[str, str] | None = None,
        on_change: Callable[[], None] | None = None,
    ) -> None:
        self._items = items
        self._store = store
        self._read = read
        self._environ = environ if environ is not None else os.environ
        self._on_change = on_change or (lambda: None)
        self.hooks: dict[str, Hook] = {}
        self.off: dict[str, str] = {}  # path -> why the hook is off
        self.state: dict[str, HookState] = {}

    def set_hooks(self, hooks: list[Hook]) -> list[str]:
        """The declared hooks, again. Each runs its fixtures first; one without fixtures,
        with failing ones, or whose secret is not set stays off. Returns the problems."""
        self.hooks, self.off = {}, {}
        problems = []
        for h in hooks:
            why = None
            if not self._environ.get(h.secret_name()):
                why = f"secret {h.secret_name()} is not set"
            else:
                failures = run_fixtures(h)
                if failures:
                    why = failures[0]
            if why:
                self.off[h.path] = why
                problems.append(f"hook {h.department}/{h.name} is off: {why}")
                continue
            self.hooks[h.path] = h
            self.state.setdefault(h.path, HookState())
        return problems

    async def receive(
        self, path: str, headers: Mapping[str, str], body: bytes, now: datetime | None = None
    ) -> Result:
        hook = self.hooks.get(path)
        if hook is None:
            return Result(404, "not found")
        st = self.state.setdefault(path, HookState())
        now = now or utcnow()
        try:
            d = verify(hook, headers, body, self._environ.get(hook.secret_name(), ""), now)
            if not self._store.first_delivery(hook.source, d.id, now, keep=DEDUPE_FOR):
                raise Rejected("replayed")
            parsed = await parse(hook, d, self._read)
        except Rejected as e:
            st.rejected[str(e)] += 1
            logger.info(f"hook {hook.department}/{hook.name}: refused ({e})")
            return Result(
                401 if str(e) in ("bad signature", "outside the clock window") else 400, str(e)
            )
        st.accepted[parsed.outcome] += 1
        st.last = now
        self._store.note_delivery(hook.source, d.id, parsed.outcome, parsed.key, parsed.checked)
        if parsed.outcome == "item" and parsed.draft is not None:
            self._items.observe(hook.source, parsed.draft, now)
            self._on_change()
        elif parsed.outcome == "resolved" and parsed.key:
            if self._items.resolve(parsed.key, now):
                self._on_change()
        return Result(202, parsed.outcome)


# --- fixtures -------------------------------------------------------------------------------

FIXTURE_SECRET = "fixture-secret"


def run_fixtures(hook: Hook, now: datetime | None = None) -> list[str]:
    """Run the recorded deliveries beside the declaration: each says what must happen
    (``item`` with its key and title, ``resolved``, ``ignored``, or ``rejected`` with a
    reason). They are signed here with a fixture secret, so no real secret is needed;
    ``sign`` may be ``bad`` or ``none``, ``age_secs`` makes a stale one, ``replay`` sends it
    twice, ``readback`` is the result the source's read gives. Returns what failed."""
    import asyncio

    folder = hook.folder
    files = sorted(folder.glob("*.json")) if folder is not None and folder.is_dir() else []
    if not files:
        return [f"no fixtures in {folder}: add recorded deliveries, good and bad"]
    failures: list[str] = []
    for f in files:
        try:
            spec = json.loads(f.read_text())
            if _in_loop():  # the daemon re-reads folders inside its loop
                got = _run_in_thread(hook, spec, now)
            else:
                got = asyncio.run(_fixture(hook, spec, now))
            want = spec.get("expect", {})
            problem = _compare(want, got)
        except Exception as e:  # noqa: BLE001 - a broken fixture is a failure, not a crash
            problem = f"could not run ({type(e).__name__}: {e})"
        if problem:
            failures.append(f"fixture {f.name}: {problem}")
    return failures


def _in_loop() -> bool:
    import asyncio

    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return False
    return True


def _run_in_thread(hook: Hook, spec: Mapping[str, Any], now: datetime | None) -> dict[str, Any]:
    import asyncio
    import concurrent.futures

    with concurrent.futures.ThreadPoolExecutor(1) as pool:
        return pool.submit(lambda: asyncio.run(_fixture(hook, spec, now))).result()


async def _fixture(hook: Hook, spec: Mapping[str, Any], now: datetime | None) -> dict[str, Any]:
    now = now or utcnow()
    body_raw = spec.get("body", {})
    body = body_raw.encode() if isinstance(body_raw, str) else json.dumps(body_raw).encode()
    headers = {k.lower(): str(v) for k, v in (spec.get("headers") or {}).items()}
    delivery = headers.get(H_GH_ID if hook.verify == "github" else H_ID, "fixture-1")
    sign = spec.get("sign", "valid")
    if hook.verify == "github":
        headers.setdefault(H_GH_ID, delivery)
        if sign != "none":
            secret = FIXTURE_SECRET if sign == "valid" else "wrong"
            headers[H_GH_SIG] = sign_github(secret, body)
    else:
        ts = str(int((now - timedelta(seconds=int(spec.get("age_secs", 0)))).timestamp()))
        headers.setdefault(H_ID, delivery)
        headers[H_TIME] = ts
        if sign != "none":
            secret = FIXTURE_SECRET if sign == "valid" else "wrong"
            headers[H_SIG] = sign_hmac(secret, ts, headers[H_ID], body)
    readback = spec.get("readback")

    async def read(_d: str, _s: str, _r: str, args: dict[str, Any]) -> Any:
        if readback is None:
            raise LookupError("no readback in the fixture")
        if isinstance(readback, Mapping) and readback.get("refuse"):
            raise ValueError("outside the allowlist")
        return readback

    store = Store()
    intake = HookIntake(
        Items(store), store, read=read, environ={hook.secret_name(): FIXTURE_SECRET}
    )
    intake.hooks[hook.path] = hook
    res = await intake.receive(hook.path, headers, body, now)
    if spec.get("replay"):
        res = await intake.receive(hook.path, headers, body, now)
    out: dict[str, Any] = {"status": res.status, "outcome": res.outcome}
    items = intake._items.open()  # noqa: SLF001
    if items:
        out["key"], out["title"], out["fields"] = items[0].key, items[0].title, items[0].fields
    store.close()
    return out


def _compare(want: Mapping[str, Any], got: Mapping[str, Any]) -> str | None:
    outcome = want.get("outcome")
    if outcome == "rejected":
        if got["status"] < 400:
            return f"expected a rejection, got {got['outcome']}"
        reason = want.get("reason")
        if reason and reason not in got["outcome"]:
            return f"expected the reason {reason!r}, got {got['outcome']!r}"
        return None
    if got["status"] >= 400:
        return f"expected {outcome}, was refused: {got['outcome']}"
    if outcome != got["outcome"]:
        return f"expected {outcome}, got {got['outcome']}"
    for k in ("key", "title"):
        if k in want and want[k] != got.get(k):
            return f"expected {k} {want[k]!r}, got {got.get(k)!r}"
    for k, v in (want.get("fields") or {}).items():
        if (got.get("fields") or {}).get(k) != v:
            return f"expected field {k}={v!r}, got {(got.get('fields') or {}).get(k)!r}"
    return None
