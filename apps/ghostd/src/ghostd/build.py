"""What this ghostd is: its version, and which kind of build (docs/releasing.md).

  dev      a checkout (``make dev``, ``uv run``): the commit, whether the tree had
           uncommitted changes when the daemon started, and the make target that started
           it when it says so (``GHOST_LAUNCHER``), so the desktop names the right restart
  deploy   a ``make deploy`` runtime: scripts/deploy.sh records the commit in the build's venv
  release  a .deb: packaging/build-debs.sh records it the same way

A recorded build is ``ghost-build.json`` in the venv's root (``sys.prefix``); a venv without
one is a checkout's. Read once, at start: two ``git`` calls for a checkout, nothing for the
others, and never on a turn.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from importlib import metadata
from pathlib import Path

from ghost_api.models import Build
from loguru import logger
from pydantic import ValidationError

BUILD_FILE = "ghost-build.json"
GIT_TIMEOUT_SECS = 2.0
# Set by scripts/dev.sh (make dev, make daemon) and make start; any other value is ignored.
LAUNCHERS = ("make-dev", "make-start")


def version() -> str:
    """The version ghostd was installed at: the root pyproject.toml's, through its own copy."""
    try:
        return metadata.version("ghostd")
    except metadata.PackageNotFoundError:
        return "unknown"


def detect(
    prefix: Path | None = None,
    source: Path | None = None,
    environ: dict[str, str] | None = None,
) -> Build:
    """The build recorded in the venv, else the checkout ``source`` (this package's own
    folder) is in, else a dev build with no commit. A checkout's says which make target
    started it, when ``GHOST_LAUNCHER`` names one."""
    recorded = _recorded((Path(sys.prefix) if prefix is None else prefix) / BUILD_FILE)
    if recorded is not None:
        return recorded
    found = _checkout(Path(__file__).resolve().parent if source is None else source)
    launcher = (os.environ if environ is None else environ).get("GHOST_LAUNCHER", "")
    if launcher in LAUNCHERS:
        found = found.model_copy(update={"launcher": launcher})
    return found


def _recorded(path: Path) -> Build | None:
    try:
        data = json.loads(path.read_text())
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as e:
        logger.warning(f"{path} is unreadable ({type(e).__name__}); reporting a dev build")
        return None
    try:
        return Build.model_validate(data)
    except ValidationError:
        logger.warning(f"{path} isn't a build record; reporting a dev build")
        return None


def _checkout(where: Path) -> Build:
    def git(*args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(where), *args],
            check=True,
            capture_output=True,
            text=True,
            timeout=GIT_TIMEOUT_SECS,
        ).stdout.strip()

    try:
        commit = git("rev-parse", "HEAD")
        # Tracked files only, as scripts/deploy.sh judges a tree.
        dirty = bool(git("status", "--porcelain", "--untracked-files=no"))
        return Build(kind="dev", commit=commit, dirty=dirty)
    except (OSError, subprocess.SubprocessError, ValidationError):
        return Build(kind="dev")


def describe(ver: str, build: Build) -> str:
    """One line: "Ghost 2026.10.0 · dev a1b2c3d", as the desktop's sidebar says it. A
    clean release names no commit; its version is the tag."""
    line = f"Ghost {ver}"
    if build.kind == "release" and not build.dirty:
        return line
    line += f" · {build.kind}"
    if build.commit:
        line += f" {build.commit[:7]}"
    if build.dirty:
        line += ", uncommitted changes"
    return line
