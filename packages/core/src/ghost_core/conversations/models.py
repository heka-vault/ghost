"""The write model for conversations (ADR 0014).

A conversation is a profile row in SQLite, three Markdown files, and a transcript.
The gate edits it through the same ``Target`` / ``Snapshot`` / ``Change`` shapes an
ability uses, so rename, reset and friends are snapshotted, bounded and audited.
The snapshot carries the full file texts: they are small, and the audit row then
keeps what was there before a reset or a compaction.
"""

from __future__ import annotations

import hashlib
import re
import unicodedata
from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field

from ghost_core.models import Change, Snapshot, Target, utcnow

GHOST = "ghost"  # conversation zero: always present, undeletable, resettable

MAX_NAME = 80
MAX_PERSONA = 4000
MAX_MEMORY = 20000
MAX_NOTES = 20000
MAX_NOTE = 1000

Role = Literal["user", "assistant", "call", "system"]


def slugify(name: str) -> str:
    """``"Accounting (EMEA)"`` -> ``"accounting-emea"``. Fixed at creation (ADR 0014)."""
    text = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    text = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return text[:MAX_NAME] or "conversation"


Scope = dict[str, tuple[str, ...]]  # ability -> the items this thread may touch (ADR 0019)


class Profile(BaseModel, frozen=True):
    slug: str
    name: str
    aliases: tuple[str, ...] = ()
    voice: str
    speed: float = 1.0
    # Per ability, what this thread may touch; an ability absent here is unscoped for
    # it. Ghost is always unscoped. Empty for every thread until the operator sets one.
    scope: Scope = Field(default_factory=dict)
    # ADR 0023. A line with no ``department`` is a department: the toolkits it is given
    # bound what its sessions reach (none: everything, like Ghost), and it is its own
    # primary line. A line with a ``department`` is a topic line in that department: it
    # reaches what the department reaches, with its own memory and scope.
    toolkits: tuple[str, ...] = ()
    department: str | None = None
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_active_at: datetime | None = None
    settled_at: datetime | None = None  # done: out of voice routing, still readable
    deleted_at: datetime | None = None

    @property
    def is_ghost(self) -> bool:
        return self.slug == GHOST

    @property
    def is_department(self) -> bool:
        return not self.is_ghost and self.department is None

    @property
    def settled(self) -> bool:
        return self.settled_at is not None

    def spoken_names(self) -> tuple[str, ...]:
        return (self.name, *self.aliases)

    def scope_for(self, ability: str) -> tuple[str, ...] | None:
        """The items this thread may touch for ``ability``; None when unscoped."""
        if self.is_ghost:
            return None
        return self.scope.get(ability)


class TurnRecord(BaseModel, frozen=True):
    id: int
    conversation: str
    session_id: str
    role: Role
    text: str
    at: datetime
    compaction_id: str | None = None  # set once a summary covers this turn


class Compaction(BaseModel, frozen=True):
    id: str
    conversation: str
    summary: str
    through_turn_id: int | None
    at: datetime
    source: Literal["auto", "manual", "edit"] = "auto"


# --- gate shapes -------------------------------------------------------------------


class ConversationTarget(Target, frozen=True):
    slug: str

    def key(self) -> str:
        return f"conversation:{self.slug}"

    def scope(self) -> str:
        return self.slug

    def spoken(self) -> str:
        return self.slug.replace("-", " ")


class ConversationSnapshot(Snapshot, frozen=True):
    exists: bool
    name: str = ""
    aliases: tuple[str, ...] = ()
    voice: str = ""
    speed: float = 1.0
    scope: Scope = Field(default_factory=dict)
    settled: bool = False
    persona: str = ""
    memory: str = ""
    notes: str = ""
    turns: int = 0

    @classmethod
    def missing(cls) -> ConversationSnapshot:
        return cls(version="missing", exists=False)

    @classmethod
    def of(
        cls,
        *,
        name: str,
        aliases: tuple[str, ...],
        voice: str,
        speed: float,
        persona: str,
        memory: str,
        notes: str,
        turns: int,
        deleted: bool = False,
        scope: Scope | None = None,
        settled: bool = False,
    ) -> ConversationSnapshot:
        scope = scope or {}
        h = hashlib.sha256()
        for part in (name, ",".join(aliases), voice, f"{speed:g}", persona, memory, notes):
            h.update(part.encode())
            h.update(b"\0")
        h.update(describe_scope(scope).encode())
        h.update(b"settled" if settled else b"open")
        h.update(str(turns).encode())
        h.update(b"deleted" if deleted else b"live")
        return cls(
            version=h.hexdigest()[:16],
            exists=not deleted,
            name=name,
            aliases=aliases,
            voice=voice,
            speed=speed,
            scope=scope,
            settled=settled,
            persona=persona,
            memory=memory,
            notes=notes,
            turns=turns,
        )

    def spoken(self) -> str:
        if not self.exists:
            return "does not exist"
        bits = [f"voice {self.voice}", f"{self.turns} turns on record"]
        if self.scope:
            bits.append("scoped to " + describe_scope(self.scope))
        if self.settled:
            bits.append("settled")
        if self.notes.strip():
            bits.append(f"{self.notes.count(chr(10)) + 1} notes")
        return f"{self.name}: " + ", ".join(bits)


