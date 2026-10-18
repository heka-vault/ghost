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


def match_wake(text: str, phrases: tuple[str, ...] | list[str]) -> WakeMatch | None:
    """Find the first wake phrase as a contiguous word sequence in ``text``.

    Case and punctuation are ignored, so "Ghost, you there?" matches
    "ghost you there". Longer phrases are tried first so "hey ghost" wins over
    a plain "ghost" if both are configured.
    """
    words = _words(text)
    if not words:
        return None
    for phrase in sorted(phrases, key=lambda p: -len(p.split())):
        pw = phrase.split()
        n = len(pw)
        for i in range(len(words) - n + 1):
            if words[i : i + n] == pw:
                return WakeMatch(phrase=phrase, remainder=" ".join(words[i + n :]))
    return None


class WakeMatchProcessor(FrameProcessor):
    """Last processor of the passive pipeline. Transcripts end here."""

    def __init__(
        self,
        phrases: tuple[str, ...],
        on_match: Callable[[WakeMatch], Awaitable[None]],
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self._phrases = phrases
        self._on_match = on_match

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        if isinstance(frame, TranscriptionFrame):
            m = match_wake(frame.text, self._phrases)
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
    ) -> None:
        self._settings = settings
        self._stt_factory = stt_factory
        self._phrases = phrases
        self._transport_factory = transport_factory or self._default_transport
        self._runner: WorkerRunner | None = None
        self._run_task: asyncio.Task | None = None

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
                    stop_secs=s.vad_stop_secs,
                    min_volume=s.vad_min_volume,
                )
            )
        )
        pipeline = Pipeline(
            [
                self._transport_factory().input(),
                vad,
                self._stt_factory(),
                WakeMatchProcessor(self._phrases, on_match),
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
) -> WakeListener:
    return WakeListener(
        settings, stt_factory=stt_factory, phrases=phrases, transport_factory=transport_factory
    )
