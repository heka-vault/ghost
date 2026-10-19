"""The model service Ghost uses, and the one client it shares across sessions.

``GhostAnthropicLLMService`` is Pipecat's Anthropic service with one more prompt-cache
breakpoint, on the system prompt. Pipecat marks the last two user messages, which caches
a conversation as it grows; but a new session starts with new words, so nothing it wrote
before is a prefix of the next session's request, and the tool schemas plus the system
prompt (about 3k tokens) were written again at every session's first call. With the
marker that prefix is read back instead (docs/benchmarks/2026-10-19-latency.md). Three
breakpoints of the four Anthropic allows.

``shared_client`` keeps one ``AsyncAnthropic`` per endpoint, key and event loop, so a
session doesn't open a new connection (and an HTTPS handshake) on its first call.
"""

from __future__ import annotations

import asyncio
import hashlib
from typing import Any

from anthropic import AsyncAnthropic
from pipecat.services.anthropic.llm import AnthropicLLMService

from ghost_voice.config import LLMTarget

CACHE_CONTROL = {"type": "ephemeral"}
SHARE_CLIENT = True  # the harness turns it off to measure the difference

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


class GhostAnthropicLLMService(AnthropicLLMService):
    """Pipecat's Anthropic service plus a cache breakpoint on the system prompt."""

    cache_system = True  # the harness turns it off to measure the difference

    def _get_llm_invocation_params(self, context):  # type: ignore[override]
        params = super()._get_llm_invocation_params(context)
        if self.cache_system and self._settings.enable_prompt_caching:
            params = with_system_breakpoint(dict(params))
        return params
