"""Agent routes (ADR 0014, ADR 0024), still ``/conversations`` on the wire. Edits made
here are the operator's own, applied through the gate's internal path so they land in the
same files Ghost writes, with the same snapshot and audit row. Switching and compacting
need a running session and are delegated to the daemon."""

from __future__ import annotations

from typing import Any

from fastapi import FastAPI, HTTPException
from ghost_core import GateError
from ghost_core.conversations import SOURCE, Conversations, ConversationTarget, Profile
from ghost_core.conversations.models import (
    AppendNote,
    Create,
    Delete,
    Move,
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
from ghost_core.models import Change, ChangeSet
from ghost_core.policy import PolicyError

from ghost_api.app import Deps
from ghost_api.models import (
    Ack,
    CompactionView,
    ConversationCreate,
    ConversationDetail,
    ConversationMove,
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


def conversation_view(convs: Conversations, p: Profile, active: str | None) -> ConversationView:
    """The control API's view of an agent; the phone listener serves the same one."""
    voice, speed = convs.voice_for(p)
    return ConversationView(
        id=p.id,
        slug=p.slug,
        name=p.name,
        aliases=list(p.aliases),
        department=p.department,
        job=p.job,
        voice=voice,
        speed=speed,
        own_voice=p.voice is not None,
        created_at=p.created_at,
        last_active_at=p.last_active_at,
        retired_at=p.retired_at,
        active=active == p.slug,
        is_ghost=p.is_ghost,
        is_default=p.is_default,
    )


def conversation_detail(convs: Conversations, p: Profile, active: str | None) -> ConversationDetail:
    """The full view of an agent: the profile plus its files and counts."""
    return ConversationDetail(
        **conversation_view(convs, p, active).model_dump(),
        persona=convs.files.persona(p.slug),
        memory=convs.files.memory(p.slug),
        notes=convs.files.note_lines(p.slug),
        turn_count=len(convs.turns(p.slug, limit=100000)),
        compactions=len(convs.compactions(p.slug)),
        dir=str(convs.files.dir(p.slug)),
    )


def add_conversation_routes(app: FastAPI, deps: Deps) -> None:
    convs: Conversations = deps.conversations
    st = deps.state

    def view(p: Profile) -> ConversationView:
        return conversation_view(convs, p, st.conversation)

    def validated_department(slug: str | None) -> str:
        if not slug:
            return "ghost"
        d = convs.departments.get(slug) or convs.departments.resolve(slug)
        if d is None:
            raise HTTPException(422, f"there is no department called {slug!r}")
        return d.slug

    def seed_memory(seed: str | None) -> str:
        if not seed:
            return ""
        try:
            return convs.handoff_memory(seed)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    def detail(p: Profile) -> ConversationDetail:
        return conversation_detail(convs, p, st.conversation)

    def get(slug: str) -> Profile:
        p = convs.get(slug)
        if p is None:
            raise HTTPException(404, f"no agent {slug!r}")
        return p

    async def apply(slug: str, op: str, change: Change, rationale: str = "") -> None:
        try:
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability=SOURCE,
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
    async def list_conversations(
        retired: bool | None = None, department: str | None = None
    ) -> list[ConversationView]:
        """Every agent, Ghost first, retired ones included (``retired_at`` set) so the app
        can show its Retired group. ``retired=true`` or ``false`` filters; ``department``
        narrows to one roster."""
        out = [view(p) for p in convs.list(include_retired=True, department=department)]
        if retired is not None:
            out = [c for c in out if (c.retired_at is not None) == retired]
        return out

    @app.post("/conversations", response_model=ConversationDetail, tags=["conversations"])
    async def create_conversation(req: ConversationCreate) -> ConversationDetail:
        """Hire an agent into a department."""
        slug = slugify(req.name)
        voices = convs.voices()
        if req.voice and voices and req.voice not in voices:
            raise HTTPException(422, f"{req.voice!r} is not an installed voice")
        if convs.departments.get(slug) is not None:
            raise HTTPException(422, f"{req.name!r} is a department's name; pick another")
        memory = seed_memory(req.seed_from)
        await apply(
            slug,
            "create",
            Create(
                name=req.name.strip(),
                aliases=tuple(a.strip() for a in req.aliases if a.strip()),
                department=validated_department(req.department),
                job=" ".join(req.job.split()),
                voice=req.voice or None,
                speed=req.speed,
                persona=req.persona,
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
        if req.name is not None or req.aliases is not None or req.job is not None:
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
                    job=" ".join(req.job.split()) if req.job is not None else None,
                ),
            )
        if req.voice is not None or req.speed is not None:
            voice = req.voice if req.voice else None  # "" means the department's
            voices = convs.voices()
            if voice and voices and voice not in voices:
                raise HTTPException(422, f"{voice!r} is not an installed voice")
            await apply(
                slug,
                "set_voice",
                SetVoice(
                    voice=voice if req.voice is not None else p.voice,
                    speed=req.speed if req.speed is not None else p.speed,
                ),
            )
        if req.persona is not None:
            await apply(slug, "set_persona", SetPersona(text=req.persona))
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
        if p.is_default:
            raise HTTPException(409, f"{p.name} is its department's own agent; it stays")
        if st.conversation == slug:
            raise HTTPException(409, "the session is with this agent; switch first")
        await apply(slug, "delete", Delete())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/reset", response_model=Ack, tags=["conversations"])
    async def reset_conversation(slug: str) -> Ack:
        get(slug)
        if st.conversation == slug:
            raise HTTPException(409, "the session is with this agent; switch or sleep first")
        await apply(slug, "reset", Reset())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/retire", response_model=Ack, tags=["conversations"])
    async def retire_conversation(slug: str) -> Ack:
        """Retire the agent (ADR 0024): off the roster and voice routing, everything kept."""
        p = get(slug)
        if p.is_default:
            raise HTTPException(409, f"{p.name} is its department's own agent; it stays")
        if st.conversation == slug:
            raise HTTPException(409, "the session is with this agent; switch first")
        if p.retired:
            return Ack(ok=True, detail="already retired")
        await apply(slug, "retire", Retire())
        return Ack(ok=True)

    @app.post("/conversations/{slug}/rehire", response_model=Ack, tags=["conversations"])
    async def rehire_conversation(slug: str) -> Ack:
        p = get(slug)
        if not p.retired:
            return Ack(ok=True, detail="not retired")
        await apply(slug, "rehire", Rehire())
        return Ack(ok=True)

    @app.post(
        "/conversations/{slug}/move", response_model=ConversationDetail, tags=["conversations"]
    )
    async def move_conversation(slug: str, req: ConversationMove) -> ConversationDetail:
        """Move the agent's folder into another department (ADR 0024). The desktop's
        drag; a move of the agent the session is with is the session's to make."""
        p = get(slug)
        if p.is_default:
            raise HTTPException(409, f"{p.name} is its department's own agent; it cannot move")
        if st.conversation == slug:
            raise HTTPException(409, "the session is with this agent; ask Ghost to move it")
        department = validated_department(req.department)
        if department == p.department:
            raise HTTPException(409, f"{p.name} is already in {department}")
        await apply(slug, "move", Move(department=department))
        return detail(get(slug))

    @app.post("/conversations/{slug}/switch", response_model=Ack, tags=["conversations"])
    async def switch_conversation(slug: str) -> Ack:
        p = get(slug)
        if p.retired:
            raise HTTPException(409, "that agent is retired; rehire it first")
        if not st.session_id:
            raise HTTPException(409, "no session; wake Ghost first")
        ok = await deps.control.switch_conversation(slug)
        return Ack(ok=ok, detail=None if ok else "could not switch")

    @app.post("/conversations/{slug}/compact", response_model=Ack, tags=["conversations"])
    async def compact_conversation(slug: str) -> Ack:
        get(slug)
        if not st.session_id or st.conversation != slug:
            raise HTTPException(409, "compacting needs a session with this agent")
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
