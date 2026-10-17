"""Wake detection.

Passive state: microphone frames stay on this machine. ``TranscriptWakeDetector``
(the default, ADR 0010) runs local VAD and Parakeet and drops every transcript
that does not contain a wake phrase. ``FileWakeDetector`` is the development
trigger: ``ghostd wake`` (or SIGUSR1); the transcript detector honours it too.
``OnnxWakeDetector`` will run the trained ``livekit-wakeword`` model once the
spike produces one (ADR 0005).

``wait()`` returns what the operator said after the wake phrase, or ``None``.
"""

from __future__ import annotations

import asyncio
import signal
from collections.abc import Callable
from pathlib import Path
from typing import Protocol

import numpy as np
from ghost_voice.wake import WakeListener
from loguru import logger


class WakeDetector(Protocol):
    async def wait(self) -> str | None:
        """Return when the wake word fires, with any words spoken after it."""
        ...

    def close(self) -> None: ...


class FileWakeDetector:
    def __init__(self, path: Path) -> None:
        self._path = path
        self._event = asyncio.Event()
        loop = asyncio.get_event_loop()
        loop.add_signal_handler(signal.SIGUSR1, self._event.set)

    async def wait(self) -> str | None:
        self._path.unlink(missing_ok=True)
        self._event.clear()
        while not self._event.is_set():
            if self._path.exists():
                self._path.unlink(missing_ok=True)
                return None
            try:
                await asyncio.wait_for(self._event.wait(), timeout=0.2)
            except TimeoutError:
                pass
        return None

    def close(self) -> None:
        self._path.unlink(missing_ok=True)


class TranscriptWakeDetector:
    """Local STT listens for a phrase; the file/SIGUSR1 trigger still works alongside."""

    def __init__(self, listener: WakeListener, manual: FileWakeDetector) -> None:
        self._listener = listener
        self._manual = manual

    async def wait(self) -> str | None:
        spoken = asyncio.ensure_future(self._listener.wait())
        manual = asyncio.ensure_future(self._manual.wait())
        try:
            done, _ = await asyncio.wait({spoken, manual}, return_when=asyncio.FIRST_COMPLETED)
            if spoken in done:
                return spoken.result().remainder or None
            return None
        finally:
            # Whether a phrase matched, the file fired, or we were cancelled
            # (Ctrl+C): release the microphone before anything else runs.
            for t in (spoken, manual):
                if not t.done():
                    t.cancel()
            await asyncio.gather(spoken, manual, return_exceptions=True)
            await self._listener.stop()

    def close(self) -> None:
        self._manual.close()


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

    async def wait(self) -> str | None:
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
        return None

    def close(self) -> None:
        if self._stream:
            self._stream.close()
