"""Feeds the operator's transcribed turns to the write gate.

Sits between STT and the user aggregator. Every TranscriptionFrame it sees is
the pipeline's own evidence of what the operator said (ADR 0007). When a
proposal is pending, the gate decides; on an affirmative the gate executes and
the verified outcome is placed in the LLM context as a system note so the model
speaks the real result.

A transcript that arrives while Ghost is still talking is not confirmation
evidence: the operator has not heard the whole readback yet, and before echo
cancellation it is most likely Ghost's own voice. Such frames pass through
untouched and the proposal stays pending until a turn after the readback ends.
"""

from __future__ import annotations

from ghost_domain import Verdict, WriteGate
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    LLMMessagesAppendFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor


class ConfirmationProcessor(FrameProcessor):
    def __init__(self, gate: WriteGate, session_id: str, **kwargs) -> None:
        super().__init__(**kwargs)
        self._gate = gate
        self._session_id = session_id
        self._bot_speaking = False

    @property
    def bot_speaking(self) -> bool:
        return self._bot_speaking

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif (
            isinstance(frame, TranscriptionFrame)
            and not self._bot_speaking
            and self._gate.pending(self._session_id)
        ):
            verdict, outcome = await self._gate.on_user_turn(self._session_id, frame.text)
            if verdict is Verdict.AFFIRMATIVE and outcome is not None:
                logger.info(f"executed {outcome.proposal_id}: {outcome.status}")
                note = (
                    "[system] The operator confirmed. Result, verified by reading the issue "
                    f"back: {outcome.spoken()} Tell the operator exactly this."
                )
            elif verdict is Verdict.NEGATIVE:
                note = "[system] The operator declined. The proposal is cancelled. Say so briefly."
            else:
                note = (
                    "[system] The pending proposal was cancelled because the operator did not "
                    "clearly confirm. If they still want it, prepare it again."
                )
            await self.push_frame(
                LLMMessagesAppendFrame(messages=[{"role": "user", "content": note}], run_llm=False)
            )
        await self.push_frame(frame, direction)
