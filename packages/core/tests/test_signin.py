"""Signing in to an app (ADR 0034): tokens in a 0600 file, the browser's way back on a
loopback address that listens only while a sign-in is open, and no browser at start."""

from __future__ import annotations

import asyncio
import socket
import stat

import pytest
from ghost_core.apps import AppSpec, NeedsSignIn
from ghost_core.signin import FileTokenStorage, SignIns
from mcp.shared.auth import OAuthToken


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


async def get(port: int, path: str) -> str:
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(f"GET {path} HTTP/1.1\r\nHost: x\r\n\r\n".encode())
    await writer.drain()
    data = await reader.read()
    writer.close()
    return data.decode()


async def test_tokens_live_in_a_private_file(tmp_path):
    store = FileTokenStorage(tmp_path / "linear" / "oauth.json")
    assert await store.get_tokens() is None and not store.has_tokens()
    await store.set_tokens(OAuthToken(access_token="secret", token_type="Bearer"))
    assert (await store.get_tokens()).access_token == "secret"
    assert stat.S_IMODE(store.path.stat().st_mode) == 0o600
    store.clear()
    assert not store.path.exists()


def test_only_an_app_that_signs_in_gets_an_auth(tmp_path):
    s = SignIns(tmp_path)
    assert s.auth_for(AppSpec(name="x", command="y"), True) is None
    assert s.auth_for(AppSpec(name="x", url="https://x/mcp"), True) is None
    assert s.auth_for(AppSpec(name="x", url="https://x/mcp", sign_in=True), True) is not None


async def test_with_nobody_at_the_browser_a_sign_in_waits_on_the_operator(tmp_path):
    s = SignIns(tmp_path, port=free_port())
    auth = s.auth_for(AppSpec(name="linear", url="https://x/mcp", sign_in=True), False)
    with pytest.raises(NeedsSignIn):
        await auth.context.redirect_handler("https://auth.example/authorize?x=1")
    assert s.pending_url("linear") is None


async def test_the_browser_comes_back_to_the_loopback_address_once(tmp_path):
    port = free_port()
    heard: list[tuple[str, str]] = []
    s = SignIns(tmp_path, port=port, on_url=lambda app, url: heard.append((app, url)))
    auth = s.auth_for(AppSpec(name="linear", url="https://x/mcp", sign_in=True), True)
    await auth.context.redirect_handler("https://auth.example/authorize?x=1")
    assert heard == [("linear", "https://auth.example/authorize?x=1")]
    assert s.pending_url("linear") == "https://auth.example/authorize?x=1"
    waiting = asyncio.ensure_future(auth.context.callback_handler())
    assert "404" in (await get(port, "/elsewhere")).split("\r\n")[0]
    page = await get(port, "/callback?code=abc&state=st")
    assert "signed in" in page
    result = await asyncio.wait_for(waiting, 2)
    assert (result.code, result.state) == ("abc", "st")
    assert s.pending_url("linear") is None
    with pytest.raises(OSError):  # the listener is gone with the sign-in
        await get(port, "/callback?code=again")
    await s.close()
