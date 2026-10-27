"""Departments (ADR 0024): a department is a folder, and Ghost is one too.

Everything the operator edits lives under ``~/.config/ghost/``::

    ~/.config/ghost/                    # Ghost: the department every wake lands in
    ├── AGENTS.md                       # Ghost's standing instructions
    ├── mcp.json                        # Ghost's servers
    ├── department.json                 # Ghost's write declarations and defaults
    ├── memory.md                       # what every agent in Ghost knows (gate-written)
    ├── skills/<name>/SKILL.md
    ├── agents/ghost/                   # agent zero
    └── departments/
        └── marketing/
            ├── AGENTS.md               # the department's rules, in every agent's prompt
            ├── mcp.json                # its servers, in the shape other clients share
            ├── department.json         # title, aliases, voice, root, write declarations
            ├── memory.md               # the department's shared memory, gate-written
            ├── skills/<name>/SKILL.md
            ├── docs/…                  # read on demand
            └── agents/<slug>/          # agent.json, persona.md, memory.md, notes.md

``mcp.json`` is ``{"mcpServers": {"<name>": {"command", "args", "env"}}}`` for stdio and
``{"type": "http", "url", "headers"}`` for streamable HTTP, so a server copies to and from
Cursor or Claude unchanged; ``"use": ["github"]`` names one of Ghost's servers instead of
starting another process. Secrets are ``${NAME}`` or ``${env:NAME}``, resolved from Ghost's
environment only. ``department.json`` holds what the gate needs, keyed by server name:
``scope``, ``reads``, ``writes.<operation>``, ``levels`` and ``targets`` (the declarations
of ADR 0022, unchanged in meaning). A department may keep its resources in a ``root``
folder elsewhere (a repository): its ``AGENTS.md``, ``mcp.json`` or ``.mcp.json``, skills
and ``docs/`` are read in place; its agents stay here.

The model never writes ``mcp.json`` or ``department.json``. This module reads the folders;
what Ghost writes (``memory.md``, a rule appended to ``AGENTS.md``) goes through the gate.
"""

from __future__ import annotations

import asyncio
import json
import re
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

from loguru import logger

from ghost_core.hooks import Hook, parse_hooks
from ghost_core.mcp import (
    SERVER_NAME_RE,
    ScopeSpec,
    Servers,
    ServerSpec,
    WriteSpec,
    _parse_scope,
    _parse_write,
    _str_list,
    _str_table,
    _table,
    secret_names,
    source_of,
)
from ghost_core.policy import Autonomy, AutonomyPolicy
from ghost_core.watches import Watch, parse_watches

GHOST = "ghost"
DEPARTMENTS_DIR = "departments"
MCP_FILE = "mcp.json"
ALT_MCP_FILE = ".mcp.json"  # a repository's own, read in place through ``root``
DEPARTMENT_FILE = "department.json"
RULES_FILE = "AGENTS.md"
MEMORY_FILE = "memory.md"
AGENTS_DIR = "agents"
DOCS_DIR = "docs"
SKILL_FOLDERS = ("skills", ".agents/skills", ".claude/skills")
SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,39}$")
MAX_RULES_CHARS = 8000  # AGENTS.md as the prompt carries it
MAX_MEMORY_CHARS = 4000  # the shared memory stays short: it is in every agent's prompt
MAX_DOC_CHARS = 6000
MAX_DOCS_LISTED = 40
_TRANSPORTS = {"stdio", "http", "streamable-http", "streamable_http", "streamableHttp"}
_SERVER_KEYS = {"type", "command", "args", "env", "url", "headers"}
_DECLARATION_KEYS = {"reads", "scope", "writes", "levels", "targets"}
_DEPARTMENT_KEYS = {
    "title",
    "aliases",
    "voice",
    "speed",
    "model",
    "root",
    "root_servers",
    "servers",
    "watches",  # ADR 0028
    "hooks",  # ADR 0027
}
# A literal that is probably a token: a known prefix, or a bearer value with no reference.
_SECRET_SHAPES = re.compile(r"^(ghp_|github_pat_|gho_|sk-|xox[abp]-|AKIA|glpat-|Bearer\s+\S{16,})")
_STOP = {"the", "a", "an", "to", "me", "department", "dept", "team", "office", "desk", "folks"}


class DepartmentError(ValueError):
    """A folder that cannot be read as a department. Safe to show the operator."""


# --- the declaration (department.json) ----------------------------------------------


