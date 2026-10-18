"""Conversations (ADR 0014): registry, name resolution, resume context, and every
operation through the gate with undo."""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.conversations import ABILITY, GHOST, SUMMARY_TEMPLATE, Conversations, slugify
from ghost_core.conversations.models import ConversationTarget, SetMemory
from ghost_core.models import ChangeSet

VOICES = ("af_heart", "am_adam", "bf_emma")


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    convs = Conversations(
        store, tmp_path / "conversations", default_voice="af_heart", voices=VOICES
    )
    gate = WriteGate(
        store,
        autonomy=AutonomyPolicy.parse(
            f"[{ABILITY}]\nset_persona = 'announce'\nreset = 'announce'\n"
            "delete = 'announce'\nforget = 'announce'\n"
        ),
    )
    gate.register(ABILITY, convs.operations())
    return store, convs, gate


async def run(gate: WriteGate, convs: Conversations, op: str, active: str = GHOST, **args):
    operation = gate.operation(ABILITY, op)
    prepared = await operation.prepare({"_active": active, **args})
    return await gate.submit(
        session_id="s1",
        ability=ABILITY,
        operation=op,
        target=prepared.target,
        changes=prepared.changes,
        rationale=prepared.rationale,
    )


def test_slugify():
    assert slugify("Accounting (EMEA)") == "accounting-emea"
    assert slugify("  Legal  ") == "legal"
    assert slugify("!!!") == "conversation"


def test_ghost_always_exists_and_lists_first(world):
    _, convs, _ = world
    assert [p.slug for p in convs.list()] == [GHOST]
    assert convs.get(GHOST).name == "Ghost"
    # idempotent
    convs.ensure_ghost()
    assert len(convs.list()) == 1


async def test_create_rename_resolve_and_undo(world):
    store, convs, gate = world
    sub = await run(gate, convs, "create", name="Accounting", aliases=["the accountants"])
    assert sub.executed and sub.outcome.status == "success"
    acc = convs.get("accounting")
    assert acc.name == "Accounting" and acc.voice == "af_heart"
    assert (convs.files.dir("accounting")).is_dir()

    assert convs.resolve("accounting").slug == "accounting"
    assert convs.resolve("the accounting team").slug == "accounting"
    assert convs.resolve("The Accountants").slug == "accounting"
    assert convs.resolve("patch me to accounting department").slug == "accounting"
    assert convs.resolve("marketing") is None

    sub = await run(
        gate, convs, "rename", conversation="accounting", name="Finance", aliases=["money"]
    )
    assert sub.outcome.status == "success"
    assert convs.get("accounting").name == "Finance"  # slug is stable
    assert convs.resolve("money people").slug == "accounting"

    out = await gate.undo("s1")
    assert out.status == "success"
    assert convs.get("accounting").name == "Accounting"
    assert convs.get("accounting").aliases == ("the accountants",)


