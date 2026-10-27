"""The catalog of apps the operator can connect (ADR 0034), from the official MCP registry.

The registry asks host applications not to query it live: "aggregators" keep their own copy
and refresh it about hourly. ``ghostd`` is its own small aggregator: ``Catalog`` keeps the
copy in a file, fetches it whole the first time and then only what changed
(``updated_since``), at most once an hour, when the Apps screen asks. Entries that are not
active, or have no way to connect that Ghost can start, are left out.

Each entry offers the ways it can be connected (``ConnectOption``):

- ``sign_in``: a remote app with no secret to type in; Ghost signs in to it (OAuth) when it
  asks, through ``ghost_core.signin``;
- ``key``: a remote app with a secret header or URL setting (an API key, a token);
- ``local``: a package Ghost starts on this computer: ``npx`` for npm, ``uvx`` for PyPI,
  ``docker`` for a container image, with its settings in its environment.

``connect`` turns an entry, one of its options and the operator's values into an
``AppSpec`` for ``apps.json`` and the secrets to put in Ghost's environment: a secret is
never written into the file, only ``${NAME}``. Catalog text is shown to the operator only;
none of it reaches the model.
"""

from __future__ import annotations

import asyncio
import json
import re
import time
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field, replace
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Literal
from urllib.parse import quote, urlparse

from loguru import logger

from ghost_core.apps import APP_NAME_RE, AppSpec, secret_name

REGISTRY = "https://registry.modelcontextprotocol.io"
MAX_AGE_SECS = 3600.0  # the registry's advice: copy about once an hour
PAGE = 100
MAX_PAGES = 2000  # a full copy stops here (200,000 records; the registry had ~25,000 in 2026-10)
SAVE_EVERY = 20  # pages: a first copy is searchable while it is still coming in
FETCH_TIMEOUT_SECS = 20.0
# First-party apps, shown before anything else when the operator has not searched. The
# registry is open to anyone, so a search for "github" finds wrappers before GitHub's own.
FEATURED = (
    "io.github.github/github-mcp-server",
    "app.linear/linear",
    "com.notion/mcp",
    "com.atlassian/atlassian-mcp-server",
    "io.github.getsentry/sentry-mcp",
    "com.figma.mcp/mcp",
    "com.stripe/mcp",
    "com.cloudflare.mcp/mcp",
    "com.supabase/mcp",
    "com.airtable/mcp",
    "net.todoist/mcp",
    "com.zapier/mcp",
)
_STATUS = "io.modelcontextprotocol.registry/official"
_VAR = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")
_RUNNERS = {"npm": "npx", "pypi": "uvx", "oci": "docker"}
_REMOTE_TYPES = {"streamable-http", "http"}

Fetch = Callable[[str], Awaitable[Mapping[str, Any]]]
Kind = Literal["sign_in", "key", "local"]


class CatalogError(ValueError):
    """The entry or the values can't make an app. Safe to show the operator."""


@dataclass(frozen=True)
class Setting:
    """One thing the operator fills in to connect: a header, an environment variable, a
    part of the URL or of an argument."""

    key: str  # the header, variable or placeholder name
    label: str
    secret: bool = False
    required: bool = True
    default: str | None = None
    choices: tuple[str, ...] = ()
    where: Literal["header", "env", "url", "arg"] = "env"


@dataclass(frozen=True)
class ConnectOption:
    id: str  # "remote-0", "package-1"
    kind: Kind
    label: str
    settings: tuple[Setting, ...] = ()
    url: str | None = None
    headers: tuple[tuple[str, str], ...] = ()  # name, value template ("" when typed in)
    runner: str | None = None  # npx, uvx, docker
    package: str | None = None  # the identifier with its version
    runtime_args: tuple[str, ...] = ()  # templates for the runner, before the package
    package_args: tuple[str, ...] = ()  # templates for the package, after it


