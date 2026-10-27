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
    assert s["phase"] == "passive" and s["apps"] == [] and s["pending"] is None
    assert client.post("/wake", json={"text": "what's up"}).json()["ok"] is True
    assert control.calls[-1] == ("wake", "what's up")
    assert client.get("/status").json()["session_id"] == "sess1"
    assert client.post("/wake").json()["ok"] is False  # already awake


def test_wake_can_open_the_session_in_a_line(world):
    """A Call button on a line's page (2026-10-24): the session starts there, not in Ghost."""
    client, state, control, *_ = world
    state.set_phase(Phase.PASSIVE)
    r = client.post("/wake", json={"conversation": "legal"})
    assert r.json()["ok"] is True
    assert control.calls[-1] == ("wake", None, "legal")
    assert client.get("/status").json()["conversation"] == "legal"
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
    assert client.get("/audit", params={"source": "nope"}).json()["total"] == 0
    # A proposal's origin is its source (ghost_core.source); the database says ability.
    (entry,) = client.get("/audit", params={"source": "fake"}).json()["items"]
    assert entry["source"] == "fake" and "ability" not in str(entry)
    assert entry["events"][0]["detail"]["source"] == "fake"


def test_sources_listing(world):
    client, *_ = world
    out = client.get("/sources").json()
    by = {a["name"]: a for a in out}
    assert by["fake"]["kind"] == "app" and by["fake"]["description"] == "fake"
    ops = {o["name"]: o for o in by["fake"]["operations"]}
    assert ops["flag"]["autonomy"] == "announce" and ops["nuke"]["autonomy"] == "confirm"
    assert by["fake"]["read_tools"][0]["name"] == "echo"
    assert by["conversations"]["kind"] == "built-in"
    conv_ops = {o["name"] for o in by["conversations"]["operations"]}
    assert {"retire", "rehire", "create", "rename"} <= conv_ops


def test_policy_round_trip_and_live_reload(world):
    client, state, control, gate, *_ = world
    v = client.get("/policy").json()
    assert v["exists"] is True and v["permissions"]["ask"] == ["fake.nuke"]
    assert gate.level("fake", "nuke").value == "confirm"
    # the raw text path, reloaded live
    r = client.put("/policy", json={"text": '[permissions]\nquiet = ["fake.nuke"]\n'}).json()
    assert r["applied"] is True and r["policy"]["permissions"]["quiet"] == ["fake.nuke"]
    assert gate.level("fake", "nuke").value == "quiet"
    # an older per-source table still reads, as a rule
    r2 = client.put("/policy", json={"text": '[fake.targets."docs"]\nnuke = "deny"\n'}).json()
    assert r2["policy"]["permissions"]["block"] == ["fake.nuke(target=docs)"]
    assert client.put("/policy", json={"text": "[fake]\nnuke = 'yolo'\n"}).status_code == 422
    bad = client.put("/policy", json={"text": '[permissions]\nblock = ["x.(y"]\n'})
    assert bad.status_code == 422


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
        "/sources",
        "/apps",
        "/catalog",
        "/permissions",
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
    assert client.get("/phone").json() == {"address": None, "addresses": [], "device": None}

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


async def test_a_change_from_a_hung_up_session_is_confirmed_from_the_waiting_list(world):
    """ADR 0030: the session ends, the change waits, a Confirm by id applies it."""
    client, state, control, gate, writer, *_ = world
    state.set_phase(Phase.PASSIVE)
    client.post("/wake")
    sub = await pend(gate)
    gate.set_aside("sess1", "hung up")
    client.post("/sleep")
    waiting = client.get("/waiting").json()
    (item,) = waiting["items"]
    assert item["source"] == "gate" and item["change"]["id"] == sub.proposal.id
    assert waiting["count"] == 1 and item["state"] == "new"
    out = client.post(f"/proposals/{sub.proposal.id}/confirm").json()
    assert out["status"] == "success" and writer.tags == ["flagged"]
    assert client.get("/waiting").json()["items"] == []
    entry = client.get("/audit").json()["items"][0]
    assert entry["status"] == "executed"
    assert client.post(f"/proposals/{sub.proposal.id}/confirm").status_code == 409


