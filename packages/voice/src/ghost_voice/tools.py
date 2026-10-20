"""Bridge abilities to Pipecat function calling.

Two kinds of tool reach the model: an ability's typed read tools and one shared
``change`` that hands a typed change set to the gate (plus session tools: switch
conversation, go to sleep). There is no undo tool: to take a change back the model
proposes the reversing change through ``change``. What ``change`` does depends on
the operator's autonomy policy (ADR 0011): the write either happens now and the
verified result comes back, or a proposal waits for a spoken yes. The model never
applies a write itself.
"""

from __future__ import annotations

import time
from collections.abc import Awaitable, Callable, Sequence
from typing import TYPE_CHECKING, Any

from ghost_core import (
    SCOPE_ARG,
    ActiveScope,
    Autonomy,
    Events,
    GateError,
    LoadedAbility,
    Operation,
    OutOfScope,
    WriteGate,
)
from ghost_core.conversations import ABILITY as CONVERSATIONS
from ghost_core.conversations import GHOST
from ghost_core.policy import PolicyError
from loguru import logger
from pipecat.adapters.schemas.function_schema import FunctionSchema
from pipecat.frames.frames import FunctionCallResultProperties
from pipecat.processors.frame_processor import FrameDirection
from pipecat.services.llm_service import FunctionCallParams, LLMService

from ghost_voice.latency import NULL_PROBE, LatencyProbe

if TYPE_CHECKING:
    from ghost_voice.conversation import ConversationRuntime

OnHangUp = Callable[[], Awaitable[None]]
OnHold = Callable[[], Awaitable[None]]
OnCall = Callable[[str, dict[str, Any]], None]

CHANGE_TOOL = "change"
SWITCH_TOOL = "switch_conversation"


