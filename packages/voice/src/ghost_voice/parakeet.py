"""Parakeet TDT 0.6B v3 through onnx-asr, as a Pipecat segmented STT service."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncGenerator

import numpy as np
from loguru import logger
from pipecat.frames.frames import ErrorFrame, Frame, TranscriptionFrame
from pipecat.services.settings import STTSettings
from pipecat.services.stt_service import SegmentedSTTService
from pipecat.utils.time import time_now_iso8601


class ParakeetSTTService(SegmentedSTTService):
    """Transcribes each VAD-delimited segment off the event loop.

    Measured on the target CPU: about 0.16 s per command, 1.7 GB RAM, 10 s to load
    (docs/benchmarks/2026-10-16-speech-cpu.md).
    """

    def __init__(
        self,
        *,
        model: str = "nemo-parakeet-tdt-0.6b-v3",
        quantization: str | None = "int8",
        threads: int = 4,
        sample_rate: int = 16000,
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

    async def run_stt(self, audio: bytes) -> AsyncGenerator[Frame, None]:
        if self._model is None:
            await asyncio.to_thread(self.load)
        assert self._model is not None
        await self.start_processing_metrics()
        await self.start_ttfb_metrics()
        pcm = np.frombuffer(audio, dtype=np.int16).astype(np.float32) / 32768.0
        try:
            text = await asyncio.to_thread(self._model.recognize, pcm, sample_rate=self.sample_rate)
        except Exception as e:  # noqa: BLE001
            logger.exception("Parakeet failed")
            yield ErrorFrame(f"stt: {e}")
            return
        finally:
            await self.stop_ttfb_metrics()
            await self.stop_processing_metrics()
        text = (text or "").strip()
        if text:
            logger.debug(f"stt: {text!r}")
            yield TranscriptionFrame(text, self._user_id, time_now_iso8601())
