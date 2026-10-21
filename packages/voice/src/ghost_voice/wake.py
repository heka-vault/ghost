"""Wake by transcript: a local STT pass over passive audio, looking for a phrase.

Interim wake path until the trained ONNX wake model exists (ADR 0010). While
passive, the microphone goes through VAD and Parakeet on this machine only.
Transcripts that do not contain a wake phrase are dropped on the spot: not
logged, not stored, not sent anywhere. A match returns the words that followed
the phrase, so "hey ghost, what needs my sign-off" becomes the first turn.
"""

from __future__ import annotations

import asyncio
import re
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from difflib import SequenceMatcher

from loguru import logger
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.audio.vad.vad_analyzer import VADParams
from pipecat.frames.frames import (
    Frame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineParams, PipelineWorker, ProcessorUnusablePolicy
from pipecat.processors.audio.vad_processor import VADProcessor
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor
from pipecat.transports.base_transport import BaseTransport
from pipecat.workers.runner import WorkerRunner

from ghost_voice import debug
from ghost_voice.config import VoiceSettings
from ghost_voice.transport import SoundDeviceTransport, SoundDeviceTransportParams

DEFAULT_WAKE_PHRASES = (
    "hey ghost",
    "yo ghost",
    "okay ghost",
    "ok ghost",
    "ghost you there",
    "ghost are you there",
    "ghost wake up",
)


@dataclass(frozen=True)
class WakeMatch:
    phrase: str
    remainder: str  # what was said after the phrase, "" if nothing


def _words(text: str) -> list[str]:
    return re.sub(r"[^\w\s]", " ", text.lower()).split()


def parse_phrases(spec: str | None) -> tuple[str, ...]:
    """Comma-separated phrases from the environment, normalised; empty -> defaults."""
    if not spec or not spec.strip():
        return DEFAULT_WAKE_PHRASES
    out = tuple(" ".join(_words(p)) for p in spec.split(",") if _words(p))
    return out or DEFAULT_WAKE_PHRASES


_PHONETIC_SUBS = (
    ("ph", "f"),
    ("gh", "g"),
    ("ck", "k"),
    ("wh", "w"),
    ("th", "t"),
    ("dg", "j"),
    ("c", "k"),
    ("q", "k"),
    ("x", "ks"),
    ("z", "s"),
)


def _phonetic(word: str) -> str:
    """A crude phonetic key: consonant skeleton after the first letter, common English
    confusions folded. "ghost" -> "gst", "goes" and "guys" -> "gs", "hey" and "he" -> "h"."""
    w = word.lower()
    for a, b in _PHONETIC_SUBS:
        w = w.replace(a, b)
    if not w:
        return ""
    out = w[0]
    for ch in w[1:]:
        if ch in "aeiouyh" or ch == out[-1]:
            continue
        out += ch
    return out


def _similar(a: str, b: str) -> float:
    return SequenceMatcher(None, a, b).ratio()


def match_wake(
    text: str, phrases: tuple[str, ...] | list[str], *, fuzzy: float = 0.0
) -> WakeMatch | None:
    """Find the first wake phrase as a contiguous word sequence in ``text``.

    Case and punctuation are ignored, so "Ghost, you there?" matches
    "ghost you there". Longer phrases are tried first so "hey ghost" wins over
    a plain "ghost" if both are configured.

    With ``fuzzy`` > 0, a window of the same word count whose phonetic key is at
    least that similar to the phrase's also matches. 0 disables it. 0.9 (ghostd's
    default) takes spellings with the phrase's own sounds, "he ghost" for "hey ghost";
    0.8 also takes "hey guys" and "he goes", which a webcam mic needed, but they are
    ordinary speech too: 2.1 false wakes an hour of read speech, none at 0.9
    (docs/benchmarks/2026-10-21-wake.md).
    """
    words = _words(text)
    if not words:
        return None
    ordered = sorted(phrases, key=lambda p: -len(p.split()))
    for phrase in ordered:
        pw = phrase.split()
        n = len(pw)
        for i in range(len(words) - n + 1):
            if words[i : i + n] == pw:
                return WakeMatch(phrase=phrase, remainder=" ".join(words[i + n :]))
    if fuzzy <= 0:
        return None
    best: tuple[float, WakeMatch] | None = None
    for phrase in ordered:
        pw = phrase.split()
        n = len(pw)
        keys = [_phonetic(w) for w in pw]
        key = "".join(keys)
        anchor = max(keys, key=len)  # the distinctive word ("ghost"), which must be present
        for i in range(len(words) - n + 1):
            window = [_phonetic(w) for w in words[i : i + n]]
            if max(_similar(w, anchor) for w in window) < 0.66:
                continue  # "see you there" is not "ghost you there"
            score = _similar("".join(window), key)
            if score >= fuzzy and (best is None or score > best[0]):
                best = (score, WakeMatch(phrase=phrase, remainder=" ".join(words[i + n :])))
    if best:
        debug.audio(f"passive: fuzzy wake {best[0]:.2f} for {text!r}")
        return best[1]
    return None


class WakeMatchProcessor(FrameProcessor):
    """Last processor of the passive pipeline. Transcripts end here."""

    def __init__(
        self,
        phrases: tuple[str, ...],
        on_match: Callable[[WakeMatch], Awaitable[None]],
        *,
        fuzzy: float = 0.0,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._phrases = phrases
        self._on_match = on_match
        self._fuzzy = fuzzy

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TranscriptionFrame):
            m = match_wake(frame.text, self._phrases, fuzzy=self._fuzzy)
            if m:
                await self._on_match(m)
            else:
                debug.audio(f"passive: no wake phrase in {frame.text!r}")
            return  # transcripts are never forwarded or kept
        if isinstance(frame, VADUserStartedSpeakingFrame):
            debug.audio("passive: speech started")
        elif isinstance(frame, VADUserStoppedSpeakingFrame):
            debug.audio("passive: speech stopped, transcribing")
        await self.push_frame(frame, direction)


class WakeListener:
    """Runs a fresh passive pipeline each time ``wait()`` is called, until a phrase matches.

    Everything is rebuilt per wait: a Pipecat worker cannot be re-run after it was
    cancelled, and the microphone must be released before the session opens it.
    """

    def __init__(
        self,
        settings: VoiceSettings,
        *,
        stt_factory: Callable[[], FrameProcessor],
        phrases: tuple[str, ...],
        transport_factory: Callable[[], BaseTransport] | None = None,
        fuzzy: float = 0.0,
    ) -> None:
        self._settings = settings
        self._stt_factory = stt_factory
        self._phrases = phrases
        self._fuzzy = fuzzy
        self._transport_factory = transport_factory or self._default_transport
        self._runner: WorkerRunner | None = None
        self._run_task: asyncio.Task | None = None

    def set_phrases(self, phrases: tuple[str, ...], fuzzy: float | None = None) -> None:
        """Change what the next passive pipeline listens for. Takes effect when the
        listener is next (re)built, so the daemon re-arms it after a settings change."""
        self._phrases = phrases
        if fuzzy is not None:
            self._fuzzy = fuzzy

    def _default_transport(self) -> BaseTransport:
        return SoundDeviceTransport(
            SoundDeviceTransportParams(
                audio_in_enabled=True,
                audio_out_enabled=False,
                input_device=self._settings.input_device,
            )
        )

    def _build(self, on_match: Callable[[WakeMatch], Awaitable[None]]) -> PipelineWorker:
        s = self._settings
        vad = VADProcessor(
            vad_analyzer=SileroVADAnalyzer(
                params=VADParams(
                    confidence=s.vad_confidence,
                    start_secs=s.vad_start_secs,
                    stop_secs=s.wake_vad_stop_secs,
                    min_volume=s.vad_min_volume,
                )
            )
        )
        pipeline = Pipeline(
            [
                self._transport_factory().input(),
                vad,
                self._stt_factory(),
                WakeMatchProcessor(self._phrases, on_match, fuzzy=self._fuzzy),
            ]
        )
        return PipelineWorker(
            pipeline,
            params=PipelineParams(audio_in_sample_rate=16000),
            processor_unusable_policy=ProcessorUnusablePolicy.END,
            # Pipecat cancels a worker that sees no speech for five minutes by default.
            # A wake listener is idle by definition; it must sit for hours.
            idle_timeout_secs=None,
        )

    async def wait(self) -> WakeMatch:
        matched = asyncio.Event()
        result: list[WakeMatch] = []

        async def on_match(m: WakeMatch) -> None:
            if matched.is_set():
                return
            logger.info(f"wake phrase: {m.phrase!r}")
            result.append(m)
            matched.set()

        backoff = 1.0
        while True:
            worker = self._build(on_match)
            self._runner = WorkerRunner(handle_sigint=False)
            await self._runner.add_workers(worker)
            self._run_task = asyncio.ensure_future(self._runner.run())
            match_task = asyncio.ensure_future(matched.wait())
            try:
                await asyncio.wait(
                    {match_task, self._run_task}, return_when=asyncio.FIRST_COMPLETED
                )
            finally:
                match_task.cancel()
                await self.stop()
            if matched.is_set():
                return result[0]
            # The pipeline ended on its own (device gone, processor failure). Rebuild it
            # rather than sit in "passive" with nobody reading the microphone.
            logger.error(f"passive listener stopped without a wake phrase; retry in {backoff:g}s")
            await asyncio.sleep(backoff)
            backoff = min(backoff * 2, 30.0)

    async def stop(self) -> None:
        if self._runner and self._run_task:
            if not self._run_task.done():
                await self._runner.cancel()
            try:
                await self._run_task
            except asyncio.CancelledError:
                pass
            except Exception as e:  # noqa: BLE001 - surfaced, then the listener is rebuilt
                logger.error(f"passive listener failed: {e!r}")
        self._run_task = None
        self._runner = None


def build_wake_listener(
    settings: VoiceSettings,
    *,
    stt_factory: Callable[[], FrameProcessor],
    phrases: tuple[str, ...],
    transport_factory: Callable[[], BaseTransport] | None = None,
    fuzzy: float = 0.0,
) -> WakeListener:
    return WakeListener(
        settings,
        stt_factory=stt_factory,
        phrases=phrases,
        transport_factory=transport_factory,
        fuzzy=fuzzy,
    )
