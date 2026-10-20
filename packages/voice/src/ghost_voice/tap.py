"""Observe a session for the control API: phase changes and spoken turns.

Two processors, because user text and assistant text live in different parts of the
pipeline. ``UserTap`` sits right after STT and sees every transcription (including
the ones an interceptor later takes). ``AssistantTap`` sits right before TTS and
collects the model's text per response, plus anything spoken directly
(``TTSSpeakFrame``). Nothing here touches passive audio: both only exist inside a
session.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Literal

from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    LLMFullResponseEndFrame,
    LLMTextFrame,
    TranscriptionFrame,
    TTSSpeakFrame,
    UserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice.conversation import ConversationMarkerFrame

Phase = Literal["listening", "thinking", "speaking"]
OnTurn = Callable[[str, str], Awaitable[None]]  # (role, text)
OnPhase = Callable[[Phase], Awaitable[None]]
OnMarker = Callable[[ConversationMarkerFrame], None]


class UserTap(FrameProcessor):
    def __init__(
        self,
        on_turn: OnTurn,
        on_phase: OnPhase,
        held: Callable[[], bool] | None = None,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._on_turn = on_turn
        self._on_phase = on_phase
        # On hold (ADR 0021) the end of Ghost's "take your time" is not "listening".
        self._held = held or (lambda: False)

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TranscriptionFrame) and frame.text.strip():
            await self._on_turn("user", frame.text)
        elif isinstance(frame, UserStoppedSpeakingFrame) and not self._held():
            # The turn is over (after the grace hold), so the model is about to run. A
            # transcript on its own isn't that: sentences arrive in pieces while the
            # operator is still talking, and the phone showed Thinking mid-sentence.
            # On hold the room's voices open turns with no words in them; not thinking.
            await self._on_phase("thinking")
        elif isinstance(frame, BotStartedSpeakingFrame):
            await self._on_phase("speaking")
        elif isinstance(frame, BotStoppedSpeakingFrame):
            await self._on_phase("hold" if self._held() else "listening")
        await self.push_frame(frame, direction)


class AssistantTap(FrameProcessor):
    def __init__(self, on_turn: OnTurn, on_marker: OnMarker | None = None, **kwargs) -> None:
        super().__init__(**kwargs)
        self._on_turn = on_turn
        self._on_marker = on_marker
        self._buf: list[str] = []

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, ConversationMarkerFrame):
            # Text generated so far was the old line's; flush it before moving on.
            text = "".join(self._buf).strip()
            self._buf.clear()
            if text:
                await self._on_turn("assistant", text)
            if self._on_marker:
                self._on_marker(frame)
        elif isinstance(frame, LLMTextFrame):
            self._buf.append(frame.text)
        elif isinstance(frame, LLMFullResponseEndFrame):
            text = "".join(self._buf).strip()
            self._buf.clear()
            if text:
                await self._on_turn("assistant", text)
        elif isinstance(frame, TTSSpeakFrame) and frame.text.strip():
            await self._on_turn("assistant", frame.text)
        await self.push_frame(frame, direction)
