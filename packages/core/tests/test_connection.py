"""The model connection: providers, the settings from before, Passwords by name, the model
list and OpenRouter's sign-in."""

from __future__ import annotations

import json
from urllib.parse import parse_qs, urlparse

import httpx
import pytest
from ghost_core.connection import (
    PROVIDERS,
    REASONING_HEADROOM,
    ModelConnection,
    format_headers,
    list_models,
    new_pkce,
    openrouter_auth_url,
    openrouter_exchange,
    openrouter_model,
    parse_headers,
    reasoning,
)

OR_KEY = "sk-or-v1-fedcba9876543210"
TOKEN = "sk-proxy-0123456789abcdef"


def test_the_operators_setup_from_before_is_openrouter_unchanged():
    # Their env on 2026-10-28: GHOST_LLM_ENDPOINT=openrouter, no model set, and
    # ANTHROPIC_BASE_URL pointing at CLIProxy (unused by the endpoint).
    env = {
        "GHOST_LLM_ENDPOINT": "openrouter",
        "OPENROUTER_API_KEY": OR_KEY,
        "ANTHROPIC_BASE_URL": "http://192.168.4.67:8317",
        "CLIPROXY_BASE_URL": "http://192.168.4.67:8317",
    }
    c = ModelConnection.from_env(env)
    t = c.target(env)
    assert (c.provider, t.protocol, t.base_url) == (
        "openrouter",
        "anthropic",
        "https://openrouter.ai/api",
    )
    assert t.model == "anthropic/claude-sonnet-5.5" and t.api_key == OR_KEY
    assert c.legacy == "GHOST_LLM_ENDPOINT=openrouter" and c.problems(env) == []


def test_the_proxy_from_before_is_the_anthropic_base_url_with_its_token():
    env = {"ANTHROPIC_BASE_URL": "http://192.168.1.5:8317/", "ANTHROPIC_AUTH_TOKEN": TOKEN}
    c = ModelConnection.from_env(env)
    t = c.target(env)
    assert (c.provider, t.base_url, t.model, t.api_key) == (
        "custom",
        "http://192.168.1.5:8317",
        "claude-sonnet-5-5",
        TOKEN,
    )
    assert t.protocol == "anthropic"
    # Said to be CLIProxy when it is the CLIProxy address.
    env["CLIPROXY_BASE_URL"] = "http://192.168.1.5:8317"
    assert ModelConnection.from_env(env).provider == "cliproxy"


def test_no_settings_at_all_is_anthropic_and_says_what_is_missing():
    c = ModelConnection.from_env({})
    assert c.provider == "anthropic" and c.base_url == "https://api.anthropic.com"
    assert c.problems({}) == ["ANTHROPIC_API_KEY is not set"]
    # A legacy token with no address is Anthropic itself, as the SDK default was.
    env = {"ANTHROPIC_AUTH_TOKEN": TOKEN}
    assert ModelConnection.from_env(env).target(env).api_key == TOKEN


def test_a_provider_wins_over_the_older_settings():
    env = {
        "GHOST_LLM_PROVIDER": "ollama",
        "GHOST_LLM_ENDPOINT": "openrouter",
        "GHOST_LLM_MODEL": "qwen3:8b",
    }
    c = ModelConnection.from_env(env)
    t = c.target(env)
    assert (t.provider, t.protocol, t.base_url, t.model, t.api_key) == (
        "ollama",
        "openai",
        "http://localhost:11434/v1",
        "qwen3:8b",
        "",
    )
    assert c.problems(env) == []  # a local server needs no key


def test_every_provider_has_a_protocol_and_an_address_or_says_so():
    for p in PROVIDERS.values():
        c = ModelConnection.from_env({"GHOST_LLM_PROVIDER": p.id, "GHOST_LLM_MODEL": "m"})
        assert c.protocol in ("anthropic", "openai")
        if p.id == "custom":
            assert any("needs an address" in x for x in c.problems({}))
        else:
            assert c.base_url.startswith("http")


