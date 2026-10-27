"""Agents (ADR 0014, ADR 0024): the registry read from the department folders, name
resolution, resume context, and every operation through the gate."""

from __future__ import annotations

import json
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
from ghost_core.departments import Departments
from ghost_core.models import ChangeSet

VOICES = ("af_heart", "am_adam", "bf_emma")


def layout(tmp_path: Path, *departments: str) -> Departments:
    cfg = tmp_path / "ghost"
    cfg.mkdir(parents=True, exist_ok=True)
    for name in departments:
        folder = cfg / "departments" / name
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "department.json").write_text(
            json.dumps({"title": name.replace("-", " ").title(), "voice": "bm_george"})
        )
        (folder / "AGENTS.md").write_text(f"# {name.title()}: the {name} desk\n\nUK dates.\n")
    return Departments(cfg)


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    convs = Conversations(
        store, layout(tmp_path, "marketing"), default_voice="af_heart", voices=VOICES
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
    assert slugify("Brand design (EMEA)") == "brand-design-emea"
    assert slugify("  Legal  ") == "legal"
    assert slugify("!!!") == "agent"


def test_ghost_and_each_department_get_their_own_agent(world):
    _, convs, _ = world
    assert [p.slug for p in convs.list()] == [GHOST, "marketing"]
    ghost = convs.get(GHOST)
    assert ghost.name == "Ghost" and ghost.is_ghost and ghost.is_default
    assert ghost.department == GHOST and ghost.voice is None
    m = convs.get("marketing")
    assert m.name == "Marketing" and m.is_default and m.department == "marketing"
    assert convs.voice_for(m) == ("bm_george", 1.0)  # the department's voice
    assert convs.voice_for(ghost) == ("af_heart", 1.0)
    assert (convs.files.dir(GHOST) / "agent.json").is_file()
    assert convs.files.dir("marketing").parent == convs.departments.get("marketing").agents_dir
    # idempotent
    convs.sync()
    assert len(convs.list()) == 2


async def test_hire_rename_resolve(world):
    store, convs, gate = world
    sub = await run(
        gate,
        convs,
        "create",
        name="Brand",
        aliases=["the brand agent"],
        department="marketing",
        job="brand design",
        persona="Warm, visual, never salesy.",
    )
    assert sub.executed and sub.outcome.status == "success"
    brand = convs.get("brand")
    assert brand.department == "marketing" and brand.job == "brand design"
    assert brand.voice is None and convs.voice_for(brand)[0] == "bm_george"  # inherited
    folder = convs.departments.get("marketing").agents_dir / "brand"
    assert folder.is_dir() and (folder / "persona.md").read_text().startswith("Warm")
    data = json.loads((folder / "agent.json").read_text())
    assert data["id"] == brand.id and data["job"] == "brand design" and "voice" not in data

    assert convs.resolve("brand").slug == "brand"
    assert convs.resolve("the brand agent").slug == "brand"
    assert convs.resolve("transfer me to marketing").slug == "marketing"
    assert convs.resolve("legal") is None
    assert [p.slug for p in convs.agents_of("marketing")] == ["brand", "marketing"]

    sub = await run(
        gate,
        convs,
        "rename",
        conversation="brand",
        name="Brand Studio",
        aliases=["studio"],
        job="the look",
    )
    assert sub.outcome.status == "success"
    again = convs.get("brand")
    assert again.name == "Brand Studio" and again.job == "the look"  # slug is stable
    assert json.loads((folder / "agent.json").read_text())["name"] == "Brand Studio"
    assert convs.resolve("studio people").slug == "brand"
    # a department's own agent keeps the department's name
    sub = await run(gate, convs, "rename", conversation="marketing", name="Ads")
    assert sub.outcome.status == "failed" and "named for its department" in sub.outcome.error
    # hired into the active agent's department when none is named
    sub = await run(gate, convs, "create", active="brand", name="Social", job="social media")
    assert sub.outcome.status == "success" and convs.get("social").department == "marketing"


async def test_hire_rejects_duplicates_department_names_and_bad_voices(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal")
    assert convs.get("legal").department == GHOST
    with pytest.raises(ValueError, match="already an agent"):
        await run(gate, convs, "create", name="legal")
    with pytest.raises(ValueError, match="already an agent"):  # the department's own agent
        await run(gate, convs, "create", name="Marketing")
    with pytest.raises(ValueError, match="not an installed voice"):
        await run(gate, convs, "create", name="Ops", voice="zz_nobody")
    with pytest.raises(ValueError, match="no department called"):
        await run(gate, convs, "create", name="Ops", department="sales")


async def test_set_voice_and_persona(world):
    _, convs, gate = world
    sub = await run(gate, convs, "set_voice", voice="am_adam", speed=1.2)
    assert sub.outcome.status == "success"
    assert convs.get(GHOST).voice == "am_adam" and convs.get(GHOST).speed == 1.2
    assert convs.voice_for(convs.get(GHOST)) == ("am_adam", 1.2)
    sub = await run(gate, convs, "set_voice", voice=None)  # back to the department's
    assert sub.outcome.status == "success" and convs.get(GHOST).voice is None
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
    # transcripts are keyed by the agent's id, not its slug
    assert ctx.recent[0].conversation == convs.get(GHOST).id

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


async def test_delete_removes_the_folder_and_hides_the_agent(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Legal", voice="bf_emma")
    await run(gate, convs, "set_persona", conversation="legal", text="Careful and formal.")
    convs.record("legal", "s1", "user", "is this contract okay")
    folder = convs.files.dir("legal")
    sub = await run(gate, convs, "delete", conversation="legal")
    assert sub.outcome.status == "success"
    assert convs.get("legal") is None and convs.resolve("legal") is None
    assert not folder.exists()
    before = sub.proposal.snapshot
    assert before.voice == "bf_emma" and before.persona.strip() == "Careful and formal."


async def test_a_departments_own_agent_cannot_be_deleted_retired_or_renamed(world):
    _, convs, gate = world
    with pytest.raises(ValueError, match="cannot be deleted"):
        await run(gate, convs, "delete")
    with pytest.raises(ValueError, match="cannot be deleted"):
        await run(gate, convs, "delete", conversation="marketing")
    with pytest.raises(ValueError, match="stays"):
        await run(gate, convs, "retire", conversation="marketing")
    sub = await run(gate, convs, "rename", name="Spectre")
    assert sub.outcome.status == "failed" and "keeps its name" in sub.outcome.error


def test_retention_window_is_honoured(tmp_path: Path):
    store = Store()
    convs = Conversations(store, layout(tmp_path), retention=timedelta(days=1))
    convs.record(GHOST, "s", "user", "fresh")
    assert convs.prune() == 0  # nothing summarized yet, nothing old


# --- the folders are the registry ---------------------------------------------------------


def test_sync_reads_folders_and_a_moved_folder_keeps_its_history(tmp_path: Path):
    deps = layout(tmp_path, "marketing", "sales")
    store = Store()
    convs = Conversations(store, deps, voices=VOICES)
    # the operator makes an agent by hand: a folder with an agent.json
    folder = deps.get("marketing").agents_dir / "brand"
    folder.mkdir()
    (folder / "agent.json").write_text(json.dumps({"name": "Brand", "job": "the look"}))
    (deps.get("marketing").agents_dir / "junk").mkdir()  # no agent.json: not an agent
    problems = convs.sync()
    brand = convs.get("brand")
    assert brand is not None and brand.department == "marketing" and brand.job == "the look"
    assert json.loads((folder / "agent.json").read_text())["id"] == brand.id  # given an id
    assert any("junk" in p for p in problems)
    convs.record("brand", "s1", "user", "the logo")
    # moved by hand into sales: same id, same transcript
    folder.rename(deps.get("sales").agents_dir / "brand")
    convs.sync()
    moved = convs.get("brand")
    assert moved.id == brand.id and moved.department == "sales"
    assert [t.text for t in convs.turns("brand")] == ["the logo"]
    assert convs.files.dir("brand") == deps.get("sales").agents_dir / "brand"
    # gone from disk: marked deleted, its history hidden from the roster
    (deps.get("sales").agents_dir / "brand" / "agent.json").unlink()
    (deps.get("sales").agents_dir / "brand").rmdir()
    convs.sync()
    assert convs.get("brand") is None


def test_a_reopened_store_keeps_timestamps_and_retired_state(tmp_path: Path):
    db = tmp_path / "ghost.db"
    deps = layout(tmp_path, "marketing")
    convs = Conversations(Store(db), deps, voices=VOICES)
    ops = {o.name: o for o in convs.operations()}
    import asyncio

    prep = asyncio.run(ops["create"].prepare({"name": "Brand", "department": "marketing"}))
    asyncio.run(convs.writer.apply(prep.target, prep.changes.steps[0], "k"))
    prep = asyncio.run(ops["retire"].prepare({"conversation": "brand"}))
    asyncio.run(convs.writer.apply(prep.target, prep.changes.steps[0], "k"))
    assert convs.get("brand").retired
    again = Conversations(Store(db), deps, voices=VOICES)
    assert again.get("brand").retired and again.resolve("brand") is None
    assert [p.slug for p in again.list(include_retired=True)] == [GHOST, "brand", "marketing"]


def test_a_pre_0024_database_is_moved_aside(tmp_path: Path):
    db = tmp_path / "ghost.db"
    import sqlite3

    conn = sqlite3.connect(db)
    conn.executescript(
        "CREATE TABLE conversations (slug TEXT PRIMARY KEY, name TEXT NOT NULL, voice TEXT);"
        "CREATE TABLE turns (id INTEGER PRIMARY KEY, conversation TEXT);"
        "CREATE TABLE compactions (id TEXT PRIMARY KEY, conversation TEXT);"
        "INSERT INTO conversations VALUES ('ghost', 'Ghost', 'af_heart');"
    )
    conn.commit()
    conn.close()
    store = Store(db)
    tables = {
        r[0] for r in store._conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
    }  # noqa: SLF001
    assert any(t.startswith("retired_conversations_") for t in tables)
    assert store.list_conversations() == []  # the new layout starts empty


# --- retire, rehire and the handoff (ADR 0019, ADR 0024) ------------------------------------


async def test_retire_hides_from_resolve_and_the_default_list_but_stays_readable(world):
    _, convs, gate = world
    await run(gate, convs, "create", name="Heka New", voice="bf_emma")
    convs.record("heka-new", "s1", "user", "we shipped the thing")
    convs.files.write("heka-new", "memory.md", "Shipped v1; v2 planning next.")
    with pytest.raises(ValueError, match="stays"):
        await run(gate, convs, "retire")
    sub = await run(gate, convs, "retire", conversation="heka new")
    assert sub.outcome.status == "success" and sub.autonomy.value == "announce"
    p = convs.get("heka-new")
    assert p.retired and p.retired_at is not None
    assert convs.resolve("heka new") is None  # out of voice routing
    assert [q.slug for q in convs.list()] == [GHOST, "marketing"]
    assert [q.slug for q in convs.list(include_retired=True)] == [GHOST, "heka-new", "marketing"]
    assert convs.turns("heka-new")[0].text == "we shipped the thing"  # readable
    assert convs.files.memory("heka-new").strip() == "Shipped v1; v2 planning next."
    with pytest.raises(ValueError, match="no agent called"):  # retired: not routable
        await run(gate, convs, "retire", conversation="heka new")

    # a new agent for the same work is seeded from the retired one's memory
    sub = await run(gate, convs, "create", name="Heka Workshop", seed_from="heka-new")
    assert sub.outcome.status == "success" and "handoff" in sub.proposal.changes.describe()
    assert convs.files.memory("heka-workshop").strip() == "Shipped v1; v2 planning next."
    assert convs.compactions("heka-workshop")[0].source == "manual"
    with pytest.raises(ValueError, match="private"):
        await run(gate, convs, "create", name="Nope", seed_from=GHOST)
    with pytest.raises(ValueError, match="no agent"):
        await run(gate, convs, "create", name="Nope", seed_from="zzz")

    # and it can come back; rehire finds it by name even though resolve() would not
    sub = await run(gate, convs, "rehire", conversation="heka new")
    assert sub.outcome.status == "success"
    assert convs.resolve("heka new").slug == "heka-new"
    with pytest.raises(ValueError, match="not retired"):
        await run(gate, convs, "rehire", conversation="heka new")


def test_unused_voice_prefers_one_nobody_uses(world):
    _, convs, _ = world
    assert convs.unused_voice() == "am_adam"  # the default and marketing's are taken
    bare = Conversations(Store(), Departments(Path("/tmp/none")), default_voice="af_heart")
    assert bare.unused_voice() == "af_heart"  # no voice list: the default


def test_list_agents_tool_groups_by_department_and_hides_retired(world):
    import asyncio

    _, convs, gate = world
    asyncio.run(run(gate, convs, "create", name="Brand", department="marketing", job="the look"))
    asyncio.run(run(gate, convs, "create", name="Old"))
    asyncio.run(run(gate, convs, "retire", conversation="old"))
    tool = convs.read_tools()[0]
    out = asyncio.run(tool.handler({}))
    by = {d["slug"]: d for d in out["departments"]}
    assert set(by) == {GHOST, "marketing"}
    assert [a["slug"] for a in by[GHOST]["agents"]] == [GHOST]
    assert by["marketing"]["about"] == "Marketing: the marketing desk"
    assert [(a["slug"], a["job"]) for a in by["marketing"]["agents"]] == [
        ("brand", "the look"),
        ("marketing", None),
    ]
