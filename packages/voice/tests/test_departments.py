"""Departments (ADR 0023): a session reaches what its department's toolkits grant, the
prompt is built from that, and a transfer swaps the tool list."""

from __future__ import annotations

from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Operation, Prepared, ReadTool, Store, ToolSource, WriteGate
from ghost_core.conversations import SOURCE, Conversations, Profile, conversations_source
from ghost_core.models import ChangeSet
from ghost_voice.conversation import ConversationRuntime
from ghost_voice.tools import register_tools
from pipecat.frames.frames import LLMSetToolsFrame
from pipecat.services.anthropic.llm import AnthropicLLMService
from test_prompt_and_tools import Doc, FakeLLM, FakeParams, FakeWriter, Tag

LLMSettings = AnthropicLLMService.Settings


class TTSSettings:  # the runtime only needs a callable that takes voice/speed
    def __init__(self, **kw):
        self.kw = kw


def source(name: str, toolkit: str | None = None, read: str = "look") -> ToolSource:
    writer = FakeWriter()

    async def echo(args):
        return {"from": name}

    async def prepare(args):
        return Prepared(Doc(name=args["name"]), ChangeSet(steps=(Tag(tag="t"),)))

    return ToolSource(
        name=name,
        prompt=f"{name.upper()} PARAGRAPH",
        read_tools=[ReadTool(f"{name}_{read}", "Look.", {}, [], echo)],
        operations=[
            Operation(
                "flag",
                "Flag.",
                {"name": {"type": "string"}},
                ["name"],
                prepare=prepare,
                writer=writer,
                risk="low",
            )
        ],
        toolkit=toolkit,
    )


@pytest.fixture
def world(tmp_path: Path):
    store = Store()
    policy = AutonomyPolicy.parse(
        "[toolkit.finance]\nskills = []\n[toolkit.finance.servers.books]\n"
        "[toolkit.code]\n[toolkit.code.servers.github]\n"
    )
    convs = Conversations(
        store, tmp_path / "c", default_voice="af_heart", toolkits=lambda: list(policy.toolkits)
    )
    gate = WriteGate(store, autonomy=policy)
    books, github = source("books"), source("github")
    everything = [books, github]
    gate.register("books", books.operations)
    gate.register("github", github.operations)
    lines = conversations_source(convs)
    gate.register(SOURCE, lines.operations)

    def reach(dep: Profile) -> list[ToolSource]:
        if dep.is_ghost or not dep.toolkits:
            return [*everything, lines]
        out = []
        for name in dep.toolkits:
            if name == "finance":
                out.append(source("books", toolkit="finance"))
            if name == "code":
                out.append(source("github", toolkit="code"))
        return [*out, lines]

    rt = ConversationRuntime(
        convs,
        gate=gate,
        session_id="s1",
        fragments=["IGNORED WHEN REACH IS SET"],
        tts_settings=TTSSettings,
        llm_settings=LLMSettings,
        sample_rate=24000,
        keep_messages=2,
        reach=reach,
    )
    llm = FakeLLM()

    async def on_sleep(): ...

    schemas = register_tools(
        llm,
        sources=[*everything, lines],
        gate=gate,
        session_id="s1",
        on_hang_up=on_sleep,
        conversations=rt,
    )
    return convs, rt, llm, schemas


def names(schemas) -> set[str]:
    return {s.name for s in schemas}


async def call(llm, name: str, args: dict) -> dict:
    p = FakeParams(name, args, llm)
    await llm.functions[name](p)
    return p.result


async def test_ghost_reaches_everything_and_a_department_only_its_toolkits(world):
    convs, rt, llm, schemas = world
    assert {"books_look", "github_look", "change", "list_conversations"} <= names(schemas)
    assert "BOOKS PARAGRAPH" in rt.system_prompt() and "GITHUB PARAGRAPH" in rt.system_prompt()
    assert "IGNORED" not in rt.system_prompt()
    # a department with the finance toolkit
    accounting = await _create(convs, rt, "Accounting", toolkits=["finance"])
    reach = rt.sources_for(accounting)
    assert [s.name for s in reach] == ["books", SOURCE] and reach[0].toolkit == "finance"
    assert "GITHUB PARAGRAPH" not in rt.system_prompt(accounting)
    schemas = rt.tool_schemas(reach)
    assert "books_look" in names(schemas) and "github_look" not in names(schemas)
    # a topic line in it reaches the same
    audit = await _create(convs, rt, "Q3 audit", department=accounting.slug)
    assert audit.department == accounting.slug and not audit.is_department
    assert [s.name for s in rt.sources_for(audit)] == ["books", SOURCE]
    assert rt.conversations.department_of(audit).slug == accounting.slug
    # a department with no toolkits reaches everything, like Ghost
    legal = await _create(convs, rt, "Legal")
    assert {s.name for s in rt.sources_for(legal)} == {"books", "github", SOURCE}


async def test_a_transfer_swaps_prompt_and_tools_and_calls_are_dispatched(world):
    convs, rt, llm, _ = world
    accounting = await _create(convs, rt, "Accounting", toolkits=["finance"])
    frames = await rt.switch(accounting)
    tools_frames = [f for f in frames if isinstance(f, LLMSetToolsFrame)]
    assert len(tools_frames) == 1
    advertised = {t.name for t in tools_frames[0].tools.standard_tools}
    assert "books_look" in advertised and "github_look" not in advertised
    assert rt.department_slug() == accounting.slug
    # the registered function for github_look still exists but refuses on this line
    out = await call(llm, "github_look", {})
    assert "not available on this line" in out["error"]
    assert (await call(llm, "books_look", {})) == {"from": "books"}
    # change on a source outside the reach is unknown here
    out = await call(
        llm, "change", {"ability": "github", "operation": "flag", "args": {"name": "x"}}
    )
    assert "unknown operation" in out["error"]
    out = await call(
        llm, "change", {"ability": "books", "operation": "flag", "args": {"name": "x"}}
    )
    assert out["status"] == "done"


async def _create(convs, rt, name: str, **kw) -> Profile:
    create = next(o for o in convs.operations() if o.name == "create")
    prepared = await create.prepare({"name": name, **kw})
    await convs.writer.apply(prepared.target, prepared.changes.steps[0], "k")
    return convs.get(prepared.target.slug)
