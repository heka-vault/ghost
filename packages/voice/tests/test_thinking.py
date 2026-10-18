from __future__ import annotations

import asyncio

import pytest
from ghost_voice.sounds import thinking_loop
from ghost_voice.thinking import ThinkingSoundCut, ThinkingSoundProcessor, ThinkingStartFrame
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    FunctionCallInProgressFrame,
    FunctionCallResultFrame,
    LLMFullResponseEndFrame,
    MixerEnableFrame,
    UserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection


def processor(delay=0.05):
    proc = ThinkingSoundProcessor(delay_secs=delay)
    pushed: list = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = push_frame  # type: ignore[method-assign]
    proc.create_task = lambda coro, *a, **k: asyncio.ensure_future(coro)  # type: ignore[method-assign]
    return proc, pushed


def mixer_frames(pushed):
    return [f.enable for f in pushed if isinstance(f, MixerEnableFrame)]


@pytest.mark.asyncio
async def test_sound_starts_after_delay_and_stops_when_bot_speaks():
    proc, pushed = processor()
    await proc.process_frame(UserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    assert mixer_frames(pushed) == []
    await asyncio.sleep(0.1)
    assert mixer_frames(pushed) == [True]
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.UPSTREAM)
    assert mixer_frames(pushed) == [True, False]


@pytest.mark.asyncio
async def test_fast_reply_never_starts_sound():
    proc, pushed = processor(delay=0.2)
    await proc.process_frame(UserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.UPSTREAM)
    await asyncio.sleep(0.3)
    assert mixer_frames(pushed) == []


@pytest.mark.asyncio
async def test_silent_llm_turn_stops_sound():
    proc, pushed = processor()
    await proc.process_frame(UserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await asyncio.sleep(0.1)
    await proc.process_frame(LLMFullResponseEndFrame(), FrameDirection.DOWNSTREAM)
    assert mixer_frames(pushed) == [True, False]


def test_loop_is_quiet_and_seamless():
    a = thinking_loop(24000)
    assert len(a) == int(24000 * 1.6)
    assert abs(int(a[0])) < 50 and abs(int(a[-1])) < 50  # starts and ends near silence
    assert 2000 < int(abs(a).max()) < 20000


@pytest.mark.asyncio
async def test_injected_turn_also_arms_sound_and_frame_is_swallowed():
    proc, pushed = processor()
    await proc.process_frame(ThinkingStartFrame(), FrameDirection.DOWNSTREAM)
    await asyncio.sleep(0.1)
    assert mixer_frames(pushed) == [True]
    assert not any(isinstance(f, ThinkingStartFrame) for f in pushed)


@pytest.mark.asyncio
async def test_tool_call_keeps_sound_through_response_end():
    proc, pushed = processor()
    await proc.process_frame(UserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await asyncio.sleep(0.1)
    call = FunctionCallInProgressFrame(
        function_name="f", tool_call_id="t1", arguments={}, cancel_on_interruption=False
    )
    await proc.process_frame(call, FrameDirection.DOWNSTREAM)
    await proc.process_frame(LLMFullResponseEndFrame(), FrameDirection.DOWNSTREAM)
    assert mixer_frames(pushed) == [True]  # still on: a tool is running
    result = FunctionCallResultFrame(function_name="f", tool_call_id="t1", arguments={}, result={})
    await proc.process_frame(result, FrameDirection.DOWNSTREAM)
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.UPSTREAM)
    assert mixer_frames(pushed) == [True, False]


@pytest.mark.asyncio
async def test_sound_resumes_after_interim_sentence_while_tool_runs():
    """'Let me check' is spoken while the tool runs; the tone pauses for it and resumes."""
    proc, pushed = processor()
    call = FunctionCallInProgressFrame(
        function_name="f", tool_call_id="t1", arguments={}, cancel_on_interruption=False
    )
    await proc.process_frame(UserStoppedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await asyncio.sleep(0.1)
    await proc.process_frame(call, FrameDirection.DOWNSTREAM)
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.UPSTREAM)
    assert mixer_frames(pushed) == [True, False]
    await proc.process_frame(BotStoppedSpeakingFrame(), FrameDirection.UPSTREAM)
    await asyncio.sleep(0.1)
    assert mixer_frames(pushed) == [True, False, True]


@pytest.mark.asyncio
async def test_cut_switches_mixer_off_as_speech_starts():
    """The cut sits before the transport: TTSStartedFrame is immediately preceded by a
    mixer-off, so no queued speech plays over the loop."""
    from pipecat.frames.frames import TTSStartedFrame, TTSStoppedFrame

    cut = ThinkingSoundCut()
    pushed: list = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    cut.push_frame = push_frame  # type: ignore[method-assign]
    await cut.process_frame(TTSStartedFrame(), FrameDirection.DOWNSTREAM)
    await cut.process_frame(TTSStoppedFrame(), FrameDirection.DOWNSTREAM)
    kinds = [
        type(f).__name__ + (f"({f.enable})" if isinstance(f, MixerEnableFrame) else "")
        for f in pushed
    ]
    assert kinds == ["MixerEnableFrame(False)", "TTSStartedFrame", "TTSStoppedFrame"]
