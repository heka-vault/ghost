"""What a department reaches, after the ADR 0024 review: writes stay in the department's
own regular files, a session reaches only its own department's files, and agent folders
keep their rows."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest
from ghost_core import Store
from ghost_core.conversations import Conversations
from ghost_core.department_tools import (
    AddRule,
    DepartmentTarget,
    DepartmentTools,
    DepartmentWriter,
)
from ghost_core.departments import Departments


def write(path: Path, data: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))


async def test_rules_and_memory_are_never_written_through_a_link(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    shared = repo / "CLAUDE.md"
    shared.write_text("# Project rules\n\n- the operator's line\n")
    cfg = tmp_path / "ghost"
    m = cfg / "departments" / "marketing"
    m.mkdir(parents=True)
    write(m / "department.json", {"title": "Marketing"})
    (m / "AGENTS.md").symlink_to(shared)
    deps = Departments(cfg)
    assert any("a link" in p for p in deps.get("marketing").problems)
    w = DepartmentWriter(deps)
    with pytest.raises(ValueError, match="a link"):
        await w.apply(DepartmentTarget(slug="marketing"), AddRule(text="Always use UK dates."), "k")
    assert shared.read_text() == "# Project rules\n\n- the operator's line\n"


async def test_a_department_s_session_reaches_only_its_own_files(tmp_path):
    cfg = tmp_path / "ghost"
    for slug in ("marketing", "accounting"):
        write(cfg / "departments" / slug / "department.json", {"title": slug.title()})
    deps = Departments(cfg)
    store = Store()
    convs = Conversations(store, deps)
    tools = DepartmentTools(deps, store, convs)
    remember = next(o for o in tools.operations() if o.name == "remember")
    with pytest.raises(ValueError, match="only Ghost reaches"):
        await remember.prepare({"department": "accounting", "text": "x", "_active": "marketing"})
    with pytest.raises(ValueError, match="only Ghost reaches"):
        await remember.prepare({"department": "ghost", "text": "x", "_active": "marketing"})
    own = await remember.prepare({"department": "marketing", "text": "x", "_active": "marketing"})
    assert own.target.scope() == "marketing"
    from_ghost = await remember.prepare(
        {"department": "accounting", "text": "x", "_active": "ghost"}
    )
    assert from_ghost.target.scope() == "accounting"
    read_doc = next(t for t in tools.read_tools() if t.name == "read_department_doc")
    with pytest.raises(ValueError, match="only Ghost reaches"):
        await read_doc.handler({"department": "accounting", "_active": "marketing"})
    assert any(o.name == "add_rule" and not o.asked_by for o in tools.operations())


def test_agent_folders_keep_their_rows_when_copied_or_edited(tmp_path):
    cfg = tmp_path / "ghost"
    m = cfg / "departments" / "marketing"
    write(m / "department.json", {"title": "Marketing"})
    write(m / "agents" / "brand" / "agent.json", {"name": "Brand"})
    (m / "agents" / "Loud").mkdir()
    write(m / "agents" / "Loud" / "agent.json", {"name": "Loud"})
    db = tmp_path / "g.db"
    convs = Conversations(Store(db), Departments(cfg))
    brand = convs.get("brand")
    assert brand is not None and convs.get("loud") is None  # not lower-case: not read
    convs.record("brand", "s", "user", "hello brand")
    shutil.copytree(m / "agents" / "brand", m / "agents" / "brand2")  # cp -r
    convs.sync()
    assert convs.get("brand").id == brand.id and len(convs.turns("brand")) == 1
    copy = convs.get("brand2")
    assert copy is not None and copy.id != brand.id and convs.turns("brand2") == []
    data = json.loads((m / "agents" / "brand" / "agent.json").read_text())
    data["id"] = "a-different-id"
    write(m / "agents" / "brand" / "agent.json", data)
    again = Conversations(Store(db), Departments(cfg))  # a restart with the edited file
    assert again.get("brand").id == brand.id
    assert json.loads((m / "agents" / "brand" / "agent.json").read_text())["id"] == brand.id


def test_department_folders_and_roots(tmp_path):
    cfg = tmp_path / "ghost"
    (cfg / "departments" / "Marketing").mkdir(parents=True)
    write(cfg / "departments" / "Marketing" / "department.json", {"title": "Marketing"})
    (cfg / "departments" / "app" / "checkout").mkdir(parents=True)
    (cfg / "departments" / "app" / "checkout" / "AGENTS.md").write_text("# App\n")
    write(cfg / "departments" / "app" / "department.json", {"root": "checkout"})
    deps = Departments(cfg)
    assert deps.get("marketing") is None  # a folder name is the slug: lower-case
    app = deps.get("app")
    assert app is not None and app.root == (cfg / "departments" / "app" / "checkout").resolve()
    assert app.rules().startswith("# App")


def test_a_folder_made_again_under_an_old_slug_takes_its_row_back(tmp_path):
    cfg = tmp_path / "ghost"
    write(cfg / "departments" / "marketing" / "department.json", {"title": "Marketing"})
    write(cfg / "departments" / "marketing" / "agents" / "brand" / "agent.json", {"name": "Brand"})
    db = tmp_path / "g.db"
    convs = Conversations(Store(db), Departments(cfg))
    ids = {s: convs.get(s).id for s in ("marketing", "brand")}
    shutil.rmtree(cfg / "departments" / "marketing")
    convs = Conversations(Store(db), Departments(cfg))  # restart: rows marked deleted
    assert convs.get("marketing") is None and convs.get("brand") is None
    write(cfg / "departments" / "marketing" / "department.json", {"title": "Marketing"})
    write(cfg / "departments" / "marketing" / "agents" / "brand" / "agent.json", {"name": "Brand"})
    convs = Conversations(Store(db), Departments(cfg))  # restart again: no collision
    assert convs.get("marketing").id == ids["marketing"]
    assert convs.get("brand").id == ids["brand"]
