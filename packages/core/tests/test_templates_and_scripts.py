"""Department templates and the gated scripts server (ADR 0023)."""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest
from ghost_core import Store
from ghost_core.conversations import Conversations
from ghost_core.scripts_server import ScriptRunner
from ghost_core.templates import Templates, discover, export_department, load_template

SHIPPED = Path(__file__).parents[3] / "infra" / "templates"


def test_shipped_templates_load_and_say_what_they_need():
    found = discover(SHIPPED, None)
    assert {"triage", "design-research", "accounting"} <= set(found)
    triage = found["triage"]
    assert triage.toolkits == ("triage",) and "triage" in triage.toolkit_specs
    assert [p.name for p in triage.skills] == ["triage"] and triage.servers == ("github",)
    assert triage.needs(["triage"], ["github"]) == []
    assert triage.needs([], ["github"]) == []  # the template carries the toolkit itself
    assert found["accounting"].needs([], []) == ["server books is not declared in the policy file"]
    assert "books" in found["accounting"].persona


def test_export_round_trips(tmp_path: Path):
    own = tmp_path / "templates"
    specs = discover(SHIPPED, None)["triage"].toolkit_specs
    folder = export_department(
        own,
        name="issues",
        title="Issues",
        description="Mine.",
        aliases=("tickets",),
        voice="af_heart",
        persona="You sort tickets.",
        toolkits=("triage",),
        toolkit_specs=specs,
        skill_folders=[SHIPPED / "triage" / "skills" / "triage"],
    )
    t = load_template(folder, "ghost")
    assert t is not None and t.title == "Issues" and t.aliases == ("tickets",)
    assert t.voice == "af_heart" and t.persona == "You sort tickets."
    assert t.toolkits == ("triage",) and t.toolkit_specs["triage"].grants["github"].writes == (
        "comment",
        "set_labels",
        "close",
        "reopen",
    )
    assert [p.name for p in t.skills] == ["triage"]
    # the operator's own template shadows a shipped one with the same name
    reg = Templates(SHIPPED, own)
    assert reg.get("issues").source == "ghost" and reg.get("triage").source == "shipped"


async def test_create_a_department_from_a_template(tmp_path: Path):
    reg = Templates(SHIPPED, None)
    with_toolkit = Conversations(
        Store(),
        tmp_path / "c",
        default_voice="af_heart",
        toolkits=lambda: ["triage"],
        templates=reg,
    )
    create = next(o for o in with_toolkit.operations() if o.name == "create")
    prepared = await create.prepare({"name": "", "template": "triage"})
    step = prepared.changes.steps[0]
    assert step.name == "Triage" and step.toolkits == ("triage",) and "triage desk" in step.persona
    assert step.aliases == ("issues", "the triage desk")
    prepared = await create.prepare({"name": "Tickets", "template": "triage"})
    assert prepared.changes.steps[0].name == "Tickets"
    # without the toolkit in the policy file, the voice path says what to do
    without = Conversations(Store(), tmp_path / "d", default_voice="af_heart", templates=reg)
    create = next(o for o in without.operations() if o.name == "create")
    with pytest.raises(ValueError, match="import the Triage template in the desktop"):
        await create.prepare({"template": "triage"})
    with pytest.raises(ValueError, match="no template called nope"):
        await create.prepare({"template": "nope"})
    listed = await next(t for t in without.read_tools() if t.name == "list_templates").handler({})
    assert {t["name"] for t in listed["templates"]} >= {"triage", "accounting"}


def make_skill_with_script(folder: Path) -> Path:
    folder.mkdir(parents=True)
    (folder / "SKILL.md").write_text("---\nname: counter\ndescription: Counts.\n---\n")
    (folder / "count.py").write_text(
        "import sys, os\nprint('args', sys.argv[1:])\n"
        "print('secret', os.environ.get('GHOST_QA_SECRET', '<unset>'))\n"
    )
    (folder / "notes.txt").write_text("not a script")
    return folder


def test_scripts_run_in_their_folder_with_a_minimal_environment(tmp_path: Path):
    skills = tmp_path / "skills"
    make_skill_with_script(skills / "counter")
    (skills / "counter" / "link.py").symlink_to(tmp_path / "elsewhere.py")
    runner = ScriptRunner(skills, [])
    assert runner.scripts("counter") == ["count.py"]
    os.environ["GHOST_QA_SECRET"] = "leak?"
    try:
        out = runner.run("counter", "count.py", ["a", "b"])
    finally:
        os.environ.pop("GHOST_QA_SECRET", None)
    assert out["exit"] == 0 and "args ['a', 'b']" in out["stdout"]
    assert "secret <unset>" in out["stdout"]  # no secret unless the server's env names it
    with pytest.raises(ValueError, match="not runnable"):
        runner.run("counter", "notes.txt")
    with pytest.raises(ValueError, match="not a script"):
        runner.run("counter", "../../etc/passwd")
    with pytest.raises(ValueError, match="not a script"):
        runner.run("counter", "link.py")
    with pytest.raises(ValueError, match="no skill"):
        runner.run("nope", "count.py")
    assert sys.executable  # the interpreter the runner uses for .py


def test_scripts_server_declares_the_shape_the_policy_expects():
    from ghost_core.scripts_server import build_server

    srv = build_server(Path("/nonexistent"), [])
    tools = {t.name: t for t in srv._tool_manager.list_tools()}  # noqa: SLF001
    assert set(tools) == {"list_scripts", "run"}
    assert tools["list_scripts"].annotations.read_only_hint and tools["run"].annotations is None
    assert set(tools["run"].parameters["properties"]) == {"skill", "script", "args"}