@dataclass(frozen=True)
class Declaration:
    """What the gate needs for one server, as a department declares it."""

    reads: tuple[str, ...] | None = None
    scope: ScopeSpec | None = None
    writes: Mapping[str, WriteSpec] = field(default_factory=dict)
    levels: Mapping[str, Autonomy] = field(default_factory=dict)
    targets: Mapping[str, Mapping[str, Autonomy]] = field(default_factory=dict)

    def applied_to(self, spec: ServerSpec) -> ServerSpec:
        """``spec``'s transport with this declaration's reads, scope and writes."""
        return replace(spec, reads=self.reads, scope=self.scope, writes=dict(self.writes))


def _levels(value: object, where: str) -> dict[str, Autonomy]:
    t = _table(value, where)
    out: dict[str, Autonomy] = {}
    for op, raw in t.items():
        try:
            out[str(op)] = Autonomy(str(raw))
        except ValueError:
            raise DepartmentError(
                f"{where}.{op}: {raw!r} is not one of {', '.join(a.value for a in Autonomy)}"
            ) from None
    return out


def parse_declaration(server: str, body: object, where: str) -> Declaration:
    t = _table(body, where)
    unknown = sorted(set(t) - _DECLARATION_KEYS)
    if unknown:
        raise DepartmentError(f"{where} has unknown key(s): {', '.join(unknown)}")
    reads = t.get("reads")
    writes_raw = _table(t.get("writes", {}), f"{where}.writes")
    targets_raw = _table(t.get("targets", {}), f"{where}.targets")
    try:
        writes = {op: _parse_write(server, op, b, where) for op, b in writes_raw.items()}
        scope = _parse_scope(t["scope"], f"{where}.scope") if "scope" in t else None
        reads_t = _str_list(reads, f"{where}.reads") if reads is not None else None
    except ValueError as e:
        raise DepartmentError(str(e)) from None
    return Declaration(
        reads=reads_t,
        scope=scope,
        writes=writes,
        levels=_levels(t.get("levels", {}), f"{where}.levels"),
        targets={
            str(name).lower(): _levels(ops, f"{where}.targets.{name}")
            for name, ops in targets_raw.items()
        },
    )


# --- the servers (mcp.json) -----------------------------------------------------------


@dataclass(frozen=True)
class McpFile:
    """What an ``mcp.json`` says: the servers' transports, and the names of Ghost's servers
    this department uses instead of starting its own."""

    servers: Mapping[str, ServerSpec] = field(default_factory=dict)
    uses: tuple[str, ...] = ()
    problems: tuple[str, ...] = ()


def looks_like_secret(value: str) -> bool:
    """A literal where a ``${NAME}`` reference belongs."""
    if secret_names(value):
        return False
    return bool(_SECRET_SHAPES.match(value.strip())) or (
        len(value) >= 32 and " " not in value.strip() and not value.startswith(("http", "/"))
    )


def parse_mcp_json(text: str, where: str) -> McpFile:
    """The portable shape. A server with a problem is left out and the problem named; the
    rest of the file still counts, so one bad entry does not take a department down."""
    try:
        raw = json.loads(text) if text.strip() else {}
    except ValueError as e:
        raise DepartmentError(f"{where}: not JSON: {e}") from None
    if not isinstance(raw, Mapping):
        raise DepartmentError(f"{where}: the file must be an object")
    problems: list[str] = []
    servers: dict[str, ServerSpec] = {}
    entries = raw.get("mcpServers", {})
    if not isinstance(entries, Mapping):
        raise DepartmentError(f"{where}: mcpServers must be an object of servers")
    for name, body in entries.items():
        at = f"{where}: {name}"
        if not SERVER_NAME_RE.match(str(name)):
            problems.append(f"{at}: a server name is letters, digits, _ or -")
            continue
        try:
            spec = _parse_server(str(name), body, at, problems)
        except ValueError as e:
            problems.append(str(e))
            continue
        servers[str(name)] = spec
    uses_raw = raw.get("use", [])
    try:
        uses = _str_list(uses_raw, f"{where}: use")
    except ValueError as e:
        problems.append(str(e))
        uses = ()
    for name in uses:
        if name in servers:
            problems.append(f"{where}: {name} is both a server here and a use of Ghost's")
    uses = tuple(dict.fromkeys(u for u in uses if u not in servers))
    extra = sorted(set(raw) - {"mcpServers", "use"})
    if extra:
        problems.append(f"{where}: unknown key(s) {', '.join(extra)} are ignored")
    return McpFile(servers=servers, uses=uses, problems=tuple(problems))


