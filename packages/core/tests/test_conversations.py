"""Conversations (ADR 0014): registry, name resolution, resume context, and every
operation through the gate."""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.conversations import (
    GHOST,
    RECENT_TEMPLATE,
    SOURCE,
    SUMMARY_TEMPLATE,
    Conversations,
    slugify,
)
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
            f"[{SOURCE}]\nset_persona = 'announce'\nreset = 'announce'\n"
            "delete = 'announce'\nforget = 'announce'\n"
        ),
    )
    gate.register(SOURCE, convs.operations())
    return store, convs, gate


async def run(gate: WriteGate, convs: Conversations, op: str, active: str = GHOST, **args):
    operation = gate.operation(SOURCE, op)
    prepared = await operation.prepare({"_active": active, **args})
    return await gate.submit(
        session_id="s1",
        ability=SOURCE,
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


async def test_create_rename_resolve(world):
    store, convs, gate = world
    sub = await run(gate, convs, "create", name="Accounting", aliases=["the accountants"])
    assert sub.executed and sub.outcome.status == "success"
    acc = convs.get("accounting")
    assert acc.name == "Accounting" and acc.voice == "am_adam"  # the first unused voice
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

    # taking it back is just another rename
    sub = await run(
        gate,
        convs,
        "rename",
        conversation="accounting",
        name="Accounting",
        aliases=["the accountants"],
    )
    assert sub.outcome.status == "success"
    assert convs.get("accounting").name == "Accounting"
    assert convs.get("accounting").aliases == ("the accountants",)


async def test_create_duplicate_and_bad_voice_rejected(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal")
    with pytest.raises(ValueError):
        await run(gate, convs, "create", name="legal")
    with pytest.raises(ValueError):
        await run(gate, convs, "create", name="Ops", voice="zz_nobody")


async def test_set_voice_and_persona(world):
    _, convs, gate = world
    sub = await run(gate, convs, "set_voice", voice="am_adam", speed=1.2)
    assert sub.outcome.status == "success"
    assert convs.get(GHOST).voice == "am_adam" and convs.get(GHOST).speed == 1.2
    sub = await run(gate, convs, "set_persona", text="You are terse and dry.")
    assert sub.outcome.status == "success"
    assert convs.files.persona(GHOST).strip() == "You are terse and dry."


async def test_remember_and_forget(world):
    _, convs, gate = world
    await run(gate, convs, "remember", text="the books close on the fifth")
    await run(gate, convs, "remember", text="Sam owns payroll")
    assert convs.files.note_lines(GHOST) == ["the books close on the fifth", "Sam owns payroll"]
    sub = await run(gate, convs, "forget", text="payroll")
    assert sub.outcome.status == "success"
    assert convs.files.note_lines(GHOST) == ["the books close on the fifth"]
    with pytest.raises(ValueError):
        await run(gate, convs, "forget", text="nothing like this")


def test_the_record_marks_where_each_session_ended(world):
    """A request that opens a session follows a marked end, not the last goodbye: run
    together, "let's work on the wake word" got "talk soon" and a hang-up (2026-10-24)."""
    _, convs, _ = world
    convs.record(GHOST, "s1", "user", "You can hang up now.")
    convs.record(GHOST, "s1", "assistant", "Glad it's working, talk soon!")
    convs.record(GHOST, "s2", "user", "What time is it?")
    record = convs.resume(GHOST).messages()[0]["content"]
    assert record.endswith(
        "Operator: You can hang up now.\nYou: Glad it's working, talk soon!\n"
        "(next session)\nOperator: What time is it?\n(end of the record)"
    )


async def test_turns_resume_compaction_and_prune(world):
    store, convs, gate = world
    convs.record(GHOST, "s1", "user", "what needs my sign-off")
    convs.record(GHOST, "s1", "call", "list_issues(repo=acme/app)")
    convs.record(GHOST, "s1", "assistant", "Two issues are waiting.")
    convs.record(GHOST, "s1", "user", "label the first one bug")
    last = convs.last_turn_id(GHOST)

    ctx = convs.resume(GHOST)
    msgs = ctx.messages()
    # one record, from the user side: never replayed as the model's own turns
    assert [m["role"] for m in msgs] == ["user"]
    record = msgs[0]["content"]
    assert record.startswith(RECENT_TEMPLATE.split("{record}")[0])
    assert "Operator: what needs my sign-off" in record
    assert "(you called list_issues(repo=acme/app))" in record
    assert "You: Two issues are waiting." in record
    assert record.endswith("Operator: label the first one bug\n(end of the record)")

    # an automatic compaction through the internal path
    out = await gate.apply_internal(
        session_id="s1",
        ability=SOURCE,
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
    # the operator edits the file by hand; the next resume reads the edit
    convs.files.write(GHOST, "memory.md", "Operator reviewed two issues. Sam owes a reply.")
    assert "Sam owes" in convs.resume(GHOST).memory

    # pruning drops only summarized turns older than the window
    assert store.prune_turns(__import__("ghost_core.models", fromlist=["utcnow"]).utcnow()) == 4
    assert convs.turns(GHOST) == []


async def test_reset_hides_turns_and_keeps_the_before_in_the_audit(world):
    _, convs, gate = world
    convs.record(GHOST, "s1", "user", "hello")
    await run(gate, convs, "remember", text="keep this")
    convs.files.write(GHOST, "memory.md", "some summary")
    sub = await run(gate, convs, "reset")
    assert sub.outcome.status == "success"
    assert convs.turns(GHOST) == [] and convs.files.memory(GHOST) == ""
    assert convs.files.note_lines(GHOST) == []
    before = sub.proposal.snapshot
    assert before.memory.strip() == "some summary" and "keep this" in before.notes


async def test_delete_hides_the_conversation(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal", voice="bf_emma")
    await run(gate, convs, "set_persona", conversation="legal", text="Careful and formal.")
    convs.record("legal", "s1", "user", "is this contract okay")
    sub = await run(gate, convs, "delete", conversation="legal")
    assert sub.outcome.status == "success"
    assert convs.get("legal") is None and convs.resolve("legal") is None
    before = sub.proposal.snapshot
    assert before.voice == "bf_emma" and before.persona.strip() == "Careful and formal."


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


# --- scope and settle (ADR 0019) -------------------------------------------------------


@pytest.fixture
def scoped(tmp_path: Path):
    store = Store()
    convs = Conversations(
        store,
        tmp_path / "conversations",
        default_voice="af_heart",
        voices=VOICES,
        scopes={"github_issues": lambda: ["acme/app", "acme/infra", "other/site"]},
    )
    gate = WriteGate(
        store, autonomy=AutonomyPolicy.parse(f"[{SOURCE}]\nset_persona = 'announce'\n")
    )
    gate.register(SOURCE, convs.operations())
    return store, convs, gate


async def test_create_with_scope_validates_against_the_allowlist(scoped):
    _, convs, gate = scoped
    sub = await run(gate, convs, "create", name="App", scope={"github_issues": ["ACME/app", ""]})
    assert sub.outcome.status == "success"
    app = convs.get("app")
    assert app.scope == {"github_issues": ("acme/app",)}  # spelled as the allowlist spells it
    assert app.scope_for("github_issues") == ("acme/app",)
    assert app.scope_for("notes") is None  # unscoped for an ability not listed
    with pytest.raises(ValueError, match="not something github_issues is allowed"):
        await run(gate, convs, "create", name="Bad", scope={"github_issues": ["acme/secret"]})
    with pytest.raises(ValueError, match="cannot be scoped"):
        await run(gate, convs, "create", name="Bad", scope={"notes": ["x"]})
    assert "github_issues" in gate.operation(SOURCE, "create").properties["scope"]["description"]


async def test_ghost_is_always_unscoped(scoped):
    _, convs, gate = scoped
    with pytest.raises(ValueError, match="Ghost is unscoped"):
        await run(gate, convs, "set_scope", ability="github_issues", items=["acme/app"])
    assert convs.get(GHOST).scope_for("github_issues") is None
    # even a row written with a scope reads back unscoped
    assert convs.validate_scope(GHOST, {}) == {}


async def test_set_scope_merges_per_ability_and_scope_of_finds_owners(scoped):
    _, convs, gate = scoped
    await run(gate, convs, "create", name="App")
    await run(gate, convs, "create", name="Infra")
    sub = await run(
        gate, convs, "set_scope", conversation="app", ability="github_issues", items=["acme/app"]
    )
    assert sub.outcome.status == "success" and "acme/app" in sub.outcome.summary()
    await run(
        gate, convs, "set_scope", conversation="infra", ability="github_issues", items="acme/infra"
    )
    assert [p.slug for p in convs.scope_of("github_issues", "acme/app")] == ["app"]
    assert [p.slug for p in convs.scope_of("github_issues", "ACME/INFRA")] == ["infra"]
    assert convs.scope_of("github_issues", "other/site") == []
    # an empty list drops the ability: the thread covers everything again
    await run(gate, convs, "set_scope", conversation="app", ability="github_issues", items=[])
    assert convs.get("app").scope == {} and convs.scope_of("github_issues", "acme/app") == []


async def test_scope_survives_a_reload_and_old_rows_load_unscoped(tmp_path: Path):
    db = tmp_path / "ghost.db"
    store = Store(db)
    # an older row without the new columns' values
    store._conn.execute(
        "INSERT INTO conversations (slug, name, aliases, voice, speed, created_at, updated_at)"
        " VALUES ('legacy', 'Legacy', '[]', 'af_heart', 1.0, '2026-01-01T00:00:00+00:00',"
        " '2026-01-01T00:00:00+00:00')"
    )
    convs = Conversations(
        store, tmp_path / "c", scopes={"github_issues": lambda: ["acme/app"]}, voices=VOICES
    )
    assert convs.get("legacy").scope == {} and not convs.get("legacy").settled
    gate = WriteGate(store)
    gate.register(SOURCE, convs.operations())
    await run(gate, convs, "create", name="App", scope={"github_issues": ["acme/app"]})
    store.close()
    again = Conversations(Store(db), tmp_path / "c", voices=VOICES)
    assert again.get("app").scope == {"github_issues": ("acme/app",)}


async def test_settle_hides_from_resolve_and_the_default_list_but_stays_readable(scoped):
    _, convs, gate = scoped
    await run(gate, convs, "create", name="Heka New", voice="bf_emma")
    convs.record("heka-new", "s1", "user", "we shipped the thing")
    convs.files.write("heka-new", "memory.md", "Shipped v1; v2 planning next.")
    with pytest.raises(ValueError, match="cannot be settled"):
        await run(gate, convs, "settle")
    sub = await run(gate, convs, "settle", conversation="heka new")
    assert sub.outcome.status == "success" and sub.autonomy.value == "announce"
    p = convs.get("heka-new")
    assert p.settled and p.settled_at is not None
    assert convs.resolve("heka new") is None  # out of voice routing
    assert [q.slug for q in convs.list()] == [GHOST]
    assert [q.slug for q in convs.list(include_settled=True)] == [GHOST, "heka-new"]
    assert convs.turns("heka-new")[0].text == "we shipped the thing"  # readable
    assert convs.files.memory("heka-new").strip() == "Shipped v1; v2 planning next."
    with pytest.raises(ValueError, match="no conversation called"):  # settled: not routable
        await run(gate, convs, "settle", conversation="heka new")

    # a new thread for the same work is seeded from the settled one's memory
    sub = await run(gate, convs, "create", name="Heka Workshop", seed_from="heka-new")
    assert sub.outcome.status == "success" and "handoff" in sub.proposal.changes.describe()
    assert convs.files.memory("heka-workshop").strip() == "Shipped v1; v2 planning next."
    assert convs.compactions("heka-workshop")[0].source == "manual"
    assert convs.get("heka-workshop").voice != "bf_emma"  # picks an unused voice
    with pytest.raises(ValueError, match="private"):
        await run(gate, convs, "create", name="Nope", seed_from=GHOST)
    with pytest.raises(ValueError, match="no thread"):
        await run(gate, convs, "create", name="Nope", seed_from="zzz")

    # and it can come back; unsettle finds it by name even though resolve() would not
    sub = await run(gate, convs, "unsettle", conversation="heka new")
    assert sub.outcome.status == "success"
    assert convs.resolve("heka new").slug == "heka-new"
    with pytest.raises(ValueError, match="not settled"):
        await run(gate, convs, "unsettle", conversation="heka new")


def test_unused_voice_prefers_one_nobody_uses(scoped):
    _, convs, _ = scoped
    assert convs.unused_voice() == "am_adam"  # Ghost has af_heart
    bare = Conversations(Store(), Path("/tmp/none"), default_voice="af_heart")
    assert bare.unused_voice() == "af_heart"  # no voice list: the default


def test_list_conversations_tool_shows_scope_and_hides_settled(scoped):
    import asyncio

    _, convs, gate = scoped
    asyncio.run(run(gate, convs, "create", name="App", scope={"github_issues": ["acme/app"]}))
    asyncio.run(run(gate, convs, "create", name="Old"))
    asyncio.run(run(gate, convs, "settle", conversation="old"))
    tool = convs.read_tools()[0]
    out = asyncio.run(tool.handler({}))
    rows = {c["slug"]: c for c in out["conversations"]}
    assert set(rows) == {GHOST, "app"}
    assert rows[GHOST]["scope"] == "everything"
    assert rows["app"]["scope"] == {"github_issues": ["acme/app"]}
