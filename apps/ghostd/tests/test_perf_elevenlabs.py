"""``ghostd perf micro``'s ElevenLabs timing, without ElevenLabs: it skips with no key, and
against a loopback stand-in it times lines and reports a failure without the key."""

from __future__ import annotations

import base64
import json
import socket

from ghost_voice import VoiceSettings
from ghostd.perf.elevenlabs import first_audio
from ghostd.perf.micro import _stats
from websockets.asyncio.server import serve

KEY = "sk_test_do_not_print_0123456789abcdef"
LINES = {"3 words": "Labeled it bug."}
ENV = ("GHOST_TTS_ENGINE", "ELEVENLABS_API_KEY", "GHOST_ELEVENLABS_VOICE")


def settings(monkeypatch, **env: str) -> VoiceSettings:
    for name in ENV:
        monkeypatch.delenv(name, raising=False)
    for name, value in env.items():
        monkeypatch.setenv(name, value)
    return VoiceSettings()  # type: ignore[call-arg]


async def answer(ws) -> None:
    """Audio for each text, ``isFinal`` when the context closes."""
    async for raw in ws:
        msg = json.loads(raw)
        ctx = msg.get("context_id")
        if msg.get("close_socket"):
            await ws.close()
            return
        if msg.get("close_context") or msg.get("flush"):
            await ws.send(json.dumps({"isFinal": True, "contextId": ctx}))
        elif msg.get("text", "").strip() and ctx:
            audio = base64.b64encode(bytes(48000)).decode()  # one second at 24 kHz
            await ws.send(json.dumps({"audio": audio, "contextId": ctx}))


async def test_skips_without_a_key(monkeypatch):
    s = settings(monkeypatch, GHOST_ELEVENLABS_VOICE="el_default")
    out = await first_audio(s, LINES, 1, _stats)
    assert "ELEVENLABS_API_KEY is not set" in out["skipped"]


async def test_times_lines_on_a_warm_socket(monkeypatch):
    s = settings(monkeypatch, ELEVENLABS_API_KEY=KEY, GHOST_ELEVENLABS_VOICE="el_default")
    async with serve(answer, "127.0.0.1", 0) as server:
        host, port = server.sockets[0].getsockname()[:2]
        out = await first_audio(s, LINES, 2, _stats, url=f"ws://{host}:{port}")
    assert out["connect"] > 0
    line = out["lines"]["3 words"]
    assert 0 < line["first"]["p50"] <= line["whole"]["p50"]
    assert line["audio"] == 1.0


async def test_reports_a_failure_without_the_key(monkeypatch):
    s = settings(monkeypatch, ELEVENLABS_API_KEY=KEY, GHOST_ELEVENLABS_VOICE="el_default")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    out = await first_audio(s, LINES, 1, _stats, url=f"ws://127.0.0.1:{port}")
    assert out["failed"] and KEY not in out["failed"]
