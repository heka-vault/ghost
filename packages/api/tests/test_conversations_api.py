"""Agent routes (ADR 0014, ADR 0035): registry, edits through the gate, switch and
compact delegated to the daemon, voices and samples, retire and rehire."""

from __future__ import annotations

from ghost_api import Phase


def test_ghost_listed_first_and_status_carries_active(world):
    client, state, control, *_ = world
    r = client.get("/conversations")
    assert r.status_code == 200
    assert [c["slug"] for c in r.json()] == ["ghost"]
    ghost = r.json()[0]
    assert ghost["is_ghost"] and not ghost["active"] and ghost["voice"] == "af_heart"
    assert "department" not in ghost and "is_default" not in ghost  # ADR 0035
    assert client.get("/status").json()["conversation"] is None


def test_create_get_update_and_delete(world):
    client, state, control, gate, writer, loaded, deps = world
    r = client.post(
        "/conversations",
        json={
            "name": "Accounting",
            "aliases": ["the accountants"],
            "voice": "am_adam",
            "job": "the books",
        },
    )
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["slug"] == "accounting" and d["voice"] == "am_adam" and d["turn_count"] == 0
    assert d["own_voice"] is True and d["job"] == "the books" and "department" not in d
    assert d["dir"].endswith("config/agents/accounting") and d["id"]

    # a duplicate is a conflict, an unknown voice is a validation error
    assert client.post("/conversations", json={"name": "accounting"}).status_code == 409
    assert client.post("/conversations", json={"name": "Ops", "voice": "zz"}).status_code == 422

    r = client.patch(
        "/conversations/accounting",
        json={
            "name": "Finance",
            "job": "the money",
            "persona": "Dry and precise.",
            "memory": "We were reconciling September.",
            "notes": ["books close on the fifth", "  ", "Sam owns payroll"],
            "speed": 1.2,
        },
    )
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["name"] == "Finance" and d["slug"] == "accounting" and d["job"] == "the money"
    assert d["persona"].strip() == "Dry and precise."
    assert d["memory"].strip() == "We were reconciling September."
    assert d["notes"] == ["books close on the fifth", "Sam owns payroll"]
    assert d["speed"] == 1.2 and d["voice"] == "am_adam"
    assert d["compactions"] == 1  # the memory edit is a version
    files = deps.conversations.files
    assert files.persona("accounting").strip() == "Dry and precise."
    # an empty voice means Ghost's
    r = client.patch("/conversations/accounting", json={"voice": ""})
    assert r.json()["own_voice"] is False and r.json()["voice"] == "af_heart"

    r = client.post("/conversations/accounting/notes", json={"text": "  call   Pat  "})
    assert r.json()["notes"][-1] == "call Pat"

    # every edit is an audited write under the ui session
    audit = client.get("/audit", params={"source": "conversations"}).json()
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
    # the active agent cannot be reset or deleted from under the session
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


def test_an_agent_without_its_own_voice_speaks_in_ghosts(world):
    client, *_ = world
    client.patch("/conversations/ghost", json={"voice": "bf_emma"})
    d = client.post("/conversations", json={"name": "Brand"}).json()
    assert d["voice"] == "bf_emma" and d["own_voice"] is False
    # the department field is gone from a hire, and so is the move
    r = client.post("/conversations", json={"name": "Sales", "department": "marketing"})
    assert r.status_code == 200 and "department" not in r.json()
    assert client.post("/conversations/brand/move", json={"department": "x"}).status_code in (
        404,
        405,
    )


def test_retire_rehire_and_seed_from(world):
    client, state, control, gate, writer, loaded, deps = world
    client.post("/conversations", json={"name": "Heka New"})
    client.patch("/conversations/heka-new", json={"memory": "Shipped v1."})
    assert client.post("/conversations/ghost/retire").status_code == 409
    r = client.post("/conversations/heka-new/retire")
    assert r.json() == {"ok": True, "detail": None}
    d = client.get("/conversations/heka-new").json()
    assert d["retired_at"] is not None and d["memory"].strip() == "Shipped v1."  # readable
    assert client.post("/conversations/heka-new/retire").json()["detail"] == "already retired"
    # listed, with retired_at, so the app can group it; filterable
    rows = client.get("/conversations").json()
    assert [c["slug"] for c in rows] == ["ghost", "heka-new"]
    assert [c["slug"] for c in client.get("/conversations", params={"retired": "true"}).json()] == [
        "heka-new"
    ]
    assert [
        c["slug"] for c in client.get("/conversations", params={"retired": "false"}).json()
    ] == ["ghost"]
    assert deps.conversations.resolve("heka new") is None  # out of voice routing
    # the session cannot be switched to a retired agent
    state.set_phase(Phase.PASSIVE)
    client.post("/wake")
    assert client.post("/conversations/heka-new/switch").status_code == 409

    # a new agent seeded from the retired one's memory
    r = client.post("/conversations", json={"name": "Heka Workshop", "seed_from": "heka-new"})
    assert r.status_code == 200, r.text
    assert r.json()["memory"].strip() == "Shipped v1." and r.json()["compactions"] == 1
    r = client.post("/conversations", json={"name": "Nope", "seed_from": "ghost"})
    assert r.status_code == 422 and "private" in r.json()["detail"]
    assert (
        client.post("/conversations", json={"name": "Nope", "seed_from": "zz"}).status_code == 422
    )
    # seed_from on update replaces the memory as an edit
    client.patch("/conversations/heka-new", json={"memory": "Shipped v1. Then v2."})
    r = client.patch("/conversations/heka-workshop", json={"seed_from": "heka-new"})
    assert r.json()["memory"].strip() == "Shipped v1. Then v2."

    r = client.post("/conversations/heka-new/rehire")
    assert r.json()["ok"] is True
    assert client.get("/conversations/heka-new").json()["retired_at"] is None
    assert client.post("/conversations/heka-new/rehire").json()["detail"] == "not retired"
    assert deps.conversations.resolve("heka new").slug == "heka-new"
    # retiring the agent the session is with is refused
    client.post("/conversations/heka-new/switch")
    assert client.post("/conversations/heka-new/retire").status_code == 409

    audit = client.get("/audit", params={"source": "conversations"}).json()
    assert {e["operation"] for e in audit["items"]} >= {"retire", "rehire", "create"}
