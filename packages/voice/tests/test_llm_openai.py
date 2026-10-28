"""The OpenAI-compatible session service (ADR 0036): tool calls translated both ways, every
call of a reply run however the server streams them, and a reply cut off at the output
cap handled as on the Anthropic path. Pipecat's own streaming loop runs here, fed by a
fake client; nothing leaves the machine."""

from __future__ import annotations

import json
from typing import Any

import pytest
from ghost_core.connection import REASONING_HEADROOM, ModelConnection
from ghost_voice import llm as llm_module
from ghost_voice.llm import (
    GhostOpenAILLMService,
    LLMReplyCutFrame,
    OpenAIClient,
    model_client,
)
from ghost_voice.thinking import ThinkingStartFrame
from openai.types.chat import ChatCompletionChunk
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.adapters.schemas.tools_schema import ToolsSchema
from pipecat.frames.frames import (
    FunctionCallFromLLM,
    LLMContextFrame,
    LLMMessagesAppendFrame,
    LLMTextFrame,
    TTSSpeakFrame,
)
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.frame_processor import FrameDirection
from pipecat.services.llm_service import LLMService
from pipecat.services.openai.llm import OpenAILLMService


def chunk(*, content=None, calls=None, finish=None) -> ChatCompletionChunk:
    delta: dict[str, Any] = {}
    if content is not None:
        delta["content"] = content
    if calls is not None:
        delta["tool_calls"] = calls
    return ChatCompletionChunk.model_validate(
        {
            "id": "c1",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "test-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }
    )


def call(index: int, *, id=None, name=None, args=None) -> dict[str, Any]:
    fn: dict[str, Any] = {}
    if name is not None:
        fn["name"] = name
    if args is not None:
        fn["arguments"] = args
    out: dict[str, Any] = {"index": index, "function": fn}
    if id is not None:
        out["id"], out["type"] = id, "function"
    return out


class FakeStream:
    def __init__(self, chunks: list[ChatCompletionChunk]) -> None:
        self.chunks = chunks
        self.closed = False

    def __aiter__(self):
        return self._gen()

    async def _gen(self):
        for c in self.chunks:
            yield c

    async def close(self) -> None:
        self.closed = True


class FakeClient:
    """``AsyncOpenAI`` as far as Pipecat and Ghost touch it."""

    def __init__(self, *replies: list[ChatCompletionChunk]) -> None:
        self.replies = list(replies)
        self.requests: list[dict[str, Any]] = []
        self.streams: list[FakeStream] = []
        self.chat = self
        self.completions = self

    async def create(self, **params: Any) -> FakeStream:
        self.requests.append(params)
        s = FakeStream(self.replies.pop(0))
        self.streams.append(s)
        return s


def service(client: FakeClient, cap: int = 2000) -> GhostOpenAILLMService:
    return GhostOpenAILLMService(
        api_key="unused",
        client=client,
        settings=OpenAILLMService.Settings(
            model="test-model", system_instruction="You are Ghost.", max_tokens=cap
        ),
    )


def context(messages: list[dict[str, Any]] | None = None) -> LLMContext:
    tool = FunctionSchema(
        name="github_list_issues",
        description="List issues",
        properties={"repo": {"type": "string"}},
        required=["repo"],
    )
    return LLMContext(
        messages=messages or [{"role": "user", "content": "What's open in ghost and infra?"}],
        tools=ToolsSchema(standard_tools=[tool]),
    )


async def run(monkeypatch, svc: GhostOpenAILLMService, ctx: LLMContext | None = None):
    """One reply through the service's frame handling, as the pipeline drives it: the tool
    calls handed to run, and every frame pushed, in order."""
    ran: list[list[FunctionCallFromLLM]] = []
    pushed: list[Any] = []

    async def run_calls(self, function_calls):
        ran.append(list(function_calls))

    async def push(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    async def nothing(*a, **k):
        return None

    monkeypatch.setattr(LLMService, "run_function_calls", run_calls)
    svc.push_frame = push  # type: ignore[method-assign]
    for m in ("start_processing_metrics", "stop_processing_metrics", "start_llm_usage_metrics"):
        monkeypatch.setattr(svc, m, nothing)
    await svc.process_frame(LLMContextFrame(context=ctx or context()), FrameDirection.DOWNSTREAM)
    return ran, pushed


def calls_of(ran: list[list[FunctionCallFromLLM]]) -> list[tuple[str, str, dict]]:
    return [(c.tool_call_id, c.function_name, dict(c.arguments)) for batch in ran for c in batch]


# --- translation ----------------------------------------------------------------------------


async def test_the_request_carries_the_system_prompt_tools_and_the_cap(monkeypatch):
    client = FakeClient([chunk(content="Hi."), chunk(finish="stop")])
    await run(monkeypatch, service(client))
    req = client.requests[0]
    assert req["model"] == "test-model" and req["stream"] is True and req["max_tokens"] == 2000
    assert req["messages"][0] == {"role": "system", "content": "You are Ghost."}
    assert req["tools"][0]["type"] == "function"
    assert req["tools"][0]["function"]["name"] == "github_list_issues"
    assert req["tools"][0]["function"]["parameters"]["required"] == ["repo"]
    assert client.streams[0].closed  # the stream is closed after the reply


async def test_a_tool_call_and_its_result_go_back_in_openai_shape(monkeypatch):
    # The context keeps Pipecat's universal shape; the adapter writes the assistant's call
    # and the tool's answer as Chat Completions messages on the next request.
    history = [
        {"role": "user", "content": "What's open in ghost?"},
        {
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "github_list_issues", "arguments": '{"repo": "ghost"}'},
                }
            ],
        },
        {"role": "tool", "tool_call_id": "call_1", "content": json.dumps({"count": 3})},
    ]
    client = FakeClient([chunk(content="Three."), chunk(finish="stop")])
    _, pushed = await run(monkeypatch, service(client), context(history))
    sent = client.requests[0]["messages"]
    assert sent[2]["tool_calls"][0]["function"]["name"] == "github_list_issues"
    assert sent[3] == {"role": "tool", "tool_call_id": "call_1", "content": '{"count": 3}'}
    assert [f.text for f in pushed if isinstance(f, LLMTextFrame)] == ["Three."]