@dataclass(frozen=True)
class CatalogEntry:
    name: str  # reverse-DNS, as the registry names it
    title: str
    description: str
    version: str
    website: str | None = None
    repository: str | None = None
    icon: str | None = None
    options: tuple[ConnectOption, ...] = ()
    featured: bool = False

    @property
    def app_name(self) -> str:
        """A short name for ``apps.json`` and the model's tool names: "github", "linear"."""
        return suggest_name(self)


# --- reading an entry ---------------------------------------------------------------


def _str(v: object) -> str:
    return v if isinstance(v, str) else ""


def _settings_of(inputs: object, where: Literal["header", "env"]) -> list[Setting]:
    out: list[Setting] = []
    for raw in inputs if isinstance(inputs, list) else []:
        if not isinstance(raw, Mapping) or not _str(raw.get("name")):
            continue
        value = _str(raw.get("value"))
        if value and not _VAR.search(value):
            continue  # fixed by the entry, nothing to type in
        out.append(
            Setting(
                key=str(raw["name"]),
                label=_str(raw.get("description")) or str(raw["name"]),
                secret=bool(raw.get("isSecret")),
                required=bool(raw.get("isRequired", where == "header")),
                default=_str(raw.get("default")) or None,
                choices=tuple(str(c) for c in raw.get("choices") or [] if isinstance(c, str)),
                where=where,
            )
        )
    return out


def _variables(
    raw: Mapping[str, Any], template: str, where: Literal["url", "arg"]
) -> list[Setting]:
    found = _VAR.findall(template)
    if not found:
        return []
    declared = raw.get("variables") if isinstance(raw.get("variables"), Mapping) else {}
    out = []
    for name in dict.fromkeys(found):
        v = declared.get(name, {}) if isinstance(declared, Mapping) else {}
        v = v if isinstance(v, Mapping) else {}
        out.append(
            Setting(
                key=name,
                label=_str(v.get("description")) or name.replace("_", " "),
                secret=bool(v.get("isSecret")),
                required=bool(v.get("isRequired", True)),
                default=_str(v.get("default")) or None,
                choices=tuple(str(c) for c in v.get("choices") or [] if isinstance(c, str)),
                where=where,
            )
        )
    return out


def _arguments(raw: object) -> tuple[list[str], list[Setting]]:
    """``runtimeArguments`` or ``packageArguments`` as argument templates."""
    args: list[str] = []
    settings: list[Setting] = []
    for a in raw if isinstance(raw, list) else []:
        if not isinstance(a, Mapping):
            continue
        value = _str(a.get("value")) or _str(a.get("default"))
        if not value and a.get("isRequired"):
            # A value the operator must give, with no template: one placeholder for it.
            key = re.sub(r"\W+", "_", _str(a.get("valueHint")) or _str(a.get("name")) or "value")
            key = key.strip("_") or "value"
            value = "{" + key + "}"
            settings.append(
                Setting(
                    key=key,
                    label=_str(a.get("description")) or key,
                    secret=bool(a.get("isSecret")),
                    where="arg",
                )
            )
        elif not value:
            continue
        else:
            settings.extend(_variables(a, value, "arg"))
        if a.get("type") == "named" and _str(a.get("name")):
            args.append(str(a["name"]))
        args.append(value)
    return args, settings


