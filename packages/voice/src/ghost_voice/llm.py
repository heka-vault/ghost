"""The model client Ghost uses everywhere, and the one connection it shares across sessions.

``model_client(target)`` is the interface (ADR 0037): the session's Pipecat service, a
one-shot completion (a watch's model step), and a probe that times the first token (the
desktop's Test connection). Two implementations sit behind it, picked by the
connection's protocol: ``AnthropicClient`` for the Messages API and ``OpenAIClient`` for
OpenAI-compatible Chat Completions (OpenAI, Ollama, LM Studio, a URL of the operator's).
Pipecat's adapters translate the session's context and tool calls both ways.

Both session services carry the same fixes on top of Pipecat:

- Every tool call in a reply runs. Pipecat 1.12's Anthropic service keeps only the last
  ``tool_use`` block of a streamed reply, and its OpenAI service reads only the first tool
  call of each chunk (Ollama sends a reply's calls in one chunk), while the prompt asks the
  model to make independent lookups together in one reply; the others were lost without a
  word. The service watches the stream itself and hands Pipecat the whole list.
- A reply that hits the output cap (``GHOST_LLM_MAX_TOKENS``) is not dropped silently.
  Anthropic ends it with ``stop_reason: max_tokens`` and Chat Completions with
  ``finish_reason: length``; Pipecat runs no calls on the first and the complete ones on
  the second. On 2026-10-24 four comments the operator had just confirmed were never
  posted, and Ghost went quiet. Now the tool calls of a cut reply are not run (half a
  batch is worse than none); the model is told, once, to redo it in smaller steps, and a
  second cut is said aloud instead. Either way the turn is flagged ``cut_off``.

The Anthropic service also puts a prompt-cache breakpoint on the system prompt. Pipecat
marks the last two user messages, which caches a conversation as it grows; but a new
session starts with new words, so nothing it wrote before is a prefix of the next
session's request, and the tool schemas plus the system prompt (about 3k tokens) were
written again at every session's first call. With the marker that prefix is read back
instead (docs/benchmarks/2026-10-19-latency.md). OpenAI caches a long prefix by itself.

``shared_client`` keeps one SDK client per connection and event loop, so a session
doesn't open a new connection (and an HTTPS handshake) on its first call.
"""

from __future__ import annotations

import asyncio
import json
import time
from abc import ABC, abstractmethod
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
from pipecat.services.llm_service import LLMService
from pipecat.services.openai.llm import OpenAILLMService

from ghost_voice.config import LLMTarget
from ghost_voice.thinking import ThinkingStartFrame

CACHE_CONTROL = {"type": "ephemeral"}
SHARE_CLIENT = True  # the harness turns it off to measure the difference
NO_KEY = "none"  # sent where a server takes no key; the SDKs refuse an empty one

# Told to the model (as the operator's side of the conversation; Anthropic has no system
# turn mid-conversation) when its reply was cut off with tool calls in it.
CUT_OFF_NOTE = (
    "Your last reply was cut off at the output limit before it finished, and nothing in it "
    "was done. Do it again in smaller steps: one or two tool calls per reply, and keep any "
    "text short."
)
# Said when the redo was cut off too.
CUT_OFF_LINE = "That was more than I can do in one go. Ask me for part of it."

_clients: dict[tuple[str, int], Any] = {}


def _new_client(target: LLMTarget) -> Any:
    extra: dict[str, Any] = {}
    if target.headers:
        extra["default_headers"] = dict(target.headers)
    if target.timeout_secs:
        extra["timeout"] = target.timeout_secs
    if target.protocol == "openai":
        from openai import AsyncOpenAI

        return AsyncOpenAI(api_key=target.api_key or NO_KEY, base_url=target.base_url, **extra)
    # base_url always given: the SDK would otherwise take ANTHROPIC_BASE_URL from the
    # environment, which an older setup points at a proxy.
    return AsyncAnthropic(api_key=target.api_key or NO_KEY, base_url=target.base_url, **extra)


def shared_client(target: LLMTarget) -> Any:
    """The process's SDK client for ``target`` (``AsyncAnthropic`` or ``AsyncOpenAI``).
    Kept per event loop: an HTTP connection pool belongs to the loop that opened it."""
    if not SHARE_CLIENT:
        return _new_client(target)
    try:
        loop = id(asyncio.get_running_loop())
    except RuntimeError:
        loop = 0
    key = (target.fingerprint(), loop)
    client = _clients.get(key)
    if client is None:
        client = _new_client(target)
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

    def parse(self) -> bool:
        text = "".join(self.json)
        try:
            args = json.loads(text) if text.strip() else {}
        except ValueError:
            return False
        if not isinstance(args, dict):
            return False
        self.args = args
        return True


