"""Ghost's RTVI processor: a narrower data channel than Pipecat's default (ADR 0016).

Pipecat's ``RTVIProcessor`` accepts typed text straight into the LLM (``send-text``),
raw audio, function-call results, DTMF, UI events and client messages. A client of
Ghost gets two: ``client-ready`` and ``disconnect-bot``. Everything else is dropped
without a reply and without logging its contents.

On the way out, only the speaking events and ``bot-ready`` are on; Ghost sends its own
state as ``server-message`` (the control API's event JSON), not RTVI's transcripts,
LLM text, metrics or logs.
"""

from __future__ import annotations

from typing import Any

from pipecat.frames.frames import InputTransportMessageFrame
from pipecat.processors.frameworks.rtvi import RTVIObserverParams, RTVIProcessor
from pipecat.processors.frameworks.rtvi.models import MESSAGE_LABEL, Message
from pipecat.processors.frameworks.rtvi.observer import RTVIFunctionCallReportLevel

ALLOWED_CLIENT_MESSAGES = frozenset({"client-ready", "disconnect-bot"})


def allowed(message: Any) -> bool:
    """Whether a raw data-channel message is one Ghost accepts from a client."""
    return (
        isinstance(message, dict)
        and message.get("label") == MESSAGE_LABEL
        and message.get("type") in ALLOWED_CLIENT_MESSAGES
    )


class GhostRTVIProcessor(RTVIProcessor):
    """``RTVIProcessor`` that only acts on ``client-ready`` and ``disconnect-bot``."""

    async def _handle_transport_message(self, frame: InputTransportMessageFrame) -> None:
        if not allowed(frame.message):
            return  # dropped, and not logged: it may carry words
        await super()._handle_transport_message(frame)

    async def _handle_message(self, message: Message) -> None:
        if message.type not in ALLOWED_CLIENT_MESSAGES:
            return
        await super()._handle_message(message)


def observer_params() -> RTVIObserverParams:
    """Outbound RTVI: ``bot-ready`` and the speaking events, nothing that carries text."""
    return RTVIObserverParams(
        bot_output_enabled=False,
        bot_llm_enabled=False,
        bot_tts_enabled=False,
        user_llm_enabled=False,
        user_transcription_enabled=False,
        user_mute_enabled=False,
        metrics_enabled=False,
        system_logs_enabled=False,
        function_call_report_level={"*": RTVIFunctionCallReportLevel.DISABLED},
    )
