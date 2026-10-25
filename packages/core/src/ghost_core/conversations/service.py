"""The agents facade (ADR 0014, ADR 0024): the registry read from the department folders,
name resolution, what a session loads on resume, turn and summary records, retention, and
the operations the gate registers under the ``conversations`` source name.

Switching is not here: it is a session action in ``ghost_voice``. Everything that
changes a row or a file is an operation below, so it runs at the operator's level
and is audited."""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import timedelta
from pathlib import Path
from typing import Any

from loguru import logger

from ghost_core.conversations.files import ConversationFiles, read_agent_json
from ghost_core.conversations.models import (
    GHOST,
    MAX_JOB,
    MAX_NOTE,
    MAX_PERSONA,
    AppendNote,
    Compaction,
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
    TurnRecord,
    slugify,
)
from ghost_core.conversations.writer import ConversationWriter
from ghost_core.departments import Department, Departments
from ghost_core.models import ChangeSet, utcnow
from ghost_core.source import Operation, Prepared, ReadTool, ToolSource
from ghost_core.store import Store

SOURCE = "conversations"  # the gate, policy and audit key for the agents' operations
DEFAULT_VOICE = "af_heart"
SUMMARY_TEMPLATE = "What this conversation has covered so far: {summary}"
NEXT_SESSION = "(next session)"
RECORD_END = "(end of the record)"
# Earlier turns arrive as a record, not as replayed chat: replayed, the model sees its own
# past claims with none of the tool calls behind them and "corrects" lookups that were real.
# Sessions are marked and the record's end is spelled out: unmarked, a request that opened
# a session ("Ghost, let's work on …") read as the tail of the last one's goodbye, and
# Ghost said "talk soon" and hung up (2026-10-24).
RECENT_TEMPLATE = (
    "The record of what was said earlier in this conversation, oldest first, with "
    f"{NEXT_SESSION} where one session ended and a later one began. The lookups and changes "
    "in it really happened; their results are not repeated here, so look again if you need "
    "current details. A goodbye in it ended that earlier session, not this one. What "
    f"follows {RECORD_END} is live.\n\n{{record}}\n{RECORD_END}"
)

_STOP = {
    "the",
    "a",
    "an",
    "to",
    "me",
    "with",
    "team",
    "department",
    "dept",
    "office",
    "desk",
    "agent",
    "people",
    "folks",
    "group",
    "conversation",
    "thread",
    "channel",
}


@dataclass(frozen=True)
class ResumeContext:
    """What a session loads when it enters an agent."""

    profile: Profile
    persona: str
    memory: str
    notes: list[str]
    recent: list[TurnRecord]

    def messages(self) -> list[dict[str, Any]]:
        """The LLM messages that stand in for the past: the rolling summary in the
        same shape the in-session summarizer uses, the pinned notes, then the record
        of the turns not yet covered by a summary."""
        out: list[dict[str, Any]] = []
        if self.memory.strip():
            out.append(
                {"role": "user", "content": SUMMARY_TEMPLATE.format(summary=self.memory.strip())}
            )
        if self.notes:
            out.append(
                {
                    "role": "user",
                    "content": "Things the operator asked you to remember, verbatim:\n- "
                    + "\n- ".join(self.notes),
                }
            )
        lines = []
        session = None
        for t in self.recent:
            if lines and t.session_id != session:
                lines.append(NEXT_SESSION)
            session = t.session_id
            if t.role == "user":
                lines.append(f"Operator: {t.text}")
            elif t.role == "assistant":
                lines.append(f"You: {t.text}")
            elif t.role == "call":
                lines.append(f"(you called {t.text})")
        if lines:
            out.append({"role": "user", "content": RECENT_TEMPLATE.format(record="\n".join(lines))})
        return _merge_adjacent(out)