@dataclass
class _Reply:
    """What one streamed reply contained: its complete tool calls in order, whether the cap
    fell inside one, and why it stopped."""

    context: Any
    calls: list[_ToolUse] = field(default_factory=list)
    cut_call: bool = False
    stop_reason: str | None = None
    watched: bool = False

    CUT_REASONS = ("max_tokens",)

    @property
    def cut_off(self) -> bool:
        return self.stop_reason in self.CUT_REASONS

    @property
    def lost(self) -> int:
        """Tool calls a cut reply was making."""
        return len(self.calls) + int(self.cut_call)


@dataclass
class _AnthropicReply(_Reply):
    """Messages API events: ``tool_use`` blocks, then ``message_delta`` with the stop."""

    _open: _ToolUse | None = None

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
        if call.parse():
            self.calls.append(call)
        else:
            self.cut_call = True  # the stream ended inside the arguments


class _WatchedStream:
    """A Chat Completions stream that tells its reply every chunk, and closes like one."""

    def __init__(self, stream: Any, reply: _OpenAIReply) -> None:
        self._stream = stream
        self._reply = reply

    def __aiter__(self) -> AsyncIterator[Any]:
        return self._chunks()

    async def _chunks(self) -> AsyncIterator[Any]:
        async for chunk in self._stream:
            self._reply.note(chunk)
            yield chunk
        self._reply.finish()

    async def close(self) -> None:
        close = getattr(self._stream, "close", None) or getattr(self._stream, "aclose", None)
        if close is not None:
            await close()


@dataclass
class _OpenAIReply(_Reply):
    """Chat Completions chunks: tool calls stream by ``index`` (one chunk may carry several,
    whole), and the last choice says ``finish_reason``; ``length`` is the cap."""

    CUT_REASONS = ("length", "max_tokens")
    _open: dict[int, _ToolUse] = field(default_factory=dict)
    _finished: bool = False

    def watch(self, stream: Any) -> _WatchedStream:
        self.watched = True
        return _WatchedStream(stream, self)

    def note(self, chunk: Any) -> None:
        for choice in getattr(chunk, "choices", None) or []:
            if getattr(choice, "index", 0) not in (0, None):
                continue
            delta = getattr(choice, "delta", None)
            for tc in getattr(delta, "tool_calls", None) or []:
                index = tc.index if getattr(tc, "index", None) is not None else len(self._open)
                call = self._open.get(index)
                if call is None:
                    call = self._open[index] = _ToolUse(id="", name="")
                if getattr(tc, "id", None) and not call.id:
                    call.id = tc.id
                fn = getattr(tc, "function", None)
                name = getattr(fn, "name", None) if fn is not None else None
                # The name comes once; a server that repeats it in every chunk is not
                # allowed to turn ``list_issues`` into ``list_issueslist_issues``.
                if name and name != call.name:
                    call.name += name
                args = getattr(fn, "arguments", None) if fn is not None else None
                if args:
                    call.json.append(args)
            reason = getattr(choice, "finish_reason", None)
            if reason:
                self.stop_reason = reason

    def finish(self) -> None:
        if self._finished:
            return
        self._finished = True
        for index in sorted(self._open):
            call = self._open[index]
            call.id = call.id or f"call_{index}"
            if call.name and call.parse():
                self.calls.append(call)
            else:
                self.cut_call = True
        if self.cut_call and not self.stop_reason:
            self.stop_reason = "length"  # the stream ended inside a call's arguments
        elif self.cut_call and not self.cut_off:
            logger.warning("model: a tool call's arguments weren't JSON; it was not run")
            self.cut_call = False


