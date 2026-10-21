"""Skills (ADR 0023): folders the model reads on demand, added and removed through the gate."""

from __future__ import annotations

from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy, Store, WriteGate
from ghost_core.skills import SOURCE, Skills, SkillSnapshot, discover, load_skill


def make_skill(
    folder: Path, name: str, description: str, body: str = "Do the thing.", **extra
) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    front = f"---\nname: {name}\ndescription: >-\n  {description}\n"
    for k, v in extra.items():
        front += f"{k}: {v}\n"
    (folder / "SKILL.md").write_text(front + "---\n\n# " + name + "\n\n" + body + "\n")
    return folder


def test_discovery_prefers_ghosts_own_and_reads_project_roots(tmp_path: Path):
    ghost = tmp_path / "ghost-skills"
    make_skill(
        ghost / "invoicing", "invoicing", "Send and chase invoices.", requires="[mcp:quickbooks]"
    )
    root = tmp_path / "project"
    make_skill(root / ".agents" / "skills" / "triage", "triage", "Triage issues.")
    make_skill(root / ".agents" / "skills" / "invoicing", "invoicing", "A shadowed copy.")
    (ghost / "not-a-skill").mkdir()
    found = discover(ghost, [root])
    assert set(found) == {"invoicing", "triage"}
    assert (
        found["invoicing"].source == "ghost"
        and found["invoicing"].description == "Send and chase invoices."
    )
    assert found["invoicing"].requires == ("mcp:quickbooks",)
    assert found["invoicing"].missing(["github"]) == ["quickbooks"]
    assert found["invoicing"].missing(["quickbooks"]) == []
    assert found["triage"].source == str(root)


def test_bad_names_and_missing_front_matter(tmp_path: Path):
    folder = tmp_path / "Weird Name"
    folder.mkdir()
    (folder / "SKILL.md").write_text("no front matter at all\n")
    assert load_skill(folder, "ghost") is None  # "weird name" is not a slug
    plain = tmp_path / "plain"
    plain.mkdir()
    (plain / "SKILL.md").write_text("Just instructions.\n")
    s = load_skill(plain, "ghost")
    assert s is not None and s.name == "plain" and s.body() == "Just instructions."


async def test_index_and_read_tool(tmp_path: Path):
    ghost = tmp_path / "skills"
    make_skill(
        ghost / "month-close",
        "month-close",
        "Close the books each month.",
        body="Step one. Step two.",
    )
    (ghost / "month-close" / "checklist.md").write_text("- reconcile\n- post\n")
    (ghost / "month-close" / "secret.bin").write_bytes(b"\xff\xfe\x00")
    skills = Skills(ghost)
    prompt = skills.index_prompt()
    assert "month-close: Close the books each month." in prompt
    assert "never permission" in prompt
    src = skills.source()
    assert src.name == SOURCE and [t.name for t in src.read_tools] == ["read_skill"]
    read = src.read_tools[0]
    out = await read.handler({"name": "Month-Close"})
    assert out["body"].startswith("# month-close") and "Step two." in out["body"]
    assert out["files"] == ["checklist.md", "secret.bin"]
    assert (await read.handler({"name": "month-close", "file": "checklist.md"}))[
        "text"
    ] == "- reconcile\n- post\n"
    assert (
        "not a text file"
        in (await read.handler({"name": "month-close", "file": "secret.bin"}))["error"]
    )
    assert (
        "not a file"
        in (await read.handler({"name": "month-close", "file": "../../etc/passwd"}))["error"]
    )
    assert (await read.handler({"name": "nope"}))["skills"] == ["month-close"]
    # narrowed to a toolkit's skills, the index and the tool shrink with it
    assert skills.index_prompt(["other"]) == ""
    assert (
        "no skill" in (await skills.read_tool(["other"]).handler({"name": "month-close"}))["error"]
    )


async def test_add_and_remove_go_through_the_gate(tmp_path: Path):
    ghost = tmp_path / "skills"
    skills = Skills(ghost)
    assert skills.source().read_tools == []  # nothing to read yet
    candidate = make_skill(tmp_path / "downloads" / "invoicing", "invoicing", "Send invoices.")
    (candidate / ".git").mkdir()
    (candidate / ".git" / "HEAD").write_text("ref")
    gate = WriteGate(
        Store(), autonomy=AutonomyPolicy.parse(f"[{SOURCE}]\nadd = 'quiet'\nremove = 'confirm'\n")
    )
    gate.register(SOURCE, skills.operations())
    add = next(o for o in skills.operations() if o.name == "add")
    prepared = await add.prepare({"path": str(candidate / "SKILL.md")})
    assert prepared.target.spoken() == "the invoicing skill"
    assert "invoicing" in prepared.target.mentions()
    sub = await gate.submit(
        session_id="s",
        ability=SOURCE,
        operation="add",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert (
        sub.autonomy is Autonomy.QUIET
        and sub.outcome is not None
        and sub.outcome.status == "success"
    )
    assert (ghost / "invoicing" / "SKILL.md").exists() and not (
        ghost / "invoicing" / ".git"
    ).exists()
    assert isinstance(sub.outcome.readback, SkillSnapshot) and sub.outcome.readback.present
    skills.refresh()
    assert [s.name for s in skills.list()] == ["invoicing"]
    # again without replace: the writer refuses
    with pytest.raises(ValueError, match="already in Ghost"):
        await add.prepare({"path": str(ghost / "invoicing")})
    sub2 = await gate.submit(
        session_id="s",
        ability=SOURCE,
        operation="add",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert (
        sub2.outcome is not None
        and sub2.outcome.status == "failed"
        and "already installed" in (sub2.outcome.error or "")
    )
    # remove waits for a yes; the request "remove the invoicing skill" is one (ADR 0017)
    remove = next(o for o in skills.operations() if o.name == "remove")
    prepared = await remove.prepare({"name": "invoicing"})
    sub3 = await gate.submit(
        session_id="s",
        ability=SOURCE,
        operation="remove",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert sub3.autonomy is Autonomy.CONFIRM and sub3.outcome is None
    out = await gate.confirm_by_request("s", "ghost, remove the invoicing skill")
    assert out is not None and out.status == "success" and not (ghost / "invoicing").exists()


async def test_only_ghosts_own_skills_can_be_removed(tmp_path: Path):
    root = tmp_path / "project"
    make_skill(root / ".agents" / "skills" / "triage", "triage", "Triage issues.")
    skills = Skills(tmp_path / "skills", [root])
    remove = next(o for o in skills.operations() if o.name == "remove")
    with pytest.raises(ValueError, match="only removes its own"):
        await remove.prepare({"name": "triage"})


async def test_add_rejects_what_is_not_a_skill(tmp_path: Path):
    skills = Skills(tmp_path / "skills")
    add = next(o for o in skills.operations() if o.name == "add")
    with pytest.raises(ValueError, match="path is required"):
        await add.prepare({})
    with pytest.raises(ValueError, match="not a folder"):
        await add.prepare({"path": str(tmp_path / "nowhere")})
    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(ValueError, match="no SKILL.md"):
        await add.prepare({"path": str(empty)})
    with pytest.raises(ValueError, match="not a skill name"):
        await add.prepare(
            {"path": str(make_skill(tmp_path / "ok", "ok", "Fine.")), "name": "Bad Name"}
        )