def test_openai_uses_its_own_cap_field():
    env = {"GHOST_LLM_PROVIDER": "openai", "OPENAI_API_KEY": "sk-x"}
    assert ModelConnection.from_env(env).target(env).cap_param == "max_completion_tokens"
    env = {"GHOST_LLM_PROVIDER": "lmstudio", "GHOST_LLM_MODEL": "m"}
    assert ModelConnection.from_env(env).target(env).cap_param == "max_tokens"


@pytest.mark.parametrize(
    ("model", "reasons", "effort"),
    [
        # From 5.1 on, and GPT-6: no reasoning, the only effort 5.4+ takes with tools.
        ("gpt-5.1", True, "none"),
        ("gpt-5.5", True, "none"),
        ("gpt-5.6-terra", True, "none"),
        ("gpt-6-sol", True, "none"),
        ("gpt-6.1-terra", True, "none"),
        ("openai/gpt-5.4-mini", True, "none"),
        # No "none" on these: low.
        ("gpt-6-astra", True, "low"),
        ("gpt-6.1-sol", True, "low"),
        # The first GPT-5s: low, since minimal turns off parallel tool calls.
        ("gpt-5", True, "low"),
        ("gpt-5-mini", True, "low"),
        ("gpt-5-nano-2025-08-07", True, "low"),
        ("o3", True, "low"),
        ("o4-mini", True, "low"),
        ("o1-mini", True, ""),
        # Reason, but take no lower effort than their own.
        ("gpt-5-pro", True, ""),
        ("gpt-5.1-chat-latest", True, ""),
        # Not reasoning models.
        ("gpt-4.1", False, ""),
        ("gpt-4o-mini", False, ""),
        ("claude-sonnet-5-5", False, ""),
        ("qwen3:8b", False, ""),
        ("llama3.1", False, ""),
    ],
)
def test_reasoning_is_a_property_of_the_model(model, reasons, effort):
    assert reasoning(model).reasons is reasons
    assert reasoning(model).effort == effort


