"""Wake detection.

Passive state: microphone frames stay on this machine. ``SpokenWakeDetector`` runs the
passive listener: by transcript (the default, ADR 0010: local VAD and Parakeet, every
transcript without a wake phrase dropped), or by the trained "hey ghost" detector with
Parakeet checking its hits (``GHOST_WAKE_MODE=onnx``, ADR 0025). ``FileWakeDetector`` is
the development trigger: ``ghostd wake`` (or SIGUSR1); every mode honours it.

``wait()`` returns what the operator said after the wake phrase, or ``None``.
"""

from __future__ import annotations

import asyncio
import signal
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Protocol

from ghost_voice.wake import WakeListener


class WakeDetector(Protocol):
    async def wait(self) -> str | None:
        """Return when the wake word fires, with any words spoken after it."""
        ...

    def close(self) -> None: ...


class FileWakeDetector:
    """The manual trigger: the wake file, SIGUSR1, or ``trigger()`` from the control API
    (optionally with words to treat as the first turn)."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._event = asyncio.Event()
        self._text: str | None = None
        loop = asyncio.get_event_loop()
        loop.add_signal_handler(signal.SIGUSR1, self._event.set)

    def trigger(self, text: str | None = None) -> None:
        self._text = text
        self._event.set()

    async def wait(self) -> str | None:
        self._path.unlink(missing_ok=True)
        self._event.clear()
        self._text = None
        while not self._event.is_set():
            if self._path.exists():
                self._path.unlink(missing_ok=True)
                return None
            try:
                await asyncio.wait_for(self._event.wait(), timeout=0.2)
            except TimeoutError:
                pass
        text, self._text = self._text, None
        return text

    def close(self) -> None:
        self._path.unlink(missing_ok=True)


class SpokenWakeDetector:
    """The passive listener hears the wake phrase; the file/SIGUSR1 trigger still works
    alongside. ``on_heard`` runs when the trained detector has decided, before the
    operator finishes speaking (the early chime); transcript wake never calls it."""

    def __init__(
        self,
        listener: WakeListener,
        manual: FileWakeDetector,
        on_heard: Callable[[], Awaitable[None]] | None = None,
    ) -> None:
        self._listener = listener
        self._manual = manual
        self._on_heard = on_heard

    async def wait(self) -> str | None:
        spoken = asyncio.ensure_future(self._listener.wait(on_heard=self._on_heard))
        manual = asyncio.ensure_future(self._manual.wait())
        try:
            done, _ = await asyncio.wait({spoken, manual}, return_when=asyncio.FIRST_COMPLETED)
            if spoken in done:
                return spoken.result().remainder or None
            return manual.result()
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
