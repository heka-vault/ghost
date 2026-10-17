"""Wake detection.

Passive state: only the local wake-word model sees microphone frames. Nothing is
transcribed, recorded, or sent (CLAUDE.md).

``FileWakeDetector`` is the development trigger: ``ghostd wake`` (or SIGUSR1).
``OnnxWakeDetector`` runs a trained ``livekit-wakeword`` model; the model adapter
is finished in the wake-word spike (handoff task 4, ADR 0005).
"""

from __future__ import annotations

import asyncio
import signal
from collections.abc import Callable
from pathlib import Path
from typing import Protocol

import numpy as np
from loguru import logger


class WakeDetector(Protocol):
    async def wait(self) -> None:
        """Return when the wake word fires."""
        ...

    def close(self) -> None: ...


class FileWakeDetector:
    def __init__(self, path: Path) -> None:
        self._path = path
        self._event = asyncio.Event()
        loop = asyncio.get_event_loop()
        loop.add_signal_handler(signal.SIGUSR1, self._event.set)

    async def wait(self) -> None:
        self._path.unlink(missing_ok=True)
        self._event.clear()
        while not self._event.is_set():
            if self._path.exists():
                self._path.unlink(missing_ok=True)
                return
            try:
                await asyncio.wait_for(self._event.wait(), timeout=0.2)
            except TimeoutError:
                pass

    def close(self) -> None:
        self._path.unlink(missing_ok=True)


class WakeModel(Protocol):
    """One frame in, a score out. Implemented against the trained ONNX model."""

    frame_samples: int

    def score(self, pcm16k: np.ndarray) -> float: ...


class OnnxWakeDetector:
    def __init__(
        self,
        model: WakeModel,
        *,
        threshold: float,
        cooldown_secs: float,
        device: int | str | None = None,
        paused: Callable[[], bool] = lambda: False,
    ) -> None:
        self._model = model
        self._threshold = threshold
        self._cooldown = cooldown_secs
        self._device = device
        self._paused = paused
        self._stream = None

    async def wait(self) -> None:
        import sounddevice as sd

        loop = asyncio.get_event_loop()
        fired = asyncio.Event()
        last = {"t": 0.0}

        def on_audio(indata, frames, time_info, status) -> None:
            if self._paused():
                return
            pcm = np.frombuffer(bytes(indata), dtype=np.int16).astype(np.float32) / 32768.0
            s = self._model.score(pcm)
            now = loop.time()
            if s >= self._threshold and now - last["t"] > self._cooldown:
                last["t"] = now
                logger.info(f"wake score {s:.2f}")
                loop.call_soon_threadsafe(fired.set)

        self._stream = sd.RawInputStream(
            samplerate=16000,
            blocksize=self._model.frame_samples,
            device=self._device,
            channels=1,
            dtype="int16",
            callback=on_audio,
        )
        with self._stream:
            await fired.wait()
        self._stream = None

    def close(self) -> None:
        if self._stream:
            self._stream.close()
