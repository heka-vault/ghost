"""``ghost-scripts``: the one way a skill's scripts run (ADR 0023), as an app Ghost ships,
so a run is a change through the gate like any other (ADR 0034).

Connected in ``apps.json`` like any app that runs on this computer; ``run`` is a change, so
the operator's permissions decide whether it asks (``ask = ["scripts.run"]``)::

    {"apps": {"scripts": {"title": "Skill scripts", "command": "ghost-scripts",
                          "args": ["--skills", "~/.config/ghost/skills",
                                   "--root", "~/projects/x"]}}}

A script runs with a minimal environment (no secrets unless the app's ``env`` names
them), in its skill's folder, with a timeout and an output cap; the audit row carries the
arguments and the outcome. ``list_scripts`` is the read.
"""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

TIMEOUT_SECS = 60.0
OUTPUT_CAP = 20_000
INTERPRETERS = {".py": [sys.executable], ".sh": ["bash"], ".js": ["node"], ".ts": ["node"]}
PASS_ENV = ("PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR")


class ScriptRunner:
    def __init__(self, skills_dir: Path, roots: list[Path]) -> None:
        self.skills_dir = skills_dir
        self.roots = roots

    def folder(self, skill: str) -> Path:
        skill = skill.strip().lower()
        if not skill or "/" in skill or skill.startswith("."):
            raise ValueError(f"{skill!r} is not a skill name")
        folders = ("skills", ".agents/skills", ".claude/skills")  # a project's, under --root
        places = [self.skills_dir / skill] + [r / f / skill for r in self.roots for f in folders]
        for p in places:
            if (p / "SKILL.md").is_file():
                return p
        raise ValueError(f"no skill named {skill!r}")

    def scripts(self, skill: str) -> list[str]:
        folder = self.folder(skill)
        out = []
        for p in sorted(folder.rglob("*")):
            if not p.is_file() or p.is_symlink() or p.name == "SKILL.md":
                continue
            rel = p.relative_to(folder)
            if any(part.startswith(".") for part in rel.parts) or len(rel.parts) > 3:
                continue
            if p.suffix in INTERPRETERS or os.access(p, os.X_OK):
                out.append(str(rel))
        return out

    def run(self, skill: str, script: str, args: list[str] | None = None) -> dict[str, Any]:
        folder = self.folder(skill)
        target = (folder / script).resolve()
        if folder.resolve() not in target.parents or not target.is_file() or target.is_symlink():
            raise ValueError(f"{script!r} is not a script of the {skill} skill")
        if str(target.relative_to(folder.resolve())) not in self.scripts(skill):
            raise ValueError(
                f"{script!r} is not runnable (not an interpreter's file or executable)"
            )
        cmd = [*INTERPRETERS.get(target.suffix, []), str(target), *[str(a) for a in args or []]]
        env = {
            k: v for k, v in os.environ.items() if k in PASS_ENV or k.startswith("GHOST_SCRIPT_")
        }
        try:
            done = subprocess.run(  # noqa: S603 - the operator declared this run through the gate
                cmd, cwd=folder, env=env, capture_output=True, text=True, timeout=TIMEOUT_SECS
            )
        except subprocess.TimeoutExpired:
            raise ValueError(f"{script} ran past {TIMEOUT_SECS:.0f}s and was stopped") from None
        return {
            "skill": skill,
            "script": script,
            "command": shlex.join(cmd[len(INTERPRETERS.get(target.suffix, [])) :]),
            "exit": done.returncode,
            "stdout": done.stdout[-OUTPUT_CAP:],
            "stderr": done.stderr[-OUTPUT_CAP:],
        }


def build_server(skills_dir: Path, roots: list[Path]):
    from mcp.server.mcpserver import MCPServer
    from mcp.types import ToolAnnotations

    runner = ScriptRunner(skills_dir, roots)
    srv = MCPServer("ghost-scripts")

    @srv.tool(annotations=ToolAnnotations(readOnlyHint=True))
    def list_scripts(skill: str) -> dict:
        """List the scripts a skill carries (files an interpreter runs, or executables)."""
        return {"skill": skill, "scripts": runner.scripts(skill)}

    @srv.tool()
    def run(skill: str, script: str, args: list[str] | None = None) -> dict:
        """Run one of a skill's scripts with arguments; returns exit code and output."""
        return runner.run(skill, script, args)

    return srv


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a skill's scripts, as an MCP server over stdio."
    )
    parser.add_argument("--skills", type=Path, default=Path("~/.config/ghost/skills"))
    parser.add_argument(
        "--root", type=Path, action="append", default=[], help="a project root with .agents/skills"
    )
    a = parser.parse_args(argv)
    build_server(a.skills.expanduser(), [r.expanduser() for r in a.root]).run(transport="stdio")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