def _merge_adjacent(msgs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: list[dict[str, Any]] = []
    for m in msgs:
        if merged and merged[-1]["role"] == m["role"]:
            merged[-1] = {
                "role": m["role"],
                "content": merged[-1]["content"] + "\n\n" + m["content"],
            }
        else:
            merged.append(dict(m))
    return merged


class Conversations:
    def __init__(
        self,
        store: Store,
        departments: Departments,
        *,
        default_voice: str = DEFAULT_VOICE,
        default_speed: float = 1.0,
        voices: Sequence[str] = (),
        recent_limit: int = 40,
        retention: timedelta = timedelta(days=90),
    ) -> None:
        """The folders are the registry; the store mirrors them and holds what only Ghost
        writes. ``sync`` reads the folders; every write below keeps both in step."""
        self._store = store
        self.departments = departments
        self.files = ConversationFiles(self.dir_for)
        self.writer = ConversationWriter(store, self.files, agents_dir=self._agents_dir)
        self.default_voice = default_voice
        self.default_speed = default_speed
        self._voices = tuple(voices)
        self._recent_limit = recent_limit
        self._retention = retention
        self.sync()

    # --- registry ------------------------------------------------------------

    def _agents_dir(self, department: str) -> Path | None:
        d = self.departments.get(department)
        return d.agents_dir if d is not None else None

    def dir_for(self, slug: str) -> Path:
        """An agent's folder: under its department's ``agents/``. A slug with no row yet
        (a hire in progress) is placed by the department the writer just recorded."""
        p = self._store.get_conversation(slug, include_deleted=True)
        department = p.department if p is not None else GHOST
        base = self._agents_dir(department) or self.departments.ghost.agents_dir
        return base / slug

    def sync(self) -> list[str]:
        """Read every department's ``agents/`` folder into the store: a folder is an agent
        (its ``agent.json`` is its identity), each department gets its default agent, and
        a row whose folder is gone is marked deleted. Returns the problems found."""
        problems: list[str] = []
        seen: set[str] = set()
        # Deleted rows are kept (slug is unique): a folder made again under an old slug
        # takes its row and id back instead of colliding with it.
        rows = {
            p.slug: p
            for p in self._store.list_conversations(include_retired=True, include_deleted=True)
        }
        ids = {p.id for p in rows.values()}
        for d in self.departments.list():
            ids.add(self._ensure_default(d, rows).id)
            seen.add(d.slug)
            if not d.agents_dir.is_dir():
                continue
            for folder in sorted(p for p in d.agents_dir.iterdir() if p.is_dir()):
                slug = folder.name.lower()
                if folder.name != slug:
                    problems.append(f"{folder}: an agent folder's name is not lower-case; not read")
                    continue
                if slug in seen:
                    if slug != d.slug:
                        problems.append(f"{folder}: another agent is already called {slug}")
                    continue
                data = read_agent_json(folder)
                existing = rows.get(slug)
                if data is None:
                    if existing is None:
                        problems.append(f"{folder}: no agent.json; not an agent")
                        continue
                    data = existing.agent_json()
                # The row's id wins over agent.json (an edited or restored file); an id
                # another agent already has (a copied folder) is a new agent.
                file_id = str(data.get("id") or "")
                if existing is not None:
                    agent_id = existing.id
                elif file_id and file_id not in ids:
                    agent_id = file_id
                else:
                    agent_id = Profile(slug=slug, name=slug).id
                ids.add(agent_id)
                p = Profile(
                    id=agent_id,
                    slug=slug,
                    name=str(
                        data.get("name")
                        or (existing.name if existing else slug.replace("-", " ").title())
                    ),
                    aliases=tuple(str(a) for a in data.get("aliases", []) if str(a).strip()),
                    department=d.slug,
                    job=str(data.get("job") or "")[:MAX_JOB],
                    voice=str(data["voice"]) if data.get("voice") else None,
                    speed=float(data["speed"]) if data.get("speed") is not None else None,
                    created_at=existing.created_at if existing else utcnow(),
                    updated_at=existing.updated_at if existing else utcnow(),
                    last_active_at=existing.last_active_at if existing else None,
                    retired_at=existing.retired_at if existing else None,
                    deleted_at=None,
                )
                if existing is None or existing.model_dump() != p.model_dump():
                    self._store.upsert_conversation(p)
                if data != p.agent_json():  # a hand-made folder gets its id written back
                    self.files.write_agent(slug, p.agent_json())
                seen.add(slug)
        for slug, p in rows.items():
            if slug not in seen and p.deleted_at is None:
                logger.warning(f"agents: {slug}'s folder is gone; marked deleted")
                self._store.upsert_conversation(p.model_copy(update={"deleted_at": utcnow()}))
        for problem in problems:
            logger.warning(f"agents: {problem}")
        return problems

    def _ensure_default(self, d: Department, rows: dict[str, Profile]) -> Profile:
        """Each department's own agent, named for it, where a transfer lands."""
        existing = rows.get(d.slug)
        folder = d.agents_dir / d.slug
        data = read_agent_json(folder) or {}
        if existing is not None and existing.deleted_at is None and folder.is_dir():
            return existing
        if existing is not None and existing.deleted_at is not None and not folder.is_dir():
            # A department made again under an old slug: its agent comes back with its id.
            data = {}
        name = "Ghost" if d.is_ghost else d.title
        p = Profile(
            id=str(data.get("id") or (existing.id if existing else ""))
            or Profile(slug=d.slug, name=name).id,
            slug=d.slug,
            name=str(data.get("name") or name),
            aliases=tuple(str(a) for a in data.get("aliases", []))
            or (("ghost", "primary", "main line") if d.is_ghost else d.aliases),
            department=d.slug,
            created_at=existing.created_at if existing else utcnow(),
        )
        self._store.upsert_conversation(p)
        if not (folder / "agent.json").is_file():
            self.files.write_agent(d.slug, p.agent_json())
        rows[d.slug] = p
        return p

    def ensure_ghost(self) -> Profile:
        p = self._store.get_conversation(GHOST)
        if p is None:
            self.sync()
            p = self._store.get_conversation(GHOST)
        assert p is not None
        return p

    def get(self, slug: str) -> Profile | None:
        return self._store.get_conversation(slug)

    def by_id(self, id: str) -> Profile | None:
        return self._store.get_conversation_by_id(id)

    def list(
        self, *, include_retired: bool = False, department: str | None = None
    ) -> list[Profile]:
        """Live agents, Ghost first. Retired ones are left out unless asked for: they
        are readable, not routable. ``department`` narrows to one roster."""
        out = self._store.list_conversations(include_retired=include_retired)
        if department is not None:
            out = [p for p in out if p.department == department]
        return out

    def agents_of(self, department: str, *, include_retired: bool = False) -> list[Profile]:
        return self.list(include_retired=include_retired, department=department)

    def default_agent(self, department: str) -> Profile | None:
        return self.get(department)

    def department_of(self, p: Profile) -> Department:
        """The department an agent works in; Ghost's when its folder is gone."""
        return self.departments.get(p.department) or self.departments.ghost

    def resolve(self, spoken: str, *, include_retired: bool = False) -> Profile | None:
        """The agent the operator most likely means by ``spoken``: exact slug, then any
        name or alias, then word overlap with filler words removed. A retired agent is
        not a match unless ``include_retired`` (to bring it back)."""
        text = spoken.strip().lower()
        if not text:
            return None
        profiles = self.list(include_retired=include_retired)
        for p in profiles:
            if p.slug == slugify(text):
                return p
        for p in profiles:
            if text in {n.lower() for n in p.spoken_names()}:
                return p
        words = _words(text)
        if not words:
            return None
        best: tuple[int, Profile] | None = None
        for p in profiles:
            cand = set()
            for n in p.spoken_names():
                cand |= _words(n.lower())
            score = len(words & cand)
            if score and (best is None or score > best[0]):
                best = (score, p)
        return best[1] if best else None

    def touch(self, slug: str) -> None:
        p = self.get(slug)
        if p is not None:
            self._store.touch_conversation(p.id)

    def voices(self) -> tuple[str, ...]:
        return self._voices

    def voice_for(self, p: Profile) -> tuple[str, float]:
        """The voice heard: the agent's own, else its department's, else the default."""
        d = self.department_of(p)
        voice = p.voice or d.voice or self.default_voice
        speed = (
            p.speed
            if p.speed is not None
            else (d.speed if d.speed is not None else self.default_speed)
        )
        return voice, speed

    def unused_voice(self) -> str:
        """A voice no live agent or department uses yet, so a new agent sounds like
        itself. The default when every installed voice is taken (or none is known)."""
        taken = {p.voice for p in self.list(include_retired=True) if p.voice}
        taken |= {d.voice for d in self.departments.list() if d.voice}
        taken.add(self.default_voice)
        for v in self._voices:
            if v not in taken:
                return v
        return self.default_voice

    def owners_of(self, server: str, item: str) -> list[Profile]:
        """Where a request about ``item`` on ``server`` can be routed (ADR 0019): the
        default agent of each department whose declaration covers it."""
        out: list[Profile] = []
        for d in self.departments.covering(server, item):
            p = self.default_agent(d.slug)
            if p is not None and not p.retired:
                out.append(p)
        return out

    # --- what a session loads -------------------------------------------------

    def resume(self, slug: str) -> ResumeContext:
        p = self.get(slug)
        if p is None:
            raise ValueError(f"no agent {slug!r}")
        return ResumeContext(
            profile=p,
            persona=self.files.persona(slug),
            memory=self.files.memory(slug),
            notes=self.files.note_lines(slug),
            recent=self._store.recent_turns(p.id, limit=self._recent_limit, uncompacted=True),
        )

    # --- transcript ------------------------------------------------------------

    def _id(self, slug: str) -> str:
        p = self._store.get_conversation(slug, include_deleted=True)
        if p is None:
            raise ValueError(f"no agent {slug!r}")
        return p.id

    def record(self, slug: str, session_id: str, role: str, text: str) -> int | None:
        text = text.strip()
        if not text:
            return None
        return self._store.add_turn(self._id(slug), session_id, role, text)

    def carry(self, session_id: str, src: str, dst: str) -> int:
        """Move the turns of one session from ``src`` to ``dst`` (save this as ...)."""
        return self._store.move_session_turns(session_id, self._id(src), self._id(dst))

    def turns(self, slug: str, *, limit: int = 200) -> list[TurnRecord]:
        return self._store.recent_turns(self._id(slug), limit=limit, uncompacted=False)

    def compactions(self, slug: str) -> list[Compaction]:
        return self._store.compactions(self._id(slug))

    def last_turn_id(self, slug: str) -> int | None:
        return self._store.last_turn_id(self._id(slug))

    def prune(self) -> int:
        """Drop summarized turns older than the retention window."""
        return self._store.prune_turns(utcnow() - self._retention)

    # --- operations for the gate --------------------------------------------------

    def read_tools(self) -> list[ReadTool]:
        async def list_agents(_args: dict[str, Any]) -> dict[str, Any]:
            return {
                "departments": [
                    {
                        "name": d.title,
                        "slug": d.slug,
                        "about": d.rules_lead(),
                        "agents": [
                            {
                                "name": p.name,
                                "slug": p.slug,
                                "aliases": list(p.aliases),
                                "job": p.job or None,
                                "last_active": p.last_active_at.isoformat()
                                if p.last_active_at
                                else None,
                            }
                            for p in self.agents_of(d.slug)
                        ],
                    }
                    for d in self.departments.list()
                ]
            }

        return [
            ReadTool(
                name="list_agents",
                description=(
                    "The departments and the agents in each, with names, aliases and jobs: "
                    "who the operator can be transferred to. Retired agents are not listed."
                ),
                properties={},
                required=[],
                handler=list_agents,
            ),
        ]

    def operations(self) -> list[Operation]:
        w = self.writer
        voices = self._voices

        def voice_prop() -> dict[str, Any]:
            prop: dict[str, Any] = {"type": "string", "description": "A Kokoro voice id."}
            if voices:
                prop["enum"] = list(voices)
            return prop

        async def create(args: dict[str, Any]) -> Prepared:
            name = _name(args.get("name"))
            slug = slugify(name)
            if self._store.get_conversation(slug, include_deleted=True) is not None:
                raise ValueError(f"there is already an agent called {name!r}")
            if self.departments.get(slug) is not None:
                raise ValueError(f"{name!r} is a department's name; pick another")
            department = self._department_arg(args.get("department"), args.get("_active"))
            voice = _voice(args.get("voice"), voices)
            persona = str(args.get("persona") or "").strip()
            if len(persona) > MAX_PERSONA:
                raise ValueError("that persona is too long")
            job = " ".join(str(args.get("job") or "").split())
            if len(job) > MAX_JOB:
                raise ValueError("keep the job to a line")
            memory = " ".join(str(args.get("memory") or "").split())
            seed = str(args.get("seed_from") or "").strip()
            if seed:
                memory = self.handoff_memory(seed)
            return Prepared(
                target=ConversationTarget(slug=slug),
                changes=ChangeSet(
                    steps=(
                        Create(
                            name=name,
                            aliases=_aliases(args.get("aliases")),
                            department=department.slug,
                            job=job,
                            voice=voice,
                            speed=_speed(args.get("speed"), None),
                            persona=persona,
                            memory=memory,
                        ),
                    )
                ),
                rationale=f"hire {name} in {department.title}",
            )

        async def retire(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            if p.is_default:
                raise ValueError(f"{p.name} is its department's own agent and stays")
            if p.retired:
                raise ValueError(f"{p.name} is already retired")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Retire(),))
            )

        async def rehire(args: dict[str, Any]) -> Prepared:
            p = self._target(args, include_retired=True)
            if not p.retired:
                raise ValueError(f"{p.name} is not retired")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Rehire(),))
            )

        async def rename(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            name = _name(args.get("name", p.name))
            aliases = _aliases(args.get("aliases")) if "aliases" in args else p.aliases
            job = (
                " ".join(str(args["job"]).split())
                if "job" in args and args["job"] is not None
                else None
            )
            if job is not None and len(job) > MAX_JOB:
                raise ValueError("keep the job to a line")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(Rename(name=name, aliases=aliases, job=job),)),
            )

        async def set_voice(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            voice = _voice(args.get("voice"), voices) if "voice" in args else p.voice
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(
                    steps=(SetVoice(voice=voice, speed=_speed(args.get("speed"), p.speed)),)
                ),
            )

        async def set_persona(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            text = str(args.get("text") or "").strip()
            if len(text) > MAX_PERSONA:
                raise ValueError("that persona is too long")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(SetPersona(text=text),)),
            )

        async def remember(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            text = " ".join(str(args.get("text") or "").split())
            if not text:
                raise ValueError("nothing to remember")
            if len(text) > MAX_NOTE:
                raise ValueError("that note is too long; keep it to a sentence or two")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(AppendNote(text=text),)),
            )

        async def forget(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            needle = " ".join(str(args.get("text") or "").split()).lower()
            lines = self.files.note_lines(p.slug)
            keep = [ln for ln in lines if needle and needle not in ln.lower()]
            if len(keep) == len(lines):
                raise ValueError("no note matches that")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(SetNotes(text="\n".join(keep)),)),
                rationale=f"forget {len(lines) - len(keep)} note(s)",
            )

        async def compact(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            text = str(args.get("summary") or "").strip()
            if not text:
                raise ValueError("a summary is required")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(
                    steps=(
                        SetMemory(
                            text=text, through_turn_id=self.last_turn_id(p.slug), source="manual"
                        ),
                    )
                ),
            )

        async def reset(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Reset(),))
            )

        async def delete(args: dict[str, Any]) -> Prepared:
            p = self._target(args, include_retired=True)
            if p.is_default:
                raise ValueError(f"{p.name} is its department's own agent; it cannot be deleted")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Delete(),))
            )

        async def move(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            if p.is_default:
                raise ValueError(
                    f"{p.name} is long-lived and about many things, so it does not move. To "
                    "move this conversation, hire an agent in the destination (change, "
                    "conversations.create with department, a job, a persona and a memory "
                    "that summarizes this session); the session's turns go with it and the "
                    "operator is transferred there. Say the agent's name."
                )
            text = str(args.get("department") or "").strip()
            if not text:
                raise ValueError("which department?")
            d = self.departments.get(text) or self.departments.resolve(text)
            if d is None:
                raise ValueError(
                    f"there is no department called {text!r}; the departments are "
                    + ", ".join(x.title for x in self.departments.list())
                )
            if d.slug == p.department:
                raise ValueError(f"{p.name} is already in {d.title}")
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(Move(department=d.slug),)),
                rationale=f"move {p.name} to {d.title}",
            )

        conv = {
            "type": "string",
            "description": "Which agent, by name. Omit for the one you are in.",
        }
        return [
            Operation(
                "create",
                (
                    "Hire a new agent: a conversation with its own persona and job, in a "
                    "department. Use it at once, without asking, when the operator says 'save "
                    "this as X', 'start an agent for X' or 'hire someone for X'; it switches "
                    "them to it. Always write a persona and a one-line job. Leave voice out to "
                    "inherit the department's."
                ),
                {
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                    "department": {
                        "type": "string",
                        "description": "the department it works in; omit for the current one",
                    },
                    "job": {
                        "type": "string",
                        "description": "what it is for, in one line (brand design, the Q3 audit)",
                    },
                    "voice": voice_prop(),
                    "speed": {"type": "number"},
                    "persona": {
                        "type": "string",
                        "description": (
                            "Who this agent is, in two or three sentences, written from what "
                            "the conversation has been about: its subject, how the operator "
                            "talks about it, and a manner that fits (brisk for ops, careful for "
                            "money, warm for people). Distinct from the other agents; never a "
                            "generic assistant. The operator edits it later."
                        ),
                    },
                    "seed_from": {
                        "type": "string",
                        "description": (
                            "Slug of a retired agent whose memory becomes this one's first "
                            "memory, as a handoff."
                        ),
                    },
                    "memory": {
                        "type": "string",
                        "description": (
                            "The new agent's first memory: a summary you write of this "
                            "session so far, when the operator moves a conversation out of "
                            "Ghost or a department's own agent (a split move, ADR 0024). "
                            "Facts, decisions, open items."
                        ),
                    },
                },
                ["name"],
                prepare=create,
                writer=w,
                risk="low",
                allowed=w.allowed,
                asked_by=("hire", "start", "save"),
            ),
            Operation(
                "retire",
                (
                    "Retire an agent: it leaves the roster and voice routing but keeps its "
                    "transcript and memory, and can be rehired. Nothing is deleted."
                ),
                {"conversation": conv},
                [],
                prepare=retire,
                writer=w,
                risk="low",
                allowed=w.allowed,
                asked_by=("retire", "fire", "let go"),
            ),
            Operation(
                "rehire",
                "Bring a retired agent back onto the roster.",
                {"conversation": conv},
                [],
                prepare=rehire,
                writer=w,
                risk="low",
                allowed=w.allowed,
                asked_by=("rehire", "bring back"),
            ),
            Operation(
                "rename",
                "Rename an agent, change what it answers to, or restate its job.",
                {
                    "conversation": conv,
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                    "job": {"type": "string"},
                },
                [],
                prepare=rename,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "set_voice",
                "Change an agent's voice or speaking speed; no voice means the department's.",
                {
                    "conversation": conv,
                    "voice": voice_prop(),
                    "speed": {"type": "number", "description": "0.5 to 2.0"},
                },
                [],
                prepare=set_voice,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "set_persona",
                ("Set who this agent is, in the operator's own words, verbatim. Never paraphrase."),
                {"conversation": conv, "text": {"type": "string"}},
                ["text"],
                prepare=set_persona,
                writer=w,
                risk="medium",
                allowed=w.allowed,
            ),
            Operation(
                "remember",
                (
                    "Pin something the operator asked you to remember, in their words. "
                    "Never summarized away."
                ),
                {"conversation": conv, "text": {"type": "string"}},
                ["text"],
                prepare=remember,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "forget",
                "Drop a pinned note that contains the given words.",
                {"conversation": conv, "text": {"type": "string"}},
                ["text"],
                prepare=forget,
                writer=w,
                risk="medium",
                allowed=w.allowed,
            ),
            Operation(
                "compact",
                (
                    "Replace this agent's rolling memory with a fresh summary you write: "
                    "facts, decisions, open items. Only when the operator asks."
                ),
                {"conversation": conv, "summary": {"type": "string"}},
                ["summary"],
                prepare=compact,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "reset",
                (
                    "Forget everything an agent knows: its memory, notes and transcript. "
                    "Keeps the name, job and voice."
                ),
                {"conversation": conv},
                [],
                prepare=reset,
                writer=w,
                risk="high",
                allowed=w.allowed,
            ),
            Operation(
                "delete",
                "Remove an agent entirely, its folder included. Retire is the usual way.",
                {"conversation": conv},
                [],
                prepare=delete,
                writer=w,
                risk="high",
                allowed=w.allowed,
            ),
            Operation(
                "move",
                (
                    "Move an agent to another department (ADR 0024): it keeps its name, "
                    "persona, memory and transcript and works with that department's tools, "
                    "rules and memory from then on. The one you are with, by default. A "
                    "department's own agent cannot move."
                ),
                {
                    "conversation": conv,
                    "department": {"type": "string", "description": "the destination"},
                },
                ["department"],
                prepare=move,
                writer=w,
                risk="medium",
                allowed=w.allowed,
                asked_by=("move",),
            ),
        ]

    def handoff_memory(self, seed: str) -> str:
        """The memory a new agent starts with when seeded from ``seed`` (a slug or a
        name): that agent's current ``memory.md``. Ghost's memory stays private to
        Ghost (ADR 0014), so it can never be the seed."""
        p = self.get(seed) or self.resolve(seed, include_retired=True)
        if p is None:
            raise ValueError(f"no agent {seed!r} to seed from")
        if p.is_ghost:
            raise ValueError(
                "Ghost's memory is private; a handoff from Ghost is a summary you write"
            )
        return self.files.memory(p.slug)

    def _department_arg(self, spoken: object, active: object) -> Department:
        """The department an operation names, else the active agent's."""
        text = str(spoken or "").strip()
        if text:
            d = self.departments.get(text) or self.departments.resolve(text)
            if d is None:
                raise ValueError(
                    f"there is no department called {text!r}; the departments are "
                    + ", ".join(x.title for x in self.departments.list())
                )
            return d
        p = self.get(str(active or GHOST))
        return self.department_of(p) if p is not None else self.departments.ghost

    def _target(self, args: dict[str, Any], *, include_retired: bool = False) -> Profile:
        """The agent an operation is about: ``conversation`` by name, else the active
        one the caller put in ``_active``."""
        spoken = args.get("conversation")
        if spoken:
            p = self.resolve(str(spoken), include_retired=include_retired)
            if p is None:
                raise ValueError(f"no agent called {spoken!r}")
            return p
        slug = str(args.get("_active") or GHOST)
        p = self.get(slug)
        if p is None:
            raise ValueError(f"no agent {slug!r}")
        return p


