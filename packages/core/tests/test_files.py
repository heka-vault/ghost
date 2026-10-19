from pathlib import Path

import pytest
from ghost_core import Operation, PolicyError, Prepared, Store, WriteGate
from ghost_core.targets import AppendText, FileTarget, FileWriter


def make(tmp_path: Path):
    root = tmp_path / "notes"
    root.mkdir()
    w = FileWriter(root)
    gate = WriteGate(Store())

    async def prepare(args):
        return Prepared(target=FileTarget(path=root / args["file"]), changes=args["changes"])

    gate.register(
        "notes",
        [Operation("append", "", {}, [], prepare=prepare, writer=w, risk="low", allowed=w.allowed)],
    )
    return gate, root


async def test_append_creates_file_and_announces(tmp_path):
    gate, root = make(tmp_path)
    t = FileTarget(path=root / "today.md")
    from ghost_core import ChangeSet

    sub = await gate.submit(
        session_id="s",
        ability="notes",
        operation="append",
        target=t,
        changes=ChangeSet(steps=(AppendText(text="demo is Friday"),)),
        rationale="",
    )
    assert sub.executed and (root / "today.md").read_text() == "demo is Friday\n"
    assert "ending 'demo is Friday'" in sub.outcome.readback.spoken()
    sub2 = await gate.submit(
        session_id="s",
        ability="notes",
        operation="append",
        target=t,
        changes=ChangeSet(steps=(AppendText(text="second"),)),
        rationale="",
    )
    assert sub2.executed and (root / "today.md").read_text() == "demo is Friday\nsecond\n"


async def test_outside_root_is_refused(tmp_path):
    gate, root = make(tmp_path)
    from ghost_core import ChangeSet

    with pytest.raises(PolicyError):
        await gate.submit(
            session_id="s",
            ability="notes",
            operation="append",
            target=FileTarget(path=tmp_path / "elsewhere.md"),
            changes=ChangeSet(steps=(AppendText(text="x"),)),
            rationale="",
        )


async def test_external_edit_makes_pending_append_stale(tmp_path):
    gate, root = make(tmp_path)
    t = FileTarget(path=root / "a.md")
    (root / "a.md").write_text("one\n")
    from ghost_core import ChangeSet

    await gate.prepare(
        session_id="s",
        ability="notes",
        operation="append",
        target=t,
        changes=ChangeSet(steps=(AppendText(text="two"),)),
        rationale="",
    )
    import os
    import time

    (root / "a.md").write_text("one\nsomeone else wrote this\n")
    os.utime(root / "a.md", ns=(time.time_ns() + 10**9, time.time_ns() + 10**9))
    out = await gate.confirm_pending("s")
    assert out.status == "failed" and "changed since" in out.error
    assert (root / "a.md").read_text() == "one\nsomeone else wrote this\n"
