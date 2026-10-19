"""Conversation routes (ADR 0014). Edits made here are the operator's own, applied
through the gate's internal path so they land in the same files Ghost writes, with
the same snapshot and audit row. Switching and compacting need a running
session and are delegated to the daemon."""

from __future__ import annotations

from typing import Any

from fastapi import FastAPI, HTTPException
from ghost_core import GateError
from ghost_core.conversations import ABILITY, Conversations, ConversationTarget, Profile
from ghost_core.conversations.models import (
    AppendNote,
    Create,
    Rename,
    SetMemory,
    SetNotes,
    SetPersona,
    SetScope,
    Settle,
    SetVoice,
    Unsettle,
    slugify,
)
from ghost_core.models import Change, ChangeSet
from ghost_core.policy import PolicyError

from ghost_api.app import Deps
from ghost_api.models import (
    Ack,
    CompactionView,
    ConversationCreate,
    ConversationDetail,
    ConversationUpdate,
    ConversationView,
    SampleRequest,
    TurnView,
    VoiceInfo,
)

UI_SESSION = "ui"

_LANG = {
    "a": "American",
    "b": "British",
    "e": "Spanish",
    "f": "French",
    "h": "Hindi",
    "i": "Italian",
    "j": "Japanese",
    "p": "Portuguese",
    "z": "Chinese",
}
_SEX = {"f": "female", "m": "male"}


def voice_label(voice_id: str) -> str:
    head, _, name = voice_id.partition("_")
    lang = _LANG.get(head[:1], head[:1]) if head else ""
    sex = _SEX.get(head[1:2], "") if len(head) > 1 else ""
    return f"{lang} {sex}: {name.capitalize()}".strip()


