"""``/templates`` (ADR 0023): list, import, export."""

from __future__ import annotations

from pathlib import Path

from ghost_core import AutonomyPolicy
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.skills import SOURCE as SKILLS
from ghost_core.skills import Skills
from ghost_core.templates import Templates

SHIPPED = Path(__file__).parents[3] / "infra" / "templates"


def test_import_and_export_a_template(world, tmp_path: Path):
    client, state, control, gate, writer, loaded, deps = world
    deps.policy_path.write_text(
        '[mcp.github]\ncommand = "ghost-mock-github"\n[mcp.github.writes.comment]\n'
        'tool = "add_issue_comment"\ntarget = ["owner", "repo", "issue_number"]\n'
    )
    gate.set_autonomy(AutonomyPolicy.load(deps.policy_path))
    skills = Skills(tmp_path / "ghost-skills")
    gate.register(SKILLS, skills.operations())
    deps.skills = skills
    deps.templates = Templates(SHIPPED, tmp_path / "own-templates")
    listed = {t["name"]: t for t in client.get("/templates").json()}
    assert listed["triage"]["source"] == "shipped" and listed["triage"]["skills"] == ["triage"]
    assert listed["accounting"]["needs"] == ["server books is not declared in the policy file"]
    r = client.post("/templates/triage/apply", json={})
    assert r.status_code == 200, r.text
    dep = r.json()
    assert dep["name"] == "Triage" and dep["toolkits"] == ["triage"] and dep["is_department"]
    assert "triage desk" in dep["persona"]
    assert [s["name"] for s in client.get("/skills").json()] == ["triage"]
    assert "triage" in AutonomyPolicy.load(deps.policy_path).toolkits
    assert client.post("/templates/triage/apply", json={}).status_code == 409  # exists
    assert client.post("/templates/nope/apply", json={}).status_code == 404
    audit = client.get("/audit").json()["items"]
    assert {(e["ability"], e["operation"]) for e in audit} >= {
        (SKILLS, "add"),
        (CONVERSATIONS, "create"),
    }
    # export writes the operator's own template, which then shadows the shipped one
    r = client.post(f"/conversations/{dep['slug']}/export")
    assert r.status_code == 200, r.text
    assert r.json()["source"] == "ghost" and r.json()["toolkits"] == ["triage"]
    assert (tmp_path / "own-templates" / "triage" / "department.toml").exists()
    assert {t["name"]: t["source"] for t in client.get("/templates").json()}["triage"] == "ghost"
    assert client.post("/conversations/ghost/export").status_code == 404