def parse_entry(raw: Mapping[str, Any]) -> CatalogEntry | None:
    """One registry record (``{"server": …, "_meta": …}``), or None when it isn't active or
    has no way to connect that Ghost can start."""
    server = raw.get("server", raw)
    if not isinstance(server, Mapping) or not _str(server.get("name")):
        return None
    meta = raw.get("_meta", {}) if isinstance(raw.get("_meta"), Mapping) else {}
    status = meta.get(_STATUS, {}) if isinstance(meta.get(_STATUS), Mapping) else {}
    if status.get("status", "active") != "active":
        return None
    options: list[ConnectOption] = []
    for i, r in enumerate(server.get("remotes") or []):
        if (
            not isinstance(r, Mapping)
            or r.get("type") not in _REMOTE_TYPES
            or not _str(r.get("url"))
        ):
            continue
        url = str(r["url"])
        settings = _variables(r, url, "url") + _settings_of(r.get("headers"), "header")
        headers = tuple(
            (str(h["name"]), _str(h.get("value")))
            for h in r.get("headers") or []
            if isinstance(h, Mapping) and _str(h.get("name"))
        )
        typed = [s for s in settings if s.required or s.secret]
        kind: Kind = "key" if any(s.secret for s in typed) else "sign_in"
        label = "Use an API key" if kind == "key" else "Sign in"
        if url.rstrip("/").endswith("readonly"):
            label += " (read only)"
        options.append(
            ConnectOption(
                id=f"remote-{i}",
                kind=kind,
                label=label,
                settings=tuple(settings),
                url=url,
                headers=headers,
            )
        )
    for i, p in enumerate(server.get("packages") or []):
        if not isinstance(p, Mapping):
            continue
        runner = _RUNNERS.get(_str(p.get("registryType")))
        transport = p.get("transport", {}) if isinstance(p.get("transport"), Mapping) else {}
        if runner is None or transport.get("type", "stdio") != "stdio":
            continue
        ident, version = _str(p.get("identifier")), _str(p.get("version"))
        if not ident:
            continue
        if runner == "npx":
            package = f"{ident}@{version}" if version else ident
        elif runner == "uvx":
            package = f"{ident}=={version}" if version else ident
        else:
            package = (
                ident if ":" in ident.rsplit("/", 1)[-1] or not version else f"{ident}:{version}"
            )
        run_args, run_settings = _arguments(p.get("runtimeArguments"))
        pkg_args, pkg_settings = _arguments(p.get("packageArguments"))
        env = _settings_of(p.get("environmentVariables"), "env")
        where = {"npx": "npm", "uvx": "Python", "docker": "a container"}[runner]
        options.append(
            ConnectOption(
                id=f"package-{i}",
                kind="local",
                label=f"Run on this computer ({where})",
                settings=tuple(env + run_settings + pkg_settings),
                runner=runner,
                package=package,
                runtime_args=tuple(run_args),
                package_args=tuple(pkg_args),
            )
        )
    if not options:
        return None
    _tell_apart(options)
    icons = server.get("icons") or []
    icon = next(
        (
            str(i["src"])
            for i in icons
            if isinstance(i, Mapping) and _str(i.get("src")).startswith("https://")
        ),
        None,
    )
    repo = server.get("repository")
    name = str(server["name"])
    return CatalogEntry(
        name=name,
        title=_plain(_str(server.get("title"))) or _title_of(name),
        description=_plain(_str(server.get("description")), title=False)[:400],
        version=_str(server.get("version")),
        website=_str(server.get("websiteUrl")) or None,
        repository=_str(repo.get("url")) if isinstance(repo, Mapping) else None,
        icon=icon,
        options=tuple(options),
        featured=name in FEATURED,
    )


_MCP_TITLE = re.compile(r"\s*\b(remote\s+)?mcp(\s+server)?\b\s*", re.I)
_MCP_PHRASE = re.compile(r"\bMCP\s+(server|servers)\b|\bMCP\b", re.I)


def _plain(text: str, *, title: bool = True) -> str:
    """The registry's words without the protocol's: the product never says MCP (ADR 0034).
    "Figma MCP Server" is "Figma"; "the official Notion MCP server" is "the official
    Notion app"."""
    if title:
        return re.sub(r"\s{2,}", " ", _MCP_TITLE.sub(" ", text)).strip(" -:|")

    def word(m: re.Match[str]) -> str:
        return "apps" if (m.group(1) or "").lower() == "servers" else "app"

    return re.sub(r"\b(app)\s+app\b", r"\1", _MCP_PHRASE.sub(word, text), flags=re.I)


