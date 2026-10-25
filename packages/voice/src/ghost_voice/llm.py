"""The model service Ghost uses, and the one client it shares across sessions.

``GhostAnthropicLLMService`` is Pipecat's Anthropic service with three things on top.

- A prompt-cache breakpoint on the system prompt. Pipecat marks the last two user
  messages, which caches a conversation as it grows; but a new session starts with new
  words, so nothing it wrote before is a prefix of the next session's request, and the tool
  schemas plus the system prompt (about 3k tokens) were written again at every session's
  first call. With the marker that prefix is read back instead
  (docs/benchmarks/2026-10-19-latency.md). Three breakpoints of the four Anthropic allows.
- Every tool call in a reply runs. Pipecat 1.12 keeps only the last ``tool_use`` block of
  a streamed reply, while the prompt asks the model to make independent lookups together
  in one reply; the earlier ones were lost without a word. The service watches the stream
  itself and hands Pipecat the whole list.
- A reply that hits the output cap (``GHOST_LLM_MAX_TOKENS``) is not dropped silently.
  Anthropic ends it with ``stop_reason: max_tokens``, and Pipecat runs tool calls only on
  ``tool_use``, so the calls it held were lost and nothing was said: on 2026-10-24 four
  comments the operator had just confirmed were never posted, and Ghost went quiet. Now the
  tool calls of a cut reply are not run (half a batch is worse than none); the model is
  told, once, to redo it in smaller steps, and a second cut is said aloud instead.

``shared_client`` keeps one ``AsyncAnthropic`` per endpoint, key and event loop, so a
session doesn't open a new connection (and an HTTPS handshake) on its first call.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
from collections.abc import AsyncIterator, Sequence
from dataclasses import dataclass, field
from typing import Any

from anthropic import AsyncAnthropic
from loguru import logger
from pipecat.frames.frames import (
    DataFrame,
    FunctionCallFromLLM,
    LLMMessagesAppendFrame,
    TTSSpeakFrame,
)
from pipecat.services.anthropic.llm import AnthropicLLMService

from ghost_voice.config import LLMTarget
from ghost_voice.thinking import ThinkingStartFrame

CACHE_CONTROL = {"type": "ephemeral"}
SHARE_CLIENT = True  # the harness turns it off to measure the difference

# Told to the model (as the operator's side of the conversation; Anthropic has no system
# turn mid-conversation) when its reply was cut off with tool calls in it.
CUT_OFF_NOTE = (
    "Your last reply was cut off at the output limit before it finished, and nothing in it "
    "was done. Do it again in smaller steps: one or two tool calls per reply, and keep any "
    "text short."
)
# Said when the redo was cut off too.
CUT_OFF_LINE = "That was more than I can do in one go. Ask me for part of it."

_clients: dict[tuple[str | None, str, int], AsyncAnthropic] = {}


def shared_client(target: LLMTarget) -> AsyncAnthropic:
    """The process's client for ``target``. Kept per event loop: an HTTP connection pool
    belongs to the loop that opened it."""
    if not SHARE_CLIENT:
        return AsyncAnthropic(api_key=target.api_key, base_url=target.base_url)
    try:
        loop = id(asyncio.get_running_loop())
    except RuntimeError:
        loop = 0
    key = (target.base_url, hashlib.sha256(target.api_key.encode()).hexdigest(), loop)
    client = _clients.get(key)
    if client is None:
        client = AsyncAnthropic(api_key=target.api_key, base_url=target.base_url)
        _clients[key] = client
    return client


def with_system_breakpoint(params: dict[str, Any]) -> dict[str, Any]:
    """Turn a string system prompt into one text block carrying a cache marker."""
    system = params.get("system")
    if isinstance(system, str) and system:
        params["system"] = [{"type": "text", "text": system, "cache_control": dict(CACHE_CONTROL)}]
    return params


@dataclass
class LLMReplyCutFrame(DataFrame):
    """A reply stopped at the output cap. ``tool_calls`` is how many it was making, the cut
    one included; none of them ran. The latency probe flags the turn ``cut_off``."""

    tool_calls: int = 0


@dataclass
class _ToolUse:
    id: str
    name: str
    json: list[str] = field(default_factory=list)
    args: dict[str, Any] = field(default_factory=dict)

    def as_function_call(self, context: Any) -> FunctionCallFromLLM:
        return FunctionCallFromLLM(
            context=context, tool_call_id=self.id, function_name=self.name, arguments=self.args
        )


@dataclass
class _Reply:
    """What one streamed reply contained: its complete ``tool_use`` blocks in order, whether
    the cap fell inside one, and the stop reason."""

    context: Any
    calls: list[_ToolUse] = field(default_factory=list)
    cut_call: bool = False
    stop_reason: str | None = None
    watched: bool = False
    _open: _ToolUse | None = None

    @property
    def cut_off(self) -> bool:
        return self.stop_reason == "max_tokens"

    @property
    def lost(self) -> int:
        """Tool calls a cut reply was making."""
        return len(self.calls) + int(self.cut_call)

    async def watch(self, stream: AsyncIterator[Any]) -> AsyncIterator[Any]:
        self.watched = True
        async for event in stream:
            self.note(event)
            yield event

    def note(self, event: Any) -> None:
        kind = getattr(event, "type", None)
        if kind == "content_block_start":
            block = event.content_block
            if getattr(block, "type", None) == "tool_use":
                self._open = _ToolUse(id=block.id, name=block.name)
        elif kind == "content_block_delta":
            partial = getattr(event.delta, "partial_json", None)
            if partial is not None and self._open is not None:
                self._open.json.append(partial)
        elif kind == "content_block_stop":
            self._close()
        elif kind == "message_delta":
            reason = getattr(getattr(event, "delta", None), "stop_reason", None)
            if reason:
                self.stop_reason = reason
            if self._open is not None:  # the message ended inside a block
                self.cut_call = True
                self._open = None

    def _close(self) -> None:
        if self._open is None:
            return
        call, self._open = self._open, None
        text = "".join(call.json)
        try:
            call.args = json.loads(text) if text.strip() else {}
        except ValueError:
            self.cut_call = True  # the stream ended inside the arguments
        else:
            self.calls.append(call)


class GhostAnthropicLLMService(AnthropicLLMService):
    """Pipecat's Anthropic service plus a cache breakpoint on the system prompt, every tool
    call of a reply, and a redo in smaller steps when a reply hits the output cap."""

    cache_system = True  # the harness turns it off to measure the difference

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self._reply: _Reply | None = None  # the reply being streamed
        self._redoing = False  # the reply in flight is the redo of a cut one

    def _get_llm_invocation_params(self, context):  # type: ignore[override]
        params = super()._get_llm_invocation_params(context)
        if self.cache_system and self._settings.enable_prompt_caching:
            params = with_system_breakpoint(dict(params))
        return params

    async def _process_context(self, context) -> None:  # type: ignore[override]
        self._reply = reply = _Reply(context)
        try:
            await super()._process_context(context)
        finally:
            self._reply = None
        if not reply.cut_off:
            self._redoing = False
            return
        await self._recover(reply)

    async def _create_message_stream(self, api_call, params):  # type: ignore[override]
        stream = await super()._create_message_stream(api_call, params)
        if self._reply is None:
            return stream
        return self._reply.watch(stream)

    async def run_function_calls(self, function_calls: Sequence[FunctionCallFromLLM]) -> None:
        reply = self._reply
        if reply is None or not reply.watched:
            await super().run_function_calls(function_calls)
            return
        if reply.cut_off:
            # Pipecat hands over nothing here (no ``tool_use`` stop), and the complete calls
            # are not run either: ``_recover`` asks for the whole batch again.
            return
        if len(reply.calls) > len(function_calls):
            # Pipecat kept the last block only.
            function_calls = [call.as_function_call(reply.context) for call in reply.calls]
        await super().run_function_calls(function_calls)

    async def _recover(self, reply: _Reply) -> None:
        # Pushed after the reply's end frame, which ``super()._process_context`` sent, so
        # the aggregator closes the cut reply before the note lands behind it. A held
        # (speculative) reply holds these too, and discards them with it.
        await self.push_frame(LLMReplyCutFrame(tool_calls=reply.lost))
        cap = self._settings.max_tokens
        if not reply.lost:
            # A long answer, cut short: the operator heard what there was of it.
            logger.warning(f"{self}: reply cut off at {cap} output tokens (text only)")
            return
        if self._redoing:
            self._redoing = False
            logger.warning(
                f"{self}: the redo was cut off at {cap} output tokens too "
                f"({reply.lost} tool calls); saying so"
            )
            await self.push_frame(TTSSpeakFrame(CUT_OFF_LINE))
            return
        self._redoing = True
        logger.warning(
            f"{self}: reply cut off at {cap} output tokens with {reply.lost} tool calls in "
            "it; none ran, asking for it in smaller steps"
        )
        await self.push_frame(ThinkingStartFrame())
        await self.push_frame(
            LLMMessagesAppendFrame(
                messages=[{"role": "user", "content": CUT_OFF_NOTE}], run_llm=True
            )
        )
