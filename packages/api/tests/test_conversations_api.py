"""Conversation routes (ADR 0014): registry, edits through the gate, switch and
compact delegated to the daemon, voices and samples."""

from __future__ import annotations

from ghost_api import Phase


def test_ghost_listed_first_and_status_carries_active(world):
    client, state, control, *_ = world
    r = client.get("/conversations")
    assert r.status_code == 200
    assert [c["slug"] for c in r.json()] == ["ghost"]
    assert r.json()[0]["is_ghost"] and not r.json()[0]["active"]
    assert client.get("/status").json()["conversation"] is None


def test_create_get_update_and_delete(world):
    client, state, control, gate, writer, loaded, deps = world
    r = client.post(
        "/conversations",
        json={"name": "Accounting", "aliases": ["the accountants"], "voice": "am_adam"},
    )
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["slug"] == "accounting" and d["voice"] == "am_adam" and d["turn_count"] == 0
    assert d["dir"].endswith("conversations/accounting")

    # a duplicate is a conflict, an unknown voice is a validation error
    assert client.post("/conversations", json={"name": "accounting"}).status_code == 409
    assert client.post("/conversations", json={"name": "Ops", "voice": "zz"}).status_code == 422

    r = client.patch(
        "/conversations/accounting",
        json={
            "name": "Finance",
            "persona": "Dry and precise.",
            "memory": "We were reconciling September.",
            "notes": ["books close on the fifth", "  ", "Sam owns payroll"],
            "speed": 1.2,
        },
    )
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["name"] == "Finance" and d["slug"] == "accounting"
    assert d["persona"].strip() == "Dry and precise."
    assert d["memory"].strip() == "We were reconciling September."
    assert d["notes"] == ["books close on the fifth", "Sam owns payroll"]
    assert d["speed"] == 1.2 and d["voice"] == "am_adam"
    assert d["compactions"] == 1  # the memory edit is a version
    files = deps.conversations.files
    assert files.persona("accounting").strip() == "Dry and precise."

    r = client.post("/conversations/accounting/notes", json={"text": "  call   Pat  "})
    assert r.json()["notes"][-1] == "call Pat"

    # every edit is an audited write under the ui session
    audit = client.get("/audit", params={"ability": "conversations"}).json()
    assert audit["total"] >= 6
    assert {e["session_id"] for e in audit["items"]} == {"ui"}

    comps = client.get("/conversations/accounting/compactions").json()
    assert len(comps) == 1 and comps[0]["source"] == "edit"

    assert client.delete("/conversations/ghost").status_code == 409
    assert client.delete("/conversations/accounting").json() == {"ok": True, "detail": None}
    assert client.get("/conversations/accounting").status_code == 404
    assert client.get("/conversations/nope").status_code == 404


def test_reset_and_turns(world):
    client, state, control, gate, writer, loaded, deps = world
    convs = deps.conversations
    convs.record("ghost", "s0", "user", "hello")
    convs.record("ghost", "s0", "assistant", "Hi.")
    turns = client.get("/conversations/ghost/turns").json()
    assert [t["text"] for t in turns] == ["hello", "Hi."] and not turns[0]["compacted"]
    assert client.post("/conversations/ghost/reset").json()["ok"] is True
    assert client.get("/conversations/ghost/turns").json() == []


def test_switch_and_compact_need_a_session(world):
    client, state, control, *_ = world
    assert client.post("/conversations/ghost/switch").status_code == 409
    assert client.post("/conversations/ghost/compact").status_code == 409
    client.post("/conversations", json={"name": "Legal"})
    state.set_phase(Phase.PASSIVE)
    assert client.post("/wake").json()["ok"] is True
    assert client.post("/conversations/legal/switch").json()["ok"] is True
    assert ("switch", "legal") in control.calls
    assert client.get("/status").json()["conversation"] == "legal"
    legal = [c for c in client.get("/conversations").json() if c["slug"] == "legal"][0]
    assert legal["active"] is True
    # the active conversation cannot be reset or deleted from under the session
    assert client.post("/conversations/legal/reset").status_code == 409
    assert client.delete("/conversations/legal").status_code == 409
    assert client.post("/conversations/ghost/compact").status_code == 409
    assert client.post("/conversations/legal/compact").json()["ok"] is True
    assert ("compact", "legal") in control.calls


def test_voices_and_sample(world):
    client, state, control, *_ = world
    r = client.get("/voices").json()
    assert r[0] == {"id": "af_heart", "label": "American female: Heart"}
    assert [v["id"] for v in r] == ["af_heart", "am_adam", "bf_emma"]
    assert client.post("/voices/bf_emma/sample", json={"speed": 0.9}).json()["ok"] is True
    assert ("sample", "bf_emma", 0.9, None) in control.calls
    assert client.post("/voices/zz_nobody/sample").status_code == 404
