"""The model connection's routes (ADR 0036): the view, saving through PUT /settings with a
Password that goes live, the model list, Test connection and OpenRouter's sign-in. No key
ever comes back out."""

from __future__ import annotations

import time
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient
from ghost_api import create_app
from ghost_api import model as model_routes
from ghost_core.connection import ModelOption

OR_KEY = "sk-or-v1-fedcba9876543210"
OPENAI_KEY = "sk-openai-0123456789"
TYPED_KEY = "sk-typed-but-not-saved"
NAMES = (
    "GHOST_LLM_PROVIDER",
    "GHOST_LLM_ENDPOINT",
    "GHOST_LLM_MODEL",
    "GHOST_LLM_PROTOCOL",
    "GHOST_LLM_BASE_URL",
    "GHOST_LLM_PASSWORD",
    "GHOST_LLM_HEADERS",
    "GHOST_LLM_TIMEOUT_SECS",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLIPROXY_BASE_URL",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "LLM_API_KEY",
)


@pytest.fixture
def env(monkeypatch):
    for k in NAMES:
        monkeypatch.delenv(k, raising=False)  # restored after, whatever the routes set
    return monkeypatch


def test_the_setup_from_before_shows_as_openrouter_without_its_key(world, env):
    client, *_, deps = world
    env.setenv("GHOST_LLM_ENDPOINT", "openrouter")
    env.setenv("OPENROUTER_API_KEY", OR_KEY)
    r = client.get("/model")
    assert r.status_code == 200
    v = r.json()
    assert (v["provider"], v["protocol"], v["wire_model"]) == (
        "openrouter",
        "anthropic",
        "anthropic/claude-sonnet-5.5",
    )
    assert v["password"] == "OPENROUTER_API_KEY" and v["password_set"] is True
    assert v["legacy"] == "GHOST_LLM_ENDPOINT=openrouter" and v["ready"] is True
    assert v["values"] == {"GHOST_LLM_PROVIDER": "openrouter"}
    assert {p["id"] for p in v["providers"]} >= {"anthropic", "openrouter", "ollama", "custom"}
    assert OR_KEY not in r.text


def test_a_key_in_the_vault_counts_without_coming_out(world, env):
    client, *_, deps = world
    env.setenv("GHOST_LLM_PROVIDER", "openrouter")
    deps.passwords.set("OPENROUTER_API_KEY", OR_KEY)
    r = client.get("/model")
    assert r.json()["password_set"] is True and r.json()["ready"] is True
    assert OR_KEY not in r.text
    # Read for a view, it isn't a use: "last used" stays empty.
    assert {m.name: m.last_used for m in deps.passwords.info()}["OPENROUTER_API_KEY"] is None


