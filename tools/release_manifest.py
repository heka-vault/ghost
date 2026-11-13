#!/usr/bin/env python3
"""The update manifest a release carries, `latest.json` (ADR 0066, docs/releasing.md).

  tools/release_manifest.py <bundle dir> <version> <base url> <notes file>

reads the signed update artifacts `tauri build` left in <bundle dir> (the .AppImage and
the .deb, each with its `.sig` beside it), and prints the manifest Tauri's updater reads:
the version, the release notes as plain text, the time, and per platform the file's URL
(<base url>/<file name>) and its signature. The AppImage is also the platform's plain
entry (`linux-x86_64`), which the updater falls back to when it cannot tell how it was
installed. Nothing here signs anything or reads a key: the signatures are the bundler's.
Standard library only.
"""

from __future__ import annotations

import datetime as dt
import json
import sys
from pathlib import Path

# The entries the updater looks up: `{os}-{arch}-{bundle_type}`, then `{os}-{arch}`.
KINDS = (("linux-x86_64-appimage", ".AppImage"), ("linux-x86_64-deb", ".deb"))
NOTES_LIMIT = 4000


class ManifestError(Exception):
    pass


def plain_notes(text: str) -> str:
    """The release notes as plain text, bounded: what Settings shows under "What's new"."""
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").split("\n")]
    out = "\n".join(lines).strip()
    if len(out) > NOTES_LIMIT:
        out = out[: NOTES_LIMIT - 1].rstrip() + "…"
    return out


def artifact(folder: Path, suffix: str, version: str) -> Path:
    found = sorted(
        p for p in folder.iterdir() if p.name.endswith(suffix) and f"_{version}_" in p.name
    )
    if len(found) != 1:
        names = ", ".join(p.name for p in found) or "none"
        raise ManifestError(f"one *_{version}_*{suffix} expected in {folder}, found {names}")
    sig = found[0].with_name(found[0].name + ".sig")
    if not sig.is_file():
        raise ManifestError(f"{found[0].name} has no signature beside it ({sig.name})")
    return found[0]


def manifest(folder: Path, version: str, base_url: str, notes: str, now: dt.datetime) -> dict:
    platforms: dict[str, dict[str, str]] = {}
    for kind, suffix in KINDS:
        path = artifact(folder, suffix, version)
        signature = path.with_name(path.name + ".sig").read_text().strip()
        if not signature:
            raise ManifestError(f"{path.name}.sig is empty")
        platforms[kind] = {"signature": signature, "url": f"{base_url.rstrip('/')}/{path.name}"}
    platforms["linux-x86_64"] = dict(platforms["linux-x86_64-appimage"])
    return {
        "version": version,
        "notes": plain_notes(notes),
        "pub_date": now.astimezone(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    folder, version, base_url, notes_file = argv
    try:
        out = manifest(
            Path(folder), version, base_url, Path(notes_file).read_text(), dt.datetime.now(dt.UTC)
        )
    except (ManifestError, OSError) as e:
        print(f"release_manifest: {e}", file=sys.stderr)
        return 1
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
