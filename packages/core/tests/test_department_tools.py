"""The departments as a tool source (ADR 0024): a rule appended verbatim, a shared fact,
the docs and the journal."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy, Store, WriteGate
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.conversations import Conversations
from ghost_core.department_tools import SOURCE, DepartmentTools
from ghost_core.departments import Departments


@pytest.fixture
def world(tmp_path: Path):
    cfg = tmp_path / "ghost"
    (cfg / "departments" / "marketing").mkdir(parents=True)
    (cfg / "departments" / "marketing" / "department.json").write_text(
        json.dumps({"title": "Marketing", "aliases": ["the marketers"]})
    )
    (cfg / "departments" / "marketing" / "AGENTS.md").write_text("# Marketing\n\nBe warm.\n")
    (cfg / "departments" / "marketing" / "docs").mkdir()
    (cfg / "departments" / "marketing" / "docs" / "voice.md").write_text("Never salesy.\n")
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "AGENTS.md").write_text("# Heka\n")
    (cfg / "departments" / "heka").mkdir()
    (cfg / "departments" / "heka" / "department.json").write_text(json.dumps({"root": str(repo)}))
    departments = Departments(cfg)
    store = Store()
    convs = Conversations(store, departments)
    tools = DepartmentTools(departments, store, convs)
    # ADR 0034: everything runs unless a rule asks first; this world asks for a rule.
    gate = WriteGate(
        store, autonomy=AutonomyPolicy.parse(f"[permissions]\nask = ['{SOURCE}.add_rule']\n")
    )
    gate.register(SOURCE, tools.operations())
    gate.register(CONVERSATIONS, convs.operations())
    return cfg, departments, store, convs, tools, gate


async def submit(gate, tools, op, **args):
    operation = next(o for o in tools.operations() if o.name == op)
    prepared = await operation.prepare(args)
    return await gate.submit(
        session_id="s1",
        ability=SOURCE,
        operation=op,
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
        conversation=args.get("_active"),
        department="marketing",
    )


async def test_by_default_a_rule_is_added_and_announced(world):
    cfg, departments, store, convs, tools, _ = world
    gate = WriteGate(store)
    gate.register(SOURCE, tools.operations())
    sub = await submit(gate, tools, "add_rule", department="marketing", text="Be brief.")
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"
    assert (cfg / "departments" / "marketing" / "AGENTS.md").read_text().endswith("Be brief.\n")


async def test_add_rule_appends_verbatim_at_confirm_after_a_spoken_yes(world):
    cfg, departments, store, convs, tools, gate = world
    sub = await submit(
        gate, tools, "add_rule", department="the marketers", text="Always use UK dates."
    )
    assert sub.autonomy is Autonomy.CONFIRM and sub.outcome is None  # the ask rule
    assert sub.proposal.preview == "On Marketing: add the rule: Always use UK dates.."
    # The request is never the yes here: the text the model wrote is read back, and the
    # operator's yes to that exact text is what writes it (nothing in code can tell a
    # paraphrase from the operator's words).
    out = await gate.confirm_by_request("s1", "ghost, add a rule to marketing: always use UK dates")
    assert out is None
    _, out = await gate.on_user_turn("s1", "yes")
    assert out is not None and out.status == "success"
    rules = (cfg / "departments" / "marketing" / "AGENTS.md").read_text()
    assert rules == "# Marketing\n\nBe warm.\n\nAlways use UK dates.\n"
    assert (
        departments.get("marketing").rules().endswith("Always use UK dates.\n…")
        or "UK dates" in departments.get("marketing").rules()
    )
    # the active agent's department when none is named
    sub = await submit(gate, tools, "add_rule", _active="marketing", text="Say hi.")
    assert sub.proposal.target.spoken() == "Marketing"
    # by voice the rule must be what the operator said, word for word
    with pytest.raises(ValueError, match="operator's words"):
        await submit(
            gate,
            tools,
            "add_rule",
            _active="marketing",
            text="Use British dates.",
            _request="ghost, add a rule to marketing: always use UK dates",
        )
    sub = await submit(
        gate,
        tools,
        "add_rule",
        _active="marketing",
        text="Always use UK dates",
        _request="Ghost, add a rule to marketing: Always use UK dates!",
    )
    assert sub.proposal.preview.endswith("Always use UK dates.")
    # a department whose rules live in a project folder is not written to
    with pytest.raises(ValueError, match="does not write there"):
        await submit(gate, tools, "add_rule", department="heka", text="x")
    with pytest.raises(ValueError, match="no department called"):
        await submit(gate, tools, "add_rule", department="sales", text="x")
    # a fresh AGENTS.md gets a heading first
    (cfg / "departments" / "marketing" / "AGENTS.md").unlink()
    departments.refresh()
    sub = await submit(gate, tools, "add_rule", department="marketing", text="Start again.")
    await gate.confirm_pending("s1")
    assert (
        cfg / "departments" / "marketing" / "AGENTS.md"
    ).read_text() == "# Marketing\n\nStart again.\n"


async def test_remember_is_the_departments_shared_memory(world):
    cfg, departments, store, convs, tools, gate = world
    sub = await submit(gate, tools, "remember", department="marketing", text="The  client is Acme.")
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"
    sub = await submit(gate, tools, "remember", department="marketing", text="FY ends in March.")
    assert sub.outcome.status == "success"
    memory = (cfg / "departments" / "marketing" / "memory.md").read_text()
    assert memory == "The client is Acme.\nFY ends in March.\n"
    assert departments.get("marketing").memory() == memory  # what every agent's prompt carries
    assert departments.get("ghost").memory() == ""  # not Ghost's
    assert "2 shared fact" in sub.outcome.readback.spoken()
    with pytest.raises(ValueError, match="nothing to remember"):
        await submit(gate, tools, "remember", department="marketing", text="  ")


async def test_docs_and_journal_are_reads(world):
    cfg, departments, store, convs, tools, gate = world
    docs = next(t for t in tools.read_tools() if t.name == "read_department_doc")
    assert await docs.handler({"department": "marketing"}) == {
        "department": "Marketing",
        "docs": ["voice.md"],
    }
    out = await docs.handler({"department": "marketing", "file": "voice.md"})
    assert out["text"] == "Never salesy.\n"
    assert "error" in await docs.handler({"department": "marketing", "file": "../AGENTS.md"})
    # the journal: executed writes in the department, by agent
    create = gate.operation(CONVERSATIONS, "create")
    prepared = await create.prepare({"name": "Brand", "department": "marketing"})
    await gate.submit(
        session_id="s1",
        ability=CONVERSATIONS,
        operation="create",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
        conversation="marketing",
        department="marketing",
    )
    await submit(gate, tools, "remember", department="marketing", text="A fact.", _active="brand")
    journal = next(t for t in tools.read_tools() if t.name == "department_journal")
    out = await journal.handler({"department": "marketing"})
    assert [e["operation"] for e in out["journal"]] == [
        "departments.remember",
        "conversations.create",
    ]
    assert out["journal"][0]["agent"] == "Brand" and out["journal"][1]["agent"] == "Marketing"
    assert "A fact" in out["journal"][0]["what"]
    assert (await journal.handler({"department": "ghost"}))["journal"] == []
    # a pending or cancelled proposal is not in the journal
    await submit(gate, tools, "add_rule", department="marketing", text="Pending.")
    assert len((await journal.handler({"department": "marketing"}))["journal"]) == 2
    assert [e["operation"] for e in tools.journal("marketing", limit=1)] == ["departments.remember"]
