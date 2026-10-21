"""Model and endpoint latency with Ghost's real system prompt and tool schemas.

Three probes per (endpoint, model), each run ``n`` times, streamed the way Pipecat
streams (cache markers on the last two user messages, as its Anthropic adapter does):

- ``plain``: a hello, no tool. TTFB, total, and the stream span (first byte to last;
  near zero with many output tokens means the endpoint buffered the reply).
- ``tool``: "what needs my sign-off": the call to list_awaiting_signoff, then the answer
  after a canned tool result. Time to the complete tool call, then the answer's TTFB.
- caching: every repeat after the first should read the prefix from the cache.

Endpoints: ``proxy`` is ANTHROPIC_BASE_URL with ANTHROPIC_AUTH_TOKEN (Ghost's own),
``openrouter`` is OpenRouter's Anthropic Messages API with OPENROUTER_API_KEY. Non-
Anthropic models go through OpenRouter's OpenAI-compatible API. Keys are never printed.
Spend is small: a few thousand input tokens per call, mostly cached.
"""

from __future__ import annotations

import json
import os
import statistics
import tempfile
import time
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.mcp import Servers
from ghost_voice import VoiceSettings
from ghost_voice.config import OPENROUTER_ANTHROPIC_URL as OPENROUTER_ANTHROPIC
from ghost_voice.config import openrouter_model
from ghost_voice.prompt import build_system_prompt
from ghost_voice.tools import register_tools
from pipecat.adapters.schemas.tools_schema import ToolsSchema
from pipecat.adapters.services.anthropic_adapter import AnthropicLLMAdapter
from pipecat.adapters.services.open_ai_adapter import OpenAILLMAdapter
from pipecat.services.anthropic.llm import AnthropicLLMService

from ghostd.config import GhostdSettings
from ghostd.perf.harness import thinking_for

OPENROUTER_OPENAI = "https://openrouter.ai/api/v1"
HELLOS = (
    "Hi Ghost, how's it going?",
    "Hey Ghost, you there?",
    "Morning, Ghost.",
    "Hello Ghost, what's up?",
    "Ghost, are you awake?",
    "Hey, good afternoon.",
)
ASKS = (
    "What needs my sign-off?",
    "Anything waiting for my sign-off?",
    "What's waiting on me to sign off?",
    "Which issues need my sign-off right now?",
    "Is there anything for me to sign off on?",
    "What do I have to sign off today?",
)
TOOL_RESULT = {
    "count": 3,
    "issues": [
        {
            "repo": "ghost-mock/infra",
            "number": 17,
            "title": "Rotate the Discord webhook",
            "labels": ["needs-signoff", "security"],
            "updated_at": "2026-10-16T09:00:00Z",
        },
        {
            "repo": "ghost-mock/app",
            "number": 151,
            "title": "Add dark mode toggle to settings",
            "labels": ["needs-signoff", "frontend"],
            "updated_at": "2026-10-16T09:00:00Z",
        },
        {
            "repo": "ghost-mock/app",
            "number": 142,
            "title": "Choose Redis or Postgres for the session cache",
            "labels": ["needs-signoff", "backend"],
            "updated_at": "2026-10-16T09:00:00Z",
        },
    ],
}


@dataclass
class Sample:
    ttfb: float | None = None
    total: float | None = None
    span: float | None = None  # first byte -> last byte
    tool_done: float | None = None  # the tool call complete (stop_reason tool_use)
    usage: dict[str, int] = field(default_factory=dict)
    tool: str | None = None
    words: int | None = None  # in the spoken answer
    error: str | None = None


async def ghost_prompt_and_tools() -> tuple[str, list[Any]]:
    """The system prompt and tool schemas a real session sends, from the mock GitHub
    server the example policy declares."""
    tmp = Path(tempfile.mkdtemp(prefix="ghost-perf-llm-"))
    store = Store(tmp / "ghost.db")
    autonomy = AutonomyPolicy.load(GhostdSettings().infra_dir / "policy.example.toml")
    github = autonomy.mcp["github"]
    servers = Servers({"github": replace(github, args=("--state", str(tmp / "mock-github.json")))})
    await servers.connect()
    sources = servers.sources()
    gate = WriteGate(store, autonomy=autonomy)
    for a in sources:
        gate.register(a.name, a.operations)
    llm = AnthropicLLMService(api_key="unused")

    async def _sleep() -> None: ...

    schemas = register_tools(llm, sources=sources, gate=gate, session_id="x", on_hang_up=_sleep)
    prompt = build_system_prompt([a.prompt for a in sources if a.prompt])
    await servers.close()
    store.close()
    return prompt, schemas


