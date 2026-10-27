"""The in-session side of conversations (ADR 0014): prompt, seeded context, transfer
frames, save-as, records and compaction persistence. No audio, no Pipecat worker."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from ghost_core import (
    AutonomyPolicy,
    Change,
    ChangeSet,
    Operation,
    Prepared,
    ReadTool,
    Snapshot,
    Store,
    Target,
    ToolSource,
    WriteGate,
)
from ghost_core.conversations import (
    GHOST,
    SOURCE,
    SUMMARY_TEMPLATE,
    Conversations,
    conversations_source,
)
from ghost_voice.conversation import ConversationMarkerFrame, ConversationRuntime, transfer_tone
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


class _Functions(dict):
    """Pipecat's table: a name not registered on its own goes to the catch-all (None)."""

    def __missing__(self, name):
        if None in self:
            return self[None]
        raise KeyError(name)


class FakeLLM:
    def __init__(self) -> None:
        self.functions: dict[str, Any] = _Functions()
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


def layout(tmp_path: Path, rules: str = "") -> Path:
    """Ghost's folder (ADR 0035), with an AGENTS.md when ``rules`` is given."""
    cfg = tmp_path / "ghost"
    cfg.mkdir(parents=True, exist_ok=True)
    if rules:
        (cfg / "AGENTS.md").write_text(rules)
    return cfg


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    convs = Conversations(store, layout(tmp_path), default_voice="af_heart")
    gate = WriteGate(
        store,
        autonomy=AutonomyPolicy.parse(f"[{SOURCE}]\nset_persona = 'announce'\n"),
    )
    loaded = conversations_source(convs)
    gate.register(SOURCE, loaded.operations)
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
        sources=[loaded],
        gate=gate,
        session_id="s1",
        on_hang_up=on_sleep,
        conversations=rt,
        on_call=rt.record_call,
    )
    return store, convs, gate, rt, llm, schemas