# --- every tool call of a reply -------------------------------------------------------------


async def test_every_tool_call_in_a_streamed_reply_runs(monkeypatch):
    client = FakeClient(
        [
            chunk(content="Checking."),
            chunk(calls=[call(0, id="call_a", name="github_list_issues", args="")]),
            chunk(calls=[call(0, args='{"repo": ')]),
            chunk(calls=[call(0, args='"ghost"}')]),
            chunk(
                calls=[call(1, id="call_b", name="github_list_issues", args='{"repo": "infra"}')]
            ),
            chunk(finish="tool_calls"),
        ]
    )
    ran, _ = await run(monkeypatch, service(client))
    assert calls_of(ran) == [
        ("call_a", "github_list_issues", {"repo": "ghost"}),
        ("call_b", "github_list_issues", {"repo": "infra"}),
    ]
    assert ran[0][0].context is not None


async def test_several_calls_in_one_chunk_all_run(monkeypatch):
    # Ollama sends a reply's calls whole, in one chunk; Pipecat reads only the first.
    reply = [
        chunk(
            calls=[
                call(0, id="call_a", name="github_list_issues", args='{"repo": "ghost"}'),
                call(1, id="call_b", name="github_list_issues", args='{"repo": "infra"}'),
            ]
        ),
        chunk(finish="tool_calls"),
    ]
    ran, _ = await run(monkeypatch, service(FakeClient(reply)))
    assert [c[2] for c in calls_of(ran)] == [{"repo": "ghost"}, {"repo": "infra"}]

    # Without the watch, Pipecat alone would have run one of the two.
    plain = OpenAILLMService(api_key="unused", settings=OpenAILLMService.Settings(model="m"))
    plain._client = FakeClient(reply)  # noqa: SLF001
    ran_plain, _ = await run(monkeypatch, plain)  # type: ignore[arg-type]
    assert len(calls_of(ran_plain)) == 1


