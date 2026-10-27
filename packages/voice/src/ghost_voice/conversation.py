"""The active agent inside one session (ADR 0014, ADR 0024).

``ConversationRuntime`` knows which agent the session is in, builds the system prompt
from what its department reaches plus the department's rules and shared memory and the
agent's persona, seeds the LLM context from its memory, notes and recent turns, records
what is said, and performs a transfer: the department's servers are started if they are
not up, Ghost's voice announces it, a short tone plays, the TTS voice changes, the context
is replaced, and the agent greets with a one-line recap. A transfer cancels any proposal
still waiting for a yes. A move re-homes the session the same way, in place.

Compaction persistence lives here too: when Pipecat's summarizer applies a summary,
the text is written to ``memory.md`` through the gate as a quiet, audited write.
"""

from __future__ import annotations

import asyncio
import math
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np
from ghost_core import ActiveScope, GateError, ToolSource, WriteGate
from ghost_core.conversations import (
    GHOST,
    SOURCE,
    SUMMARY_TEMPLATE,
    Conversations,
    ConversationTarget,
    Profile,
)
from ghost_core.conversations.models import SetMemory
from ghost_core.models import ChangeSet
from ghost_core.policy import Autonomy, PolicyError
from loguru import logger
from pipecat.adapters.schemas.tools_schema import ToolsSchema
from pipecat.frames.frames import (
    ControlFrame,
    Frame,
    LLMMessagesUpdateFrame,
    LLMSetToolsFrame,
    LLMUpdateSettingsFrame,
    OutputAudioRawFrame,
    TTSSpeakFrame,
    TTSUpdateSettingsFrame,
)

from ghost_voice.prompt import build_system_prompt

HANDOFF_NOTE = (
    "[system] The operator was just transferred to you from {origin}.{outcome} "
    "Greet them in one short sentence as yourself and say in a few words where this "
    "conversation left off, or that it is new. Then handle whatever they say next as "
    "usual, including asking to be transferred somewhere else.{handoff}"
)
MOVED_NOTE = (
    "[system] This conversation was just moved to the {department} department at the "
    "operator's request; that is done, do not move it again. You now work with that "
    "department's tools, rules and memory. Say in a few words that you are in {department} "
    "now, then carry on where you left off."
)

Reach = Callable[[Profile], list[ToolSource]]
OnEnter = Callable[[Profile], Awaitable[None]]
OnLeave = Callable[[Profile], None]


@dataclass
class ConversationMarkerFrame(ControlFrame):
    """Travels with the transfer frames so that text spoken after it is recorded under
    the new agent and text before it under the old one, whatever the timing."""

    slug: str = ""


def transfer_tone(sample_rate: int) -> bytes:
    """Two quick rising notes, like a line connecting. 16-bit mono."""
    out = []
    for freq, length in ((740.0, 0.07), (988.0, 0.11)):
        n = int(sample_rate * length)
        t = np.arange(n) / sample_rate
        env = np.minimum(t / 0.004, 1.0) * np.exp(-t / (length * 0.45))
        out.append(env * np.sin(2 * math.pi * freq * t))
        out.append(np.zeros(int(sample_rate * 0.03)))
    audio = np.concatenate(out)
    audio *= 0.35 / max(1e-6, float(np.abs(audio).max()))
    return (audio * 32767).astype(np.int16).tobytes()


