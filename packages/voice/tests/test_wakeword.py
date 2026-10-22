"""The trained detector's passive path (ADR 0025): when a hit wakes Ghost, when Parakeet's
check drops it, and what becomes the first turn.

No model is loaded: a fake scorer fires on chosen frames and a fake recognizer stands in
for Parakeet. Positions are counted in samples, so the audio needn't play in real time.
"""

from __future__ import annotations

import pytest
from ghost_voice.wake import WakeMatch
from ghost_voice.wakeword import STEP, WINDOW, Detector, DetectorWakeProcessor
from pipecat.frames.frames import (
    InputAudioRawFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.tests.utils import run_test

RATE = 16000
FRAME = bytes(STEP * 2)  # 80 ms of silence; the fakes don't listen
FIRST_SCORE = WINDOW // STEP  # the frame whose end completes the first 2 s window


class Scorer:
    """Fires on the given 1-based score calls."""

    def __init__(self, hits: set[int], score: float = 0.9) -> None:
        self.hits = hits
        self.score_value = score
        self.calls = 0

    def score(self, window) -> float:
        self.calls += 1
        return self.score_value if self.calls in self.hits else 0.0


class Recognizer:
    def __init__(self, *texts: str) -> None:
        self.texts = list(texts)
        self.calls = 0

    def __call__(self, pcm) -> str:
        self.calls += 1
        return self.texts[min(self.calls, len(self.texts)) - 1]


def audio(secs: float) -> list:
    return [
        InputAudioRawFrame(audio=FRAME, sample_rate=RATE, num_channels=1)
        for _ in range(round(secs * RATE / STEP))
    ]


async def run(detector: Detector, frames: list) -> list[str]:
    """What happened, in order: "heard", then "match:<phrase>|<remainder>"."""
    events: list[str] = []

    async def on_heard() -> None:
        events.append("heard")

    async def on_match(m: WakeMatch) -> None:
        events.append(f"match:{m.phrase}|{m.remainder}")

    proc = DetectorWakeProcessor(detector, on_match, on_heard=on_heard, end_secs=0.3)
    await run_test(proc, frames_to_send=frames)
    return events


def detector(scorer: Scorer, recognize: Recognizer, *, verify: bool = True) -> Detector:
    return Detector(scorer=scorer, recognize=recognize, threshold=0.5, verify=verify)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_a_confirmed_hit_wakes_with_what_followed():
    scorer = Scorer({3})  # "ghost" ends about 2.2 s in
    recognize = Recognizer("Hey Ghost, what needs my sign-off?")
    events = await run(
        detector(scorer, recognize),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == ["heard", "match:hey ghost|what needs my sign off"]


@pytest.mark.asyncio
async def test_an_unconfirmed_hit_is_dropped():
    recognize = Recognizer("He goes to the gym every morning.")
    events = await run(
        detector(Scorer({3}), recognize),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(1.0)],
    )
    assert events == []
    assert recognize.calls == 1


@pytest.mark.asyncio
async def test_without_the_check_the_hit_alone_wakes_at_once():
    """The chime can play the moment the detector fires; the first turn still comes from
    the whole utterance, where a looser spelling finds the phrase's end."""
    recognize = Recognizer("He goes, what needs my sign-off?")
    heard_before_stop: list[bool] = []
    events: list[str] = []

    async def on_heard() -> None:
        heard_before_stop.append(recognize.calls == 0)
        events.append("heard")

    async def on_match(m: WakeMatch) -> None:
        events.append(f"match:{m.phrase}|{m.remainder}")

    proc = DetectorWakeProcessor(
        detector(Scorer({3}), recognize, verify=False), on_match, on_heard=on_heard, end_secs=0.3
    )
    await run_test(
        proc,
        frames_to_send=[
            VADUserStartedSpeakingFrame(),
            *audio(3.0),
            VADUserStoppedSpeakingFrame(),
            *audio(0.4),
        ],
    )
    assert events == ["heard", "match:hey ghost|what needs my sign off"]
    assert heard_before_stop == [True]
    assert recognize.calls == 1  # only to split off the first turn


@pytest.mark.asyncio
async def test_a_pause_after_the_phrase_doesnt_end_the_utterance():
    recognize = Recognizer("Hey Ghost.", "Hey Ghost, any new issues today?")
    events = await run(
        detector(Scorer({3}), recognize),
        [
            VADUserStartedSpeakingFrame(),
            *audio(2.4),
            VADUserStoppedSpeakingFrame(),  # the comma: checked here, and confirmed
            *audio(0.16),
            VADUserStartedSpeakingFrame(),  # ... and they go on
            *audio(1.0),
            VADUserStoppedSpeakingFrame(),
            *audio(0.4),
        ],
    )
    assert events == ["heard", "match:hey ghost|any new issues today"]
    assert recognize.calls == 2


@pytest.mark.asyncio
async def test_talking_on_is_checked_a_second_after_the_hit():
    recognize = Recognizer("Hey Ghost, read me", "Hey Ghost, read me the latest bug.")
    events = await run(
        detector(Scorer({3}), recognize),
        [VADUserStartedSpeakingFrame(), *audio(3.4)],  # no pause yet
    )
    assert events == ["heard"]
    assert recognize.calls == 1


@pytest.mark.asyncio
async def test_a_low_score_is_not_a_hit():
    recognize = Recognizer("Hey Ghost.")
    events = await run(
        Detector(scorer=Scorer({3}, score=0.2), recognize=recognize, threshold=0.5),  # type: ignore[arg-type]
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == []
    assert recognize.calls == 0  # Parakeet never ran


def test_the_listener_builds_the_detector_pipeline(monkeypatch):
    """Detector mode replaces the transcript of every utterance: no STT in the pipeline,
    and the session's short VAD stop so the check can run at the first pause."""
    import ghost_voice.wake as wake_mod
    from ghost_voice.config import VoiceSettings
    from ghost_voice.wake import WakeListener
    from pipecat.processors.frame_processor import FrameProcessor

    class Transport:
        def input(self):
            return FrameProcessor()

    seen = []
    real = wake_mod.SileroVADAnalyzer
    monkeypatch.setattr(
        wake_mod, "SileroVADAnalyzer", lambda params: seen.append(params) or real(params=params)
    )
    monkeypatch.setenv("ANTHROPIC_AUTH_TOKEN", "test")
    built_stt = []
    listener = WakeListener(
        VoiceSettings(),
        stt_factory=lambda: built_stt.append(1) or FrameProcessor(),
        phrases=("hey ghost",),
        transport_factory=Transport,
        detector=detector(Scorer(set()), Recognizer("")),
    )

    async def on_match(_m):
        pass

    worker = listener._build(on_match)  # noqa: SLF001
    outer = worker._pipeline.processors  # noqa: SLF001 - the worker wraps ours
    ours = next(p for p in outer if type(p).__name__ == "Pipeline")
    names = [type(p).__name__ for p in ours.processors]
    assert "DetectorWakeProcessor" in names
    assert built_stt == []
    assert seen[0].stop_secs == VoiceSettings().vad_stop_secs
