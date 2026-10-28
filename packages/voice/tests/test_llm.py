"""Which endpoint Ghost's model calls go to, that keys never show, the system-prompt cache
marker, the one client per process, every tool call of a reply, and a reply cut off at the
output cap."""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest
from ghost_voice import llm as llm_module
from ghost_voice.config import (
    LLM_FIELDS,
    OPENROUTER_ANTHROPIC_URL,
    VoiceSettings,
    openrouter_model,
)
from ghost_voice.llm import (
    GhostAnthropicLLMService,
    LLMReplyCutFrame,
    shared_client,
    with_system_breakpoint,
)
from ghost_voice.thinking import ThinkingStartFrame
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.adapters.schemas.tools_schema import ToolsSchema
from pipecat.frames.frames import (
    FunctionCallFromLLM,
    LLMFullResponseEndFrame,
    LLMMessagesAppendFrame,
    TTSSpeakFrame,
)
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.services.anthropic.llm import AnthropicLLMService
from pipecat.services.llm_service import LLMService

PROXY_TOKEN = "sk-proxy-0123456789abcdef"
OR_KEY = "sk-or-v1-fedcba9876543210"


@pytest.fixture
def env(monkeypatch):
    for k in (*LLM_FIELDS.values(), "CLIPROXY_BASE_URL", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
        monkeypatch.delenv(k, raising=False)
    monkeypatch.setenv("ANTHROPIC_BASE_URL", "http://192.168.1.5:8317")
    monkeypatch.setenv("ANTHROPIC_AUTH_TOKEN", PROXY_TOKEN)
    monkeypatch.setenv("OPENROUTER_API_KEY", OR_KEY)
    return monkeypatch


def test_default_is_the_proxy_as_before(env):
    t = VoiceSettings().llm_target()
    assert (t.protocol, t.base_url, t.model) == (
        "anthropic",
        "http://192.168.1.5:8317",
        "claude-sonnet-5-5",
    )
    assert t.api_key == PROXY_TOKEN


def test_openrouter_uses_its_url_key_and_model_id(env):
    env.setenv("GHOST_LLM_ENDPOINT", "OpenRouter")
    t = VoiceSettings().llm_target()
    assert t.endpoint == "openrouter" and t.base_url == OPENROUTER_ANTHROPIC_URL
    assert t.model == "anthropic/claude-sonnet-5.5" and t.api_key == OR_KEY


def test_a_provider_and_password_by_name(env):
    env.setenv("GHOST_LLM_PROVIDER", "openai")
    env.setenv("GHOST_LLM_MODEL", "gpt-5-mini")
    env.setenv("OPENAI_API_KEY", "sk-openai-1")
    t = VoiceSettings().llm_target()
    assert (t.provider, t.protocol, t.model, t.api_key) == (
        "openai",
        "openai",
        "gpt-5-mini",
        "sk-openai-1",
    )
    # The key is read when the target is built: a password saved since is used next time.
    s = VoiceSettings()
    env.setenv("OPENAI_API_KEY", "sk-openai-2")
    assert s.llm_target().api_key == "sk-openai-2"


def test_openrouter_model_ids():
    assert openrouter_model("claude-sonnet-5-5") == "anthropic/claude-sonnet-5.5"
    assert openrouter_model("claude-haiku-4-5") == "anthropic/claude-haiku-4.5"
    assert openrouter_model("claude-sonnet-5") == "anthropic/claude-sonnet-5"
    assert openrouter_model("anthropic/claude-opus-5.5") == "anthropic/claude-opus-5.5"


def test_keys_never_show(env):
    for endpoint in ("proxy", "openrouter"):
        env.setenv("GHOST_LLM_ENDPOINT", endpoint)
        s = VoiceSettings()
        t = s.llm_target()
        shown = " ".join([repr(s), str(s), repr(t), t.describe(), *s.llm_problems()])
        assert PROXY_TOKEN not in shown and OR_KEY not in shown
    assert "via openrouter" in t.describe()


def test_problems_name_the_missing_variable_not_a_value(env):
    env.setenv("GHOST_LLM_ENDPOINT", "openrouter")
    env.delenv("OPENROUTER_API_KEY")
    assert VoiceSettings().llm_problems() == [
        "GHOST_LLM_ENDPOINT=openrouter but OPENROUTER_API_KEY is not set"
    ]
    env.setenv("GHOST_LLM_PROVIDER", "anthropic")
    assert VoiceSettings().llm_problems() == ["ANTHROPIC_API_KEY is not set"]


def test_an_unknown_endpoint_is_refused(env):
    env.setenv("GHOST_LLM_ENDPOINT", "direct")
    with pytest.raises(ValueError, match="proxy, openrouter"):
        VoiceSettings()


def test_the_anthropic_client_never_falls_back_to_the_environment(env):
    # ANTHROPIC_BASE_URL points at a proxy in an older setup; Anthropic itself is asked for.
    env.setenv("GHOST_LLM_PROVIDER", "anthropic")
    env.setenv("ANTHROPIC_API_KEY", "sk-ant-1")
    client = shared_client(VoiceSettings().llm_target())
    assert str(client.base_url).startswith("https://api.anthropic.com")


def service(cache_system: bool = True) -> GhostAnthropicLLMService:
    svc = GhostAnthropicLLMService(
        api_key="unused",
        settings=GhostAnthropicLLMService.Settings(
            model="claude-sonnet-5-5",
            system_instruction="You are Ghost.",
            enable_prompt_caching=True,
        ),
    )
    svc.cache_system = cache_system
    return svc


def context() -> LLMContext:
    tool = FunctionSchema(name="get_issue", description="d", properties={}, required=[])
    return LLMContext(
        messages=[
            {"role": "user", "content": "What needs my sign-off?"},
            {"role": "assistant", "content": "Three."},
            {"role": "user", "content": "And the first one?"},
        ],
        tools=ToolsSchema(standard_tools=[tool]),
    )


def markers(params) -> int:
    n = sum(1 for b in params["system"] if isinstance(b, dict) and b.get("cache_control"))
    for m in params["messages"]:
        if isinstance(m["content"], list):
            n += sum(1 for b in m["content"] if isinstance(b, dict) and b.get("cache_control"))
    return n


def test_the_system_prompt_carries_a_cache_marker():
    params = service()._get_llm_invocation_params(context())  # noqa: SLF001
    assert params["system"] == [
        {"type": "text", "text": "You are Ghost.", "cache_control": {"type": "ephemeral"}}
    ]
    # Pipecat's own two markers stay on the last two user messages: three of four.
    assert markers(params) == 3
    assert params["tools"][0]["name"] == "get_issue"  # tools render before system: cached too


def test_without_the_marker_the_system_prompt_is_a_string():
    params = service(cache_system=False)._get_llm_invocation_params(context())  # noqa: SLF001
    assert params["system"] == "You are Ghost."


def test_with_system_breakpoint_leaves_other_shapes_alone():
    assert with_system_breakpoint({"system": ""}) == {"system": ""}
    blocks = [{"type": "text", "text": "x"}]
    assert with_system_breakpoint({"system": blocks})["system"] is blocks


async def test_one_client_per_endpoint_and_key(env):
    s = VoiceSettings()
    a, b = shared_client(s.llm_target()), shared_client(s.llm_target())
    assert a is b
    env.setenv("GHOST_LLM_ENDPOINT", "openrouter")
    assert shared_client(VoiceSettings().llm_target()) is not a
    llm_module.SHARE_CLIENT = False
    try:
        assert shared_client(s.llm_target()) is not a
    finally:
        llm_module.SHARE_CLIENT = True


# --- every tool call of a reply, and a reply cut off at the output cap --------------------


def events(*blocks, stop: str = "tool_use") -> list:
    """A streamed Anthropic reply: its content blocks, then the stop reason."""
    out = [
        SimpleNamespace(
            type="message_start",
            message=SimpleNamespace(usage=SimpleNamespace(input_tokens=8, output_tokens=1)),
        )
    ]
    for i, block in enumerate(blocks):
        out += block(i)
    out.append(
        SimpleNamespace(
            type="message_delta",
            delta=SimpleNamespace(stop_reason=stop, stop_sequence=None),
            usage=SimpleNamespace(output_tokens=400),
        )
    )
    out.append(SimpleNamespace(type="message_stop"))
    return out


def text(s: str):
    def block(i: int) -> list:
        return [
            SimpleNamespace(
                type="content_block_start",
                index=i,
                content_block=SimpleNamespace(type="text", text=""),
            ),
            SimpleNamespace(type="content_block_delta", index=i, delta=SimpleNamespace(text=s)),
            SimpleNamespace(type="content_block_stop", index=i),
        ]

    return block


def tool(name: str, *json_parts: str, id: str = ""):
    """A ``tool_use`` block whose arguments stream in ``json_parts``."""

    def block(i: int) -> list:
        return [
            SimpleNamespace(
                type="content_block_start",
                index=i,
                content_block=SimpleNamespace(
                    type="tool_use", id=id or f"toolu_{name}_{i}", name=name, input={}
                ),
            ),
            *[
                SimpleNamespace(
                    type="content_block_delta", index=i, delta=SimpleNamespace(partial_json=p)
                )
                for p in json_parts
            ],
            SimpleNamespace(type="content_block_stop", index=i),
        ]

    return block


def pipecat_like(evs: list):
    """What Pipecat 1.12's streaming loop does with a reply: it keeps the last ``tool_use``
    block only, and runs it only when the stop reason is ``tool_use``."""

    async def api_call(**params):
        async def stream():
            for e in evs:
                yield e

        return stream()

    async def process(self, context):
        last, acc, calls = None, "", []
        async for e in await self._create_message_stream(api_call, {}):
            if e.type == "content_block_start" and e.content_block.type == "tool_use":
                last, acc = e.content_block, ""
            elif e.type == "content_block_delta" and hasattr(e.delta, "partial_json") and last:
                acc += e.delta.partial_json
            elif e.type == "message_delta" and e.delta.stop_reason == "tool_use" and last:
                calls.append(
                    FunctionCallFromLLM(
                        context=context,
                        tool_call_id=last.id,
                        function_name=last.name,
                        arguments=json.loads(acc) if acc else {},
                    )
                )
        await self.run_function_calls(calls)
        await self.push_frame(LLMFullResponseEndFrame())

    return process


async def reply(monkeypatch, evs: list, svc: GhostAnthropicLLMService | None = None):
    """Stream one reply through the service: the tool calls Pipecat was handed to run, and
    the frames the service pushed."""
    svc = svc or service()
    ran: list[list[FunctionCallFromLLM]] = []
    pushed: list = []

    async def run(self, function_calls):
        ran.append(list(function_calls))

    async def push(frame, direction=None):
        pushed.append(frame)

    monkeypatch.setattr(LLMService, "run_function_calls", run)
    monkeypatch.setattr(AnthropicLLMService, "_process_context", pipecat_like(evs))
    svc.push_frame = push  # type: ignore[method-assign]
    await svc._process_context(context())  # noqa: SLF001
    return svc, ran, pushed


def names(ran: list[list[FunctionCallFromLLM]]) -> list[tuple[str, str, dict]]:
    return [(c.tool_call_id, c.function_name, dict(c.arguments)) for calls in ran for c in calls]


async def test_every_tool_call_in_a_reply_runs(monkeypatch):
    # The prompt asks for independent lookups together; Pipecat alone ran only the last.
    _, ran, _ = await reply(
        monkeypatch,
        events(
            text("Checking."),
            tool("list_issues", '{"repo": ', '"ghost"}', id="t1"),
            tool("list_issues", '{"repo": "ecosystem"}', id="t2"),
            tool("issue_read", id="t3"),
        ),
    )
    assert names(ran) == [
        ("t1", "list_issues", {"repo": "ghost"}),
        ("t2", "list_issues", {"repo": "ecosystem"}),
        ("t3", "issue_read", {}),
    ]
    assert ran[0][0].context is not None


async def test_one_tool_call_runs_as_pipecat_hands_it_over(monkeypatch):
    _, ran, _ = await reply(monkeypatch, events(tool("issue_read", '{"n": 8}', id="t1")))
    assert names(ran) == [("t1", "issue_read", {"n": 8})]


async def test_an_answer_without_tools_is_untouched(monkeypatch):
    _, ran, pushed = await reply(monkeypatch, events(text("Three."), stop="end_turn"))
    assert ran == [[]]
    assert not any(isinstance(f, LLMReplyCutFrame) for f in pushed)


async def test_a_reply_cut_off_inside_its_tool_calls_is_redone_in_smaller_steps(monkeypatch):
    # The car on 2026-10-24: four comments in one reply at a 400-token cap. Two complete,
    # the third cut mid-arguments; Pipecat saw ``max_tokens`` and ran none, silently.
    _, ran, pushed = await reply(
        monkeypatch,
        events(
            tool("change", '{"op": "comment", "n": 390}', id="t1"),
            tool("change", '{"op": "comment", "n": 389}', id="t2"),
            tool("change", '{"op": "comm', id="t3"),
            stop="max_tokens",
        ),
    )
    assert ran == []  # half a batch is not run
    assert [f.tool_calls for f in pushed if isinstance(f, LLMReplyCutFrame)] == [3]
    kinds = [type(f).__name__ for f in pushed]
    assert (
        kinds.index("LLMFullResponseEndFrame")
        < kinds.index("ThinkingStartFrame")
        < kinds.index("LLMMessagesAppendFrame")
    )
    note = next(f for f in pushed if isinstance(f, LLMMessagesAppendFrame))
    assert note.run_llm is True
    assert note.messages == [{"role": "user", "content": llm_module.CUT_OFF_NOTE}]
    assert "smaller steps" in llm_module.CUT_OFF_NOTE
    assert not any(isinstance(f, TTSSpeakFrame) for f in pushed)


async def test_a_redo_cut_off_again_is_said_not_retried(monkeypatch):
    cut = events(
        tool("change", '{"n": 1}', id="t1"), tool("change", '{"n": ', id="t2"), stop="max_tokens"
    )
    svc, _, pushed = await reply(monkeypatch, cut)
    assert any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    _, ran, pushed = await reply(monkeypatch, cut, svc=svc)
    assert ran == []
    assert not any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == [llm_module.CUT_OFF_LINE]


async def test_a_redo_that_completes_lets_the_next_cut_be_redone(monkeypatch):
    cut = events(tool("change", '{"n": ', id="t1"), stop="max_tokens")
    svc, _, _ = await reply(monkeypatch, cut)
    await reply(monkeypatch, events(tool("change", '{"n": 1}', id="t1")), svc=svc)  # whole
    _, _, pushed = await reply(monkeypatch, cut, svc=svc)
    assert any(isinstance(f, LLMMessagesAppendFrame) for f in pushed)
    assert not any(isinstance(f, TTSSpeakFrame) for f in pushed)


async def test_a_long_answer_cut_short_is_only_flagged(monkeypatch):
    _, ran, pushed = await reply(
        monkeypatch, events(text("Fifty-four open issues. The first"), stop="max_tokens")
    )
    assert ran == []
    assert [f.tool_calls for f in pushed if isinstance(f, LLMReplyCutFrame)] == [0]
    assert not any(
        isinstance(f, (LLMMessagesAppendFrame, TTSSpeakFrame, ThinkingStartFrame)) for f in pushed
    )


def test_the_output_cap_leaves_room_for_a_batch_of_tool_calls(env):
    assert VoiceSettings().llm_max_tokens >= 1500