def _parse_server(name: str, body: object, where: str, problems: list[str]) -> ServerSpec:
    t = _table(body, where)
    unknown = sorted(set(t) - _SERVER_KEYS)
    if unknown:
        problems.append(f"{where}: key(s) {', '.join(unknown)} are not read by Ghost")
    kind = t.get("type")
    url = t.get("url")
    command = t.get("command")
    if kind is not None and str(kind) not in _TRANSPORTS:
        raise ValueError(f"{where}: transport {kind!r} is not stdio or streamable http")
    if bool(url) == bool(command):
        raise ValueError(f"{where} needs exactly one of url or command")
    headers = _str_table(t.get("headers", {}), f"{where}.headers")
    env = _str_table(t.get("env", {}), f"{where}.env")
    for label, table in (("headers", headers), ("env", env)):
        for key, value in table.items():
            if looks_like_secret(value):
                problems.append(
                    f"{where}: {label} {key} looks like a literal secret; use ${{NAME}} and "
                    "set it in Ghost's environment"
                )
    return ServerSpec(
        name=name,
        url=str(url) if url else None,
        headers=headers,
        command=str(command) if command else None,
        args=_str_list(t.get("args", []), f"{where}.args"),
        env=env,
    )


def render_mcp_json(servers: Mapping[str, ServerSpec], uses: Sequence[str] = ()) -> str:
    """The portable shape back out, for a folder Ghost writes (a new department)."""
    out: dict[str, Any] = {"mcpServers": {}}
    for name, s in servers.items():
        if s.url:
            entry: dict[str, Any] = {"type": "http", "url": s.url}
            if s.headers:
                entry["headers"] = dict(s.headers)
        else:
            entry = {"command": s.command}
            if s.args:
                entry["args"] = list(s.args)
            if s.env:
                entry["env"] = dict(s.env)
        out["mcpServers"][name] = entry
    if uses:
        out["use"] = list(uses)
    return json.dumps(out, indent=2) + "\n"


# --- a department ------------------------------------------------------------------


