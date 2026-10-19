"""Turn-start rule that cannot be tripped by Ghost hearing itself.

Pipecat's default starts a user turn on any VAD activity, and starting a turn
while the bot is speaking is an interruption. On a workstation with open
speakers, Kokoro's own voice reaching the microphone is VAD activity, so with
the default rule Ghost cut itself off after a few words.

Echo cancellation in PipeWire (``infra/pipewire``) is the real fix. This
strategy is the backstop for when it leaks:

- While Ghost is silent, VAD starts the turn at once, as before.
- While Ghost is speaking, only a transcript of at least ``min_words`` words
  starts (and so interrupts) the turn. Shorter fragments are discarded so they
  never leak into the next turn. ``min_words=0`` disables barge-in entirely.
"""

from __future__ import annotations

import asyncio
import time

from pipecat.audio.turn.smart_turn.base_smart_turn import BaseSmartTurn
from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import LocalSmartTurnAnalyzerV3
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
)
from pipecat.turns.types import ProcessFrameResult
from pipecat.turns.user_start import BaseUserTurnStartStrategy
from pipecat.turns.user_stop.turn_analyzer_user_turn_stop_strategy import (
    TurnAnalyzerUserTurnStopStrategy,
)

from ghost_voice import debug
from ghost_voice.latency import NULL_PROBE, LatencyProbe


class BargeInUserTurnStartStrategy(BaseUserTurnStartStrategy):
    def __init__(self, *, min_words: int = 2, **kwargs) -> None:
        super().__init__(**kwargs)
        self._min_words = min_words
        self._bot_speaking = False

    @property
    def bot_speaking(self) -> bool:
        return self._bot_speaking

    async def handle_user_turn_started(self) -> None:
        # A turn opened by any strategy ends the bot's speech (interruption), so
        # subsequent fragments belong to the operator.
        self._bot_speaking = False

    async def process_frame(self, frame: Frame) -> ProcessFrameResult:
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif isinstance(frame, VADUserStartedSpeakingFrame):
            if not self._bot_speaking:
                debug.audio("turn: started by VAD")
                await self.trigger_user_turn_started()
                return ProcessFrameResult.STOP
            debug.audio("turn: VAD while Ghost speaks, waiting for words")
        elif isinstance(frame, (TranscriptionFrame, InterimTranscriptionFrame)):
            return await self._on_text(frame.text)
        return ProcessFrameResult.CONTINUE

    async def _on_text(self, text: str) -> ProcessFrameResult:
        words = len(text.split())
        if not self._bot_speaking:
            if words >= 1:
                await self.trigger_user_turn_started()
                return ProcessFrameResult.STOP
            return ProcessFrameResult.CONTINUE
        if self._min_words > 0 and words >= self._min_words:
            debug.audio(f"turn: barge-in on {text!r}")
            await self.trigger_user_turn_started()
            return ProcessFrameResult.STOP
        # Too short to be a deliberate interruption while the bot talks: most
        # likely the bot's own voice. Drop it rather than queue it.
        debug.audio(f"turn: dropped {text!r} while Ghost speaks")
        await self.trigger_reset_aggregation()
        return ProcessFrameResult.CONTINUE


class PatientTurnStopStrategy(TurnAnalyzerUserTurnStopStrategy):
    """Smart Turn's end of turn, held for ``grace_secs`` before it counts.

    Smart Turn ends a turn as soon as the words sound finished, and people pause in the
    middle of a thought ("are there any other open issues I need to … look at"). Its
    verdict waits here a moment; if the operator starts speaking again it is dropped, the
    turn stays open, and the next words join it. ``grace_secs=0`` is Smart Turn as is.
    """

    def __init__(
        self, *, grace_secs: float = 0.0, probe: LatencyProbe | None = None, **kwargs
    ) -> None:
        super().__init__(**kwargs)
        self._grace = grace_secs
        self._held: asyncio.Task | None = None
        self._probe = probe or NULL_PROBE

    async def trigger_user_turn_stopped(self, **kwargs) -> None:
        if self._grace <= 0:
            await super().trigger_user_turn_stopped(**kwargs)
            return
        await self._release_hold()

        async def later() -> None:
            began = time.monotonic()
            await asyncio.sleep(self._grace)
            self._held = None
            self._probe.held(time.monotonic() - began)
            await TurnAnalyzerUserTurnStopStrategy.trigger_user_turn_stopped(self, **kwargs)

        self._held = self.task_manager.create_task(later(), f"{self}::grace")

    async def _release_hold(self) -> None:
        if self._held is not None:
            held, self._held = self._held, None
            await self.task_manager.cancel_task(held)

    async def process_frame(self, frame: Frame) -> ProcessFrameResult:
        if isinstance(frame, VADUserStartedSpeakingFrame) and self._held is not None:
            debug.audio("turn: kept open, the operator went on")
            self._probe.kept_open()
            await self._release_hold()
        return await super().process_frame(frame)

    async def handle_user_turn_started(self) -> None:
        await self._release_hold()
        await super().handle_user_turn_started()

    async def handle_user_turn_stopped(self) -> None:
        await self._release_hold()
        await super().handle_user_turn_stopped()

    async def cleanup(self) -> None:
        await self._release_hold()
        await super().cleanup()


class SmartTurn(LocalSmartTurnAnalyzerV3):
    """Smart Turn v3 whose loaded ONNX session is shared by every session's analyzer.

    An analyzer buffers one audio stream's speech and silence, so a session that
    ended mid-utterance (or was cancelled) would hand its half-turn to the next one,
    and two live sessions would mix their audio. ``fork()`` gives each pipeline its
    own analyzer over the one loaded model.
    """

    def fork(self) -> SmartTurn:
        other = SmartTurn.__new__(SmartTurn)
        BaseSmartTurn.__init__(other, sample_rate=self._init_sample_rate, params=self._params)
        other._log_data = self._log_data
        other._session = self._session
        return other
