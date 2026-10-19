"""Serve the app on a user-owned Unix socket."""

from __future__ import annotations

import asyncio
import os
from pathlib import Path

import uvicorn
from fastapi import FastAPI
from loguru import logger


def default_socket_path() -> Path:
    base = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "ghost"
    base.mkdir(parents=True, exist_ok=True, mode=0o700)
    return base / "api.sock"


SHUTDOWN_SECS = 2.0


class ApiServer:
    def __init__(self, app: FastAPI, socket_path: Path) -> None:
        self.path = socket_path
        # An open /events WebSocket (the desktop app) never finishes on its own; without a
        # limit uvicorn waits for it forever and the daemon never exits.
        config = uvicorn.Config(
            app,
            uds=str(socket_path),
            log_level="warning",
            lifespan="off",
            access_log=False,
            timeout_graceful_shutdown=SHUTDOWN_SECS,
        )
        self._server = uvicorn.Server(config)
        self._task: asyncio.Task[None] | None = None

    async def start(self) -> None:
        self.path.unlink(missing_ok=True)
        self._task = asyncio.ensure_future(self._server.serve())
        for _ in range(100):  # wait for the socket to appear
            if self.path.exists():
                os.chmod(self.path, 0o600)
                logger.info(f"api: listening on {self.path}")
                return
            await asyncio.sleep(0.02)
        logger.warning("api: socket did not appear")

    async def stop(self) -> None:
        self._server.should_exit = True
        if self._task:
            try:
                await asyncio.wait_for(asyncio.shield(self._task), SHUTDOWN_SECS + 2)
            except TimeoutError:
                logger.warning("api: connections still open at shutdown; closing them")
                self._server.force_exit = True
                self._task.cancel()
                await asyncio.gather(self._task, return_exceptions=True)
            except Exception:  # noqa: BLE001 - the server's own failure, already logged
                pass
        self.path.unlink(missing_ok=True)
