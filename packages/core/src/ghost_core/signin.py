"""Signing in to an app (ADR 0034): OAuth 2.1 with PKCE, the way the MCP authorization spec
asks, through the operator's browser.

Ghost has no public address, so it cannot publish a client ID metadata document; it
registers itself with the app's authorization server (dynamic client registration), which
the SDK does when the server offers it. The browser comes back to a loopback address,
``http://127.0.0.1:<port>/callback``, which listens only while a sign-in is open, on
127.0.0.1 only, for one path. The port is fixed (``GHOST_SIGNIN_PORT``) because a
registration names its redirect exactly.

Tokens and the registration are kept per app in ``~/.local/share/ghost/apps/<name>/
oauth.json`` (0600). They never reach the control API, a log line or the model; the SDK
refreshes them. When an app wants a new sign-in and nobody is at the browser (the daemon
starting), the connection fails as ``NeedsSignIn`` and the app waits on the operator's list.
"""

from __future__ import annotations

import asyncio
import json
import os
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

from loguru import logger

from ghost_core.apps import AppSpec, NeedsSignIn

DEFAULT_PORT = 47823
CLIENT_NAME = "Ghost"
_PAGE = (
    "<!doctype html><meta charset=utf-8><title>Ghost</title>"
    "<body style='font:16px system-ui;margin:4em;text-align:center'>"
    "<p>{message}</p><p>You can close this tab.</p>"
)


