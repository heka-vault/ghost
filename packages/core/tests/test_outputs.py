"""What Ghost makes (ADR 0033): a file in out/, through the gate, never run."""

from __future__ import annotations

import json
import os
import stat
from pathlib import Path

import pytest
from ghost_core import Autonomy, Store, WriteGate
from ghost_core.conversations import Conversations
from ghost_core.departments import Departments
from ghost_core.outputs import MAX_BYTES, SOURCE, OutputError, Outputs, OutputTarget, list_outputs

PAGE = "<!doctype html><title>Week</title><h1>Week in review</h1>"


@pytest.fixture
def world(tmp_path: Path):
    cfg = tmp_path / "ghost"
    (cfg / "departments" / "marketing").mkdir(parents=True)
    (cfg / "departments" / "marketing" / "department.json").write_text(
        json.dumps({"title": "Marketing"})
    )
    departments = Departments(cfg)
    store = Store()
    convs = Conversations(store, departments)
    outputs = Outputs(departments, convs)
    gate = WriteGate(store)
    gate.register(SOURCE, outputs.operations())
    return cfg, departments, outputs, gate


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
    cfg, departments, outputs, gate = world
    sub = await make(gate, outputs, name="week.html", content=PAGE, _active="ghost")
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"
    path = cfg / "out" / "week.html"
    assert path.read_text() == PAGE
    assert not os.stat(path).st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    assert (
        sub.outcome.readback.exists
        and "week.html in Ghost's outputs" in sub.proposal.target.spoken()
    )
    assert [f["name"] for f in list_outputs(departments.ghost)] == ["week.html"]


async def test_a_department_writes_into_its_own_out_folder(world):
    cfg, _, outputs, gate = world
    sub = await make(gate, outputs, name="plan.md", content="# Plan\n", _active="marketing")
    assert (cfg / "departments" / "marketing" / "out" / "plan.md").is_file()
    assert sub.proposal.target.scope() == "marketing"


async def test_replacing_asks_first(world):
    _, _, outputs, gate = world
    await make(gate, outputs, name="week.html", content=PAGE, _active="ghost")
    with pytest.raises(OutputError, match="replace"):
        await make(gate, outputs, name="week.html", content=PAGE, _active="ghost")
    sub = await make(
        gate, outputs, "replace", name="week.html", content=PAGE + "<p>v2", _active="ghost"
    )
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
    cfg, _, outputs, gate = world
    with pytest.raises(OutputError):
        await make(gate, outputs, name=name, content="x", _active="ghost")
    assert not (cfg / "out").exists() or list((cfg / "out").iterdir()) == []


async def test_too_big_or_empty_is_refused(world):
    _, _, outputs, gate = world
    with pytest.raises(OutputError, match="KB"):
        await make(gate, outputs, name="big.txt", content="x" * (MAX_BYTES + 1), _active="ghost")
    with pytest.raises(OutputError, match="nothing"):
        await make(gate, outputs, name="empty.txt", content="  ", _active="ghost")


async def test_never_through_a_link(world, tmp_path):
    cfg, _, outputs, gate = world
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (cfg / "out").symlink_to(elsewhere)
    target = OutputTarget(department="ghost", name="week.html")
    assert not outputs.writer.allowed(target)
    with pytest.raises(Exception, match="not allowed|link"):
        await make(gate, outputs, name="week.html", content=PAGE, _active="ghost")
    assert list(elsewhere.iterdir()) == []


async def test_the_model_lists_what_is_there(world):
    _, _, outputs, gate = world
    await make(gate, outputs, name="week.html", content=PAGE, _active="ghost")
    out = await outputs.read_tools()[0].handler({"_active": "ghost"})
    assert out == {
        "folder": "Ghost's outputs",
        "files": [{"name": "week.html", "bytes": len(PAGE)}],
    }
