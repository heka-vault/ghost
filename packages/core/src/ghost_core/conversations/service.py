"""The conversations facade (ADR 0014): the registry, name resolution, what a session
loads on resume, turn and summary records, retention, and the operations the gate
registers under the ``conversations`` ability name.

Switching is not here: it is a session action in ``ghost_voice``. Everything that
changes a row or a file is an operation below, so it runs at the operator's level
and is audited."""

from __future__ import annotations

import re
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import timedelta
from pathlib import Path
from typing import Any

from ghost_core.ability import Operation, Prepared, ReadTool
from ghost_core.conversations.files import ConversationFiles
from ghost_core.conversations.models import (
    GHOST,
    MAX_NOTE,
    MAX_PERSONA,
    AppendNote,
    Compaction,
    ConversationTarget,
    Create,
    Delete,
    Profile,
    Rename,
    Reset,
    Scope,
    SetMemory,
    SetNotes,
    SetPersona,
    SetScope,
    Settle,
    SetVoice,
    TurnRecord,
    Unsettle,
    describe_scope,
    slugify,
)
from ghost_core.conversations.writer import ConversationWriter
from ghost_core.models import ChangeSet, utcnow
from ghost_core.store import Store

ABILITY = "conversations"
DEFAULT_VOICE = "af_heart"
SUMMARY_TEMPLATE = "What this conversation has covered so far: {summary}"
# Earlier turns arrive as a record, not as replayed chat: replayed, the model sees its own
# past claims with none of the tool calls behind them and "corrects" lookups that were real.
RECENT_TEMPLATE = (
    "The record of what was said earlier in this conversation, oldest first. The lookups "
    "and changes in it really happened; their results are not repeated here, so look "
    "again if you need current details. What follows the record is live.\n\n{record}"
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
    "line",
    "people",
    "folks",
    "group",
    "conversation",
    "channel",
}