@dataclass
class Department:
    slug: str
    path: Path  # the folder under ~/.config/ghost: the root itself for Ghost
    title: str
    aliases: tuple[str, ...] = ()
    voice: str | None = None
    speed: float | None = None
    model: str | None = None  # reserved (ADR 0024): not read yet
    root: Path | None = None  # resources read in place from another folder
    servers: dict[str, ServerSpec] = field(default_factory=dict)  # its own, declared
    uses: tuple[str, ...] = ()  # Ghost's servers it reaches through its own declaration
    declarations: dict[str, Declaration] = field(default_factory=dict)
    watches: dict[str, Watch] = field(default_factory=dict)  # ADR 0028
    hooks: dict[str, Hook] = field(default_factory=dict)  # ADR 0027
    problems: list[str] = field(default_factory=list)

    @property
    def is_ghost(self) -> bool:
        return self.slug == GHOST

    @property
    def resources(self) -> Path:
        """Where ``AGENTS.md``, the skills and ``docs/`` are read: the root when set."""
        return self.root or self.path

    @property
    def agents_dir(self) -> Path:
        return self.path / AGENTS_DIR

    @property
    def memory_path(self) -> Path:
        return self.path / MEMORY_FILE

    @property
    def rules_path(self) -> Path:
        return self.resources / RULES_FILE

    def spoken_names(self) -> tuple[str, ...]:
        return (self.title, *self.aliases)

    # --- what a session reads ------------------------------------------------------

    def rules(self) -> str:
        """``AGENTS.md``, as every agent's prompt carries it. Text from a root is untrusted
        like issue text; the prompt says so."""
        return _read_text(self.rules_path, MAX_RULES_CHARS)

    def rules_lead(self) -> str:
        """The first line of the rules: what Ghost's prompt lists the department by."""
        for line in self.rules().splitlines():
            text = line.strip().lstrip("#").strip()
            if text:
                return text[:200]
        return ""

    def memory(self) -> str:
        return _read_text(self.memory_path, MAX_MEMORY_CHARS)

    def skill_places(self) -> list[tuple[Path, str]]:
        """The skill folders this department reads, nearest first, with the source label
        each skill carries (``ghost_core.skills``)."""
        out: list[tuple[Path, str]] = []
        for rel in SKILL_FOLDERS:
            folder = self.resources / rel
            if folder.is_dir():
                out.append((folder, self.slug))
        return out

    def docs(self) -> list[str]:
        """The files under ``docs/``, relative, text only by name, never through a link."""
        folder = self.resources / DOCS_DIR
        if not folder.is_dir() or folder.is_symlink():
            return []
        out: list[str] = []
        for p in sorted(folder.rglob("*")):
            if p.is_symlink() or not p.is_file() or p.name.startswith("."):
                continue
            rel = p.relative_to(folder)
            if any(part.startswith(".") for part in rel.parts) or len(rel.parts) > 3:
                continue
            out.append(str(rel))
            if len(out) >= MAX_DOCS_LISTED:
                break
        return out

    def read_doc(self, name: str) -> str:
        """One of ``docs()``, confined to that folder."""
        folder = (self.resources / DOCS_DIR).resolve()
        target = (self.resources / DOCS_DIR / name).resolve()
        if folder not in target.parents or name not in self.docs():
            raise DepartmentError(f"{name!r} is not a document of {self.title}")
        return _read_text(target, MAX_DOC_CHARS)

    # --- what the gate and the audit see ---------------------------------------------

    def source_name(self, server: str) -> str:
        """A department's own server is ``<department>/<server>`` to the gate and the
        audit; one it uses, and Ghost's, keep their bare names."""
        if self.is_ghost or server in self.uses:
            return server
        return f"{self.slug}/{server}"

    def declared(self, server: str) -> Declaration:
        return self.declarations.get(server, Declaration())

    def spec_for_use(self, ghost_spec: ServerSpec) -> ServerSpec:
        """Ghost's server seen through this department's declaration. The allowlist never
        widens: no scope here means Ghost's, no items here means Ghost's items, and items
        Ghost does not allow are dropped (an empty result falls back to Ghost's)."""
        decl = self.declared(ghost_spec.name)
        spec = decl.applied_to(ghost_spec)
        gs, ds = ghost_spec.scope, decl.scope
        if gs is None:
            return spec
        # The kind and the arguments are Ghost's (they say how an item is read off a
        # call); the department only ever narrows the items.
        if ds is None or not ds.items or not gs.items:
            return replace(spec, scope=gs)
        allowed = {i.lower() for i in gs.items}
        items = tuple(i for i in ds.items if i.lower() in allowed) or tuple(gs.items)
        return replace(spec, scope=replace(gs, items=items))

    def policy(self, base: AutonomyPolicy) -> AutonomyPolicy:
        """The policy a session in this department runs under: the file's ``[defaults]``
        and Ghost's own operations, plus this department's levels and targets for its
        servers, under the names the gate knows them by."""
        skills = {k: dict(v) for k, v in base.skills.items()}
        targets = {k: {t: dict(ops) for t, ops in v.items()} for k, v in base.targets.items()}
        for server, decl in self.declarations.items():
            name = self.source_name(server)
            if decl.levels:
                skills.setdefault(name, {}).update(decl.levels)
            for target, ops in decl.targets.items():
                targets.setdefault(name, {}).setdefault(target, {}).update(ops)
        return AutonomyPolicy(
            default=base.default,
            skills=skills,
            targets=targets,
            source=f"{base.source} + {self.path / DEPARTMENT_FILE}",
        )

    def covers(self, server: str, item: str) -> bool:
        """Whether this department's declaration for ``server`` allows ``item``."""
        decl = self.declarations.get(server)
        if decl is None or decl.scope is None or not decl.scope.items:
            return False
        return decl.scope.allows(item)


def _read_text(path: Path, cap: int) -> str:
    try:
        if path.is_symlink() or not path.is_file():
            return ""
        text = path.read_text(errors="replace")
    except OSError:
        return ""
    return text if len(text) <= cap else text[:cap].rstrip() + "…"


def _root_of(value: object, where: str, base: Path) -> Path | None:
    """``root`` as a path: ``~`` expanded, a relative one taken from the department's
    folder (never from the daemon's working directory)."""
    if value in (None, ""):
        return None
    if not isinstance(value, str):
        raise DepartmentError(f"{where}.root must be a path")
    root = Path(value).expanduser()
    return root if root.is_absolute() else (base / root).resolve()


