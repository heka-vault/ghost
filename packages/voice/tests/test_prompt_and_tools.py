from __future__ import annotations

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
from ghost_voice.prompt import build_system_prompt
from ghost_voice.tools import register_tools


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

    async def push_frame(self, frame, *_):
        self.pushed.append(frame)


class FakeParams:
    def __init__(self, name, arguments, llm=None):
        self.function_name = name
        self.arguments = arguments
        self.result: Any = None
        self.llm = llm

    async def result_callback(self, result, **_):
        self.result = result


class Doc(Target, frozen=True):
    name: str

    def key(self):
        return f"doc:{self.name}"


class DocSnap(Snapshot, frozen=True):
    tags: tuple[str, ...]

    def spoken(self):
        return "tags " + (", ".join(self.tags) or "none")


class Tag(Change, frozen=True):
    tag: str

    def describe(self):
        return f"add {self.tag}"


class FakeWriter:
    def __init__(self):
        self.tags: list[str] = []
        self.version = 0

    async def fetch(self, target):
        return DocSnap(version=str(self.version), tags=tuple(self.tags))

    async def apply(self, target, change, key):
        match change:
            case Tag(tag=t):
                self.tags.append(t)
        self.version += 1


def fake_source(writer) -> ToolSource:
    async def echo(args):
        return {"echo": args}

    async def prepare(args):
        if args.get("name") == "forbidden":
            raise ValueError("bad name")
        return Prepared(Doc(name=args["name"]), ChangeSet(steps=(Tag(tag="flagged"),)))

    def op(name, risk):
        return Operation(
            name,
            f"{name}.",
            {"name": {"type": "string"}},
            ["name"],
            prepare=prepare,
            writer=writer,
            risk=risk,
            allowed=lambda t: t.name != "other",
        )

    return ToolSource(
        name="fake",
        prompt="FAKE FRAGMENT",
        read_tools=[ReadTool("echo", "Echo.", {"x": {"type": "string"}}, ["x"], echo)],
        operations=[op("flag", "low"), op("nuke", "high"), op("never", "low")],
    )


@pytest.fixture
def wired():
    llm = FakeLLM()
    writer = FakeWriter()
    loaded = fake_source(writer)
    gate = WriteGate(Store(), autonomy=AutonomyPolicy.parse('[fake]\nnever = "deny"\n'))
    gate.register("fake", loaded.operations)
    slept = []

    async def on_sleep():
        slept.append(True)

    schemas = register_tools(llm, sources=[loaded], gate=gate, session_id="s", on_hang_up=on_sleep)
    return llm, gate, schemas, slept, writer


def test_prompt_includes_fragments_and_persona():
    p = build_system_prompt(["FRAG"])
    assert "You are Ghost" in p and "FRAG" in p and "not Claude Code" in p


def test_only_reads_change_and_sleep_are_exposed(wired):
    _, _, schemas, _, _ = wired
    assert sorted(s.name for s in schemas) == ["bring_up_change", "change", "echo", "hang_up"]


def test_denied_ops_are_not_offered(wired):
    _, _, schemas, _, _ = wired
    change = next(s for s in schemas if s.name == "change")
    assert "fake.flag" in change.description and "fake.nuke" in change.description
    assert "fake.never" not in change.description


async def test_read_tool_round_trip(wired):
    llm, *_ = wired
    p = FakeParams("echo", {"x": "hi"})
    await llm.functions["echo"](p)
    assert p.result == {"echo": {"x": "hi"}}


def call(op, name="readme"):
    return FakeParams("change", {"ability": "fake", "operation": op, "args": {"name": name}})


async def test_low_risk_change_is_done_at_once(wired):
    llm, gate, _, _, writer = wired
    p = call("flag")
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and "add flagged" in p.result["result"]
    assert "flagged" in p.result["state_now"] and gate.pending("s") is None
    assert writer.tags == ["flagged"]


async def test_high_risk_change_needs_confirmation(wired):
    llm, gate, _, _, writer = wired
    p = call("nuke")
    await llm.functions["change"](p)
    assert p.result["status"] == "needs_confirmation" and "add flagged" in p.result["summary"]
    assert gate.pending("s") is not None and writer.tags == []


async def test_denied_op_errors_even_if_called(wired):
    llm, gate, _, _, _ = wired
    p = call("never")
    await llm.functions["change"](p)
    assert "error" in p.result and gate.pending("s") is None


async def test_change_rejects_disallowed_target_and_bad_args(wired):
    llm, gate, _, _, _ = wired
    p = call("flag", name="other")
    await llm.functions["change"](p)
    assert "not allowed" in p.result["error"]
    p = call("flag", name="forbidden")
    await llm.functions["change"](p)
    assert p.result == {"error": "bad name"}


async def test_dotted_operation_name_is_tolerated(wired):
    llm, gate, _, _, writer = wired
    p = FakeParams("change", {"ability": "fake", "operation": "fake.flag", "args": {"name": "x"}})
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and writer.tags == ["flagged"]
    p = FakeParams("change", {"operation": "flag", "args": {"name": "y"}})  # ability omitted
    await llm.functions["change"](p)
    assert p.result["status"] == "done"


def test_no_undo_tool(wired):
    llm, *_ = wired
    assert "undo" not in llm.functions


async def test_hang_up_calls_back(wired):
    llm, _, _, slept, _ = wired
    await llm.functions["hang_up"](FakeParams("hang_up", {}, llm=llm))
    assert slept == [True]
    assert llm.pushed == []  # the model's own goodbye is spoken, not a fixed word


def test_prompt_carries_the_department_and_follows_the_routing_setting():
    plain = build_system_prompt(["F"])
    assert "standing instructions" not in plain and "ask whether to transfer them" in plain
    agent = build_system_prompt(
        ["F"],
        name="Brand",
        is_ghost=False,
        department="Marketing",
        job="brand design",
        rules="# Marketing\n\nUK dates.",
        rules_from_root=True,
        memory="The client is Acme.",
        persona="Warm.",
    )
    assert (
        "You are Brand, an agent in the operator's Marketing department, hired for brand design"
        in agent
    )
    assert "Marketing department's standing instructions, read from its project folder" in agent
    assert "UK dates." in agent and "What the Marketing department knows" in agent
    assert (
        "The client is Acme." in agent and "Who you are, in the operator's words:\nWarm." in agent
    )
    default = build_system_prompt(
        ["F"], name="Marketing", is_ghost=False, department="Marketing", is_default=True
    )
    assert "You are Marketing, the operator's Marketing department" in default
    ghost = build_system_prompt(
        ["F"], roster="- Marketing: the brand. Agents: Brand (brand design)"
    )
    assert "The departments, each with its agents" in ghost and "Brand (brand design)" in ghost
    routed = build_system_prompt(["F"], routing=True)
    assert "transfers the operator there by itself" in routed
