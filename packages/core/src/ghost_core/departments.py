"""Departments (ADR 0024): a department is a folder, and Ghost is one too.

Everything the operator edits lives under ``~/.config/ghost/``::

    ~/.config/ghost/                    # Ghost: the department every wake lands in
    ├── AGENTS.md                       # Ghost's standing instructions
    ├── apps.json                       # the apps connected to Ghost (ADR 0034)
    ├── department.json                 # Ghost's title, voice, watches, hooks
    ├── memory.md                       # what every agent in Ghost knows (gate-written)
    ├── skills/<name>/SKILL.md
    ├── agents/ghost/                   # agent zero
    └── departments/
        └── marketing/
            ├── AGENTS.md               # the department's rules, in every agent's prompt
            ├── department.json         # title, aliases, voice, root, permissions, watches
            ├── memory.md               # the department's shared memory, gate-written
            ├── skills/<name>/SKILL.md
            ├── docs/…                  # read on demand
            └── agents/<slug>/          # agent.json, persona.md, memory.md, notes.md

Apps are Ghost's: connected once, reached from every department (ADR 0034). A department
narrows what its sessions may do with ``permissions`` in its ``department.json``: ``block``,
``ask`` and ``quiet`` rules, added to the operator's own (``ghost_core.permissions``); it
cannot ``allow``. A department may keep its resources in a ``root`` folder elsewhere (a
repository): its ``AGENTS.md``, skills and ``docs/`` are read in place; its agents stay here.

The model never writes ``department.json``. This module reads the folders; what Ghost
writes (``memory.md``, a rule appended to ``AGENTS.md``) goes through the gate.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from loguru import logger

from ghost_core.hooks import Hook, parse_hooks
from ghost_core.permissions import Permissions
from ghost_core.policy import AutonomyPolicy
from ghost_core.watches import Watch, parse_watches

GHOST = "ghost"
DEPARTMENTS_DIR = "departments"
OLD_APPS_FILE = "mcp.json"  # a department's servers, retired by ADR 0034
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
_DEPARTMENT_KEYS = {
    "title",
    "aliases",
    "voice",
    "speed",
    "model",
    "root",
    "permissions",  # ADR 0034
    "watches",  # ADR 0028
    "hooks",  # ADR 0027
}
_RETIRED_KEYS = {"servers", "root_servers"}  # ADR 0034: apps are Ghost's, with no declarations
_STOP = {"the", "a", "an", "to", "me", "department", "dept", "team", "office", "desk", "folks"}


class DepartmentError(ValueError):
    """A folder that cannot be read as a department. Safe to show the operator."""


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
    permissions: Permissions = field(default_factory=Permissions)  # ADR 0034: narrows only
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

    # --- what the gate sees ------------------------------------------------------------

    def policy(self, base: AutonomyPolicy) -> AutonomyPolicy:
        """The policy a session in this department runs under: the operator's, with this
        department's own rules added (they only ever add care)."""
        return base.with_rules(self.permissions, str(self.path / DEPARTMENT_FILE))


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
    unknown = sorted(set(data) - _DEPARTMENT_KEYS - _RETIRED_KEYS)
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

    retired = sorted(set(data) & _RETIRED_KEYS)
    if retired:
        problems.append(
            f"{file}: {', '.join(retired)} is no longer read: apps are connected to Ghost "
            "once, in the Apps screen, and a department narrows them with permissions"
        )
    # A repository's own .mcp.json (through root) is its coding tools' business, not Ghost's.
    if (path / OLD_APPS_FILE).is_file():
        problems.append(
            f"{path / OLD_APPS_FILE}: not read any more; "
            + (
                f"rename it {path / 'apps.json'} (it reads the same shape)"
                if is_ghost
                else "connect its apps in Ghost's Apps screen"
            )
        )
    permissions = Permissions()
    body = data.get("permissions")
    if body is not None and is_ghost:
        problems.append(f"{file}: Ghost's permissions are in policy.toml; these are ignored")
    elif body is not None:
        try:
            if not isinstance(body, Mapping):
                raise ValueError(f"{file}: permissions must be an object of rule lists")
            permissions = Permissions.parse(body, f"{file}: permissions", narrow_only=True)
        except ValueError as e:
            problems.append(str(e))
    watches, watch_problems = parse_watches(data.get("watches"), slug, str(file))
    problems.extend(watch_problems)
    hooks, hook_problems = parse_hooks(data.get("hooks"), slug, str(file), path)
    problems.extend(hook_problems)
    return Department(
        slug=slug,
        path=path,
        title=title,
        aliases=aliases,
        voice=voice,
        speed=speed,
        model=model,
        root=root,
        permissions=permissions,
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
    return out


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
    "MEMORY_FILE",
    "RULES_FILE",
    "SKILL_FOLDERS",
    "Department",
    "DepartmentError",
    "Departments",
    "discover",
    "load_department",
    "slugify",
]
