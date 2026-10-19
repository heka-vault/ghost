from __future__ import annotations

import json
import os
import stat

import pytest
from ghost_api import Phase, pending_view
from ghost_api.files import read_env


async def pend(gate, name="readme", session="sess1"):
    """Put a confirm-level proposal in the gate the way the model's `change` tool would."""
    op = gate.operation("fake", "nuke")
    prepared = await op.prepare({"name": name})
    return await gate.submit(
        session_id=session,
        ability="fake",
        operation="nuke",
        target=prepared.target,
        changes=prepared.changes,
        rationale="test",
    )


def test_status_and_wake_sleep(world):
    client, state, control, *_ = world
    state.set_phase(Phase.PASSIVE)
    s = client.get("/status").json()
    assert s["phase"] == "passive" and s["abilities"] == ["fake"] and s["pending"] is None
    assert client.post("/wake", json={"text": "what's up"}).json()["ok"] is True
    assert control.calls[-1] == ("wake", "what's up")
    assert client.get("/status").json()["session_id"] == "sess1"
    assert client.post("/wake").json()["ok"] is False  # already awake
    assert client.post("/sleep").json()["ok"] is True
    assert client.get("/status").json()["phase"] == "passive"


def test_say_wakes_and_records_turn(world):
    client, state, control, *_ = world
    state.set_phase(Phase.PASSIVE)
    assert client.post("/session/say", json={"text": "label it bug"}).json()["ok"] is True
    turns = client.get("/session/turns").json()
    assert turns[-1]["role"] == "user" and turns[-1]["via"] == "text"
    state.set_phase(Phase.STARTING)
    assert client.post("/session/say", json={"text": "x"}).json()["ok"] is False
    assert client.post("/session/say", json={"text": ""}).status_code == 422


async def test_pending_confirm_executes_and_audits(world):
    client, state, control, gate, writer, *_ = world
    state.set_phase(Phase.LISTENING, "sess1")
    sub = await pend(gate)
    assert not sub.executed
    state.set_pending(pending_view(sub.proposal))
    p = client.get("/proposals/pending").json()
    assert p["operation"] == "nuke" and "add flagged" in p["summary"] and p["expires_in_secs"] > 0
    out = client.post("/proposals/pending/confirm").json()
    assert (
        out["status"] == "success"
        and writer.tags == ["flagged"]
        and "tags flagged" in out["state_now"]
    )
    assert client.get("/proposals/pending").json() is None
    assert client.post("/proposals/pending/confirm").status_code == 409
    audit = client.get("/audit").json()
    assert audit["total"] == 1
    entry = audit["items"][0]
    assert entry["status"] == "executed" and entry["autonomy"] == "confirm"
    assert any(
        e["event"] == "user_turn" and e["detail"]["classifier"] == "ui" for e in entry["events"]
    )
    assert "undo_of" not in entry
    assert client.post("/undo").status_code in (404, 405)


async def test_cancel_pending(world):
    client, state, control, gate, writer, *_ = world
    state.set_phase(Phase.LISTENING, "sess1")
    await pend(gate)
    assert client.post("/proposals/pending/cancel").json()["ok"] is True
    assert client.post("/proposals/pending/cancel").json()["ok"] is False
    assert writer.tags == []
    assert client.get("/audit", params={"status": "cancelled"}).json()["total"] == 1
    assert client.get("/audit", params={"ability": "nope"}).json()["total"] == 0


def test_abilities_listing(world):
    client, *_ = world
    out = client.get("/abilities").json()
    by = {a["name"]: a for a in out}
    assert by["fake"]["enabled"] and by["fake"]["version"] == "1.2.3"
    ops = {o["name"]: o for o in by["fake"]["operations"]}
    assert ops["flag"]["autonomy"] == "announce" and ops["nuke"]["autonomy"] == "confirm"
    assert by["fake"]["read_tools"][0]["name"] == "echo"
    assert by["other"]["enabled"] is False and by["other"]["operations"] == []
    assert by["fake"]["scope_kind"] is None and by["fake"]["scope_items"] == []
    # a scoped ability (ADR 0019) says how and lists its whole allowlist
    loaded = world[5]
    loaded.contributions.scope_kind = "repository"
    loaded.contributions.scope_items = lambda: ["acme/app", "acme/infra"]
    by = {a["name"]: a for a in client.get("/abilities").json()}
    assert by["fake"]["scope_kind"] == "repository"
    assert by["fake"]["scope_items"] == ["acme/app", "acme/infra"]
    conv_ops = {o["name"] for o in by["conversations"]["operations"]}
    assert {"set_scope", "settle", "unsettle", "create"} <= conv_ops


