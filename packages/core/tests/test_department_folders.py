"""Departments as folders (ADR 0024): discovery, mcp.json, department.json, the levels a
department's session runs under, and a server seen through another declaration."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy
from ghost_core.departments import (
    DEPARTMENT_FILE,
    GHOST,
    MCP_FILE,
    RULES_FILE,
    DepartmentError,
    Departments,
    discover,
    load_department,
    looks_like_secret,
    parse_mcp_json,
    render_mcp_json,
)
from ghost_core.mcp import ServerSpec, fill_secrets, secret_names
from test_mcp_writes import connected, spec

MCP = {
    "mcpServers": {
        "figma": {
            "command": "figma-mcp",
            "args": ["--stdio"],
            "env": {"FIGMA_TOKEN": "${FIGMA_TOKEN}"},
        },
        "mobbin": {
            "type": "http",
            "url": "http://proxy.lan:8318/mobbin/mcp",
            "headers": {"Authorization": "Bearer ${env:MCP_PROXY_KEY}"},
        },
    },
    "use": ["github"],
}
DECLARATION = {
    "title": "Marketing",
    "aliases": ["the marketers"],
    "voice": "bf_emma",
    "servers": {
        "github": {
            "reads": ["issue_read", "list_issues"],
            "scope": {"kind": "repository", "arguments": ["owner", "repo"], "items": ["acme/web"]},
            "writes": {
                "comment": {
                    "tool": "add_issue_comment",
                    "target": ["owner", "repo", "issue_number"],
                    "read": {"tool": "issue_read", "with": {"method": "get"}},
                    "asked_by": ["comment"],
                    "risk": "low",
                }
            },
            "levels": {"comment": "confirm"},
            "targets": {"Acme/Web": {"comment": "deny"}},
        },
        "figma": {"levels": {"export": "quiet"}},
    },
}


def write(folder: Path, name: str, data: object) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / name).write_text(json.dumps(data) if not isinstance(data, str) else data)


@pytest.fixture
def config(tmp_path: Path) -> Path:
    cfg = tmp_path / "ghost"
    write(cfg, MCP_FILE, {"mcpServers": {"github": {"command": "github-mcp-server"}}})
    write(
        cfg,
        DEPARTMENT_FILE,
        {"servers": {"github": {"levels": {"close": "announce"}}}},
    )
    write(cfg, RULES_FILE, "# Ghost\n\nBe brief.\n")
    marketing = cfg / "departments" / "marketing"
    write(marketing, MCP_FILE, MCP)
    write(marketing, DEPARTMENT_FILE, DECLARATION)
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
    assert set(ghost.servers) == {"github"} and ghost.uses == ()
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


def test_mcp_json_is_the_portable_shape(config: Path):
    m = discover(config)["marketing"]
    assert set(m.servers) == {"figma", "mobbin"} and m.uses == ("github",)
    figma, mobbin = m.servers["figma"], m.servers["mobbin"]
    assert figma.transport == "stdio" and figma.args == ("--stdio",)
    assert figma.env == {"FIGMA_TOKEN": "${FIGMA_TOKEN}"}
    assert mobbin.transport == "http" and mobbin.headers["Authorization"].endswith("MCP_PROXY_KEY}")
    # secrets in either spelling come from the environment only
    assert fill_secrets("Bearer ${env:K}", {"K": "x"}) == "Bearer x"
    assert fill_secrets("${K} $K", {"K": "x"}) == "x x"
    assert secret_names("a ${env:ONE} $TWO ${THREE}") == ["ONE", "TWO", "THREE"]
    with pytest.raises(KeyError):
        fill_secrets("${env:MISSING}", {})
    # the declaration shapes what the gate sees of each server
    assert set(figma.writes) == set() and m.declared("figma").levels == {"export": Autonomy.QUIET}
    gh = m.declared("github")
    assert gh.reads == ("issue_read", "list_issues") and gh.scope.items == ("acme/web",)
    assert set(gh.writes) == {"comment"} and gh.writes["comment"].asked_by == ("comment",)
    assert gh.targets == {"acme/web": {"comment": Autonomy.DENY}}
    assert m.covers("github", "ACME/web") and not m.covers("github", "acme/app")
    assert not m.covers("figma", "anything")  # no scope declared
    # round trip
    again = parse_mcp_json(render_mcp_json(m.servers, m.uses), "x")
    assert again.servers == m.servers and again.uses == ("github",)


def test_mcp_json_problems_name_the_entry_and_keep_the_rest():
    file = parse_mcp_json(
        json.dumps(
            {
                "mcpServers": {
                    "ok": {"command": "x"},
                    "both": {"command": "x", "url": "http://y"},
                    "sse": {"type": "sse", "url": "http://y"},
                    "Bad Name": {"command": "x"},
                    "leaky": {"command": "x", "env": {"TOKEN": "ghp_" + "a" * 36}},
                    "odd": {"command": "x", "disabled": True},
                },
                "use": ["ok", "github"],
                "extra": 1,
            }
        ),
        "mcp.json",
    )
    assert set(file.servers) == {"ok", "leaky", "odd"} and file.uses == ("github",)
    text = "\n".join(file.problems)
    assert "both needs exactly one of url or command" in text
    assert "transport 'sse'" in text and "Bad Name" in text
    assert "leaky: env TOKEN looks like a literal secret" in text
    assert "odd: key(s) disabled are not read" in text
    assert "ok is both a server here and a use" in text and "unknown key(s) extra" in text
    assert looks_like_secret("sk-abcdefghijklmnop") and looks_like_secret("Bearer " + "q" * 20)
    assert not looks_like_secret("${env:TOKEN}") and not looks_like_secret("http://x.y/mcp")
    with pytest.raises(DepartmentError, match="not JSON"):
        parse_mcp_json("{", "f")
    assert parse_mcp_json("", "f").servers == {}


def test_department_json_problems_do_not_lose_the_department(tmp_path: Path):
    folder = tmp_path / "departments" / "ops"
    write(folder, DEPARTMENT_FILE, "{not json")
    d = load_department("ops", folder)
    assert (
        d.title == "Ops"
        and d.servers == {}
        and any("not json" in p.lower() or "Expecting" in p for p in d.problems)
    )
    write(
        folder,
        DEPARTMENT_FILE,
        {
            "title": "Ops",
            "root": str(tmp_path / "nowhere"),
            "bogus": 1,
            "servers": {"books": {"levels": {"post": "loud"}}, "stray": {}},
        },
    )
    write(folder, MCP_FILE, {"mcpServers": {"books": {"command": "books-mcp"}}})
    d = load_department("ops", folder)
    text = "\n".join(d.problems)
    assert "root" in text and "is not a folder" in text and d.root is None
    assert "unknown key(s) bogus" in text
    assert "'loud' is not one of" in text
    assert "stray is declared but mcp.json neither starts nor uses it" in text
    assert set(d.servers) == {"books"}  # the broken declaration is left out, the server stays


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
    assert set(d.servers) == {"gh"} and d.problems == []
    assert [str(p.relative_to(repo)) for p, _ in d.skill_places()] == [
        ".agents/skills",
        ".claude/skills",
    ]
    assert d.agents_dir == folder / "agents"  # never in the repository
    # Ghost keeps its resources at the config root
    ghost_dir = tmp_path / "g"
    write(ghost_dir, DEPARTMENT_FILE, {"root": str(repo)})
    write(ghost_dir, MCP_FILE, {"mcpServers": {}, "use": ["gh"]})
    g = load_department(GHOST, ghost_dir, is_ghost=True)
    assert g.root is None and g.uses == () and len(g.problems) == 2


def test_levels_live_beside_the_declaration_and_name_the_source(config: Path):
    base = AutonomyPolicy.parse(
        '[defaults]\nautonomy = "confirm"\n[conversations]\nrename = "quiet"\n'
    )
    found = discover(config)
    m = found["marketing"]
    assert m.source_name("figma") == "marketing/figma"  # its own server
    assert m.source_name("github") == "github"  # one it uses keeps the bare name
    assert found[GHOST].source_name("github") == "github"
    p = m.policy(base)
    assert p.resolve("marketing/figma", "export") is Autonomy.QUIET
    assert p.resolve("github", "comment", "acme/web") is Autonomy.DENY  # the target rule
    assert p.resolve("github", "comment", "acme/other") is Autonomy.CONFIRM
    assert p.resolve("conversations", "rename") is Autonomy.QUIET  # the file's own stay
    assert p.resolve("github", "close", risk="low") is Autonomy.CONFIRM  # [defaults]
    g = found[GHOST].policy(base)
    assert g.resolve("github", "close") is Autonomy.ANNOUNCE  # Ghost's department.json
    assert g.resolve("github", "comment", "acme/web") is Autonomy.CONFIRM  # not marketing's
    assert base.resolve("marketing/figma", "export", risk="low") is Autonomy.CONFIRM  # untouched


def test_registry_resolves_lists_covers_and_creates(config: Path):
    reg = Departments(config)
    assert [d.slug for d in reg.list()] == [GHOST, "marketing"]
    assert reg.resolve("the marketers").slug == "marketing"
    assert reg.resolve("move this to marketing").slug == "marketing"
    assert reg.resolve("ghost").is_ghost and reg.resolve("legal") is None
    assert [d.slug for d in reg.covering("github", "acme/web")] == ["marketing"]
    assert reg.covering("github", "acme/app") == []
    assert reg.problems() == []
    d = reg.create("Design Research", aliases=["research"], voice="am_adam")
    assert d.slug == "design-research" and d.title == "Design Research" and d.voice == "am_adam"
    assert (d.path / RULES_FILE).read_text().startswith("# Design Research")
    assert (d.path / "agents").is_dir() and json.loads((d.path / MCP_FILE).read_text()) == {
        "mcpServers": {}
    }
    assert reg.resolve("research").slug == "design-research"
    with pytest.raises(DepartmentError, match="already"):
        reg.create("marketing")
    with pytest.raises(DepartmentError, match="already"):
        reg.create("Ghost")
    reg.ensure_layout()
    assert (config / "agents").is_dir()


async def test_a_view_shares_the_connection_but_not_the_declaration():
    """A department that uses one of Ghost's servers reaches it through its own reads,
    scope and writes, over the one process Ghost started."""
    from ghost_core.departments import parse_declaration

    async with connected(spec()) as (server, world):
        decl = parse_declaration(
            "github",
            {
                "reads": ["issue_read"],
                "scope": {
                    "kind": "repository",
                    "arguments": ["owner", "repo"],
                    "items": ["acme/web"],
                },
                "writes": {
                    "retitle": {
                        "tool": "issue_write",
                        "with": {"method": "update"},
                        "args": ["owner", "repo", "issue_number", "title"],
                        "target": ["owner", "repo", "issue_number"],
                    }
                },
            },
            "d.json",
        )
        view = server.view(decl.applied_to(server.spec))
        assert view.connected and view.tools is server.tools
        assert [t.name for t in view.offered()] == ["issue_read"]
        assert {t.name for t in server.offered()} == {"issue_read", "list_issues"}
        assert [o.name for o in view.operations()] == ["retitle"]
        assert "close" in {o.name for o in server.operations()}
        prepared = await view.operations()[0].prepare({"issue_number": 7, "title": "Typo!"})
        assert prepared.target.scope() == "acme/web"  # the view's one item fills itself in
        with pytest.raises(ValueError, match="not an allowed repository"):
            await view.operations()[0].prepare(
                {"owner": "acme", "repo": "app", "issue_number": 42, "title": "x"}
            )
        out = await view.call(
            "issue_read", {"method": "get", "owner": "acme", "repo": "web", "issue_number": 7}
        )
        assert out["title"] == "Typo"
        with pytest.raises(ValueError, match="cannot be called"):
            server.view(ServerSpec(name="other", command="x"))