def test_a_reasoning_model_gets_its_lowest_effort_and_room_to_reason():
    env = {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5.5", "OPENAI_API_KEY": "k"}
    t = ModelConnection.from_env(env).target(env)
    assert t.chat_params(2000) == {
        "max_completion_tokens": 2000 + REASONING_HEADROOM,
        "reasoning_effort": "none",
    }
    # The model decides, not the provider: a gpt-5 behind your own address too ...
    env = {
        "GHOST_LLM_PROVIDER": "custom",
        "GHOST_LLM_PROTOCOL": "openai",
        "GHOST_LLM_BASE_URL": "http://localhost:4000/v1",
        "GHOST_LLM_MODEL": "gpt-5",
    }
    t = ModelConnection.from_env(env).target(env)
    assert t.chat_params(2000) == {
        "max_completion_tokens": 2000 + REASONING_HEADROOM,
        "reasoning_effort": "low",
    }
    # ... and a model that doesn't reason, even at OpenAI, keeps the plain cap.
    env = {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-4.1", "OPENAI_API_KEY": "k"}
    assert ModelConnection.from_env(env).target(env).chat_params(2000) == {
        "max_completion_tokens": 2000
    }
    env = {"GHOST_LLM_PROVIDER": "ollama", "GHOST_LLM_MODEL": "qwen3:8b"}
    assert ModelConnection.from_env(env).target(env).chat_params(2000) == {"max_tokens": 2000}


def test_the_messages_api_never_gets_a_reasoning_effort():
    # An OpenAI id named on an Anthropic-protocol provider (OpenRouter's Messages API).
    env = {"GHOST_LLM_PROVIDER": "openrouter", "GHOST_LLM_MODEL": "openai/gpt-5.5"}
    t = ModelConnection.from_env(env).target(env)
    assert not t.reasoning.reasons and t.output_cap(2000) == 2000


def test_a_custom_url_picks_its_protocol_and_password_by_name():
    env = {
        "GHOST_LLM_PROVIDER": "custom",
        "GHOST_LLM_PROTOCOL": "openai",
        "GHOST_LLM_BASE_URL": "http://gpu-box:8000/v1",
        "GHOST_LLM_MODEL": "llama",
        "GHOST_LLM_PASSWORD": "GPU_BOX_KEY",
        "GPU_BOX_KEY": "k-123",
        "GHOST_LLM_HEADERS": "X-Team: ghost; HTTP-Referer: https://ghost.local/app",
        "GHOST_LLM_TIMEOUT_SECS": "30",
    }
    c = ModelConnection.from_env(env)
    t = c.target(env)
    assert (t.protocol, t.base_url, t.api_key, t.timeout_secs) == (
        "openai",
        "http://gpu-box:8000/v1",
        "k-123",
        30.0,
    )
    assert dict(t.headers) == {"X-Team": "ghost", "HTTP-Referer": "https://ghost.local/app"}


def test_bad_settings_are_problems_not_crashes():
    env = {
        "GHOST_LLM_PROVIDER": "skynet",
        "GHOST_LLM_HEADERS": "X-Title Ghost",
        "GHOST_LLM_TIMEOUT_SECS": "soon",
        "GHOST_LLM_PASSWORD": "my key",
        "GHOST_LLM_BASE_URL": "localhost:8080",
    }
    c = ModelConnection.from_env(env)
    words = " ".join(c.problems(env))
    for bit in ("GHOST_LLM_PROVIDER=skynet", "Name: value", "seconds", "a name like", "full URL"):
        assert bit in words


def test_keys_never_show():
    env = {"GHOST_LLM_PROVIDER": "openrouter", "OPENROUTER_API_KEY": OR_KEY}
    c = ModelConnection.from_env(env)
    t = c.target(env)
    shown = " ".join([repr(c), repr(t), t.describe(), c.describe(), *c.problems(env)])
    assert OR_KEY not in shown
    assert "via openrouter" in t.describe()
    p = new_pkce()
    assert p.verifier not in repr(p) and p.state not in repr(p)


def test_problems_name_the_missing_password_not_a_value():
    env = {"GHOST_LLM_ENDPOINT": "openrouter"}
    assert ModelConnection.from_env(env).problems(env) == [
        "GHOST_LLM_ENDPOINT=openrouter but OPENROUTER_API_KEY is not set"
    ]
    env = {"GHOST_LLM_PROVIDER": "openrouter"}
    assert ModelConnection.from_env(env).problems(env) == [
        "sign in to OpenRouter (OPENROUTER_API_KEY is not set)"
    ]
    env = {"GHOST_LLM_PROVIDER": "custom", "GHOST_LLM_BASE_URL": "http://x:1"}
    assert ModelConnection.from_env(env).problems(env) == []  # a URL of your own may take no key


def test_openrouter_model_ids():
    assert openrouter_model("claude-sonnet-5-5") == "anthropic/claude-sonnet-5.5"
    assert openrouter_model("claude-haiku-4-5") == "anthropic/claude-haiku-4.5"
    assert openrouter_model("claude-sonnet-5") == "anthropic/claude-sonnet-5"
    assert openrouter_model("openai/gpt-5") == "openai/gpt-5"


def test_headers_parse():
    assert parse_headers("") == {}
    assert parse_headers("X-Title: Ghost; ;HTTP-Referer: https://a.b/c") == {
        "X-Title": "Ghost",
        "HTTP-Referer": "https://a.b/c",
    }
    assert format_headers({"X-Title": "Ghost", "X-B": "1"}) == "X-Title: Ghost; X-B: 1"
    with pytest.raises(ValueError, match="not a header name"):
        parse_headers("bad name: x")
    # What settings.env would quote, or a shell reading it would expand, stays out.
    for value in ('"x"', "$KEY", "${KEY}", "a\\b"):
        with pytest.raises(ValueError, match="can't hold"):
            parse_headers(f"X-A: {value}")


# --- the model list --------------------------------------------------------------------


def transport(seen: list[httpx.Request], status: int = 200, body: object = None):
    def handle(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(status, json=body if body is not None else {"data": []})

    return httpx.MockTransport(handle)


async def test_models_from_anthropic_send_the_key_to_its_own_address_only():
    seen: list[httpx.Request] = []
    env = {"GHOST_LLM_PROVIDER": "anthropic", "ANTHROPIC_API_KEY": "sk-ant-1"}
    body = {
        "data": [
            {"id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5"},
            {"id": "claude-haiku-5-5"},
        ]
    }
    async with httpx.AsyncClient(transport=transport(seen, body=body)) as http:
        models = await list_models(ModelConnection.from_env(env), env, client=http)
    assert [m.id for m in models] == ["claude-haiku-5-5", "claude-sonnet-5-5"]
    assert models[1].name == "Claude Sonnet 5.5"
    r = seen[0]
    assert str(r.url).startswith("https://api.anthropic.com/v1/models")
    assert r.headers["x-api-key"] == "sk-ant-1" and "authorization" not in r.headers


async def test_models_from_an_openai_compatible_server():
    seen: list[httpx.Request] = []
    env = {"GHOST_LLM_PROVIDER": "ollama"}
    body = {"object": "list", "data": [{"id": "qwen3:8b", "object": "model"}]}
    async with httpx.AsyncClient(transport=transport(seen, body=body)) as http:
        models = await list_models(ModelConnection.from_env(env), env, client=http)
    assert [m.id for m in models] == ["qwen3:8b"]
    assert str(seen[0].url) == "http://localhost:11434/v1/models"
    assert "authorization" not in seen[0].headers


async def test_a_refused_key_is_said_without_the_key():
    env = {"GHOST_LLM_PROVIDER": "openai", "OPENAI_API_KEY": "sk-secret-1"}
    async with httpx.AsyncClient(transport=transport([], status=401)) as http:
        with pytest.raises(ValueError) as e:
            await list_models(ModelConnection.from_env(env), env, client=http)
    assert "refused the key" in str(e.value) and "sk-secret-1" not in str(e.value)


# --- OpenRouter's sign-in --------------------------------------------------------------


def test_the_sign_in_address_carries_the_challenge_and_state():
    p = new_pkce()
    u = urlparse(openrouter_auth_url("http://localhost:47823/callback", p))
    q = {k: v[0] for k, v in parse_qs(u.query).items()}
    assert (u.scheme, u.netloc, u.path) == ("https", "openrouter.ai", "/auth")
    assert q == {
        "callback_url": "http://localhost:47823/callback",
        "code_challenge": p.challenge,
        "code_challenge_method": "S256",
        "state": p.state,
    }
    assert p.verifier not in u.query
    import base64
    import hashlib

    want = base64.urlsafe_b64encode(hashlib.sha256(p.verifier.encode()).digest()).rstrip(b"=")
    assert p.challenge == want.decode()


async def test_the_code_is_traded_for_a_key():
    seen: list[httpx.Request] = []
    p = new_pkce()
    async with httpx.AsyncClient(transport=transport(seen, body={"key": OR_KEY})) as http:
        assert await openrouter_exchange("code-1", p, client=http) == OR_KEY
    r = seen[0]
    assert str(r.url) == "https://openrouter.ai/api/v1/auth/keys" and r.method == "POST"
    assert json.loads(r.content) == {
        "code": "code-1",
        "code_verifier": p.verifier,
        "code_challenge_method": "S256",
    }
    async with httpx.AsyncClient(transport=transport([], status=403)) as http:
        with pytest.raises(ValueError, match="refused"):
            await openrouter_exchange("code-1", p, client=http)