async def test_create_duplicate_and_bad_voice_rejected(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal")
    with pytest.raises(ValueError):
        await run(gate, convs, "create", name="legal")
    with pytest.raises(ValueError):
        await run(gate, convs, "create", name="Ops", voice="zz_nobody")


async def test_set_voice_persona_and_undo(world):
    _, convs, gate = world
    sub = await run(gate, convs, "set_voice", voice="am_adam", speed=1.2)
    assert sub.outcome.status == "success"
    assert convs.get(GHOST).voice == "am_adam" and convs.get(GHOST).speed == 1.2
    sub = await run(gate, convs, "set_persona", text="You are terse and dry.")
    assert sub.outcome.status == "success"
    assert convs.files.persona(GHOST).strip() == "You are terse and dry."
    out = await gate.undo("s1")
    assert out.status == "success" and convs.files.persona(GHOST) == ""


async def test_remember_forget_and_undo(world):
    _, convs, gate = world
    await run(gate, convs, "remember", text="the books close on the fifth")
    await run(gate, convs, "remember", text="Sam owns payroll")
    assert convs.files.note_lines(GHOST) == ["the books close on the fifth", "Sam owns payroll"]
    sub = await run(gate, convs, "forget", text="payroll")
    assert sub.outcome.status == "success"
    assert convs.files.note_lines(GHOST) == ["the books close on the fifth"]
    out = await gate.undo("s1")
    assert out.status == "success"
    assert convs.files.note_lines(GHOST) == ["the books close on the fifth", "Sam owns payroll"]
    with pytest.raises(ValueError):
        await run(gate, convs, "forget", text="nothing like this")


async def test_turns_resume_compaction_and_prune(world):
    store, convs, gate = world
    convs.record(GHOST, "s1", "user", "what needs my sign-off")
    convs.record(GHOST, "s1", "call", "list_issues(repo=acme/app)")
    convs.record(GHOST, "s1", "assistant", "Two issues are waiting.")
    convs.record(GHOST, "s1", "user", "label the first one bug")
    last = convs.last_turn_id(GHOST)

    ctx = convs.resume(GHOST)
    msgs = ctx.messages()
    assert msgs[0] == {"role": "user", "content": "what needs my sign-off"}
    assert "[earlier I called list_issues" in msgs[1]["content"]
    assert msgs[-1]["role"] == "user"

    # an automatic compaction through the internal path
    out = await gate.apply_internal(
        session_id="s1",
        ability=ABILITY,
        operation="compact",
        target=ConversationTarget(slug=GHOST),
        changes=ChangeSet(
            steps=(SetMemory(text="Operator reviewed two issues.", through_turn_id=last),)
        ),
        rationale="auto",
    )
    assert out.status == "success"
    assert convs.files.memory(GHOST).strip() == "Operator reviewed two issues."
    versions = convs.compactions(GHOST)
    assert len(versions) == 1 and versions[0].through_turn_id == last
    ctx = convs.resume(GHOST)
    assert ctx.recent == []  # all covered
    assert ctx.messages()[0]["content"] == SUMMARY_TEMPLATE.format(
        summary="Operator reviewed two issues."
    )
    # the operator edits the file by hand; the next resume reads the edit, and an
    # undo of the compaction now refuses to roll over that edit (stale)
    convs.files.write(GHOST, "memory.md", "Operator reviewed two issues. Sam owes a reply.")
    assert "Sam owes" in convs.resume(GHOST).memory
    out = await gate.undo("s1")
    assert out.status == "failed" and "changed since" in out.error
    assert "Sam owes" in convs.files.memory(GHOST)

    # pruning drops only summarized turns older than the window
    assert store.prune_turns(__import__("ghost_core.models", fromlist=["utcnow"]).utcnow()) == 4
    assert convs.turns(GHOST) == []


async def test_reset_hides_turns_and_undo_restores(world):
    _, convs, gate = world
    convs.record(GHOST, "s1", "user", "hello")
    await run(gate, convs, "remember", text="keep this")
    convs.files.write(GHOST, "memory.md", "some summary")
    sub = await run(gate, convs, "reset")
    assert sub.outcome.status == "success"
    assert convs.turns(GHOST) == [] and convs.files.memory(GHOST) == ""
    assert convs.files.note_lines(GHOST) == []
    out = await gate.undo("s1")
    assert out.status == "success"
    assert [t.text for t in convs.turns(GHOST)] == ["hello"]
    assert convs.files.memory(GHOST).strip() == "some summary"
    assert convs.files.note_lines(GHOST) == ["keep this"]


async def test_delete_and_undo_restores_profile(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal", voice="bf_emma")
    await run(gate, convs, "set_persona", conversation="legal", text="Careful and formal.")
    convs.record("legal", "s1", "user", "is this contract okay")
    sub = await run(gate, convs, "delete", conversation="legal")
    assert sub.outcome.status == "success"
    assert convs.get("legal") is None and convs.resolve("legal") is None
    out = await gate.undo("s1")
    assert out.status == "success"
    p = convs.get("legal")
    assert p is not None and p.voice == "bf_emma"
    assert convs.files.persona("legal").strip() == "Careful and formal."
    assert [t.text for t in convs.turns("legal")] == ["is this contract okay"]


async def test_ghost_cannot_be_deleted_or_renamed(world):
    _, convs, gate = world
    with pytest.raises(ValueError):
        await run(gate, convs, "delete")
    sub = await run(gate, convs, "rename", name="Spectre")
    assert sub.outcome.status == "failed" and "keeps its name" in sub.outcome.error


def test_retention_window_is_honoured(tmp_path: Path):
    store = Store()
    convs = Conversations(store, tmp_path / "c", retention=timedelta(days=1))
    convs.record(GHOST, "s", "user", "fresh")
    assert convs.prune() == 0  # nothing summarized yet, nothing old