def register_tools(
    llm: LLMService,
    *,
    abilities: Sequence[LoadedAbility],
    gate: WriteGate,
    session_id: str,
    on_hang_up: OnHangUp,
    on_hold: OnHold | None = None,
    events: Events | None = None,
    conversations: ConversationRuntime | None = None,
    on_call: OnCall | None = None,
    last_request: Callable[[], tuple[str, str]] | None = None,
    probe: LatencyProbe | None = None,
) -> list[FunctionSchema]:
    """``probe`` gets the time each step inside a call takes (ghost_voice.latency):
    ``read`` for a read tool (the ability's client, an external API for GitHub), and
    for ``change`` the ability's ``prepare``, the gate's ``submit`` (snapshot, and the
    write itself below ``confirm``) and the ``request`` check that may execute it."""
    schemas: list[FunctionSchema] = []
    probe = probe or NULL_PROBE
    events = events or Events()
    record = on_call or (lambda _n, _a: None)
    last_request = last_request or (lambda: ("", ""))

    def scope_for(ability: LoadedAbility) -> Callable[[], ActiveScope | None]:
        """The active thread's scope for a scoped ability, read at call time so a
        switch mid-session is seen by the next call; None for an unscoped ability."""
        if conversations is None or not ability.contributions.scope_kind:
            return lambda: None
        return lambda: conversations.scope_for(ability.name)

    async def out_of_scope(params: FunctionCallParams, e: OutOfScope) -> None:
        """A request about something another thread owns (ADR 0019). With department
        routing on, patch the operator through and let that thread handle it; off, tell
        the model which thread it belongs to so Ghost can offer."""
        owner = conversations.owner_of(e.ability, e.item) if conversations else None
        if (
            owner is not None
            and conversations is not None
            and conversations.routing
            and conversations.switch_allowed()
        ):
            request, _ = last_request()
            await _transfer(
                params,
                conversations,
                owner,
                handoff=request,
                result={"status": "routed", "to": owner.name, "item": e.item},
                outcome=(
                    f"They asked about {e.item}, which is yours, so Ghost routed them here. "
                    "After your greeting, handle that request now."
                ),
            )
            return
        result: dict[str, Any] = {"error": str(e), "item": e.item}
        if owner is not None:
            result["belongs_to"] = owner.name
            result["hint"] = f"offer to patch them through to {owner.name}"
        await params.result_callback(result)

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
            llm.register_function(
                tool.name,
                _wrap_read(tool.handler, record, probe, scope=scope_for(a), on_scope=out_of_scope),
            )
    scopes = {a.name: scope_for(a) for a in abilities}

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
        aname = str(a.get("ability") or a.get("skill") or "")  # "skill" for older contexts
        op_name = str(a.get("operation") or "")
        if "." in op_name:  # the model echoed "ability.operation" from the listing
            head, _, tail = op_name.rpartition(".")
            if head in ops:
                aname, op_name = head, tail
        if not aname and len(ops) == 1:
            aname = next(iter(ops))
        op = ops.get(aname, {}).get(op_name)
        call_id = getattr(params, "tool_call_id", "")
        if op is None:
            probe.tool_status(call_id, "error")
            await params.result_callback({"error": f"unknown operation {aname!r}.{op_name!r}"})
            return
        probe.tool_operation(call_id, f"{aname}.{op.name}")
        args = dict(a.get("args") or {})
        record(f"{aname}.{op_name}", args)
        if aname == CONVERSATIONS and conversations is not None:
            args["_active"] = conversations.active.slug
        scope = scopes.get(aname, lambda: None)()
        if scope is not None:
            args[SCOPE_ARG] = scope
        try:
            began = time.perf_counter()
            prepared = await op.prepare(args)
            probe.tool_step(call_id, "prepare", time.perf_counter() - began)
            began = time.perf_counter()
            sub = await gate.submit(
                session_id=session_id,
                ability=aname,
                operation=op.name,
                target=prepared.target,
                changes=prepared.changes,
                rationale=str(a.get("rationale") or prepared.rationale),
                scope=scope,
                conversation=conversations.active.slug if conversations else None,
            )
            probe.tool_step(call_id, "submit", time.perf_counter() - began)
        except OutOfScope as e:
            probe.tool_status(call_id, "out_of_scope")
            await out_of_scope(params, e)
            return
        except (ValueError, PolicyError, GateError) as e:
            probe.tool_status(call_id, "error")
            await params.result_callback({"error": str(e)})
            return
        if sub.outcome is not None:
            probe.tool_status(call_id, f"done:{sub.autonomy.value}")
            logger.info(
                f"{sub.autonomy.value} {sub.proposal.id}: {sub.outcome.status} "
                f"{sub.outcome.summary()}"
            )
            result = {
                "status": "done",
                "result": sub.outcome.summary(),
                "state_now": _state(sub.outcome),
                "how_to_say_it": (
                    "a few words, in passing"
                    if sub.autonomy is Autonomy.ANNOUNCE
                    else "only if asked, or if it failed"
                ),
            }
            # "Save this as accounting": a created conversation is switched into at
            # once, carrying this session's turns along (ADR 0014).
            if (
                aname == CONVERSATIONS
                and op.name == "create"
                and conversations is not None
                and sub.outcome.status == "success"
                and bool(args.get("switch", True))
            ):
                target = conversations.conversations.get(sub.proposal.target.scope())
                if target is not None:
                    conversations.carry_session_turns(target)
                    await _transfer(
                        params,
                        conversations,
                        target,
                        handoff="",
                        result=result,
                        outcome=(
                            f"The conversation {target.name!r} was just created at their "
                            "request and they are now on it; that is done, do not create "
                            "or switch again."
                        ),
                    )
                    await events.emit(
                        "outcome", ability=aname, operation=op.name, status=sub.outcome.status
                    )
                    return
            # A settled thread leaves voice routing (ADR 0019): settling the one the
            # operator is in hands them back to Ghost.
            if (
                aname == CONVERSATIONS
                and op.name == "settle"
                and conversations is not None
                and sub.outcome.status == "success"
                and sub.proposal.target.scope() == conversations.active.slug
            ):
                ghost = conversations.conversations.get(GHOST)
                if ghost is not None:
                    await _transfer(
                        params,
                        conversations,
                        ghost,
                        handoff="",
                        result=result,
                        outcome=(
                            f"They just settled the {conversations.active.name!r} thread at "
                            "their request; that is done, do not settle or switch again."
                        ),
                    )
                    await events.emit(
                        "outcome", ability=aname, operation=op.name, status=sub.outcome.status
                    )
                    return
            await params.result_callback(result)
            await events.emit(
                "outcome", ability=aname, operation=op.name, status=sub.outcome.status
            )
            return
        logger.info(f"pending {sub.proposal.id}: {sub.proposal.preview}")
        # The operator may already have said yes by asking for it: "close infra 17".
        request, before = last_request()
        began = time.perf_counter()
        outcome = await gate.confirm_by_request(session_id, request, question=before)
        probe.tool_step(call_id, "request", time.perf_counter() - began)
        probe.tool_status(call_id, "done:request" if outcome is not None else "needs_confirmation")
        if outcome is not None:
            logger.info(f"executed {outcome.proposal_id}: {outcome.status} (as asked)")
            await params.result_callback(
                {
                    "status": "done",
                    "result": outcome.summary(),
                    "state_now": _state(outcome),
                    "how_to_say_it": "a few words, in passing; they asked for exactly this",
                }
            )
            await events.emit("outcome", ability=aname, operation=op.name, status=outcome.status)
            return
        await params.result_callback(
            {
                "status": "needs_confirmation",
                "summary": sub.proposal.preview,
                "expires_in_secs": 60,
            }
        )

    if ops:
        llm.register_function(CHANGE_TOOL, change)

    if conversations is not None:
        schemas.append(
            FunctionSchema(
                name=SWITCH_TOOL,
                description=(
                    "Patch the operator through to another saved conversation by name "
                    '("accounting", "legal", "ghost" to come back). The transfer is '
                    "announced and the other line greets them; say nothing else yourself. "
                    "If no conversation matches, the result says so: then ask in one short "
                    "sentence whether to start one, and on yes call change with "
                    "conversations.create."
                ),
                properties={
                    "name": {"type": "string", "description": "The conversation to go to."},
                    "handoff": {
                        "type": "string",
                        "description": "One line for the other side about what the operator "
                        "wants, if the request said.",
                    },
                },
                required=["name"],
            )
        )

        async def switch(params: FunctionCallParams) -> None:
            name = str(params.arguments.get("name") or "")
            record(SWITCH_TOOL, dict(params.arguments))
            if not conversations.switch_allowed():
                await params.result_callback({"error": "switching conversations is not allowed"})
                return
            target = conversations.resolve(name)
            if target is None:
                names = [p.name for p in conversations.conversations.list()]
                await params.result_callback(
                    {
                        "status": "no_match",
                        "known": names,
                        "hint": "ask whether to start a conversation with that name",
                    }
                )
                return
            if target.slug == conversations.active.slug:
                await params.result_callback(
                    {"status": "already_here", "conversation": target.name}
                )
                return
            await _transfer(
                params,
                conversations,
                target,
                handoff=str(params.arguments.get("handoff") or ""),
                result={"status": "transferred", "to": target.name},
            )

        llm.register_function(SWITCH_TOOL, switch)

    schemas.append(
        FunctionSchema(
            name="hang_up",
            description=(
                "End the conversation: the operator said goodbye. Say a short, warm "
                "goodbye in the same reply."
            ),
            properties={},
            required=[],
        )
    )

    async def hang_up(params: FunctionCallParams) -> None:
        # The model's goodbye in this same response is spoken (ghost_voice.narration lets
        # hang_up's text through); no follow-up completion runs, and the call ends as the
        # goodbye finishes.
        await params.result_callback(
            {"ok": True}, properties=FunctionCallResultProperties(run_llm=False)
        )
        await on_hang_up()

    llm.register_function("hang_up", hang_up)

    if on_hold is not None:
        schemas.append(
            FunctionSchema(
                name="hold",
                description=(
                    "Wait: the operator asked you to hang on or turned to talk to someone "
                    "else. You hear nothing until they say your name again; the "
                    "conversation and any open question keep."
                ),
                properties={},
                required=[],
            )
        )

        async def hold(params: FunctionCallParams) -> None:
            # No follow-up completion: the words in this response are all Ghost says
            # before it goes quiet.
            await params.result_callback(
                {"ok": True}, properties=FunctionCallResultProperties(run_llm=False)
            )
            await on_hold()

        llm.register_function("hold", hold)
    return schemas


