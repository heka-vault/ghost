"""Ghost's rules as a tool source (ADR 0035): ``rules.add`` appends the operator's words to
AGENTS.md, verbatim, through the gate."""

from __future__ import annotations

from pathlib import Path

import pytest
from ghost_core import Autonomy, AutonomyPolicy, Store, WriteGate
from ghost_core.rules import SOURCE, AddRule, Rules, RulesTarget


@pytest.fixture
def world(tmp_path: Path):
    cfg = tmp_path / "ghost"
    cfg.mkdir()
    (cfg / "AGENTS.md").write_text("# Ghost\n\nBe warm.\n")
    rules = Rules(cfg)
    # ADR 0034: everything runs unless a rule asks first; this world asks for a rule.
    gate = WriteGate(
        Store(), autonomy=AutonomyPolicy.parse(f"[permissions]\nask = ['{SOURCE}.add']\n")
    )
    gate.register(SOURCE, rules.operations())
    return cfg, rules, gate


async def submit(gate, rules, **args):
    prepared = await rules.operations()[0].prepare(args)
    return await gate.submit(
        session_id="s1",
        ability=SOURCE,
        operation="add",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
        asked_by=prepared.asked_by,
    )


def test_one_operation_and_no_words_that_ask_for_it(world):
    _, rules, _ = world
    (op,) = rules.operations()
    assert op.name == "add" and op.asked_by == () and op.required == ["text"]
    assert set(op.properties) == {"text"}  # no department, no file: Ghost's AGENTS.md only


async def test_by_default_a_rule_is_added_and_announced(world, tmp_path):
    cfg, rules, _ = world
    gate = WriteGate(Store())
    gate.register(SOURCE, rules.operations())
    sub = await submit(gate, rules, text="Be brief.")
    assert sub.autonomy is Autonomy.ANNOUNCE and sub.outcome.status == "success"
    assert (cfg / "AGENTS.md").read_text() == "# Ghost\n\nBe warm.\n\nBe brief.\n"
    assert rules.text().endswith("Be brief.\n")


async def test_a_rule_is_appended_verbatim_at_confirm_after_a_spoken_yes(world):
    cfg, rules, gate = world
    sub = await submit(gate, rules, text="Always use UK dates.")
    assert sub.autonomy is Autonomy.CONFIRM and sub.outcome is None  # the ask rule
    assert sub.proposal.preview == "On Ghost's rules: add the rule: Always use UK dates.."
    # The request is never the yes here: the text the model wrote is read back, and the
    # operator's yes to that exact text is what writes it.
    assert await gate.confirm_by_request("s1", "ghost, add a rule: always use UK dates") is None
    _, out = await gate.on_user_turn("s1", "yes")
    assert out is not None and out.status == "success"
    assert (cfg / "AGENTS.md").read_text() == "# Ghost\n\nBe warm.\n\nAlways use UK dates.\n"


async def test_by_voice_the_rule_is_the_operators_words(world):
    _, rules, gate = world
    with pytest.raises(ValueError, match="operator's words"):
        await submit(
            gate,
            rules,
            text="Use British dates.",
            _request="ghost, add a rule: always use UK dates",
        )
    sub = await submit(
        gate, rules, text="Always use UK dates", _request="Ghost, add a rule: Always use UK dates!"
    )
    assert sub.proposal.preview.endswith("Always use UK dates.")
    with pytest.raises(ValueError, match="required"):
        await submit(gate, rules, text="  ")
    with pytest.raises(ValueError, match="too long"):
        await submit(gate, rules, text="x" * 1001)


async def test_a_fresh_agents_md_gets_a_heading(world):
    cfg, rules, gate = world
    (cfg / "AGENTS.md").unlink()
    await submit(gate, rules, text="Start again.")
    await gate.confirm_pending("s1")
    assert (cfg / "AGENTS.md").read_text() == "# Ghost\n\nStart again.\n"


async def test_never_written_through_a_link(tmp_path):
    shared = tmp_path / "CLAUDE.md"
    shared.write_text("# Project rules\n")
    cfg = tmp_path / "ghost"
    cfg.mkdir()
    (cfg / "AGENTS.md").symlink_to(shared)
    rules = Rules(cfg)
    with pytest.raises(ValueError, match="a link"):
        await rules.writer.apply(RulesTarget(), AddRule(text="Always use UK dates."), "k")
    assert shared.read_text() == "# Project rules\n"
    assert rules.text() == ""  # and not read through one either
