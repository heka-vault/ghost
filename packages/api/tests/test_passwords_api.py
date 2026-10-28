"""The Passwords page's routes (ADR 0036): names and metadata out, values only in. A fake
keyring and temp files throughout."""

from __future__ import annotations

from ghost_core.apps import AppSpec
from ghost_core.envfile import read_env

TOKEN = "sk-ant-api03-thisIsTheOperatorsToken"


def rows(view):
    return {p["name"]: p for p in view["passwords"]}


def test_the_list_names_ghosts_own_and_never_a_value(world):
    client, *_, deps = world
    v = client.get("/passwords").json()
    assert v["vault"]["state"] == "open" and v["vault"]["path"].endswith("passwords.vault")
    own = rows(v)["ANTHROPIC_AUTH_TOKEN"]
    assert own["set"] is False and own["ghost"] is True
    assert own["used_by"] == ["The model connection"]
    assert v["importable"] == []

    r = client.put("/passwords/ANTHROPIC_AUTH_TOKEN", json={"value": TOKEN})
    assert r.status_code == 200 and TOKEN not in r.text
    out = r.json()
    # The model connection reads its key by name at each session (ADR 0037): no restart.
    assert out["restart_required"] is False
    row = rows(out["passwords"])["ANTHROPIC_AUTH_TOKEN"]
    assert row["set"] is True and row["updated"] and row["last_used"] is None
    assert TOKEN not in client.get("/passwords").text
    assert TOKEN not in client.get("/settings").text
    assert {s["name"]: s["set"] for s in client.get("/settings").json()["secrets"]}[
        "ANTHROPIC_AUTH_TOKEN"
    ]
    assert deps.passwords.value("ANTHROPIC_AUTH_TOKEN") == TOKEN


def test_an_apps_password_and_one_of_the_operators_own(world):
    client, *_, deps = world

    class FakeApps:
        specs = {
            "proxy": AppSpec(
                name="proxy", title="Proxy", url="http://x", headers={"A": "${PROXY_KEY}"}
            )
        }

        def is_up(self, name):
            return False

        async def revive(self, name):
            self.revived = name
            return False

    deps.apps = FakeApps()
    proxy = rows(client.get("/passwords").json())["PROXY_KEY"]
    assert proxy == {**proxy, "set": False, "used_by": ["Proxy"], "ghost": False}
    r = client.put("/passwords/PROXY_KEY", json={"value": "proxy-key-0123456789"}).json()
    assert r["restart_required"] is False  # the app connects again now
    assert deps.apps.revived == "proxy"
    mine = client.put("/passwords/MY_SCRIPT_TOKEN", json={"value": "mine-0123456789"}).json()
    assert rows(mine["passwords"])["MY_SCRIPT_TOKEN"]["used_by"] == []
    gone = client.delete("/passwords/MY_SCRIPT_TOKEN").json()
    assert "MY_SCRIPT_TOKEN" not in rows(gone["passwords"])
    assert client.delete("/passwords/MY_SCRIPT_TOKEN").status_code == 404


def test_bad_names_empty_values_and_a_locked_keyring(world):
    client, *_, deps = world
    assert client.put("/passwords/lower", json={"value": "x" * 20}).status_code == 422
    r = client.put("/passwords/A_TOKEN", json={"value": "   "})
    assert r.status_code == 422 and "empty" in r.json()["detail"]
    long = "y" * 20_000
    r = client.put("/passwords/A_TOKEN", json={"value": long})
    assert r.status_code == 422 and long not in r.text  # never echoed back
    deps.passwords.state = "locked"
    deps.passwords.problem = "Your keyring is locked"
    r = client.put("/passwords/A_TOKEN", json={"value": "a-good-value-123"})
    assert r.status_code == 423 and "Unlock" in r.json()["detail"]


def test_unlock_opens_the_vault_and_the_daemon_follows(world):
    client, _state, control, *_, deps = world
    deps.passwords.set("HF_TOKEN", "hf_0123456789abcdef")
    ring = deps.passwords.keyring
    ring.locked = True
    assert not deps.passwords.open()
    v = client.get("/passwords").json()
    assert v["vault"] == {**v["vault"], "state": "locked", "problem": "Your keyring is locked"}
    # Whether each is set can't be known while it's locked: unknown, never "not set".
    assert all(p["set"] is None for p in v["passwords"])
    r = client.delete("/passwords/HF_TOKEN")
    assert r.status_code == 423 and "Unlock" in r.json()["detail"]
    v = client.post("/passwords/unlock").json()
    assert v["vault"]["state"] == "open" and rows(v)["HF_TOKEN"]["set"] is True
    assert ("passwords_opened",) in control.calls


