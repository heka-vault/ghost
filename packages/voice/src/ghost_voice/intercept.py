"""Run ability turn interceptors before the model sees a user turn (ADR 0012).

Sits right after STT. Each ``TranscriptionFrame`` spoken while Ghost is quiet is
offered to the interceptors in order; the first ``Handled`` wins, the frame is
dropped, and the model never sees it. Interceptors can speak a line, cancel a
pending change, undo the last one, or end the session. Anything they do goes
through the same gate as everything else.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence

from ghost_core import GateError, Handled, TurnInterceptor, UserTurn, WriteGate
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    TranscriptionFrame,
    TTSSpeakFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug


class InterceptorProcessor(FrameProcessor):
    def __init__(
        self,
        interceptors: Sequence[TurnInterceptor],
        *,
        gate: WriteGate,
        session_id: str,
        on_sleep: Callable[[], Awaitable[None]],
        last_assistant: Callable[[], str] | None = None,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._interceptors = list(interceptors)
        self._gate = gate
        self._session_id = session_id
        self._on_sleep = on_sleep
        self._last_assistant = last_assistant or (lambda: "")
        self._bot_speaking = False

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif (
            isinstance(frame, TranscriptionFrame) and not self._bot_speaking and self._interceptors
        ):
            turn = UserTurn(
                text=frame.text,
                pending=self._gate.pending(self._session_id) is not None,
                last_assistant=self._last_assistant(),
            )
            for interceptor in self._interceptors:
                try:
                    handled = await interceptor.on_user_turn(turn)
                except Exception:  # noqa: BLE001 - an ability bug must not eat the turn
                    logger.exception(f"interceptor {interceptor.name} failed")
                    continue
                if handled is not None:
                    debug.audio(f"intercepted by {interceptor.name}: {frame.text!r} -> {handled}")
                    await self._handle(handled)
                    return  # the model never sees this turn
        await self.push_frame(frame, direction)

    async def _handle(self, h: Handled) -> None:
        say = h.say
        if h.action == "cancel":
            self._gate.cancel(self._session_id, "operator cancelled")
        elif h.action == "undo":
            try:
                outcome = await self._gate.undo(self._session_id)
                say = say or outcome.summary()
            except GateError as e:
                say = str(e)
        elif h.action == "sleep":
            self._gate.cancel(self._session_id, "went to sleep")
        if say:
            await self.push_frame(TTSSpeakFrame(say))
        if h.action == "sleep":
            logger.info("interceptor: sleep")
            await self._on_sleep()