def _marked(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Pipecat's cache markers: the last two user messages get cache_control."""
    out = json.loads(json.dumps(messages))
    users = [i for i, m in enumerate(out) if m["role"] == "user"][-2:]
    for i in users:
        c = out[i]["content"]
        if isinstance(c, str):
            out[i]["content"] = c = [{"type": "text", "text": c}]
        c[-1]["cache_control"] = {"type": "ephemeral"}
    return out


async def _anthropic_call(
    client, model: str, system: str, tools, messages, thinking, *, system_breakpoint: bool = False
) -> tuple[Sample, list[Any]]:
    s = Sample()
    params: dict[str, Any] = {
        "model": model,
        "max_tokens": 400,
        # Pipecat sends the system prompt as a string with no cache marker; with
        # system_breakpoint it carries one, so tools + system are cached across sessions.
        "system": (
            [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}]
            if system_breakpoint
            else system
        ),
        "tools": tools,
        "messages": _marked(messages),
        "stream": True,
    }
    if thinking:
        params["thinking"] = {"type": thinking}
    began = time.perf_counter()
    blocks: list[Any] = []
    text: list[str] = []
    partial = ""
    last = None
    try:
        stream = await client.messages.create(**params)
        async for ev in stream:
            now = time.perf_counter()
            if ev.type in ("content_block_start", "content_block_delta"):
                if s.ttfb is None:
                    s.ttfb = now - began
                last = now
            if ev.type == "message_start":
                u = ev.message.usage
                s.usage = {
                    "in": u.input_tokens or 0,
                    "cache_read": getattr(u, "cache_read_input_tokens", 0) or 0,
                    "cache_write": getattr(u, "cache_creation_input_tokens", 0) or 0,
                }
            elif ev.type == "content_block_start" and ev.content_block.type == "tool_use":
                s.tool = ev.content_block.name
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": ev.content_block.id,
                        "name": ev.content_block.name,
                        "input": {},
                    }
                )
                partial = ""
            elif ev.type == "content_block_delta":
                if getattr(ev.delta, "text", None):
                    text.append(ev.delta.text)
                elif getattr(ev.delta, "partial_json", None):
                    partial += ev.delta.partial_json
            elif ev.type == "content_block_stop" and blocks and blocks[-1]["type"] == "tool_use":
                blocks[-1]["input"] = json.loads(partial) if partial else {}
            elif ev.type == "message_delta":
                s.usage["out"] = getattr(ev.usage, "output_tokens", 0) or 0
                if getattr(ev.delta, "stop_reason", None) == "tool_use":
                    s.tool_done = now - began
        s.total = time.perf_counter() - began
        if s.ttfb is not None and last is not None:
            s.span = last - (began + s.ttfb)
    except Exception as e:  # noqa: BLE001
        s.error = f"{type(e).__name__}: {str(e)[:160]}"
    if text:
        s.words = len("".join(text).split())
        blocks.insert(0, {"type": "text", "text": "".join(text)})
    return s, blocks


async def _openai_call(
    client, model: str, system: str, tools, messages
) -> tuple[Sample, dict[str, Any] | None]:
    s = Sample()
    began = time.perf_counter()
    text: list[str] = []
    call: dict[str, Any] | None = None
    last = None
    try:
        stream = await client.chat.completions.create(
            model=model,
            messages=[{"role": "system", "content": system}, *messages],
            tools=tools,
            max_tokens=400,
            stream=True,
            stream_options={"include_usage": True},
        )
        async for chunk in stream:
            now = time.perf_counter()
            if chunk.usage:
                details = getattr(chunk.usage, "prompt_tokens_details", None)
                s.usage = {
                    "in": chunk.usage.prompt_tokens or 0,
                    "out": chunk.usage.completion_tokens or 0,
                    "cache_read": (getattr(details, "cached_tokens", 0) or 0) if details else 0,
                }
            if not chunk.choices:
                continue
            d = chunk.choices[0].delta
            if d and (d.content or d.tool_calls):
                if s.ttfb is None:
                    s.ttfb = now - began
                last = now
            if d and d.content:
                text.append(d.content)
            if d and d.tool_calls:
                for tc in d.tool_calls:
                    if call is None:
                        call = {"id": tc.id or "call_0", "name": "", "arguments": ""}
                    if tc.function and tc.function.name:
                        call["name"] = tc.function.name
                        s.tool = tc.function.name
                    if tc.function and tc.function.arguments:
                        call["arguments"] += tc.function.arguments
            if chunk.choices[0].finish_reason == "tool_calls":
                s.tool_done = now - began
        s.total = time.perf_counter() - began
        if s.ttfb is not None and last is not None:
            s.span = last - (began + s.ttfb)
    except Exception as e:  # noqa: BLE001
        s.error = f"{type(e).__name__}: {str(e)[:160]}"
    if text:
        s.words = len("".join(text).split())
    return s, call


