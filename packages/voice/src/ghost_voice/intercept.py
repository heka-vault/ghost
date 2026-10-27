"""Run ability turn interceptors before the model sees a user turn (ADR 0012).

Sits right after STT. Each ``TranscriptionFrame`` spoken while Ghost is quiet is
offered to the interceptors in order, on its own and then joined with the earlier
pieces of the same turn ("I will talk to you" + "later"); the first ``Handled`` wins,
the frame is dropped, the turn's earlier pieces are taken back from the aggregator
(``on_handled``), and the model never sees any of it. Interceptors can speak a line, cancel a
pending change, put the session on hold, or end it. Anything they do goes
through the same gate as everything else.
"""

from __future__ import annotations

import time
from collections.abc import Awaitable, Callable, Sequence

from ghost_core import Handled, TurnInterceptor, UserTurn, WriteGate
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    InterruptionFrame,
    TranscriptionFrame,
    TTSSpeakFrame,
    UserStartedSpeakingFrame,
    UserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug
from ghost_voice.latency import NULL_PROBE, LatencyProbe
from ghost_voice.thinking import ThinkingStopFrame


class InterceptorProcessor(FrameProcessor):
    def __init__(
        self,
        interceptors: Sequence[TurnInterceptor],
        *,
        gate: WriteGate,
        session_id: str,
        on_hang_up: Callable[[], Awaitable[None]],
        on_hold: Callable[[], Awaitable[None]] | None = None,
        on_handled: Callable[[], Awaitable[None]] | None = None,
        last_assistant: Callable[[], str] | None = None,
        probe: LatencyProbe | None = None,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._probe = probe or NULL_PROBE
        self._interceptors = list(interceptors)
        self._gate = gate
        self._session_id = session_id
        self._on_hang_up = on_hang_up
        self._on_hold = on_hold
        self._on_handled = on_handled
        self._turn_pieces: list[str] = []  # this turn's transcripts that went to the model
        self._last_assistant = last_assistant or (lambda: "")
        self._bot_speaking = False

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif isinstance(frame, (UserStartedSpeakingFrame, UserStoppedSpeakingFrame)):
            self._turn_pieces.clear()  # a new turn, or this one has gone to the model
        elif (
            isinstance(frame, TranscriptionFrame) and not self._bot_speaking and self._interceptors
        ):
            pending = self._gate.pending(self._session_id) is not None
            last = self._last_assistant()
            began = time.perf_counter()
            # The piece on its own, then the whole turn so far: STT hands a goodbye over in
            # pieces, and the earlier ones have already gone down to the aggregator.
            texts = [frame.text]
            if self._turn_pieces:
                texts.append(" ".join([*self._turn_pieces, frame.text]))
            for text in texts:
                turn = UserTurn(text=text, pending=pending, last_assistant=last)
                for interceptor in self._interceptors:
                    try:
                        handled = await interceptor.on_user_turn(turn)
                    except Exception:  # noqa: BLE001 - an ability bug must not eat the turn
                        logger.exception(f"interceptor {interceptor.name} failed")
                        continue
                    if handled is not None:
                        debug.audio(f"intercepted by {interceptor.name}: {text!r} -> {handled}")
                        self._probe.intercepted(interceptor.name, time.perf_counter() - began)
                        if self._turn_pieces and self._on_handled:
                            await self._on_handled()  # the earlier pieces never run the model
                        self._turn_pieces.clear()
                        await self._handle(handled)
                        return  # the model never sees this turn
            self._probe.interceptors(time.perf_counter() - began)
            self._turn_pieces.append(frame.text)
        await self.push_frame(frame, direction)

    async def _handle(self, h: Handled) -> None:
        say = h.say
        # The model is not going to run on this turn: no thinking sound for it.
        await self.push_frame(ThinkingStopFrame())
        if h.action == "cancel":
            self._gate.cancel(self._session_id, "operator cancelled")
        elif h.action == "hang_up":
            # The change it was waiting on stays on the waiting list (ADR 0030).
            self._gate.set_aside(self._session_id, "hung up")
            # A reply to an earlier fragment ("okay, um") may still be on its way: cut it,
            # so the goodbye is the last thing said and the session ends now.
            await self.push_frame(InterruptionFrame())
        elif h.action == "hold":
            # Whatever Ghost was saying stops; a pending question stays pending for later.
            await self.push_frame(InterruptionFrame())
        if say:
            await self.push_frame(TTSSpeakFrame(say))
        if h.action == "hang_up":
            logger.info("interceptor: hang up")
            await self._on_hang_up()
        elif h.action == "hold" and self._on_hold:
            logger.info("interceptor: hold")
            await self._on_hold()
