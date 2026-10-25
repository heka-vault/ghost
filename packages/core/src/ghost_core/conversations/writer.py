"""Applies agent changes: the folder in its department plus the mirror row in SQLite."""

from __future__ import annotations

import shutil
import uuid
from collections.abc import Callable
from pathlib import Path

from ghost_core.conversations.files import MEMORY, NOTES, PERSONA, ConversationFiles
from ghost_core.conversations.models import (
    GHOST,
    AppendNote,
    Compaction,
    ConversationSnapshot,
    ConversationTarget,
    Create,
    Delete,
    Move,
    Profile,
    Rehire,
    Rename,
    Reset,
    Retire,
    SetMemory,
    SetNotes,
    SetPersona,
    SetVoice,
    slugify,
)
from ghost_core.models import Change, Snapshot, Target, utcnow
from ghost_core.store import Store


class ConversationWriter:
    def __init__(
        self,
        store: Store,
        files: ConversationFiles,
        *,
        agents_dir: Callable[[str], Path | None],
    ) -> None:
        """``agents_dir`` gives a department's ``agents/`` folder, or None for a department
        that does not exist."""
        self._store = store
        self._files = files
        self._agents_dir = agents_dir

    def allowed(self, target: Target) -> bool:
        return isinstance(target, ConversationTarget) and bool(target.slug)

    async def fetch(self, target: Target) -> Snapshot:
        assert isinstance(target, ConversationTarget)
        p = self._store.get_conversation(target.slug, include_deleted=True)
        if p is None:
            return ConversationSnapshot.missing()
        deleted = p.deleted_at is not None
        return ConversationSnapshot.of(
            name=p.name,
            aliases=p.aliases,
            department=p.department,
            job=p.job,
            voice=p.voice,
            speed=p.speed,
            persona=self._files.persona(p.slug) if not deleted else "",
            memory=self._files.memory(p.slug) if not deleted else "",
            notes=self._files.notes(p.slug) if not deleted else "",
            turns=self._store.turn_count(p.id),
            deleted=deleted,
            retired=p.retired,
        )

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        assert isinstance(target, ConversationTarget)
        slug = target.slug
        store, files = self._store, self._files
        match change:
            case Create(
                name=name,
                aliases=aliases,
                department=department,
                job=job,
                voice=voice,
                speed=speed,
                persona=persona,
                memory=memory,
            ):
                if self._agents_dir(department) is None:
                    raise ValueError(f"there is no department called {department!r}")
                if store.get_conversation(slug, include_deleted=True) is not None:
                    raise ValueError(f"{name!r} already exists")
                folder = self._agents_dir(department) / slug  # type: ignore[operator]
                if folder.exists():
                    raise ValueError(f"{folder} already exists; pick another name")
                p = Profile(
                    slug=slug,
                    name=name,
                    aliases=aliases,
                    department=department,
                    job=job,
                    voice=voice,
                    speed=speed,
                )
                store.upsert_conversation(p)
                files.write_agent(slug, p.agent_json())
                files.write(slug, PERSONA, persona)
                if memory.strip():
                    files.write(slug, MEMORY, memory)
                    store.add_compaction(
                        Compaction(
                            id=uuid.uuid4().hex,
                            conversation=p.id,
                            summary=memory,
                            through_turn_id=None,
                            at=utcnow(),
                            source="manual",
                        )
                    )
            case Retire():
                p = self._live(slug)
                if p.is_default:
                    raise ValueError(f"{p.name} is its department's own agent; it stays")
                if not p.retired:
                    self._save(
                        p.model_copy(update={"retired_at": utcnow(), "updated_at": utcnow()})
                    )
            case Rehire():
                p = self._live(slug)
                if p.retired:
                    self._save(p.model_copy(update={"retired_at": None, "updated_at": utcnow()}))
            case Move(department=department):
                p = self._live(slug)
                if p.is_default:
                    raise ValueError(f"{p.name} is its department's own agent; it cannot move")
                dest = self._agents_dir(department)
                if dest is None:
                    raise ValueError(f"there is no department called {department!r}")
                if department == p.department:
                    raise ValueError(f"{p.name} is already in {department}")
                if (dest / slug).exists():
                    raise ValueError(f"{dest / slug} already exists")
                src = files.dir(slug)
                dest.mkdir(parents=True, exist_ok=True)
                if src.is_dir():
                    shutil.move(str(src), str(dest / slug))
                store.upsert_conversation(
                    p.model_copy(update={"department": department, "updated_at": utcnow()})
                )
            case Rename(name=name, aliases=aliases, job=job):
                if slug == GHOST:
                    raise ValueError("Ghost keeps its name")
                p = self._live(slug)
                if p.is_default and slugify(name) != slug:
                    raise ValueError(f"{p.name} is named for its department")
                self._save(
                    p.model_copy(
                        update={
                            "name": name,
                            "aliases": aliases,
                            "job": p.job if job is None else job,
                            "updated_at": utcnow(),
                        }
                    )
                )
            case SetVoice(voice=voice, speed=speed):
                p = self._live(slug)
                self._save(
                    p.model_copy(update={"voice": voice, "speed": speed, "updated_at": utcnow()})
                )
            case SetPersona(text=text):
                self._live(slug)
                files.write(slug, PERSONA, text)
            case AppendNote(text=text):
                self._live(slug)
                current = files.notes(slug)
                line = " ".join(text.split())
                files.write(
                    slug, NOTES, (current.rstrip("\n") + "\n" if current.strip() else "") + line
                )
            case SetNotes(text=text):
                self._live(slug)
                files.write(slug, NOTES, text)
            case SetMemory(text=text, through_turn_id=through, source=source):
                p = self._live(slug)
                files.write(slug, MEMORY, text)
                cid = uuid.uuid4().hex
                store.add_compaction(
                    Compaction(
                        id=cid,
                        conversation=p.id,
                        summary=text,
                        through_turn_id=through,
                        at=utcnow(),
                        source=source,
                    )
                )
                if through is not None:
                    store.mark_compacted(p.id, through, cid)
            case Reset():
                p = self._live(slug)
                files.write(slug, MEMORY, "")
                files.write(slug, NOTES, "")
                store.tag_turns(p.id, idempotency_key)
            case Delete():
                p = self._live(slug)
                if p.is_default:
                    raise ValueError(f"{p.name} is its department's own agent; it stays")
                folder = files.dir(slug)
                store.upsert_conversation(p.model_copy(update={"deleted_at": utcnow()}))
                store.tag_turns(p.id, idempotency_key)
                if folder.is_dir():
                    shutil.rmtree(folder)
            case _:
                raise TypeError(f"ConversationWriter cannot apply {type(change).__name__}")

    def _save(self, p: Profile) -> None:
        """Identity changes land in both places: the row and the folder's agent.json."""
        self._store.upsert_conversation(p)
        self._files.write_agent(p.slug, p.agent_json())

    def _live(self, slug: str) -> Profile:
        p = self._store.get_conversation(slug)
        if p is None:
            raise ValueError(f"no agent {slug!r}")
        return p
