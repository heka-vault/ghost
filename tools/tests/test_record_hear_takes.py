"""scripts/record-hear-takes.py: the plan read from docs/benchmarks/hear-plan.json, the take
lengths, the names, the manifest the harness reads, and resume. No microphone: the capture
code is never loaded."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "record-hear-takes.py"


@pytest.fixture(scope="module")
def rec():
    spec = importlib.util.spec_from_file_location("record_hear_takes", SCRIPT)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod  # dataclasses look their module up
    spec.loader.exec_module(mod)
    return mod


def test_the_plan_loads_with_unique_ids_in_their_blocks(rec):
    plan = rec.load_plan()
    ids = [t.id for t in plan]
    assert len(ids) == len(set(ids))
    assert all(t.id.startswith(t.block) for t in plan)
    assert {t.kind for t in plan} == {"speech", "reply", "silence"}
    assert rec.blocks_of(plan) == tuple(sorted(rec.blocks_of(plan)))


def test_the_plan_does_not_take_too_long_to_record(rec):
    minutes = rec.plan_minutes(rec.load_plan())
    assert 8 < minutes < 20, minutes


def test_take_lengths_follow_the_line(rec):
    assert rec.take_seconds("reply", "Yes.") == rec.SECS_REPLY
    assert rec.take_seconds("silence", "", 6) == 6.0
    short = rec.take_seconds("speech", "Read me issue four oh three.")
    paused = rec.take_seconds("speech", "Read me issue four oh three ... from the app repo.")
    assert rec.SECS_MIN <= short < paused
    assert paused * 2 == int(paused * 2)  # half seconds


def test_a_pause_and_a_silent_take_are_shown_plainly(rec):
    plan = {t.id: t for t in rec.load_plan()}
    assert "(pause)" in plan["B01"].shown
    assert plan["H01"].shown.startswith("(")
    assert plan["A01"].shown == plan["A01"].text


def test_blocks_parse_and_unknown_ones_are_refused(rec):
    plan = rec.load_plan()
    assert rec.parse_blocks("a, d", plan) == ("A", "D")
    assert rec.parse_blocks("", plan) is None
    with pytest.raises(ValueError):
        rec.parse_blocks("Z", plan)
    assert {t.block for t in rec.select(plan, ("D",))} == {"D"}


def test_the_manifest_is_what_the_harness_reads(rec, tmp_path):
    take = {t.id: t for t in rec.load_plan()}["A04"]
    e = rec.entry(take, raw=True, rms=-30.04, peak=-6.0)
    # the fields crates/ghost-voice/src/perf/hear/plan.rs `Entry` needs
    for key in ("id", "file", "raw", "block", "condition", "kind", "text", "tags", "terms"):
        assert key in e
    assert e["file"] == "A04-speech.aec.wav" and e["raw"] == "A04-speech.raw.wav"
    assert e["terms"] == ["heka vault", "Ghost"]
    assert rec.entry(take, raw=False, rms=0, peak=0)["raw"] is None
    path = tmp_path / rec.MANIFEST
    rec.save_manifest(path, {"A04": e})
    assert json.loads(path.read_text()) == e


def test_resume_skips_what_is_done_and_last_line_wins(rec, tmp_path):
    plan = rec.load_plan()
    path = tmp_path / rec.MANIFEST
    first, second = plan[0], plan[1]
    lines = [
        json.dumps(rec.skipped_entry(first)),
        "{torn",
        json.dumps(rec.entry(first, raw=False, rms=-20, peak=-3)),
        json.dumps(rec.skipped_entry(second)),
    ]
    path.write_text("\n".join(lines) + "\n")
    entries = rec.load_manifest(path)
    assert not entries[first.id].get("skipped")
    todo = rec.pending(plan, entries)
    assert first not in todo and second not in todo
    assert second in rec.pending(plan, entries, retry_skipped=True)


def test_the_session_is_dated(rec):
    import datetime as dt

    assert rec.default_session(dt.date(2026, 11, 6)).name == "2026-11-06"
