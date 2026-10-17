from __future__ import annotations

import pytest
from ghost_domain import Verdict
from ghost_voice.confirmation import ConfirmationProcessor
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    LLMMessagesAppendFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection


class FakeGate:
    """Always has a pending proposal; records what it was asked to judge."""

    def __init__(self) -> None:
        self.turns: list[str] = []

    def pending(self, session_id):
        return object()

    async def on_user_turn(self, session_id, text, now=None):
        self.turns.append(text)
        return Verdict.NEGATIVE, None


def processor():
    gate = FakeGate()
    proc = ConfirmationProcessor(gate, "s1")
    pushed = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = push_frame  # type: ignore[method-assign]
    return proc, gate, pushed


def said(text: str):
    return TranscriptionFrame(text=text, user_id="op", timestamp="0")


@pytest.mark.asyncio
async def test_transcript_while_bot_speaking_is_not_evidence():
    proc, gate, pushed = processor()
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("yes"), FrameDirection.DOWNSTREAM)
    assert gate.turns == []
    assert not any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    assert any(isinstance(f, TranscriptionFrame) for f in pushed)  # still passed downstream


@pytest.mark.asyncio
async def test_transcript_after_readback_reaches_gate():
    proc, gate, pushed = processor()
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(BotStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("no"), FrameDirection.DOWNSTREAM)
    assert gate.turns == ["no"]
    assert any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