def _tell_apart(options: list[ConnectOption]) -> None:
    """Options that would read the same ("Use an API key" fifteen times) get the place
    they connect to after their label."""
    seen: dict[str, int] = {}
    for o in options:
        seen[o.label] = seen.get(o.label, 0) + 1
    for i, o in enumerate(options):
        if seen[o.label] > 1 and o.url:
            u = urlparse(o.url)
            where = (u.netloc + u.path.rstrip("/")).removeprefix("www.")
            options[i] = replace(o, label=f"{o.label} ({where})")


def _title_of(name: str) -> str:
    """ "com.notion/mcp" -> "Notion"; "io.github.getsentry/sentry-mcp" -> "Sentry"."""
    ns, _, tail = name.partition("/")
    word = tail.removesuffix("-mcp").removesuffix("-mcp-server").removeprefix("mcp-")
    if word in ("", "mcp", "server", "mcp-server"):
        parts = [
            p
            for p in ns.split(".")
            if p not in ("com", "io", "app", "net", "org", "ai", "dev", "mcp", "github")
        ]
        word = parts[-1] if parts else ns
    return word.replace("-", " ").replace("_", " ").strip().title() or name


def suggest_name(entry: CatalogEntry) -> str:
    base = re.sub(r"[^a-z0-9]+", "-", entry.title.lower()).strip("-")[:30] or "app"
    return base if APP_NAME_RE.match(base) else "app"


# --- connecting -----------------------------------------------------------------------


def _literal(text: str) -> str:
    """Text Ghost did not write (the registry's, or a setting the operator typed) as it
    must reach ``apps.json``: with no ``$``, so it can never become a ``${NAME}`` filled
    from Ghost's environment (``ghost_core.apps.fill_secrets``)."""
    return text.replace("$", "")


def connect(
    entry: CatalogEntry, option_id: str, app_name: str, values: Mapping[str, str]
) -> tuple[AppSpec, dict[str, str]]:
    """The app to write into ``apps.json``, and the secrets to put in Ghost's environment.
    A secret's value goes only into the second; the file holds ``${NAME}``."""
    if not APP_NAME_RE.match(app_name):
        raise CatalogError("the name is lower-case letters, digits, - or _ (up to 40)")
    option = next((o for o in entry.options if o.id == option_id), None)
    if option is None:
        raise CatalogError(f"{entry.title} has no way to connect called {option_id}")
    secrets: dict[str, str] = {}
    filled: dict[str, str] = {}
    for s in option.settings:
        value = (values.get(s.key) or "").strip() or (s.default or "")
        if not value:
            if s.required:
                raise CatalogError(f"{s.label} is needed")
            continue
        if s.choices and value not in s.choices:
            raise CatalogError(f"{s.label} is one of {', '.join(s.choices)}")
        if s.secret:
            if s.where == "header" and s.key.lower() == "authorization" and " " not in value:
                value = f"Bearer {value}"  # a bare token pasted where the header goes
            name = secret_name(app_name, s.key)
            secrets[name] = value
            filled[s.key] = "${" + name + "}"
        else:
            filled[s.key] = _literal(value)  # a setting is never a reference

    def fill(template: str) -> str:
        """The entry's template with the settings in it. The template is the registry's
        text, so it is literal too: ``$ANTHROPIC_API_KEY`` in a fixed header would
        otherwise be filled from Ghost's environment at connect and sent to the entry's
        own server. The only references in the result are the ones written above."""
        return _VAR.sub(lambda m: filled.get(m[1], ""), _literal(template))

    common = {"title": entry.title, "catalog": entry.name}
    if option.url:
        headers = {
            name: fill(tpl) if tpl else filled[name]
            for name, tpl in option.headers
            if (fill(tpl) if tpl else filled.get(name))
        }
        spec = AppSpec(
            name=app_name,
            url=fill(option.url),
            headers=headers,
            sign_in=option.kind == "sign_in",
            **common,
        )
        return spec, secrets
    env = {s.key: filled[s.key] for s in option.settings if s.where == "env" and s.key in filled}
    args_before = [fill(a) for a in option.runtime_args]
    args_after = [fill(a) for a in option.package_args]
    package = _literal(str(option.package))
    if option.runner == "docker":
        passes = [x for k in env for x in ("-e", k)]
        args = ["run", "-i", "--rm", *passes, *args_before, package, *args_after]
    elif option.runner == "npx":
        args = ["-y", *args_before, package, *args_after]
    else:
        args = [*args_before, package, *args_after]
    spec = AppSpec(name=app_name, command=option.runner, args=tuple(args), env=env, **common)
    return spec, secrets


