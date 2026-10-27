"""Bridge tool sources to Pipecat function calling.

Three kinds of tool reach the model: a source's typed read tools; each connected app's
changes, under the app's name like its reads (ADR 0034); and one shared ``change`` for
Ghost's own operations (the agents, the departments, the outputs), plus the session's
tools (switch, hang up, hold). A change of either kind goes to the gate. What happens
then is the operator's permissions (ADR 0011, ADR 0034): it happens now and the result
comes back, or it waits for a spoken yes. There is no undo tool: to take a change back
the model asks for the reversing change. The model never applies a change itself.
"""

from __future__ import annotations

import time
from collections.abc import Awaitable, Callable, Sequence
from typing import TYPE_CHECKING, Any

from ghost_core import (
    Autonomy,
    Events,
    GateError,
    Operation,
    ToolSource,
    WriteGate,
)
from ghost_core.conversations import GHOST
from ghost_core.conversations import SOURCE as CONVERSATIONS
from ghost_core.department_tools import SOURCE as DEPARTMENTS
from ghost_core.outputs import SOURCE as OUTPUTS
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
BRING_UP_TOOL = "bring_up_change"


def register_tools(
    llm: LLMService,
    *,
    sources: Sequence[ToolSource],
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
    ``read`` for a read tool (the app behind it), and for a change the source's
    ``prepare``, the gate's ``submit`` (the call itself below ``confirm``) and the
    ``request`` check that may execute it."""
    probe = probe or NULL_PROBE
    events = events or Events()
    record = on_call or (lambda _n, _a: None)
    last_request = last_request or (lambda: ("", ""))

    def active_sources() -> list[ToolSource]:
        """What the active agent reaches (ADR 0024): its department's sources, or every
        source when the session was built without departments."""
        if conversations is not None and conversations.has_reach:
            return conversations.sources_for(conversations.active)
        return list(sources)

    def department() -> str | None:
        return conversations.department_slug() if conversations is not None else None

    def offered_ops(srcs: Sequence[ToolSource]) -> dict[str, dict[str, Operation]]:
        """Operations the department's permissions do not block outright."""
        out: dict[str, dict[str, Operation]] = {}
        for a in srcs:
            offered = {
                op.name: op for op in a.operations if gate.offers(a.name, op.name, department())
            }
            if offered:
                out[a.name] = offered
        return out

    def offered_reads(srcs: Sequence[ToolSource]):
        for a in srcs:
            for tool in a.read_tools:
                if gate_offers_read(a.name, tool.operation or tool.name):
                    yield tool, a

    def gate_offers_read(source: str, operation: str) -> bool:
        return gate.offers(source, operation, department(), read=True)

    def change_schema(ops: dict[str, dict[str, Operation]]) -> FunctionSchema:
        own = {
            a: o
            for a, aops in ops.items()
            if (o := [op for op in aops.values() if op.tool_name is None])
        }
        op_lines = "\n".join(
            f"- {aname}.{op.name}: {op.description} Args: {sorted(op.properties)}"
            f" (required: {op.required})"
            for aname, aops in own.items()
            for op in aops
        )
        return FunctionSchema(
            name=CHANGE_TOOL,
            description=(
                "Make a change. Returns status 'done' with the verified result when the "
                "change was applied at once, or 'needs_confirmation' with a summary when "
                "the operator must okay it first: then ask in one short sentence and stop.\n"
                "Operations:\n" + op_lines
            ),
            properties={
                "ability": {
                    "type": "string",
                    "enum": list(own),
                    "description": "What it changes: 'conversations' (the agents), "
                    "'departments', 'outputs' or 'skills'.",
                },
                "operation": {"type": "string"},
                "args": {"type": "object", "description": "Arguments for the operation."},
                "rationale": {"type": "string", "description": "One sentence: why."},
            },
            required=["ability", "operation", "args"],
        )

    def schemas_for(srcs: Sequence[ToolSource]) -> list[FunctionSchema]:
        """The tools advertised for a list of sources: their reads, each app's changes
        under their own names, `change` for Ghost's own operations, and the session's own
        tools. A transfer advertises the new agent's."""
        out: list[FunctionSchema] = [
            FunctionSchema(
                name=tool.name,
                description=tool.description,
                properties=tool.properties,
                required=tool.required,
            )
            for tool, _ in offered_reads(srcs)
        ]
        ops = offered_ops(srcs)
        for aops in ops.values():
            for op in aops.values():
                if op.tool_name is not None:
                    out.append(
                        FunctionSchema(
                            name=op.tool_name,
                            description=op.description,
                            properties=op.properties,
                            required=op.required,
                        )
                    )
        if any(op.tool_name is None for aops in ops.values() for op in aops.values()):
            out.append(change_schema(ops))
        out.extend(session_schemas)
        return out

    # Every read tool and every app's change is one catch-all handler: a call goes to the
    # version the active agent reaches now (an app connected since the session was built
    # is there), or is refused when the permissions block it.
    async def dispatch(params: FunctionCallParams) -> None:
        name = params.function_name
        reach_now = active_sources()
        changes = {
            op.tool_name: (op, aname)
            for aname, aops in offered_ops(reach_now).items()
            for op in aops.values()
            if op.tool_name is not None
        }
        if name in changes:
            op, aname = changes[name]
            args = dict(params.arguments)
            rationale = str(args.pop("rationale", "") or "")
            await run_change(params, aname, op, args, rationale)
            return
        current = {t.name: (t, a) for t, a in offered_reads(reach_now)}
        hit = current.get(name)
        if hit is None:
            record(name, dict(params.arguments))
            await params.result_callback({"error": f"{name} is not available here"})
            return
        tool, a = hit
        if gate.blocks_read(a.name, tool.operation or tool.name, params.arguments, department()):
            record(name, dict(params.arguments))
            await params.result_callback(
                {"error": f"{name} is blocked by the operator's permissions for that"}
            )
            return
        if a.name in (DEPARTMENTS, OUTPUTS) and conversations is not None:
            params.arguments = {**params.arguments, "_active": conversations.active.slug}
        await _wrap_read(tool.handler, record, probe)(params)

    llm.register_function(None, dispatch)

    any_ops = conversations is not None or any(
        op.tool_name is None for a in sources for op in a.operations
    )
    session_schemas: list[FunctionSchema] = []  # switch, hang_up, hold; filled below

    async def change(params: FunctionCallParams) -> None:
        reach_now = active_sources()
        ops = offered_ops(reach_now)
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
        if op is None:
            probe.tool_status(getattr(params, "tool_call_id", ""), "error")
            await params.result_callback({"error": f"unknown operation {aname!r}.{op_name!r}"})
            return
        await run_change(
            params, aname, op, dict(a.get("args") or {}), str(a.get("rationale") or "")
        )

    async def run_change(
        params: FunctionCallParams,
        aname: str,
        op: Operation,
        args: dict[str, Any],
        rationale: str,
    ) -> None:
        """One change, from `change` or from an app's own tool: prepare, the gate, and
        what the model is told."""
        op_name = op.name
        call_id = getattr(params, "tool_call_id", "")
        probe.tool_operation(call_id, f"{aname}.{op.name}")
        record(f"{aname}.{op_name}", args)
        if aname in (CONVERSATIONS, DEPARTMENTS, OUTPUTS) and conversations is not None:
            args["_active"] = conversations.active.slug
        if aname == DEPARTMENTS:
            args["_request"] = last_request()[0]  # a rule must be the operator's words
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
                rationale=rationale or prepared.rationale,
                conversation=conversations.active.slug if conversations else None,
                department=department(),
                asked_by=prepared.asked_by,
            )
            probe.tool_step(call_id, "submit", time.perf_counter() - began)
        except (ValueError, PolicyError, GateError) as e:
            probe.tool_status(call_id, "error")
            await params.result_callback({"error": str(e)})
            return
        except Exception as e:  # noqa: BLE001 - an app's failure is the answer
            logger.warning(f"change {aname}.{op_name} failed: {type(e).__name__}: {e}")
            probe.tool_status(call_id, "error")
            await params.result_callback({"error": f"{aname}.{op_name} failed: {e}"})
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
            # "Save this as accounting": a hired agent is switched into at once,
            # carrying this session's turns along (ADR 0014).
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
                            f"The agent {target.name!r} was just hired at their request "
                            "and they are now with it; that is done, do not hire or "
                            "switch again."
                        ),
                    )
                    await events.emit(
                        "outcome", ability=aname, operation=op.name, status=sub.outcome.status
                    )
                    return
            # "Move this to accounting" (ADR 0024): the agent the operator is with moved
            # departments, so the session is re-homed in place: that department's rules,
            # memory and voice, and "Moved to Accounting."
            if (
                aname == CONVERSATIONS
                and op.name == "move"
                and conversations is not None
                and sub.outcome.status == "success"
                and sub.proposal.target.scope() == conversations.active.slug
            ):
                moved = conversations.conversations.get(conversations.active.slug)
                if moved is not None:
                    await _transfer(
                        params, conversations, moved, handoff="", result=result, rehome=True
                    )
                    await events.emit(
                        "outcome", ability=aname, operation=op.name, status=sub.outcome.status
                    )
                    return
            # A retired agent leaves voice routing (ADR 0024): retiring the one the
            # operator is with hands them to its department's own agent, else to Ghost.
            if (
                aname == CONVERSATIONS
                and op.name == "retire"
                and conversations is not None
                and sub.outcome.status == "success"
                and sub.proposal.target.scope() == conversations.active.slug
            ):
                home = conversations.conversations.default_agent(
                    conversations.active.department
                ) or conversations.conversations.get(GHOST)
                if home is not None:
                    await _transfer(
                        params,
                        conversations,
                        home,
                        handoff="",
                        result=result,
                        outcome=(
                            f"They just retired the agent {conversations.active.name!r} at "
                            "their request; that is done, do not retire or switch again."
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
                # ADR 0030: unanswered, it waits on the operator's waiting list.
                "waits_minutes": int(gate.pending_ttl.total_seconds() // 60),
            }
        )

    if any_ops:
        llm.register_function(CHANGE_TOOL, change)

    if conversations is not None:
        session_schemas.append(
            FunctionSchema(
                name=SWITCH_TOOL,
                description=(
                    "Transfer the operator to a department or to one of its agents by name "
                    '("marketing", "the brand agent", "ghost" to come back). The transfer is '
                    "announced and the agent greets them; say nothing else yourself. "
                    "If nothing matches, the result says so: then ask in one short "
                    "sentence whether to hire an agent for it, and on yes call change with "
                    "conversations.create."
                ),
                properties={
                    "name": {"type": "string", "description": "The department or agent to go to."},
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
                        "hint": "ask whether to hire an agent with that name",
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

    session_schemas.append(
        FunctionSchema(
            name=BRING_UP_TOOL,
            description=(
                "Take up a change waiting on the operator's waiting list (its 'change' id "
                "from waiting_list) when they ask about it: returns 'done' if their words "
                "already asked for exactly it, else 'needs_confirmation' with a summary to "
                "read out in one short sentence before you stop."
            ),
            properties={"change": {"type": "string"}},
            required=["change"],
        )
    )

    async def bring_up(params: FunctionCallParams) -> None:
        change_id = str(params.arguments.get("change") or "")
        record(BRING_UP_TOOL, dict(params.arguments))
        p = gate.bring_back(change_id, session_id)
        if p is None:
            await params.result_callback({"error": "That change isn't waiting any more."})
            return
        # "Yes, close seventeen": the operator's own words may already be the yes (ADR 0017).
        request, before = last_request()
        outcome = await gate.confirm_by_request(session_id, request, question=before)
        if outcome is not None:
            await params.result_callback(
                {"status": "done", "result": outcome.summary(), "state_now": _state(outcome)}
            )
            await events.emit("outcome", operation="confirmed", status=outcome.status)
            return
        await params.result_callback({"status": "needs_confirmation", "summary": p.preview})

    llm.register_function(BRING_UP_TOOL, bring_up)

    session_schemas.append(
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
        session_schemas.append(
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
    if conversations is not None:
        conversations.tool_schemas = schemas_for
    return schemas_for(active_sources())


async def _transfer(
    params: FunctionCallParams,
    conversations: ConversationRuntime,
    target,
    *,
    handoff: str,
    result: dict[str, Any],
    outcome: str = "",
    rehome: bool = False,
) -> None:
    """Return the tool result without a model run, then, once the aggregator has
    appended it to the context, feed the transfer frames in at the pipeline source.
    Not out of the LLM service: a frame the LLM pushes never passes through the LLM
    itself (the system prompt would not change), and anything pushed downstream
    queues behind the audio being played."""
    worker = getattr(params, "pipeline_worker", None)
    frames = await conversations.switch(target, handoff=handoff, outcome=outcome, rehome=rehome)

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


def _wrap_read(
    handler: Callable[[dict[str, Any]], Awaitable[dict[str, Any]]],
    record: OnCall,
    probe: LatencyProbe = NULL_PROBE,
):
    async def run(params: FunctionCallParams) -> None:
        args = dict(params.arguments)
        record(params.function_name, args)
        began = time.perf_counter()
        status = "ok"
        try:
            result = await handler(args)
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
