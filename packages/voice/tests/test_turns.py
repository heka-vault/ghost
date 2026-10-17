from __future__ import annotations

import pytest
from ghost_voice.turns import BargeInUserTurnStartStrategy
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
)
from pipecat.turns.types import ProcessFrameResult


def strategy(min_words: int = 2):
    s = BargeInUserTurnStartStrategy(min_words=min_words)
    events: list[str] = []

    @s.event_handler("on_user_turn_started")
    async def _started(_s, _params):
        events.append("started")

    @s.event_handler("on_reset_aggregation")
    async def _reset(_s):
        events.append("reset")

    return s, events


def vad_start():
    return VADUserStartedSpeakingFrame(start_secs=0.2, timestamp=0.0)


def said(text: str):
    return TranscriptionFrame(text=text, user_id="op", timestamp="0")


@pytest.mark.asyncio
async def test_vad_starts_turn_while_bot_silent():
    s, events = strategy()
    assert await s.process_frame(vad_start()) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_vad_ignored_while_bot_speaking():
    s, events = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(vad_start()) is ProcessFrameResult.CONTINUE
    assert events == []


@pytest.mark.asyncio
async def test_short_fragment_while_bot_speaking_is_dropped():
    s, events = strategy(min_words=2)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("the")) is ProcessFrameResult.CONTINUE
    assert events == ["reset"]


@pytest.mark.asyncio
async def test_enough_words_while_bot_speaking_interrupts():
    s, events = strategy(min_words=2)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("stop that")) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_zero_min_words_disables_barge_in():
    s, events = strategy(min_words=0)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("please stop talking now")) is ProcessFrameResult.CONTINUE
    assert events == ["reset"]


@pytest.mark.asyncio
async def test_single_word_starts_turn_once_bot_stopped():
    s, events = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    await s.process_frame(BotStoppedSpeakingFrame())
    assert await s.process_frame(said("yes")) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_turn_start_clears_bot_speaking():
    s, _ = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    await s.handle_user_turn_started()
    assert s.bot_speaking is False
