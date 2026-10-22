"""Interim transcripts while Ghost speaks (issue #5): when they are made, that they never
overlap on the shared model, that only barge-in acts on them, and never the context.

No real model is loaded: a fake stands in for onnx-asr's recognizer.
"""

from __future__ import annotations

import threading
import time

import numpy as np
import pytest
from ghost_voice.confirmation import ConfirmationProcessor
from ghost_voice.echo import EchoFilter
from ghost_voice.hold import HoldGate
from ghost_voice.intercept import InterceptorProcessor
from ghost_voice.parakeet import ParakeetSTTService
from ghost_voice.tap import UserTap
from ghost_voice.turns import BargeInUserTurnStartStrategy
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    InputAudioRawFrame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
    UserStartedSpeakingFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.pipeline.worker import PipelineParams
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import (
    LLMContextAggregatorPair,
    LLMUserAggregatorParams,
)
from pipecat.processors.frame_processor import FrameDirection
from pipecat.tests.utils import SleepFrame, run_test
from pipecat.turns.user_stop import SpeechTimeoutUserTurnStopStrategy
from pipecat.turns.user_turn_strategies import UserTurnStrategies

RATE = 16000
CHUNK = bytes(int(RATE * 0.02) * 2)  # 20 ms of silence; the fake model doesn't listen


class FakeModel:
    """onnx-asr's ``recognize``, with a delay, counting calls that overlap."""

    def __init__(self, text: str = "stop that", delay: float = 0.0) -> None:
        self.text = text
        self.delay = delay
        self.calls = 0
        self.active = 0
        self.most_at_once = 0
        self._lock = threading.Lock()

    def recognize(self, pcm, sample_rate):
        with self._lock:
            self.calls += 1
            self.active += 1
            self.most_at_once = max(self.most_at_once, self.active)
        time.sleep(self.delay)
        with self._lock:
            self.active -= 1
        return self.text


def template(model: FakeModel) -> ParakeetSTTService:
    stt = ParakeetSTTService()
    stt._model = model  # noqa: SLF001 - what load() would set
    return stt


def audio(secs: float) -> list:
    """``secs`` of 20 ms frames at real-time pace."""
    out: list = []
    for _ in range(round(secs / 0.02)):
        out += [InputAudioRawFrame(audio=CHUNK, sample_rate=RATE, num_channels=1), SleepFrame(0.02)]
    return out


def vad_start():
    return VADUserStartedSpeakingFrame(start_secs=0.2)


def vad_stop():
    return VADUserStoppedSpeakingFrame(stop_secs=0.2)


async def run(stt: ParakeetSTTService, frames: list) -> list:
    down, _ = await run_test(
        stt,
        frames_to_send=[*frames, SleepFrame(0.2)],
        pipeline_params=PipelineParams(audio_in_sample_rate=RATE),
    )
    return down


def interims(down: list) -> list[str]:
    return [f.text for f in down if isinstance(f, InterimTranscriptionFrame)]


def finals(down: list) -> list[str]:
    return [f.text for f in down if isinstance(f, TranscriptionFrame)]


# --- when they are made ------------------------------------------------------------


async def test_interims_while_ghost_speaks_and_the_operator_is_mid_sentence():
    model = FakeModel()
    down = await run(
        template(model).fork(interim_secs=0.05),
        [BotStartedSpeakingFrame(), vad_start(), *audio(0.3), vad_stop()],
    )
    assert len(interims(down)) >= 3
    assert set(interims(down)) == {"stop that"}
    assert finals(down) == ["stop that"]  # the final still comes, once, at the pause


