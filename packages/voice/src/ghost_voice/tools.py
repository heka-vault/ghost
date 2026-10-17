"""Bridge abilities to Pipecat function calling.

Three kinds of tool reach the model: an ability's typed read tools, one shared
``change`` that hands a typed change set to the gate, and ``undo``. What ``change``
does depends on the operator's autonomy policy (ADR 0011): the write either happens
now and the verified result comes back, or a proposal waits for a spoken yes. The
model never applies a write itself.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from typing import Any

from ghost_core import Autonomy, Events, GateError, LoadedAbility, Operation, WriteGate
from ghost_core.policy import PolicyError
from loguru import logger
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.services.llm_service import FunctionCallParams, LLMService

OnSleep = Callable[[], Awaitable[None]]

CHANGE_TOOL = "change"
UNDO_TOOL = "undo"


def register_tools(
    llm: LLMService,
    *,
    abilities: Sequence[LoadedAbility],
    gate: WriteGate,
    session_id: str,
    on_sleep: OnSleep,
    events: Events | None = None,
) -> list[FunctionSchema]:
    schemas: list[FunctionSchema] = []
    events = events or Events()

    for a in abilities:
        for tool in a.contributions.read_tools:
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
    ops: dict[str, dict[str, Operation]] = {}
    for a in abilities:
        offered = {
            op.name: op
            for op in a.contributions.operations
            if gate.level(a.name, op.name) is not Autonomy.DENY
        }
        if offered:
            ops[a.name] = offered
    op_lines = "\n".join(
        f"- {aname}.{op.name}: {op.description} Args: {sorted(op.properties)}"
        f" (required: {op.required})"
        for aname, aops in ops.items()
        for op in aops.values()
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
                    "ability": {"type": "string", "enum": list(ops)},
                    "operation": {"type": "string"},
                    "args": {"type": "object", "description": "Arguments for the operation."},
                    "rationale": {"type": "string", "description": "One sentence: why."},
                },
                required=["ability", "operation", "args"],
            )
        )

    async def change(params: FunctionCallParams) -> None:
        a = params.arguments
        aname = str(a.get("ability") or a.get("skill"))  # "skill" for older contexts
        op_name = str(a.get("operation"))
        op = ops.get(aname, {}).get(op_name)
        if op is None:
            await params.result_callback({"error": f"unknown operation {aname!r}.{op_name!r}"})
            return
        try:
            prepared = await op.prepare(dict(a.get("args") or {}))
            sub = await gate.submit(
                session_id=session_id,
                ability=aname,
                operation=op.name,
                target=prepared.target,
                changes=prepared.changes,
                rationale=str(a.get("rationale") or prepared.rationale),
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
            await events.emit(
                "outcome", ability=aname, operation=op.name, status=sub.outcome.status
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
        await events.emit("outcome", operation="undo", status=outcome.status)

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
    return outcome.readback.spoken() if outcome.readback else "unknown"


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
