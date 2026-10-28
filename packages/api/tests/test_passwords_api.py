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
    assert out["restart_required"] is True
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
    assert all(not p["set"] for p in v["passwords"])
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