class _CutOffRecovery:
    """Every tool call of a reply, and a redo in smaller steps when a reply hits the cap.
    Mixed into both session services, ahead of Pipecat's."""

    _reply_type: type[_Reply] = _Reply
    # Pipecat's Anthropic service sends the reply's end frame from _process_context; its
    # OpenAI service sends it from process_frame, after _process_context has returned.
    _recover_after_frame = False

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self._reply: Any = None  # the reply being streamed
        self._redoing = False  # the reply in flight is the redo of a cut one
        self._cut: _Reply | None = None  # a cut reply waiting for its end frame

    def _output_cap(self) -> Any:
        return self._settings.max_tokens  # type: ignore[attr-defined]

    async def _process_context(self, context) -> None:  # type: ignore[override]
        self._reply = reply = self._reply_type(context)
        try:
            await super()._process_context(context)  # type: ignore[misc]
        finally:
            self._reply = None
        if not reply.cut_off:
            self._redoing = False
            return
        if self._recover_after_frame:
            self._cut = reply  # after the reply's end frame, which process_frame sends
            return
        await self._recover(reply)

    async def run_function_calls(self, function_calls: Sequence[FunctionCallFromLLM]) -> None:
        reply = self._reply
        if reply is None or not reply.watched:
            await super().run_function_calls(function_calls)  # type: ignore[misc]
            return
        if isinstance(reply, _OpenAIReply):
            reply.finish()  # the stream has ended when Pipecat gets here
        if reply.cut_off:
            # Not run: ``_recover`` asks for the whole batch again.
            return
        if reply.calls:
            # Ours, whole: Pipecat kept the last block (Anthropic) or the first call of
            # each chunk (Chat Completions).
            function_calls = [call.as_function_call(reply.context) for call in reply.calls]
        await super().run_function_calls(function_calls)  # type: ignore[misc]

    async def _recover(self, reply: _Reply) -> None:
        # Pushed after the reply's end frame, which ``super()._process_context`` sent, so
        # the aggregator closes the cut reply before the note lands behind it. A held
        # (speculative) reply holds these too, and discards them with it.
        push = self.push_frame  # type: ignore[attr-defined]
        await push(LLMReplyCutFrame(tool_calls=reply.lost))
        cap = self._output_cap()
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
            await push(TTSSpeakFrame(CUT_OFF_LINE))
            return
        self._redoing = True
        logger.warning(
            f"{self}: reply cut off at {cap} output tokens with {reply.lost} tool calls in "
            "it; none ran, asking for it in smaller steps"
        )
        await push(ThinkingStartFrame())
        await push(
            LLMMessagesAppendFrame(
                messages=[{"role": "user", "content": CUT_OFF_NOTE}], run_llm=True
            )
        )


class GhostAnthropicLLMService(_CutOffRecovery, AnthropicLLMService):
    """Pipecat's Anthropic service plus a cache breakpoint on the system prompt, every tool
    call of a reply, and a redo in smaller steps when a reply hits the output cap."""

    cache_system = True  # the harness turns it off to measure the difference
    _reply_type = _AnthropicReply

    def _get_llm_invocation_params(self, context):  # type: ignore[override]
        params = super()._get_llm_invocation_params(context)
        if self.cache_system and self._settings.enable_prompt_caching:
            params = with_system_breakpoint(dict(params))
        return params

    async def _create_message_stream(self, api_call, params):  # type: ignore[override]
        stream = await super()._create_message_stream(api_call, params)
        if self._reply is None:
            return stream
        return self._reply.watch(stream)


class GhostOpenAILLMService(_CutOffRecovery, OpenAILLMService):
    """Pipecat's OpenAI-compatible service with the shared client, every tool call of a
    reply, and a redo in smaller steps when a reply hits the output cap."""

    _reply_type = _OpenAIReply
    _recover_after_frame = True

    async def process_frame(self, frame, direction):  # type: ignore[override]
        await super().process_frame(frame, direction)
        cut, self._cut = self._cut, None
        if cut is not None:
            await self._recover(cut)

    def __init__(self, *args: Any, client: Any = None, **kwargs: Any) -> None:
        self._given_client = client  # read by create_client, which Pipecat's init calls
        super().__init__(*args, **kwargs)

    def create_client(self, api_key=None, base_url=None, **kwargs):  # type: ignore[override]
        if self._given_client is not None:
            return self._given_client
        return super().create_client(api_key=api_key, base_url=base_url, **kwargs)

    def _output_cap(self) -> Any:
        s = self._settings
        return s.max_tokens if isinstance(s.max_tokens, int) else s.max_completion_tokens

    async def get_chat_completions(self, context):  # type: ignore[override]
        stream = await super().get_chat_completions(context)
        if self._reply is None:
            return stream
        return self._reply.watch(stream)


# --- the interface ----------------------------------------------------------------------


@dataclass
class Probe:
    """One tiny request, timed the way ``make perf-llm`` times it: from just before the
    streaming call to its first content event (a text or tool block on the Messages API, a
    chunk with text or a tool call on Chat Completions)."""

    ttft: float | None = None  # seconds
    total: float | None = None
    text: str = ""
    model: str = ""  # as the provider answered, when it says
    error: str | None = None


def _error(e: BaseException) -> str:
    """An SDK error in a few words. Status codes and the class, never a header or a key."""
    status = getattr(e, "status_code", None)
    name = type(e).__name__
    body = getattr(e, "body", None)
    detail = ""
    if isinstance(body, dict):
        err = body.get("error")
        msg = err.get("message") if isinstance(err, dict) else body.get("message")
        if isinstance(msg, str):
            detail = msg
    return (f"{name} (HTTP {status})" if status else name) + (f": {detail[:160]}" if detail else "")