class FileTokenStorage:
    """The SDK's ``TokenStorage``, as one 0600 JSON file."""

    def __init__(self, path: Path) -> None:
        self.path = path

    def _read(self) -> dict[str, Any]:
        try:
            data = json.loads(self.path.read_text())
            return data if isinstance(data, dict) else {}
        except (OSError, ValueError):
            return {}

    def _write(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        tmp = self.path.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(data, f)
        tmp.replace(self.path)

    async def get_tokens(self) -> Any:
        from mcp.shared.auth import OAuthToken

        raw = self._read().get("tokens")
        return OAuthToken.model_validate(raw) if raw else None

    async def set_tokens(self, tokens: Any) -> None:
        data = self._read()
        data["tokens"] = tokens.model_dump(mode="json", exclude_none=True)
        self._write(data)

    async def get_client_info(self) -> Any:
        from mcp.shared.auth import OAuthClientInformationFull

        raw = self._read().get("client")
        return OAuthClientInformationFull.model_validate(raw) if raw else None

    async def set_client_info(self, client_info: Any) -> None:
        data = self._read()
        data["client"] = client_info.model_dump(mode="json", exclude_none=True)
        self._write(data)

    def has_tokens(self) -> bool:
        return bool(self._read().get("tokens"))

    def clear(self) -> None:
        self.path.unlink(missing_ok=True)


@dataclass
class _Flow:
    app: str
    url: str | None = None
    code: asyncio.Future[Any] | None = None


class SignIns:
    """Every app's sign-in. ``auth_for`` is what ``ghost_core.apps.Apps`` asks for when it
    connects an app that signs in; ``on_url`` hears the address the operator's browser
    should open (the desktop opens it)."""

    def __init__(
        self,
        data_dir: Path,
        *,
        port: int = DEFAULT_PORT,
        on_url: Callable[[str, str], None] | None = None,
    ) -> None:
        self.data_dir = data_dir
        self.port = port
        self.on_url = on_url
        self._flow: _Flow | None = None  # one at a time: a new sign-in replaces the open one
        self._server: asyncio.base_events.Server | None = None

    @property
    def redirect_uri(self) -> str:
        return f"http://127.0.0.1:{self.port}/callback"

    def storage(self, app: str) -> FileTokenStorage:
        return FileTokenStorage(self.data_dir / app / "oauth.json")

    def signed_in(self, app: str) -> bool:
        return self.storage(app).has_tokens()

    def forget(self, app: str) -> None:
        """The app was disconnected: its tokens and registration go."""
        self.storage(app).clear()
        if self._flow is not None and self._flow.app == app:
            self._cancel_flow()

    def pending_url(self, app: str) -> str | None:
        flow = self._flow
        return flow.url if flow is not None and flow.app == app else None

    def auth_for(self, spec: AppSpec, interactive: bool) -> Any:
        """An httpx2 Auth for ``spec``, or None when it doesn't sign in. Not interactive
        (the daemon starting, a watch reviving it), a sign-in that needs the browser
        fails as ``NeedsSignIn``; stored tokens are used and refreshed either way."""
        if not spec.sign_in or not spec.url:
            return None
        from mcp.client.auth import AuthorizationCodeResult, OAuthClientProvider
        from mcp.shared.auth import OAuthClientMetadata

        name = spec.name

        async def redirect(url: str) -> None:
            if not interactive:
                raise NeedsSignIn(f"{spec.shown} needs you to sign in again")
            await self._start_flow(name, url)

        async def callback() -> AuthorizationCodeResult:
            flow = self._flow
            if flow is None or flow.app != name or flow.code is None:
                raise NeedsSignIn(f"the sign-in to {spec.shown} was replaced")
            try:
                params = await flow.code
            finally:
                await self._end_flow(flow)
            return AuthorizationCodeResult(
                code=params.get("code", ""), state=params.get("state"), iss=params.get("iss")
            )

        metadata = OAuthClientMetadata(
            client_name=CLIENT_NAME,
            redirect_uris=[self.redirect_uri],  # type: ignore[list-item]
            grant_types=["authorization_code", "refresh_token"],
            response_types=["code"],
            token_endpoint_auth_method="none",
        )
        return OAuthClientProvider(
            server_url=spec.url,
            client_metadata=metadata,
            storage=self.storage(name),
            redirect_handler=redirect,
            callback_handler=callback,
        )

    # --- the browser's way back ----------------------------------------------------------

    async def _start_flow(self, app: str, url: str) -> None:
        self._cancel_flow()
        loop = asyncio.get_running_loop()
        self._flow = _Flow(app=app, url=url, code=loop.create_future())
        if self._server is None:
            self._server = await asyncio.start_server(self._handle, "127.0.0.1", self.port)
        logger.info(f"sign-in: {app} is waiting for the browser")
        if self.on_url is not None:
            self.on_url(app, url)

    def _cancel_flow(self) -> None:
        flow = self._flow
        if flow is not None and flow.code is not None and not flow.code.done():
            flow.code.set_exception(NeedsSignIn("the sign-in was replaced"))
        self._flow = None

    async def _end_flow(self, flow: _Flow) -> None:
        if self._flow is flow:
            self._flow = None
        if self._flow is None and self._server is not None:
            self._server.close()
            server, self._server = self._server, None
            await server.wait_closed()

    async def close(self) -> None:
        self._cancel_flow()
        if self._server is not None:
            self._server.close()
            await self._server.wait_closed()
            self._server = None

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        """One request: ``GET /callback?code=…&state=…``. Anything else is a 404."""
        try:
            line = await asyncio.wait_for(reader.readline(), 5.0)
            while (await asyncio.wait_for(reader.readline(), 5.0)) not in (b"\r\n", b"\n", b""):
                pass  # headers: nothing in them is read
            parts = line.decode("latin-1").split()
            target = urlparse(parts[1]) if len(parts) >= 2 and parts[0] == "GET" else None
            flow = self._flow
            if target is None or target.path != "/callback" or flow is None or flow.code is None:
                self._reply(writer, 404, "Nothing is waiting for this.")
                return
            q = {k: v[0] for k, v in parse_qs(target.query).items() if v}
            if "error" in q:
                if not flow.code.done():
                    flow.code.set_exception(
                        NeedsSignIn(f"the sign-in was refused ({q['error'][:40]})")
                    )
                self._reply(writer, 200, "The sign-in didn't go through.")
                return
            if not flow.code.done():
                flow.code.set_result(q)
            self._reply(writer, 200, "You're signed in.")
        except (TimeoutError, ConnectionError, UnicodeError):
            return
        finally:
            await writer.drain()
            writer.close()

    @staticmethod
    def _reply(writer: asyncio.StreamWriter, status: int, message: str) -> None:
        body = _PAGE.format(message=message).encode()
        reason = "OK" if status == 200 else "Not Found"
        writer.write(
            f"HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n"
            f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
            + body
        )
