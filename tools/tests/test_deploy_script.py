"""scripts/deploy.sh's drop-in across the vault (ADR 0036): a rollback writes the form the
build it goes back to needs, refuses a pre-vault build whose ~/.config/ghost/env is gone,
and `off` refuses a unit that would not start. Everything in a temp HOME and runtime with a
fake systemctl: never the operator's service, unit or runtime."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "deploy.sh"
OLD = "a" * 40  # main before the vault: its own ghost-render-env.sh, no record
NEW = "b" * 40  # a vault build, recorded
UNRECORDED = "c" * 40  # a vault build deployed before builds recorded their form

FAKE_SYSTEMCTL = """#!/usr/bin/env bash
echo "$*" >> "$FAKE_LOG"
case "$*" in
  *ActiveState*) echo active ;;
  *FragmentPath*) echo "$FAKE_UNIT" ;;
  *"-p ExecStart "*) echo "{ path=/usr/bin/true ; }" ;;
  *is-active*) echo active ;;
esac
"""


def _exe(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(0o755)


@pytest.fixture
def world(tmp_path: Path):
    home = tmp_path / "home"
    runtime = tmp_path / "runtime"
    for sha in (OLD, NEW, UNRECORDED):
        _exe(runtime / sha / "venv" / "bin" / "ghostd", "#!/bin/sh\nexit 0\n")
        (runtime / sha / "commit").write_text(sha + "\n")
    _exe(runtime / OLD / "src" / "infra" / "systemd" / "ghost-render-env.sh", "#!/bin/sh\n")
    (runtime / NEW / "dropin-form").write_text("vault\n")
    os.symlink(runtime / NEW, runtime / "current")
    os.symlink(runtime / OLD, runtime / "previous")
    _exe(tmp_path / "bin" / "systemctl", FAKE_SYSTEMCTL)
    _exe(tmp_path / "bin" / "journalctl", "#!/bin/sh\n")
    (home / ".config" / "ghost").mkdir(parents=True)
    env = {
        "PATH": f"{tmp_path / 'bin'}:/usr/bin:/bin",
        "HOME": str(home),
        "GHOST_RUNTIME_DIR": str(runtime),
        "GHOST_SYSTEMCTL": str(tmp_path / "bin" / "systemctl"),
        "GHOST_JOURNALCTL": str(tmp_path / "bin" / "journalctl"),
        "FAKE_LOG": str(tmp_path / "systemctl.log"),
        "FAKE_UNIT": str(tmp_path / "ghostd.service"),
    }
    dropin = home / ".config" / "systemd" / "user" / "ghostd.service.d" / "60-runtime.conf"

    def run(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SCRIPT), *args], env=env, capture_output=True, text=True, timeout=60
        )

    return run, home, runtime, dropin


def _current(runtime: Path) -> str:
    return Path(os.readlink(runtime / "current")).name


def test_each_build_gets_the_drop_in_it_needs(world):
    run, _home, runtime, _ = world
    vault = run("dropin", NEW)
    assert vault.returncode == 0, vault.stderr
    assert "ExecStartPre=\n" in vault.stdout and "ghost-render-env" not in vault.stdout
    assert "EnvironmentFile=-%h/.config/ghost/settings.env" in vault.stdout
    old = run("dropin", OLD).stdout
    assert f"ExecStartPre={runtime}/current/src/infra/systemd/ghost-render-env.sh" in old
    # Optional: the renderer runs first and writes it, even after the import deleted it.
    assert "EnvironmentFile=\nEnvironmentFile=-%h/.config/ghost/env\n" in old
    assert f"ExecStart={runtime}/current/venv/bin/ghostd run" in old
    # No record and no renderer in its tree: a vault build.
    assert "ghost-render-env" not in run("dropin", UNRECORDED).stdout


def test_rollback_to_a_build_from_before_the_vault(world):
    run, home, runtime, dropin = world
    (home / ".config" / "ghost" / "env").write_text("ANTHROPIC_AUTH_TOKEN=x\n")
    r = run("rollback")
    assert r.returncode == 0, r.stderr
    assert _current(runtime) == OLD
    text = dropin.read_text()
    assert f"ExecStartPre={runtime}/current/src/infra/systemd/ghost-render-env.sh" in text
    assert "EnvironmentFile=-%h/.config/ghost/env" in text
    assert "settings.env" not in text
    # And forward again: the vault build's own form.
    r = run("rollback")
    assert r.returncode == 0, r.stderr
    assert _current(runtime) == NEW
    text = dropin.read_text()
    assert (
        "ghost-render-env" not in text and "EnvironmentFile=-%h/.config/ghost/settings.env" in text
    )


def test_rollback_refuses_a_pre_vault_build_once_env_is_gone(world):
    run, _home, runtime, dropin = world
    r = run("rollback")
    assert r.returncode == 1
    assert "from before Ghost's vault" in r.stderr and "~/.config/ghost/env" in r.stderr
    assert _current(runtime) == NEW  # nothing moved
    assert not dropin.exists()


def test_rollback_to_a_pre_vault_build_with_heka_vault_still_here(world):
    """Its renderer writes ~/.config/ghost/env from heka-vault's config again: allowed."""
    run, home, runtime, dropin = world
    heka = home / ".config" / "heka-vault" / "ghost.env"
    heka.parent.mkdir(parents=True)
    heka.write_text("HEKA_VAULT_URL=http://vault.lan\n")
    r = run("rollback")
    assert r.returncode == 0, r.stderr
    assert _current(runtime) == OLD
    assert "from heka-vault again" in r.stderr
    assert f"ExecStartPre={runtime}/current/src/infra/systemd/ghost-render-env.sh" in (
        dropin.read_text()
    )


def test_off_accepts_a_pre_step_whose_failure_is_ignored(world, tmp_path):
    """``ExecStartPre=-…``: systemd starts the unit even when it fails, so it isn't fatal."""
    run, _home, _runtime, dropin = world
    dropin.parent.mkdir(parents=True)
    dropin.write_text("[Service]\n")
    (tmp_path / "ghostd.service").write_text(
        "[Service]\n"
        "ExecStartPre=-%h/projects/active/ghost/infra/systemd/gone.sh\n"
        "ExecStartPre=@-%h/also/gone.sh arg\n"
        "EnvironmentFile=-%h/.config/ghost/settings.env\n"
        "ExecStart=/usr/bin/true\n"
    )
    r = run("off")
    assert r.returncode == 0, r.stderr
    assert not dropin.exists()


def test_off_refuses_a_unit_from_before_the_vault(world, tmp_path):
    run, home, _runtime, dropin = world
    dropin.parent.mkdir(parents=True)
    dropin.write_text("[Service]\n")
    unit = tmp_path / "ghostd.service"
    unit.write_text(
        "[Service]\n"
        "ExecStartPre=%h/projects/active/ghost/infra/systemd/ghost-render-env.sh\n"
        "EnvironmentFile=%h/.config/ghost/env\n"
        "ExecStart=/usr/bin/true\n"
    )
    r = run("off")
    assert r.returncode == 1
    assert "ghost-render-env.sh, which isn't there" in r.stderr
    assert "make service-install" in r.stderr
    assert dropin.exists()

    unit.write_text(
        "[Service]\nEnvironmentFile=-%h/.config/ghost/settings.env\nExecStart=/usr/bin/true\n"
    )
    r = run("off")
    assert r.returncode == 0, r.stderr
    assert not dropin.exists()
