"""The gated scripts server (ADR 0023, ADR 0034): a skill's scripts run only through it."""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest
from ghost_core.scripts_server import ScriptRunner


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
    # a project root's skills (--root) may sit in any of the three usual folders
    root = tmp_path / "repo"
    make_skill_with_script(root / ".claude" / "skills" / "deploy")
    assert ScriptRunner(skills, [root]).scripts("deploy") == ["count.py"]
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
