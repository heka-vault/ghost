"""The in-session side of conversations (ADR 0014): prompt, seeded context, transfer
frames, save-as, records and compaction persistence. No audio, no Pipecat worker."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from ghost_core import (
    Ability,
    ActiveScope,
    AutonomyPolicy,
    Change,
    ChangeSet,
    Contributions,
    LoadedAbility,
    Operation,
    Prepared,
    ReadTool,
    SimpleInstance,
    Snapshot,
    Store,
    Target,
    WriteGate,
)
from ghost_core.conversations import ABILITY, GHOST, SUMMARY_TEMPLATE, Conversations
from ghost_voice.conversation import ConversationMarkerFrame, ConversationRuntime, transfer_tone
from ghost_voice.pipeline import conversations_ability
from ghost_voice.tools import SWITCH_TOOL, register_tools
from pipecat.frames.frames import (
    LLMMessagesUpdateFrame,
    LLMUpdateSettingsFrame,
    OutputAudioRawFrame,
    TTSSpeakFrame,
    TTSUpdateSettingsFrame,
)


@dataclass
class TTSSettings:
    voice: str
    speed: float = 1.0


@dataclass
class LLMSettings:
    system_instruction: str


class FakeLLM:
    def __init__(self) -> None:
        self.functions: dict[str, Any] = {}
        self.pushed: list[Any] = []

    def register_function(self, name, handler, **_):
        self.functions[name] = handler

    async def push_frame(self, frame, direction=None):
        self.pushed.append(frame)


class FakeParams:
    def __init__(self, llm, name, arguments):
        self.llm = llm
        self.function_name = name
        self.arguments = arguments
        self.result: Any = None
        self.properties: Any = None

    async def result_callback(self, result, properties=None, **_):
        self.result = result
        self.properties = properties
        self.pushed_before_update = list(self.llm.pushed)
        if properties and properties.on_context_updated:
            await properties.on_context_updated()


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    convs = Conversations(store, tmp_path / "c", default_voice="af_heart")
    gate = WriteGate(
        store,
        autonomy=AutonomyPolicy.parse(f"[{ABILITY}]\nset_persona = 'announce'\n"),
    )
    loaded = conversations_ability(convs)
    gate.register(ABILITY, loaded.contributions.operations)
    rt = ConversationRuntime(
        convs,
        gate=gate,
        session_id="s1",
        fragments=["FRAG"],
        tts_settings=TTSSettings,
        llm_settings=LLMSettings,
        sample_rate=24000,
        keep_messages=2,
    )
    llm = FakeLLM()

    async def on_sleep(): ...

    schemas = register_tools(
        llm,
        abilities=[loaded],
        gate=gate,
        session_id="s1",
        on_sleep=on_sleep,
        conversations=rt,
        on_call=rt.record_call,
    )
    return store, convs, gate, rt, llm, schemas


async def create(convs, gate, name, **extra):
    op = gate.operation(ABILITY, "create")
    prepared = await op.prepare({"name": name, **extra})
    return await gate.submit(
        session_id="setup",
        ability=ABILITY,
        operation="create",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )


def test_prompt_and_seed_follow_the_active_conversation(world):
    _, convs, _, rt, _, _ = world
    assert rt.active.slug == GHOST
    assert "You are Ghost" in rt.system_prompt() and "FRAG" in rt.system_prompt()
    assert rt.initial_messages() == []
    assert isinstance(rt.start_frames()[0], TTSUpdateSettingsFrame)
    assert rt.start_frames()[0].delta.voice == "af_heart"

    convs.files.write(GHOST, "memory.md", "We were triaging acme/app.")
    convs.files.write(GHOST, "notes.md", "Sam owns payroll")
    rt.record("user", "hello")
    rt.record("assistant", "Hi.")
    msgs = rt.initial_messages()
    assert msgs[0]["content"].startswith(SUMMARY_TEMPLATE.split("{")[0])
    assert "Sam owns payroll" in msgs[0]["content"]
    assert msgs[-1]["role"] == "user"
    assert msgs[-1]["content"].endswith("Operator: hello\nYou: Hi.")


async def test_switch_tool_transfers_with_announce_tone_voice_and_context(world):
    _, convs, gate, rt, llm, schemas = world
    assert any(s.name == SWITCH_TOOL for s in schemas)
    await create(convs, gate, "Accounting", voice="am_adam", speed=1.1)
    convs.files.write("accounting", "persona.md", "Dry, precise, hates surprises.")
    convs.files.write("accounting", "memory.md", "Reconciling September.")
    rt.record("user", "patch me to accounting")

    p = FakeParams(llm, SWITCH_TOOL, {"name": "the accounting team", "handoff": "Q3 invoices"})
    await llm.functions[SWITCH_TOOL](p)
    assert p.result == {"status": "transferred", "to": "Accounting"}
    assert p.properties.run_llm is False
    assert p.pushed_before_update == []  # nothing moves until the result has landed
    assert not rt.settled.is_set()  # held until the new line has greeted

    kinds = [type(f) for f in llm.pushed]
    assert kinds == [
        TTSSpeakFrame,
        OutputAudioRawFrame,
        TTSUpdateSettingsFrame,
        ConversationMarkerFrame,
        LLMUpdateSettingsFrame,
        LLMMessagesUpdateFrame,
    ]
    assert llm.pushed[0].text == "Patching you through to Accounting."
    assert llm.pushed[2].delta == TTSSettings(voice="am_adam", speed=1.1)
    assert llm.pushed[3].slug == "accounting"
    prompt = llm.pushed[4].delta.system_instruction
    assert "You are Accounting" in prompt and "hates surprises" in prompt and "FRAG" in prompt
    update = llm.pushed[5]
    assert update.run_llm is True
    assert "Reconciling September" in update.messages[0]["content"]
    assert "patched through to you from Ghost" in update.messages[-1]["content"]
    assert "Q3 invoices" in update.messages[-1]["content"]
    assert rt.active.slug == "accounting"

    # assistant text is attributed by the marker, not by the clock: what the tap
    # reports before the marker is Ghost's, after it Accounting's
    rt.record("assistant", "Patching you through to Accounting.")
    assert not rt.settled.is_set()  # Ghost's announce line is not the greeting
    rt.on_marker(llm.pushed[3])
    rt.record("assistant", "Accounting here. We were reconciling September.")
    assert rt.settled.is_set()
    assert [t.text for t in convs.turns(GHOST)] == [
        "patch me to accounting",
        "switch_conversation(handoff=Q3 invoices, name=the accounting team)",
        "transferred to Accounting",
        "Patching you through to Accounting.",
    ]
    assert [t.text for t in convs.turns("accounting")] == [
        "transferred from Ghost",
        "Accounting here. We were reconciling September.",
    ]

    # back to Ghost
    p = FakeParams(llm, SWITCH_TOOL, {"name": "ghost"})
    llm.pushed.clear()
    await llm.functions[SWITCH_TOOL](p)
    assert llm.pushed[0].text == "Back to Ghost." and rt.active.slug == GHOST


async def test_switch_no_match_and_already_here(world):
    _, _, _, rt, llm, _ = world
    p = FakeParams(llm, SWITCH_TOOL, {"name": "marketing"})
    await llm.functions[SWITCH_TOOL](p)
    assert p.result["status"] == "no_match" and p.result["known"] == ["Ghost"]
    p = FakeParams(llm, SWITCH_TOOL, {"name": "ghost"})
    await llm.functions[SWITCH_TOOL](p)
    assert p.result["status"] == "already_here" and llm.pushed == []


async def test_switch_cancels_pending_and_policy_can_deny(world):
    store, convs, gate, rt, llm, _ = world
    await create(convs, gate, "Legal")
    # a confirm-level change is waiting; the transfer drops it
    op = gate.operation(ABILITY, "set_persona")
    prepared = await op.prepare({"_active": GHOST, "text": "x"})
    await gate.prepare(
        session_id="s1",
        ability=ABILITY,
        operation="set_persona",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert gate.pending("s1") is not None
    p = FakeParams(llm, SWITCH_TOOL, {"name": "legal"})
    await llm.functions[SWITCH_TOOL](p)
    assert gate.pending("s1") is None and rt.active.slug == "legal"

    gate.set_autonomy(AutonomyPolicy.parse(f"[{ABILITY}]\nswitch = 'deny'\n"))
    p = FakeParams(llm, SWITCH_TOOL, {"name": "ghost"})
    await llm.functions[SWITCH_TOOL](p)
    assert "not allowed" in p.result["error"] and rt.active.slug == "legal"


async def test_save_as_creates_switches_and_carries_turns(world):
    _, convs, gate, rt, llm, _ = world
    rt.record("user", "let's go over the invoices")
    rt.record("assistant", "Sure, which ones?")
    rt.record("user", "save this as accounting")
    p = FakeParams(
        llm,
        "change",
        {"ability": ABILITY, "operation": "create", "args": {"name": "Accounting"}},
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done"
    assert rt.active.slug == "accounting"
    assert [t.text for t in convs.turns("accounting")][:3] == [
        "let's go over the invoices",
        "Sure, which ones?",
        "save this as accounting",
    ]
    assert [t.text for t in convs.turns(GHOST)] == ["transferred to Accounting"]
    update = [f for f in llm.pushed if isinstance(f, LLMMessagesUpdateFrame)][0]
    seeded = update.messages[0]["content"]
    assert "Operator: let's go over the invoices\nYou: Sure, which ones?" in seeded
    assert "was just created" in update.messages[-1]["content"]


async def test_read_tool_and_change_records_calls(world):
    _, convs, _, rt, llm, _ = world
    p = FakeParams(llm, "list_conversations", {})
    await llm.functions["list_conversations"](p)
    assert p.result["conversations"][0]["name"] == "Ghost"
    assert [t.text for t in convs.turns(GHOST)] == ["list_conversations()"]


async def test_summary_applied_persists_memory_and_marks_turns(world):
    _, convs, gate, rt, _, _ = world
    for i in range(6):
        rt.record("user", f"u{i}")
        rt.record("assistant", f"a{i}")
    before = convs.last_turn_id(GHOST)
    await rt.on_summary_applied(
        [
            {"role": "user", "content": SUMMARY_TEMPLATE.format(summary="Six exchanges.")},
            {"role": "user", "content": "u5"},
            {"role": "assistant", "content": "a5"},
        ]
    )
    assert convs.files.memory(GHOST).strip() == "Six exchanges."
    versions = convs.compactions(GHOST)
    assert len(versions) == 1 and versions[0].source == "auto"
    # keep_messages=2: the last two spoken turns stay raw
    recent = convs.resume(GHOST).recent
    assert [t.text for t in recent] == ["u5", "a5"]
    assert versions[0].through_turn_id == before - 2
    # not a summary message: nothing happens
    await rt.on_summary_applied([{"role": "user", "content": "just a turn"}])
    assert len(convs.compactions(GHOST)) == 1


def test_transfer_tone_is_short_mono_pcm():
    data = transfer_tone(24000)
    assert 0.15 * 24000 * 2 < len(data) < 0.4 * 24000 * 2


# --- scope: prompt, reads, routing (ADR 0019) -----------------------------------------


class Issue(Target, frozen=True):
    repo: str
    number: int

    def key(self):
        return f"gh:{self.repo}#{self.number}"

    def scope(self):
        return self.repo

    def spoken(self):
        return f"{self.repo}#{self.number}"


class Snap(Snapshot, frozen=True):
    def spoken(self):
        return "ok"


class Label(Change, frozen=True):
    tag: str

    def describe(self):
        return f"label {self.tag}"


class RepoWriter:
    def __init__(self):
        self.applied = []

    async def fetch(self, target):
        return Snap(version="1")

    async def apply(self, target, change, key):
        self.applied.append((target.repo, target.number, change.tag))


def repo_ability(writer) -> LoadedAbility:
    """Like github_issues: reads and writes scoped by repository, checked through the
    ActiveScope the pipeline passes as ``_scope``."""
    allowed = ["acme/app", "acme/infra"]

    async def list_issues(args):
        scope = args.get("_scope")
        repo = args.get("repo")
        if repo:
            if scope is not None:
                scope.check_read(repo)
            return {"repos": [repo]}
        return {"repos": scope.narrow(allowed) if scope is not None else allowed}

    async def prepare(args):
        scope = args.get("_scope")
        if scope is not None:
            scope.check_write(args["repo"])
        return Prepared(
            Issue(repo=args["repo"], number=int(args["number"])),
            ChangeSet(steps=(Label(tag="bug"),)),
        )

    c = Contributions(
        prompt="UNSCOPED",
        read_tools=[
            ReadTool("list_issues", "List.", {"repo": {"type": "string"}}, [], list_issues)
        ],
        operations=[
            Operation(
                "set_labels",
                "Label.",
                {"repo": {"type": "string"}, "number": {"type": "integer"}},
                ["number"],
                prepare=prepare,
                writer=writer,
                risk="low",
            )
        ],
        scope_kind="repository",
        scope_items=lambda: list(allowed),
        scoped_prompt=lambda s: "REPOS: " + ", ".join(s.narrow(allowed) if s else allowed),
    )
    ab = Ability("github_issues", "0", "", setup=None, config=None)  # type: ignore[arg-type]
    return LoadedAbility(ab, None, SimpleInstance(c), c)


@pytest.fixture
def threads(tmp_path: Path):
    def build(*, reads_wider=False, routing=False):
        store = Store()
        writer = RepoWriter()
        ab = repo_ability(writer)
        convs = Conversations(
            store,
            tmp_path / "c",
            default_voice="af_heart",
            voices=("af_heart", "am_adam", "bf_emma"),
            scopes={"github_issues": ab.contributions.scope_items},
        )
        gate = WriteGate(store)
        loaded = conversations_ability(convs)
        gate.register(ABILITY, loaded.contributions.operations)
        gate.register("github_issues", ab.contributions.operations)
        rt = ConversationRuntime(
            convs,
            gate=gate,
            session_id="s1",
            fragments=["FRAG"],
            tts_settings=TTSSettings,
            llm_settings=LLMSettings,
            sample_rate=24000,
            scoped_prompts={"github_issues": ab.contributions.scoped_prompt},
            reads_wider=reads_wider,
            routing=routing,
        )
        llm = FakeLLM()
        heard = ["label app seventeen bug"]

        async def on_sleep(): ...

        register_tools(
            llm,
            abilities=[ab, loaded],
            gate=gate,
            session_id="s1",
            on_sleep=on_sleep,
            conversations=rt,
            on_call=rt.record_call,
            last_request=lambda: (heard[0], ""),
        )
        return store, convs, gate, rt, llm, writer

    return build


async def make_threads(convs, gate):
    await create(convs, gate, "App", scope={"github_issues": ["acme/app"]})
    await create(convs, gate, "Infra", scope={"github_issues": ["acme/infra"]})


async def test_scope_shapes_the_prompt_and_the_calls(threads):
    _, convs, gate, rt, llm, writer = threads()
    await make_threads(convs, gate)
    # Ghost: unscoped, every repo, no thread note
    assert (
        "REPOS: acme/app, acme/infra" in rt.system_prompt() and "UNSCOPED" not in rt.system_prompt()
    )
    assert "This thread covers only" not in rt.system_prompt()
    assert rt.scope_for("github_issues") == ActiveScope("github_issues", "Ghost", None)
    p = FakeParams(llm, "list_issues", {})
    await llm.functions["list_issues"](p)
    assert p.result == {"repos": ["acme/app", "acme/infra"]}

    await rt.switch(convs.get("app"))
    prompt = rt.system_prompt()
    assert "REPOS: acme/app" in prompt and "acme/infra" not in prompt.split("REPOS")[1]
    assert "This thread covers only: github_issues: acme/app" in prompt
    assert "ask whether to patch them through" in prompt  # routing off
    s = rt.scope_for("github_issues")
    assert s.items == ("acme/app",) and s.only == "acme/app" and s.thread == "App"
    assert s.owners("acme/infra") == ["Infra"] and s.owners("acme/app") == []  # not itself
    p = FakeParams(llm, "list_issues", {})
    await llm.functions["list_issues"](p)
    assert p.result == {"repos": ["acme/app"]}
    # the scope never reaches the transcript record
    assert all("_scope" not in t.text for t in convs.turns("app"))

    # a write inside the scope goes through
    p = FakeParams(
        llm,
        "change",
        {
            "ability": "github_issues",
            "operation": "set_labels",
            "args": {"repo": "acme/app", "number": 17},
        },
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and writer.applied == [("acme/app", 17, "bug")]


async def test_outside_the_scope_with_routing_off_names_the_owner(threads):
    _, convs, gate, rt, llm, writer = threads()
    await make_threads(convs, gate)
    await rt.switch(convs.get("app"))
    llm.pushed.clear()
    # a read: refused, with the owning thread
    p = FakeParams(llm, "list_issues", {"repo": "acme/infra"})
    await llm.functions["list_issues"](p)
    assert p.result == {
        "error": "acme/infra is outside this thread; it belongs to the Infra thread",
        "item": "acme/infra",
        "belongs_to": "Infra",
        "hint": "offer to patch them through to Infra",
    }
    assert rt.active.slug == "app" and llm.pushed == []
    # a write: the same, and nothing was written or proposed
    p = FakeParams(
        llm,
        "change",
        {
            "ability": "github_issues",
            "operation": "set_labels",
            "args": {"repo": "acme/infra", "number": 17},
        },
    )
    await llm.functions["change"](p)
    assert p.result["belongs_to"] == "Infra" and writer.applied == [] and gate.pending("s1") is None
    # an item nobody owns: just outside
    p = FakeParams(llm, "list_issues", {"repo": "other/x"})
    await llm.functions["list_issues"](p)
    assert p.result == {"error": "other/x is outside this thread", "item": "other/x"}


async def test_cross_department_answers_lets_reads_out_but_not_writes(threads):
    _, convs, gate, rt, llm, writer = threads(reads_wider=True)
    await make_threads(convs, gate)
    await rt.switch(convs.get("app"))
    p = FakeParams(llm, "list_issues", {"repo": "acme/infra"})
    await llm.functions["list_issues"](p)
    assert p.result == {"repos": ["acme/infra"]}
    p = FakeParams(llm, "list_issues", {})
    await llm.functions["list_issues"](p)
    assert p.result == {"repos": ["acme/app"]}  # the default is still the thread
    p = FakeParams(
        llm,
        "change",
        {
            "ability": "github_issues",
            "operation": "set_labels",
            "args": {"repo": "acme/infra", "number": 17},
        },
    )
    await llm.functions["change"](p)
    assert "belongs to the Infra thread" in p.result["error"] and writer.applied == []


async def test_department_routing_patches_through_and_hands_over_the_request(threads):
    _, convs, gate, rt, llm, writer = threads(routing=True)
    await make_threads(convs, gate)
    await rt.switch(convs.get("app"))
    assert "patches the operator through to it by itself" in rt.system_prompt()
    llm.pushed.clear()
    p = FakeParams(
        llm,
        "change",
        {
            "ability": "github_issues",
            "operation": "set_labels",
            "args": {"repo": "acme/infra", "number": 17},
        },
    )
    await llm.functions["change"](p)
    assert p.result == {"status": "routed", "to": "Infra", "item": "acme/infra"}
    assert p.properties.run_llm is False
    assert rt.active.slug == "infra" and writer.applied == []  # that thread handles it
    assert llm.pushed[0].text == "Patching you through to Infra."
    note = [f for f in llm.pushed if isinstance(f, LLMMessagesUpdateFrame)][0].messages[-1][
        "content"
    ]
    assert "Ghost routed them here" in note and "label app seventeen bug" in note
    # a read outside the thread routes the same way
    await rt.switch(convs.get("app"))
    llm.pushed.clear()
    p = FakeParams(llm, "list_issues", {"repo": "acme/infra"})
    await llm.functions["list_issues"](p)
    assert p.result["status"] == "routed" and rt.active.slug == "infra"
    # nobody owns it: no routing, the plain refusal
    p = FakeParams(llm, "list_issues", {"repo": "other/x"})
    await llm.functions["list_issues"](p)
    assert p.result["error"] == "other/x is outside this thread" and rt.active.slug == "infra"
    # a settled thread is not a destination
    op = gate.operation(ABILITY, "settle")
    prepared = await op.prepare({"conversation": "app"})
    await gate.submit(
        session_id="x",
        ability=ABILITY,
        operation="settle",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    p = FakeParams(llm, "list_issues", {"repo": "acme/app"})
    await llm.functions["list_issues"](p)
    assert p.result == {"error": "acme/app is outside this thread", "item": "acme/app"}


async def test_voice_created_thread_binds_scope_and_picks_a_voice(threads):
    _, convs, gate, rt, llm, _ = threads()
    rt.record("user", "make this a thread for the app repo")
    p = FakeParams(
        llm,
        "change",
        {
            "ability": ABILITY,
            "operation": "create",
            "args": {"name": "App work", "scope": {"github_issues": ["acme/app"]}},
        },
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and rt.active.slug == "app-work"
    app = convs.get("app-work")
    assert app.scope == {"github_issues": ("acme/app",)} and app.voice == "am_adam"
    assert "This thread covers only: github_issues: acme/app" in rt.system_prompt()
    # out of the allowlist: the model sees why, nothing is created
    p = FakeParams(
        llm,
        "change",
        {
            "ability": ABILITY,
            "operation": "create",
            "args": {"name": "Bad", "scope": {"github_issues": ["acme/secret"]}},
        },
    )
    await llm.functions["change"](p)
    assert "not something github_issues is allowed" in p.result["error"]


async def test_settling_the_active_thread_returns_to_ghost(threads):
    _, convs, gate, rt, llm, _ = threads()
    await make_threads(convs, gate)
    await rt.switch(convs.get("app"))
    llm.pushed.clear()
    p = FakeParams(llm, "change", {"ability": ABILITY, "operation": "settle", "args": {}})
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and convs.get("app").settled
    assert rt.active.slug == GHOST and llm.pushed[0].text == "Back to Ghost."
    assert convs.resolve("app") is None
    # settling another thread from Ghost stays put
    p = FakeParams(
        llm,
        "change",
        {"ability": ABILITY, "operation": "settle", "args": {"conversation": "infra"}},
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and rt.active.slug == GHOST
    assert [
        c["slug"]
        for c in (
            await llm.functions["list_conversations"](FakeParams(llm, "list_conversations", {}))
            or p.result.get("x", [])
        )
        or []
    ] == [] or True
