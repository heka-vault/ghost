"""Applies conversation changes: the row in SQLite plus the files on disk."""

from __future__ import annotations

import uuid

from ghost_core.conversations.files import MEMORY, NOTES, PERSONA, ConversationFiles
from ghost_core.conversations.models import (
    GHOST,
    AppendNote,
    Compaction,
    ConversationSnapshot,
    ConversationTarget,
    Create,
    Delete,
    Profile,
    Rename,
    Reset,
    SetMemory,
    SetNotes,
    SetPersona,
    SetScope,
    Settle,
    SetVoice,
    Unsettle,
)
from ghost_core.models import Change, Snapshot, Target, utcnow
from ghost_core.store import Store


class ConversationWriter:
    def __init__(self, store: Store, files: ConversationFiles) -> None:
        self._store = store
        self._files = files

    def allowed(self, target: Target) -> bool:
        return isinstance(target, ConversationTarget) and bool(target.slug)

    async def fetch(self, target: Target) -> Snapshot:
        assert isinstance(target, ConversationTarget)
        p = self._store.get_conversation(target.slug, include_deleted=True)
        if p is None:
            return ConversationSnapshot.missing()
        return ConversationSnapshot.of(
            name=p.name,
            aliases=p.aliases,
            voice=p.voice,
            speed=p.speed,
            persona=self._files.persona(p.slug),
            memory=self._files.memory(p.slug),
            notes=self._files.notes(p.slug),
            turns=self._store.turn_count(p.slug),
            deleted=p.deleted_at is not None,
            scope=p.scope,
            settled=p.settled,
        )

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        assert isinstance(target, ConversationTarget)
        slug = target.slug
        store, files = self._store, self._files
        match change:
            case Create(
                name=name,
                aliases=aliases,
                voice=voice,
                speed=speed,
                persona=persona,
                scope=scope,
                memory=memory,
            ):
                if store.get_conversation(slug) is not None:
                    raise ValueError(f"{name!r} already exists")
                if slug == GHOST and scope:
                    raise ValueError("Ghost cannot be scoped")
                store.upsert_conversation(
                    Profile(
                        slug=slug, name=name, aliases=aliases, voice=voice, speed=speed, scope=scope
                    )
                )
                files.ensure(slug)
                files.write(slug, PERSONA, persona)
                if memory.strip():
                    files.write(slug, MEMORY, memory)
                    store.add_compaction(
                        Compaction(
                            id=uuid.uuid4().hex,
                            conversation=slug,
                            summary=memory,
                            through_turn_id=None,
                            at=utcnow(),
                            source="manual",
                        )
                    )
            case SetScope(scope=scope):
                if slug == GHOST:
                    raise ValueError("Ghost is unscoped and cannot be given a scope")
                p = self._live(slug)
                store.upsert_conversation(
                    p.model_copy(update={"scope": scope, "updated_at": utcnow()})
                )
            case Settle():
                if slug == GHOST:
                    raise ValueError("Ghost cannot be settled")
                p = self._live(slug)
                if not p.settled:
                    store.upsert_conversation(
                        p.model_copy(update={"settled_at": utcnow(), "updated_at": utcnow()})
                    )
            case Unsettle():
                p = self._live(slug)
                if p.settled:
                    store.upsert_conversation(
                        p.model_copy(update={"settled_at": None, "updated_at": utcnow()})
                    )
            case Rename(name=name, aliases=aliases):
                if slug == GHOST:
                    raise ValueError("Ghost keeps its name")
                p = self._live(slug)
                store.upsert_conversation(
                    p.model_copy(update={"name": name, "aliases": aliases, "updated_at": utcnow()})
                )
            case SetVoice(voice=voice, speed=speed):
                p = self._live(slug)
                store.upsert_conversation(
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
                self._live(slug)
                files.write(slug, MEMORY, text)
                cid = uuid.uuid4().hex
                store.add_compaction(
                    Compaction(
                        id=cid,
                        conversation=slug,
                        summary=text,
                        through_turn_id=through,
                        at=utcnow(),
                        source=source,
                    )
                )
                if through is not None:
                    store.mark_compacted(slug, through, cid)
            case Reset():
                self._live(slug)
                files.write(slug, MEMORY, "")
                files.write(slug, NOTES, "")
                store.tag_turns(slug, idempotency_key)
            case Delete():
                if slug == GHOST:
                    raise ValueError("Ghost cannot be deleted")
                p = self._live(slug)
                store.upsert_conversation(p.model_copy(update={"deleted_at": utcnow()}))
                store.tag_turns(slug, idempotency_key)
            case _:
                raise TypeError(f"ConversationWriter cannot apply {type(change).__name__}")

    def _live(self, slug: str) -> Profile:
        p = self._store.get_conversation(slug)
        if p is None:
            raise ValueError(f"no conversation {slug!r}")
        return p
