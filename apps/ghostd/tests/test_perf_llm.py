"""`ghostd perf llm`'s targets (ADR 0037): each named endpoint is its own provider, reached
over that provider's protocol with its own key, whatever the configured connection is. No
request is sent here."""

from __future__ import annotations

import pytest
from anthropic import AsyncAnthropic
from ghost_core.connection import CLIPROXY_DEFAULT_URL, REASONING_HEADROOM
from ghost_voice import VoiceSettings
from ghost_voice import llm as llm_module
from ghost_voice.config import LLM_FIELDS
from ghostd.perf.llm import CLAUDE_MODELS, build_targets, endpoint_connection
from openai import AsyncOpenAI

# The configured connection is OpenAI, the case the review found: before, `proxy` kept
# the provider and sent the OpenAI key to api.openai.com as an Anthropic request.
ENV = {
    "GHOST_LLM_PROVIDER": "openai",
    "GHOST_LLM_MODEL": "gpt-5.5",
    "OPENAI_API_KEY": "sk-openai-0123",
    "ANTHROPIC_AUTH_TOKEN": "sk-proxy-4567",
    "OPENROUTER_API_KEY": "sk-or-89ab",
}


@pytest.fixture(autouse=True)
def _fresh_clients():
    llm_module._clients.clear()  # noqa: SLF001
    yield
    llm_module._clients.clear()  # noqa: SLF001


def voice(**over: str) -> VoiceSettings:
    # Every connection field set, so the shell's own environment doesn't leak in.
    fields = {f: "" for f in LLM_FIELDS} | {"llm_provider": "openai", "llm_model": "gpt-5.5"}
    fields |= over
    return VoiceSettings().model_copy(update=fields)  # type: ignore[call-arg]


def targets(endpoints: list[str], models: list[str] | None = None, **over: str):
    return build_targets(
        endpoints=endpoints,
        models=models or [],
        openrouter_models=[],
        thinking="between_tools",
        voice=voice(**over),
        env=ENV,
    )


def test_proxy_is_cliproxy_over_anthropics_api_whatever_the_provider():
    rows = targets(["proxy"], anthropic_base_url="http://proxy.lan:8317")
    assert [r["label"] for r in rows] == list(CLAUDE_MODELS)
    for r in rows:
        t = r["target"]
        assert (t.provider, t.protocol, r["kind"]) == ("cliproxy", "anthropic", "anthropic")
        assert t.base_url == "http://proxy.lan:8317"
        assert t.api_key == ENV["ANTHROPIC_AUTH_TOKEN"]
        assert isinstance(r["client"], AsyncAnthropic)
        assert r["sysbp"] is False


def test_proxy_without_an_address_is_cliproxys_own():
    t = endpoint_connection("proxy+sysbp", voice())
    assert t.provider == "cliproxy" and t.base_url == CLIPROXY_DEFAULT_URL
    rows = targets(["proxy+sysbp"])
    assert all(r["sysbp"] for r in rows)


def test_openrouter_is_its_anthropic_api_with_its_own_key():
    rows = targets(["openrouter"], ["claude-sonnet-5-5"])
    (r,) = rows
    t = r["target"]
    assert (t.provider, t.protocol) == ("openrouter", "anthropic")
    assert t.model == "anthropic/claude-sonnet-5.5" and r["label"] == "claude-sonnet-5-5"
    assert t.api_key == ENV["OPENROUTER_API_KEY"]
    assert isinstance(r["client"], AsyncAnthropic)


def test_a_provider_id_speaking_openai_gets_the_openai_client_and_its_default_model():
    (r,) = targets(["openai"])
    t = r["target"]
    assert (t.protocol, r["kind"], t.model) == ("openai", "openai", "gpt-5")
    assert isinstance(r["client"], AsyncOpenAI)
    assert t.chat_params(2000)["max_completion_tokens"] == 2000 + REASONING_HEADROOM


def test_current_is_the_configured_connection_on_its_own_protocol():
    (r,) = targets(["current"])
    t = r["target"]
    assert (t.provider, t.protocol, t.model) == ("openai", "openai", "gpt-5.5")
    assert isinstance(r["client"], AsyncOpenAI)
    assert t.chat_params(2000)["reasoning_effort"] == "none"


def test_an_endpoint_without_its_key_is_skipped(capsys):
    rows = build_targets(
        endpoints=["anthropic", "current"],
        models=[],
        openrouter_models=[],
        thinking="between_tools",
        voice=voice(),
        env={k: v for k, v in ENV.items() if k != "ANTHROPIC_API_KEY"},
    )
    assert [r["ep"] for r in rows] == ["current"]
    out = capsys.readouterr().out
    assert "skipping anthropic" in out and "sk-" not in out


def test_an_unknown_endpoint_says_what_is_known():
    with pytest.raises(SystemExit, match="unknown endpoint 'nope'"):
        targets(["nope"])


def test_openrouter_models_go_over_its_openai_api():
    rows = build_targets(
        endpoints=[],
        models=[],
        openrouter_models=["openai/gpt-5.5", "qwen/qwen3-235b"],
        thinking="between_tools",
        voice=voice(),
        env=ENV,
    )
    assert [r["ep"] for r in rows] == ["openrouter-openai"] * 2
    a, b = (r["target"] for r in rows)
    assert a.protocol == "openai" and a.base_url == "https://openrouter.ai/api/v1"
    assert a.api_key == ENV["OPENROUTER_API_KEY"]
    assert a.chat_params(2000)["reasoning_effort"] == "none"
    assert b.chat_params(2000) == {"max_tokens": 2000}
