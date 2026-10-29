"""scripts/deploy.sh: the drop-in a build runs with (ADR 0036, ADR 0039), a rollback to the
build before, a build from before the Rust daemon refused, and `off` refusing a unit that
would not start. Everything in a temp HOME and runtime with a fake systemctl: never the
operator's service, unit or runtime."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "deploy.sh"
OLD = "a" * 40  # a build from before the Rust daemon: a venv, no bin/ghostd
NEW = "b" * 40  # the build the service runs
PREV = "c" * 40  # the build before it
ORT = "libonnxruntime.so.1.24.4"

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
    for sha in (NEW, PREV):
        _exe(runtime / sha / "bin" / "ghostd", "#!/bin/sh\nexit 0\n")
        (runtime / sha / "commit").write_text(sha + "\n")
    _exe(runtime / OLD / "venv" / "bin" / "ghostd", "#!/bin/sh\nexit 0\n")
    (runtime / OLD / "commit").write_text(OLD + "\n")
    (runtime / NEW / "lib").mkdir()
    (runtime / NEW / "lib" / ORT).write_bytes(b"")
    os.symlink(runtime / NEW, runtime / "current")
    os.symlink(runtime / PREV, runtime / "previous")
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


def _point(runtime: Path, link: str, sha: str) -> None:
    os.unlink(runtime / link)
    os.symlink(runtime / sha, runtime / link)


def test_the_drop_in_runs_the_builds_daemon_with_settings_only(world):
    run, _home, runtime, _ = world
    r = run("dropin", NEW)
    assert r.returncode == 0, r.stderr
    text = r.stdout
    assert "ExecStartPre=\n" in text and "ghost-render-env" not in text
    assert "EnvironmentFile=-%h/.config/ghost/settings.env" in text
    assert f"Environment=GHOST_INFRA_DIR={runtime}/current/src/infra" in text
    assert f"ExecStart={runtime}/current/bin/ghostd run" in text
    assert f"ExecStopPost={runtime}/current/bin/ghostd restore-volumes" in text
    assert "venv" not in text and "GHOST_DAEMON_IMPL" not in text


def test_the_drop_in_names_the_runtime_library_the_build_carries(world):
    run, _home, runtime, _ = world
    assert f"Environment=ORT_DYLIB_PATH={runtime}/current/lib/{ORT}" in run("dropin", NEW).stdout
    # A build without one leaves the daemon to find it (the bundle's, or the cache).
    assert "ORT_DYLIB_PATH" not in run("dropin", PREV).stdout


def test_rollback_goes_to_the_build_before_and_rewrites_the_drop_in(world):
    run, _home, runtime, dropin = world
    r = run("rollback")
    assert r.returncode == 0, r.stderr
    assert _current(runtime) == PREV
    assert Path(os.readlink(runtime / "previous")).name == NEW
    text = dropin.read_text()
    assert f"ExecStart={runtime}/current/bin/ghostd run" in text
    assert "ORT_DYLIB_PATH" not in text  # PREV carries no library
    # And forward again.
    r = run("rollback")
    assert r.returncode == 0, r.stderr
    assert _current(runtime) == NEW
    assert f"Environment=ORT_DYLIB_PATH={runtime}/current/lib/{ORT}" in dropin.read_text()


def test_rollback_refuses_a_build_from_before_the_rust_daemon(world):
    run, _home, runtime, dropin = world
    _point(runtime, "previous", OLD)
    r = run("rollback")
    assert r.returncode == 1
    assert "from before the Rust daemon" in r.stderr and "no bin/ghostd" in r.stderr
    assert _current(runtime) == NEW  # nothing moved
    assert not dropin.exists()


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
