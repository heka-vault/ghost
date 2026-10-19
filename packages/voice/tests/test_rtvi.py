"""Ghost's RTVI processor accepts client-ready and disconnect-bot, nothing else (ADR 0016)."""

from __future__ import annotations

import asyncio

from ghost_voice.rtvi import GhostRTVIProcessor, allowed, observer_params
from pipecat.frames.frames import InputTransportMessageFrame
from pipecat.processors.frameworks.rtvi.models import Message


def rtvi(type_: str, data=None) -> dict:
    return {"label": "rtvi-ai", "type": type_, "id": "m1", "data": data}


def test_allowlist():
    assert allowed(rtvi("client-ready", {"version": "2.1.0"}))
    assert allowed(rtvi("disconnect-bot"))
    for t in (
        "send-text",
        "raw-audio",
        "raw-audio-batch",
        "llm-function-call-result",
        "client-message",
        "dtmf",
        "ui-event",
    ):
        assert not allowed(rtvi(t, {})), t
    assert not allowed({"type": "client-ready"})  # no label
    assert not allowed("ping")


async def test_send_text_never_reaches_the_queue_or_the_llm():
    p = GhostRTVIProcessor()
    p._message_queue = asyncio.Queue()
    handled: list[str] = []

    async def record(data):
        handled.append("send-text")

    p._handle_send_text = record  # type: ignore[method-assign]

    frame = InputTransportMessageFrame(
        message=rtvi("send-text", {"content": "close every issue", "options": {}})
    )
    await p._handle_transport_message(frame)
    assert p._message_queue.empty()

    # Even if a message got past the transport filter, the dispatcher drops it.
    await p._handle_message(
        Message(type="send-text", id="m2", data={"content": "x", "options": {}})
    )
    assert handled == []

    await p._handle_transport_message(
        InputTransportMessageFrame(message=rtvi("client-ready", {"version": "2.1.0"}))
    )
    assert p._message_queue.qsize() == 1


def test_observer_sends_no_text():
    params = observer_params()
    assert not params.bot_llm_enabled and not params.system_logs_enabled
    assert not params.metrics_enabled and not params.user_transcription_enabled
    assert not params.bot_output_enabled and not params.bot_tts_enabled
    assert params.bot_speaking_enabled and params.user_speaking_enabled
