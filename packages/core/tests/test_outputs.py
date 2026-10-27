"""What Ghost makes (ADR 0033): a file in its one out/ folder (ADR 0035), through the
gate, never run."""

from __future__ import annotations

import os
import stat
from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy, Store, WriteGate
from ghost_core.outputs import MAX_BYTES, SOURCE, OutputError, Outputs, OutputTarget, list_outputs

PAGE = "<!doctype html><title>Week</title><h1>Week in review</h1>"


@pytest.fixture
def world(tmp_path: Path):
    cfg = tmp_path / "ghost"
    cfg.mkdir()
    store = Store()
    outputs = Outputs(cfg)
    gate = WriteGate(store)
    gate.register(SOURCE, outputs.operations())
    return cfg, outputs, gate


async def make(gate, outputs, op="write", **args):
    operation = next(o for o in outputs.operations() if o.name == op)
    prepared = await operation.prepare(args)
    return await gate.submit(
        session_id="s1",
        ability=SOURCE,
        operation=op,
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )


async def test_a_new_file_is_written_at_announce_and_never_executable(world):
    cfg, outputs, gate = world
    sub = await make(gate, outputs, name="week.html", content=PAGE)
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"
    path = cfg / "out" / "week.html"
    assert path.read_text() == PAGE
    assert not os.stat(path).st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    assert (
        sub.outcome.readback.exists
        and "week.html in Ghost's outputs" in sub.proposal.target.spoken()
    )
    assert [f["name"] for f in list_outputs(cfg)] == ["week.html"]
    assert sub.proposal.target.scope() == "week.html"  # what a permission rule matches


async def test_replacing_is_its_own_operation_and_an_ask_rule_makes_it_wait(world):
    _, outputs, gate = world
    await make(gate, outputs, name="week.html", content=PAGE)
    with pytest.raises(OutputError, match="replace"):
        await make(gate, outputs, name="week.html", content=PAGE)
    sub = await make(gate, outputs, "replace", name="week.html", content=PAGE + "<p>v2")
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"  # ADR 0034
    gate.set_autonomy(AutonomyPolicy.parse('[permissions]\nask = ["outputs.replace"]\n'))
    sub = await make(gate, outputs, "replace", name="week.html", content=PAGE + "<p>v3")
    assert sub.autonomy is Autonomy.CONFIRM and sub.outcome is None  # waits for a yes


@pytest.mark.parametrize(
    "name",
    [
        "../escape.html",
        "notes/week.md",
        ".hidden.md",
        "run.exe",
        "week",
        "a" * 120 + ".md",
        "/etc/passwd.txt",
    ],
)
async def test_a_bad_name_is_refused_before_any_write(world, name):
    cfg, outputs, gate = world
    with pytest.raises(OutputError):
        await make(gate, outputs, name=name, content="x")
    assert not (cfg / "out").exists() or list((cfg / "out").iterdir()) == []


async def test_too_big_or_empty_is_refused(world):
    _, outputs, gate = world
    with pytest.raises(OutputError, match="KB"):
        await make(gate, outputs, name="big.txt", content="x" * (MAX_BYTES + 1))
    with pytest.raises(OutputError, match="nothing"):
        await make(gate, outputs, name="empty.txt", content="  ")


async def test_never_through_a_link(world, tmp_path):
    cfg, outputs, gate = world
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (cfg / "out").symlink_to(elsewhere)
    target = OutputTarget(name="week.html")
    assert not outputs.writer.allowed(target)
    with pytest.raises(Exception, match="not allowed|link"):
        await make(gate, outputs, name="week.html", content=PAGE)
    assert list(elsewhere.iterdir()) == []


async def test_the_model_lists_what_is_there(world):
    _, outputs, gate = world
    await make(gate, outputs, name="week.html", content=PAGE)
    out = await outputs.read_tools()[0].handler({})
    assert out == {
        "folder": "Ghost's outputs",
        "files": [{"name": "week.html", "bytes": len(PAGE)}],
    }
