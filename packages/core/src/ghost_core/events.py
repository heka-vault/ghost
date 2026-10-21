"""Fire-and-forget lifecycle events for ability listeners."""

from __future__ import annotations

import asyncio
from collections.abc import Iterable
from typing import Any

from loguru import logger

from ghost_core.source import Event, EventListener


class Events:
    def __init__(self, listeners: Iterable[EventListener] = (), timeout: float = 2.0) -> None:
        self._listeners = list(listeners)
        self._timeout = timeout

    def add(self, listener: EventListener) -> None:
        self._listeners.append(listener)

    async def emit(self, kind: Any, **data: Any) -> None:
        """Call every listener with a deadline so none of them can stall speech."""
        if not self._listeners:
            return
        event = Event(kind=kind, data=data)

        async def one(listener: EventListener) -> None:
            try:
                await asyncio.wait_for(listener(event), self._timeout)
            except Exception as e:  # noqa: BLE001 - a listener never breaks a session
                logger.warning(f"listener {getattr(listener, '__name__', listener)} failed: {e}")

        await asyncio.gather(*(one(ln) for ln in self._listeners))
