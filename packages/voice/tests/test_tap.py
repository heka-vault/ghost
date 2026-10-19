"""The phase the apps show follows the turn, not the transcript fragments."""

from __future__ import annotations

from ghost_voice.tap import UserTap
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    TranscriptionFrame,
    UserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection


def tap():
    turns: list[tuple[str, str]] = []
    phases: list[str] = []

    async def on_turn(role, text):
        turns.append((role, text))

    async def on_phase(phase):
        phases.append(phase)

    t = UserTap(on_turn, on_phase)

    async def push(frame, direction=FrameDirection.DOWNSTREAM):
        pass

    t.push_frame = push  # type: ignore[method-assign]
    return t, turns, phases


def said(text):
    return TranscriptionFrame(text=text, user_id="op", timestamp="0")


async def test_fragments_record_turns_but_thinking_waits_for_the_turn_end():
    t, turns, phases = tap()
    for f in (said("Yes, I'm going to do it."), said("Setting should follow."), said("The")):
        await t.process_frame(f, FrameDirection.DOWNSTREAM)
    assert [r for r, _ in turns] == ["user", "user", "user"]
    assert phases == []  # still the operator's turn
    await t.process_frame(UserStoppedSpeakingFrame(), FrameDirection.UPSTREAM)
    assert phases == ["thinking"]
    await t.process_frame(BotStartedSpeakingFrame(), FrameDirection.UPSTREAM)
    await t.process_frame(BotStoppedSpeakingFrame(), FrameDirection.UPSTREAM)
    assert phases == ["thinking", "speaking", "listening"]