def _p50(xs: list[float | None]) -> float | None:
    v = [x for x in xs if x is not None]
    return statistics.median(v) if v else None


def _p95(xs: list[float | None]) -> float | None:
    v = sorted(x for x in xs if x is not None)
    if not v:
        return None
    return v[min(len(v) - 1, round(0.95 * (len(v) - 1)))]


def _f(x: float | None) -> str:
    return "-" if x is None else f"{x:.2f}"


async def run_llm(
    *,
    models: list[str],
    endpoints: list[str],
    openrouter_models: list[str],
    thinking: str,
    n: int,
    out: Path,
) -> int:
    from anthropic import AsyncAnthropic
    from openai import AsyncOpenAI

    system, schemas = await ghost_prompt_and_tools()
    tools_schema = ToolsSchema(standard_tools=schemas)
    a_tools = AnthropicLLMAdapter().to_provider_tools_format(tools_schema)
    o_tools = OpenAILLMAdapter().to_provider_tools_format(tools_schema)
    voice = VoiceSettings()  # type: ignore[call-arg]
    or_key = os.environ.get("OPENROUTER_API_KEY", "")

    # Every target runs once per round, so a slow minute on one endpoint hits all of them.
    # Each round's lines differ, like a new session's first words: only a prefix up to the
    # system prompt and tools can come from the cache, and only if something marks it.
    targets: list[dict[str, Any]] = []
    for ep in endpoints:
        if ep in ("proxy", "proxy+sysbp"):
            proxy = voice.model_copy(update={"llm_endpoint": "proxy"}).llm_target()
            client = AsyncAnthropic(api_key=proxy.api_key, base_url=proxy.base_url)
            targets += [
                {
                    "ep": ep,
                    "label": m,
                    "wire": m,
                    "client": client,
                    "kind": "anthropic",
                    "thinking": thinking_for(m, None if "haiku" in m else thinking),
                }
                for m in models
            ]
        elif ep in ("openrouter", "openrouter+sysbp") and or_key:
            client = AsyncAnthropic(api_key=or_key, base_url=OPENROUTER_ANTHROPIC)
            targets += [
                {
                    "ep": ep,
                    "label": m,
                    "wire": openrouter_model(m),
                    "client": client,
                    "kind": "anthropic",
                    "thinking": thinking_for(m, None if "haiku" in m else thinking),
                }
                for m in models
            ]
    if openrouter_models and or_key:
        oclient = AsyncOpenAI(api_key=or_key, base_url=OPENROUTER_OPENAI)
        targets += [
            {"ep": "openrouter-openai", "label": m, "wire": m, "client": oclient, "kind": "openai"}
            for m in openrouter_models
        ]
    samples: dict[int, tuple[list[Sample], list[Sample], list[Sample]]] = {
        i: ([], [], []) for i in range(len(targets))
    }
    for rnd in range(n):
        hello, ask = HELLOS[rnd % len(HELLOS)], ASKS[rnd % len(ASKS)]
        for i, t in enumerate(targets):
            plain, tool1, tool2 = samples[i]
            if t["kind"] == "anthropic":
                sysbp = t["ep"].endswith("+sysbp")
                s0, _ = await _anthropic_call(
                    t["client"],
                    t["wire"],
                    system,
                    a_tools,
                    [{"role": "user", "content": hello}],
                    t["thinking"],
                    system_breakpoint=sysbp,
                )
                plain.append(s0)
                s1, blocks = await _anthropic_call(
                    t["client"],
                    t["wire"],
                    system,
                    a_tools,
                    [{"role": "user", "content": ask}],
                    t["thinking"],
                    system_breakpoint=sysbp,
                )
                tool1.append(s1)
                use = next((b for b in blocks if b.get("type") == "tool_use"), None)
                if use is None:
                    continue
                msgs = [
                    {"role": "user", "content": ask},
                    {"role": "assistant", "content": blocks},
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": use["id"],
                                "content": json.dumps(TOOL_RESULT),
                            }
                        ],
                    },
                ]
                s2, _ = await _anthropic_call(
                    t["client"],
                    t["wire"],
                    system,
                    a_tools,
                    msgs,
                    t["thinking"],
                    system_breakpoint=sysbp,
                )
                tool2.append(s2)
            else:
                s0, _ = await _openai_call(
                    t["client"], t["wire"], system, o_tools, [{"role": "user", "content": hello}]
                )
                plain.append(s0)
                s1, call = await _openai_call(
                    t["client"], t["wire"], system, o_tools, [{"role": "user", "content": ask}]
                )
                tool1.append(s1)
                if call is None:
                    continue
                msgs = [
                    {"role": "user", "content": ask},
                    {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "id": call["id"],
                                "type": "function",
                                "function": {
                                    "name": call["name"],
                                    "arguments": call["arguments"] or "{}",
                                },
                            }
                        ],
                    },
                    {
                        "role": "tool",
                        "tool_call_id": call["id"],
                        "content": json.dumps(TOOL_RESULT),
                    },
                ]
                s2, _ = await _openai_call(t["client"], t["wire"], system, o_tools, msgs)
                tool2.append(s2)
        print(f"  round {rnd + 1}/{n} done")
    rows: list[dict[str, Any]] = []
    for i, t in enumerate(targets):
        rows.append(_row(t["ep"], t["label"], *samples[i]))
        print(_line(rows[-1]))

    (out / "llm.json").write_text(json.dumps(rows, indent=1))
    md = [
        "| endpoint | model | hello ttfb p50 | p95 | hello total p50 | span p50 | out tok | "
        "tool call done p50 | p95 | answer ttfb p50 | answer total p50 | answer span (out tok) "
        "| tool ok | words p50 | cache read/write (2nd+) | errors |",
        "|" + "---|" * 16,
    ]
    for r in rows:
        md.append(
            f"| {r['endpoint']} | {r['model']} | {_f(r['hello_ttfb'])} | {_f(r['hello_ttfb_p95'])} "
            f"| {_f(r['hello_total'])} | {_f(r['hello_span'])} | {r['hello_out']} "
            f"| {_f(r['tool_done'])} | {_f(r['tool_done_p95'])} | {_f(r['answer_ttfb'])} "
            f"| {_f(r['answer_total'])} | {_f(r['answer_span'])} ({r['answer_out']}) "
            f"| {r['tool_ok']} | {r['words']} | {r['cache']} "
            f"| {r['errors'] or ''} |"
        )
    (out / "llm.md").write_text("\n".join(md) + "\n")
    print(f"\nllm: {out / 'llm.md'}")
    return 0


