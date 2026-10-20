"""Don't speak what Ghost says while it reaches for a tool.

A response that calls a tool usually opens with narration ("Let me pull up the first one",
"I'll read the comments too"). The operator wants the answer, not the steps, and a prompt
rule alone didn't stop it. This sits right after the LLM: a response's text is held until
the response ends, then spoken; if the response calls a tool instead, the held text is
dropped, never spoken and never added to the context. The answer comes in the next
response, after the tool's result, and is spoken as usual.

A response that calls only ``hang_up`` or ``hold`` is the exception: its text is the goodbye
(or the "take your time")
("Sounds good, talk later!") and is spoken.

Once the session is going to sleep (``close()``), nothing later is spoken: a reply to
something said just before the goodbye ("thanks" -> "You're welcome") would otherwise come
after it and hold the call open. The model's own goodbye response is kept
(``keep_current=True``); the no-model shortcut cuts the current response too.

Holding costs the time to generate a reply (a sentence or two) before TTS starts. Past
``max_chars`` the text is released as it comes: a reply that long isn't a preamble.
"""

from __future__ import annotations

import random

from pipecat.frames.frames import (
    Frame,
    FunctionCallsStartedFrame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
    TTSSpeakFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug


class NarrationCut(FrameProcessor):
    SPOKEN_WITH = frozenset({"hang_up", "hold"})

    def __init__(self, *, max_chars: int = 240, **kwargs) -> None:
        super().__init__(**kwargs)
        self._max = max_chars
        self._held: list[LLMTextFrame] = []
        self._calling = False  # this response called a tool: drop its text
        self._released = False  # this response is long: pass its text through
        self._closed = False  # going to sleep: say nothing more
        self._closing = False  # say nothing after the current response
        self._saw_text = False  # this response produced words

    def close(self, *, keep_current: bool = False) -> None:
        if keep_current:
            self._closing = True
            return
        self._closed = True
        self._held.clear()

    def _reset(self) -> None:
        self._held.clear()
        self._calling = False
        self._released = False
        self._saw_text = False

    async def _release(self) -> None:
        held, self._held = self._held, []
        for f in held:
            await self.push_frame(f)

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if direction is not FrameDirection.DOWNSTREAM:
            await self.push_frame(frame, direction)
            return
        if isinstance(frame, LLMFullResponseStartFrame):
            self._reset()
            if self._closing:
                self._closed = True
        elif (
            isinstance(frame, FunctionCallsStartedFrame)
            and {c.function_name for c in frame.function_calls} <= self.SPOKEN_WITH
            and frame.function_calls
        ):
            self._released = True  # the goodbye: speak what was held and what follows
            await self._release()
            if not self._saw_text and {c.function_name for c in frame.function_calls} == {
                "hang_up"
            }:
                # The model hung up without a word (seen 2026-10-20: the call closed in
                # silence). The goodbye is Ghost's, not optional: say one of its own.
                from ghost_voice.intents import GOODBYES

                await self.push_frame(TTSSpeakFrame(random.choice(GOODBYES)), direction)
        elif isinstance(frame, FunctionCallsStartedFrame):
            if self._held:
                debug.audio(f"narration: dropped {''.join(f.text for f in self._held)!r}")
            self._held.clear()
            self._calling = True
        elif isinstance(frame, LLMTextFrame):
            self._saw_text = True
            if self._calling or self._closed:
                return
            if self._released:
                await self.push_frame(frame, direction)
                return
            self._held.append(frame)
            if sum(len(f.text) for f in self._held) > self._max:
                self._released = True
                await self._release()
            return
        elif isinstance(frame, LLMFullResponseEndFrame):
            if not self._calling:
                await self._release()
            self._reset()
        elif isinstance(frame, InterruptionFrame):
            self._reset()
        await self.push_frame(frame, direction)
