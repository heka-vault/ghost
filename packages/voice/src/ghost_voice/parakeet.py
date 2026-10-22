"""Parakeet TDT 0.6B v3 through onnx-asr, as a Pipecat segmented STT service."""

from __future__ import annotations

import asyncio
import threading
import time
from collections.abc import AsyncGenerator

import numpy as np
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    CancelFrame,
    EndFrame,
    ErrorFrame,
    Frame,
    InputAudioRawFrame,
    InterimTranscriptionFrame,
    InterruptionFrame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.processors.frame_processor import FrameDirection
from pipecat.services.settings import STTSettings
from pipecat.services.stt_service import SegmentedSTTService
from pipecat.utils.time import time_now_iso8601

from ghost_voice import debug

# An interim reads at most this much of the open segment: enough for the pre-roll and the
# first words, which is all barge-in needs, and it keeps each pass near 0.2 s when the
# segment runs long (Parakeet's time grows with the audio, about 30 ms a second).
INTERIM_WINDOW_SECS = 4.0


class ParakeetSTTService(SegmentedSTTService):
    """Transcribes each VAD-delimited segment off the event loop.

    Measured on the target CPU: about 0.16 s per command, 1.7 GB RAM, 10 s to load
    (docs/benchmarks/2026-10-16-speech-cpu.md).

    A segment is transcribed once the VAD says it ended, so an operator talking over
    Ghost was heard, and could cut it off (``turns.py``), only after they paused. With
    ``interim_secs`` > 0, while Ghost is speaking and the operator's segment is open, the
    segment so far (the pre-roll included) is transcribed again every ``interim_secs`` and
    pushed as an ``InterimTranscriptionFrame``. Only the barge-in rule and the echo filter
    read those; the context, the interceptors, the confirmation path and the transcript
    store take final ``TranscriptionFrame``s only. The wake listener's fork has them off.

    One recognition runs at a time on the loaded model, across every fork: a final waits
    for an interim in flight, and an interim tick that finds the model busy is skipped.
    """

    def __init__(
        self,
        *,
        model: str = "nemo-parakeet-tdt-0.6b-v3",
        quantization: str | None = "int8",
        threads: int = 4,
        sample_rate: int = 16000,
        interim_secs: float = 0.0,
        **kwargs,
    ) -> None:
        # Measured p50 is 0.16 s per command; 0.5 s leaves headroom under load.
        kwargs.setdefault("ttfs_p99_latency", 0.5)
        super().__init__(
            sample_rate=sample_rate,
            settings=STTSettings(model=model, language=None),
            **kwargs,
        )
        self._model_name = model
        self._quantization = quantization
        self._threads = threads
        self._model = None
        self._busy = threading.Lock()  # shared with every fork, like the model
        self._interim_secs = interim_secs
        self._bot_speaking = False
        self._segment = 0  # bumped at each VAD start and stop: a late interim is stale
        self._interim_due: float | None = None
        self._interim_task: asyncio.Task | None = None

    def fork(self, *, interim_secs: float = 0.0) -> ParakeetSTTService:
        """A fresh processor sharing this instance's loaded model.

        A Pipecat processor belongs to one pipeline at a time, so each session and
        each passive wake listener gets its own; the 1.7 GB model is loaded once.
        ``interim_secs`` is for sessions (see the class doc); passive audio gets none.
        """
        other = ParakeetSTTService(
            model=self._model_name,
            quantization=self._quantization,
            threads=self._threads,
            sample_rate=self.sample_rate,
            interim_secs=interim_secs,
        )
        other._model = self._model
        other._busy = self._busy
        return other

    def wants_wav_segments(self) -> bool:
        return False  # raw int16 PCM

    def can_generate_metrics(self) -> bool:
        return True

    def load(self) -> None:
        """Load synchronously. ghostd calls this once at startup, not per session."""
        if self._model is not None:
            return
        import onnx_asr
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = self._threads
        so.inter_op_num_threads = 1
        logger.info(f"Loading {self._model_name} ({self._quantization})")
        self._model = onnx_asr.load_model(
            self._model_name, quantization=self._quantization, sess_options=so
        )

    def recognize(self, pcm: np.ndarray) -> str:
        """Transcribe float32 16 kHz PCM in the calling thread (call it off the event loop),
        taking its turn on the shared model. The wake detector's check (ADR 0025)."""
        if self._model is None:
            self.load()
        # Called on the loaded template, which never runs in a pipeline: its sample_rate
        # is only set when one starts, so use the rate it was built for.
        return (self._recognize(pcm, rate=self._init_sample_rate) or "").strip()

    def _recognize(
        self, pcm: np.ndarray, *, wait: bool = True, rate: int | None = None
    ) -> str | None:
        """In a worker thread. None, without running, if ``wait`` is off and the model is busy."""
        if not self._busy.acquire(blocking=wait):
            return None
        try:
            assert self._model is not None
            return self._model.recognize(pcm, sample_rate=rate or self.sample_rate)
        finally:
            self._busy.release()

    async def run_stt(self, audio: bytes) -> AsyncGenerator[Frame, None]:
        if self._model is None:
            await asyncio.to_thread(self.load)
        assert self._model is not None
        await self.start_processing_metrics()
        await self.start_ttfb_metrics()
        pcm = np.frombuffer(audio, dtype=np.int16).astype(np.float32) / 32768.0
        try:
            text = await asyncio.to_thread(self._recognize, pcm)
        except Exception as e:  # noqa: BLE001
            logger.exception("Parakeet failed")
            yield ErrorFrame(f"stt: {e}")
            return
        finally:
            await self.stop_ttfb_metrics()
            await self.stop_processing_metrics()
        text = (text or "").strip()
        # Transcripts are logged only under GHOST_DEBUG_AUDIO (ghost_voice.debug).
        if text:
            debug.audio(f"heard: {text!r}")
            yield TranscriptionFrame(text, self._user_id, time_now_iso8601())

    # --- interim transcripts while Ghost speaks -------------------------------------

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        if isinstance(frame, BotStartedSpeakingFrame):
            self._bot_speaking = True
        elif isinstance(frame, (BotStoppedSpeakingFrame, InterruptionFrame)):
            self._bot_speaking = False  # an interruption silences Ghost before BotStopped
        elif isinstance(frame, (VADUserStartedSpeakingFrame, VADUserStoppedSpeakingFrame)):
            self._segment += 1
            self._interim_due = None
        await super().process_frame(frame, direction)

    async def process_audio_frame(self, frame: InputAudioRawFrame, direction: FrameDirection):
        await super().process_audio_frame(frame, direction)
        if self._interim_secs > 0:
            self._maybe_interim()

    def _maybe_interim(self) -> None:
        if not (self._bot_speaking and self._user_speaking and self.is_usable):
            self._interim_due = None
            return
        now = time.monotonic()
        if self._interim_due is None:
            self._interim_due = now + self._interim_secs  # the first one an interval in
            return
        if now < self._interim_due:
            return
        self._interim_due = now + self._interim_secs
        if self._interim_task is not None or self._busy.locked():
            return  # a recognition is in flight: skip this tick
        window = int(self.sample_rate * INTERIM_WINDOW_SECS) * 2
        audio = bytes(self._audio_buffer[-window:]) + self._trailing_silence()
        self._interim_task = self.create_task(self._interim(audio, self._segment), "interim")

    async def _interim(self, audio: bytes, segment: int) -> None:
        pcm = np.frombuffer(audio, dtype=np.int16).astype(np.float32) / 32768.0
        try:
            text = await asyncio.to_thread(self._recognize, pcm, wait=False)
        except Exception:  # noqa: BLE001 - an interim is a hint; the final still comes
            logger.exception("Parakeet interim failed")
            return
        finally:
            self._interim_task = None
        text = (text or "").strip()
        # The segment ended (its final is queued behind this) or Ghost went quiet: stale.
        if not text or segment != self._segment or not (self._bot_speaking and self._user_speaking):
            return
        debug.audio(f"interim: {text!r}")
        await self.push_frame(InterimTranscriptionFrame(text, self._user_id, time_now_iso8601()))

    async def _cancel_interim(self) -> None:
        if self._interim_task is not None:
            task, self._interim_task = self._interim_task, None
            await self.cancel_task(task)

    async def stop(self, frame: EndFrame) -> None:
        await self._cancel_interim()
        await super().stop(frame)

    async def cancel(self, frame: CancelFrame) -> None:
        await self._cancel_interim()
        await super().cancel(frame)