def load_department(slug: str, path: Path, *, is_ghost: bool = False) -> Department:
    """The department in ``path``. A missing file is an empty one; a broken file is a
    problem on the department, which still exists and still has its agents."""
    problems: list[str] = []
    data: dict[str, Any] = {}
    file = path / DEPARTMENT_FILE
    if file.is_file():
        try:
            raw = json.loads(file.read_text())
            if not isinstance(raw, Mapping):
                raise DepartmentError(f"{file}: the file must be an object")
            data = dict(raw)
        except (ValueError, OSError, DepartmentError) as e:
            problems.append(f"{file}: {e}")
            data = {}
    unknown = sorted(set(data) - _DEPARTMENT_KEYS)
    if unknown:
        problems.append(f"{file}: unknown key(s) {', '.join(unknown)} are ignored")
    root: Path | None = None
    try:
        root = _root_of(data.get("root"), str(file), path)
    except DepartmentError as e:
        problems.append(str(e))
    if root is not None and not root.is_dir():
        problems.append(f"{file}: root {root} is not a folder")
        root = None
    if is_ghost and root is not None:
        problems.append(f"{file}: Ghost keeps its resources here; root is ignored")
        root = None
    for linked in (RULES_FILE, MEMORY_FILE):
        at = (root or path) / linked if linked == RULES_FILE else path / linked
        if at.is_symlink():
            problems.append(f"{at}: a link; Ghost reads and writes only regular files here")
    title = str(data.get("title") or ("Ghost" if is_ghost else slug.replace("-", " ").title()))
    aliases_raw = data.get("aliases", [])
    aliases = tuple(
        str(a).strip()
        for a in (aliases_raw if isinstance(aliases_raw, list) else [])
        if str(a).strip()
    )
    voice = str(data["voice"]) if data.get("voice") else None
    speed = None
    if data.get("speed") is not None:
        try:
            speed = float(data["speed"])
        except (TypeError, ValueError):
            problems.append(f"{file}: speed must be a number")
    model = str(data["model"]) if data.get("model") else None

    declarations: dict[str, Declaration] = {}
    servers_raw = data.get("servers", {})
    if not isinstance(servers_raw, Mapping):
        problems.append(f"{file}: servers must be an object keyed by server name")
        servers_raw = {}
    for name, body in servers_raw.items():
        try:
            declarations[str(name)] = parse_declaration(str(name), body, f"{file}: {name}")
        except (DepartmentError, ValueError) as e:
            problems.append(str(e))

    # A repository's own server file (root) names commands to run and secrets to hand
    # them, so it counts only when department.json says root_servers: true; otherwise
    # the department's own mcp.json is read and the root's is a problem to see.
    candidates: list[Path] = []
    if root is not None:
        in_root = [p for p in (root / MCP_FILE, root / ALT_MCP_FILE) if p.is_file()]
        if data.get("root_servers") is True:
            candidates.extend(in_root)
        elif in_root:
            problems.append(
                f"{in_root[0]}: a server file in the root; its servers are not started "
                f'until {file} says "root_servers": true'
            )
    candidates.append(path / MCP_FILE)
    mcp_path = next((p for p in candidates if p.is_file()), None)
    mcp = McpFile()
    if mcp_path is not None:
        try:
            mcp = parse_mcp_json(mcp_path.read_text(), str(mcp_path))
        except (DepartmentError, OSError) as e:
            problems.append(str(e))
    problems.extend(mcp.problems)
    uses = mcp.uses
    if is_ghost and uses:
        problems.append(f"{mcp_path}: Ghost has no one to use servers from; use is ignored")
        uses = ()
    servers = {
        name: declarations.get(name, Declaration()).applied_to(spec)
        for name, spec in mcp.servers.items()
    }
    for name in declarations:
        if name not in servers and name not in uses:
            problems.append(f"{file}: {name} is declared but {MCP_FILE} neither starts nor uses it")
    watches, watch_problems = parse_watches(data.get("watches"), slug, str(file))
    problems.extend(watch_problems)
    for wname, w in list(watches.items()):
        if w.server not in servers and w.server not in uses:
            problems.append(
                f"{file}: watch {wname} reads {w.server}, which this department neither "
                "starts nor uses; the watch is off"
            )
            del watches[wname]
    hooks, hook_problems = parse_hooks(data.get("hooks"), slug, str(file), path)
    problems.extend(hook_problems)
    for hname, h in list(hooks.items()):
        lacking = sorted(
            {e.check.server for e in h.events.values() if e.check is not None}
            - set(servers)
            - set(uses)
        )
        if lacking:
            problems.append(
                f"{file}: hook {hname} checks with {', '.join(lacking)}, which this department "
                "neither starts nor uses; the hook is off"
            )
            del hooks[hname]
    return Department(
        slug=slug,
        path=path,
        title=title,
        aliases=aliases,
        voice=voice,
        speed=speed,
        model=model,
        root=root,
        servers=servers,
        uses=uses,
        declarations=declarations,
        watches=watches,
        hooks=hooks,
        problems=problems,
    )


def discover(config_dir: Path) -> dict[str, Department]:
    """Ghost at the config root, then every folder under ``departments/``."""
    out = {GHOST: load_department(GHOST, config_dir, is_ghost=True)}
    folder = config_dir / DEPARTMENTS_DIR
    if not folder.is_dir():
        return out
    for p in sorted(folder.iterdir()):
        if not p.is_dir():
            continue
        slug = p.name
        if slug == GHOST or not SLUG_RE.match(slug):
            logger.warning(f"departments: {p} is not a department name (lower-case slug); skipped")
            continue
        out[slug] = load_department(slug, p)
    _check_uses(out)
    return out


