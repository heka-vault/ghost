"""One version, written everywhere (#33, docs/releasing.md).

The root pyproject.toml is the source; tools/version.py writes every copy. These tests
read each copy with its own format's parser, not the script's patterns, so a pattern that
drifts can't hide a copy that disagrees."""

from __future__ import annotations

import datetime as dt
import importlib.util
import json
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]

_spec = importlib.util.spec_from_file_location("ghost_version_script", ROOT / "tools/version.py")
assert _spec and _spec.loader
version_script = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = version_script
_spec.loader.exec_module(version_script)


def _toml(path: str) -> dict:
    with (ROOT / path).open("rb") as f:
        return tomllib.load(f)


def _json(path: str) -> dict:
    return json.loads((ROOT / path).read_text())


def every_copy() -> dict[str, str]:
    """What each copy says, read as its format says to."""
    out: dict[str, str] = {}
    for p in ("tools/pyproject.toml", "tools/oracle/pyproject.toml"):
        out[p] = _toml(p)["project"]["version"]
    lock = {pkg["name"]: pkg["version"] for pkg in _toml("uv.lock")["package"]}
    for name in version_script.WORKSPACE:
        out[f"uv.lock {name}"] = lock[name]
    oracle = {pkg["name"]: pkg["version"] for pkg in _toml("tools/oracle/uv.lock")["package"]}
    out["tools/oracle/uv.lock"] = oracle[version_script.ORACLE]
    for p in (
        "package.json",
        "apps/desktop/package.json",
        "apps/mobile/package.json",
        "packages/client-ts/package.json",
        "packages/ui/package.json",
        "apps/desktop/src-tauri/tauri.conf.json",
    ):
        out[p] = _json(p)["version"]
    out["apps/mobile/app.json"] = _json("apps/mobile/app.json")["expo"]["version"]
    out["Cargo.toml"] = _toml("Cargo.toml")["workspace"]["package"]["version"]
    out["apps/desktop/src-tauri/Cargo.toml"] = _toml("apps/desktop/src-tauri/Cargo.toml")[
        "package"
    ]["version"]
    cargo = _toml("Cargo.lock")["package"]
    out["Cargo.lock"] = next(p["version"] for p in cargo if p["name"] == "heka-ghost-desktop")
    # Gradle has no parser here: every quoted value on a version line, however it is written.
    gradle = (ROOT / version_script.GRADLE_MODULE).read_text()
    for i, line in enumerate(gradle.splitlines()):
        words = line.split()
        if words and words[0] in {"version", "versionName"}:
            out[f"build.gradle:{i + 1}"] = line.split("'")[1] if "'" in line else line.split('"')[1]
    assert sum(k.startswith("build.gradle") for k in out) == 2
    return out


def test_every_copy_says_the_root_version():
    want = _toml("pyproject.toml")["project"]["version"]
    wrong = {k: v for k, v in every_copy().items() if v != want}
    assert not wrong, f"pyproject.toml says {want}; run make version V={want}. Wrong: {wrong}"
    assert version_script.disagreeing() == []


def test_the_version_is_a_date_version():
    version_script.parse(version_script.current())


def test_no_manifest_is_left_out():
    """A new package.json, pyproject.toml, Cargo.toml or build.gradle of Ghost’s (the root, apps/,
    packages/, tools/) must be one of the script's copies."""
    try:
        files = subprocess.run(
            ["git", "-C", str(ROOT), "ls-files"], check=True, capture_output=True, text=True
        ).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        pytest.skip("not a git checkout")
    manifests = {
        f
        for f in files
        if Path(f).name in {"package.json", "pyproject.toml", "Cargo.toml", "build.gradle"}
        and (Path(f).parent == Path(".") or f.startswith(("apps/", "packages/", "tools/")))
    }
    known = {c.path for c in version_script.COPIES} | {version_script.SOURCE}
    assert manifests - known == set()


@pytest.mark.parametrize(
    ("tags", "today", "want"),
    [
        ([], dt.date(2026, 10, 28), "2026.10.0"),
        (["v0.1.0"], dt.date(2026, 10, 28), "2026.10.0"),
        (["v2026.10.0"], dt.date(2026, 10, 28), "2026.10.1"),
        (["v2026.10.0", "v2026.10.9", "v2026.10.2"], dt.date(2026, 10, 1), "2026.10.10"),
        (["v2026.10.3"], dt.date(2026, 11, 1), "2026.11.0"),
        (["v2026.1.4", "v2026.10.0"], dt.date(2026, 1, 5), "2026.1.5"),
        (["v2026.10.01", "nightly", "2026.10.5"], dt.date(2026, 10, 2), "2026.10.0"),
    ],
)
def test_next_version_counts_this_months_tags(tags, today, want):
    assert version_script.next_version(tags, today) == want


