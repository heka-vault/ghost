"""The trained "hey ghost" detector, with Parakeet as its check (ADR 0025).

While passive in ``GHOST_WAKE_MODE=onnx`` the microphone goes to a small acoustic model
(``livekit-wakeword``, ONNX on onnxruntime) instead of to a transcript of every utterance.
Every 80 ms it scores the last 2 s. A score at or above the threshold is a hit, decided
the moment "ghost" ends:

- with the check on (``GHOST_WAKE_VERIFY``, the default), Parakeet transcribes from 2 s
  before the hit to the operator's first pause, or 1 s after the hit if they keep
  talking, and ``match_wake`` must find "hey ghost" in it (the same strict matcher
  transcript wake uses). A hit it doesn't confirm is dropped;
- with the check off the hit alone wakes Ghost.

Either way ``on_heard`` runs as soon as it is decided, so the chime can play while the
operator is still talking, and listening goes on until they finish: the words after the
phrase become the first turn, as with transcript wake. Only "hey ghost" wakes Ghost in
this mode; the other phrases need transcript wake.

Nothing leaves the machine, and as with transcript wake the transcripts are dropped here:
logged only under ``GHOST_DEBUG_AUDIO`` (``ghost_voice.debug``). The INFO log gets scores
and decisions, never words.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from loguru import logger
from pipecat.frames.frames import (
    Frame,
    InputAudioRawFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from ghost_voice import debug
from ghost_voice.wake import WakeMatch, match_wake

SAMPLE_RATE = 16000
STEP = 1280  # 80 ms: how often the detector scores
WINDOW = 32000  # 2 s: what each score covers
PRE_SECS = 2.0  # audio before the hit that the check and the first turn are read from
CHECK_SECS = 1.0  # the check runs this long after the hit at the latest
MAX_LISTEN_SECS = 12.0  # after a hit, the most Ghost waits for the operator to finish
# The detector already knew it heard "hey ghost"; finding where the phrase ends in the
# transcript, to split off the first turn, may take a looser spelling ("he goes").
REMAINDER_FUZZY = 0.8

DETECTOR_PHRASE = "hey ghost"
DEFAULT_MODEL_NAME = "hey_ghost.onnx"


class WakeWordScorer:
    """The detector, loaded once: ``score()`` takes the last 2 s and returns 0-1."""

    def __init__(self, model_path: Path, *, threads: int = 1) -> None:
        import onnxruntime as ort
        from livekit.wakeword import WakeWordModel

        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        self.path = model_path
        self._model = WakeWordModel(models=[model_path], sess_options=so)

    def score(self, window: np.ndarray) -> float:
        """``window``: 2 s of 16 kHz int16 PCM. Called off the event loop."""
        scores = self._model.predict(window)
        return max(scores.values(), default=0.0)


@dataclass(frozen=True)
class Detector:
    """What the passive listener needs to wake on the trained model."""

    scorer: WakeWordScorer
    recognize: Callable[[np.ndarray], str]  # float32 16 kHz -> text; Parakeet, off the loop
    threshold: float
    verify: bool = True
    cooldown_secs: float = 2.0
    phrase: str = DETECTOR_PHRASE


class DetectorWakeProcessor(FrameProcessor):
    """Last processor of the passive pipeline in detector mode, after the VAD.

    ``end_secs`` is the silence, beyond the VAD's own ``stop_secs``, that ends the
    operator's utterance once Ghost is woken: short pauses (a comma after "hey ghost")
    don't end it. Positions are counted in samples received, not wall time, so a slow
    check never shortens what is heard.
    """

    def __init__(
        self,
        detector: Detector,
        on_match: Callable[[WakeMatch], Awaitable[None]],
        *,
        on_heard: Callable[[], Awaitable[None]] | None = None,
        fuzzy: float = 0.9,
        end_secs: float = 0.3,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._d = detector
        self._on_match = on_match
        self._on_heard = on_heard
        self._fuzzy = fuzzy
        self._end = int(end_secs * SAMPLE_RATE)
        self._keep = int((PRE_SECS + MAX_LISTEN_SECS + 1.0) * SAMPLE_RATE)
        self._buf = np.zeros(0, dtype=np.int16)
        self._base = 0  # sample index of _buf[0]
        self._n = 0  # samples received
        self._scored = 0  # samples up to the last score
        self._speaking = False
        self._stopped_at: int | None = None  # where the VAD last said the operator stopped
        self._hit: int | None = None
        self._last_hit = -(10**12)
        self._woken = False  # decided: listening on for the rest of what they say
        self._done = False

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, InputAudioRawFrame):
            await self._audio(frame.audio)
            return  # passive audio ends here
        if isinstance(frame, VADUserStartedSpeakingFrame):
            self._speaking = True
            self._stopped_at = None
        elif isinstance(frame, VADUserStoppedSpeakingFrame):
            self._speaking = False
            self._stopped_at = self._n
            if self._hit is not None and not self._woken:
                await self._check()
        await self.push_frame(frame, direction)

    # --- audio ----------------------------------------------------------------------

    async def _audio(self, audio: bytes) -> None:
        if self._done:
            return
        pcm = np.frombuffer(audio, dtype=np.int16)
        self._buf = np.concatenate((self._buf, pcm))
        self._n += len(pcm)
        if len(self._buf) > self._keep:
            drop = len(self._buf) - self._keep
            self._buf = self._buf[drop:]
            self._base += drop
        while self._n - self._scored >= STEP and not self._done:
            self._scored += STEP
            if self._hit is None and self._scored >= WINDOW:
                await self._score(self._scored)
        if self._hit is None or self._done:
            return
        if not self._woken and self._n - self._hit >= CHECK_SECS * SAMPLE_RATE:
            await self._check()  # still talking a second after "ghost"
        elif self._woken and (
            self._finished() or self._n - self._hit >= MAX_LISTEN_SECS * SAMPLE_RATE
        ):
            await self._finish()

    def _finished(self) -> bool:
        return (
            not self._speaking
            and self._stopped_at is not None
            and self._n - self._stopped_at >= self._end
        )

    def _heard(self) -> np.ndarray:
        """From ``PRE_SECS`` before the hit to now, as float32 for Parakeet."""
        assert self._hit is not None
        start = max(0, self._hit - int(PRE_SECS * SAMPLE_RATE) - self._base)
        return self._buf[start:].astype(np.float32) / 32768.0

    # --- decisions ------------------------------------------------------------------

    async def _score(self, end: int) -> None:
        window = self._buf[end - WINDOW - self._base : end - self._base]
        if len(window) < WINDOW:
            return
        score = await asyncio.to_thread(self._d.scorer.score, window)
        if score < self._d.threshold or end - self._last_hit < self._d.cooldown_secs * SAMPLE_RATE:
            return
        self._hit = self._last_hit = end
        logger.info(f"wake: detector heard {self._d.phrase!r} ({score:.2f})")
        if not self._d.verify:
            await self._wake()

    async def _check(self) -> None:
        text = await asyncio.to_thread(self._d.recognize, self._heard())
        if match_wake(text, (self._d.phrase,), fuzzy=self._fuzzy):
            logger.info("wake: confirmed")
            await self._wake()
            if self._finished():
                await self._finish()
        else:
            # Transcripts are logged only under GHOST_DEBUG_AUDIO (ghost_voice.debug).
            debug.audio(f"passive: detector hit not confirmed: {text!r}")
            logger.info("wake: not confirmed")
            self._hit = None

    async def _wake(self) -> None:
        self._woken = True
        if self._on_heard:
            await self._on_heard()

    async def _finish(self) -> None:
        self._done = True
        text = await asyncio.to_thread(self._d.recognize, self._heard())
        m = match_wake(text, (self._d.phrase,), fuzzy=min(self._fuzzy, REMAINDER_FUZZY))
        debug.audio(f"passive: woken by the detector, heard {text!r}")
        await self._on_match(WakeMatch(phrase=self._d.phrase, remainder=m.remainder if m else ""))