def test_import_moves_the_plaintext_files_into_the_vault(world):
    client, *_, deps = world
    config = deps.env_path.parent
    deps.env_path.write_text(
        f"ANTHROPIC_AUTH_TOKEN={TOKEN}\nANTHROPIC_BASE_URL=http://proxy:8317\n"
    )
    (config / "apps.env").write_text("GHOST_APP_MOBBIN_KEY=mobbin-0123456789\n")
    before = client.get("/passwords").json()
    assert {s["path"].rsplit("/", 1)[-1] for s in before["importable"]} == {"env", "apps.env"}
    r = client.post("/passwords/import")
    assert r.status_code == 200 and TOKEN not in r.text
    out = r.json()
    assert out["imported"] == ["ANTHROPIC_AUTH_TOKEN", "GHOST_APP_MOBBIN_KEY"]
    assert out["settings"] == ["ANTHROPIC_BASE_URL"] and out["restart_required"] is True
    assert {p.rsplit("/", 1)[-1] for p in out["removed"]} == {"env", "apps.env"}
    assert out["passwords"]["importable"] == []
    assert not deps.env_path.exists() and not (config / "apps.env").exists()
    assert read_env(deps.settings_path) == {"ANTHROPIC_BASE_URL": "http://proxy:8317"}
    assert deps.passwords.value("ANTHROPIC_AUTH_TOKEN") == TOKEN


def test_settings_never_shows_or_writes_a_password(world):
    """settings.env is plain text: PUT /settings refuses a password under ``values`` and
    GET /settings never returns one, whatever it is called (ADR 0036, ADR 0037)."""
    client, *_, deps = world

    class FakeApps:
        specs = {
            "proxy": AppSpec(
                name="proxy", title="Proxy", url="http://x", headers={"A": "${PROXY_ACCESS}"}
            )
        }

        def is_up(self, name):
            return True

    deps.apps = FakeApps()
    deps.settings_path.write_text("GHOST_LLM_PASSWORD=MY_MODEL\n")
    for name in (
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "LLM_API_KEY",
        "PROXY_ACCESS",  # an app's ${NAME}, not shaped like a password
        "MY_MODEL",  # the one the model connection names
        "MCP_PROXY_KEY",  # shaped like one
        "GHOST_APP_MOBBIN_KEY",
    ):
        r = client.put("/settings", json={"values": {name: "sk-the-value-0123456789"}})
        assert r.status_code == 422, name
        assert "sk-the-value-0123456789" not in r.text and "Passwords" in r.json()["detail"]
    assert read_env(deps.settings_path) == {"GHOST_LLM_PASSWORD": "MY_MODEL"}

    # One already written there by hand never comes back, and may be taken out.
    deps.passwords.set("HAND_ADDED", "hand-added-0123456789")
    deps.settings_path.write_text(
        "GHOST_LLM_PASSWORD=MY_MODEL\nMCP_PROXY_KEY=proxy-key-0123456789\n"
        "OPENAI_API_KEY=sk-openai-0123456789\nPROXY_ACCESS=proxy-access-0123456\n"
        "MY_MODEL=my-model-key-0123456\nHAND_ADDED=hand-added-0123456789\n"
        "GHOST_REPOS=acme/app\n"
    )
    r = client.get("/settings")
    assert r.json()["values"] == {"GHOST_LLM_PASSWORD": "MY_MODEL", "GHOST_REPOS": "acme/app"}
    for leaked in ("proxy-key", "sk-openai", "proxy-access", "my-model-key", "hand-added"):
        assert leaked not in r.text
    r = client.put("/settings", json={"values": {"MCP_PROXY_KEY": None}})
    assert r.status_code == 200 and "MCP_PROXY_KEY" not in read_env(deps.settings_path)
    assert "proxy-access" not in r.text


def test_no_validation_error_echoes_what_was_sent(world):
    """FastAPI's 422 carries each error's input; Ghost's says where and why only."""
    client, *_ = world
    typed = "sk-typed-" + "k" * 5000
    r = client.post("/model/models", json={"values": {}, "password_value": typed})
    assert r.status_code == 422 and "sk-typed-" not in r.text
    assert r.json()["detail"][0]["loc"] == ["body", "password_value"]
    r = client.put("/settings", json={"secrets": {"GITHUB_TOKEN": ["ghp_inside_a_list_0123"]}})
    assert r.status_code == 422 and "ghp_inside_a_list" not in r.text
    r = client.put("/passwords/A_TOKEN", json={"value": {"nested": "ghp_nested_value_0123"}})
    assert r.status_code == 422 and "ghp_nested_value" not in r.text
    assert set(r.json()["detail"][0]) == {"type", "loc", "msg"}
