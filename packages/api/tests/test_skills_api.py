"""``/skills`` (ADR 0023): list, read, add and remove through the gate."""

from __future__ import annotations

from pathlib import Path

from ghost_core.skills import SOURCE, Skills


def make_skill(folder: Path, name: str, description: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: {description}\n---\n\nSteps.\n"
    )
    return folder


def test_skills_routes(world, tmp_path: Path):
    client, state, control, gate, writer, loaded, deps = world
    skills = Skills(tmp_path / "ghost-skills")
    gate.register(SOURCE, skills.operations())
    deps.skills = skills
    assert client.get("/skills").json() == []
    candidate = make_skill(tmp_path / "dl" / "invoicing", "invoicing", "Send invoices.")
    r = client.post("/skills", json={"path": str(candidate)})
    assert r.status_code == 200, r.text
    assert r.json()["name"] == "invoicing" and r.json()["source"] == "ghost"
    assert [s["name"] for s in client.get("/skills").json()] == ["invoicing"]
    body = client.get("/skills/invoicing").json()
    assert body["body"] == "Steps." and body["files"] == []
    assert client.get("/skills/nope").status_code == 404
    # the add is audited like any write from the desktop
    audit = client.get("/audit").json()
    entries = audit if isinstance(audit, list) else audit.get("items", [])
    assert any(e["source"] == "skills" and e["operation"] == "add" for e in entries)
    again = client.post("/skills", json={"path": str(candidate)})
    assert again.status_code == 409
    assert client.delete("/skills/invoicing").json()["ok"] is True
    assert client.get("/skills").json() == []
    assert client.post("/skills", json={"path": str(tmp_path / "nowhere")}).status_code == 409
