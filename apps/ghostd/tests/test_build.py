"""What the daemon says it is (#33): its version and its build, in the status."""

from __future__ import annotations

import json
import subprocess

import pytest
from ghost_api import Build
from ghostd import build

SHA = "0123456789abcdef0123456789abcdef01234567"


def test_the_version_is_the_installed_one():
    from importlib import metadata

    assert build.version() == metadata.version("ghostd")


@pytest.mark.parametrize(
    ("record", "want"),
    [
        ({"kind": "deploy", "commit": SHA}, Build(kind="deploy", commit=SHA)),
        (
            {"kind": "release", "commit": SHA, "dirty": False},
            Build(kind="release", commit=SHA),
        ),
        ({"kind": "release", "dirty": False}, Build(kind="release")),
    ],
)
def test_a_recorded_build_is_believed(tmp_path, record, want):
    (tmp_path / build.BUILD_FILE).write_text(json.dumps(record))
    assert build.detect(prefix=tmp_path, source=tmp_path) == want


@pytest.mark.parametrize(
    "text",
    ["{", json.dumps({"kind": "nightly"}), json.dumps({"kind": "deploy", "commit": "; rm"})],
)
def test_a_bad_record_is_a_dev_build(tmp_path, text):
    (tmp_path / build.BUILD_FILE).write_text(text)
    assert build.detect(prefix=tmp_path, source=tmp_path) == Build(kind="dev")


def _git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)


def test_a_checkout_is_dev_with_its_commit_and_whether_it_is_dirty(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q")
    (repo / "a.py").write_text("x = 1\n")
    _git(repo, "add", "a.py")
    _git(
        repo, "-c", "user.email=t@example.com", "-c", "user.name=T", "-c",
        "commit.gpgSign=false", "commit", "-q", "-m", "a",
    )  # fmt: skip
    head = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True
    ).stdout.strip()
    prefix = tmp_path / "venv"
    prefix.mkdir()
    assert build.detect(prefix=prefix, source=repo) == Build(kind="dev", commit=head)
    (repo / "untracked.txt").write_text("not counted")
    assert build.detect(prefix=prefix, source=repo).dirty is False
    (repo / "a.py").write_text("x = 2\n")
    assert build.detect(prefix=prefix, source=repo) == Build(kind="dev", commit=head, dirty=True)


@pytest.mark.parametrize(
    ("launcher", "want"),
    [("make-dev", "make-dev"), ("make-start", "make-start"), ("systemd", None), ("", None)],
)
def test_a_checkout_says_which_make_target_started_it(tmp_path, launcher, want):
    env = {"GHOST_LAUNCHER": launcher}
    assert build.detect(prefix=tmp_path, source=tmp_path, environ=env).launcher == want


def test_a_recorded_build_has_no_launcher(tmp_path):
    (tmp_path / build.BUILD_FILE).write_text(json.dumps({"kind": "deploy", "commit": SHA}))
    env = {"GHOST_LAUNCHER": "make-dev"}
    assert build.detect(prefix=tmp_path, source=tmp_path, environ=env).launcher is None


def test_outside_a_checkout_it_is_dev_without_a_commit(tmp_path):
    assert build.detect(prefix=tmp_path, source=tmp_path) == Build(kind="dev")


EDITED = ", uncommitted changes"


@pytest.mark.parametrize(
    ("b", "want"),
    [
        (Build(kind="dev", commit=SHA), "Ghost 2026.10.0 · dev 0123456"),
        (Build(kind="dev", commit=SHA, dirty=True), "Ghost 2026.10.0 · dev 0123456" + EDITED),
        (Build(kind="dev"), "Ghost 2026.10.0 · dev"),
        (Build(kind="deploy", commit=SHA), "Ghost 2026.10.0 · deploy 0123456"),
        (Build(kind="release", commit=SHA), "Ghost 2026.10.0"),
        (
            Build(kind="release", commit=SHA, dirty=True),
            "Ghost 2026.10.0 · release 0123456" + EDITED,
        ),
    ],
)
def test_describe(b, want):
    assert build.describe("2026.10.0", b) == want