def describe_scope(scope: Scope) -> str:
    """``github_issues: acme/app, acme/infra; notes: work`` or ``everything``."""
    parts = [f"{ab}: {', '.join(items)}" for ab, items in sorted(scope.items()) if items]
    return "; ".join(parts) if parts else "everything"


class Create(Change, frozen=True):
    kind: Literal["create"] = "create"
    name: str = Field(min_length=1, max_length=MAX_NAME)
    aliases: tuple[str, ...] = ()
    voice: str
    speed: float = Field(default=1.0, ge=0.5, le=2.0)
    persona: str = Field(default="", max_length=MAX_PERSONA)
    scope: Scope = Field(default_factory=dict)
    # A handoff: the first memory, copied from a settled thread's memory.md (ADR 0019).
    memory: str = Field(default="", max_length=MAX_MEMORY)
    toolkits: tuple[str, ...] = ()  # ADR 0023: a department's reach
    department: str | None = None  # ADR 0023: a topic line's department

    def describe(self) -> str:
        extra = f" scoped to {describe_scope(self.scope)}" if self.scope else ""
        if self.department:
            extra += f" in the {self.department} department"
        if self.toolkits:
            extra += " with the " + ", ".join(self.toolkits) + " toolkit(s)"
        seed = " with a handoff memory" if self.memory.strip() else ""
        return f"start conversation {self.name!r}{extra}{seed}"


class SetToolkits(Change, frozen=True):
    """Replace a department's toolkits (ADR 0023). Empty: everything, like Ghost."""

    kind: Literal["set_toolkits"] = "set_toolkits"
    toolkits: tuple[str, ...] = ()

    def describe(self) -> str:
        return "toolkits: " + (", ".join(self.toolkits) or "none, so everything")


class SetScope(Change, frozen=True):
    """Replace the thread's whole scope map. Ghost cannot be scoped."""

    kind: Literal["set_scope"] = "set_scope"
    scope: Scope = Field(default_factory=dict)

    def describe(self) -> str:
        return f"scope: {describe_scope(self.scope)}"


class Settle(Change, frozen=True):
    """Mark the thread done: out of voice routing and the active list, everything kept."""

    kind: Literal["settle"] = "settle"

    def describe(self) -> str:
        return "settle the thread"


class Unsettle(Change, frozen=True):
    kind: Literal["unsettle"] = "unsettle"

    def describe(self) -> str:
        return "bring the thread back"


class Rename(Change, frozen=True):
    kind: Literal["rename"] = "rename"
    name: str = Field(min_length=1, max_length=MAX_NAME)
    aliases: tuple[str, ...] = ()

    def describe(self) -> str:
        extra = f" (also {', '.join(self.aliases)})" if self.aliases else ""
        return f"rename to {self.name!r}{extra}"


class SetVoice(Change, frozen=True):
    kind: Literal["set_voice"] = "set_voice"
    voice: str
    speed: float = Field(default=1.0, ge=0.5, le=2.0)

    def describe(self) -> str:
        return f"voice {self.voice} at {self.speed:g}x"


class SetPersona(Change, frozen=True):
    kind: Literal["set_persona"] = "set_persona"
    text: str = Field(max_length=MAX_PERSONA)

    def describe(self) -> str:
        return f"persona: {self.text.strip()[:60]!r}" if self.text.strip() else "clear persona"


class AppendNote(Change, frozen=True):
    kind: Literal["append_note"] = "append_note"
    text: str = Field(min_length=1, max_length=MAX_NOTE)

    def describe(self) -> str:
        return f"remember: {self.text.strip()}"


class SetNotes(Change, frozen=True):
    """The whole notes file. Produced by ``forget``."""

    kind: Literal["set_notes"] = "set_notes"
    text: str = Field(max_length=MAX_NOTES)

    def describe(self) -> str:
        return "update notes"


class SetMemory(Change, frozen=True):
    """Replace the rolling summary (a compaction, or an edit in the app)."""

    kind: Literal["set_memory"] = "set_memory"
    text: str = Field(max_length=MAX_MEMORY)
    through_turn_id: int | None = None
    source: Literal["auto", "manual", "edit"] = "auto"

    def describe(self) -> str:
        return "compact" if self.source != "edit" else "edit memory"


class Reset(Change, frozen=True):
    """Wipe memory, notes and the transcript; keep the profile. Turns are hidden with
    the marker rather than deleted, and pruned later with the summarized ones."""

    kind: Literal["reset"] = "reset"

    def describe(self) -> str:
        return "reset: forget everything"


class Delete(Change, frozen=True):
    kind: Literal["delete"] = "delete"

    def describe(self) -> str:
        return "delete the conversation"
