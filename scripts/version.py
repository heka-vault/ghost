#!/usr/bin/env python3
"""Ghost's version, YYYY.M.N (docs/releasing.md).

The root pyproject.toml is the one source; every other copy is written from it.

  scripts/version.py               the version, any copy that disagrees, the next release's
  scripts/version.py set 2026.10.1 write it to the root pyproject.toml and every copy
  scripts/version.py set next      the same, with the next release's version
  scripts/version.py next          print the next release's version
  scripts/version.py check         exit 1 when a copy disagrees with the root
  scripts/version.py check-tag v…  exit 1 unless the tag names the root's version
  scripts/version.py release       tag main's HEAD vYYYY.M.N, after its checks; never pushes

N is the Nth release of the month, from 0, with no leading zeros: Cargo, Tauri, npm and
Debian all take it as an ordinary three-part version. The next release's version comes
from today's date and the v* tags that exist. Standard library only.
"""

from __future__ import annotations

import datetime as dt
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = "pyproject.toml"
FORMAT = re.compile(r"^(\d{4})\.(1[0-2]|[1-9])\.(0|[1-9]\d*)$")
TAG = re.compile(r"^v(\d{4})\.(1[0-2]|[1-9])\.(0|[1-9]\d*)$")
MAIN = "main"
REMOTE = "origin"

# The Python workspace: the root and its members, as uv.lock names them.
WORKSPACE = ("ghost", "ghostd", "ghost-core", "ghost-voice", "ghost-api")


@dataclass(frozen=True)
class Copy:
    """One place the version is written, and the pattern whose one group is the value."""

    path: str
    pattern: str
    count: int = 1  # how many matches the file must have

    def regex(self) -> re.Pattern[str]:
        return re.compile(self.pattern, re.MULTILINE)


def _project_toml(path: str) -> Copy:
    # The first `version = "..."` line is the [project] (or Cargo [package]) one: each of
    # these files starts with that table.
    return Copy(path, r'\A(?:(?!^version = ).*\n)*?^version = "([^"]*)"$')


def _top_json(path: str, indent: int = 2) -> Copy:
    return Copy(path, rf'^ {{{indent}}}"version": "([^"]*)",?$')


def _lock(path: str, name: str) -> Copy:
    return Copy(path, rf'^name = "{re.escape(name)}"\nversion = "([^"]*)"$')


GRADLE_MODULE = "apps/mobile/modules/ghost-call/android/build.gradle"

COPIES: tuple[Copy, ...] = (
    _project_toml("apps/ghostd/pyproject.toml"),
    _project_toml("packages/core/pyproject.toml"),
    _project_toml("packages/voice/pyproject.toml"),
    _project_toml("packages/api/pyproject.toml"),
    *(_lock("uv.lock", name) for name in WORKSPACE),
    _top_json("package.json"),
    _top_json("apps/desktop/package.json"),
    _top_json("apps/mobile/package.json"),
    _top_json("packages/client-ts/package.json"),
    _top_json("packages/ui/package.json"),
    _top_json("apps/mobile/app.json", indent=4),  # expo.version
    # The Cargo workspace: its [workspace.package] version is inherited by the daemon's crates
    # under crates/; the shell keeps its own copy; one lock file at the root.
    _project_toml("Cargo.toml"),
    _project_toml("apps/desktop/src-tauri/Cargo.toml"),
    _lock("Cargo.lock", "heka-ghost-desktop"),
    _top_json("apps/desktop/src-tauri/tauri.conf.json"),
    # The phone's call module (apps/mobile/android itself is generated and not committed).
    Copy(GRADLE_MODULE, r"^version = '([^']*)'$"),
    Copy(GRADLE_MODULE, r"^ +versionName '([^']*)'$"),
)


class VersionError(Exception):
    pass


def current(root: Path = ROOT) -> str:
    with (root / SOURCE).open("rb") as f:
        return str(tomllib.load(f)["project"]["version"])


def parse(version: str) -> tuple[int, int, int]:
    m = FORMAT.match(version)
    if not m:
        raise VersionError(
            f"{version!r} is not YYYY.M.N (the month without a leading zero, N from 0): "
            "for example 2026.10.0"
        )
    return int(m[1]), int(m[2]), int(m[3])


def read(copy: Copy, root: Path = ROOT) -> str:
    text = (root / copy.path).read_text()
    found = copy.regex().findall(text)
    if len(found) != copy.count:
        raise VersionError(f"{copy.path}: expected one version, found {len(found)}")
    return found[0]


def write(copy: Copy, version: str, root: Path = ROOT) -> bool:
    """Write ``version`` into one copy; True when the file changed."""
    path = root / copy.path
    text = path.read_text()
    m = list(copy.regex().finditer(text))
    if len(m) != copy.count:
        raise VersionError(f"{copy.path}: expected one version, found {len(m)}")
    start, end = m[0].span(1)
    new = text[:start] + version + text[end:]
    if new == text:
        return False
    path.write_text(new)
    return True


