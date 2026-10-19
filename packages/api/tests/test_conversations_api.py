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


# --- scope and settle (ADR 0019) -------------------------------------------------------


def test_scope_round_trips_and_is_validated(world):
    client, state, control, gate, writer, loaded, deps = world
    deps.conversations._scopes = {"fake": lambda: ["acme/app", "acme/infra"]}  # noqa: SLF001
    r = client.post("/conversations", json={"name": "App", "scope": {"fake": ["ACME/app"]}})
    assert r.status_code == 200, r.text
    d = r.json()
    assert d["scope"] == {"fake": ["acme/app"]} and d["settled_at"] is None
    assert d["voice"] == "am_adam"  # an unused voice when none is given
    assert client.get("/conversations/app").json()["scope"] == {"fake": ["acme/app"]}
    # outside the allowlist, an unknown ability, and Ghost: validation errors
    r = client.post("/conversations", json={"name": "Bad", "scope": {"fake": ["acme/x"]}})
    assert r.status_code == 422 and "acme/x" in r.json()["detail"]
    r = client.post("/conversations", json={"name": "Bad", "scope": {"nope": ["x"]}})
    assert r.status_code == 422
    r = client.patch("/conversations/ghost", json={"scope": {"fake": ["acme/app"]}})
    assert r.status_code == 422 and "Ghost" in r.json()["detail"]
    # update replaces the whole map; {} makes it unscoped
    r = client.patch("/conversations/app", json={"scope": {"fake": ["acme/infra", "acme/app"]}})
    assert r.json()["scope"] == {"fake": ["acme/infra", "acme/app"]}
    r = client.patch("/conversations/app", json={"scope": {}})
    assert r.json()["scope"] == {}
    ghost = client.get("/conversations/ghost").json()
    assert ghost["scope"] == {} and ghost["settled_at"] is None


def test_settle_unsettle_and_seed_from(world):
    client, state, control, gate, writer, loaded, deps = world
    client.post("/conversations", json={"name": "Heka New"})
    client.patch("/conversations/heka-new", json={"memory": "Shipped v1."})
    assert client.post("/conversations/ghost/settle").status_code == 409
    r = client.post("/conversations/heka-new/settle")
    assert r.json() == {"ok": True, "detail": None}
    d = client.get("/conversations/heka-new").json()
    assert d["settled_at"] is not None and d["memory"].strip() == "Shipped v1."  # readable
    assert client.post("/conversations/heka-new/settle").json()["detail"] == "already settled"
    # listed, with settled_at, so the app can group it; filterable
    rows = client.get("/conversations").json()
    assert [c["slug"] for c in rows] == ["ghost", "heka-new"]
    assert [c["slug"] for c in client.get("/conversations", params={"settled": "true"}).json()] == [
        "heka-new"
    ]
    assert [
        c["slug"] for c in client.get("/conversations", params={"settled": "false"}).json()
    ] == ["ghost"]
    assert deps.conversations.resolve("heka new") is None  # out of voice routing
    # the session cannot be switched into a settled thread
    state.set_phase(Phase.PASSIVE)
    client.post("/wake")
    assert client.post("/conversations/heka-new/switch").status_code == 409

    # a new thread seeded from the settled one's memory
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

    r = client.post("/conversations/heka-new/unsettle")
    assert r.json()["ok"] is True
    assert client.get("/conversations/heka-new").json()["settled_at"] is None
    assert client.post("/conversations/heka-new/unsettle").json()["detail"] == "not settled"
    assert deps.conversations.resolve("heka new").slug == "heka-new"
    # settling the conversation the session is in is refused
    client.post("/conversations/heka-new/switch")
    assert client.post("/conversations/heka-new/settle").status_code == 409

    audit = client.get("/audit", params={"ability": "conversations"}).json()
    assert {e["operation"] for e in audit["items"]} >= {"settle", "unsettle", "create"}
