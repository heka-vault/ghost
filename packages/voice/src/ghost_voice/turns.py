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
  An interim transcript counts (``parakeet.py`` re-reads the operator's open
  segment while Ghost speaks), so the operator needn't pause to cut Ghost off.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import Callable

from pipecat.audio.turn.smart_turn.base_smart_turn import BaseSmartTurn
from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import LocalSmartTurnAnalyzerV3
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    EagerEndOfTurnCancelFrame,
    Frame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
)
from pipecat.turns.types import ProcessFrameResult, UserTurnSpeculation
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
            return await self._on_text(
                frame.text, interim=isinstance(frame, InterimTranscriptionFrame)
            )
        return ProcessFrameResult.CONTINUE

    async def _on_text(self, text: str, *, interim: bool = False) -> ProcessFrameResult:
        words = len(text.split())
        if not self._bot_speaking:
            if words >= 1:
                await self.trigger_user_turn_started()
                return ProcessFrameResult.STOP
            return ProcessFrameResult.CONTINUE
        if self._min_words > 0 and words >= self._min_words:
            debug.audio(f"turn: barge-in on {'interim ' if interim else ''}{text!r}")
            await self.trigger_user_turn_started()
            return ProcessFrameResult.STOP
        if interim:
            return ProcessFrameResult.CONTINUE  # nothing was aggregated; the next may say more
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

    With ``speculate`` the model starts answering at the verdict rather than after the
    hold, so the hold costs nothing when the operator is done (Pipecat's speculative
    inference). The answer is held inside the LLM service until the hold ends and the
    turn is confirmed, and thrown away if the operator goes on. A speculative answer that
    reaches a tool call withdraws itself and the confirmed turn runs the call, so nothing
    is written for a turn that hadn't ended.

    The speculation answers ``turn_text()``, the user aggregator's whole turn so far: the
    analyzer's own ``_text`` is only the latest transcript, so a turn kept open across a
    pause would be answered from its second half. Set it once the aggregator exists.
    """

    def __init__(
        self,
        *,
        grace_secs: float = 0.0,
        speculate: bool = False,
        probe: LatencyProbe | None = None,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._grace = grace_secs
        self._speculate = speculate
        self._held: asyncio.Task | None = None
        self._speculation: UserTurnSpeculation | None = None
        self._probe = probe or NULL_PROBE
        self.turn_text: Callable[[], str] | None = None

    def _heard(self) -> str:
        return self.turn_text() if self.turn_text else self._text

    async def trigger_user_turn_stopped(self, **kwargs) -> None:
        if self._grace <= 0:
            await super().trigger_user_turn_stopped(**kwargs)
            return
        await self._release_hold()
        heard = self._heard()
        if self._speculate and heard.strip():
            self._speculation = UserTurnSpeculation(text=heard)
            self._probe.speculation("started")
            await self.trigger_user_turn_inference_triggered(speculation=self._speculation)

        async def later() -> None:
            began = time.monotonic()
            await asyncio.sleep(self._grace)
            self._held = None
            self._probe.held(time.monotonic() - began)
            speculation, self._speculation = self._speculation, None
            if speculation is not None and speculation.text == self._heard():
                self._probe.speculation("kept")
                await self.trigger_user_turn_finalized(confirms_speculation=True, **kwargs)
                return
            if speculation is not None:  # the words changed (late, or an interceptor took them)
                self._probe.speculation("missed")
                await self.trigger_user_turn_speculation_cancelled()
            await TurnAnalyzerUserTurnStopStrategy.trigger_user_turn_stopped(self, **kwargs)

        self._held = self.task_manager.create_task(later(), f"{self}::grace")

    async def _release_hold(self) -> None:
        if self._held is not None:
            held, self._held = self._held, None
            await self.task_manager.cancel_task(held)
        if self._speculation is not None:
            self._speculation = None
            self._probe.speculation("withdrawn")
            await self.trigger_user_turn_speculation_cancelled()

    async def process_frame(self, frame: Frame) -> ProcessFrameResult:
        if isinstance(frame, VADUserStartedSpeakingFrame) and self._held is not None:
            debug.audio("turn: kept open, the operator went on")
            self._probe.kept_open()
            await self._release_hold()
        elif isinstance(frame, EagerEndOfTurnCancelFrame) and self._speculation is not None:
            # The LLM service withdrew it: the answer reached a tool call. The confirmed
            # turn has to run its own inference, which makes the call.
            self._speculation = None
            self._probe.speculation("tool")
        return await super().process_frame(frame)

    async def handle_user_turn_started(self) -> None:
        await self._release_hold()
        await super().handle_user_turn_started()

    async def handle_user_turn_stopped(self) -> None:
        await self._release_hold()
        await super().handle_user_turn_stopped()

    async def cleanup(self) -> None:
        self._speculation = None  # the session is ending; nothing is left to withdraw it from
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
