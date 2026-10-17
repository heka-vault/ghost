from __future__ import annotations

from typing import Any

import pytest
from ghost_domain import ChangeSet, RepoPolicy, SetLabels, Snapshot, Store, Target, WriteGate
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
    async def fetch(self, target):
        return Snapshot(title="T", state="open", labels=("needs-signoff",), updated_at="x")

    async def set_labels(self, *a): ...
    async def add_comment(self, *a, **k): ...
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
        return [
            PrepareOp(
                "flag",
                "Flag it.",
                {"repo": {"type": "string"}, "number": {"type": "integer"}},
                ["repo", "number"],
            )
        ]

    async def prepare(self, operation, args):
        if operation != "flag":
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
    gate = WriteGate(Store(), FakeWriter(), RepoPolicy.from_csv("acme/app"))
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


def test_only_reads_prepare_and_sleep_are_exposed(wired):
    _, _, schemas, _ = wired
    assert sorted(s.name for s in schemas) == ["echo", "go_to_sleep", "prepare"]


async def test_read_tool_round_trip(wired):
    llm, *_ = wired
    p = FakeParams("echo", {"x": "hi"})
    await llm.functions["echo"](p)
    assert p.result == {"echo": {"x": "hi"}}


async def test_prepare_stores_proposal_and_returns_preview(wired):
    llm, gate, _, _ = wired
    p = FakeParams(
        "prepare", {"skill": "fake", "operation": "flag", "args": {"repo": "acme/app", "number": 1}}
    )
    await llm.functions["prepare"](p)
    assert "preview" in p.result and gate.pending("s") is not None


async def test_prepare_rejects_disallowed_repo(wired):
    llm, gate, _, _ = wired
    p = FakeParams(
        "prepare",
        {"skill": "fake", "operation": "flag", "args": {"repo": "acme/other", "number": 1}},
    )
    await llm.functions["prepare"](p)
    assert "error" in p.result and gate.pending("s") is None


async def test_go_to_sleep_calls_back(wired):
    llm, _, _, slept = wired
    await llm.functions["go_to_sleep"](FakeParams("go_to_sleep", {}))
    assert slept == [True]