@pytest.mark.parametrize(
    "frames",
    [
        # Ghost is quiet: an ordinary turn, which waits for its final
        [vad_start(), *audio(0.3), vad_stop()],
        # Ghost speaks, the operator doesn't
        [BotStartedSpeakingFrame(), *audio(0.3)],
        # the operator's segment ended: nothing more until the next one opens
        [BotStartedSpeakingFrame(), vad_start(), vad_stop(), *audio(0.3)],
        # Ghost stopped (or was interrupted) while the operator goes on
        [BotStartedSpeakingFrame(), vad_start(), BotStoppedSpeakingFrame(), *audio(0.3)],
    ],
    ids=["ghost-quiet", "operator-quiet", "segment-closed", "ghost-stopped"],
)
async def test_no_interims_outside_ghost_speaking_over_an_open_segment(frames):
    model = FakeModel()
    down = await run(template(model).fork(interim_secs=0.05), frames)
    assert interims(down) == []
    assert model.calls == len(finals(down))  # and no pass was spent on one


@pytest.mark.parametrize(
    "fork",
    [lambda t: t.fork(), lambda t: t.fork(interim_secs=0.0)],
    ids=["wake-listener-fork", "zero"],
)
async def test_off_at_zero_and_for_the_wake_listener(fork):
    """ghostd builds the passive listener with ``stt_factory=models.stt.fork``: the
    default fork, which never makes an interim, whatever the frames say."""
    model = FakeModel()
    down = await run(fork(template(model)), [BotStartedSpeakingFrame(), vad_start(), *audio(0.3)])
    assert interims(down) == []
    assert model.calls == 0


# --- one recognition at a time -------------------------------------------------------


async def test_recognitions_never_overlap_and_busy_ticks_are_skipped():
    model = FakeModel(delay=0.1)  # slower than the interval
    down = await run(
        template(model).fork(interim_secs=0.03),
        [BotStartedSpeakingFrame(), vad_start(), *audio(0.5), vad_stop(), SleepFrame(0.2)],
    )
    assert model.most_at_once == 1
    assert 2 <= len(interims(down)) <= 6  # not one per 0.03 s tick
    assert finals(down) == ["stop that"]


@pytest.mark.parametrize("goes_on", [False, True], ids=["pause", "pause-and-go-on"])
async def test_an_interim_in_flight_at_the_pause_is_dropped_and_the_final_waits(goes_on):
    model = FakeModel(delay=0.2)
    # the first interim starts 0.1 s in and is still running when the segment ends; the
    # operator may already be in the next one when it returns, with the old one's words
    then = [vad_start(), *audio(0.1)] if goes_on else []
    down = await run(
        template(model).fork(interim_secs=0.1),
        [BotStartedSpeakingFrame(), vad_start(), *audio(0.14), vad_stop(), *then, SleepFrame(0.5)],
    )
    assert interims(down) == []  # stale: the final is the word on this segment
    assert finals(down) == ["stop that"]
    assert model.calls == 2 and model.most_at_once == 1


def test_forks_share_one_model_and_one_turn_at_it():
    model = FakeModel()
    t = template(model)
    session, wake = t.fork(interim_secs=0.3), t.fork()
    assert session._model is wake._model and session._busy is wake._busy  # noqa: SLF001
    pcm = np.zeros(1600, dtype=np.float32)
    with wake._busy:  # noqa: SLF001 - another pipeline's final is running
        assert session._recognize(pcm, wait=False) is None  # noqa: SLF001
    assert model.calls == 0
    assert session._recognize(pcm, wait=False) == "stop that"  # noqa: SLF001


# --- only barge-in reads them ----------------------------------------------------------


def interim(text: str) -> InterimTranscriptionFrame:
    return InterimTranscriptionFrame(text=text, user_id="op", timestamp="0")


def collect(proc):
    pushed: list = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = push_frame  # type: ignore[method-assign]
    return pushed