def add_conversation_routes(app: FastAPI, deps: Deps) -> None:
    convs: Conversations = deps.conversations
    st = deps.state

    def view(p: Profile) -> ConversationView:
        return ConversationView(
            slug=p.slug,
            name=p.name,
            aliases=list(p.aliases),
            voice=p.voice,
            speed=p.speed,
            scope={a: list(items) for a, items in p.scope.items()},
            created_at=p.created_at,
            last_active_at=p.last_active_at,
            settled_at=p.settled_at,
            active=st.conversation == p.slug,
            is_ghost=p.is_ghost,
        )

    def validated_scope(slug: str, scope: dict[str, list[str]]) -> dict[str, tuple[str, ...]]:
        try:
            return convs.validate_scope(slug, scope)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    def seed_memory(seed: str | None) -> str:
        if not seed:
            return ""
        try:
            return convs.handoff_memory(seed)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    def detail(p: Profile) -> ConversationDetail:
        return ConversationDetail(
            **view(p).model_dump(),
            persona=convs.files.persona(p.slug),
            memory=convs.files.memory(p.slug),
            notes=convs.files.note_lines(p.slug),
            turn_count=len(convs.turns(p.slug, limit=100000)),
            compactions=len(convs.compactions(p.slug)),
            dir=str(convs.files.dir(p.slug)),
        )

    def get(slug: str) -> Profile:
        p = convs.get(slug)
        if p is None:
            raise HTTPException(404, f"no conversation {slug!r}")
        return p

    async def apply(slug: str, op: str, change: Change, rationale: str = "") -> None:
        try:
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability=ABILITY,
                operation=op,
                target=ConversationTarget(slug=slug),
                changes=ChangeSet(steps=(change,)),
                rationale=rationale or f"{op} from the desktop app",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not applied")
        st.set_conversation(st.conversation, changed=slug)

    @app.get("/conversations", response_model=list[ConversationView], tags=["conversations"])
    async def list_conversations(settled: bool | None = None) -> list[ConversationView]:
        """Every live thread, Ghost first, settled ones included (``settled_at`` set) so
        the app can show its Settled group. ``settled=true`` or ``false`` filters."""
        out = [view(p) for p in convs.list(include_settled=True)]
        if settled is not None:
            out = [c for c in out if (c.settled_at is not None) == settled]
        return out

    @app.post("/conversations", response_model=ConversationDetail, tags=["conversations"])
    async def create_conversation(req: ConversationCreate) -> ConversationDetail:
        slug = slugify(req.name)
        voices = convs.voices()
        if req.voice and voices and req.voice not in voices:
            raise HTTPException(422, f"{req.voice!r} is not an installed voice")
        scope = validated_scope(slug, req.scope)
        memory = seed_memory(req.seed_from)
        await apply(
            slug,
            "create",
            Create(
                name=req.name.strip(),
                aliases=tuple(a.strip() for a in req.aliases if a.strip()),
                voice=req.voice or convs.unused_voice(),
                speed=req.speed if req.speed is not None else convs.default_speed,
                persona=req.persona,
                scope=scope,
                memory=memory,
            ),
        )
        return detail(get(slug))

    @app.get("/conversations/{slug}", response_model=ConversationDetail, tags=["conversations"])
    async def get_conversation(slug: str) -> ConversationDetail:
        return detail(get(slug))

    @app.patch("/conversations/{slug}", response_model=ConversationDetail, tags=["conversations"])
    async def update_conversation(slug: str, req: ConversationUpdate) -> ConversationDetail:
        p = get(slug)
        if req.name is not None or req.aliases is not None:
            await apply(
                slug,
                "rename",
                Rename(
                    name=(req.name or p.name).strip(),
                    aliases=tuple(
                        a.strip()
                        for a in (req.aliases if req.aliases is not None else p.aliases)
                        if a.strip()
                    ),
                ),
            )
        if req.voice is not None or req.speed is not None:
            voice = req.voice or p.voice
            voices = convs.voices()
            if voices and voice not in voices:
                raise HTTPException(422, f"{voice!r} is not an installed voice")
            await apply(
                slug,
                "set_voice",
                SetVoice(voice=voice, speed=req.speed if req.speed is not None else p.speed),
            )
        if req.persona is not None:
            await apply(slug, "set_persona", SetPersona(text=req.persona))
        if req.scope is not None:
            if p.is_ghost and any(req.scope.values()):
                raise HTTPException(422, "Ghost is unscoped and cannot be given a scope")
            await apply(slug, "set_scope", SetScope(scope=validated_scope(slug, req.scope)))
        if req.seed_from is not None:
            await apply(
                slug,
                "compact",
                SetMemory(text=seed_memory(req.seed_from), through_turn_id=None, source="edit"),
                rationale=f"memory seeded from {req.seed_from} in the desktop app",
            )
        if req.memory is not None:
            await apply(
                slug,
                "compact",
                SetMemory(text=req.memory, through_turn_id=None, source="edit"),
                rationale="memory edited in the desktop app",
            )
        if req.notes is not None:
            await apply(
                slug,
                "forget",
                SetNotes(text="\n".join(n.strip() for n in req.notes if n.strip())),
                rationale="notes edited in the desktop app",
            )
        return detail(get(slug))

    @app.post(
        "/conversations/{slug}/notes", response_model=ConversationDetail, tags=["conversations"]
    )
    async def add_note(slug: str, body: dict[str, Any]) -> ConversationDetail:
        get(slug)
        text = " ".join(str(body.get("text") or "").split())
        if not text:
            raise HTTPException(422, "text is required")
        await apply(slug, "remember", AppendNote(text=text))
        return detail(get(slug))

    @app.delete("/conversations/{slug}", response_model=Ack, tags=["conversations"])
    async def delete_conversation(slug: str) -> Ack:
        p = get(slug)
        if p.is_ghost:
            raise HTTPException(409, "Ghost cannot be deleted; reset it instead")
        if st.conversation == slug:
            raise HTTPException(409, "the session is in this conversation; switch first")
        from ghost_core.conversations.models import Delete

        await apply(slug, "delete", Delete())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/reset", response_model=Ack, tags=["conversations"])
    async def reset_conversation(slug: str) -> Ack:
        get(slug)
        if st.conversation == slug:
            raise HTTPException(409, "the session is in this conversation; switch or sleep first")
        from ghost_core.conversations.models import Reset

        await apply(slug, "reset", Reset())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/settle", response_model=Ack, tags=["conversations"])
    async def settle_conversation(slug: str) -> Ack:
        """Mark the thread done (ADR 0019): out of voice routing, everything kept."""
        p = get(slug)
        if p.is_ghost:
            raise HTTPException(409, "Ghost cannot be settled")
        if st.conversation == slug:
            raise HTTPException(409, "the session is in this conversation; switch first")
        if p.settled:
            return Ack(ok=True, detail="already settled")
        await apply(slug, "settle", Settle())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/unsettle", response_model=Ack, tags=["conversations"])
    async def unsettle_conversation(slug: str) -> Ack:
        p = get(slug)
        if not p.settled:
            return Ack(ok=True, detail="not settled")
        await apply(slug, "unsettle", Unsettle())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/switch", response_model=Ack, tags=["conversations"])
    async def switch_conversation(slug: str) -> Ack:
        p = get(slug)
        if p.settled:
            raise HTTPException(409, "that thread is settled; bring it back first")
        if not st.session_id:
            raise HTTPException(409, "no session; wake Ghost first")
        ok = await deps.control.switch_conversation(slug)
        return Ack(ok=ok, detail=None if ok else "could not switch")

    @app.post("/conversations/{slug}/compact", response_model=Ack, tags=["conversations"])
    async def compact_conversation(slug: str) -> Ack:
        get(slug)
        if not st.session_id or st.conversation != slug:
            raise HTTPException(409, "compacting needs a session in this conversation")
        ok = await deps.control.compact_conversation(slug)
        return Ack(ok=ok)

    @app.get("/conversations/{slug}/turns", response_model=list[TurnView], tags=["conversations"])
    async def conversation_turns(slug: str, limit: int = 200) -> list[TurnView]:
        get(slug)
        return [
            TurnView(
                id=t.id,
                session_id=t.session_id,
                role=t.role,
                text=t.text,
                at=t.at,
                compacted=t.compaction_id is not None,
            )
            for t in convs.turns(slug, limit=min(max(limit, 1), 2000))
        ]

    @app.get(
        "/conversations/{slug}/compactions",
        response_model=list[CompactionView],
        tags=["conversations"],
    )
    async def conversation_compactions(slug: str) -> list[CompactionView]:
        get(slug)
        return [
            CompactionView(
                id=c.id,
                at=c.at,
                source=c.source,
                summary=c.summary,
                through_turn_id=c.through_turn_id,
            )
            for c in convs.compactions(slug)
        ]

    @app.get("/voices", response_model=list[VoiceInfo], tags=["conversations"])
    async def voices() -> list[VoiceInfo]:
        return [VoiceInfo(id=v, label=voice_label(v)) for v in deps.control.voices()]

    @app.post("/voices/{voice}/sample", response_model=Ack, tags=["conversations"])
    async def sample_voice(voice: str, req: SampleRequest | None = None) -> Ack:
        req = req or SampleRequest()
        known = deps.control.voices()
        if known and voice not in known:
            raise HTTPException(404, f"no voice {voice!r}")
        ok = await deps.control.sample_voice(voice, req.speed, req.text)
        return Ack(ok=ok, detail=None if ok else "could not play a sample now")
