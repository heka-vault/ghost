"""The in-session side of conversations (ADR 0014): prompt, seeded context, transfer
frames, save-as, records and compaction persistence. No audio, no Pipecat worker."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from ghost_core import AutonomyPolicy, Store, WriteGate
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