def _check_uses(departments: dict[str, Department]) -> None:
    """A use names one of Ghost's servers, and its items stay inside Ghost's allowlist:
    anything else is a problem the operator sees (``spec_for_use`` applies the same
    limits whether or not the operator has read them)."""
    ghost = departments[GHOST]
    for d in departments.values():
        if d.is_ghost:
            continue
        for name in d.uses:
            gspec = ghost.servers.get(name)
            if gspec is None:
                d.problems.append(
                    f"{d.path / MCP_FILE}: uses {name}, which Ghost's {MCP_FILE} does not start"
                )
                continue
            ds, gs = d.declared(name).scope, gspec.scope
            if (
                ds is not None
                and gs is not None
                and (ds.kind, ds.arguments) != (gs.kind, gs.arguments)
            ):
                d.problems.append(
                    f"{d.path / DEPARTMENT_FILE}: {name} scope is {gs.kind} by "
                    f"{', '.join(gs.arguments)} (Ghost's); the department's kind and "
                    "arguments are ignored"
                )
            if ds is not None and gs is not None and gs.items and ds.items:
                allowed = {i.lower() for i in gs.items}
                extra = [i for i in ds.items if i.lower() not in allowed]
                if extra:
                    d.problems.append(
                        f"{d.path / DEPARTMENT_FILE}: {name} items {', '.join(extra)} are "
                        "outside Ghost's allowlist; ignored"
                    )


def _words(text: str) -> set[str]:
    return {w for w in re.findall(r"[a-z0-9]+", text.lower()) if w not in _STOP}


class Departments:
    """The registry: the folders as they are now. ``refresh`` re-reads them."""

    def __init__(self, config_dir: Path) -> None:
        self.config_dir = Path(config_dir).expanduser()
        self._by_slug: dict[str, Department] = {}
        # Called after every re-read (the gate rebuilds the departments' levels).
        self.on_refresh: list[Callable[[], None]] = []
        self.refresh()

    def refresh(self) -> None:
        self._by_slug = discover(self.config_dir)
        for hook in list(self.on_refresh):
            hook()

    def ensure_layout(self) -> None:
        """The folders a fresh install needs: ``departments/`` and Ghost's ``agents/``."""
        (self.config_dir / DEPARTMENTS_DIR).mkdir(parents=True, exist_ok=True)
        (self.config_dir / AGENTS_DIR).mkdir(parents=True, exist_ok=True)

    @property
    def ghost(self) -> Department:
        return self._by_slug[GHOST]

    def get(self, slug: str) -> Department | None:
        return self._by_slug.get(slug.strip().lower())

    def list(self) -> list[Department]:
        """Ghost first, then the rest by title."""
        rest = sorted((d for d in self._by_slug.values() if not d.is_ghost), key=lambda d: d.title)
        return [self.ghost, *rest]

    def resolve(self, spoken: str) -> Department | None:
        """The department the operator most likely means: slug, title or alias, then word
        overlap with filler words dropped."""
        text = spoken.strip().lower()
        if not text:
            return None
        for d in self.list():
            if d.slug == text or text in {n.lower() for n in d.spoken_names()}:
                return d
        words = _words(text)
        if not words:
            return None
        best: tuple[int, Department] | None = None
        for d in self.list():
            cand: set[str] = set()
            for n in d.spoken_names():
                cand |= _words(n)
            score = len(words & cand)
            if score and (best is None or score > best[0]):
                best = (score, d)
        return best[1] if best else None

    def covering(self, server: str, item: str) -> list[Department]:
        """The departments whose declaration for ``server`` covers ``item``: where a
        request about it can be routed (ADR 0019). Ghost is unscoped and never one."""
        return [d for d in self.list() if not d.is_ghost and d.covers(server, item)]

    def problems(self) -> list[str]:
        return [f"{d.slug}: {p}" for d in self.list() for p in d.problems]

    def create(
        self, title: str, *, aliases: Sequence[str] = (), voice: str | None = None
    ) -> Department:
        """A new department folder with a short ``AGENTS.md`` and its ``department.json``.
        The operator's action (the desktop's New department); the model never calls it."""
        slug = slugify(title)
        if slug == GHOST or slug in self._by_slug:
            raise DepartmentError(f"there is already a department called {title!r}")
        folder = self.config_dir / DEPARTMENTS_DIR / slug
        if folder.exists():
            raise DepartmentError(f"{folder} already exists")
        (folder / AGENTS_DIR).mkdir(parents=True)
        data: dict[str, Any] = {"title": title.strip()}
        if aliases:
            data["aliases"] = [a for a in aliases if a.strip()]
        if voice:
            data["voice"] = voice
        (folder / DEPARTMENT_FILE).write_text(json.dumps(data, indent=2) + "\n")
        (folder / RULES_FILE).write_text(f"# {title.strip()}\n\n")
        (folder / MCP_FILE).write_text(render_mcp_json({}))
        self.refresh()
        return self._by_slug[slug]


