"""Which endpoint Ghost's model calls go to, that keys never show, the system-prompt cache
marker, and the one client per process."""

from __future__ import annotations

import pytest
from ghost_voice import llm as llm_module
from ghost_voice.config import OPENROUTER_ANTHROPIC_URL, VoiceSettings, openrouter_model
from ghost_voice.llm import GhostAnthropicLLMService, shared_client, with_system_breakpoint
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.adapters.schemas.tools_schema import ToolsSchema
from pipecat.processors.aggregators.llm_context import LLMContext

PROXY_TOKEN = "sk-proxy-0123456789abcdef"
OR_KEY = "sk-or-v1-fedcba9876543210"


@pytest.fixture
def env(monkeypatch):
    for k in ("GHOST_LLM_ENDPOINT", "GHOST_LLM_MODEL"):
        monkeypatch.delenv(k, raising=False)
    monkeypatch.setenv("ANTHROPIC_BASE_URL", "http://192.168.1.5:8317")
    monkeypatch.setenv("ANTHROPIC_AUTH_TOKEN", PROXY_TOKEN)
    monkeypatch.setenv("OPENROUTER_API_KEY", OR_KEY)
    return monkeypatch


def test_default_is_the_proxy_as_before(env):
    t = VoiceSettings().llm_target()
    assert (t.endpoint, t.base_url, t.model) == (
        "proxy",
        "http://192.168.1.5:8317",
        "claude-sonnet-5-5",
    )
    assert t.api_key == PROXY_TOKEN


def test_openrouter_uses_its_url_key_and_model_id(env):
    env.setenv("GHOST_LLM_ENDPOINT", "OpenRouter")
    t = VoiceSettings().llm_target()
    assert t.endpoint == "openrouter" and t.base_url == OPENROUTER_ANTHROPIC_URL
    assert t.model == "anthropic/claude-sonnet-5.5" and t.api_key == OR_KEY


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
        assert "via " + endpoint in t.describe()


def test_problems_name_the_missing_variable_not_a_value(env):
    env.setenv("GHOST_LLM_ENDPOINT", "openrouter")
    env.delenv("OPENROUTER_API_KEY")
    assert VoiceSettings().llm_problems() == [
        "GHOST_LLM_ENDPOINT=openrouter but OPENROUTER_API_KEY is not set"
    ]
    env.setenv("GHOST_LLM_ENDPOINT", "proxy")
    env.delenv("ANTHROPIC_AUTH_TOKEN")
    assert VoiceSettings().llm_problems() == ["ANTHROPIC_AUTH_TOKEN is not set"]


def test_an_unknown_endpoint_is_refused(env):
    env.setenv("GHOST_LLM_ENDPOINT", "direct")
    with pytest.raises(ValueError, match="proxy, openrouter"):
        VoiceSettings()


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
