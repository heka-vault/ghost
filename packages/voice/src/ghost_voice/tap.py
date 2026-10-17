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
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

Phase = Literal["listening", "thinking", "speaking"]
OnTurn = Callable[[str, str], Awaitable[None]]  # (role, text)
OnPhase = Callable[[Phase], Awaitable[None]]


class UserTap(FrameProcessor):
    def __init__(self, on_turn: OnTurn, on_phase: OnPhase, **kwargs) -> None:
        super().__init__(**kwargs)
        self._on_turn = on_turn
        self._on_phase = on_phase

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TranscriptionFrame) and frame.text.strip():
            await self._on_turn("user", frame.text)
            await self._on_phase("thinking")
        elif isinstance(frame, BotStartedSpeakingFrame):
            await self._on_phase("speaking")
        elif isinstance(frame, BotStoppedSpeakingFrame):
            await self._on_phase("listening")
        await self.push_frame(frame, direction)


class AssistantTap(FrameProcessor):
    def __init__(self, on_turn: OnTurn, **kwargs) -> None:
        super().__init__(**kwargs)
        self._on_turn = on_turn
        self._buf: list[str] = []

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, LLMTextFrame):
            self._buf.append(frame.text)
        elif isinstance(frame, LLMFullResponseEndFrame):
            text = "".join(self._buf).strip()
            self._buf.clear()
            if text:
                await self._on_turn("assistant", text)
        elif isinstance(frame, TTSSpeakFrame) and frame.text.strip():
            await self._on_turn("assistant", frame.text)
        await self.push_frame(frame, direction)
