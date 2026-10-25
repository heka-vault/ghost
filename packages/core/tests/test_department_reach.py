"""What a department reaches, after the ADR 0024 review: a used server's operations are
the department's own in the gate, levels follow the folder, the allowlist never widens,
writes stay in the department's own regular files, and the fleet retries a start that
brought nothing up."""

from __future__ import annotations

import asyncio
import json
import shutil
from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy, PolicyError, Store, WriteGate
from ghost_core.conversations import Conversations
from ghost_core.department_tools import (
    AddRule,
    DepartmentTarget,
    DepartmentTools,
    DepartmentWriter,
)
from ghost_core.departments import Departments, Fleet
from ghost_core.mcp import Servers
from test_mcp_writes import connected

CLOSE = {
    "tool": "issue_write",
    "with": {"method": "update", "state": "closed"},
    "target": ["owner", "repo", "issue_number"],
    "read": {"tool": "issue_read", "with": {"method": "get"}},
    "asked_by": ["close"],
}
SCOPE = {"kind": "repository", "arguments": ["owner", "repo"]}


def write(path: Path, data: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))


def layout(tmp_path: Path, *, marketing_items=("acme/web",), marketing_close=CLOSE) -> Path:
    cfg = tmp_path / "ghost"
    write(cfg / "mcp.json", {"mcpServers": {"github": {"command": "x"}}})
    write(
        cfg / "department.json",
        {
            "servers": {
                "github": {
                    "scope": {**SCOPE, "items": ["acme/app", "acme/web"]},
                    "writes": {"close": CLOSE},
                    "levels": {"close": "quiet"},
                }
            }
        },
    )
    m = cfg / "departments" / "marketing"
    write(m / "mcp.json", {"mcpServers": {}, "use": ["github"]})
    write(
        m / "department.json",
        {
            "title": "Marketing",
            "servers": {
                "github": {
                    "scope": {**SCOPE, "items": list(marketing_items)},
                    "writes": {"close": marketing_close},
                    "levels": {"close": "quiet"},
                }
            },
        },
    )
    return cfg


async def fleet_over(deps: Departments, server) -> Fleet:
    fleet = Fleet(deps)
    fleet.ghost = Servers({})
    fleet.ghost.servers = {"github": server}
    return fleet


async def test_a_used_server_keeps_each_department_s_own_operation_in_the_gate(tmp_path):
    """Ghost allows two repositories; Marketing uses the server with one of them and a
    pinned title on close. After a transfer registers Marketing's view, a Ghost session
    still writes with Ghost's declaration, and Marketing with its own."""
    pinned = {**CLOSE, "with": {**CLOSE["with"], "title": "MARKETING-PINNED"}}
    deps = Departments(layout(tmp_path, marketing_close=pinned))
    async with connected(deps.ghost.servers["github"]) as (server, world):
        fleet = await fleet_over(deps, server)
        gate = WriteGate(Store(), autonomy=AutonomyPolicy())
        gate.attach_departments(deps)
        for src in fleet.sources_for(deps.ghost):  # daemon start
            gate.register(src.name, src.operations)
        for src in fleet.sources_for(deps.get("marketing")):  # a transfer in
            gate.register(src.name, src.operations, department="marketing")

        ghost_close = gate.operation("github", "close", "ghost")
        marketing_close = gate.operation("github", "close", "marketing")
        assert ghost_close is not marketing_close

        prepared = await ghost_close.prepare({"owner": "acme", "repo": "app", "issue_number": 42})
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="close",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
            department="ghost",
        )
        assert sub.outcome is not None and sub.outcome.status == "success"
        assert world.issues[("acme", "app", 42)]["state"] == "closed"
        assert world.issues[("acme", "app", 42)].get("title") != "MARKETING-PINNED"

        prepared = await marketing_close.prepare(
            {"owner": "acme", "repo": "web", "issue_number": 7}
        )
        sub = await gate.submit(
            session_id="s2",
            ability="github",
            operation="close",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
            department="marketing",
        )
        assert sub.outcome is not None and sub.outcome.status == "success"
        assert world.issues[("acme", "web", 7)]["title"] == "MARKETING-PINNED"

        # Marketing's view does not allow acme/app, whatever Ghost's says
        prepared = await ghost_close.prepare({"owner": "acme", "repo": "app", "issue_number": 42})
        with pytest.raises(PolicyError):
            await gate.submit(
                session_id="s3",
                ability="github",
                operation="close",
                target=prepared.target,
                changes=prepared.changes,
                rationale="",
                department="marketing",
            )
        assert gate.operations("github", "marketing")[0] is marketing_close


