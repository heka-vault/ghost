"""Ghost's folder (ADR 0035): the rules, watches.json and hooks.json read from it, and
what an older layout left there named, not read."""

from __future__ import annotations

import json
from pathlib import Path

from ghost_core.folder import GhostFolder, read_rules, stale

WATCH = {
    "app": "github",
    "read": "list_workflow_runs",
    "every": 60,
    "items": "workflow_runs",
    "fields": {"id": {"at": "id", "type": "int"}},
    "key": "{id}",
    "title": "Run {id} failed",
}


def test_an_empty_folder_reads_as_nothing_wrong(tmp_path: Path):
    folder = GhostFolder(tmp_path)
    assert folder.watches == {} and folder.hooks == {} and folder.problems == []
    assert folder.rules() == ""
    folder.ensure_layout()
    assert (tmp_path / "agents").is_dir()


def test_the_rules_are_agents_md_capped(tmp_path: Path):
    (tmp_path / "AGENTS.md").write_text("# Ghost\n\n" + "x" * 9000)
    text = read_rules(tmp_path)
    assert text.startswith("# Ghost") and text.endswith("…") and len(text) < 8100
    assert GhostFolder(tmp_path).rules() == text


def test_watches_and_hooks_load_from_their_files(tmp_path: Path):
    (tmp_path / "watches.json").write_text(json.dumps({"ci": WATCH}))
    folder = GhostFolder(tmp_path)
    w = folder.watches["ci"]
    assert w.source == "watch:ci"
    draft = w.draft({"id": 7}, "tell")
    assert draft is not None and draft.key == "ci:7"  # <watch>:<key>, no other prefix
    # a broken file is a problem, and the folder still reads
    (tmp_path / "hooks.json").write_text("{not json")
    (tmp_path / "watches.json").write_text(json.dumps(["ci"]))
    seen = []
    folder.on_refresh.append(lambda: seen.append(1))
    folder.refresh()
    assert seen == [1] and folder.watches == {} and folder.hooks == {}
    assert any("hooks.json" in p for p in folder.problems)
    assert any("watches.json" in p and "keyed by name" in p for p in folder.problems)


def test_what_an_older_layout_left_is_named_and_not_read(tmp_path: Path):
    (tmp_path / "department.json").write_text(
        json.dumps({"title": "Ghost", "voice": "bm_george", "watches": {"ci": WATCH}})
    )
    (tmp_path / "memory.md").write_text("Shared facts.\n")
    (tmp_path / "docs").mkdir()
    (tmp_path / "mcp.json").write_text("{}")
    (tmp_path / "departments" / "marketing").mkdir(parents=True)
    (tmp_path / "departments" / "sales").mkdir(parents=True)
    lines = stale(tmp_path)
    assert len(lines) == 6
    old = next(line for line in lines if "department.json" in line)
    assert "watches.json" in old and "hooks.json" in old and "by hand" in old
    assert any("memory.md" in line for line in lines)
    assert any(line.startswith(str(tmp_path / "docs")) for line in lines)
    assert any("mcp.json" in line and "apps.json" in line for line in lines)
    assert [line for line in lines if "/departments/" in line] == [
        line for line in lines if "marketing" in line or "sales" in line
    ]
    folder = GhostFolder(tmp_path)
    assert folder.watches == {}  # department.json's watches are not read
    assert folder.problems == lines


def test_an_empty_departments_folder_is_not_named(tmp_path: Path):
    (tmp_path / "departments").mkdir()
    assert stale(tmp_path) == []
