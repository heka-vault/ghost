"""The write model for agents (ADR 0014, ADR 0035).

An agent is a conversation with a persona and a job: a folder under Ghost's ``agents/``
(``agent.json``, ``persona.md``, ``memory.md``, ``notes.md``), a mirror row in SQLite, and a
transcript keyed by its stable id. The operator's word is *agent*; the code and the control
API keep *conversation*. The gate edits it through the same ``Target`` / ``Snapshot`` /
``Change`` shapes an app's writes use, so hire, retire and the rest are
snapshotted, bounded and audited. The snapshot carries the full file texts: they are
small, and the audit row then keeps what was there before a reset or a compaction.
"""

from __future__ import annotations

import hashlib
import re
import unicodedata
import uuid
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, Field

from ghost_core.models import Change, Snapshot, Target, utcnow

GHOST = "ghost"  # agent zero, where every wake lands

MAX_NAME = 80
MAX_JOB = 200
MAX_PERSONA = 4000
MAX_MEMORY = 20000
MAX_NOTES = 20000
MAX_NOTE = 1000

Role = Literal["user", "assistant", "call", "system"]


def slugify(name: str) -> str:
    """``"Brand design (EMEA)"`` -> ``"brand-design-emea"``. Fixed at creation (ADR 0014)."""
    text = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    text = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return text[:MAX_NAME] or "agent"


def new_id() -> str:
    return uuid.uuid4().hex


class Profile(BaseModel, frozen=True):
    id: str = Field(default_factory=new_id)
    slug: str
    name: str
    aliases: tuple[str, ...] = ()
    job: str = ""  # what it was hired for, in a line
    voice: str | None = None  # None: Ghost's voice (ADR 0035)
    speed: float | None = None
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_active_at: datetime | None = None
    retired_at: datetime | None = None  # off the roster and voice routing; files kept
    deleted_at: datetime | None = None

    @property
    def is_ghost(self) -> bool:
        return self.slug == GHOST

    @property
    def retired(self) -> bool:
        return self.retired_at is not None

    def spoken_names(self) -> tuple[str, ...]:
        return (self.name, *self.aliases)

    def agent_json(self) -> dict[str, Any]:
        """What the folder's ``agent.json`` carries: identity, never state."""
        out: dict[str, Any] = {"id": self.id, "name": self.name}
        if self.aliases:
            out["aliases"] = list(self.aliases)
        if self.job:
            out["job"] = self.job
        if self.voice:
            out["voice"] = self.voice
        if self.speed is not None:
            out["speed"] = self.speed
        return out


class TurnRecord(BaseModel, frozen=True):
    id: int
    conversation: str  # the agent's id
    session_id: str
    role: Role
    text: str
    at: datetime
    compaction_id: str | None = None  # set once a summary covers this turn


class Compaction(BaseModel, frozen=True):
    id: str
    conversation: str  # the agent's id
    summary: str
    through_turn_id: int | None
    at: datetime
    source: Literal["auto", "manual", "edit"] = "auto"


# --- gate shapes -------------------------------------------------------------------


class ConversationTarget(Target, frozen=True):
    slug: str

    def key(self) -> str:
        return f"agent:{self.slug}"

    def scope(self) -> str:
        return self.slug

    def spoken(self) -> str:
        return self.slug.replace("-", " ")

    def mentions(self) -> set[str]:
        words = set(self.slug.split("-"))
        return {self.spoken()} | {w for w in words if len(w) > 2}


class ConversationSnapshot(Snapshot, frozen=True):
    exists: bool
    name: str = ""
    aliases: tuple[str, ...] = ()
    job: str = ""
    voice: str | None = None
    speed: float | None = None
    retired: bool = False
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
        job: str,
        voice: str | None,
        speed: float | None,
        persona: str,
        memory: str,
        notes: str,
        turns: int,
        deleted: bool = False,
        retired: bool = False,
    ) -> ConversationSnapshot:
        h = hashlib.sha256()
        for part in (
            name,
            ",".join(aliases),
            job,
            voice or "",
            f"{speed:g}" if speed is not None else "",
            persona,
            memory,
            notes,
        ):
            h.update(part.encode())
            h.update(b"\0")
        h.update(b"retired" if retired else b"active")
        h.update(str(turns).encode())
        h.update(b"deleted" if deleted else b"live")
        return cls(
            version=h.hexdigest()[:16],
            exists=not deleted,
            name=name,
            aliases=aliases,
            job=job,
            voice=voice,
            speed=speed,
            retired=retired,
            persona=persona,
            memory=memory,
            notes=notes,
            turns=turns,
        )

    def spoken(self) -> str:
        if not self.exists:
            return "does not exist"
        bits = [f"{self.turns} turns on record"]
        if self.job:
            bits.insert(0, self.job)
        if self.voice:
            bits.append(f"voice {self.voice}")
        if self.retired:
            bits.append("retired")
        if self.notes.strip():
            bits.append(f"{self.notes.count(chr(10)) + 1} notes")
        return f"{self.name}: " + ", ".join(bits)


class Create(Change, frozen=True):
    """Hire: a new agent, with a persona and a job."""

    kind: Literal["create"] = "create"
    name: str = Field(min_length=1, max_length=MAX_NAME)
    aliases: tuple[str, ...] = ()
    job: str = Field(default="", max_length=MAX_JOB)
    voice: str | None = None
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str = Field(default="", max_length=MAX_PERSONA)
    # The first memory: a handoff from a retired agent, or a summary of this session.
    memory: str = Field(default="", max_length=MAX_MEMORY)

    def describe(self) -> str:
        job = f" for {self.job}" if self.job.strip() else ""
        seed = " with a handoff memory" if self.memory.strip() else ""
        return f"hire {self.name!r}{job}{seed}"


class Retire(Change, frozen=True):
    """Fire: off the roster, out of voice routing and the prompt; everything kept."""

    kind: Literal["retire"] = "retire"

    def describe(self) -> str:
        return "retire the agent"


class Rehire(Change, frozen=True):
    kind: Literal["rehire"] = "rehire"

    def describe(self) -> str:
        return "bring the agent back"


class Rename(Change, frozen=True):
    kind: Literal["rename"] = "rename"
    name: str = Field(min_length=1, max_length=MAX_NAME)
    aliases: tuple[str, ...] = ()
    job: str | None = Field(default=None, max_length=MAX_JOB)  # None: unchanged

    def describe(self) -> str:
        extra = f" (also {', '.join(self.aliases)})" if self.aliases else ""
        job = f", job: {self.job}" if self.job is not None else ""
        return f"rename to {self.name!r}{extra}{job}"


class SetVoice(Change, frozen=True):
    kind: Literal["set_voice"] = "set_voice"
    voice: str | None = None  # None: back to Ghost's voice
    speed: float | None = Field(default=None, ge=0.5, le=2.0)

    def describe(self) -> str:
        if self.voice is None:
            return "voice: Ghost's"
        speed = f" at {self.speed:g}x" if self.speed is not None else ""
        return f"voice {self.voice}{speed}"


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
    """Remove the agent: its folder goes, its transcript is hidden. The operator's, in
    the desktop; retire is the usual way."""

    kind: Literal["delete"] = "delete"

    def describe(self) -> str:
        return "delete the agent"
