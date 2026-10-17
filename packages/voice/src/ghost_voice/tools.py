"""Bridge skills to Pipecat function calling.

Three kinds of tool reach the model: a skill's typed read tools, one shared
``change`` that hands a typed change set to the domain gate, and ``undo``. What
``change`` does depends on the operator's autonomy policy (ADR 0011): the write
either happens now and the verified result comes back, or a proposal waits for
a spoken yes. The model never applies a write itself.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from ghost_domain import Autonomy, GateError, WriteGate
from ghost_domain.policy import PolicyError
from ghost_skills import Skill
from loguru import logger
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.services.llm_service import FunctionCallParams, LLMService

OnSleep = Callable[[], Awaitable[None]]

CHANGE_TOOL = "change"
UNDO_TOOL = "undo"


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

    # Operations the policy denies outright are not offered at all.
    ops = {
        s.name: {
            op.name: op
            for op in s.prepare_ops
            if gate.level(s.name, op.name, None, op.risk) is not Autonomy.DENY
        }
        for s in skills
    }
    ops = {k: v for k, v in ops.items() if v}
    op_lines = "\n".join(
        f"- {sname}.{op.name}: {op.description} Args: {sorted(op.properties)}"
        f" (required: {op.required})"
        for sname, sops in ops.items()
        for op in sops.values()
    )
    if ops:
        schemas.append(
            FunctionSchema(
                name=CHANGE_TOOL,
                description=(
                    "Make a change. Returns status 'done' with the verified result when the "
                    "change was applied at once, or 'needs_confirmation' with a summary when "
                    "the operator must okay it first: then ask in one short sentence and stop.\n"
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

    async def change(params: FunctionCallParams) -> None:
        a = params.arguments
        skill = by_name.get(str(a.get("skill")))
        op_name = str(a.get("operation"))
        op = ops.get(skill.name, {}).get(op_name) if skill else None
        if skill is None or op is None:
            await params.result_callback(
                {"error": f"unknown operation {a.get('skill')!r}.{op_name!r}"}
            )
            return
        try:
            req = await skill.prepare(op_name, dict(a.get("args") or {}))
            sub = await gate.submit(
                session_id=session_id,
                skill=skill.name,
                operation=req.operation,
                target=req.target,
                changes=req.changes,
                rationale=str(a.get("rationale") or req.rationale),
                risk=op.risk,
            )
        except (ValueError, PolicyError, GateError) as e:
            await params.result_callback({"error": str(e)})
            return
        if sub.outcome is not None:
            logger.info(
                f"{sub.autonomy.value} {sub.proposal.id}: {sub.outcome.status} "
                f"{sub.outcome.summary()}"
            )
            await params.result_callback(
                {
                    "status": "done",
                    "result": sub.outcome.summary(),
                    "state_now": _state(sub.outcome),
                    "how_to_say_it": (
                        "a few words, in passing"
                        if sub.autonomy is Autonomy.ANNOUNCE
                        else "only if asked, or if it failed"
                    ),
                }
            )
            return
        logger.info(f"pending {sub.proposal.id}: {sub.proposal.preview}")
        await params.result_callback(
            {
                "status": "needs_confirmation",
                "summary": sub.proposal.preview,
                "expires_in_secs": 60,
            }
        )

    if ops:
        llm.register_function(CHANGE_TOOL, change)

    schemas.append(
        FunctionSchema(
            name=UNDO_TOOL,
            description=(
                "Reverse the last change made in this conversation (within ten minutes). "
                "Returns the verified result."
            ),
            properties={},
            required=[],
        )
    )

    async def undo(params: FunctionCallParams) -> None:
        try:
            outcome = await gate.undo(session_id)
        except GateError as e:
            await params.result_callback({"error": str(e)})
            return
        logger.info(f"undo {outcome.proposal_id}: {outcome.status}")
        await params.result_callback(
            {"status": "done", "result": outcome.summary(), "state_now": _state(outcome)}
        )

    llm.register_function(UNDO_TOOL, undo)

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


def _state(outcome) -> str:
    s = outcome.readback
    if s is None:
        return "unknown"
    return f"{s.state}, labels: {', '.join(s.labels) if s.labels else 'none'}"


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
