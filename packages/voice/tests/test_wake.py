from __future__ import annotations

import asyncio

import pytest
from ghost_voice.wake import (
    DEFAULT_WAKE_PHRASES,
    WakeMatch,
    match_opening,
    match_wake,
    parse_phrases,
)

PHRASES = ("hey ghost", "ghost you there", "ghost are you there", "okay ghost")


@pytest.mark.parametrize(
    "text, phrase, remainder",
    [
        ("Hey Ghost.", "hey ghost", ""),
        ("hey, ghost!", "hey ghost", ""),
        ("Ghost, you there?", "ghost you there", ""),
        ("Ghost, are you there?", "ghost are you there", ""),
        ("Okay Ghost, what needs my sign-off?", "okay ghost", "what needs my sign off"),
        ("um hey ghost what's pending", "hey ghost", "what s pending"),
    ],
)
def test_matches(text, phrase, remainder):
    m = match_wake(text, PHRASES)
    assert m is not None
    assert m.phrase == phrase
    assert m.remainder == remainder


@pytest.mark.parametrize(
    "text",
    [
        "",
        "the ghost of christmas past",
        "hey, is the ghost build green",
        "you there ghost",
        "ghostly",
        "hey ghosts",
    ],
)
def test_non_matches(text):
    assert match_wake(text, PHRASES) is None


def test_longest_phrase_wins():
    m = match_wake("hey ghost", ("ghost", "hey ghost"))
    assert m is not None and m.phrase == "hey ghost"


def test_parse_phrases_defaults_and_normalises():
    assert parse_phrases(None) == DEFAULT_WAKE_PHRASES
    assert parse_phrases("  ,  ") == DEFAULT_WAKE_PHRASES
    assert parse_phrases("Hey Ghost, Ghost: you there?") == ("hey ghost", "ghost you there")


# --- listener lifecycle -----------------------------------------------------------


class _Transport:
    """Transport whose input is an inert processor; nothing touches a sound device."""

    def input(self):
        from pipecat.processors.frame_processor import FrameProcessor

        return FrameProcessor()

    def output(self):  # pragma: no cover - passive listener never calls it
        raise AssertionError("passive listener must not open an output")


def _listener(monkeypatch):
    from ghost_voice.config import VoiceSettings
    from ghost_voice.wake import WakeListener
    from pipecat.processors.frame_processor import FrameProcessor

    monkeypatch.setenv("ANTHROPIC_AUTH_TOKEN", "test")
    return WakeListener(
        VoiceSettings(),
        stt_factory=FrameProcessor,
        phrases=PHRASES,
        transport_factory=_Transport,
    )


def test_passive_worker_has_no_idle_timeout(monkeypatch):
    """Pipecat cancels an idle worker after five minutes by default; a wake listener is
    idle by definition and must sit for hours."""

    async def on_match(_m):
        pass

    worker = _listener(monkeypatch)._build(on_match)
    assert worker._idle_timeout_secs is None


def test_passive_vad_has_its_own_stop(monkeypatch):
    """The listener waits longer for the end of speech than a turn does, so a comma after
    "hey ghost" doesn't split the phrase from what follows; the session's VAD keeps its own."""
    import ghost_voice.wake as wake_mod
    from ghost_voice.config import VoiceSettings

    seen = []
    real = wake_mod.SileroVADAnalyzer
    monkeypatch.setattr(
        wake_mod, "SileroVADAnalyzer", lambda params: seen.append(params) or real(params=params)
    )
    monkeypatch.setenv("GHOST_WAKE_VAD_STOP_SECS", "0.7")

    async def on_match(_m):
        pass

    _listener(monkeypatch)._build(on_match)
    assert seen[0].stop_secs == 0.7
    assert VoiceSettings().vad_stop_secs == 0.2


async def test_listener_rebuilds_when_pipeline_ends_without_match(monkeypatch):
    """A worker that exits on its own (device vanished, processor failure) is replaced,
    so the daemon never reports "passive" with nobody reading the microphone."""
    import ghost_voice.wake as wake_mod
    from ghost_voice.wake import WakeMatch

    runs: list[int] = []
    callbacks: list = []

    class FakeRunner:
        def __init__(self, **_kw):
            self.cancelled = False

        async def add_workers(self, *_w):
            pass

        async def run(self):
            runs.append(len(runs))
            if len(runs) == 1:
                return  # first pipeline dies quietly, like the idle-timeout cancel did
            await callbacks[-1](WakeMatch(phrase="hey ghost", remainder="hi"))
            await asyncio.sleep(3600)  # a live pipeline blocks until cancelled

        async def cancel(self):
            self.cancelled = True

    listener = _listener(monkeypatch)
    monkeypatch.setattr(wake_mod, "WorkerRunner", FakeRunner)
    monkeypatch.setattr(listener, "_build", lambda cb: callbacks.append(cb) or object())

    async def no_sleep(_s):
        pass

    monkeypatch.setattr(asyncio, "sleep", no_sleep)

    m = await asyncio.wait_for(listener.wait(), 5)
    assert m.remainder == "hi"
    assert runs == [0, 1]
    assert listener._runner is None and listener._run_task is None