class ModelClient(ABC):
    """Ghost's model, whichever protocol reaches it."""

    protocol: str

    def __init__(self, target: LLMTarget, *, share: bool = True) -> None:
        self.target = target
        self.client = shared_client(target) if share else _new_client(target)

    @property
    @abstractmethod
    def settings_type(self) -> type:
        """The Pipecat settings class, for a mid-session settings update (a transfer)."""

    @abstractmethod
    def service(self, *, system_instruction: str, max_tokens: int, thinking: str) -> LLMService:
        """The session's Pipecat service, with every robustness fix."""

    @abstractmethod
    async def complete(self, *, system: str, user: str, max_tokens: int) -> str:
        """One short answer, not streamed."""

    @abstractmethod
    async def probe(self, *, prompt: str, max_tokens: int, thinking: str | None) -> Probe:
        """Stream one tiny request and time its first token."""


class AnthropicClient(ModelClient):
    protocol = "anthropic"

    @property
    def settings_type(self) -> type:
        return AnthropicLLMService.Settings

    def service(self, *, system_instruction: str, max_tokens: int, thinking: str) -> LLMService:
        t = self.target
        return GhostAnthropicLLMService(
            api_key=t.api_key or NO_KEY,
            client=self.client,
            settings=AnthropicLLMService.Settings(
                model=t.model,
                system_instruction=system_instruction,
                max_tokens=max_tokens,
                enable_prompt_caching=True,
                thinking=AnthropicLLMService.ThinkingConfig(type=thinking),
            ),
        )

    async def complete(self, *, system: str, user: str, max_tokens: int) -> str:
        msg = await self.client.messages.create(
            model=self.target.model,
            max_tokens=max_tokens,
            system=system,
            messages=[{"role": "user", "content": user}],
        )
        return "".join(getattr(b, "text", "") for b in msg.content)

    async def probe(self, *, prompt: str, max_tokens: int, thinking: str | None) -> Probe:
        p = Probe()
        params: dict[str, Any] = {
            "model": self.target.model,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": prompt}],
            "stream": True,
        }
        if thinking:
            params["thinking"] = {"type": thinking}
        text: list[str] = []
        began = time.perf_counter()
        try:
            stream = await self.client.messages.create(**params)
            async for ev in stream:
                if ev.type in ("content_block_start", "content_block_delta") and p.ttft is None:
                    p.ttft = time.perf_counter() - began
                if ev.type == "message_start":
                    p.model = getattr(ev.message, "model", "") or ""
                elif ev.type == "content_block_delta" and getattr(ev.delta, "text", None):
                    text.append(ev.delta.text)
            p.total = time.perf_counter() - began
        except Exception as e:  # noqa: BLE001 - the probe reports, it doesn't raise
            p.error = _error(e)
        p.text = "".join(text).strip()
        return p


class OpenAIClient(ModelClient):
    protocol = "openai"

    @property
    def settings_type(self) -> type:
        return OpenAILLMService.Settings

    def service(self, *, system_instruction: str, max_tokens: int, thinking: str) -> LLMService:
        t = self.target
        return GhostOpenAILLMService(
            api_key=t.api_key or NO_KEY,
            base_url=t.base_url,
            client=self.client,
            settings=OpenAILLMService.Settings(
                model=t.model,
                system_instruction=system_instruction,
                **{t.cap_param: max_tokens},
            ),
        )

    async def complete(self, *, system: str, user: str, max_tokens: int) -> str:
        r = await self.client.chat.completions.create(
            model=self.target.model,
            messages=[{"role": "system", "content": system}, {"role": "user", "content": user}],
            **{self.target.cap_param: max_tokens},
        )
        return (r.choices[0].message.content or "") if r.choices else ""

    async def probe(self, *, prompt: str, max_tokens: int, thinking: str | None) -> Probe:
        p = Probe()
        text: list[str] = []
        began = time.perf_counter()
        try:
            stream = await self.client.chat.completions.create(
                model=self.target.model,
                messages=[{"role": "user", "content": prompt}],
                stream=True,
                **{self.target.cap_param: max_tokens},
            )
            async for chunk in stream:
                if getattr(chunk, "model", None):
                    p.model = chunk.model
                if not chunk.choices:
                    continue
                d = chunk.choices[0].delta
                if d and (d.content or d.tool_calls) and p.ttft is None:
                    p.ttft = time.perf_counter() - began
                if d and d.content:
                    text.append(d.content)
            p.total = time.perf_counter() - began
        except Exception as e:  # noqa: BLE001
            p.error = _error(e)
        p.text = "".join(text).strip()
        return p


def model_client(target: LLMTarget, *, share: bool = True) -> ModelClient:
    """The client for ``target``'s protocol. ``share=False`` for a one-off (a connection
    being tried out) that shouldn't stay in the process's pool."""
    cls = OpenAIClient if target.protocol == "openai" else AnthropicClient
    return cls(target, share=share)