class ConversationRuntime:
    def __init__(
        self,
        conversations: Conversations,
        *,
        gate: WriteGate,
        session_id: str,
        fragments: Sequence[str],
        tts_settings: Callable[..., Any],
        llm_settings: Callable[..., Any],
        sample_rate: int,
        keep_messages: int = 20,
        start: str = GHOST,
        on_switch: Callable[[Profile], Awaitable[None]] | None = None,
        reach: Reach | None = None,
        routing: bool = False,
        on_enter: OnEnter | None = None,
        on_leave: OnLeave | None = None,
    ) -> None:
        """``reach(agent)`` gives the sources a session in that agent's department gets
        (ADR 0024); None means every session gets ``fragments`` and whatever tools were
        registered, as before departments. ``on_enter`` runs before a transfer's frames are
        built (the department's servers start then); ``on_leave`` after one, with the
        agent left. ``routing`` is the operator's department-routing setting (ADR 0019)."""
        self._conversations = conversations
        self._gate = gate
        self._session_id = session_id
        self._fragments = list(fragments)
        self._reach = reach
        self._on_enter = on_enter
        self._on_leave = on_leave
        # Set by register_tools: the schemas for a list of sources, for a transfer.
        self.tool_schemas: Callable[[list[ToolSource]], list[Any]] | None = None
        self.routing = routing
        self._tts_settings = tts_settings
        self._llm_settings = llm_settings
        self._sample_rate = sample_rate
        self._keep = keep_messages
        self._on_switch = on_switch
        p = conversations.get(start) or conversations.ensure_ghost()
        self.active: Profile = p
        self._speaking_as: str = p.slug  # follows the marker frames through the pipeline
        # Cleared from the start of a transfer until the new agent has greeted; a
        # typed turn that arrives in that window waits (bounded), or it would run on
        # the old context and race the greeting.
        self.settled = asyncio.Event()
        self.settled.set()
        conversations.touch(p.slug)

    # --- what the session is built from -----------------------------------------

    @property
    def conversations(self) -> Conversations:
        return self._conversations

    @property
    def has_reach(self) -> bool:
        return self._reach is not None

    def sources_for(self, profile: Profile | None = None) -> list[ToolSource]:
        """The sources a session in this agent reaches (ADR 0024): its department's, read
        on every call, so a server that came up since is seen at once."""
        if self._reach is None:
            return []
        return list(self._reach(profile or self.active))

    def roster(self) -> str:
        """Ghost's listing: each department by name with the first line of its rules, and
        its agents by name and job."""
        lines = []
        for d in self._conversations.departments.list():
            if d.is_ghost:
                continue
            agents = [
                p.name + (f" ({p.job})" if p.job else "")
                for p in self._conversations.agents_of(d.slug)
                if not p.is_default
            ]
            about = d.rules_lead()
            line = f"- {d.title}" + (f": {about}" if about else "")
            if agents:
                line += f". Agents: {', '.join(agents)}"
            lines.append(line)
        return "\n".join(lines)

    def system_prompt(self, profile: Profile | None = None) -> str:
        p = profile or self.active
        if self._reach is None:
            fragments = list(self._fragments)
        else:
            fragments = []
            for src in self.sources_for(p):
                if src.scope_kind and src.scoped_prompt:
                    fragments.append(src.scoped_prompt(self.scope_for(src.name, p)))
                elif src.prompt:
                    fragments.append(src.prompt)
        d = self._conversations.department_of(p)
        return build_system_prompt(
            [f for f in fragments if f],
            persona=self._conversations.files.persona(p.slug),
            name=p.name,
            is_ghost=p.is_ghost,
            department=d.title,
            job=p.job,
            is_default=p.is_default,
            rules=d.rules(),
            rules_from_root=d.root is not None,
            memory=d.memory(),
            roster=self.roster() if p.is_ghost else "",
            routing=self.routing,
        )

    # --- scope (ADR 0019) -------------------------------------------------------------

    def scope_for(self, ability: str, profile: Profile | None = None) -> ActiveScope:
        """What the pipeline hands a scoped source on each call: unscoped (the server's own
        allowlist is the limit), naming the departments that cover an item outside it."""
        p = profile or self.active
        convs = self._conversations
        server = ability.rpartition("/")[2]

        def owners(item: str) -> list[str]:
            return [q.name for q in convs.owners_of(server, item) if q.department != p.department]

        return ActiveScope(ability=ability, thread=p.name, items=None, owners=owners)

    def department_slug(self) -> str:
        """The department the active agent works in (ADR 0024), for the audit."""
        return self.active.department

    def owner_of(self, ability: str, item: str) -> Profile | None:
        """The agent to route a request about ``item`` to: the default agent of the first
        other department whose declaration covers it."""
        server = ability.rpartition("/")[2]
        for q in self._conversations.owners_of(server, item):
            if q.department != self.active.department:
                return q
        return None

    def initial_messages(self, profile: Profile | None = None) -> list[dict[str, Any]]:
        p = profile or self.active
        return self._conversations.resume(p.slug).messages()

    def start_frames(self) -> list[Frame]:
        """Set the TTS voice for the agent the session opens in."""
        return [self._voice_frame(self.active)]

    # --- records --------------------------------------------------------------------

    def record(self, role: str, text: str) -> None:
        """User turns belong to the agent the session is in now; assistant text
        belongs to the one that was speaking when it was generated (see the marker)."""
        slug = self._speaking_as if role == "assistant" else self.active.slug
        self._conversations.record(slug, self._session_id, role, text)
        if role == "assistant" and slug == self.active.slug and not self.settled.is_set():
            self.settled.set()  # the new agent has greeted; turns may flow again

    def on_marker(self, frame: ConversationMarkerFrame) -> None:
        self._speaking_as = frame.slug

    def record_call(self, name: str, args: dict[str, Any]) -> None:
        shown = ", ".join(
            f"{k}={_short(v)}" for k, v in sorted(args.items()) if not k.startswith("_")
        )
        self.record("call", f"{name}({shown})")

    def carry_session_turns(self, target: Profile) -> int:
        """Save-as: what was said in this session so far moves to ``target``."""
        return self._conversations.carry(self._session_id, self.active.slug, target.slug)

    # --- transfer --------------------------------------------------------------------

    def resolve(self, spoken: str) -> Profile | None:
        """An agent by name, or a department by name (its own agent)."""
        p = self._conversations.resolve(spoken)
        if p is not None:
            return p
        d = self._conversations.departments.resolve(spoken)
        if d is not None:
            return self._conversations.default_agent(d.slug)
        return None

    def switch_allowed(self) -> bool:
        try:
            level = self._gate.autonomy.resolve(SOURCE, "switch", risk="low")
        except Exception:  # noqa: BLE001
            return True
        return level is not Autonomy.DENY

    async def switch(
        self,
        target: Profile,
        *,
        handoff: str = "",
        outcome: str = "",
        rehome: bool = False,
    ) -> list[Frame]:
        """Perform the transfer and return the frames that carry it out, in order:
        the announce line, the tone, the voice change, the marker, the new system
        prompt, the new tool list, and the context replacement that makes the agent
        greet. The caller feeds them in at the pipeline source once any in-flight tool
        result has been appended to the context (``settled`` is cleared until then).
        ``outcome`` is what just happened (a created agent), told to the new agent so it
        does not redo it. ``rehome`` re-enters the same agent after a move (ADR 0024):
        the destination's servers, rules, memory and voice, in place."""
        origin = self.active
        if target.slug == origin.slug and not rehome:
            return []
        if self._gate.pending(self._session_id) is not None:
            # Set aside, not cancelled: it waits on the waiting list (ADR 0030).
            self._gate.set_aside(self._session_id, "switched conversation")
        self.settled.clear()
        if self._on_enter is not None:
            await self._on_enter(target)  # the department's servers, before anything is said
        dept = self._conversations.department_of(target)
        if rehome:
            line = f"Moved to {dept.title}."
            self.record("system", f"moved to {dept.title}")
        else:
            line = "Back to Ghost." if target.is_ghost else f"Transferring you to {target.name}."
            self.record("system", f"transferred to {target.name}")
        self.active = target
        self._conversations.touch(target.slug)
        if not rehome:
            self.record("system", f"transferred from {origin.name}")
        if self._on_switch:
            await self._on_switch(target)
        if self._on_leave is not None and origin.department != target.department:
            self._on_leave(origin)
        if rehome:
            note = MOVED_NOTE.format(department=dept.title)
            messages = self.initial_messages(target) + [{"role": "user", "content": note}]
        else:
            note = HANDOFF_NOTE.format(
                origin=origin.name,
                outcome=f" {outcome.strip()}" if outcome.strip() else "",
                handoff=f" Handoff from {origin.name}: {handoff.strip()}"
                if handoff.strip()
                else "",
            )
            messages = self.initial_messages(target) + [{"role": "user", "content": note}]
        return [
            TTSSpeakFrame(line, append_to_context=False),  # the tap records it
            OutputAudioRawFrame(
                audio=transfer_tone(self._sample_rate),
                sample_rate=self._sample_rate,
                num_channels=1,
            ),
            self._voice_frame(target),
            ConversationMarkerFrame(slug=target.slug),
            LLMUpdateSettingsFrame(
                delta=self._llm_settings(system_instruction=self.system_prompt(target))
            ),
            *(
                [LLMSetToolsFrame(tools=ToolsSchema(self.tool_schemas(self.sources_for(target))))]
                if self._reach is not None and self.tool_schemas is not None
                else []
            ),
            LLMMessagesUpdateFrame(messages=messages, run_llm=True),
        ]

    def transfer_done(self) -> None:
        """The transfer frames are in the pipeline. ``settled`` is set when the new
        agent's greeting has been generated (see ``record``); a caller that could not
        queue the frames releases it here so nothing waits forever."""
        if self._speaking_as == self.active.slug:
            self.settled.set()

    def _voice_frame(self, p: Profile) -> Frame:
        voice, speed = self._conversations.voice_for(p)
        return TTSUpdateSettingsFrame(delta=self._tts_settings(voice=voice, speed=speed))

    # --- compaction --------------------------------------------------------------------

    async def on_summary_applied(self, messages: Sequence[dict[str, Any]]) -> None:
        """Called after Pipecat replaced older messages with a summary. The summary is
        the first non-system message; persist it as the agent's memory and mark the
        turns it stands in for. Which turns is approximate: everything but the last
        ``keep_messages`` spoken turns, which errs toward keeping more raw."""
        summary = _summary_text(messages)
        if not summary:
            return
        slug = self.active.slug
        recent = self._conversations.resume(slug).recent
        spoken = [t for t in recent if t.role in ("user", "assistant")]
        through = spoken[-self._keep - 1].id if len(spoken) > self._keep else None
        try:
            out = await self._gate.apply_internal(
                session_id=self._session_id,
                ability=SOURCE,
                operation="compact",
                target=ConversationTarget(slug=slug),
                changes=ChangeSet(
                    steps=(SetMemory(text=summary, through_turn_id=through, source="auto"),)
                ),
                rationale="automatic compaction",
            )
        except (GateError, PolicyError) as e:
            logger.warning(f"compaction not persisted: {e}")
            return
        logger.info(f"compacted {slug}: {out.status}, memory {len(summary)} chars")


def _summary_text(messages: Sequence[dict[str, Any]]) -> str:
    prefix = SUMMARY_TEMPLATE.split("{summary}")[0]
    for m in messages:
        if m.get("role") == "system":
            continue
        content = m.get("content")
        if isinstance(content, str) and content.startswith(prefix):
            return content[len(prefix) :].strip()
        return ""
    return ""


def _short(v: Any) -> str:
    s = str(v)
    return s if len(s) <= 40 else s[:37] + "..."