def disagreeing(root: Path = ROOT) -> list[tuple[str, str]]:
    """Every copy whose version isn't the root's, as (path, what it says)."""
    want = current(root)
    out: list[tuple[str, str]] = []
    for copy in COPIES:
        got = read(copy, root)
        if got != want:
            out.append((copy.path, got))
    return out


def set_version(version: str, root: Path = ROOT) -> list[str]:
    """Write ``version`` to the root and every copy; the paths that changed."""
    parse(version)
    changed: list[str] = []
    for copy in (_project_toml(SOURCE), *COPIES):
        if write(copy, version, root) and copy.path not in changed:
            changed.append(copy.path)
    return changed


def next_version(tags: list[str], today: dt.date) -> str:
    """This month's next release: one past the highest vYYYY.M.N tag of the month, or N=0."""
    ns = [
        int(m[3])
        for t in tags
        if (m := TAG.match(t)) and (int(m[1]), int(m[2])) == (today.year, today.month)
    ]
    return f"{today.year}.{today.month}.{max(ns) + 1 if ns else 0}"


def _git(*args: str, root: Path = ROOT) -> str:
    return subprocess.run(
        ["git", "-C", str(root), *args], check=True, capture_output=True, text=True
    ).stdout.strip()


def tags(root: Path = ROOT) -> list[str]:
    return [t for t in _git("tag", "--list", "v*", root=root).splitlines() if t]


def release(root: Path = ROOT, today: dt.date | None = None) -> str:
    """Tag main's HEAD with the root's version. Refuses unless HEAD is main, clean, the same
    commit as the remote's main (so it reached main through a PR that passed CI), every copy
    agrees and the version is this month's next one. Returns the tag; pushes nothing."""
    today = today or dt.date.today()
    branch = _git("rev-parse", "--abbrev-ref", "HEAD", root=root)
    if branch != MAIN:
        raise VersionError(f"release from {MAIN}, not {branch}: git switch {MAIN} && git pull")
    if _git("status", "--porcelain", "--untracked-files=no", root=root):
        raise VersionError("the working tree has uncommitted changes; a release is a commit")
    try:
        _git("fetch", "--quiet", "--tags", REMOTE, MAIN, root=root)
    except subprocess.CalledProcessError as e:
        raise VersionError(f"couldn't fetch {REMOTE}: {e.stderr.strip()}") from e
    head = _git("rev-parse", "HEAD", root=root)
    remote = _git("rev-parse", f"{REMOTE}/{MAIN}", root=root)
    if head != remote:
        raise VersionError(
            f"HEAD {head[:12]} is not {REMOTE}/{MAIN} {remote[:12]}: git pull (or push "
            "through a PR) so the tagged commit is the one CI passed on main"
        )
    bad = disagreeing(root)
    if bad:
        raise VersionError(
            "copies disagree with pyproject.toml: "
            + ", ".join(f"{p} ({v})" for p, v in bad)
            + "; make version V=… in a PR"
        )
    version = current(root)
    parse(version)
    tag = f"v{version}"
    existing = tags(root)
    if tag in existing:
        raise VersionError(f"{tag} exists already; bump with make version V=next in a PR")
    want = next_version(existing, today)
    if version != want:
        raise VersionError(
            f"main says {version}, but this month's next release is {want}: "
            f"make version V={want} in a PR, merge it, then make release"
        )
    _git("tag", "--annotate", tag, "--message", f"Ghost {version}", root=root)
    return tag


def main(argv: list[str]) -> int:
    cmd = argv[0] if argv else "show"
    try:
        if cmd == "show":
            version = current()
            print(f"version {version} (pyproject.toml)")
            for path, got in disagreeing():
                print(f"  {path} says {got}")
            print(f"next release: {next_version(tags(), dt.date.today())}")
        elif cmd == "set" and len(argv) == 2:
            version = next_version(tags(), dt.date.today()) if argv[1] == "next" else argv[1]
            changed = set_version(version)
            print(f"version {version}: " + (", ".join(changed) if changed else "nothing to change"))
        elif cmd == "next":
            print(next_version(tags(), dt.date.today()))
        elif cmd == "check":
            bad = disagreeing()
            for path, got in bad:
                print(f"{path} says {got}, pyproject.toml {current()}", file=sys.stderr)
            if bad:
                print(f"make version V={current()} writes them all", file=sys.stderr)
                return 1
            print(f"version {current()}: every copy agrees")
        elif cmd == "check-tag" and len(argv) == 2:
            want = f"v{current()}"
            if argv[1] != want:
                print(f"tag {argv[1]} but pyproject.toml says {want}", file=sys.stderr)
                return 1
        elif cmd == "release":
            tag = release()
            sha = _git("rev-parse", "--short=12", "HEAD")
            print(f"tagged {tag} at {sha}. Nothing is pushed; to publish the .debs:")
            print(f"  git push {REMOTE} {tag}")
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except VersionError as e:
        print(f"version: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
