"""What Ghost says while reaching for a tool is not spoken; answers are."""

from __future__ import annotations

from ghost_voice.narration import NarrationCut
from pipecat.frames.frames import (
    FunctionCallsStartedFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
)
from pipecat.processors.frame_processor import FrameDirection


def cut(max_chars: int = 240):
    c = NarrationCut(max_chars=max_chars)
    out: list = []

    async def push(frame, direction=FrameDirection.DOWNSTREAM):
        out.append(frame)

    c.push_frame = push  # type: ignore[method-assign]
    return c, out


async def feed(c, *frames):
    for f in frames:
        await c.process_frame(f, FrameDirection.DOWNSTREAM)


def texts(out):
    return "".join(f.text for f in out if isinstance(f, LLMTextFrame))


async def test_narration_before_a_tool_call_is_dropped():
    c, out = cut()
    await feed(
        c,
        LLMFullResponseStartFrame(),
        LLMTextFrame("Let me pull up "),
        LLMTextFrame("the first one."),
        FunctionCallsStartedFrame(function_calls=[]),
        LLMTextFrame(" (late text from the same response)"),
        LLMFullResponseEndFrame(),
    )
    assert texts(out) == ""
    assert any(isinstance(f, FunctionCallsStartedFrame) for f in out)
    assert isinstance(out[-1], LLMFullResponseEndFrame)


async def test_an_answer_is_spoken_whole_at_the_end():
    c, out = cut()
    await feed(c, LLMFullResponseStartFrame(), LLMTextFrame("Two are "), LLMTextFrame("waiting."))
    assert texts(out) == ""  # held until the response ends
    await feed(c, LLMFullResponseEndFrame())
    assert texts(out) == "Two are waiting."
    assert isinstance(out[-1], LLMFullResponseEndFrame)


async def test_a_long_reply_is_released_as_it_comes():
    c, out = cut(max_chars=10)
    await feed(
        c, LLMFullResponseStartFrame(), LLMTextFrame("Issue 142 is "), LLMTextFrame("the cache.")
    )
    assert texts(out) == "Issue 142 is the cache."


async def test_nothing_more_is_said_once_going_to_sleep():
    c, out = cut()
    c.close()
    await feed(
        c, LLMFullResponseStartFrame(), LLMTextFrame("You're welcome."), LLMFullResponseEndFrame()
    )
    assert texts(out) == ""


async def test_the_goodbye_with_hang_up_is_spoken_and_nothing_after():
    from pipecat.frames.frames import FunctionCallFromLLM

    c, out = cut()
    bye = FunctionCallFromLLM(function_name="hang_up", tool_call_id="t", arguments={}, context=None)
    await feed(c, LLMFullResponseStartFrame(), LLMTextFrame("Sounds good, talk later!"))
    c.close(keep_current=True)  # the tool ran
    await feed(c, FunctionCallsStartedFrame(function_calls=[bye]), LLMFullResponseEndFrame())
    assert texts(out) == "Sounds good, talk later!"
    await feed(
        c, LLMFullResponseStartFrame(), LLMTextFrame("You're welcome."), LLMFullResponseEndFrame()
    )
    assert texts(out) == "Sounds good, talk later!"
