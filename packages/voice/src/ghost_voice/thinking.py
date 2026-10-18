"""Plays a soft loop while Ghost is thinking.

"Thinking" is any gap where the model or a tool is working and nothing is being
spoken: after the operator's turn ends, while a tool call runs, and during the
follow-up inference after a tool result. The sound comes from a
``SoundfileMixer`` on the output transport; this processor only switches it on
and off with ``MixerEnableFrame``. A short delay avoids a blip on fast replies.

It sits between the LLM and TTS. From there it sees, going downstream, the
broadcast user-speaking frames, the LLM's response and function-call frames,
and ``ThinkingStartFrame`` (queued by the session for turns that did not come
through the microphone); going upstream, the transport's bot-speaking frames.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass

from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    CancelFrame,
    ControlFrame,
    EndFrame,
    Frame,
    FunctionCallInProgressFrame,
    FunctionCallResultFrame,
    LLMFullResponseEndFrame,
    MixerEnableFrame,
    TTSStartedFrame,
    UserStartedSpeakingFrame,
    UserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug


@dataclass
class ThinkingStartFrame(ControlFrame):
    """Queued with a programmatic first turn: the model is about to run."""


class ThinkingSoundProcessor(FrameProcessor):
    """Sound is on when the model or a tool is busy and Ghost is not speaking."""

    def __init__(self, *, delay_secs: float = 0.5, **kwargs) -> None:
        super().__init__(**kwargs)
        self._delay = delay_secs
        self._timer: asyncio.Task | None = None
        self._on = False
        self._busy = False
        self._bot_speaking = False
        self._calls_in_progress = 0

    @property
    def playing(self) -> bool:
        return self._on

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, (UserStoppedSpeakingFrame, ThinkingStartFrame)):
            self._busy = True
        elif isinstance(frame, FunctionCallInProgressFrame):
            self._calls_in_progress += 1
            self._busy = True
            debug.audio(f"thinking: tool {frame.function_name} started")
        elif isinstance(frame, FunctionCallResultFrame):
            self._calls_in_progress = max(0, self._calls_in_progress - 1)
            self._busy = True  # the model runs again on the result
            debug.audio(f"thinking: tool {frame.function_name} returned")
        elif isinstance(frame, LLMFullResponseEndFrame):
            # Text-only response finished: TTS is about to speak. If a tool is
            # still running the model is not done.
            if self._calls_in_progress == 0:
                self._busy = False
        elif isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif isinstance(frame, UserStartedSpeakingFrame):
            self._busy = False  # interruption: the operator has the floor
        elif isinstance(frame, (EndFrame, CancelFrame)):
            self._busy = False
        await self._reconcile()
        if isinstance(frame, ThinkingStartFrame):
            return
        await self.push_frame(frame, direction)

    async def _reconcile(self) -> None:
        want = self._busy and not self._bot_speaking
        if want and not self._on:
            if self._timer is None or self._timer.done():
                self._timer = self.create_task(self._after_delay())
        elif not want:
            self._cancel_timer()
            if self._on:
                await self._set(False)

    async def _after_delay(self) -> None:
        await asyncio.sleep(self._delay)
        if self._busy and not self._bot_speaking:
            await self._set(True)

    def _cancel_timer(self) -> None:
        if self._timer and not self._timer.done():
            self._timer.cancel()
        self._timer = None

    async def _set(self, on: bool) -> None:
        if on == self._on:
            return
        self._on = on
        debug.audio(f"thinking sound {'on' if on else 'off'}")
        await self.push_frame(MixerEnableFrame(enable=on), FrameDirection.DOWNSTREAM)


class ThinkingSoundCut(FrameProcessor):
    """Sits directly before the output transport and switches the mixer off the moment
    TTS starts producing speech.

    The ``ThinkingSoundProcessor`` upstream also sends an off, but only when playback
    has started, and that frame then queues behind whatever the TTS service is
    synthesising; the loop used to run about half a second into Ghost's reply.
    """

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TTSStartedFrame) and direction == FrameDirection.DOWNSTREAM:
            debug.audio("thinking sound cut: speech starting")
            await self.push_frame(MixerEnableFrame(enable=False), FrameDirection.DOWNSTREAM)
        await self.push_frame(frame, direction)
