"""Drop transcripts that are Ghost hearing itself.

The barge-in rule (``turns.py``) only guards the time Ghost is speaking. On the phone the
echo comes back late: Ghost's audio crosses the network, plays on the speakerphone, leaks
past the phone's echo canceller, and crosses back, so its transcript lands after
``BotStoppedSpeaking`` and reads as a new user turn ("Hey, what can I do?" right after
the greeting). This filter sits right after STT. A final transcript that arrives while
Ghost is speaking, or within ``window_secs`` after, and whose words are mostly a run of
what Ghost just said, is dropped before anything else sees it.

Short answers are never echo: a transcript under ``min_words`` always passes, so “yes” or
“close it” after “Close it?” still reach the confirmation path. An interim transcript
(``parakeet.py``, only while Ghost speaks) serves nothing but barge-in, so two of Ghost's
own words are enough to drop one; the final that follows is judged as above.
"""

from __future__ import annotations

import re
import time
from collections import deque
from difflib import SequenceMatcher

from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    Frame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug

_WORD = re.compile(r"[a-z0-9']+")
INTERIM_MIN_WORDS = 2  # an interim this long cuts Ghost off (GHOST_BARGE_IN_MIN_WORDS default)


def words(text: str) -> list[str]:
    return _WORD.findall(text.lower())


def echo_share(heard: str, spoken: str) -> float:
    """How much of ``heard`` is a run of ``spoken``, word by word, from 0 to 1."""
    h, s = words(heard), words(spoken)
    if not h or not s:
        return 0.0
    m = SequenceMatcher(a=h, b=s, autojunk=False)
    return sum(b.size for b in m.get_matching_blocks()) / len(h)


class EchoFilter(FrameProcessor):
    def __init__(
        self,
        *,
        window_secs: float = 3.0,
        min_words: int = 3,
        threshold: float = 0.8,
        clock=time.monotonic,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._window = window_secs
        self._min_words = min_words
        self._threshold = threshold
        self._clock = clock
        self._spoken: deque[str] = deque(maxlen=3)
        self._speaking = False
        self._stopped_at = float("-inf")

    def spoke(self, text: str) -> None:
        """What Ghost said, as the assistant tap records it (the greeting included)."""
        if text.strip():
            self._spoken.append(text)

    def is_echo(self, text: str, *, min_words: int | None = None) -> bool:
        least = self._min_words if min_words is None else min_words
        if not self._spoken or len(words(text)) < least:
            return False
        recent = self._speaking or self._clock() - self._stopped_at <= self._window
        if not recent:
            return False
        return max(echo_share(text, s) for s in self._spoken) >= self._threshold

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, BotStartedSpeakingFrame):
            self._speaking = True
        elif isinstance(frame, BotStoppedSpeakingFrame):
            self._speaking = False
            self._stopped_at = self._clock()
        elif (isinstance(frame, TranscriptionFrame) and self.is_echo(frame.text)) or (
            isinstance(frame, InterimTranscriptionFrame)
            and self.is_echo(frame.text, min_words=INTERIM_MIN_WORDS)
        ):
            debug.audio(f"echo: dropped {frame.text!r}")
            return
        await self.push_frame(frame, direction)