async def _transfer(
    params: FunctionCallParams,
    conversations: ConversationRuntime,
    target,
    *,
    handoff: str,
    result: dict[str, Any],
    outcome: str = "",
) -> None:
    """Return the tool result without a model run, then, once the aggregator has
    appended it to the context, feed the transfer frames in at the pipeline source.
    Not out of the LLM service: a frame the LLM pushes never passes through the LLM
    itself (the system prompt would not change), and anything pushed downstream
    queues behind the audio being played."""
    worker = getattr(params, "pipeline_worker", None)
    frames = await conversations.switch(target, handoff=handoff, outcome=outcome)

    async def on_context_updated() -> None:
        try:
            if worker is not None:
                await worker.queue_frames(frames)
            else:
                for frame in frames:
                    await params.llm.push_frame(frame, FrameDirection.DOWNSTREAM)
        except Exception:
            conversations.transfer_done()
            raise

    if not frames:
        await params.result_callback(result)
        return
    await params.result_callback(
        result,
        properties=FunctionCallResultProperties(
            run_llm=False, on_context_updated=on_context_updated
        ),
    )


def _state(outcome) -> str:
    return outcome.readback.spoken() if outcome.readback else "unknown"


OnScope = Callable[[FunctionCallParams, OutOfScope], Awaitable[None]]


def _wrap_read(
    handler: Callable[[dict[str, Any]], Awaitable[dict[str, Any]]],
    record: OnCall,
    probe: LatencyProbe = NULL_PROBE,
    *,
    scope: Callable[[], ActiveScope | None] = lambda: None,
    on_scope: OnScope | None = None,
):
    async def run(params: FunctionCallParams) -> None:
        args = dict(params.arguments)
        record(params.function_name, args)
        active = scope()
        if active is not None:
            args[SCOPE_ARG] = active
        began = time.perf_counter()
        status = "ok"
        try:
            result = await handler(args)
        except OutOfScope as e:
            call_id = getattr(params, "tool_call_id", "")
            probe.tool_step(call_id, "read", time.perf_counter() - began)
            probe.tool_status(call_id, "out_of_scope")
            if on_scope is not None:
                await on_scope(params, e)
            else:
                await params.result_callback({"error": str(e), "item": e.item})
            return
        except (ValueError, PolicyError) as e:
            result = {"error": str(e)}
            status = "error"
        except Exception as e:  # noqa: BLE001
            logger.exception(f"tool {params.function_name} failed")
            result = {"error": f"{type(e).__name__}: {e}"}
            status = "error"
        call_id = getattr(params, "tool_call_id", "")
        probe.tool_step(call_id, "read", time.perf_counter() - began)
        probe.tool_status(call_id, status)
        await params.result_callback(result)

    return run
