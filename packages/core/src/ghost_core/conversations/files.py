"""The three Markdown files a conversation owns, under one folder per slug.

``persona.md`` is who this line is; ``memory.md`` is the current rolling summary;
``notes.md`` is what the operator asked to be remembered, one line each, verbatim.
The operator may open and edit any of them; Ghost writes them only through the
gate (``ConversationWriter``)."""

from __future__ import annotations

from pathlib import Path

PERSONA = "persona.md"
MEMORY = "memory.md"
NOTES = "notes.md"


class ConversationFiles:
    def __init__(self, root: Path) -> None:
        self.root = root

    def dir(self, slug: str) -> Path:
        return self.root / slug

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