# --- phonetic near-misses ----------------------------------------------------------


@pytest.mark.parametrize(
    "heard, phrase, remainder",
    [
        ("Hey guys.", "hey ghost", ""),  # what Parakeet made of "hey ghost" through the AEC
        ("There he goes.", "hey ghost", ""),
        ("Hey ghosts.", "hey ghost", ""),
        ("He goes what a fear is.", "hey ghost", "what a fear is"),
        ("okay goes", "okay ghost", ""),
    ],
)
def test_fuzzy_matches_asr_spellings(heard, phrase, remainder):
    m = match_wake(heard, DEFAULT_WAKE_PHRASES, fuzzy=0.8)
    assert m is not None and (m.phrase, m.remainder) == (phrase, remainder)


@pytest.mark.parametrize(
    "heard",
    [
        "There you go.",
        "We go.",
        "No see you there.",
        "see you there",
        "hey there",
        "hey toast",
        "the ghost",
        "he gets it",
        "",
    ],
)
def test_fuzzy_rejects_unrelated_speech(heard):
    assert match_wake(heard, DEFAULT_WAKE_PHRASES, fuzzy=0.8) is None


@pytest.mark.parametrize(
    "heard",
    [
        # Each woke Ghost at 0.8 in the 2026-10-21 measurements: LibriSpeech, then the
        # room's confusable sentences.
        "He set off abruptly for the bull",
        "As she awaited her guests, she surveyed the table",
        "He sat down, weak, bewildered",
        "Here, they said is a rascal who has been harrying our coast",
        "Hey guys, what's up?",
        "He goes to the gym every morning.",
        "Hey Gus, how have you been?",
        "Okay, coast is clear.",
        "Hey goat cheese or cheddar.",
    ],
)
def test_strict_fuzzy_rejects_measured_false_wakes(heard):
    assert match_wake(heard, DEFAULT_WAKE_PHRASES, fuzzy=0.9) is None


@pytest.mark.parametrize("heard", ["He ghost.", "Hay ghost, what's new?", "Ok, ghost."])
def test_strict_fuzzy_keeps_same_sound_spellings(heard):
    assert match_wake(heard, DEFAULT_WAKE_PHRASES, fuzzy=0.9) is not None


def test_fuzzy_off_by_default():
    assert match_wake("Hey guys.", DEFAULT_WAKE_PHRASES) is None


def test_exact_match_wins_over_fuzzy():
    m = match_wake("hey ghost close it", DEFAULT_WAKE_PHRASES, fuzzy=0.8)
    assert m == WakeMatch(phrase="hey ghost", remainder="close it")


@pytest.mark.parametrize(
    ("heard", "asked"),
    [
        ("Ghost, what is the status of the infra repo?", "what is the status of the infra repo"),
        ("So, Ghost, let's work on the wake word.", "let s work on the wake word"),
        ("Ghost.", ""),
    ],
)
def test_addressing_ghost_opens_with_the_name(heard, asked):
    assert match_opening(heard, "ghost", fuzzy=0.9) == WakeMatch("ghost", asked)


@pytest.mark.parametrize(
    "heard",
    [
        "The ghost story was good.",  # the name inside a sentence
        "We should fix that bug in Ghost.",
        "Coast, I need you.",  # not the name's sound
        "",
    ],
)
def test_the_name_must_open_what_was_said(heard):
    assert match_opening(heard, "ghost", fuzzy=0.8) is None


def test_a_looser_opening_takes_the_names_near_spellings():
    """The check stays strict; splitting off the request may take "goes" for "ghost"."""
    assert match_opening("Goes, I need you.", "ghost", fuzzy=0.9) is None
    loose = match_opening("Goes, I need you.", "ghost", fuzzy=0.8)
    assert loose == WakeMatch("ghost", "i need you")
