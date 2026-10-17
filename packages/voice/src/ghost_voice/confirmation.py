"""Feeds the operator's transcribed turns to the write gate.

Sits between STT and the user aggregator. Every TranscriptionFrame it sees is
the pipeline's own evidence of what the operator said (ADR 0007). When a
proposal is pending, the gate decides; on an affirmative the gate executes and
the verified outcome is placed in the LLM context as a system note so the model
speaks the real result.
"""

from __future__ import annotations

from ghost_domain import Verdict, WriteGate
from loguru import logger
from pipecat.frames.frames import Frame, LLMMessagesAppendFrame, TranscriptionFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor


class ConfirmationProcessor(FrameProcessor):
    def __init__(self, gate: WriteGate, session_id: str, **kwargs) -> None:
        super().__init__(**kwargs)
        self._gate = gate
        self._session_id = session_id

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TranscriptionFrame) and self._gate.pending(self._session_id):
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