# --- the copy -----------------------------------------------------------------------------


async def _http_fetch(url: str) -> Mapping[str, Any]:
    import httpx

    async with httpx.AsyncClient(timeout=FETCH_TIMEOUT_SECS) as client:
        r = await client.get(url, headers={"Accept": "application/json"})
        r.raise_for_status()
        data = r.json()
        return data if isinstance(data, Mapping) else {}


@dataclass
class _Copy:
    fetched_at: float = 0.0
    since: str = ""  # RFC 3339, the registry's clock, for the next incremental fetch
    records: dict[str, dict[str, Any]] = field(default_factory=dict)
    cursor: str = ""  # where an unfinished first copy goes on from
    started: str = ""  # when that first copy began: its ``since`` once it is done


class Catalog:
    """The local copy and its search. ``refresh`` is called by the API when the Apps
    screen opens; it fetches at most once per ``max_age`` and never on a voice turn."""

    def __init__(
        self,
        path: Path,
        *,
        fetch: Fetch | None = None,
        registry: str = REGISTRY,
        max_age: float = MAX_AGE_SECS,
    ) -> None:
        self.path = path
        self._fetch = fetch or _http_fetch
        self.registry = registry.rstrip("/")
        self.max_age = max_age
        self._copy = self._load()
        self._entries: dict[str, CatalogEntry] = {}
        self._index()
        self._refreshing: asyncio.Task[None] | None = None
        self.problem: str | None = None

    def _load(self) -> _Copy:
        try:
            raw = json.loads(self.path.read_text())
            return _Copy(
                fetched_at=float(raw.get("fetched_at", 0)),
                since=str(raw.get("since", "")),
                records=dict(raw.get("records", {})),
                cursor=str(raw.get("cursor", "")),
                started=str(raw.get("started", "")),
            )
        except (OSError, ValueError, AttributeError):
            return _Copy()

    def _save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(
            json.dumps(
                {
                    "fetched_at": self._copy.fetched_at,
                    "since": self._copy.since,
                    "cursor": self._copy.cursor,
                    "started": self._copy.started,
                    "records": self._copy.records,
                }
            )
        )
        tmp.replace(self.path)

    def _index(self) -> None:
        entries: dict[str, CatalogEntry] = {}
        for raw in self._copy.records.values():
            e = parse_entry(raw)
            if e is not None:
                entries[e.name] = e
        self._entries = entries

    @property
    def fetched_at(self) -> float:
        return self._copy.fetched_at

    @property
    def refreshing(self) -> bool:
        return self._refreshing is not None and not self._refreshing.done()

    @property
    def stale(self) -> bool:
        return time.time() - self._copy.fetched_at > self.max_age

    def refresh_soon(self) -> None:
        """Start a refresh in the background when the copy is older than ``max_age``."""
        if self.stale and not self.refreshing:
            self._refreshing = asyncio.ensure_future(self._refresh_logged())

    async def wait(self, secs: float) -> None:
        """Wait up to ``secs`` for a refresh under way to have something to show: the
        first-party apps of a first copy come in a second or two, the rest later."""
        task = self._refreshing
        deadline = time.monotonic() + secs
        while task is not None and not task.done() and not self._entries:
            if time.monotonic() >= deadline:
                return
            await asyncio.sleep(0.1)

    async def _refresh_logged(self) -> None:
        try:
            await self.refresh()
        except Exception as e:  # noqa: BLE001 - the copy we have stays; the screen says so
            self.problem = f"couldn't reach the catalog ({type(e).__name__})"
            logger.warning(f"catalog: {self.problem}")

    async def refresh(self) -> int:
        """Fetch what changed since the last copy. The first time: the first-party apps
        one by one (so the screen has them in a second or two), then everything, page by
        page, searchable as it comes in and resumed where it stopped if the daemon
        restarts. Returns the number of records fetched."""
        began = time.perf_counter()
        now = datetime.now(UTC).isoformat(timespec="seconds").replace("+00:00", "Z")
        fetched = 0
        first = not self._copy.since
        if first and not self._copy.started:
            self._copy.started = now
            fetched += await self._featured()
            self._index()
            self._save()
        cursor = self._copy.cursor if first else ""
        for page_no in range(MAX_PAGES):
            url = f"{self.registry}/v0.1/servers?limit={PAGE}&version=latest"
            if not first:
                url += f"&updated_since={quote(self._copy.since)}&include_deleted=true"
            if cursor:
                url += f"&cursor={quote(cursor)}"
            page = await self._fetch(url)
            fetched += self._keep(page.get("servers") or [])
            meta = page.get("metadata") or {}
            cursor = meta.get("nextCursor") if isinstance(meta, Mapping) else None
            if first:
                self._copy.cursor = cursor or ""
                if page_no % SAVE_EVERY == SAVE_EVERY - 1:
                    self._index()
                    self._save()
            if not cursor:
                break
        self._copy.fetched_at = time.time()
        self._copy.since = self._copy.started if first else now
        self._copy.cursor = ""
        self._copy.started = ""
        self.problem = None
        self._index()
        self._save()
        logger.info(
            f"catalog: {fetched} record(s) fetched in {time.perf_counter() - began:.1f}s, "
            f"{len(self._entries)} apps"
        )
        return fetched

    def _keep(self, records: list[Any]) -> int:
        n = 0
        for rec in records:
            if not isinstance(rec, Mapping) or not isinstance(rec.get("server"), Mapping):
                continue
            name = rec["server"].get("name")
            if isinstance(name, str):
                self._copy.records[name] = dict(rec)
                n += 1
        return n

    async def _featured(self) -> int:
        async def one(name: str) -> list[Any]:
            url = f"{self.registry}/v0.1/servers/{quote(name, safe='')}/versions/latest"
            try:
                rec = await self._fetch(url)
            except Exception:  # noqa: BLE001 - one missing entry doesn't stop the copy
                return []
            return [rec] if isinstance(rec, Mapping) and "server" in rec else []

        found = await asyncio.gather(*(one(n) for n in FEATURED))
        return self._keep([r for rs in found for r in rs])

    def get(self, name: str) -> CatalogEntry | None:
        return self._entries.get(name)

    def search(
        self, query: str = "", *, limit: int = 40, offset: int = 0
    ) -> tuple[list[CatalogEntry], int]:
        """Entries matching every word of ``query`` in the title, the name or the
        description: first-party ones first, then an exact title, a title that starts
        with it, one that holds it, then the rest. Returns a page and the total."""
        q = query.strip().lower()
        words = [w for w in re.findall(r"[a-z0-9]+", q) if w]
        hits: list[tuple[tuple[int, int, int, str], CatalogEntry]] = []
        for e in self._entries.values():
            title = e.title.lower()
            hay = f"{title} {e.name.lower()} {e.description.lower()}"
            if words and not all(w in hay for w in words):
                continue
            if not words and not e.featured:
                continue
            featured = FEATURED.index(e.name) if e.featured else len(FEATURED)
            if title == q:
                closeness = 0
            elif title.startswith(q):
                closeness = 1
            elif all(w in title for w in words):
                closeness = 2
            else:
                closeness = 3
            hits.append(((featured, closeness, len(title), title), e))
        hits.sort(key=lambda h: h[0])
        entries = [e for _, e in hits]
        return entries[offset : offset + limit], len(entries)
