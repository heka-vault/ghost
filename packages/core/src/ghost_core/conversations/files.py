"""The files an agent owns, in its folder under Ghost's ``agents/`` (ADR 0035).

``agent.json`` is who it is (id, name, aliases, job, its own voice when it has one);
``persona.md`` is how it speaks; ``memory.md`` is the current rolling summary;
``notes.md`` is what the operator asked to be remembered, one line each, verbatim.
The operator may open and edit any of them; Ghost writes them only through the
gate (``ConversationWriter``). Where a slug's folder is comes from the registry
(``locate``)."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

AGENT = "agent.json"
PERSONA = "persona.md"
MEMORY = "memory.md"
NOTES = "notes.md"


class ConversationFiles:
    def __init__(self, locate: Callable[[str], Path]) -> None:
        self._locate = locate

    def dir(self, slug: str) -> Path:
        return self._locate(slug)

    def path(self, slug: str, name: str) -> Path:
        return self.dir(slug) / name

    def read(self, slug: str, name: str) -> str:
        p = self.path(slug, name)
        return p.read_text() if p.exists() else ""

    def write(self, slug: str, name: str, text: str) -> None:
        d = self.dir(slug)
        d.mkdir(parents=True, exist_ok=True)
        p = self.path(slug, name)
        if text.strip():
            p.write_text(text if text.endswith("\n") else text + "\n")
        elif p.exists():
            p.write_text("")

    def write_agent(self, slug: str, data: dict[str, Any]) -> None:
        d = self.dir(slug)
        d.mkdir(parents=True, exist_ok=True)
        (d / AGENT).write_text(json.dumps(data, indent=2) + "\n")

    def persona(self, slug: str) -> str:
        return self.read(slug, PERSONA)

    def memory(self, slug: str) -> str:
        return self.read(slug, MEMORY)

    def notes(self, slug: str) -> str:
        return self.read(slug, NOTES)

    def note_lines(self, slug: str) -> list[str]:
        return [ln.strip() for ln in self.notes(slug).splitlines() if ln.strip()]

    def ensure(self, slug: str) -> None:
        self.dir(slug).mkdir(parents=True, exist_ok=True)


def read_agent_json(folder: Path) -> dict[str, Any] | None:
    """The identity an agent folder carries, or None when there is none or it is broken."""
    file = folder / AGENT
    if not file.is_file():
        return None
    try:
        data = json.loads(file.read_text())
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None
