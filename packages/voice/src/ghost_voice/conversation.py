"""The active conversation inside one session (ADR 0014).

``ConversationRuntime`` knows which conversation the session is in, builds the
system prompt with that conversation's persona, seeds the LLM context from its
memory, notes and recent turns, records what is said, and performs a transfer:
Ghost's voice announces it, a short tone plays, the TTS voice changes, the context
is replaced, and the new line greets with a one-line recap. A transfer cancels any
proposal still waiting for a yes.

Compaction persistence lives here too: when Pipecat's summarizer applies a summary,
the text is written to ``memory.md`` through the gate as a quiet, audited write.
"""

from __future__ import annotations

import asyncio
import math
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np
from ghost_core import ActiveScope, GateError, WriteGate
from ghost_core.conversations import (
    GHOST,
    SOURCE,
    SUMMARY_TEMPLATE,
    Conversations,
    ConversationTarget,
    Profile,
    describe_scope,
)
from ghost_core.conversations.models import SetMemory
from ghost_core.models import ChangeSet
from ghost_core.policy import Autonomy, PolicyError
from loguru import logger
from pipecat.frames.frames import (
    ControlFrame,
    Frame,
    LLMMessagesUpdateFrame,
    LLMUpdateSettingsFrame,
    OutputAudioRawFrame,
    TTSSpeakFrame,
    TTSUpdateSettingsFrame,
)

from ghost_voice.prompt import build_system_prompt

HANDOFF_NOTE = (
    "[system] The operator was just patched through to you from {origin}.{outcome} "
    "Greet them in one short sentence as yourself and say in a few words where this "
    "conversation left off, or that it is new. Then handle whatever they say next as "
    "usual, including asking to be patched somewhere else.{handoff}"
)