def _words(text: str) -> set[str]:
    return {w for w in re.findall(r"[a-z0-9]+", text) if w not in _STOP}


def _name(value: object) -> str:
    name = " ".join(str(value or "").split())
    if not name:
        raise ValueError("a name is required")
    if len(name) > 80:
        raise ValueError("that name is too long")
    return name


def _aliases(value: object) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        value = [v for v in re.split(r"[,;]", value)]
    if not isinstance(value, list):
        raise ValueError("aliases must be a list")
    out = []
    for v in value:
        s = " ".join(str(v).split())
        if s and s.lower() not in {o.lower() for o in out}:
            out.append(s)
    return tuple(out[:12])


def _voice(value: object, voices: Sequence[str]) -> str | None:
    if value is None or value == "":
        return None
    v = str(value).strip()
    if voices and v not in voices:
        raise ValueError(f"{v!r} is not an installed voice")
    if not re.fullmatch(r"[a-z]{2}_[a-z0-9]+", v):
        raise ValueError(f"{v!r} is not a voice id")
    return v


def _speed(value: object, default: float | None) -> float | None:
    if value is None or value == "":
        return default
    try:
        s = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        raise ValueError("speed must be a number") from None
    if not 0.5 <= s <= 2.0:
        raise ValueError("speed must be between 0.5 and 2")
    return round(s, 2)


def conversations_source(convs: Conversations) -> ToolSource:
    """The agents' own read tools and operations as a tool source, so the gate, the policy
    file and the audit treat them like any server's (ADR 0014)."""
    return ToolSource(
        name=SOURCE,
        prompt="",
        read_tools=convs.read_tools(),
        operations=convs.operations(),
    )


def names_arg(value: Any) -> list[str]:
    """A list argument the model may pass as a list or a comma-separated string."""
    if value is None:
        return []
    if isinstance(value, str):
        return [s.strip() for s in re.split(r"[,;]", value) if s.strip()]
    if isinstance(value, list):
        return [str(v).strip() for v in value if str(v).strip()]
    raise ValueError("expected a list of names")