def slugify(name: str) -> str:
    text = re.sub(r"[^a-z0-9]+", "-", name.strip().lower()).strip("-")
    text = text[:40]
    return text if SLUG_RE.match(text) else "department"


__all__ = [
    "AGENTS_DIR",
    "DEPARTMENTS_DIR",
    "DEPARTMENT_FILE",
    "DOCS_DIR",
    "GHOST",
    "MCP_FILE",
    "MEMORY_FILE",
    "RULES_FILE",
    "SKILL_FOLDERS",
    "Declaration",
    "Department",
    "DepartmentError",
    "Departments",
    "Fleet",
    "McpFile",
    "all_sources",
    "discover",
    "load_department",
    "looks_like_secret",
    "parse_declaration",
    "parse_mcp_json",
    "render_mcp_json",
    "slugify",
]


# --- the servers a department reaches ----------------------------------------------------


class Fleet:
    """Every department's servers (ADR 0024). Ghost's connect at daemon start; a
    department's own start when a session enters it (``enter``) and stay up for the
    session plus an idle timeout (``leave``). What a session reaches is ``sources_for``:
    the department's own servers under ``<department>/<server>``, and the servers it
    uses from Ghost's fleet, seen through its own declaration."""

    def __init__(
        self,
        departments: Departments,
        *,
        environ: Mapping[str, str] | None = None,
        idle_secs: float = 600.0,
    ) -> None:
        self.departments = departments
        self._environ = environ
        self.idle_secs = idle_secs
        self.ghost = Servers(departments.ghost.servers, environ)
        self.own: dict[str, Servers] = {}
        self._timers: dict[str, asyncio.TimerHandle] = {}
        self._starting: dict[str, asyncio.Future[Servers | None]] = {}
        self.started: dict[str, float] = {}  # slug -> how long the last start took
        self._failed: dict[str, dict[str, str]] = {}  # slug -> server -> why, last start
        # ADR 0028: a department with a watch keeps its servers up while the daemon runs.
        self.pinned: set[str] = set()

    def failures(self, slug: str) -> list[str]:
        """The department's servers that did not connect at the last start, for the
        operator (``GET /departments``, ``ghostd check``)."""
        failed = self.own[slug].failed if slug in self.own else self._failed.get(slug, {})
        return [f"{name}: not connected ({why})" for name, why in failed.items()]

    async def connect(self) -> None:
        """Ghost's servers, at daemon start."""
        await self.ghost.connect()

    def is_up(self, slug: str) -> bool:
        return slug == GHOST or slug in self.own

    async def enter(self, slug: str) -> Servers | None:
        """The department's own servers, started now if they are not up (measured, on the
        latency line: the announcement and tone cover the wait). None for Ghost and for
        a department with no servers of its own."""
        if slug == GHOST:
            return None
        d = self.departments.get(slug)
        if d is None or not d.servers:
            return None
        self._cancel_timer(slug)
        if slug in self.own:
            return self.own[slug]
        if slug in self._starting:
            return await self._starting[slug]
        loop = asyncio.get_running_loop()
        fut: asyncio.Future[Servers | None] = loop.create_future()
        self._starting[slug] = fut
        began = time.perf_counter()
        servers = Servers(d.servers, self._environ)
        try:
            await servers.connect()
            self.started[slug] = time.perf_counter() - began
            self._failed[slug] = dict(servers.failed)
            if not servers.servers:
                # Nothing came up: keep nothing, so the next transfer tries again.
                logger.warning(
                    f"department {slug}: no server connected "
                    f"({'; '.join(f'{n}: {w}' for n, w in servers.failed.items())})"
                )
                await servers.close()
                fut.set_result(None)
                return None
            self.own[slug] = servers
            logger.info(
                f"department {slug}: {len(servers.servers)} server(s) up in "
                f"{self.started[slug]:.2f}s"
                + (f"; not connected: {', '.join(servers.failed)}" if servers.failed else "")
            )
            fut.set_result(servers)
        except BaseException as e:
            fut.set_exception(e)
            raise
        finally:
            self._starting.pop(slug, None)
        return servers

    def leave(self, slug: str) -> None:
        """The session left the department: its servers stay up for ``idle_secs``, then
        close, so a quick return costs nothing and an idle department costs nothing."""
        if slug == GHOST or slug not in self.own or slug in self.pinned:
            return
        self._cancel_timer(slug)
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        timer_box: list[asyncio.TimerHandle] = []

        def fire() -> None:
            # Only the timer still on record stops the servers: a session that came
            # back in the meantime cancelled and replaced it (so the handle differs).
            if timer_box and self._timers.get(slug) is timer_box[0]:
                asyncio.ensure_future(self.stop(slug))

        timer_box.append(loop.call_later(self.idle_secs, fire))
        self._timers[slug] = timer_box[0]

    async def pin(self, slug: str) -> None:
        """Keep the department's own servers up (a watch reads them). Started now if they
        are not up; ``unpin`` lets them idle out again."""
        if slug == GHOST:
            return
        self.pinned.add(slug)
        await self.enter(slug)

    def unpin(self, slug: str) -> None:
        if slug in self.pinned:
            self.pinned.discard(slug)
            self.leave(slug)

    def watch_server(self, d: Department, name: str) -> Any:
        """The server a watch in ``d`` reads, seen through ``d``'s declaration (its reads
        and its allowlist): Ghost's for Ghost, the department's own, or a use of Ghost's.
        None when it is not up."""
        if d.is_ghost:
            return self.ghost.servers.get(name)
        if name in d.uses:
            server = self.ghost.servers.get(name)
            return server.view(d.spec_for_use(server.spec)) if server is not None else None
        own = self.own.get(d.slug)
        return own.servers.get(name) if own is not None else None

    async def watch_read(self, watch: Watch) -> Any:
        """A watch's read (ADR 0028): the daemon calls it, no model."""
        return await self.read_for(watch.department, watch.server, watch.read, dict(watch.args))

    async def read_for(
        self, department: str, server_name: str, read: str, args: dict[str, Any]
    ) -> Any:
        """A read the daemon makes for a department (a watch's look, a webhook's check at
        the source, ADR 0027/0028): one of the reads its declaration offers, its allowlist
        applied, the result not trimmed. A server whose transport died is brought back
        first; a department's own servers are started if they are not up."""
        d = self.departments.get(department)
        if d is None:
            raise ValueError(f"no department {department}")
        own = not d.is_ghost and server_name not in d.uses
        servers = self.own.get(d.slug) if own else self.ghost
        if servers is None:
            await self.enter(d.slug)
            servers = self.own.get(d.slug)
            if servers is None:
                raise ValueError(f"{d.title}'s servers are not up")
            self.leave(d.slug)  # idles out unless a watch pinned it
        if not servers.is_up(server_name):
            await servers.revive(server_name)
        server = self.watch_server(d, server_name)
        if server is None:
            raise ValueError(f"{server_name} is not connected")
        return await server.watch_read(read, args)

    async def stop(self, slug: str) -> None:
        self._cancel_timer(slug)
        servers = self.own.pop(slug, None)
        if servers is not None:
            await servers.close()
            logger.info(f"department {slug}: servers closed")

    def _cancel_timer(self, slug: str) -> None:
        timer = self._timers.pop(slug, None)
        if timer is not None:
            timer.cancel()

    def sources_for(self, d: Department) -> list[Any]:
        """What a session in ``d`` reaches from the servers that are up now: Ghost's whole
        for Ghost; a department's own (prefixed) plus Ghost's servers it uses, through its
        own declaration. A used server that is not connected is left out."""
        if d.is_ghost:
            return self.ghost.sources()
        out: list[Any] = []
        own = self.own.get(d.slug)
        if own is not None:
            out.extend(own.sources(prefix=d.slug))
        for name in d.uses:
            server = self.ghost.servers.get(name)
            if server is None:
                continue
            view = server.view(d.spec_for_use(server.spec))
            source = source_of(view)
            if source.read_tools or source.operations:
                out.append(source)
        return out

    async def close(self) -> None:
        for slug in list(self.own):
            await self.stop(slug)
        await self.ghost.close()


def all_sources(fleet: Fleet) -> list[Any]:
    """Every source any department reaches, for registering operations with the gate and
    for the control API's listing: Ghost's servers, each department's own (as they are
    declared, connected or not), and each use."""
    out: list[Any] = list(fleet.ghost.sources())
    for d in fleet.departments.list():
        if d.is_ghost:
            continue
        out.extend(fleet.sources_for(d))
    return out
