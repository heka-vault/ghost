"""The licences of what Ghost is built from, for Settings › About (ADR 0072).

Writes `apps/desktop/public/licences.json` from the lock files the repository already has:
`Cargo.lock` through `cargo metadata --locked --offline` (each crate's own `license` field)
and the desktop's production JavaScript through `pnpm licenses list --prod --json`. The page
lists it as data; nothing here is fetched at run time. `make licences` runs it; run it again
after a dependency changes (tests/test_licences.py says when it is behind Cargo.lock).
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "apps" / "desktop" / "public" / "licences.json"


def rust() -> list[dict[str, str]]:
    meta = json.loads(
        subprocess.run(
            [
                "cargo",
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--offline",
                "--all-features",
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    members = set(meta["workspace_members"])
    out = {
        (p["name"], p["version"]): p.get("license") or "see the crate"
        for p in meta["packages"]
        if p["id"] not in members and p.get("source")
    }
    # A crate in Cargo.lock that no build of the workspace resolves (a build tool's own
    # binary): its licence from its manifest in the local registry.
    for name, version in lock_packages():
        if (name, version) not in out:
            out[(name, version)] = registry_licence(name, version)
    return [{"name": n, "version": v, "license": lic} for (n, v), lic in sorted(out.items())]


def lock_packages() -> list[tuple[str, str]]:
    text = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    out = []
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        version = re.search(r'^version = "([^"]+)"', block, re.M)
        if name and version and re.search(r"^source = ", block, re.M):
            out.append((name.group(1), version.group(1)))
    return out


def registry_licence(name: str, version: str) -> str:
    home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for manifest in (home / "registry" / "src").glob(f"*/{name}-{version}/Cargo.toml"):
        m = re.search(r'^license = "([^"]+)"', manifest.read_text(encoding="utf-8"), re.M)
        if m:
            return m.group(1)
    return "see the crate"


def js() -> list[dict[str, str]]:
    raw = json.loads(
        subprocess.run(
            ["pnpm", "licenses", "list", "--prod", "--json"],
            cwd=ROOT / "apps" / "desktop",
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    out: dict[tuple[str, str], str] = {}
    for lic, pkgs in raw.items():
        for p in pkgs:
            for v in p.get("versions") or [p.get("version", "")]:
                out[(p["name"], v)] = lic
    return [{"name": n, "version": v, "license": lic} for (n, v), lic in sorted(out.items())]


def build(rust_list: list[dict[str, str]], js_list: list[dict[str, str]]) -> dict:
    return {
        "ghost": "MIT",
        "about": (
            "The open-source software Ghost is built from, and each one's licence,"
            " from the repository's lock files."
        ),
        "rust": rust_list,
        "javascript": js_list,
    }


def main() -> int:
    doc = build(rust(), js())
    OUT.write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {len(doc['rust'])} crates, {len(doc['javascript'])} packages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