@pytest.mark.parametrize("bad", ["0.1", "2026.10", "2026.010.0", "2026.13.0", "2026.10.01", "v1"])
def test_only_date_versions_are_accepted(bad):
    with pytest.raises(version_script.VersionError):
        version_script.parse(bad)


def _copy_tree(dest: Path) -> None:
    for path in {version_script.SOURCE, *(c.path for c in version_script.COPIES)}:
        (dest / path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / path, dest / path)


def test_set_writes_every_copy_and_nothing_else(tmp_path):
    _copy_tree(tmp_path)
    before = {p: (tmp_path / p).read_text() for p in {c.path for c in version_script.COPIES}}
    version_script.set_version("2031.2.7", tmp_path)
    assert version_script.current(tmp_path) == "2031.2.7"
    assert version_script.disagreeing(tmp_path) == []
    for p, text in before.items():
        after = (tmp_path / p).read_text()
        # One line changed per copy (two in uv.lock), the rest byte for byte.
        changed = [a for a, b in zip(after.splitlines(), text.splitlines(), strict=True) if a != b]
        assert changed and all("2031.2.7" in line for line in changed), p
    assert version_script.set_version("2031.2.7", tmp_path) == []


def _git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True
    ).stdout.strip()


@pytest.fixture
def repo(tmp_path):
    """A checkout of a copy of the versioned files, with a bare origin, on main."""
    origin, work = tmp_path / "origin.git", tmp_path / "work"
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(origin)], check=True)
    work.mkdir()
    _copy_tree(work)
    _git(work, "init", "-q", "-b", "main")
    _git(work, "config", "user.email", "test@example.com")
    _git(work, "config", "user.name", "Test")
    _git(work, "config", "tag.gpgSign", "false")
    _git(work, "config", "commit.gpgSign", "false")
    version_script.set_version("2026.10.0", work)
    _git(work, "add", "-A")
    _git(work, "commit", "-q", "-m", "init")
    _git(work, "remote", "add", "origin", str(origin))
    _git(work, "push", "-q", "origin", "main")
    return work


OCT = dt.date(2026, 10, 28)


def test_release_tags_main_and_pushes_nothing(repo):
    assert version_script.release(repo, OCT) == "v2026.10.0"
    assert _git(repo, "tag", "--list") == "v2026.10.0"
    assert _git(repo, "ls-remote", "--tags", "origin") == ""


def test_release_wants_the_next_version(repo):
    version_script.release(repo, OCT)
    with pytest.raises(version_script.VersionError, match="exists already"):
        version_script.release(repo, OCT)


def test_release_wants_this_months_version(repo):
    with pytest.raises(version_script.VersionError, match="next release is 2026.11.0"):
        version_script.release(repo, dt.date(2026, 11, 2))


def test_release_wants_the_commit_ci_passed_on_main(repo):
    (repo / "README").write_text("x")
    _git(repo, "add", "README")
    _git(repo, "commit", "-q", "-m", "not through a PR")
    with pytest.raises(version_script.VersionError, match="is not origin/main"):
        version_script.release(repo, OCT)


def test_release_refuses_a_branch_or_a_dirty_tree(repo):
    (repo / "package.json").write_text((repo / "package.json").read_text() + " ")
    with pytest.raises(version_script.VersionError, match="uncommitted"):
        version_script.release(repo, OCT)
    _git(repo, "checkout", "-q", "--", "package.json")
    _git(repo, "switch", "-q", "-c", "feat/x")
    with pytest.raises(version_script.VersionError, match="release from main"):
        version_script.release(repo, OCT)


def test_release_refuses_copies_that_disagree(repo):
    version_script.write(version_script.COPIES[0], "2026.9.9", repo)
    _git(repo, "commit", "-q", "-am", "drift")
    _git(repo, "push", "-q", "origin", "main")
    with pytest.raises(version_script.VersionError, match="disagree"):
        version_script.release(repo, OCT)
