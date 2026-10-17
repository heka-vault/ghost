"""Bridge skills to Pipecat function calling.

Only two kinds of tool reach the model: a skill's typed read tools, and one
shared ``prepare`` that stores a proposal through the domain gate. There is no
tool that executes a write (ADR 0007).
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from ghost_domain import WriteGate
from ghost_domain.policy import PolicyError
from ghost_skills import Skill
from loguru import logger
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.services.llm_service import FunctionCallParams, LLMService

OnSleep = Callable[[], Awaitable[None]]


def register_tools(
    llm: LLMService,
    *,
    skills: list[Skill],
    gate: WriteGate,
    session_id: str,
    on_sleep: OnSleep,
) -> list[FunctionSchema]:
    schemas: list[FunctionSchema] = []
    by_name: dict[str, Skill] = {s.name: s for s in skills}

    for skill in skills:
        for tool in skill.read_tools:
            schemas.append(
                FunctionSchema(
                    name=tool.name,
                    description=tool.description,
                    properties=tool.properties,
                    required=tool.required,
                )
            )
            llm.register_function(tool.name, _wrap_read(tool.handler))

    ops = {s.name: {op.name: op for op in s.prepare_ops} for s in skills}
    op_lines = "\n".join(
        f"- {sname}.{op.name}: {op.description} Args: {sorted(op.properties)}"
        f" (required: {op.required})"
        for sname, sops in ops.items()
        for op in sops.values()
    )
    schemas.append(
        FunctionSchema(
            name="prepare",
            description=(
                "Prepare a change for the operator to confirm. Nothing is written. "
                "Returns a preview; read it aloud word for word, then stop and wait.\n"
                "Operations:\n" + op_lines
            ),
            properties={
                "skill": {"type": "string", "enum": list(ops)},
                "operation": {"type": "string"},
                "args": {"type": "object", "description": "Arguments for the operation."},
                "rationale": {"type": "string", "description": "One sentence: why."},
            },
            required=["skill", "operation", "args"],
        )
    )

    async def prepare(params: FunctionCallParams) -> None:
        a = params.arguments
        skill = by_name.get(str(a.get("skill")))
        if skill is None:
            await params.result_callback({"error": f"unknown skill {a.get('skill')!r}"})
            return
        try:
            req = await skill.prepare(str(a.get("operation")), dict(a.get("args") or {}))
            proposal = await gate.prepare(
                session_id=session_id,
                skill=skill.name,
                operation=req.operation,
                target=req.target,
                changes=req.changes,
                rationale=str(a.get("rationale") or req.rationale),
            )
        except (ValueError, PolicyError) as e:
            await params.result_callback({"error": str(e)})
            return
        logger.info(f"proposal {proposal.id} prepared: {proposal.preview}")
        await params.result_callback(
            {"proposal_id": proposal.id, "preview": proposal.preview, "expires_in_secs": 60}
        )

    llm.register_function("prepare", prepare)

    schemas.append(
        FunctionSchema(
            name="go_to_sleep",
            description="End the conversation and return to passive listening.",
            properties={},
            required=[],
        )
    )

    async def go_to_sleep(params: FunctionCallParams) -> None:
        await params.result_callback({"ok": True})
        await on_sleep()

    llm.register_function("go_to_sleep", go_to_sleep)
    return schemas


def _wrap_read(handler: Callable[[dict[str, Any]], Awaitable[dict[str, Any]]]):
    async def run(params: FunctionCallParams) -> None:
        try:
            result = await handler(dict(params.arguments))
        except (ValueError, PolicyError) as e:
            result = {"error": str(e)}
        except Exception as e:  # noqa: BLE001
            logger.exception(f"tool {params.function_name} failed")
            result = {"error": f"{type(e).__name__}: {e}"}
        await params.result_callback(result)

    return run
