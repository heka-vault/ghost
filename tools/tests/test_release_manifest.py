"""The update manifest a release carries (ADR 0066): tools/release_manifest.py."""

from __future__ import annotations

import datetime as dt
import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("ghost_release_manifest", ROOT / "tools/release_manifest.py")
assert _spec and _spec.loader
rm = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = rm
_spec.loader.exec_module(rm)

BASE = "https://github.com/heka-vault/ghost/releases/download/v2026.11.1"
NOW = dt.datetime(2026, 11, 13, 12, 0, tzinfo=dt.UTC)


def bundle(tmp: Path, version: str = "2026.11.1") -> Path:
    for name in (f"heka-ghost-desktop_{version}_amd64.AppImage", f"heka-ghost-desktop_{version}_amd64.deb"):
        (tmp / name).write_bytes(b"x")
        (tmp / f"{name}.sig").write_text(f"sig-of-{name}\n")
    return tmp


def test_every_kind_the_updater_looks_up_is_there_with_its_signature(tmp_path: Path) -> None:
    m = rm.manifest(bundle(tmp_path), "2026.11.1", BASE + "/", "## What's new\n* a fix\n", NOW)
    assert m["version"] == "2026.11.1"
    assert m["pub_date"] == "2026-11-13T12:00:00Z"
    app = m["platforms"]["linux-x86_64-appimage"]
    assert app["url"] == f"{BASE}/heka-ghost-desktop_2026.11.1_amd64.AppImage"
    assert app["signature"] == "sig-of-heka-ghost-desktop_2026.11.1_amd64.AppImage"
    deb = m["platforms"]["linux-x86_64-deb"]
    assert deb["url"] == f"{BASE}/heka-ghost-desktop_2026.11.1_amd64.deb"
    # The plain entry, for an install the updater cannot name, is the AppImage.
    assert m["platforms"]["linux-x86_64"] == app


def test_a_missing_signature_refuses(tmp_path: Path) -> None:
    bundle(tmp_path)
    (tmp_path / "heka-ghost-desktop_2026.11.1_amd64.deb.sig").unlink()
    with pytest.raises(rm.ManifestError, match="no signature"):
        rm.manifest(tmp_path, "2026.11.1", BASE, "", NOW)


def test_another_versions_files_are_not_taken(tmp_path: Path) -> None:
    bundle(tmp_path, "2026.11.0")
    with pytest.raises(rm.ManifestError, match="found none"):
        rm.manifest(tmp_path, "2026.11.1", BASE, "", NOW)


def test_notes_are_bounded_plain_text(tmp_path: Path) -> None:
    m = rm.manifest(bundle(tmp_path), "2026.11.1", BASE, "a\r\nb   \n\n" + "z" * 9000, NOW)
    assert m["notes"].startswith("a\nb\n\nz")
    assert len(m["notes"]) == rm.NOTES_LIMIT
    assert m["notes"].endswith("…")