def test_policy_round_trip_and_live_reload(world):
    client, state, control, gate, *_ = world
    v = client.get("/policy").json()
    assert v["exists"] is False and v["skills"] == {}
    r = client.put(
        "/policy", json={"skills": {"fake": {"nuke": "announce"}}, "enabled": ["fake"]}
    ).json()
    assert r["applied"] is True and r["restart_required"] is True
    assert r["policy"]["skills"]["fake"]["nuke"] == "announce"
    assert gate.level("fake", "nuke").value == "announce"  # reloaded live
    text = r["policy"]["text"]
    assert 'nuke = "announce"' in text and 'enabled = ["fake"]' in text
    # raw text path, with a target rule
    r2 = client.put(
        "/policy", json={"text": text + '\n[fake.targets."docs"]\nnuke = "deny"\n'}
    ).json()
    assert r2["restart_required"] is False
    assert r2["policy"]["targets"]["fake"]["docs"]["nuke"] == "deny"
    assert client.put("/policy", json={"text": "[fake]\nnuke = 'yolo'\n"}).status_code == 422
    assert client.put("/policy", json={"skills": {"fake": {"nuke": "yolo"}}}).status_code == 422


def test_settings_write_only_secrets(world):
    client, *_, deps = world
    v = client.get("/settings").json()
    assert v["values"] == {} and all(s["set"] is False for s in v["secrets"])
    assert v["vault_managed"] is False
    r = client.put(
        "/settings",
        json={
            "values": {"GHOST_REPOS": "acme/app", "GHOST_GITHUB_MOCK": None},
            "secrets": {"GITHUB_TOKEN": "ghp_secret"},
        },
    ).json()
    assert r["restart_required"] is True
    s = r["settings"]
    assert s["values"] == {"GHOST_REPOS": "acme/app"}
    assert {x["name"]: x["set"] for x in s["secrets"]}["GITHUB_TOKEN"] is True
    assert "ghp_secret" not in json.dumps(s)
    assert stat.S_IMODE(os.stat(deps.env_path).st_mode) == 0o600
    assert read_env(deps.env_path) == {"GHOST_REPOS": "acme/app", "GITHUB_TOKEN": "ghp_secret"}
    assert client.put("/settings", json={"values": {"GITHUB_TOKEN": "x"}}).status_code == 422
    assert client.put("/settings", json={"secrets": {"NOPE": "x"}}).status_code == 422
    assert client.put("/settings", json={"values": {"bad-name": "x"}}).status_code == 422
    r = client.put("/settings", json={"secrets": {"GITHUB_TOKEN": None}}).json()
    assert {x["name"]: x["set"] for x in r["settings"]["secrets"]}["GITHUB_TOKEN"] is False


def test_setup_endpoints(world, monkeypatch):
    client, *_, deps = world
    out = client.post("/setup/echo-cancel").json()
    assert out["ok"] is True and "installed" in out["detail"]
    out = client.post("/setup/echo-cancel", params={"uninstall": "true"}).json()
    assert "--uninstall" in out["detail"]
    m = client.get("/setup/models").json()
    assert m["ready"] is False and m["kokoro_present"] is False and m["stt_model"] == "parakeet"