async def test_a_level_edited_in_the_folder_reaches_the_gate_at_the_next_refresh(tmp_path):
    cfg = layout(tmp_path)
    deps = Departments(cfg)
    gate = WriteGate(Store(), autonomy=AutonomyPolicy())
    gate.attach_departments(deps)
    async with connected(deps.ghost.servers["github"]) as (server, _):
        fleet = await fleet_over(deps, server)
        for src in fleet.sources_for(deps.ghost):
            gate.register(src.name, src.operations)
        for src in fleet.sources_for(deps.get("marketing")):
            gate.register(src.name, src.operations, department="marketing")
        assert gate.level("github", "close", department="marketing") is Autonomy.QUIET
        data = json.loads((cfg / "departments" / "marketing" / "department.json").read_text())
        data["servers"]["github"]["levels"]["close"] = "deny"
        write(cfg / "departments" / "marketing" / "department.json", data)
        deps.refresh()  # what GET /departments and reload_policy do
        assert gate.level("github", "close", department="marketing") is Autonomy.DENY
        # a new policy file is laid under the departments' levels again
        gate.set_autonomy(AutonomyPolicy.parse("[defaults]\nautonomy = 'confirm'\n"))
        assert gate.level("github", "close", department="marketing") is Autonomy.DENY
        assert gate.level("github", "close", department="ghost") is Autonomy.QUIET


def test_a_use_never_widens_ghost_s_allowlist(tmp_path):
    cfg = layout(tmp_path, marketing_items=("acme/web", "other/repo"))
    deps = Departments(cfg)
    m = deps.get("marketing")
    assert any("other/repo" in p and "outside Ghost's allowlist" in p for p in m.problems)
    spec = m.spec_for_use(deps.ghost.servers["github"])
    assert spec.scope is not None and spec.scope.items == ("acme/web",)
    # no scope of its own: Ghost's; no items of its own: Ghost's items
    data = json.loads((cfg / "departments" / "marketing" / "department.json").read_text())
    del data["servers"]["github"]["scope"]
    write(cfg / "departments" / "marketing" / "department.json", data)
    deps.refresh()
    spec = deps.get("marketing").spec_for_use(deps.ghost.servers["github"])
    assert spec.scope is not None and spec.scope.items == ("acme/app", "acme/web")
    data["servers"]["github"]["scope"] = dict(SCOPE)
    write(cfg / "departments" / "marketing" / "department.json", data)
    deps.refresh()
    spec = deps.get("marketing").spec_for_use(deps.ghost.servers["github"])
    assert spec.scope is not None and spec.scope.items == ("acme/app", "acme/web")
    # a use of a server Ghost does not start is a problem
    write(cfg / "departments" / "marketing" / "mcp.json", {"mcpServers": {}, "use": ["figma"]})
    deps.refresh()
    assert any("figma" in p and "does not start" in p for p in deps.get("marketing").problems)


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


async def test_a_start_that_brings_nothing_up_is_retried_and_reported(tmp_path):
    cfg = tmp_path / "ghost"
    write(cfg / "departments" / "marketing" / "department.json", {"title": "Marketing"})
    write(
        cfg / "departments" / "marketing" / "mcp.json",
        {"mcpServers": {"figma": {"command": "no-such-command-ghost-test"}}},
    )
    deps = Departments(cfg)
    fleet = Fleet(deps, idle_secs=0.05)
    assert await fleet.enter("marketing") is None
    assert not fleet.is_up("marketing")
    assert fleet.failures("marketing") and "figma" in fleet.failures("marketing")[0]
    assert await fleet.enter("marketing") is None  # tried again, not a cached failure
    # leaving then returning before the idle timer fires keeps the servers up
    fleet.own["marketing"] = Servers({})
    fleet.leave("marketing")
    await fleet.enter("marketing")
    await asyncio.sleep(0.1)
    assert fleet.is_up("marketing")
    fleet.leave("marketing")
    await asyncio.sleep(0.1)
    assert not fleet.is_up("marketing")