def _row(
    ep: str, model: str, plain: list[Sample], tool1: list[Sample], tool2: list[Sample]
) -> dict[str, Any]:
    later = [s for s in plain[1:] + tool1[1:] if s.usage]
    reads = [s.usage.get("cache_read", 0) for s in later]
    writes = [s.usage.get("cache_write", 0) for s in later]
    errors = sorted({s.error for s in plain + tool1 + tool2 if s.error})
    outs = [s.usage.get("out") for s in plain if s.usage.get("out") is not None]
    answer_outs = [s.usage.get("out") for s in tool2 if s.usage.get("out") is not None]
    later_ttfb = [s.ttfb for s in plain[1:] + tool1[1:]]
    return {
        "endpoint": ep,
        "model": model,
        "n": len(plain),
        "hello_ttfb": _p50([s.ttfb for s in plain]),
        "hello_ttfb_p95": _p95([s.ttfb for s in plain]),
        "hello_total": _p50([s.total for s in plain]),
        "hello_span": _p50([s.span for s in plain]),
        "hello_out": int(statistics.median(outs)) if outs else None,
        "tool_done": _p50([s.tool_done for s in tool1]),
        "tool_done_p95": _p95([s.tool_done for s in tool1]),
        "answer_ttfb": _p50([s.ttfb for s in tool2]),
        "answer_total": _p50([s.total for s in tool2]),
        "answer_span": _p50([s.span for s in tool2]),
        "answer_out": int(statistics.median(answer_outs)) if answer_outs else None,
        "cold_ttfb": plain[0].ttfb if plain else None,
        "warm_ttfb": _p50(later_ttfb),
        "tool_ok": f"{sum(1 for s in tool1 if s.tool == 'list_awaiting_signoff')}/{len(tool1)}",
        "words": _p50([float(s.words) for s in tool2 if s.words is not None]),
        "cache": f"{int(statistics.median(reads)) if reads else '-'}/"
        f"{int(statistics.median(writes)) if writes else '-'}",
        "in_first": plain[0].usage if plain and plain[0].usage else None,
        "errors": "; ".join(errors)[:300],
    }


def _line(r: dict[str, Any]) -> str:
    return (
        f"  {r['endpoint']:<18} {r['model']:<32} hello ttfb {_f(r['hello_ttfb'])} "
        f"(span {_f(r['hello_span'])}, {r['hello_out']} tok)  tool {_f(r['tool_done'])}  "
        f"answer ttfb {_f(r['answer_ttfb'])} total {_f(r['answer_total'])}  "
        f"tool ok {r['tool_ok']}  cache {r['cache']}"
        + (f"  ERR {r['errors'][:120]}" if r["errors"] else "")
    )
