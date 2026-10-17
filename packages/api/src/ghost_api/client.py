"""A tiny client over the Unix socket for the CLI (`ghostd wake|say|sleep|status`)."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import httpx


class GhostClient:
    def __init__(self, socket_path: Path, timeout: float = 10.0) -> None:
        self._http = httpx.Client(
            transport=httpx.HTTPTransport(uds=str(socket_path)),
            base_url="http://ghost",
            timeout=timeout,
        )

    def close(self) -> None:
        self._http.close()

    def status(self) -> dict[str, Any]:
        r = self._http.get("/status")
        r.raise_for_status()
        return r.json()

    def wake(self, text: str | None = None) -> dict[str, Any]:
        r = self._http.post("/wake", json={"text": text} if text else None)
        r.raise_for_status()
        return r.json()

    def sleep(self) -> dict[str, Any]:
        r = self._http.post("/sleep")
        r.raise_for_status()
        return r.json()

    def say(self, text: str) -> dict[str, Any]:
        r = self._http.post("/session/say", json={"text": text})
        r.raise_for_status()
        return r.json()

    def confirm(self) -> dict[str, Any]:
        r = self._http.post("/proposals/pending/confirm")
        r.raise_for_status()
        return r.json()
