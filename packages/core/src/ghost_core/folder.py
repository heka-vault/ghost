"""Ghost's folder (ADR 0035): everything the operator edits, in one place::

    ~/.config/ghost/
    ├── AGENTS.md          # rules for every agent, as the operator wrote them
    ├── apps.json          # connected apps (ADR 0034)
    ├── watches.json       # what Ghost watches (ADR 0028)
    ├── hooks.json         # webhooks it takes (ADR 0027), fixtures in hooks/<name>/
    ├── skills/<name>/SKILL.md
    ├── out/               # what Ghost makes (ADR 0033)
    ├── agents/<slug>/     # agent.json, persona.md, memory.md, notes.md
    └── policy.toml, env, settings.env, apps.env

This module reads the folder. The model never writes ``watches.json``, ``hooks.json`` or
``apps.json``; a rule appended to ``AGENTS.md`` goes through the gate (``ghost_core.rules``).
What older layouts left here (a ``department.json``, a shared ``memory.md``, ``docs/``,
``departments/``) is not read; ``stale`` names each, with no migration.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

from ghost_core.hooks import Hook, parse_hooks
from ghost_core.watches import Watch, parse_watches

RULES_FILE = "AGENTS.md"
AGENTS_DIR = "agents"
WATCHES_FILE = "watches.json"
HOOKS_FILE = "hooks.json"
MAX_RULES_CHARS = 8000  # AGENTS.md as the prompt carries it
# Left by older layouts, no longer read (ADR 0034, ADR 0035).
OLD_APPS_FILE = "mcp.json"
OLD_DEPARTMENT_FILE = "department.json"
OLD_MEMORY_FILE = "memory.md"
OLD_DOCS_DIR = "docs"
OLD_DEPARTMENTS_DIR = "departments"


def read_text(path: Path, cap: int) -> str:
    """A regular file's text, capped; empty when it is missing, a link or unreadable."""
    try:
        if path.is_symlink() or not path.is_file():
            return ""
        text = path.read_text(errors="replace")
    except OSError:
        return ""
    return text if len(text) <= cap else text[:cap].rstrip() + "…"


def read_rules(config_dir: Path) -> str:
    """Ghost's ``AGENTS.md``, as every agent's prompt carries it."""
    return read_text(Path(config_dir) / RULES_FILE, MAX_RULES_CHARS)


def _read_json(file: Path, problems: list[str]) -> Any:
    """The file's object, None when there is no file; a broken one is a problem."""
    if not file.is_file():
        return None
    try:
        data = json.loads(file.read_text())
    except (ValueError, OSError) as e:
        problems.append(f"{file}: {e}")
        return None
    if not isinstance(data, dict):
        problems.append(f"{file}: the file must be an object keyed by name")
        return None
    return data


def stale(path: Path) -> list[str]:
    """What an older layout left in Ghost's folder that is no longer read (ADR 0035), one
    line each, for ``ghostd check`` and the waiting list. Nothing is moved for the operator."""
    out: list[str] = []
    old = path / OLD_DEPARTMENT_FILE
    if old.is_file():
        out.append(
            f"{old}: not read any more; put its watches in {path / WATCHES_FILE} and its "
            f"hooks in {path / HOOKS_FILE} by hand (its title, voice, permissions and "
            "servers are not read: permissions are in policy.toml, apps in the Apps screen)"
        )
    if (path / OLD_APPS_FILE).is_file():
        out.append(
            f"{path / OLD_APPS_FILE}: not read any more; rename it {path / 'apps.json'} "
            "(it reads the same shape)"
        )
    if (path / OLD_MEMORY_FILE).is_file():
        out.append(
            f"{path / OLD_MEMORY_FILE}: not read any more; each agent keeps its own memory "
            f"in {path / AGENTS_DIR}/<name>/memory.md"
        )
    if (path / OLD_DOCS_DIR).is_dir():
        out.append(
            f"{path / OLD_DOCS_DIR}: not read any more; make what Ghost should read on "
            "demand into a skill"
        )
    folder = path / OLD_DEPARTMENTS_DIR
    if folder.is_dir():
        for p in sorted(folder.iterdir()):
            out.append(
                f"{p}: not read any more; hire its agents again, "
                f"add its rules to {path / RULES_FILE} and its watches and hooks to "
                f"{WATCHES_FILE} and {HOOKS_FILE} by hand"
            )
    return out


class GhostFolder:
    """Ghost's folder as it is now. ``refresh`` re-reads ``watches.json`` and
    ``hooks.json`` (the desktop's reload, or the policy's) and runs ``on_refresh``."""

    def __init__(self, path: Path) -> None:
        self.path = Path(path).expanduser()
        self.watches: dict[str, Watch] = {}
        self.hooks: dict[str, Hook] = {}
        self.problems: list[str] = []
        # Called after every re-read (the daemon restarts the watches and hooks).
        self.on_refresh: list[Callable[[], None]] = []
        self.refresh()

    @property
    def rules_path(self) -> Path:
        return self.path / RULES_FILE

    @property
    def agents_dir(self) -> Path:
        return self.path / AGENTS_DIR

    @property
    def watches_path(self) -> Path:
        return self.path / WATCHES_FILE

    @property
    def hooks_path(self) -> Path:
        return self.path / HOOKS_FILE

    def rules(self) -> str:
        """``AGENTS.md``, as every agent's prompt carries it."""
        return read_rules(self.path)

    def ensure_layout(self) -> None:
        """The folder a fresh install needs: ``agents/`` for agent zero."""
        self.agents_dir.mkdir(parents=True, exist_ok=True)

    def refresh(self) -> None:
        problems: list[str] = []
        if self.rules_path.is_symlink():
            problems.append(f"{self.rules_path}: a link; Ghost reads and writes only regular files")
        body = _read_json(self.watches_path, problems)
        self.watches, found = parse_watches(body, str(self.watches_path))
        problems += found
        body = _read_json(self.hooks_path, problems)
        self.hooks, found = parse_hooks(body, str(self.hooks_path), self.path)
        problems += found
        self.problems = problems + stale(self.path)
        for hook in list(self.on_refresh):
            hook()


__all__ = [
    "AGENTS_DIR",
    "HOOKS_FILE",
    "MAX_RULES_CHARS",
    "RULES_FILE",
    "WATCHES_FILE",
    "GhostFolder",
    "read_rules",
    "read_text",
    "stale",
]
