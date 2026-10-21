"""What the ADR 0023 QA review found, pinned down."""

from __future__ import annotations

from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Store
from ghost_core.conversations import Conversations
from ghost_core.mcp import source_of
from ghost_core.skills import Skills
from ghost_core.toolkits import Grant, ToolkitSpec, merge
from test_mcp_writes import POLICY, connected, spec


@pytest.mark.parametrize("items", [[], ["Acme/Ap"]])
async def test_a_grant_narrowed_to_nothing_reaches_nothing(items):
    async with connected(spec()) as (server, _):
        view = source_of(server, Grant(server="github", items=tuple(items)), "t")
        labels = next(o for o in view.operations if o.name == "set_labels")
        for repo in ("app", "web"):
            with pytest.raises(ValueError, match="not an allowed repository"):
                await labels.prepare(
                    {"owner": "acme", "repo": repo, "issue_number": 1, "labels": ["x"]}
                )
    p = AutonomyPolicy.parse(POLICY + "[toolkit.t]\n[toolkit.t.servers.github]\nitems = []\n")
    assert any("items is empty" in m for m in p.check_toolkits())


def test_nothing_in_a_toolkit_can_break_out_of_its_string():
    evil = 'x"\n[mcp.evil]\ncommand = "sh"\n'
    p = AutonomyPolicy.parse(POLICY)
    p = AutonomyPolicy.from_plain(
        default=None,
        skills={},
        targets={},
        mcp=p.mcp,
        toolkits={
            "t": ToolkitSpec(
                name="t", description=evil, grants={"github": Grant(server="github", items=(evil,))}
            )
        },
    )
    again = AutonomyPolicy.parse(p.to_toml())
    assert "evil" not in again.mcp and again.toolkits["t"].description == evil
    with pytest.raises(ValueError, match="is not a name"):
        AutonomyPolicy.parse('[toolkit."a b"]\n')


def test_merge_is_the_union_with_the_most_cautious_level():
    a = Grant(server="s", reads=("r1",), writes=("w1",), items=("x",), levels={"w1": "announce"})
    b = Grant(server="s", reads=("r2",), writes=None, items=("y",), levels={"w1": "confirm"})
    m = merge([a, b])
    assert m.reads == ("r1", "r2") and m.writes is None and m.items == ("x", "y")
    assert m.levels == {"w1": "confirm"}


async def test_a_department_with_lines_cannot_be_deleted(tmp_path: Path):
    convs = Conversations(Store(), tmp_path / "c", default_voice="af_heart")
    ops = {o.name: o for o in convs.operations()}
    for args in ({"name": "Accounting"}, {"name": "Q3", "department": "accounting"}):
        prep = await ops["create"].prepare(args)
        await convs.writer.apply(prep.target, prep.changes.steps[0], "k")
    prep = await ops["delete"].prepare({"conversation": "accounting"})
    with pytest.raises(ValueError, match="still has lines"):
        await convs.writer.apply(prep.target, prep.changes.steps[0], "k")


def test_a_narrowed_skills_view_reads_only(tmp_path: Path):
    s = Skills(tmp_path / "skills")
    assert {o.name for o in s.source().operations} == {"add", "remove"}
    assert s.source(["month-close"]).operations == []