async def create(convs, gate, name, **extra):
    op = gate.operation(SOURCE, "create")
    prepared = await op.prepare({"name": name, **extra})
    return await gate.submit(
        session_id="setup",
        ability=SOURCE,
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
    assert msgs[-1]["content"].endswith("Operator: hello\nYou: Hi.\n(end of the record)")


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
    assert llm.pushed[0].text == "Transferring you to Accounting."
    assert llm.pushed[2].delta == TTSSettings(voice="am_adam", speed=1.1)
    assert llm.pushed[3].slug == "accounting"
    prompt = llm.pushed[4].delta.system_instruction
    assert "You are Accounting, one of the operator's agents" in prompt
    assert "hates surprises" in prompt
    assert "FRAG" in prompt
    update = llm.pushed[5]
    assert update.run_llm is True
    assert "Reconciling September" in update.messages[0]["content"]
    assert "transferred to you from Ghost" in update.messages[-1]["content"]
    assert "Q3 invoices" in update.messages[-1]["content"]
    assert rt.active.slug == "accounting"

    # assistant text is attributed by the marker, not by the clock: what the tap
    # reports before the marker is Ghost's, after it Accounting's
    rt.record("assistant", "Transferring you to Accounting.")
    assert not rt.settled.is_set()  # Ghost's announce line is not the greeting
    rt.on_marker(llm.pushed[3])
    rt.record("assistant", "Accounting here. We were reconciling September.")
    assert rt.settled.is_set()
    assert [t.text for t in convs.turns(GHOST)] == [
        "patch me to accounting",
        "switch_conversation(handoff=Q3 invoices, name=the accounting team)",
        "transferred to Accounting",
        "Transferring you to Accounting.",
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
    op = gate.operation(SOURCE, "set_persona")
    prepared = await op.prepare({"_active": GHOST, "text": "x"})
    await gate.prepare(
        session_id="s1",
        ability=SOURCE,
        operation="set_persona",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )
    assert gate.pending("s1") is not None
    p = FakeParams(llm, SWITCH_TOOL, {"name": "legal"})
    await llm.functions[SWITCH_TOOL](p)
    assert gate.pending("s1") is None and rt.active.slug == "legal"

    gate.set_autonomy(AutonomyPolicy.parse(f"[{SOURCE}]\nswitch = 'deny'\n"))
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
        {"ability": SOURCE, "operation": "create", "args": {"name": "Accounting"}},
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
    assert "was just hired" in update.messages[-1]["content"]


async def test_read_tool_and_change_records_calls(world):
    _, convs, _, rt, llm, _ = world
    p = FakeParams(llm, "list_agents", {})
    await llm.functions["list_agents"](p)
    assert p.result["agents"][0]["name"] == "Ghost"
    assert [t.text for t in convs.turns(GHOST)] == ["list_agents()"]


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


# --- every agent's reach: one set of permissions (ADR 0034, ADR 0035) ----------------


class Issue(Target, frozen=True):
    repo: str
    number: int

    def key(self):
        return f"gh:{self.repo}#{self.number}"

    def scope(self):
        return self.repo

    def spoken(self):
        return f"{self.repo}#{self.number}"

    def rule_args(self):
        return {"repo": self.repo, "number": self.number}


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


def repo_source(writer, name: str) -> ToolSource:
    """Like a connected GitHub app (ADR 0034): every agent reaches it, under the
    operator's one set of permissions (ADR 0035)."""

    async def list_issues(args):
        return {"repos": [args["repo"]] if args.get("repo") else ["acme/app", "acme/infra"]}

    async def prepare(args):
        return Prepared(
            Issue(repo=args["repo"], number=int(args["number"])),
            ChangeSet(steps=(Label(tag="bug"),)),
        )

    return ToolSource(
        name=name,
        prompt=f"{name.upper()} is connected.",
        read_tools=[
            ReadTool(
                f"{name}_list_issues",
                "List.",
                {"repo": {"type": "string"}},
                [],
                list_issues,
                operation="list_issues",
            )
        ],
        operations=[
            Operation(
                "set_labels",
                "Label.",
                {"repo": {"type": "string"}, "number": {"type": "integer"}},
                ["number"],
                prepare=prepare,
                writer=writer,
                tool_name=f"{name}_set_labels",
            )
        ],
    )


RULES = "# Ghost\n\nUK dates.\n"
BLOCK_SECRET = '[permissions]\nblock = ["github.*(repo=acme/secret)"]\n'


@pytest.fixture
def agents(tmp_path: Path):
    """Ghost and a hired agent reach one github app; the operator's permissions block every
    call about acme/secret, whichever agent makes it."""

    def build():
        store = Store()
        writer = RepoWriter()
        convs = Conversations(
            store,
            layout(tmp_path, RULES),
            default_voice="af_heart",
            voices=("af_heart", "am_adam", "bf_emma"),
        )
        gate = WriteGate(store, autonomy=AutonomyPolicy.parse(BLOCK_SECRET))
        agents = conversations_source(convs)
        gate.register(SOURCE, agents.operations)
        github = repo_source(writer, "github")
        gate.register("github", github.operations)

        def reach(p):
            return [github, agents]

        rt = ConversationRuntime(
            convs,
            gate=gate,
            session_id="s1",
            fragments=["FRAG"],
            tts_settings=TTSSettings,
            llm_settings=LLMSettings,
            sample_rate=24000,
            reach=reach,
        )
        llm = FakeLLM()
        heard = ["label app seventeen bug"]

        async def on_sleep(): ...

        register_tools(
            llm,
            sources=[github, agents],
            gate=gate,
            session_id="s1",
            on_hang_up=on_sleep,
            conversations=rt,
            on_call=rt.record_call,
            last_request=lambda: (heard[0], ""),
        )
        return store, convs, gate, rt, llm, writer

    return build


async def hire(gate, name: str, **args):
    create = gate.operation(SOURCE, "create")
    prepared = await create.prepare({"name": name, **args})
    await gate.submit(
        session_id="setup",
        ability=SOURCE,
        operation="create",
        target=prepared.target,
        changes=prepared.changes,
        rationale="",
    )


async def test_every_agent_reaches_the_same_apps_and_reads_ghosts_rules(agents):
    _, convs, gate, rt, llm, writer = agents()
    await hire(gate, "Brand", job="the look")
    # Ghost: the app, the rules, the roster of agents by name and job
    prompt = rt.system_prompt()
    assert "GITHUB is connected." in prompt and "FRAG" not in prompt
    assert "standing instructions, for every agent:\n# Ghost\n\nUK dates." in prompt
    assert "other agents, for transfers:\n- Brand: the look" in prompt
    assert "department" not in prompt.lower()
    p = FakeParams(llm, "github_list_issues", {})
    await llm.functions["github_list_issues"](p)
    assert p.result == {"repos": ["acme/app", "acme/infra"]}

    await rt.switch(convs.get("brand"))
    prompt = rt.system_prompt()
    assert "GITHUB is connected." in prompt  # every agent reaches the app
    assert "You are Brand, one of the operator's agents, hired for the look" in prompt
    assert "UK dates." in prompt  # Ghost's AGENTS.md is in every agent's prompt
    assert "for transfers" not in prompt  # only Ghost lists them
    assert "department" not in prompt.lower()
    p = FakeParams(llm, "github_set_labels", {"repo": "acme/app", "number": 17})
    await llm.functions[None](p)
    assert p.result["status"] == "done" and writer.applied == [("acme/app", 17, "bug")]


async def test_what_the_permissions_block_is_refused_for_every_agent(agents):
    _, convs, gate, rt, llm, writer = agents()
    await hire(gate, "Brand")
    for slug in (GHOST, "brand"):
        await rt.switch(convs.get(slug))
        p = FakeParams(llm, "github_list_issues", {"repo": "acme/secret"})
        await llm.functions["github_list_issues"](p)
        assert "blocked" in p.result["error"]
        p = FakeParams(llm, "github_set_labels", {"repo": "acme/secret", "number": 17})
        await llm.functions[None](p)
        assert "blocked" in p.result["error"] and writer.applied == []
        assert gate.pending("s1") is None


async def test_a_voice_hired_agent_and_the_tools_follow(agents):
    _, convs, gate, rt, llm, writer = agents()
    rt.record("user", "hire someone for the release notes")
    p = FakeParams(
        llm,
        "change",
        {
            "ability": SOURCE,
            "operation": "create",
            "args": {"name": "Release Notes", "job": "release notes", "voice": "am_adam"},
        },
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and rt.active.slug == "release-notes"
    agent = convs.get("release-notes")
    assert agent.job == "release notes" and agent.voice == "am_adam"
    assert (
        "You are Release Notes, one of the operator's agents, hired for release notes"
        in rt.system_prompt()
    )
    assert [s.name for s in rt.sources_for()] == ["github", SOURCE]


async def test_a_transfer_names_an_agent(agents):
    _, convs, gate, rt, llm, _ = agents()
    await hire(gate, "Maya", aliases=["the brand agent"])
    assert rt.resolve("Maya").slug == "maya"
    assert rt.resolve("the brand agent").slug == "maya"
    assert rt.resolve("marketing") is None
    p = FakeParams(llm, SWITCH_TOOL, {"name": "marketing"})
    await llm.functions[SWITCH_TOOL](p)
    assert p.result["status"] == "no_match" and p.result["known"] == ["Ghost", "Maya"]
    p = FakeParams(llm, SWITCH_TOOL, {"name": "maya"})
    await llm.functions[SWITCH_TOOL](p)
    assert p.result == {"status": "transferred", "to": "Maya"} and rt.active.slug == "maya"


async def test_retiring_the_active_agent_returns_to_ghost(agents):
    _, convs, gate, rt, llm, _ = agents()
    await hire(gate, "Brand")
    await rt.switch(convs.get("brand"))
    llm.pushed.clear()
    p = FakeParams(llm, "change", {"ability": SOURCE, "operation": "retire", "args": {}})
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and convs.get("brand").retired
    assert rt.active.slug == GHOST and llm.pushed[0].text == "Back to Ghost."
    assert convs.resolve("brand") is None
    # retiring another agent from Ghost stays put
    await hire(gate, "Social")
    p = FakeParams(
        llm,
        "change",
        {"ability": SOURCE, "operation": "retire", "args": {"conversation": "social"}},
    )
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and rt.active.slug == GHOST


async def read(_args):
    return {"ok": True}


async def test_tools_connected_after_the_session_was_built_are_dispatched(tmp_path: Path):
    """Every read goes through one catch-all dispatched to the active agent's reach, so an
    app connected since is there at once (ADR 0034)."""
    store = Store()
    convs = Conversations(store, layout(tmp_path), default_voice="af_heart")
    gate = WriteGate(store, autonomy=AutonomyPolicy())
    agents = conversations_source(convs)
    gate.register(SOURCE, agents.operations)
    figma = ToolSource(
        name="figma",
        read_tools=[ReadTool("figma_get_file", "x", {}, [], read, operation="get_file")],
        operations=[
            Operation("export", "x", {}, [], prepare=None, writer=None, tool_name="figma_export")
        ],
    )
    connected: list[ToolSource] = []  # figma connects after the session is built
    rt = ConversationRuntime(
        convs,
        gate=gate,
        session_id="s",
        fragments=[],
        tts_settings=TTSSettings,
        llm_settings=LLMSettings,
        sample_rate=24000,
        reach=lambda p: [*connected, agents],
    )
    llm = FakeLLM()

    async def hang(): ...

    register_tools(
        llm, sources=[agents], gate=gate, session_id="s", on_hang_up=hang, conversations=rt
    )
    connected[:] = [figma]
    gate.register("figma", figma.operations)
    advertised = {s.name for s in rt.tool_schemas(rt.sources_for())}
    assert {"figma_get_file", "figma_export"} <= advertised
    p = FakeParams(llm, "figma_get_file", {})
    await llm.functions["figma_get_file"](p)
    assert p.result == {"ok": True}
    gate.set_autonomy(AutonomyPolicy.parse('[permissions]\nblock = ["figma"]\n'))
    assert "figma_get_file" not in {s.name for s in rt.tool_schemas(rt.sources_for())}
    p = FakeParams(llm, "figma_get_file", {})
    await llm.functions["figma_get_file"](p)
    assert "not available here" in p.result["error"]