async def test_a_server_without_call_ids_gets_stable_ones(monkeypatch):
    reply = [
        chunk(calls=[call(0, name="github_list_issues", args='{"repo": "ghost"}')]),
        chunk(finish="tool_calls"),
    ]
    ran, _ = await run(monkeypatch, service(FakeClient(reply)))
    assert calls_of(ran) == [("call_0", "github_list_issues", {"repo": "ghost"})]


async def test_a_name_repeated_in_every_chunk_is_not_doubled(monkeypatch):
    reply = [
        chunk(calls=[call(0, id="c", name="github_list_issues", args='{"repo": ')]),
        chunk(calls=[call(0, name="github_list_issues", args='"ghost"}')]),
        chunk(finish="tool_calls"),
    ]
    ran, _ = await run(monkeypatch, service(FakeClient(reply)))
    assert calls_of(ran) == [("c", "github_list_issues", {"repo": "ghost"})]


async def test_an_answer_without_tools_is_untouched(monkeypatch):
    client = FakeClient([chunk(content="Three "), chunk(content="open."), chunk(finish="stop")])
    ran, pushed = await run(monkeypatch, service(client))
    assert ran == []
    assert "".join(f.text for f in pushed if isinstance(f, LLMTextFrame)) == "Three open."
    assert not any(isinstance(f, LLMReplyCutFrame) for f in pushed)


# --- a reply cut off at the output cap ------------------------------------------------------


def cut_reply() -> list[ChatCompletionChunk]:
    return [
        chunk(calls=[call(0, id="t1", name="change", args='{"op": "comment", "n": 390}')]),
        chunk(calls=[call(1, id="t2", name="change", args='{"op": "comment", "n": 389}')]),
        chunk(calls=[call(2, id="t3", name="change", args='{"op": "comm')]),
        chunk(finish="length"),
    ]


async def test_a_reply_cut_off_inside_its_tool_calls_is_redone_in_smaller_steps(monkeypatch):
    ran, pushed = await run(monkeypatch, service(FakeClient(cut_reply())))
    assert ran == []  # half a batch is not run; Pipecat alone would have run two
    assert [f.tool_calls for f in pushed if isinstance(f, LLMReplyCutFrame)] == [3]
    kinds = [type(f).__name__ for f in pushed]
    # The reply's end frame first, so the aggregator closes it before the note lands.
    assert (
        kinds.index("LLMFullResponseEndFrame")
        < kinds.index("LLMReplyCutFrame")
        < kinds.index("ThinkingStartFrame")
        < kinds.index("LLMMessagesAppendFrame")
    )
    note = next(f for f in pushed if isinstance(f, LLMMessagesAppendFrame))
    assert note.run_llm is True
    assert note.messages == [{"role": "user", "content": llm_module.CUT_OFF_NOTE}]
    assert not any(isinstance(f, TTSSpeakFrame) for f in pushed)


async def test_a_stream_that_ends_inside_a_call_without_a_reason_is_cut(monkeypatch):
    reply = [chunk(calls=[call(0, id="t1", name="change", args='{"n": ')])]
    ran, pushed = await run(monkeypatch, service(FakeClient(reply)))
    assert ran == []
    assert [f.tool_calls for f in pushed if isinstance(f, LLMReplyCutFrame)] == [1]


async def test_a_redo_cut_off_again_is_said_not_retried(monkeypatch):
    svc = service(FakeClient(cut_reply(), cut_reply()))
    _, pushed = await run(monkeypatch, svc)
    assert any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    ran, pushed = await run(monkeypatch, svc)
    assert ran == []
    assert not any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == [llm_module.CUT_OFF_LINE]


async def test_a_long_answer_cut_short_is_only_flagged(monkeypatch):
    reply = [chunk(content="Fifty-four open issues. The first"), chunk(finish="length")]
    ran, pushed = await run(monkeypatch, service(FakeClient(reply)))
    assert ran == []
    assert [f.tool_calls for f in pushed if isinstance(f, LLMReplyCutFrame)] == [0]
    assert not any(
        isinstance(f, (LLMMessagesAppendFrame, TTSSpeakFrame, ThinkingStartFrame)) for f in pushed
    )


