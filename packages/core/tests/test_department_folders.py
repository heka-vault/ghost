"""Departments as folders (ADR 0024): discovery, department.json, a root read in place, and
the permissions a department adds over the operator's (ADR 0034)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy
from ghost_core.departments import (
    DEPARTMENT_FILE,
    GHOST,
    RULES_FILE,
    DepartmentError,
    Departments,
    discover,
    load_department,
)

MARKETING = {
    "title": "Marketing",
    "aliases": ["the marketers"],
    "voice": "bf_emma",
    "permissions": {
        "block": ["github.*(repo!=web)"],
        "ask": ["github.add_issue_comment"],
        "quiet": ["figma.export"],
    },
}


def write(folder: Path, name: str, data: object) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / name).write_text(json.dumps(data) if not isinstance(data, str) else data)


@pytest.fixture
def config(tmp_path: Path) -> Path:
    cfg = tmp_path / "ghost"
    write(cfg, DEPARTMENT_FILE, {"title": "Ghost"})
    write(cfg, RULES_FILE, "# Ghost\n\nBe brief.\n")
    marketing = cfg / "departments" / "marketing"
    write(marketing, DEPARTMENT_FILE, MARKETING)
    write(marketing, RULES_FILE, "# Marketing: the brand and the channels\n\nUK dates.\n")
    (marketing / "docs").mkdir()
    (marketing / "docs" / "voice.md").write_text("Warm, never salesy.\n")
    (marketing / "skills" / "launch").mkdir(parents=True)
    (marketing / "skills" / "launch" / "SKILL.md").write_text("---\nname: launch\n---\nGo.\n")
    (cfg / "departments" / "Bad Name").mkdir()
    return cfg


def test_discovery_reads_ghost_and_each_folder(config: Path):
    found = discover(config)
    assert set(found) == {GHOST, "marketing"}  # "Bad Name" is skipped
    ghost = found[GHOST]
    assert ghost.is_ghost and ghost.title == "Ghost" and ghost.path == config
    assert ghost.rules_lead() == "Ghost" and ghost.problems == []
    m = found["marketing"]
    assert m.title == "Marketing" and m.aliases == ("the marketers",) and m.voice == "bf_emma"
    assert m.rules_lead() == "Marketing: the brand and the channels"
    assert "UK dates" in m.rules()
    assert m.docs() == ["voice.md"] and m.read_doc("voice.md").startswith("Warm")
    with pytest.raises(DepartmentError):
        m.read_doc("../AGENTS.md")
    assert [p.name for p, _ in m.skill_places()] == ["skills"]
    assert m.agents_dir == config / "departments" / "marketing" / "agents"
    assert m.problems == []


def test_a_department_adds_care_over_the_operators_permissions(config: Path):
    base = AutonomyPolicy.parse(
        '[permissions]\nallow = ["github.*"]\n[conversations]\nrename = "quiet"\n'
    )
    found = discover(config)
    p = found["marketing"].policy(base)
    assert p.resolve("github", "issue_write", {"repo": "app"}) is Autonomy.DENY
    assert p.resolve("github", "issue_write", {"repo": "web"}) is Autonomy.ANNOUNCE
    assert p.resolve("github", "add_issue_comment", {"repo": "web"}) is Autonomy.CONFIRM
    assert p.resolve("figma", "export") is Autonomy.QUIET
    assert p.resolve("conversations", "rename") is Autonomy.QUIET  # the file's own stay
    g = found[GHOST].policy(base)
    assert g.resolve("github", "issue_write", {"repo": "app"}) is Autonomy.ANNOUNCE
    assert base.resolve("figma", "export") is Autonomy.ANNOUNCE  # untouched


def test_a_department_cannot_allow_and_ghost_keeps_its_permissions_in_the_policy(tmp_path):
    folder = tmp_path / "departments" / "ops"
    write(folder, DEPARTMENT_FILE, {"permissions": {"allow": ["github"]}})
    d = load_department("ops", folder)
    assert any("only block, ask or quiet" in p for p in d.problems)
    assert d.permissions.allow == ()
    ghost = tmp_path / "g"
    write(ghost, DEPARTMENT_FILE, {"permissions": {"block": ["gmail"]}})
    g = load_department(GHOST, ghost, is_ghost=True)
    assert any("policy.toml" in p for p in g.problems) and g.permissions.block == ()


def test_department_json_problems_do_not_lose_the_department(tmp_path: Path):
    folder = tmp_path / "departments" / "ops"
    write(folder, DEPARTMENT_FILE, "{not json")
    d = load_department("ops", folder)
    assert d.title == "Ops" and any("Expecting" in p for p in d.problems)
    write(
        folder,
        DEPARTMENT_FILE,
        {
            "title": "Ops",
            "root": str(tmp_path / "nowhere"),
            "bogus": 1,
            "servers": {"books": {"levels": {"post": "quiet"}}},
            "permissions": {"block": ["not a rule!"]},
        },
    )
    write(folder, "mcp.json", {"mcpServers": {"books": {"command": "books-mcp"}}})
    d = load_department("ops", folder)
    text = "\n".join(d.problems)
    assert "root" in text and "is not a folder" in text and d.root is None
    assert "unknown key(s) bogus" in text
    assert "servers is no longer read" in text  # ADR 0034: apps are Ghost's
    assert "connect its apps in Ghost's Apps screen" in text
    assert "is not a rule" in text


def test_a_root_is_read_in_place_and_agents_stay_home(tmp_path: Path):
    repo = tmp_path / "repo"
    (repo / ".agents" / "skills" / "deploy").mkdir(parents=True)
    (repo / ".claude" / "skills" / "review").mkdir(parents=True)
    (repo / "docs").mkdir()
    (repo / "docs" / "runbook.md").write_text("Steps.\n")
    (repo / RULES_FILE).write_text("# Heka\n")
    write(repo, ".mcp.json", {"mcpServers": {"gh": {"command": "github-mcp-server"}}})
    folder = tmp_path / "ghost" / "departments" / "heka"
    write(folder, DEPARTMENT_FILE, {"root": str(repo)})
    d = load_department("heka", folder)
    assert d.root == repo and d.rules_lead() == "Heka" and d.docs() == ["runbook.md"]
    assert d.problems == []  # the repository's own .mcp.json is its coding tools' business
    assert [str(p.relative_to(repo)) for p, _ in d.skill_places()] == [
        ".agents/skills",
        ".claude/skills",
    ]
    assert d.agents_dir == folder / "agents"  # never in the repository
    # Ghost keeps its resources at the config root; its old mcp.json is named
    ghost_dir = tmp_path / "g"
    write(ghost_dir, DEPARTMENT_FILE, {"root": str(repo)})
    write(ghost_dir, "mcp.json", {"mcpServers": {}})
    g = load_department(GHOST, ghost_dir, is_ghost=True)
    assert g.root is None and len(g.problems) == 2
    assert any("rename it" in p and "apps.json" in p for p in g.problems)


def test_registry_resolves_lists_and_creates(config: Path):
    reg = Departments(config)
    assert [d.slug for d in reg.list()] == [GHOST, "marketing"]
    assert reg.resolve("the marketers").slug == "marketing"
    assert reg.resolve("move this to marketing").slug == "marketing"
    assert reg.resolve("ghost").is_ghost and reg.resolve("legal") is None
    assert reg.problems() == []
    d = reg.create("Design Research", aliases=["research"], voice="am_adam")
    assert d.slug == "design-research" and d.title == "Design Research" and d.voice == "am_adam"
    assert (d.path / RULES_FILE).read_text().startswith("# Design Research")
    assert (d.path / "agents").is_dir() and not (d.path / "mcp.json").exists()
    assert reg.resolve("research").slug == "design-research"
    with pytest.raises(DepartmentError, match="already"):
        reg.create("marketing")
    with pytest.raises(DepartmentError, match="already"):
        reg.create("Ghost")
    reg.ensure_layout()
    assert (config / "agents").is_dir()
