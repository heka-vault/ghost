"""The page scripts' chunk ids for error reports (ADR 0067): tools/report_chunks.py."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "ghost_report_chunks", ROOT / "tools/report_chunks.py"
)
assert _spec and _spec.loader
rc = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = rc
_spec.loader.exec_module(rc)

MAP = {"version": 3, "sources": ["../../src/main.tsx"], "names": [], "mappings": "AAAA"}


def dist(tmp: Path, scripts: dict[str, str], maps: bool = True) -> Path:
    d = tmp / "dist"
    (d / "assets").mkdir(parents=True)
    (d / "index.html").write_text("<script src=/assets/x.js></script>")
    for name, text in scripts.items():
        (d / "assets" / name).write_text(text, newline="")
        if maps:
            (d / "assets" / f"{name}.map").write_text(json.dumps(MAP))
    return d


def test_the_id_is_posthog_clis_stable_chunk_id():
    # crates/ghost-reports chunks.rs checks the same value with Uuid::new_v5, the CLI's call.
    assert rc.chunk_id("code();") == "1bf5417a-ec0f-5e20-9c5f-4f9085e56f72"


def test_inject_writes_the_clis_two_marks_and_a_manifest(tmp_path):
    d = dist(tmp_path, {"index-AbC12.js": "code();", "floating-Q1.js": "other();\r\n"})
    out = tmp_path / "maps"
    manifest = rc.inject(d, out)
    cid = rc.chunk_id("code();")
    assert manifest["assets/index-AbC12.js"] == cid
    # The script: unchanged, then the comment, so the map stays exact.
    assert (d / "assets/index-AbC12.js").read_bytes() == f"code();\n//# chunkId={cid}".encode()
    # Line endings are the script's own: the id is of its exact text.
    assert manifest["assets/floating-Q1.js"] == rc.chunk_id("other();\r\n")
    m = json.loads((out / "assets/index-AbC12.js.map").read_text())
    assert m["chunk_id"] == cid and m["mappings"] == "AAAA"
    assert (out / "assets/index-AbC12.js").read_bytes() == (
        d / "assets/index-AbC12.js"
    ).read_bytes()
    assert json.loads((d / rc.MANIFEST).read_text()) == manifest


def test_no_map_is_left_to_ship(tmp_path):
    d = dist(tmp_path, {"index-AbC12.js": "code();"})
    rc.inject(d, tmp_path / "maps")
    assert not list(d.rglob("*.map"))


def test_a_stray_map_fails_the_build(tmp_path):
    d = dist(tmp_path, {"index-AbC12.js": "code();"})
    (d / "stray.css.map").write_text("{}")
    with pytest.raises(rc.InjectError, match="would ship"):
        rc.inject(d, tmp_path / "maps")


def test_a_build_without_maps_is_refused(tmp_path):
    d = dist(tmp_path, {"index-AbC12.js": "code();"}, maps=False)
    with pytest.raises(rc.InjectError, match="no script with a source map"):
        rc.inject(d, tmp_path / "maps")


def test_a_script_injected_twice_is_refused(tmp_path):
    d = dist(
        tmp_path, {"index-AbC12.js": "code();\n//# chunkId=1bf5417a-ec0f-5e20-9c5f-4f9085e56f72"}
    )
    with pytest.raises(rc.InjectError, match="already carries"):
        rc.inject(d, tmp_path / "maps")


def test_the_cli_says_how_it_is_used(capsys):
    assert rc.main([]) == 2
    assert "usage" in capsys.readouterr().err


def test_a_map_naming_an_absolute_path_is_refused(tmp_path):
    d = dist(tmp_path, {"index-AbC12.js": "code();"})
    bad = dict(MAP, sources=["/home/runner/work/ghost/ghost/apps/desktop/src/main.tsx"])
    (d / "assets/index-AbC12.js.map").write_text(json.dumps(bad))
    with pytest.raises(rc.InjectError, match="absolute path"):
        rc.inject(d, tmp_path / "maps")
