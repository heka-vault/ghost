"""What the desktop reads from Ghost's folder (ADR 0035): the rules, the outputs, the
watches and webhooks, with no department in any of it."""

from __future__ import annotations

import json

WATCH = {
    "app": "github",
    "read": "list_workflow_runs",
    "args": {"owner": "heka-vault", "repo": "ghost"},
    "every": 60,
    "items": "workflow_runs",
    "fields": {"id": {"at": "id", "type": "int"}},
    "key": "{id}",
    "title": "CI failed: {id}",
}
HOOK = {
    "verify": "github",
    "secret": "${GITHUB_HOOK_SECRET}",
    "events": {
        "workflow_run": {
            "fields": {
                "id": {"at": "workflow_run.id", "type": "int"},
                "conclusion": {"at": "workflow_run.conclusion"},
            },
            "when": [{"field": "conclusion", "op": "==", "value": "failure"}],
            "key": "{id}",
            "title": "CI failed: {id}",
            "check": {
                "app": "github",
                "read": "get_workflow_run",
                "args": {"run_id": "{id}"},
                "fields": {"conclusion": {"at": "conclusion"}},
            },
        }
    },
}


def test_the_rules_are_read_and_added_through_the_gate(world):
    client, state, control, gate, writer, loaded, deps = world
    r = client.get("/rules")
    assert r.status_code == 200
    assert r.json() == {"path": str(deps.rules.path), "text": ""}
    r = client.post("/rules", json={"text": "  Always use UK dates.  "})
    assert r.status_code == 200, r.text
    out = r.json()
    assert out["status"] == "success" and out["applied"] == ["add the rule: Always use UK dates."]
    assert client.get("/rules").json()["text"] == "# Ghost\n\nAlways use UK dates.\n"
    # through the gate: audited under the desktop's own session, like the spoken rules.add
    audit = client.get("/audit", params={"source": "rules"}).json()
    (entry,) = audit["items"]
    assert entry["operation"] == "add" and entry["session_id"] == "ui"
    assert "department" not in entry
    assert client.post("/rules", json={"text": ""}).status_code == 422
    assert client.post("/rules", json={"text": "x" * 1001}).status_code == 422


def test_a_rule_the_permissions_block_is_refused(world, tmp_path):
    client, state, control, gate, writer, loaded, deps = world
    deps.policy_path.write_text('[permissions]\nblock = ["rules"]\n')
    control.reload_policy()
    r = client.post("/rules", json={"text": "Be brief."})
    assert r.status_code == 409 and "blocked" in r.json()["detail"]
    assert client.get("/rules").json()["text"] == ""


def test_outputs_are_listed_from_one_folder(world):
    client, state, control, gate, writer, loaded, deps = world
    assert client.get("/outputs").json() == []
    out = deps.outputs.folder
    out.mkdir(parents=True)
    (out / "week.html").write_text("<h1>Week</h1>")
    (out / ".hidden.md").write_text("x")
    rows = client.get("/outputs").json()
    assert [(f["name"], f["bytes"]) for f in rows] == [("week.html", 13)]
    assert rows[0]["path"] == str(out / "week.html") and rows[0]["modified"]


def test_watches_lists_the_watches_and_hooks_and_what_is_wrong(world):
    """``Watch.app`` is the app it reads (ADR 0034); the listing said ``server`` and failed."""
    client, state, control, gate, writer, loaded, deps = world
    assert client.get("/watches").json() == {
        "watches": [],
        "hooks": [],
        "listening": [],
        "problems": [],
    }
    config = deps.folder.path
    (config / "watches.json").write_text(json.dumps({"ci": WATCH}))
    (config / "hooks.json").write_text(json.dumps({"ci": HOOK}))
    deps.hooks_listening = ["127.0.0.1:8766"]
    r = client.get("/watches")  # re-read now
    assert r.status_code == 200, r.text
    v = r.json()
    assert [(w["name"], w["app"], w["read"]) for w in v["watches"]] == [
        ("ci", "github", "list_workflow_runs")
    ]
    assert [(h["name"], h["path"], h["on"]) for h in v["hooks"]] == [("ci", "/hooks/ci", False)]
    assert v["listening"] == ["127.0.0.1:8766"] and v["problems"] == []
    # a broken file, and what an older layout left, are problems to show
    (config / "watches.json").write_text("{nope")
    (config / "department.json").write_text(json.dumps({"title": "Ghost"}))
    v = client.get("/watches").json()
    assert v["watches"] == [] and len(v["problems"]) == 2
    assert any("department.json" in p and "watches.json" in p for p in v["problems"])


def test_the_department_routes_are_gone(world):
    client, *_ = world
    assert client.get("/departments").status_code == 404
    assert client.get("/departments/ghost").status_code == 404
    assert client.get("/departments/ghost/journal").status_code == 404
    assert client.post("/departments", json={"name": "Sales"}).status_code == 404
    assert client.post("/departments/ghost/rules", json={"text": "x"}).status_code == 404
    status = client.get("/status").json()
    assert "department" not in status
    assert all("department" not in i for i in client.get("/waiting").json()["items"])
