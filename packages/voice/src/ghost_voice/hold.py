"""Hold (ADR 0021): "hang on, Ghost" keeps the session while the operator talks to someone
else. While on hold the gate below drops everything the microphone hears, on the spot,
without logging it, storing it or showing it to the model; the only thing it looks for is
the operator addressing Ghost by name at the start of an utterance ("okay Ghost, we have a
solution"), which resumes the session with that utterance as the next turn.

The rule is the desk's passive rule applied inside a session: local STT runs so the wake
phrase can be heard, nothing else leaves the machine. The session's context, the thread
and any question Ghost had asked all survive the hold.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable

from pipecat.frames.frames import (
    Frame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
    UserStartedSpeakingFrame,
    UserStoppedSpeakingFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug
from ghost_voice.wake import DEFAULT_WAKE_PHRASES, _words, match_wake

# What may come before the name and still count as addressing Ghost: "okay Ghost",
# "alright, so, Ghost", "um, hey Ghost". Anything else before it ("we should fix that in
# Ghost") is the room talking about Ghost, not to it.
LEAD_INS = frozenset(
    {"okay", "ok", "alright", "all", "right", "hey", "so", "um", "uh", "oh", "and", "well", "now"}
)

OnResume = Callable[[str], Awaitable[None]]


# Addressing Ghost on hold is "Ghost, ..." first of all: the bare name, which the passive
# wake phrases leave out on purpose (too easy to wake by), is right here, because Ghost is
# waiting to be called and only the start of an utterance counts.
BARE_NAME = ("ghost",)


def addressed(
    text: str,
    phrases: tuple[str, ...] | list[str] = DEFAULT_WAKE_PHRASES,
    *,
    fuzzy: float = 0.0,
) -> str | None:
    """The rest of ``text`` when it starts by addressing Ghost, else None. "" when the
    name was all that was said ("Ghost?").

    Exact first, on the wake phrases and the bare name. Then, with ``fuzzy`` > 0, the
    phonetic near-miss the desk's wake detector also accepts ("he goes" for "hey ghost"),
    on the multi-word phrases only and only as a short call (the near-miss and at most two
    words after it): a near-miss of the bare name ("guys") or "so he goes to the store"
    would otherwise resume Ghost from across the room."""
    words = _words(text)
    if not words:
        return None
    m = match_wake(text, (*BARE_NAME, *phrases))
    if m is None and fuzzy > 0:
        m = match_wake(text, [p for p in phrases if len(p.split()) > 1], fuzzy=fuzzy)
        if m is not None and len(_words(m.remainder)) > 2:
            m = None
    if m is None:
        return None
    # Where the phrase sits: everything before it must be a lead-in ("okay", "so").
    tail = len(_words(m.remainder))
    start = len(words) - tail - len(m.phrase.split())
    if start < 0 or not all(w in LEAD_INS for w in words[:start]):
        return None
    return m.remainder


# Audio is not dropped: the VAD that segments speech for STT runs in the user aggregator,
# downstream of here, and pushes its speaking frames back up to STT. Starving it of audio
# silenced STT on hold, and nothing could bring Ghost back (2026-10-20).
_DROPPED = (
    InterimTranscriptionFrame,
    TranscriptionFrame,
    UserStartedSpeakingFrame,
    UserStoppedSpeakingFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)


class HoldGate(FrameProcessor):
    """Right after STT. Off hold it passes everything. On hold it drops what the mic
    hears, except an utterance addressed to Ghost, which ends the hold through
    ``on_resume(remainder)`` (the frame itself is dropped; the resume injects the words)."""

    def __init__(
        self,
        *,
        on_resume: OnResume,
        phrases: tuple[str, ...] | list[str] = DEFAULT_WAKE_PHRASES,
        fuzzy: float = 0.0,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._on_resume = on_resume
        self._phrases = tuple(phrases)
        self._fuzzy = fuzzy
        self.on_hold = False

    def hold(self) -> None:
        self.on_hold = True

    def release(self) -> None:
        self.on_hold = False

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if not self.on_hold or direction != FrameDirection.DOWNSTREAM:
            await self.push_frame(frame, direction)
            return
        if isinstance(frame, TranscriptionFrame):
            remainder = addressed(frame.text, self._phrases, fuzzy=self._fuzzy)
            if remainder is None:
                debug.audio(f"hold: dropped {frame.text!r}")
                return
            self.on_hold = False
            await self._on_resume(remainder)
            return
        if isinstance(frame, _DROPPED):
            return
        await self.push_frame(frame, direction)
