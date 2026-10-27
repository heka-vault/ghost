"""The webhook listener (ADR 0027): a third app in ``ghostd``, for webhooks only.

It serves ``POST /hooks/<name>`` for the hooks ``hooks.json`` declares (ADR 0035) and
nothing else: no phone route, nothing of the control API, no static files. It binds
its own addresses (``GHOST_HOOKS_LISTEN``, comma separated; off when empty), apart from the
phone listener and the control API's socket. How the internet reaches those addresses is
the operator's network.

A bare ASGI app, so it sees the exact bytes: the body is capped while it is read, and the
path, the headers and those bytes go to ``ghost_core.hooks.HookIntake``, which verifies
the signature before anything else. The reply says only accepted or refused; it never
echoes the body. Nothing here is logged but the outcome.
"""

from __future__ import annotations

import asyncio
import json
import socket
from typing import Any, Protocol

import uvicorn
from loguru import logger
from starlette.types import Receive, Scope, Send

from ghostd.phone.listener import _Server, parse_listen

MAX_BODY = 256 * 1024
PREFIX = "/hooks/"


class Intake(Protocol):
    def knows(self, path: str) -> bool: ...

    async def receive(self, path: str, headers: dict[str, str], body: bytes) -> Any: ...


async def _reply(send: Send, status: int, body: dict[str, Any]) -> None:
    payload = json.dumps(body).encode()
    await send(
        {
            "type": "http.response.start",
            "status": status,
            "headers": [
                (b"content-type", b"application/json"),
                (b"content-length", str(len(payload)).encode()),
                (b"cache-control", b"no-store"),
            ],
        }
    )
    await send({"type": "http.response.body", "body": payload})


def hooks_app(intake: Intake):
    """The ASGI app. Anything but ``POST /hooks/<name>`` is a 404 or 405 with no body
    read."""

    async def app(scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            return
        path = scope["path"]
        parts = path[len(PREFIX) :].split("/") if path.startswith(PREFIX) else []
        if len(parts) != 1 or not all(parts):
            await _reply(send, 404, {"error": "not_found"})
            return
        if scope["method"] != "POST":
            await _reply(send, 405, {"error": "method_not_allowed"})
            return
        if not intake.knows(path):  # ADR 0027: an unknown source is a 404, no body read
            await _reply(send, 404, {"error": "not_found"})
            return
        body = b""
        more = True
        while more:
            msg = await receive()
            if msg["type"] != "http.request":
                return  # the sender went away
            body += msg.get("body", b"")
            more = msg.get("more_body", False)
            if len(body) > MAX_BODY:
                await _reply(send, 413, {"error": "too_large"})
                return
        headers = {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in scope["headers"]}
        try:
            result = await intake.receive(path, headers, body)
        except Exception:  # noqa: BLE001 - the sender learns nothing of our failure
            logger.exception("hooks: intake failed")
            await _reply(send, 500, {"error": "internal"})
            return
        ok = result.status < 400
        await _reply(send, result.status, {"ok": ok} if ok else {"error": "refused"})

    return app


class HooksListener:
    def __init__(self, listen: str, intake: Intake) -> None:
        self.addresses = [a.strip() for a in listen.split(",") if a.strip()]
        if not self.addresses:
            raise ValueError("GHOST_HOOKS_LISTEN is empty")
        for a in self.addresses:
            parse_listen(a)
        self.app = hooks_app(intake)
        self._servers: list[tuple[str, _Server, asyncio.Task[None]]] = []

    async def start(self) -> None:
        for address in self.addresses:
            host, port = parse_listen(address)
            family = socket.AF_INET6 if ":" in host else socket.AF_INET
            sock = socket.socket(family, socket.SOCK_STREAM)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                sock.bind((host, port))
            except OSError:
                sock.close()
                raise
            sock.set_inheritable(True)
            config = uvicorn.Config(
                self.app,
                log_level="warning",
                access_log=False,  # paths and senders stay out of the journal
                lifespan="off",
                timeout_graceful_shutdown=2.0,
            )
            server = _Server(config)
            task = asyncio.ensure_future(server.serve(sockets=[sock]))
            for _ in range(200):
                if server.started or task.done():
                    break
                await asyncio.sleep(0.02)
            if not server.started:
                server.should_exit = True
                await asyncio.gather(task, return_exceptions=True)
                raise RuntimeError(f"hooks listener did not start on {address}")
            self._servers.append((address, server, task))
            logger.info(f"hooks: listening on {address}")

    @property
    def port(self) -> int:
        assert self._servers, "not started"
        return self._servers[0][1].servers[0].sockets[0].getsockname()[1]

    async def stop(self) -> None:
        for _, server, _ in self._servers:
            server.should_exit = True
        tasks = [t for _, _, t in self._servers]
        if tasks:
            try:
                await asyncio.wait_for(asyncio.shield(asyncio.gather(*tasks)), 4.0)
            except TimeoutError:
                for _, server, task in self._servers:
                    server.force_exit = True
                    task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
        self._servers = []