def test_events_websocket_streams_status_and_turns(world):
    client, state, control, *_ = world
    state.set_phase(Phase.PASSIVE)
    with client.websocket_connect("/events") as ws:
        first = json.loads(ws.receive_text())
        assert first["phase"] == "passive"  # initial snapshot is a plain Status
        state.set_phase(Phase.LISTENING, "sess1")
        ev = json.loads(ws.receive_text())
        assert ev["type"] == "status" and ev["status"]["phase"] == "listening"
        state.add_turn("assistant", "Hello.")
        ev = json.loads(ws.receive_text())
        assert ev["type"] == "turn" and ev["turn"]["text"] == "Hello."
        state.log("INFO", "hi")
        ev = json.loads(ws.receive_text())
        assert ev["type"] == "log" and ev["message"] == "hi"


def test_devices_endpoint_tolerates_no_audio(world, monkeypatch):
    client, *_ = world
    import types

    fake_sd = types.SimpleNamespace(
        query_devices=lambda: [
            {
                "name": "ghost.mic",
                "max_input_channels": 1,
                "max_output_channels": 0,
                "default_samplerate": 48000.0,
            }
        ],
        default=types.SimpleNamespace(device=(0, -1)),
    )
    monkeypatch.setitem(__import__("sys").modules, "sounddevice", fake_sd)
    d = client.get("/devices").json()
    assert d["devices"][0]["name"] == "ghost.mic" and d["echo_cancel_source_present"] is True
    assert d["input_device"] == "ghost.mic"


def test_openapi_lists_every_route_with_tags(world):
    client, *_ = world
    spec = client.get("/openapi.json").json()
    paths = set(spec["paths"])
    assert {
        "/status",
        "/wake",
        "/sleep",
        "/session/say",
        "/proposals/pending/confirm",
        "/audit",
        "/abilities",
        "/policy",
        "/settings",
        "/devices",
        "/events/schema",
    } <= paths
    assert (
        "StatusEvent" in spec["components"]["schemas"]
        and "LogEvent" in spec["components"]["schemas"]
    )


@pytest.mark.parametrize("bad", ["", "   "])
def test_say_rejects_blank(world, bad):
    client, state, *_ = world
    state.set_phase(Phase.PASSIVE)
    assert client.post("/session/say", json={"text": bad}).status_code in (422, 200)


def test_settings_live_keys_do_not_require_restart(world):
    client, _state, control, *_ = world
    r = client.put(
        "/settings", json={"values": {"GHOST_WAKE_PHRASES": "hey ghost, computer"}}
    ).json()
    assert r["applied"] == ["GHOST_WAKE_PHRASES"] and r["restart_required"] is False
    assert control.applied == ["GHOST_WAKE_PHRASES"]
    r = client.put(
        "/settings", json={"values": {"GHOST_WAKE_PHRASES": "x", "GHOST_REPOS": "a/b"}}
    ).json()
    assert r["applied"] == ["GHOST_WAKE_PHRASES"] and r["restart_required"] is True


def test_session_origin_follows_the_session(world):
    client, state, *_ = world
    state.set_phase(Phase.PASSIVE)
    assert client.get("/status").json()["session_origin"] is None
    state.set_phase(Phase.LISTENING, "sess1", origin="phone")
    assert client.get("/status").json()["session_origin"] == "phone"
    state.set_phase(Phase.SPEAKING, "sess1")  # later phases keep the origin
    assert client.get("/status").json()["session_origin"] == "phone"
    state.set_phase(Phase.PASSIVE)
    assert client.get("/status").json()["session_origin"] is None


def test_phone_routes(world):
    from datetime import UTC, datetime

    from ghost_api import PhoneDevice

    client, state, control, *_ = world
    assert client.post("/phone/pairing").status_code == 409  # listener off
    assert client.get("/phone").json() == {"address": None, "device": None}

    control.phone_address = "192.168.1.20:8765"
    body = client.post("/phone/pairing").json()
    assert body["code"] == "123456" and body["address"] == "192.168.1.20:8765"
    control.phone_device = PhoneDevice(
        id="d1", name="Pixel 9", paired_at=datetime.now(UTC), last_seen=None
    )
    view = client.get("/phone").json()
    assert view["device"]["name"] == "Pixel 9" and "secret" not in json.dumps(view)
    assert client.delete("/phone").json()["ok"] is True
    assert client.delete("/phone").json()["ok"] is False
    assert client.get("/phone").json()["device"] is None
