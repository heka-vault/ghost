"""Kokoro gets a clause at a time; NarrationCut still holds a short reply until it ends."""

from __future__ import annotations

import re

import numpy as np
from ghost_voice.clauses import ClauseTextAggregator
from ghost_voice.kokoro import KokoroTTS
from ghost_voice.narration import NarrationCut
from pipecat.frames.frames import (
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
)
from pipecat.pipeline.worker import PipelineParams
from pipecat.processors.frame_processor import FrameDirection
from pipecat.tests.utils import SleepFrame, run_test
from pipecat.utils.text.simple_text_aggregator import SimpleTextAggregator

LIST = (
    "Three need your sign-off: rotating the Discord webhook in infra, the dark mode toggle "
    "in app, and the Redis versus Postgres call for the session cache."
)


def tokens(text: str) -> list[str]:
    return re.findall(r"\S+\s*", text)  # roughly how a model streams


async def chunks(text: str, min_words: int = 4) -> list[str]:
    agg = ClauseTextAggregator(min_words=min_words)
    out = []
    for tok in tokens(text):
        async for a in agg.aggregate(tok):
            out.append(a.text)
    rest = await agg.flush()
    if rest and rest.text:
        out.append(rest.text)
    return out


async def test_a_list_answer_starts_at_its_first_clause():
    assert await chunks(LIST) == [
        "Three need your sign-off:",
        "rotating the Discord webhook in infra,",
        "the dark mode toggle in app,",
        "and the Redis versus Postgres call for the session cache.",
    ]


async def test_short_openers_numbers_and_times_are_not_split():
    assert await chunks("Sure, approved it.") == ["Sure, approved it."]
    assert await chunks("We have 3,000 users and the 10:30 build failed, so hold off.") == [
        "We have 3,000 users and the 10:30 build failed,",
        "so hold off.",
    ]
    assert await chunks("Done - closed infra seventeen, as you asked.") == [
        "Done - closed infra seventeen,",
        "as you asked.",
    ]


async def test_sentences_still_end_chunks_and_clauses_concatenate_back():
    text = "Closed it. Issue one forty two is next, and it blocks two others."
    out = await chunks(text)
    assert out == ["Closed it.", "Issue one forty two is next,", "and it blocks two others."]
    assert " ".join(out) == text


async def test_interruption_drops_the_pending_clause():
    agg = ClauseTextAggregator()
    async for _ in agg.aggregate("Three need your sign"):
        pass
    await agg.handle_interruption()
    assert await agg.flush() is None


class FakeKokoro:
    voices = ["af_heart"]

    def __init__(self) -> None:
        self.spoken: list[str] = []

    async def create_stream(self, text, *, voice, lang, speed):
        self.spoken.append(text)
        yield np.zeros(240, dtype=np.float32), 24000


def kokoro_tts(kokoro: FakeKokoro, clause_min_words: int | None) -> KokoroTTS:
    template = KokoroTTS(
        kokoro=kokoro,  # type: ignore[arg-type]
        settings=KokoroTTS.Settings(voice="af_heart"),
        clause_min_words=clause_min_words,
    )
    return template.fork()


async def say_through_tts(clause_min_words: int | None) -> list[str]:
    """A held reply released by NarrationCut at the end of the response, then TTS."""
    kokoro = FakeKokoro()
    cut = NarrationCut()
    released: list = []

    async def push(frame, direction=FrameDirection.DOWNSTREAM):
        released.append(frame)

    cut.push_frame = push  # type: ignore[method-assign]
    frames = [LLMFullResponseStartFrame(), *(LLMTextFrame(t) for t in tokens(LIST))]
    frames.append(LLMFullResponseEndFrame())
    for f in frames:
        await cut.process_frame(f, FrameDirection.DOWNSTREAM)
    held = [f for f in released if isinstance(f, LLMTextFrame)]
    assert "".join(f.text for f in held) == LIST  # the whole reply, released at its end
    await run_test(
        kokoro_tts(kokoro, clause_min_words),
        frames_to_send=[*released, SleepFrame(sleep=0.2)],
        pipeline_params=PipelineParams(audio_out_sample_rate=24000),
    )
    return kokoro.spoken


async def test_narration_cut_then_kokoro_clause_by_clause():
    assert await say_through_tts(4) == [
        "Three need your sign-off:",
        "rotating the Discord webhook in infra,",
        "the dark mode toggle in app,",
        "and the Redis versus Postgres call for the session cache.",
    ]


async def test_whole_sentences_when_clauses_are_off():
    assert await say_through_tts(None) == [LIST]


def test_fork_keeps_or_overrides_the_clause_setting():
    kokoro = FakeKokoro()
    template = KokoroTTS(
        kokoro=kokoro,  # type: ignore[arg-type]
        settings=KokoroTTS.Settings(voice="af_heart"),
        clause_min_words=4,
    )
    assert isinstance(template.fork()._text_aggregator, ClauseTextAggregator)  # noqa: SLF001
    plain = template.fork(clause_min_words=None)._text_aggregator  # noqa: SLF001
    assert type(plain) is SimpleTextAggregator
