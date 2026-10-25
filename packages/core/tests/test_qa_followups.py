"""What the 2026-10-20 QA review found, pinned down."""

from __future__ import annotations

import os
from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.mcp import Servers, ServerSpec, ToolError, _brief, _coerce
from ghost_core.skills import SOURCE, Skills

PROBE = """
import json, os, sys
from mcp.server.mcpserver import MCPServer
srv = MCPServer("probe")
@srv.tool()
def env(name: str) -> str:
    "What the server sees."
    return os.environ.get(name, "<unset>")
srv.run(transport="stdio")
"""


async def test_stdio_child_gets_only_its_declared_environment(tmp_path: Path):
    script = tmp_path / "probe.py"
    script.write_text(PROBE)
    os.environ["GHOST_QA_UNRELATED_SECRET"] = "must-not-leak"
    try:
        spec = ServerSpec(
            name="probe",
            command=os.environ.get("VIRTUAL_ENV", str(Path(os.sys.executable).parent.parent))
            and os.sys.executable,
            args=(str(script),),
            env={"PROBE_OWN": "$GHOST_QA_OWN"},
            reads=("env",),
        )
        servers = Servers({"probe": spec}, environ={**os.environ, "GHOST_QA_OWN": "given"})
        await servers.connect()
        assert "probe" in servers.servers, servers.failed
        server = servers.servers["probe"]
        assert (await server.call("env", {"name": "PROBE_OWN"})) == {"result": "given"}
        assert (await server.call("env", {"name": "GHOST_QA_UNRELATED_SECRET"})) == {
            "result": "<unset>"
        }
        assert (await server.call("env", {"name": "PATH"}))["result"] != "<unset>"
        # kill the child: the next call marks the server gone instead of pretending
        proc = server._task  # noqa: SLF001
        assert proc is not None
        await servers.close()
        assert not server.connected
        with pytest.raises(ToolError, match="not connected"):
            await server.call("env", {"name": "PATH"})
    finally:
        os.environ.pop("GHOST_QA_UNRELATED_SECRET", None)


def test_server_names_are_slugs():
    from ghost_core.mcp import parse_servers

    with pytest.raises(ValueError, match="server name"):
        parse_servers({"github.heka": {"url": "http://x"}}, "test")


def test_department_levels_merge_over_the_plain_section():
    """A department.json's levels for a server sit over the file's own section for it
    (ADR 0024); the file's other operations stay."""
    from pathlib import Path as P

    from ghost_core.departments import Declaration
    from ghost_core.departments import Department as Dept
    from ghost_core.policy import Autonomy

    base = AutonomyPolicy.parse('[github]\ncomment = "quiet"\nclose = "announce"\n')
    d = Dept(
        slug="ghost",
        path=P("/nonexistent"),
        title="Ghost",
        declarations={"github": Declaration(levels={"close": Autonomy.CONFIRM})},
    )
    p = d.policy(base)
    assert {k: v.value for k, v in p.skills["github"].items()} == {
        "close": "confirm",
        "comment": "quiet",
    }
    again = AutonomyPolicy.parse(p.to_toml())
    assert again.skills == p.skills


def test_brief_keeps_abbreviations():
    assert (
        _brief("Search issues (e.g. by label). Returns a list.") == "Search issues (e.g. by label)."
    )
    assert _brief("One. Two.") == "One."


def test_coerce_by_schema_type():
    assert _coerce("42", {"type": "integer"}) == 42
    assert _coerce("42", {"anyOf": [{"type": "integer"}, {"type": "null"}]}) == 42
    assert _coerce("a, b", {"type": "array"}) == ["a", "b"]
    assert _coerce("true", {"type": "boolean"}) is True
    assert _coerce(7, {"type": "string"}) == "7"
    assert _coerce("x", {"type": "integer"}) == "x"  # left for the server to reject


def make_skill(folder: Path, name: str, description: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: {description}\n---\n\nSteps.\n"
    )
    return folder


async def test_symlinks_never_enter_a_skill(tmp_path: Path):
    secret = tmp_path / "env"
    secret.write_text("TOKEN=hunter2\n")
    candidate = make_skill(tmp_path / "dl" / "sneaky", "sneaky", "Reads your env.")
    (candidate / "notes.txt").symlink_to(secret)
    skills = Skills(tmp_path / "skills")
    add = next(o for o in skills.operations() if o.name == "add")
    with pytest.raises(ValueError, match="symlink"):
        await add.prepare({"path": str(candidate)})
    # a project skill read in place: a linked file is neither listed nor readable
    root = tmp_path / "project"
    folder = make_skill(root / ".agents" / "skills" / "linked", "linked", "Has a link.")
    (folder / "leak.txt").symlink_to(secret)
    (folder / "ok.txt").write_text("fine")
    skills = Skills(tmp_path / "skills", root / ".agents" / "skills")
    skill = skills.get("linked")
    assert skill is not None and skill.files() == ["ok.txt"]
    out = await skills.read_tool().handler({"name": "linked", "file": "leak.txt"})
    assert "not a file" in out["error"]


async def test_add_with_a_new_name_and_remove_agree(tmp_path: Path):
    candidate = make_skill(tmp_path / "dl" / "original", "original", "Something.")
    ghost = tmp_path / "skills"
    skills = Skills(ghost)
    gate = WriteGate(
        Store(), autonomy=AutonomyPolicy.parse(f"[{SOURCE}]\nadd = 'quiet'\nremove = 'quiet'\n")
    )
    gate.register(SOURCE, skills.operations())
    add = next(o for o in skills.operations() if o.name == "add")
    prepared = await add.prepare({"path": str(candidate), "name": "renamed"})
    await gate.submit(
        session_id="s",
        ability=SOURCE,
        operation="add",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    skills.refresh()
    assert [s.name for s in skills.list()] == [
        "renamed"
    ]  # the folder names it, not the front matter
    remove = next(o for o in skills.operations() if o.name == "remove")
    prepared = await remove.prepare({"name": "renamed"})
    out = await gate.submit(
        session_id="s",
        ability=SOURCE,
        operation="remove",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert (
        out.outcome is not None
        and out.outcome.status == "success"
        and not (ghost / "renamed").exists()
    )
    with pytest.raises(LookupError):
        await remove.prepare({"name": "renamed"})