def test_saving_a_connection_and_its_password_is_live(world, env):
    client, *_, deps = world
    r = client.put(
        "/settings",
        json={"values": {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5-mini"}},
    )
    assert r.status_code == 200, r.text
    # The key is a password like any other, saved on its own into the vault (ADR 0036).
    r = client.put("/passwords/OPENAI_API_KEY", json={"value": OPENAI_KEY})
    assert r.status_code == 200, r.text
    assert r.json()["restart_required"] is False  # the next session reads it by name
    assert OPENAI_KEY not in r.text
    assert deps.passwords.value("OPENAI_API_KEY") == OPENAI_KEY
    assert not (deps.env_path.parent / "apps.env").exists() and not deps.env_path.exists()
    v = client.get("/model").json()
    assert (v["provider"], v["model"], v["password_set"]) == ("openai", "gpt-5-mini", True)
    rows = {p["name"]: p for p in client.get("/passwords").json()["passwords"]}
    assert rows["OPENAI_API_KEY"]["set"] and rows["OPENAI_API_KEY"]["ghost"]
    assert rows["OPENAI_API_KEY"]["used_by"] == ["The model connection"]


def test_the_passwords_page_lists_only_the_model_keys_in_play(world, env):
    client, *_ = world
    names = [p["name"] for p in client.get("/passwords").json()["passwords"]]
    # Nothing set up: Anthropic is the provider, so its key is listed; the others wait.
    assert "ANTHROPIC_API_KEY" in names
    assert "OPENAI_API_KEY" not in names and "LLM_API_KEY" not in names
    assert "OPENROUTER_API_KEY" in names  # Jev's too


def test_jev_still_wants_a_restart_for_openrouters_key(world, env):
    client, *_ = world
    r = client.put("/passwords/OPENROUTER_API_KEY", json={"value": OR_KEY})
    assert r.json()["restart_required"] is True


def test_a_password_the_connection_doesnt_name_is_refused(world, env):
    client, *_ = world
    r = client.put("/settings", json={"secrets": {"SOMEBODY_ELSES_KEY": "x"}})
    assert r.status_code == 422


def test_a_custom_password_name_is_the_models_once_named(world, env):
    client, *_ = world
    values = {
        "GHOST_LLM_PROVIDER": "custom",
        "GHOST_LLM_BASE_URL": "http://gpu-box:8000/v1",
        "GHOST_LLM_PROTOCOL": "openai",
        "GHOST_LLM_PASSWORD": "GPU_BOX_KEY",
    }
    assert client.put("/settings", json={"values": values}).status_code == 200
    r = client.put("/passwords/GPU_BOX_KEY", json={"value": "k-0123456789"})
    assert r.status_code == 200 and r.json()["restart_required"] is False
    rows = {p["name"]: p for p in r.json()["passwords"]["passwords"]}
    assert rows["GPU_BOX_KEY"]["used_by"] == ["The model connection"]
    assert client.get("/model").json()["password_set"] is True


async def saved_list(conn, env):
    assert env.get(conn.password) == OPENAI_KEY  # from the vault, by name
    return [ModelOption("gpt-5")]


def test_the_model_list_reads_the_saved_key_from_the_vault(world, env):
    client, *_, deps = world
    deps.passwords.set("OPENAI_API_KEY", OPENAI_KEY)
    env.setattr(model_routes, "list_models", saved_list)
    r = client.post("/model/models", json={"values": {"GHOST_LLM_PROVIDER": "openai"}})
    assert r.json()["models"] == [{"id": "gpt-5", "name": None}]
    assert OPENAI_KEY not in r.text
    used = {m.name: m.last_used for m in deps.passwords.info()}
    assert used["OPENAI_API_KEY"] is not None  # a list is a use


async def fake_list(conn, env):
    assert env.get(conn.password) == TYPED_KEY  # the typed key, for this request only
    return [ModelOption("gpt-5"), ModelOption("gpt-5-mini", "GPT-5 mini")]


def test_the_model_list_for_a_draft(world, env):
    client, *_ = world
    env.setattr(model_routes, "list_models", fake_list)
    r = client.post(
        "/model/models",
        json={"values": {"GHOST_LLM_PROVIDER": "openai"}, "password_value": TYPED_KEY},
    )
    assert r.status_code == 200
    assert r.json() == {
        "models": [{"id": "gpt-5", "name": None}, {"id": "gpt-5-mini", "name": "GPT-5 mini"}],
        "default": "gpt-5",
        "error": None,
    }
    assert TYPED_KEY not in r.text


def test_a_model_list_that_cant_be_read_says_why(world, env):
    client, *_ = world

    async def unreachable(conn, env):
        raise ValueError("couldn’t reach http://localhost:11434 (ConnectError)")

    env.setattr(model_routes, "list_models", unreachable)
    r = client.post("/model/models", json={"values": {"GHOST_LLM_PROVIDER": "ollama"}})
    assert r.status_code == 200
    assert r.json()["models"] == [] and "couldn’t reach" in r.json()["error"]


def test_a_draft_takes_only_connection_settings(world, env):
    client, *_ = world
    r = client.post("/model/models", json={"values": {"GHOST_WAKE_PHRASES": "x"}})
    assert r.status_code == 422


def test_test_connection_times_the_first_token(world, env):
    client, *_, deps = world
    seen = {}

    async def probe(e, prompt, max_tokens):
        seen.update(env=dict(e), prompt=prompt, max_tokens=max_tokens)
        return SimpleNamespace(
            ttft=0.4123,
            total=0.61,
            text="Hello there, nice to meet.",
            model="gpt-5-2026",
            error=None,
        )

    deps.model_probe = probe
    r = client.post(
        "/model/test",
        json={
            "values": {"GHOST_LLM_PROVIDER": "openai", "GHOST_LLM_MODEL": "gpt-5"},
            "password_value": TYPED_KEY,
        },
    )
    assert r.status_code == 200
    v = r.json()
    assert v["ok"] is True and (v["ttft_ms"], v["total_ms"]) == (412, 610)
    assert v["model"] == "gpt-5" and v["answered_by"] == "gpt-5-2026"
    assert v["where"].startswith("openai (openai, https://api.openai.com")
    assert seen["env"]["OPENAI_API_KEY"] == TYPED_KEY and seen["max_tokens"] <= 64
    assert TYPED_KEY not in r.text
    import os

    assert "OPENAI_API_KEY" not in os.environ  # tried, not kept


def test_a_failed_test_says_why(world, env):
    client, *_, deps = world

    async def probe(e, prompt, max_tokens):
        return SimpleNamespace(
            ttft=None, total=None, text="", model="", error="AuthenticationError (HTTP 401)"
        )

    deps.model_probe = probe
    v = client.post("/model/test", json={"values": {"GHOST_LLM_PROVIDER": "anthropic"}}).json()
    assert v["ok"] is False and v["error"] == "AuthenticationError (HTTP 401)"


class FakeSignIns:
    port = 47823
    localhost_redirect_uri = "http://localhost:47823/callback"

    def __init__(self) -> None:
        self.urls: list[str] = []

    async def browser_return(self, name, url, wait_secs):
        self.urls.append(url)
        return {"code": "the-code"}


def test_signing_in_to_openrouter_keeps_the_key_as_a_password(world, env):
    _, *_, deps = world
    env.setenv("GHOST_LLM_PROVIDER", "openrouter")
    deps.signins = FakeSignIns()
    traded = []

    async def exchange(code, pkce):
        traded.append(code)
        return OR_KEY

    env.setattr(model_routes, "openrouter_exchange", exchange)
    with TestClient(create_app(deps)) as client:
        r = client.post("/model/sign-in")
        assert r.status_code == 200
        url = r.json()["url"]
        assert url.startswith("https://openrouter.ai/auth?")
        assert "callback_url=http%3A%2F%2Flocalhost%3A47823%2Fcallback" in url
        assert "code_challenge_method=S256" in url
        for _ in range(50):
            v = client.get("/model").json()
            if not v["sign_in_pending"]:
                break
            time.sleep(0.02)
    assert traded == ["the-code"]
    assert deps.passwords.value("OPENROUTER_API_KEY") == OR_KEY  # in the vault (ADR 0036)
    assert not deps.env_path.exists()
    assert v["sign_in_error"] is None and v["password_set"] is True
    assert OR_KEY not in r.text


def test_a_sign_in_that_comes_back_for_another_request_is_refused(world, env):
    _, *_, deps = world

    class Forged(FakeSignIns):
        async def browser_return(self, name, url, wait_secs):
            return {"code": "c", "state": "not-ours"}

    deps.signins = Forged()
    with TestClient(create_app(deps)) as client:
        client.post("/model/sign-in")
        for _ in range(50):
            v = client.get("/model").json()
            if not v["sign_in_pending"]:
                break
            time.sleep(0.02)
    assert "another request" in (v["sign_in_error"] or "")
    assert "OPENROUTER_API_KEY" not in deps.passwords


def test_no_sign_in_while_the_vault_is_locked(world, env):
    client, *_, deps = world
    deps.signins = FakeSignIns()
    from ghost_core.passwords import MemoryKeyring, Passwords

    deps.passwords.set("GITHUB_TOKEN", "ghp_0123456789")  # a vault on disk to open
    locked = Passwords(deps.passwords.path, MemoryKeyring(locked=True), environ={})
    locked.open()
    deps.passwords = locked
    r = client.post("/model/sign-in")
    assert r.status_code == 423 and "Unlock" in r.json()["detail"]
    assert deps.signins.urls == []  # nobody was sent to sign in


def test_no_list_is_asked_for_without_the_key(world, env):
    client, *_ = world

    async def never(conn, env):
        raise AssertionError("asked without a key")

    env.setattr(model_routes, "list_models", never)
    r = client.post("/model/models", json={"values": {"GHOST_LLM_PROVIDER": "anthropic"}})
    assert r.json()["error"] == "Save its password to see the models it offers."


def test_a_locked_keyring_is_said_not_a_missing_key(world, env):
    """While the vault can't be read the environment has no key; that is not "not set"."""
    from ghost_core.passwords import MemoryKeyring, Passwords

    client, *_, deps = world
    env.setenv("GHOST_LLM_PROVIDER", "openrouter")
    deps.passwords.set("OPENROUTER_API_KEY", OR_KEY)
    locked = Passwords(deps.passwords.path, MemoryKeyring(locked=True), environ={})
    locked.open()
    deps.passwords = locked

    async def never(conn, env):
        raise AssertionError("asked without a key")

    env.setattr(model_routes, "list_models", never)
    v = client.get("/model").json()
    assert v["ready"] is False and v["password_set"] is False
    assert len(v["problems"]) == 1
    assert "keyring is locked" in v["problems"][0] and "not set" not in v["problems"][0]
    r = client.post("/model/models", json={"values": {"GHOST_LLM_PROVIDER": "anthropic"}})
    assert r.json()["error"].startswith("Unlock your keyring")