@dataclass(frozen=True)
class ResumeContext:
    """What a session loads when it enters a conversation."""

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
        for t in self.recent:
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
        root: Path,
        *,
        default_voice: str = DEFAULT_VOICE,
        default_speed: float = 1.0,
        voices: Sequence[str] = (),
        recent_limit: int = 40,
        retention: timedelta = timedelta(days=90),
        scopes: Mapping[str, Callable[[], list[str]]] | None = None,
    ) -> None:
        """``scopes`` is what a thread can be scoped to: ability name -> a function
        returning that ability's whole allowlist (``Contributions.scope_items``). A
        thread's scope must stay within it (ADR 0019)."""
        self._store = store
        self.files = ConversationFiles(root)
        self.writer = ConversationWriter(store, self.files)
        self.default_voice = default_voice
        self.default_speed = default_speed
        self._voices = tuple(voices)
        self._recent_limit = recent_limit
        self._retention = retention
        self._scopes: dict[str, Callable[[], list[str]]] = dict(scopes or {})
        self.ensure_ghost()

    # --- registry ------------------------------------------------------------

    def ensure_ghost(self) -> Profile:
        p = self._store.get_conversation(GHOST, include_deleted=True)
        if p is None:
            p = Profile(
                slug=GHOST,
                name="Ghost",
                aliases=("ghost", "primary", "main line"),
                voice=self.default_voice,
                speed=self.default_speed,
            )
            self._store.upsert_conversation(p)
        elif p.deleted_at is not None:
            p = p.model_copy(update={"deleted_at": None})
            self._store.upsert_conversation(p)
        self.files.ensure(GHOST)
        return p

    def get(self, slug: str) -> Profile | None:
        return self._store.get_conversation(slug)

    def list(self, *, include_settled: bool = False) -> list[Profile]:
        """Live conversations, Ghost first. Settled threads (ADR 0019) are left out
        unless asked for: they are readable, not routable."""
        return self._store.list_conversations(include_settled=include_settled)

    def resolve(self, spoken: str, *, include_settled: bool = False) -> Profile | None:
        """The conversation the operator most likely means by ``spoken``: exact slug,
        then any name or alias, then word overlap with filler words removed. A settled
        thread is not a match unless ``include_settled`` (to bring it back)."""
        text = spoken.strip().lower()
        if not text:
            return None
        profiles = self.list(include_settled=include_settled)
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
        self._store.touch_conversation(slug)

    def voices(self) -> tuple[str, ...]:
        return self._voices

    def unused_voice(self) -> str:
        """A voice no live conversation uses yet, so a new thread sounds like itself.
        The default when every installed voice is taken (or none is known)."""
        taken = {p.voice for p in self.list(include_settled=True)}
        for v in self._voices:
            if v not in taken:
                return v
        return self.default_voice

    # --- scope (ADR 0019) -----------------------------------------------------------

    def scope_kinds(self) -> dict[str, Callable[[], list[str]]]:
        return dict(self._scopes)

    def validate_scope(self, slug: str, scope: Mapping[str, Sequence[str]]) -> Scope:
        """A thread's scope as it will be stored: each ability must be one that can be
        scoped, each item within that ability's allowlist (spelled as the allowlist
        spells it). Ghost is always unscoped. Empty item lists drop the ability."""
        if slug == GHOST and any(scope.values()):
            raise ValueError("Ghost is unscoped and cannot be given a scope")
        out: Scope = {}
        for ability, items in scope.items():
            if ability not in self._scopes:
                raise ValueError(f"{ability} cannot be scoped")
            allowed = {a.lower(): a for a in self._scopes[ability]()}
            kept: list[str] = []
            for raw in items:
                item = str(raw).strip()
                if not item:
                    continue
                canonical = allowed.get(item.lower())
                if canonical is None:
                    raise ValueError(
                        f"{item} is not something {ability} is allowed to touch; "
                        f"it allows {', '.join(sorted(allowed.values())) or 'nothing'}"
                    )
                if canonical not in kept:
                    kept.append(canonical)
            if kept:
                out[ability] = tuple(kept)
        return out

    def scope_of(self, ability: str, item: str) -> list[Profile]:
        """The live threads whose scope for ``ability`` covers ``item`` (a target's
        ``scope()`` string). Used to tell the operator where something belongs."""
        needle = item.strip().lower()
        return [
            p
            for p in self.list()
            if not p.is_ghost and needle in {i.lower() for i in p.scope.get(ability, ())}
        ]

    # --- what a session loads -------------------------------------------------

    def resume(self, slug: str) -> ResumeContext:
        p = self.get(slug)
        if p is None:
            raise ValueError(f"no conversation {slug!r}")
        return ResumeContext(
            profile=p,
            persona=self.files.persona(slug),
            memory=self.files.memory(slug),
            notes=self.files.note_lines(slug),
            recent=self._store.recent_turns(slug, limit=self._recent_limit, uncompacted=True),
        )

    # --- transcript ------------------------------------------------------------

    def record(self, slug: str, session_id: str, role: str, text: str) -> int | None:
        text = text.strip()
        if not text:
            return None
        return self._store.add_turn(slug, session_id, role, text)

    def carry(self, session_id: str, src: str, dst: str) -> int:
        """Move the turns of one session from ``src`` to ``dst`` (save this as ...)."""
        return self._store.move_session_turns(session_id, src, dst)

    def turns(self, slug: str, *, limit: int = 200) -> list[TurnRecord]:
        return self._store.recent_turns(slug, limit=limit, uncompacted=False)

    def compactions(self, slug: str) -> list[Compaction]:
        return self._store.compactions(slug)

    def last_turn_id(self, slug: str) -> int | None:
        return self._store.last_turn_id(slug)

    def prune(self) -> int:
        """Drop summarized turns older than the retention window."""
        return self._store.prune_turns(utcnow() - self._retention)

    # --- operations for the gate --------------------------------------------------

    def read_tools(self) -> list[ReadTool]:
        async def list_conversations(_args: dict[str, Any]) -> dict[str, Any]:
            return {
                "conversations": [
                    {
                        "name": p.name,
                        "slug": p.slug,
                        "aliases": list(p.aliases),
                        "voice": p.voice,
                        "scope": {a: list(items) for a, items in p.scope.items()} or "everything",
                        "last_active": p.last_active_at.isoformat() if p.last_active_at else None,
                    }
                    for p in self.list()
                ]
            }

        return [
            ReadTool(
                name="list_conversations",
                description=(
                    "The saved conversations (threads, lines) the operator can be patched "
                    "through to, with their names, aliases and what each one covers. Settled "
                    "threads are not listed."
                ),
                properties={},
                required=[],
                handler=list_conversations,
            )
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
                raise ValueError(f"there is already a conversation called {name!r}")
            voice = _voice(args.get("voice"), voices) or self.unused_voice()
            persona = str(args.get("persona") or "").strip()
            if len(persona) > MAX_PERSONA:
                raise ValueError("that persona is too long")
            scope = self.validate_scope(slug, _scope_arg(args.get("scope")))
            memory = ""
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
                            voice=voice,
                            speed=_speed(args.get("speed"), self.default_speed),
                            persona=persona,
                            scope=scope,
                            memory=memory,
                        ),
                    )
                ),
                rationale=f"start a conversation called {name}",
            )

        async def set_scope(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            if p.is_ghost:
                raise ValueError("Ghost is unscoped and cannot be given a scope")
            ability = str(args.get("ability") or "").strip()
            if not ability:
                raise ValueError("which ability's scope? (for example github_issues)")
            items = args.get("items")
            if isinstance(items, str):
                items = [s for s in re.split(r"[,;]", items)]
            if items is None:
                items = []
            if not isinstance(items, list):
                raise ValueError("items must be a list")
            merged = {a: list(v) for a, v in p.scope.items()}
            merged[ability] = [str(i) for i in items]
            scope = self.validate_scope(p.slug, merged)
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(SetScope(scope=scope),)),
                rationale=f"{p.name} now covers {describe_scope(scope)}",
            )

        async def settle(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            if p.is_ghost:
                raise ValueError("Ghost cannot be settled")
            if p.settled:
                raise ValueError(f"{p.name} is already settled")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Settle(),))
            )

        async def unsettle(args: dict[str, Any]) -> Prepared:
            p = self._target(args, include_settled=True)
            if not p.settled:
                raise ValueError(f"{p.name} is not settled")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Unsettle(),))
            )

        async def rename(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            name = _name(args.get("name", p.name))
            aliases = _aliases(args.get("aliases")) if "aliases" in args else p.aliases
            return Prepared(
                target=ConversationTarget(slug=p.slug),
                changes=ChangeSet(steps=(Rename(name=name, aliases=aliases),)),
            )

        async def set_voice(args: dict[str, Any]) -> Prepared:
            p = self._target(args)
            voice = _voice(args.get("voice"), voices) or p.voice
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
            p = self._target(args)
            if p.is_ghost:
                raise ValueError("Ghost cannot be deleted; reset it instead")
            return Prepared(
                target=ConversationTarget(slug=p.slug), changes=ChangeSet(steps=(Delete(),))
            )

        conv = {
            "type": "string",
            "description": "Which conversation, by name. Omit for the one you are in.",
        }
        scope_prop = {
            "type": "object",
            "description": (
                "What the thread covers, per ability: {ability: [item, ...]}, for example "
                '{"github_issues": ["acme/app"]}. Bind what the conversation was about; '
                "omit for a thread that covers everything."
                + (
                    f" Scopable abilities: {', '.join(sorted(self._scopes))}."
                    if self._scopes
                    else ""
                )
            ),
            "additionalProperties": {"type": "array", "items": {"type": "string"}},
        }
        return [
            Operation(
                "create",
                (
                    "Start a new saved conversation (a thread, a line the operator can be "
                    "patched to later). Use it at once, without asking, when the operator says "
                    "'save this as X', 'make this a thread for X' or 'start a conversation "
                    "called X'; it switches them to it. When the recent conversation was about "
                    "particular repositories or items, put them in scope. Always write a "
                    "persona. Leave voice out and one that no other thread uses is picked."
                ),
                {
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                    "voice": voice_prop(),
                    "speed": {"type": "number"},
                    "persona": {
                        "type": "string",
                        "description": (
                            "Who this thread is, in two or three sentences, written for this "
                            "thread from what the conversation has been about: its subject, how "
                            "the operator talks about it, and a manner that fits (brisk for ops, "
                            "careful for money, warm for people). Make it distinct from the other "
                            "threads; never a generic assistant. The operator edits it later."
                        ),
                    },
                    "scope": scope_prop,
                    "seed_from": {
                        "type": "string",
                        "description": (
                            "Slug of a settled thread whose memory becomes this one's first "
                            "memory, as a handoff."
                        ),
                    },
                },
                ["name"],
                prepare=create,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "set_scope",
                (
                    "Set what a thread covers for one ability: the items it may read and "
                    "change. An empty list makes it cover everything for that ability. "
                    "Ghost itself always covers everything."
                ),
                {
                    "conversation": conv,
                    "ability": {"type": "string", "description": "github_issues, ..."},
                    "items": {"type": "array", "items": {"type": "string"}},
                },
                ["ability", "items"],
                prepare=set_scope,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "settle",
                (
                    "Mark a thread done: it leaves voice routing and the active list but "
                    "keeps its transcript and memory, and can be brought back. Nothing is "
                    "deleted."
                ),
                {"conversation": conv},
                [],
                prepare=settle,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "unsettle",
                "Bring a settled thread back into use.",
                {"conversation": conv},
                [],
                prepare=unsettle,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "rename",
                "Rename a conversation or change what it answers to.",
                {
                    "conversation": conv,
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                },
                [],
                prepare=rename,
                writer=w,
                risk="low",
                allowed=w.allowed,
            ),
            Operation(
                "set_voice",
                "Change the voice or speaking speed of a conversation.",
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
                (
                    "Set who this conversation is, in the operator's own words, verbatim. "
                    "Never paraphrase."
                ),
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
                    "Replace this conversation's rolling memory with a fresh summary you write: "
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
                    "Forget everything in a conversation: its memory, notes and transcript. "
                    "Keeps the name and voice."
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
                "Remove a saved conversation entirely. Ghost itself cannot be deleted.",
                {"conversation": conv},
                [],
                prepare=delete,
                writer=w,
                risk="high",
                allowed=w.allowed,
            ),
        ]

    def handoff_memory(self, seed: str) -> str:
        """The memory a new thread starts with when seeded from ``seed`` (a slug or a
        name): that thread's current ``memory.md``. Ghost's memory stays private to
        Ghost (ADR 0014), so it can never be the seed."""
        p = self.get(seed) or self.resolve(seed, include_settled=True)
        if p is None:
            raise ValueError(f"no thread {seed!r} to seed from")
        if p.is_ghost:
            raise ValueError(
                "Ghost's memory is private; a handoff from Ghost is a summary you write"
            )
        return self.files.memory(p.slug)

    def _target(self, args: dict[str, Any], *, include_settled: bool = False) -> Profile:
        """The conversation an operation is about: ``conversation`` by name, else the
        active one the caller put in ``_active``."""
        spoken = args.get("conversation")
        if spoken:
            p = self.resolve(str(spoken), include_settled=include_settled)
            if p is None:
                raise ValueError(f"no conversation called {spoken!r}")
            return p
        slug = str(args.get("_active") or GHOST)
        p = self.get(slug)
        if p is None:
            raise ValueError(f"no conversation {slug!r}")
        return p


def _scope_arg(value: object) -> dict[str, list[str]]:
    """The model's ``scope`` argument: a map of ability -> items, or nothing."""
    if value is None or value == "" or value == {}:
        return {}
    if not isinstance(value, Mapping):
        raise ValueError("scope must be a map of ability to items")
    out: dict[str, list[str]] = {}
    for ability, items in value.items():
        if isinstance(items, str):
            items = [s for s in re.split(r"[,;]", items)]
        if not isinstance(items, list):
            raise ValueError(f"scope for {ability} must be a list")
        out[str(ability)] = [str(i) for i in items]
    return out


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


def _speed(value: object, default: float) -> float:
    if value is None or value == "":
        return default
    try:
        s = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        raise ValueError("speed must be a number") from None
    if not 0.5 <= s <= 2.0:
        raise ValueError("speed must be between 0.5 and 2")
    return round(s, 2)
