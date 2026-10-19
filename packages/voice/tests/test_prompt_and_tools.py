from __future__ import annotations

from typing import Any

import pytest
from ghost_core import (
    Ability,
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
from ghost_voice.prompt import build_system_prompt
from ghost_voice.tools import register_tools


class FakeLLM:
    def __init__(self) -> None:
        self.functions: dict[str, Any] = {}
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


def fake_ability(writer) -> LoadedAbility:
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

    c = Contributions(
        prompt="FAKE FRAGMENT",
        read_tools=[ReadTool("echo", "Echo.", {"x": {"type": "string"}}, ["x"], echo)],
        operations=[op("flag", "low"), op("nuke", "high"), op("never", "low")],
    )
    ab = Ability("fake", "0", "", setup=None, config=None)  # type: ignore[arg-type]
    return LoadedAbility(ab, None, SimpleInstance(c), c)


@pytest.fixture
def wired():
    llm = FakeLLM()
    writer = FakeWriter()
    loaded = fake_ability(writer)
    gate = WriteGate(Store(), autonomy=AutonomyPolicy.parse('[fake]\nnever = "deny"\n'))
    gate.register("fake", loaded.contributions.operations)
    slept = []

    async def on_sleep():
        slept.append(True)

    schemas = register_tools(llm, abilities=[loaded], gate=gate, session_id="s", on_sleep=on_sleep)
    return llm, gate, schemas, slept, writer


def test_prompt_includes_fragments_and_persona():
    p = build_system_prompt(["FRAG"])
    assert "You are Ghost" in p and "FRAG" in p and "not Claude Code" in p


def test_only_reads_change_and_sleep_are_exposed(wired):
    _, _, schemas, _, _ = wired
    assert sorted(s.name for s in schemas) == ["change", "echo", "go_to_sleep"]


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


async def test_go_to_sleep_calls_back(wired):
    llm, _, _, slept, _ = wired
    await llm.functions["go_to_sleep"](FakeParams("go_to_sleep", {}, llm=llm))
    assert slept == [True]
    assert llm.pushed == []  # the model's own goodbye is spoken, not a fixed word


def test_prompt_scope_note_follows_the_routing_setting():
    assert "This thread covers only" not in build_system_prompt(["F"])
    offer = build_system_prompt(["F"], scope="github_issues: acme/app", routing=False)
    assert "This thread covers only: github_issues: acme/app" in offer
    assert "ask whether to patch them through" in offer
    routed = build_system_prompt(["F"], scope="github_issues: acme/app", routing=True)
    assert "patches the operator through to it by itself" in routed
