"""A department's own tools get a handler even though its servers come up after the
session was built (ADR 0024 review): every read goes through one catch-all dispatched to
the active agent's reach."""

from __future__ import annotations

import json
from pathlib import Path

from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.conversations import Conversations, conversations_source
from ghost_core.department_tools import DepartmentTools, departments_source
from ghost_core.departments import Departments
from ghost_core.source import Operation, ReadTool, ToolSource
from ghost_voice.conversation import ConversationRuntime
from ghost_voice.tools import register_tools
from test_conversation_runtime import FakeLLM, FakeParams, LLMSettings, TTSSettings


async def read(_args):
    return {"ok": True}


async def test_the_department_s_own_reads_are_dispatched(tmp_path: Path):
    cfg = tmp_path / "ghost"
    (cfg / "departments" / "marketing").mkdir(parents=True)
    (cfg / "departments" / "marketing" / "department.json").write_text(
        json.dumps({"title": "Marketing"})
    )
    (cfg / "departments" / "marketing" / "docs").mkdir()
    (cfg / "departments" / "marketing" / "docs" / "voice.md").write_text("Never salesy.\n")
    deps = Departments(cfg)
    store = Store()
    convs = Conversations(store, deps, default_voice="af_heart")
    gate = WriteGate(store, autonomy=AutonomyPolicy())
    agents = conversations_source(convs)
    gate.register("conversations", agents.operations)
    dtools = DepartmentTools(deps, store, convs)
    gate.register("departments", dtools.operations())
    figma = ToolSource(
        name="marketing/figma",
        read_tools=[ReadTool("figma_get_file", "x", {}, [], read)],
        operations=[
            Operation(
                "export", "x", {}, [], prepare=None, writer=None, risk="low", allowed=lambda t: True
            )
        ],
    )
    gate.register("marketing/figma", figma.operations, department="marketing")

    def reach(p):
        out = [agents, departments_source(dtools)]
        if p.department == "marketing":
            out.insert(0, figma)
        return out

    marketing = convs.default_agent("marketing")
    for start in ("ghost", marketing.slug):
        rt = ConversationRuntime(
            convs,
            gate=gate,
            session_id="s",
            fragments=[],
            tts_settings=TTSSettings,
            llm_settings=LLMSettings,
            sample_rate=24000,
            reach=reach,
            start=start,
        )
        llm = FakeLLM()

        async def hang(): ...

        schemas = register_tools(
            llm, sources=[agents], gate=gate, session_id="s", on_hang_up=hang, conversations=rt
        )
        advertised = {s.name for s in schemas}
        assert {"read_department_doc", "department_journal"} <= advertised
        p = FakeParams(llm, "read_department_doc", {"file": "voice.md"})
        await llm.functions["read_department_doc"](p)
        if start == "ghost":
            assert "figma_get_file" not in advertised
            assert p.result.get("error")  # Ghost has no docs; the handler ran
            p = FakeParams(llm, "figma_get_file", {})
            await llm.functions["figma_get_file"](p)
            assert "not available in this department" in p.result["error"]
        else:
            assert "figma_get_file" in advertised
            assert p.result == {
                "department": "Marketing",
                "file": "voice.md",
                "text": "Never salesy.\n",
            }
            p = FakeParams(llm, "figma_get_file", {})
            await llm.functions["figma_get_file"](p)
            assert p.result == {"ok": True}
