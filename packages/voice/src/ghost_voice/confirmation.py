"""Feeds the operator's transcribed turns to the write gate.

Sits between STT and the user aggregator. Every TranscriptionFrame it sees is
the pipeline's own evidence of what the operator said (ADR 0007). When a
proposal is pending (a ``confirm``-level operation, ADR 0011), the gate's
classifier decides; on an affirmative the gate executes and the verified outcome
is placed in the LLM context as a system note so the model speaks the real
result. Ghost's last reply (read from the LLM context, since TTS text never
travels upstream) is passed along as the question being answered, so a
classifier can read "yeah" in context.

A transcript that arrives while Ghost is still talking is not confirmation
evidence: the operator has not heard the whole readback yet, and before echo
cancellation it is most likely Ghost's own voice. Such frames pass through
untouched and the proposal stays pending until a turn after the readback ends.
"""

from __future__ import annotations

from collections.abc import Callable

from ghost_core import Events, Verdict, WriteGate
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    LLMMessagesAppendFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug


class ConfirmationProcessor(FrameProcessor):
    def __init__(
        self,
        gate: WriteGate,
        session_id: str,
        *,
        last_assistant: Callable[[], str] | None = None,
        events: Events | None = None,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._gate = gate
        self._session_id = session_id
        self._bot_speaking = False
        self._last_assistant = last_assistant or (lambda: "")
        self._events = events or Events()

    @property
    def bot_speaking(self) -> bool:
        return self._bot_speaking

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._bot_speaking = False
        elif isinstance(frame, TranscriptionFrame) and self._gate.pending(self._session_id):
            debug.audio(f"confirmation: ignored {frame.text!r}, Ghost still speaking")
        if (
            isinstance(frame, TranscriptionFrame)
            and not self._bot_speaking
            and self._gate.pending(self._session_id)
        ):
            verdict, outcome = await self._gate.on_user_turn(
                self._session_id, frame.text, question=self._last_assistant()
            )
            debug.audio(f"confirmation: {verdict.name} for {frame.text!r}")
            if verdict is Verdict.AFFIRMATIVE and outcome is not None:
                logger.info(f"executed {outcome.proposal_id}: {outcome.status}")
                await self._events.emit("outcome", operation="confirmed", status=outcome.status)
                note = (
                    "[system] The operator said yes. Verified result: "
                    f"{outcome.summary()} Say what happened in a few words."
                )
            elif verdict is Verdict.NEGATIVE:
                note = "[system] The operator said no. Nothing was changed. Acknowledge briefly."
            else:
                note = (
                    "[system] The operator did not clearly say yes, so the pending change was "
                    "dropped. If they still want it, call change again."
                )
            await self.push_frame(
                LLMMessagesAppendFrame(messages=[{"role": "user", "content": note}], run_llm=False)
            )
        await self.push_frame(frame, direction)