async def test_interims_reach_no_transcript_store_interceptor_or_confirmation():
    turns: list[tuple[str, str]] = []

    async def on_turn(role, text):
        turns.append((role, text))

    async def on_phase(_phase):
        pass

    class Interceptor:
        name = "always"
        seen: list[str] = []

        async def on_user_turn(self, turn):
            self.seen.append(turn.text)

    class Gate:
        turns: list[str] = []

        def pending(self, _session_id):
            return object()  # a change waits for a yes

        def cancel(self, _session_id, _reason=""):
            pass

        async def on_user_turn(self, _session_id, text, **_kw):
            self.turns.append(text)

    async def nothing():
        pass

    tap = UserTap(on_turn, on_phase)
    catcher = Interceptor()
    gate = Gate()
    intercept = InterceptorProcessor([catcher], gate=gate, session_id="s", on_hang_up=nothing)
    confirm = ConfirmationProcessor(gate, "s")  # type: ignore[arg-type]
    for proc in (tap, intercept, confirm):
        pushed = collect(proc)
        # Ghost is quiet: the moment a final would be taken as a turn or a yes
        await proc.process_frame(interim("yes close it"), FrameDirection.DOWNSTREAM)
        assert [f.text for f in pushed if isinstance(f, InterimTranscriptionFrame)] == [
            "yes close it"
        ]  # passed on, for the turn strategy below
    assert turns == [] and catcher.seen == [] and gate.turns == []


async def test_hold_drops_interims():
    async def resume(_rest):
        raise AssertionError("an interim never resumes a hold")

    hold = HoldGate(on_resume=resume)
    pushed = collect(hold)
    hold.hold()
    await hold.process_frame(interim("okay ghost we're back"), FrameDirection.DOWNSTREAM)
    assert pushed == []


async def test_an_interim_echo_of_two_of_ghosts_words_is_dropped():
    echo = EchoFilter()
    pushed = collect(echo)
    echo.spoke("I found three open issues in the app repository.")
    await echo.process_frame(BotStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    for frame in (interim("three issues"), interim("wait stop"), interim("stop the")):
        await echo.process_frame(frame, FrameDirection.DOWNSTREAM)
    final = TranscriptionFrame(text="three issues", user_id="op", timestamp="0")
    await echo.process_frame(final, FrameDirection.DOWNSTREAM)
    kept = [f.text for f in pushed if not isinstance(f, BotStartedSpeakingFrame)]
    # a two-word final is still a short answer, judged by the usual rule
    assert kept == ["wait stop", "stop the", "three issues"]


def user_aggregator():
    context = LLMContext()
    user, _assistant = LLMContextAggregatorPair(
        context,
        user_params=LLMUserAggregatorParams(
            user_turn_strategies=UserTurnStrategies(
                start=[BargeInUserTurnStartStrategy(min_words=2)],
                stop=[SpeechTimeoutUserTurnStopStrategy(user_speech_timeout=0.1)],
            ),
        ),
    )
    return user, context


@pytest.mark.parametrize("heard, cuts_in", [(["wait"], False), (["wait", "wait stop"], True)])
async def test_a_two_word_interim_while_ghost_speaks_cuts_it_off(heard, cuts_in):
    user, context = user_aggregator()
    frames: list = [BotStartedSpeakingFrame(), vad_start()]
    for text in heard:
        frames += [interim(text), SleepFrame(0.05)]
    down, up = await run_test(user, frames_to_send=[*frames, SleepFrame(0.2)])
    started = [f for f in [*down, *up] if isinstance(f, UserStartedSpeakingFrame)]
    assert bool(started) is cuts_in
    assert not any(isinstance(f, InterimTranscriptionFrame) for f in down)
    assert context.get_messages() == []  # no final yet: nothing for the model


async def test_only_the_final_becomes_context():
    user, context = user_aggregator()
    await run_test(
        user,
        frames_to_send=[
            BotStartedSpeakingFrame(),
            vad_start(),
            interim("wait stop"),
            SleepFrame(0.05),
            interim("wait stop that's"),
            BotStoppedSpeakingFrame(),
            vad_stop(),
            TranscriptionFrame(
                text="wait stop that's the wrong one", user_id="op", timestamp="0", finalized=True
            ),
            SleepFrame(0.5),
        ],
    )
    assert [m.get("content") for m in context.get_messages()] == ["wait stop that's the wrong one"]
