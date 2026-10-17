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
                await self.trigger_user_turn_started()
                return ProcessFrameResult.STOP
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
            await self.trigger_user_turn_started()
            return ProcessFrameResult.STOP
        # Too short to be a deliberate interruption while the bot talks: most
        # likely the bot's own voice. Drop it rather than queue it.
        await self.trigger_reset_aggregation()
        return ProcessFrameResult.CONTINUE