async def test_acknowledge_and_snooze_an_item(world):
    client, state, control, gate, *_ = world
    sub = await pend(gate)
    gate.set_aside("sess1", "hung up")
    key = client.get("/waiting").json()["items"][0]["key"]
    assert client.post("/waiting/ack", json={"key": key}).json()["ok"] is True
    view = client.get("/waiting").json()
    assert view["items"][0]["state"] == "acknowledged" and view["count"] == 0
    assert client.post("/waiting/snooze", json={"key": key, "minutes": 30}).json()["ok"]
    assert client.get("/waiting").json()["items"][0]["state"] == "snoozed"
    assert client.post("/waiting/ack", json={"key": "nope"}).json()["ok"] is False
    assert client.post(f"/proposals/{sub.proposal.id}/cancel").json()["ok"] is True
    assert client.get("/waiting").json()["items"] == []


def test_routes_are_edited_as_fields_and_kept_with_the_levels(world):
    client, *_, deps = world
    deps.policy_path.write_text('[defaults]\npending_ttl = 900\n[fake]\nnuke = "confirm"\n')
    view = client.get("/policy").json()["routes"]
    assert view["enabled"] == ["digest", "phone"] and view["available"] == ["digest"]
    routes = {
        **view,
        "enabled": ["desk", "digest"],
        "quiet_hours": "22:00-07:00",
        "greeting": "Eric, got a minute?",
    }
    r = client.put("/policy", json={"routes": routes})
    assert r.status_code == 200, r.text
    text = deps.policy_path.read_text()
    assert (
        '[routes]\nenabled = ["digest", "desk"]' in text and 'quiet_hours = "22:00-07:00"' in text
    )
    # the rest of the file as it was, an older table included
    assert '[fake]\nnuke = "confirm"' in text and "pending_ttl = 900" in text
    client.put("/permissions", json={"set": {"rule": "fake.nuke", "level": "quiet"}})
    text = deps.policy_path.read_text()
    assert '"Eric, got a minute?"' in text and 'quiet = ["fake.nuke"]' in text  # kept [routes]
    bad = client.put("/policy", json={"routes": {**routes, "escalate_after": 0}})
    assert bad.status_code == 422


def test_saving_the_routes_keeps_the_rest_of_the_file_comments_and_all(world):
    """Saving "How Ghost reaches you" rewrote policy.toml from the parsed policy and
    dropped every comment the operator (and the shipped example) wrote."""
    from pathlib import Path

    client, *_, deps = world
    example = Path(__file__).parents[3] / "infra" / "policy.example.toml"
    original = example.read_text()
    deps.policy_path.write_text(original)
    view = client.get("/policy").json()["routes"]
    routes = {**view, "enabled": ["digest", "desk"], "urgency": {"eng/ci": "call"}}
    assert client.put("/policy", json={"routes": routes}).status_code == 200
    text = deps.policy_path.read_text()
    assert text.startswith(original.rstrip("\n"))  # every line of it, comments included
    assert '[routes]\nenabled = ["digest", "desk"]' in text
    assert '[routes.urgency]\n"eng/ci" = "call"' in text
    # Saved again: the routes table is replaced, not added, and the rest stays.
    routes = {**routes, "enabled": ["digest"], "urgency": {}}
    assert client.put("/policy", json={"routes": routes}).status_code == 200
    again = deps.policy_path.read_text()
    assert again.startswith(original.rstrip("\n"))
    assert again.count("\n[routes]") == 1 and "\n[routes.urgency]" not in again
    assert '\n[routes]\nenabled = ["digest"]' in again
    # A table in the middle of the file is replaced where it is.
    deps.policy_path.write_text(
        '# mine\n[routes]\n# the old way\nenabled = ["digest"]\n\n'
        "# keep this: it is about the defaults\n[defaults]\npending_ttl = 900\n"
    )
    assert client.put("/policy", json={"routes": routes}).status_code == 200
    text = deps.policy_path.read_text()
    assert text.startswith('# mine\n[routes]\nenabled = ["digest"]')
    assert "# keep this: it is about the defaults\n[defaults]\npending_ttl = 900\n" in text