# --- the interface ----------------------------------------------------------------------


def test_the_protocol_picks_the_client():
    env = {"GHOST_LLM_PROVIDER": "ollama", "GHOST_LLM_MODEL": "qwen3:8b"}
    c = model_client(ModelConnection.from_env(env).target(env))
    assert isinstance(c, OpenAIClient)
    assert c.settings_type is OpenAILLMService.Settings
    svc = c.service(system_instruction="x", max_tokens=2000, thinking="between_tools")
    assert isinstance(svc, GhostOpenAILLMService)
    assert svc._client is c.client  # noqa: SLF001 - the one shared client
    assert svc._settings.max_tokens == 2000  # noqa: SLF001


def test_openai_itself_gets_its_cap_field():
    env = {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-4.1", "OPENAI_API_KEY": "s"}
    c = model_client(ModelConnection.from_env(env).target(env))
    svc = c.service(system_instruction="x", max_tokens=2000, thinking="between_tools")
    assert svc._settings.max_completion_tokens == 2000  # noqa: SLF001


async def test_a_reasoning_model_is_asked_for_its_lowest_effort_with_room_to_reason(monkeypatch):
    # The request as Pipecat sends it, for a session on gpt-5.5 at OpenAI. Untested live.
    env = {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5.5", "OPENAI_API_KEY": "k"}
    c = model_client(ModelConnection.from_env(env).target(env))
    c.client = FakeClient([chunk(content="Hi."), chunk(finish="stop")])
    svc = c.service(system_instruction="x", max_tokens=2000, thinking="between_tools")
    await run(monkeypatch, svc)
    req = c.client.requests[0]
    assert req["reasoning_effort"] == "none"
    assert req["max_completion_tokens"] == 2000 + REASONING_HEADROOM
    assert "max_tokens" not in req or not isinstance(req["max_tokens"], int)


async def test_a_one_shot_and_a_probe_carry_the_effort_too():
    env = {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "o4-mini", "OPENAI_API_KEY": "k"}
    c = model_client(ModelConnection.from_env(env).target(env))
    c.client = FakeClient([chunk(content="Hi."), chunk(finish="stop")])
    await c.probe(prompt="Say hi.", max_tokens=16, thinking=None)
    req = c.client.requests[0]
    assert req["reasoning_effort"] == "low"
    assert req["max_completion_tokens"] == 16 + REASONING_HEADROOM


async def test_a_model_that_doesnt_reason_gets_no_effort(monkeypatch):
    env = {"GHOST_LLM_PROVIDER": "ollama", "GHOST_LLM_MODEL": "qwen3:8b"}
    c = model_client(ModelConnection.from_env(env).target(env))
    c.client = FakeClient([chunk(content="Hi."), chunk(finish="stop")])
    await run(monkeypatch, c.service(system_instruction="x", max_tokens=2000, thinking="x"))
    req = c.client.requests[0]
    assert "reasoning_effort" not in req and req["max_tokens"] == 2000


async def test_the_probe_times_the_first_token():
    env = {"GHOST_LLM_PROVIDER": "ollama", "GHOST_LLM_MODEL": "qwen3:8b"}
    c = model_client(ModelConnection.from_env(env).target(env))
    c.client = FakeClient([chunk(content="Hello"), chunk(content=" there."), chunk(finish="stop")])
    p = await c.probe(prompt="Say hi.", max_tokens=16, thinking=None)
    assert p.error is None and p.text == "Hello there." and p.model == "test-model"
    assert p.ttft is not None and p.total is not None and p.ttft <= p.total
    assert c.client.requests[0]["max_tokens"] == 16


@pytest.fixture(autouse=True)
def _fresh_clients():
    llm_module._clients.clear()  # noqa: SLF001
    yield
    llm_module._clients.clear()  # noqa: SLF001
