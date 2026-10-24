"""The trained detectors' passive path (ADR 0025): when a hit wakes Ghost, when Parakeet's
check drops it, and what becomes the first turn, for "hey ghost" and for "Ghost, …".

No model is loaded: a fake scorer fires on chosen frames and a fake recognizer stands in
for Parakeet. Positions are counted in samples, so the audio needn't play in real time.
"""

from __future__ import annotations

import pytest
from ghost_voice.wake import WakeMatch
from ghost_voice.wakeword import STEP, WINDOW, Detector, DetectorWakeProcessor, WakeModel
from pipecat.frames.frames import (
    InputAudioRawFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.tests.utils import run_test

RATE = 16000
FRAME = bytes(STEP * 2)  # 80 ms of silence; the fakes don't listen
FIRST_SCORE = WINDOW // STEP  # the frame whose end completes the first 2 s window


HEY = WakeModel(name="hey_ghost", phrase="hey ghost", threshold=0.5)
ADDRESS = WakeModel(name="ghost_address", phrase="ghost", threshold=0.5, opening=True)


class Scorer:
    """``model`` fires on the given 1-based score calls."""

    def __init__(self, hits: set[int], score: float = 0.9, model: str = "hey_ghost") -> None:
        self.hits = hits
        self.score_value = score
        self.model = model
        self.calls = 0

    def score(self, window) -> dict[str, float]:
        self.calls += 1
        return {self.model: self.score_value if self.calls in self.hits else 0.0}


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
    return Detector(scorer=scorer, recognize=recognize, models=(HEY, ADDRESS), verify=verify)


@pytest.mark.asyncio
async def test_a_confirmed_hit_wakes_with_what_followed():
    scorer = Scorer({3})  # "ghost" ends about 2.2 s in
    recognize = Recognizer("Hey Ghost, what needs my sign-off?")
    events = await run(
        detector(scorer, recognize),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == ["heard", "match:hey ghost|what needs my sign-off?"]


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
    assert events == ["heard", "match:hey ghost|what needs my sign-off?"]
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
    assert events == ["heard", "match:hey ghost|any new issues today?"]
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
        Detector(scorer=Scorer({3}, score=0.2), recognize=recognize, models=(HEY,)),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == []
    assert recognize.calls == 0  # Parakeet never ran


@pytest.mark.asyncio
async def test_addressing_ghost_by_name_wakes_with_the_whole_request():
    """ "Ghost, what is the status of …": the model fires as the opener ends, and the
    request starts after the name, not after the opener."""
    recognize = Recognizer("Ghost, what is the status of the infra repo?")
    events = await run(
        detector(Scorer({3}, model="ghost_address"), recognize),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == ["heard", "match:ghost|what is the status of the infra repo?"]


@pytest.mark.asyncio
async def test_the_name_inside_a_sentence_is_not_addressing_ghost():
    recognize = Recognizer("The ghost story we read last night was great.")
    events = await run(
        detector(Scorer({3}, model="ghost_address"), recognize),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(1.0)],
    )
    assert events == []


@pytest.mark.asyncio
async def test_without_the_check_a_misheard_name_keeps_the_request():
    """The model heard "Ghost, …" but Parakeet wrote "Coast": the request is all of it,
    rather than a greeting that drops what the operator asked."""
    recognize = Recognizer("Coast, let's work on the wake word.")
    events = await run(
        detector(Scorer({3}, model="ghost_address"), recognize, verify=False),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == ["heard", "match:ghost|Coast, let's work on the wake word."]


@pytest.mark.asyncio
async def test_each_model_answers_to_its_own_threshold():
    recognize = Recognizer("Ghost, I need you to look at the build.")
    events = await run(
        Detector(
            scorer=Scorer({3}, score=0.6, model="ghost_address"),
            recognize=recognize,
            models=(HEY, WakeModel("ghost_address", "ghost", threshold=0.7, opening=True)),
        ),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(0.4)],
    )
    assert events == []
    assert recognize.calls == 0


@pytest.mark.asyncio
async def test_a_model_can_be_checked_while_the_others_are_not():
    """ "hey ghost" wakes at once; a "Ghost, …" hit waits for Parakeet."""
    checked = WakeModel("ghost_address", "ghost", threshold=0.5, opening=True, verify=True)
    recognize = Recognizer("He goes to the gym every morning.")
    events = await run(
        Detector(
            scorer=Scorer({3}, model="ghost_address"),
            recognize=recognize,
            models=(HEY, checked),
            verify=False,
        ),
        [VADUserStartedSpeakingFrame(), *audio(2.6), VADUserStoppedSpeakingFrame(), *audio(1.0)],
    )
    assert events == []  # checked, and turned down
    assert recognize.calls == 1


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
