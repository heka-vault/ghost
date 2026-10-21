"""Hold (ADR 0021): the gate drops the room, resumes on the name at the start."""

from __future__ import annotations

import pytest
from ghost_voice.hold import HoldGate, addressed
from pipecat.frames.frames import (
    InputAudioRawFrame,
    TranscriptionFrame,
    TTSSpeakFrame,
    UserStartedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection


@pytest.mark.parametrize(
    "text,expect",
    [
        ("Ghost, we have a solution.", "we have a solution"),
        ("okay ghost, close seventeen", "close seventeen"),
        ("Alright, so, Ghost: what did we decide?", "what did we decide"),
        ("hey ghost", ""),
        ("Ghost?", ""),
        # the room talking about Ghost, not to it
        ("we should fix that in Ghost", None),
        ("I think Ghost could do that part", None),
        ("let's move on", None),
        ("", None),
    ],
)
def test_addressed_at_the_start_only(text, expect):
    # the bare name counts even when the wake phrases (the desk's defaults) leave it out
    assert addressed(text, ("hey ghost", "okay ghost")) == expect


@pytest.mark.parametrize(
    "text,expect",
    [
        ("he goes", ""),  # Parakeet's "hey ghost"
        ("he goes, you there", "you there"),
        ("hey guys", ""),
        ("guys, let's move on", None),  # a near-miss of the bare name never counts
        ("so he goes to the store", None),  # a near-miss only as a short call
    ],
)
def test_addressed_fuzzy_like_the_wake_detector(text, expect):
    assert addressed(text, ("hey ghost", "okay ghost"), fuzzy=0.8) == expect


async def make():
    resumed: list[str] = []

    async def on_resume(remainder: str) -> None:
        resumed.append(remainder)

    gate = HoldGate(on_resume=on_resume, phrases=("ghost",))
    pushed: list = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    gate.push_frame = push_frame  # type: ignore[method-assign]
    return gate, pushed, resumed


async def test_off_hold_everything_passes():
    gate, pushed, resumed = await make()
    await gate.process_frame(TranscriptionFrame("close it", "u", "t"), FrameDirection.DOWNSTREAM)
    assert type(pushed[-1]) is TranscriptionFrame and resumed == []


async def test_on_hold_the_room_is_dropped_and_the_name_resumes():
    gate, pushed, resumed = await make()
    gate.hold()
    before = len(pushed)
    await gate.process_frame(UserStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await gate.process_frame(
        TranscriptionFrame("what about the ghost repo", "u", "t"), FrameDirection.DOWNSTREAM
    )
    assert len(pushed) == before and resumed == []
    # audio still flows: the aggregator's VAD below needs it to segment speech for STT
    await gate.process_frame(
        InputAudioRawFrame(audio=b"\0\0", sample_rate=16000, num_channels=1),
        FrameDirection.DOWNSTREAM,
    )
    assert isinstance(pushed[-1], InputAudioRawFrame)
    # Ghost's own frames going down still pass (a "take your time" on its way to TTS)
    await gate.process_frame(TTSSpeakFrame("Take your time."), FrameDirection.DOWNSTREAM)
    assert isinstance(pushed[-1], TTSSpeakFrame)
    await gate.process_frame(
        TranscriptionFrame("okay Ghost, we picked option two", "u", "t"), FrameDirection.DOWNSTREAM
    )
    assert resumed == ["we picked option two"] and not gate.on_hold
    # the resuming frame itself is not forwarded: the resume injects the words
    assert not isinstance(pushed[-1], TranscriptionFrame)


async def test_session_end_runs_after_run_however_it_ended():
    """build_session hangs the hold timer's cancel on after_run; it must run when the
    runner returns on its own (the desktop's End) and when it fails, not only on hang_up."""
    from ghost_voice.pipeline import GhostSession

    class Runner:
        def __init__(self, fail: bool) -> None:
            self.fail = fail

        async def run(self) -> None:
            if self.fail:
                raise RuntimeError("pipeline died")

    calls = []
    for fail in (False, True):
        s = GhostSession("s", worker=None, runner=Runner(fail), _ended=None)  # type: ignore[arg-type]
        s.after_run = lambda: calls.append("stopped")
        if fail:
            with pytest.raises(RuntimeError):
                await s.run()
        else:
            await s.run()
    assert calls == ["stopped", "stopped"]
