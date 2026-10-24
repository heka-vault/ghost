"""The trained wake detectors, with Parakeet as their check (ADR 0025).

While passive in ``GHOST_WAKE_MODE=onnx`` the microphone goes to small acoustic models
(``livekit-wakeword``, ONNX on onnxruntime) instead of to a transcript of every utterance.
Every 80 ms they score the last 2 s, sharing one pass of the audio features. Each model
listens for one thing (``model_meaning``):

- ``hey_ghost``: "hey ghost". The words after it are the first turn.
- ``ghost_*`` (``ghost_address``, or one model per opener): the operator addressing Ghost
  by name with a request, trained on openers such as "Ghost, I need you", "Ghost, let's
  work on", "Ghost, what is the status of" and "Ghost, where did we leave off".
  Everything after "Ghost" is the first turn, so the request arrives whole: "what is the
  status of the infra repo".

A model's score at or above its threshold is a hit, decided the moment its phrase ends:

- with the check on (``GHOST_WAKE_VERIFY``, the default), Parakeet transcribes from just
  before the phrase to the operator's first pause, or 1 s after the hit if they keep
  talking, and the model's phrase must be in it: ``match_wake`` for "hey ghost" (the same
  strict matcher transcript wake uses), ``match_opening`` for "Ghost, …", which must
  open what was said. A hit it doesn't confirm is dropped;
- with the check off the hit alone wakes Ghost.

Either way ``on_heard`` runs as soon as it is decided, so the chime can play while the
operator is still talking, and listening goes on until they finish. Only what the
models were trained on wakes Ghost in this mode; the other phrases need transcript wake.

Nothing leaves the machine, and as with transcript wake the transcripts are dropped here:
logged only under ``GHOST_DEBUG_AUDIO`` (``ghost_voice.debug``). The INFO log gets scores
and decisions, never words.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

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
from ghost_voice.wake import WakeMatch, match_opening, match_wake

SAMPLE_RATE = 16000
STEP = 1280  # 80 ms: how often the detector scores
WINDOW = 32000  # 2 s: what each score covers
PRE_SECS = 2.0  # audio before the hit that the check and the first turn are read from
# An opener ("Ghost, what did we leave off on") can fill the detector's whole 2 s window;
# a little more makes sure the transcript has the name.
OPENING_PRE_SECS = 2.5
CHECK_SECS = 1.0  # the check runs this long after the hit at the latest
MAX_LISTEN_SECS = 12.0  # after a hit, the most Ghost waits for the operator to finish
# The detector already knew it heard "hey ghost"; finding where the phrase ends in the
# transcript, to split off the first turn, may take a looser spelling ("he goes").
REMAINDER_FUZZY = 0.8

DETECTOR_PHRASE = "hey ghost"
DEFAULT_MODEL_NAME = "hey_ghost.onnx"
ADDRESS_MODEL_GLOB = "ghost_*.onnx"  # Ghost addressed by name: one model, or one per opener


def model_meaning(stem: str) -> tuple[str, bool]:
    """A model file's stem (how livekit-wakeword names its score) -> the phrase the check
    and the first turn go by, and whether that phrase must open what was said."""
    if stem.startswith("ghost_"):
        return "ghost", True
    return DETECTOR_PHRASE, False


class Scorer(Protocol):
    def score(self, window: np.ndarray) -> dict[str, float]: ...


class WakeWordScorer:
    """The models, loaded once: ``score()`` takes the last 2 s and returns each model's
    score (0-1) by name. The audio features are computed once for all of them."""

    def __init__(self, model_paths: Sequence[Path], *, threads: int = 1) -> None:
        import onnxruntime as ort
        from livekit.wakeword import WakeWordModel

        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        self.paths = list(model_paths)
        self._model = WakeWordModel(models=self.paths, sess_options=so)

    def score(self, window: np.ndarray) -> dict[str, float]:
        """``window``: 2 s of 16 kHz int16 PCM. Called off the event loop."""
        return self._model.predict(window)


@dataclass(frozen=True)
class WakeModel:
    """One trained model and what its hit means."""

    name: str  # the model file's stem
    phrase: str
    threshold: float
    opening: bool = False  # the phrase opens what was said; the rest is the request
    verify: bool | None = None  # Parakeet checks this model's hits; None: the detector's rule

    @classmethod
    def known(cls, path: Path, threshold: float, *, verify: bool | None = None) -> WakeModel:
        phrase, opening = model_meaning(path.stem)
        return cls(
            name=path.stem, phrase=phrase, threshold=threshold, opening=opening, verify=verify
        )

    def match(self, text: str, fuzzy: float) -> WakeMatch | None:
        if self.opening:
            return match_opening(text, self.phrase, fuzzy=fuzzy)
        return match_wake(text, (self.phrase,), fuzzy=fuzzy)


@dataclass(frozen=True)
class Detector:
    """What the passive listener needs to wake on the trained models."""

    scorer: Scorer
    recognize: Callable[[np.ndarray], str]  # float32 16 kHz -> text; Parakeet, off the loop
    models: tuple[WakeModel, ...]
    verify: bool = True
    cooldown_secs: float = 2.0


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
        self._keep = int((OPENING_PRE_SECS + MAX_LISTEN_SECS + 1.0) * SAMPLE_RATE)
        self._buf = np.zeros(0, dtype=np.int16)
        self._base = 0  # sample index of _buf[0]
        self._n = 0  # samples received
        self._scored = 0  # samples up to the last score
        self._speaking = False
        self._stopped_at: int | None = None  # where the VAD last said the operator stopped
        self._hit: int | None = None
        self._model: WakeModel = detector.models[0]  # the model behind the hit
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
        """From just before the hit's phrase to now, as float32 for Parakeet."""
        assert self._hit is not None
        pre = OPENING_PRE_SECS if self._model.opening else PRE_SECS
        start = max(0, self._hit - int(pre * SAMPLE_RATE) - self._base)
        return self._buf[start:].astype(np.float32) / 32768.0

    # --- decisions ------------------------------------------------------------------

    async def _score(self, end: int) -> None:
        window = self._buf[end - WINDOW - self._base : end - self._base]
        if len(window) < WINDOW:
            return
        scores = await asyncio.to_thread(self._d.scorer.score, window)
        hits = [(scores.get(m.name, 0.0), m) for m in self._d.models]
        over = [(s, m) for s, m in hits if s >= m.threshold]
        score, model = max(over, key=lambda h: h[0], default=(0.0, None))
        if model is None or end - self._last_hit < self._d.cooldown_secs * SAMPLE_RATE:
            return
        self._hit = self._last_hit = end
        self._model = model
        logger.info(f"wake: detector heard {model.name} ({score:.2f})")
        if not (self._d.verify if model.verify is None else model.verify):
            await self._wake()

    async def _check(self) -> None:
        text = await asyncio.to_thread(self._d.recognize, self._heard())
        if self._model.match(text, self._fuzzy):
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
        model = self._model
        m = model.match(text, min(self._fuzzy, REMAINDER_FUZZY))
        debug.audio(f"passive: woken by {model.name}, heard {text!r}")
        if m:
            remainder = m.remainder
        elif model.opening:
            # The model heard "Ghost, <opener>" but the transcript lost the name ("Coast,
            # what is…"): the request is still all of it, which beats dropping it.
            remainder = text.strip()
        else:
            remainder = ""
        await self._on_match(WakeMatch(phrase=model.phrase, remainder=remainder))
