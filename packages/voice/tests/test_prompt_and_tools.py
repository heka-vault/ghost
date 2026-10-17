from __future__ import annotations

from typing import Any

import pytest
from ghost_domain import (
    AutonomyPolicy,
    ChangeSet,
    RepoPolicy,
    SetLabels,
    Snapshot,
    Store,
    Target,
    WriteGate,
)
from ghost_skills.base import PrepareOp, PrepareRequest, ReadTool
from ghost_voice.prompt import build_system_prompt
from ghost_voice.tools import register_tools


class FakeLLM:
    def __init__(self) -> None:
        self.functions: dict[str, Any] = {}

    def register_function(self, name, handler, **_):
        self.functions[name] = handler


class FakeParams:
    def __init__(self, name, arguments):
        self.function_name = name
        self.arguments = arguments
        self.result: Any = None

    async def result_callback(self, result, **_):
        self.result = result


class FakeWriter:
    def __init__(self):
        self.labels = ["needs-signoff"]
        self.writes = []

    async def fetch(self, target):
        return Snapshot(title="T", state="open", labels=tuple(self.labels), updated_at="x")

    async def set_labels(self, target, add, remove):
        self.writes.append(("set_labels", add, remove))
        self.labels = [x for x in self.labels if x not in remove] + list(add)

    async def add_comment(self, *a, **k): ...
    async def delete_comment(self, *a, **k): ...
    async def set_state(self, *a): ...


class FakeSkill:
    name = "fake"
    prompt_fragment = "FAKE FRAGMENT"

    @property
    def read_tools(self):
        async def echo(args):
            return {"echo": args}

        return [ReadTool("echo", "Echo.", {"x": {"type": "string"}}, ["x"], echo)]

    @property
    def prepare_ops(self):
        issue = {"repo": {"type": "string"}, "number": {"type": "integer"}}
        return [
            PrepareOp("flag", "Flag it.", issue, ["repo", "number"], risk="low"),
            PrepareOp("nuke", "Dangerous.", issue, ["repo", "number"], risk="high"),
            PrepareOp("never", "Denied by policy.", issue, ["repo", "number"], risk="low"),
        ]

    async def prepare(self, operation, args):
        if operation not in {"flag", "nuke", "never"}:
            raise ValueError("bad op")
        owner, repo = args["repo"].split("/")
        return PrepareRequest(
            operation,
            Target(owner=owner, repo=repo, number=int(args["number"])),
            ChangeSet(steps=(SetLabels(add=("flagged",)),)),
            "because",
        )

    async def aclose(self): ...


@pytest.fixture
def wired():
    llm = FakeLLM()
    gate = WriteGate(
        Store(),
        FakeWriter(),
        RepoPolicy.from_csv("acme/app"),
        autonomy=AutonomyPolicy.parse('[fake]\nnever = "deny"\n'),
    )
    slept = []

    async def on_sleep():
        slept.append(True)

    schemas = register_tools(
        llm, skills=[FakeSkill()], gate=gate, session_id="s", on_sleep=on_sleep
    )
    return llm, gate, schemas, slept


def test_prompt_includes_fragments_and_persona():
    p = build_system_prompt(["FRAG"])
    assert "You are Ghost" in p and "FRAG" in p and "not Claude Code" in p


def test_only_reads_change_undo_and_sleep_are_exposed(wired):
    _, _, schemas, _ = wired
    assert sorted(s.name for s in schemas) == ["change", "echo", "go_to_sleep", "undo"]


def test_denied_ops_are_not_offered(wired):
    _, _, schemas, _ = wired
    change = next(s for s in schemas if s.name == "change")
    assert "fake.flag" in change.description and "fake.nuke" in change.description
    assert "fake.never" not in change.description


async def test_read_tool_round_trip(wired):
    llm, *_ = wired
    p = FakeParams("echo", {"x": "hi"})
    await llm.functions["echo"](p)
    assert p.result == {"echo": {"x": "hi"}}


def call(op, repo="acme/app"):
    return FakeParams(
        "change", {"skill": "fake", "operation": op, "args": {"repo": repo, "number": 1}}
    )


async def test_low_risk_change_is_done_at_once(wired):
    llm, gate, _, _ = wired
    p = call("flag")
    await llm.functions["change"](p)
    assert p.result["status"] == "done" and "add flagged" in p.result["result"]
    assert "flagged" in p.result["state_now"] and gate.pending("s") is None


async def test_high_risk_change_needs_confirmation(wired):
    llm, gate, _, _ = wired
    p = call("nuke")
    await llm.functions["change"](p)
    assert p.result["status"] == "needs_confirmation" and "add flagged" in p.result["summary"]
    assert gate.pending("s") is not None


async def test_denied_op_errors_even_if_called(wired):
    llm, gate, _, _ = wired
    p = call("never")
    await llm.functions["change"](p)
    assert "error" in p.result and gate.pending("s") is None


async def test_change_rejects_disallowed_repo(wired):
    llm, gate, _, _ = wired
    p = call("flag", repo="acme/other")
    await llm.functions["change"](p)
    assert "error" in p.result and gate.pending("s") is None


async def test_undo_reverses_last_change(wired):
    llm, gate, _, _ = wired
    await llm.functions["change"](call("flag"))
    u = FakeParams("undo", {})
    await llm.functions["undo"](u)
    assert u.result["status"] == "done" and "remove flagged" in u.result["result"]
    u2 = FakeParams("undo", {})
    await llm.functions["undo"](u2)
    assert "error" in u2.result


async def test_go_to_sleep_calls_back(wired):
    llm, _, _, slept = wired
    await llm.functions["go_to_sleep"](FakeParams("go_to_sleep", {}))
    assert slept == [True]