@dataclass
class ConversationMarkerFrame(ControlFrame):
    """Travels with the transfer frames so that text spoken after it is recorded under
    the new conversation and text before it under the old one, whatever the timing."""

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
        scoped_prompts: Mapping[str, Callable[[ActiveScope | None], str]] | None = None,
        reads_wider: bool = False,
        routing: bool = False,
    ) -> None:
        """``scoped_prompts`` are the scoped sources' ``ToolSource.scoped_prompt``
        by ability name, rebuilt for the active thread; ``reads_wider`` and ``routing``
        are the operator's crossing-streams settings (ADR 0019)."""
        self._conversations = conversations
        self._gate = gate
        self._session_id = session_id
        self._fragments = list(fragments)
        self._scoped_prompts = dict(scoped_prompts or {})
        self.reads_wider = reads_wider
        self.routing = routing
        self._tts_settings = tts_settings
        self._llm_settings = llm_settings
        self._sample_rate = sample_rate
        self._keep = keep_messages
        self._on_switch = on_switch
        p = conversations.get(start) or conversations.ensure_ghost()
        self.active: Profile = p
        self._speaking_as: str = p.slug  # follows the marker frames through the pipeline
        # Cleared from the start of a transfer until the new line has greeted; a
        # typed turn that arrives in that window waits (bounded), or it would run on
        # the old context and race the greeting.
        self.settled = asyncio.Event()
        self.settled.set()
        conversations.touch(p.slug)

    # --- what the session is built from -----------------------------------------

    @property
    def conversations(self) -> Conversations:
        return self._conversations

    def system_prompt(self, profile: Profile | None = None) -> str:
        p = profile or self.active
        fragments = list(self._fragments)
        for ability, fn in self._scoped_prompts.items():
            fragments.append(fn(self.scope_for(ability, p)))
        return build_system_prompt(
            [f for f in fragments if f],
            persona=self._conversations.files.persona(p.slug),
            name=p.name,
            is_ghost=p.is_ghost,
            scope=describe_scope(p.scope) if p.scope and not p.is_ghost else "",
            routing=self.routing,
        )

    # --- scope (ADR 0019) -------------------------------------------------------------

    def scope_for(self, ability: str, profile: Profile | None = None) -> ActiveScope:
        """The active thread's scope for one ability, as the pipeline hands it to the
        ability on each call. Unscoped (``items`` None) for Ghost and for a thread
        with no scope for that ability."""
        p = profile or self.active
        convs = self._conversations

        def owners(item: str) -> list[str]:
            return [q.name for q in convs.scope_of(ability, item) if q.slug != p.slug]

        return ActiveScope(
            ability=ability,
            thread=p.name,
            items=p.scope_for(ability),
            reads_wider=self.reads_wider,
            owners=owners,
        )

    def owner_of(self, ability: str, item: str) -> Profile | None:
        """The thread to route a request about ``item`` to: the first live thread
        whose scope covers it, other than the active one."""
        for q in self._conversations.scope_of(ability, item):
            if q.slug != self.active.slug:
                return q
        return None

    def initial_messages(self, profile: Profile | None = None) -> list[dict[str, Any]]:
        p = profile or self.active
        return self._conversations.resume(p.slug).messages()

    def start_frames(self) -> list[Frame]:
        """Set the TTS voice for the conversation the session opens in."""
        return [self._voice_frame(self.active)]

    # --- records --------------------------------------------------------------------

    def record(self, role: str, text: str) -> None:
        """User turns belong to the conversation the session is in now; assistant text
        belongs to the one that was speaking when it was generated (see the marker)."""
        slug = self._speaking_as if role == "assistant" else self.active.slug
        self._conversations.record(slug, self._session_id, role, text)
        if role == "assistant" and slug == self.active.slug and not self.settled.is_set():
            self.settled.set()  # the new line has greeted; turns may flow again

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
        return self._conversations.resolve(spoken)

    def switch_allowed(self) -> bool:
        try:
            level = self._gate.autonomy.resolve(SOURCE, "switch", risk="low")
        except Exception:  # noqa: BLE001
            return True
        return level is not Autonomy.DENY

    async def switch(self, target: Profile, *, handoff: str = "", outcome: str = "") -> list[Frame]:
        """Perform the transfer and return the frames that carry it out, in order:
        the announce line, the tone, the voice change, the marker, the new system
        prompt, and the context replacement that makes the new line greet. The caller
        feeds them in at the pipeline source once any in-flight tool result has been
        appended to the context (``settled`` is cleared until then). ``outcome`` is
        what just happened (a created conversation), told to the new line so it does
        not redo it."""
        origin = self.active
        if target.slug == origin.slug:
            return []
        if self._gate.pending(self._session_id) is not None:
            self._gate.cancel(self._session_id, "switched conversation")
        self.settled.clear()
        line = "Back to Ghost." if target.is_ghost else f"Patching you through to {target.name}."
        self.record("system", f"transferred to {target.name}")
        self.active = target
        self._conversations.touch(target.slug)
        self.record("system", f"transferred from {origin.name}")
        if self._on_switch:
            await self._on_switch(target)
        note = HANDOFF_NOTE.format(
            origin=origin.name,
            outcome=f" {outcome.strip()}" if outcome.strip() else "",
            handoff=f" Handoff from {origin.name}: {handoff.strip()}" if handoff.strip() else "",
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
            LLMMessagesUpdateFrame(messages=messages, run_llm=True),
        ]

    def transfer_done(self) -> None:
        """The transfer frames are in the pipeline. ``settled`` is set when the new
        line's greeting has been generated (see ``record``); a caller that could not
        queue the frames releases it here so nothing waits forever."""
        if self._speaking_as == self.active.slug:
            self.settled.set()

    def _voice_frame(self, p: Profile) -> Frame:
        return TTSUpdateSettingsFrame(delta=self._tts_settings(voice=p.voice, speed=p.speed))

    # --- compaction --------------------------------------------------------------------

    async def on_summary_applied(self, messages: Sequence[dict[str, Any]]) -> None:
        """Called after Pipecat replaced older messages with a summary. The summary is
        the first non-system message; persist it as the conversation's memory and mark
        the turns it stands in for. Which turns is approximate: everything but the
        last ``keep_messages`` spoken turns, which errs toward keeping more raw."""
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
