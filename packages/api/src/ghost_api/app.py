"""The FastAPI app. Local only, over a Unix socket; the daemon owns the state and
implements ``Controller``; this module only translates HTTP into calls on it.

What never passes through here: passive audio or passive transcripts (the idle-audio
rule), secret values (write-only), or a way for the model to reach any of it.
"""

from __future__ import annotations

import asyncio
import dataclasses
import subprocess
from collections.abc import Sequence
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Protocol

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from ghost_core import (
    AutonomyPolicy,
    ChangeSet,
    GateError,
    Outcome,
    Proposal,
    Store,
    ToolSource,
    WriteGate,
)
from ghost_core.policy import PolicyError
from loguru import logger

from ghost_api.files import read_env, update_env
from ghost_api.models import (
    API_VERSION,
    Ack,
    AuditEntry,
    AuditEventView,
    AuditPage,
    DepartmentCreate,
    DepartmentView,
    DeviceInfo,
    DevicesView,
    Event,
    HookInfo,
    ItemKey,
    JournalEntry,
    McpScopeInfo,
    McpServerInfo,
    McpToolInfo,
    McpWriteInfo,
    ModelsView,
    OperationInfo,
    OutcomeView,
    PendingProposal,
    PhonePairing,
    PhoneView,
    PolicyResult,
    PolicyUpdate,
    PolicyView,
    ReadToolInfo,
    RouteName,
    RoutesView,
    RuleAdd,
    SayRequest,
    SecretState,
    SettingsResult,
    SettingsUpdate,
    SettingsView,
    SkillAdd,
    SkillBody,
    SkillInfo,
    SnoozeRequest,
    SourceInfo,
    Status,
    WaitingItemView,
    WaitingView,
    WakeRequest,
    WatchInfo,
)
from ghost_api.state import DaemonState, now


class Controller(Protocol):
    """What the daemon lets the API do. Every method is safe to call at any time."""

    async def wake(self, first_utterance: str | None, conversation: str | None = None) -> bool: ...
    async def sleep(self) -> bool: ...
    async def say(self, text: str, conversation: str | None = None) -> bool: ...
    async def confirm_pending(self) -> Outcome | None: ...
    async def cancel_pending(self) -> bool: ...
    # a change on the waiting list, by id (ADR 0030); with or without a session
    async def confirm_proposal(self, proposal_id: str) -> Outcome | None: ...
    async def cancel_proposal(self, proposal_id: str) -> bool: ...
    def reload_policy(self) -> AutonomyPolicy: ...
    def apply_settings(self, values: dict[str, str | None]) -> list[str]: ...  # keys live now
    # conversations (ADR 0014)
    async def switch_conversation(self, slug: str) -> bool: ...  # in a session only
    async def compact_conversation(self, slug: str) -> bool: ...  # asks the model, in session
    def voices(self) -> list[str]: ...
    async def sample_voice(self, voice: str, speed: float, text: str | None) -> bool: ...
    # the paired phone (ADR 0016)
    def phone_pairing(self) -> PhonePairing | None: ...  # None when the listener is off
    def phone_view(self) -> PhoneView: ...
    async def forget_phone(self) -> bool: ...  # True if a phone was paired


class Deps:
    def __init__(
        self,
        *,
        state: DaemonState,
        control: Controller,
        gate: WriteGate,
        store: Store,
        sources: Sequence[ToolSource],
        policy_path: Path,
        settings_path: Path,
        env_path: Path,
        vault_env_path: Path,
        infra_dir: Path,
        cache_dir: Path,
        input_device: str | None,
        output_device: str | None,
        stt_model: str,
        conversations: Any = None,  # ghost_core.conversations.Conversations
        mcp: Any = None,  # ghost_core.mcp.Servers: Ghost's own
        skills: Any = None,  # ghost_core.skills.Skills
        departments: Any = None,  # ghost_core.departments.Departments
        fleet: Any = None,  # ghost_core.departments.Fleet
        department_tools: Any = None,  # ghost_core.department_tools.DepartmentTools
        waiting: Any = None,  # ghost_core.waiting.WaitingList
        watches: Any = None,  # ghost_core.watches.WatchRunner
        hooks: Any = None,  # ghost_core.hooks.HookIntake
    ) -> None:
        self.waiting = waiting
        self.watches = watches
        self.hooks = hooks
        self.conversations = conversations
        self.mcp = mcp
        self.skills = skills
        self.departments = departments
        self.fleet = fleet
        self.department_tools = department_tools
        self.state = state
        self.control = control
        self.gate = gate
        self.store = store
        self.sources = list(sources)
        self.policy_path = policy_path
        self.settings_path = settings_path
        self.env_path = env_path
        self.vault_env_path = vault_env_path
        self.infra_dir = infra_dir
        self.cache_dir = cache_dir
        self.input_device = input_device
        self.output_device = output_device
        self.stt_model = stt_model


def mcp_secret_names(values: dict[str, str]) -> set[str]:
    """The secret names the operator listed for MCP servers (GHOST_MCP_SECRETS, a comma
    list): settable from the desktop like the built-in ones, never readable."""
    raw = values.get("GHOST_MCP_SECRETS", "") or ""
    return {n.strip() for n in raw.split(",") if n.strip() and n.strip().isupper()}


def pending_view(p: Proposal, at: datetime | None = None) -> PendingProposal:
    at = at or now()
    return PendingProposal(
        id=p.id,
        ability=p.ability,
        operation=p.operation,
        target=p.target.spoken(),
        summary=p.preview,
        rationale=p.rationale,
        expires_at=p.expires_at,
        expires_in_secs=max(0.0, (p.expires_at - at).total_seconds()),
    )


def outcome_view(o: Outcome) -> OutcomeView:
    return OutcomeView(
        proposal_id=o.proposal_id,
        target=o.target.spoken(),
        status=o.status,
        applied=list(o.applied),
        failed_step=o.failed_step,
        error=o.error,
        state_now=o.readback.spoken() if o.readback else None,
        summary=o.summary(),
    )


def status_snapshot(deps: Deps) -> Status:
    """The one place a ``Status`` is built: the control API, its event stream and the
    phone listener's ``/hello`` all use it."""
    st = deps.state
    pending = None
    if st.session_id:
        p = deps.gate.pending(st.session_id)
        pending = pending_view(p) if p else None
    return Status(
        phase=st.phase,
        session_id=st.session_id,
        models_ready=st.models_ready,
        uptime_secs=st.uptime_secs,
        version=st.version,
        wake_mode=st.wake_mode,
        servers=list(deps.mcp.servers) if deps.mcp is not None else [],
        pending=pending,
        conversation=st.conversation,
        department=st.department,
        session_origin=st.session_origin,  # type: ignore[arg-type]
        ending=st.ending,
        waiting=st.waiting,
    )


def waiting_view(deps: Deps) -> WaitingView:
    """The waiting list (ADR 0029), re-read now: the control API and the phone listener
    both serve this."""
    if deps.waiting is None:
        return WaitingView(items=[], count=0)
    at = now()
    items = deps.waiting.refresh(at)
    changes = {p.id: p for p in deps.gate.waiting(at)}
    out = []
    for i in items:
        change = changes.get(str(i.fields.get("proposal"))) if i.source == "gate" else None
        out.append(
            WaitingItemView(
                key=i.key,
                source=i.source,
                department=i.department,
                title=i.title,
                link=i.link,
                state=i.state(at),
                first_seen=i.first_seen,
                changed_at=i.changed_at,
                age_secs=i.age_seconds(at),
                delivered_at=i.delivered_at,
                delivered_via=i.delivered_via,
                acknowledged_at=i.acknowledged_at,
                snoozed_until=i.snoozed_until,
                change=pending_view(change, at) if change else None,
            )
        )
    return WaitingView(items=out, count=sum(1 for i in items if i.needs_alerting(at)))


def audit_page(
    store: Store,
    *,
    limit: int,
    offset: int,
    ability: str | None = None,
    status: str | None = None,
    session_id: str | None = None,
) -> AuditPage:
    """A page of the audit log, newest first. Shared by the control API and the phone."""
    rows, total = store.list_proposals(
        limit=limit, offset=offset, ability=ability, status=status, session_id=session_id
    )
    items = [
        AuditEntry(
            id=p.id,
            session_id=p.session_id,
            ability=p.ability,
            operation=p.operation,
            target=p.target.spoken(),
            summary=p.preview,
            autonomy=p.autonomy,
            status=p.status.value,
            created_at=p.created_at,
            conversation=p.conversation,
            department=p.department,
            events=[
                AuditEventView(at=at, event=ev, detail=detail)
                for at, ev, detail in store.events(p.id)
            ],
        )
        for p in rows
    ]
    return AuditPage(items=items, total=total, limit=limit, offset=offset)


def create_app(deps: Deps) -> FastAPI:
    app = FastAPI(
        title="Ghost control API",
        version=API_VERSION,
        description=(
            "Local control surface for ghostd over a Unix socket. Desktop and mobile apps "
            "talk to this; the model never does. Events stream over /events."
        ),
    )
    st = deps.state

    def status() -> Status:
        return status_snapshot(deps)

    st.set_status_provider(status)
    if deps.conversations is not None:
        from ghost_api.conversations import add_conversation_routes

        add_conversation_routes(app, deps)

    # --- status + control -------------------------------------------------------

    @app.get("/status", response_model=Status, tags=["session"])
    async def get_status() -> Status:
        return status()

    @app.post("/wake", response_model=Ack, tags=["session"])
    async def wake(req: WakeRequest | None = None) -> Ack:
        ok = await deps.control.wake(req.text if req else None, req.conversation if req else None)
        return Ack(ok=ok, detail=None if ok else "not passive, or no such line")

    @app.post("/sleep", response_model=Ack, tags=["session"])
    async def sleep() -> Ack:
        ok = await deps.control.sleep()
        return Ack(ok=ok, detail=None if ok else "no session")

    @app.post("/session/say", response_model=Ack, tags=["session"])
    async def say(req: SayRequest) -> Ack:
        ok = await deps.control.say(req.text, req.conversation)
        return Ack(ok=ok, detail=None if ok else "daemon is still starting")

    @app.get("/session/turns", response_model=list[Any], tags=["session"])
    async def turns() -> list[Any]:
        return list(st.turns)

    # --- pending proposal --------------------------------------------------------

    @app.get("/proposals/pending", response_model=PendingProposal | None, tags=["writes"])
    async def pending() -> PendingProposal | None:
        return status().pending

    @app.post("/proposals/pending/confirm", response_model=OutcomeView, tags=["writes"])
    async def confirm() -> OutcomeView:
        outcome = await deps.control.confirm_pending()
        if outcome is None:
            raise HTTPException(409, "nothing pending")
        return outcome_view(outcome)

    @app.post("/proposals/pending/cancel", response_model=Ack, tags=["writes"])
    async def cancel() -> Ack:
        return Ack(ok=await deps.control.cancel_pending())

    @app.post("/proposals/{proposal_id}/confirm", response_model=OutcomeView, tags=["writes"])
    async def confirm_one(proposal_id: str) -> OutcomeView:
        """A Confirm on a change on the waiting list (ADR 0030): the stale check runs."""
        outcome = await deps.control.confirm_proposal(proposal_id)
        if outcome is None:
            raise HTTPException(409, "that change isn't waiting")
        return outcome_view(outcome)

    @app.post("/proposals/{proposal_id}/cancel", response_model=Ack, tags=["writes"])
    async def cancel_one(proposal_id: str) -> Ack:
        return Ack(ok=await deps.control.cancel_proposal(proposal_id))

    # --- the waiting list (ADR 0029) ----------------------------------------------

    @app.get("/waiting", response_model=WaitingView, tags=["waiting"])
    async def waiting() -> WaitingView:
        return waiting_view(deps)

    @app.post("/waiting/ack", response_model=Ack, tags=["waiting"])
    async def acknowledge(body: ItemKey) -> Ack:
        """The operator heard it: it stops alerting until it changes. Not a gate write;
        it changes Ghost's own bookkeeping only."""
        ok = deps.waiting is not None and deps.waiting.acknowledge(body.key)
        return Ack(ok=bool(ok))

    @app.post("/waiting/snooze", response_model=Ack, tags=["waiting"])
    async def snooze(body: SnoozeRequest) -> Ack:
        until = now() + timedelta(minutes=body.minutes)
        ok = deps.waiting is not None and deps.waiting.snooze(body.key, until)
        return Ack(ok=bool(ok))

    # --- audit ---------------------------------------------------------------------

    @app.get("/audit", response_model=AuditPage, tags=["writes"])
    async def audit(
        limit: int = Query(50, ge=1, le=500),
        offset: int = Query(0, ge=0),
        ability: str | None = None,
        status_: str | None = Query(None, alias="status"),
        session_id: str | None = None,
    ) -> AuditPage:
        return audit_page(
            deps.store,
            limit=limit,
            offset=offset,
            ability=ability,
            status=status_,
            session_id=session_id,
        )

    # --- the paired phone (ADR 0016) -------------------------------------------------

    @app.post("/phone/pairing", response_model=PhonePairing, tags=["phone"])
    async def phone_pairing() -> PhonePairing:
        """A new six-digit code for the phone, good for two minutes and five tries. It
        replaces any earlier code. 409 when GHOST_PHONE_LISTEN is off."""
        pairing = deps.control.phone_pairing()
        if pairing is None:
            raise HTTPException(409, "the phone listener is off (set GHOST_PHONE_LISTEN)")
        return pairing

    @app.get("/phone", response_model=PhoneView, tags=["phone"])
    async def phone() -> PhoneView:
        return deps.control.phone_view()

    @app.delete("/phone", response_model=Ack, tags=["phone"])
    async def forget_phone() -> Ack:
        """Forget the paired phone; its signatures stop working at once."""
        ok = await deps.control.forget_phone()
        return Ack(ok=ok, detail=None if ok else "no phone paired")

    # --- mcp servers (ADR 0022) ----------------------------------------------------

    def server_info(name: str, spec, servers, department: str | None) -> McpServerInfo:
        live = servers.servers.get(name) if servers is not None else None
        autonomy = deps.gate.policy_for(department)
        source = name if department in (None, "ghost") else f"{department}/{name}"
        offered = {t.name for t in live.offered()} if live else set()
        ops = {o.name: o for o in live.operations()} if live else {}
        writes = []
        for op_name, w in spec.writes.items():
            level = autonomy.resolve(source, op_name, risk=w.risk)  # type: ignore[arg-type]
            built = ops.get(op_name)
            writes.append(
                McpWriteInfo(
                    operation=op_name,
                    tool=w.tool,
                    risk=w.risk,
                    autonomy=level.value,
                    asked_by=list(w.asked_by),
                    target=list(w.target),
                    read=w.read.tool if w.read else None,
                    offered=built is not None and level.value != "deny",
                    properties=built.properties if built else {},
                    required=list(built.required) if built else [],
                )
            )
        return McpServerInfo(
            name=name,
            transport=spec.transport,
            connected=live is not None,
            error=servers.failed.get(name) if servers is not None else "not started",
            tools=[
                McpToolInfo(name=t.name, description=t.description, offered=t.name in offered)
                for t in (live.tools if live else [])
            ],
            writes=writes,
            scope=McpScopeInfo(
                kind=spec.scope.kind,
                arguments=list(spec.scope.arguments),
                items=list(spec.scope.items),
            )
            if spec.scope
            else None,
            department=None if department in (None, "ghost") else department,
        )

    @app.get("/mcp", response_model=list[McpServerInfo], tags=["mcp"])
    async def mcp_servers() -> list[McpServerInfo]:
        """Ghost's servers, then each department's own (ADR 0024), connected or not."""
        out = []
        if deps.mcp is not None:
            for name, spec in deps.mcp.specs.items():
                out.append(server_info(name, spec, deps.mcp, None))
        if deps.departments is not None and deps.fleet is not None:
            for d in deps.departments.list():
                if d.is_ghost:
                    continue
                own = deps.fleet.own.get(d.slug)
                for name, spec in d.servers.items():
                    out.append(server_info(name, spec, own, d.slug))
        return out

    # --- departments (ADR 0024) --------------------------------------------------------

    def department_view(d) -> DepartmentView:
        convs = deps.conversations
        agents = (
            [p.slug for p in convs.agents_of(d.slug, include_retired=True)]
            if convs is not None
            else []
        )
        agents.sort(key=lambda s: (s != d.slug, s))
        skills = (
            [s.name for s in deps.skills.list(d.skill_places())] if deps.skills is not None else []
        )
        return DepartmentView(
            slug=d.slug,
            title=d.title,
            aliases=list(d.aliases),
            path=str(d.path),
            root=str(d.root) if d.root else None,
            voice=d.voice,
            is_ghost=d.is_ghost,
            rules_lead=d.rules_lead(),
            rules=d.rules(),
            memory=d.memory(),
            servers=list(d.servers),
            uses=list(d.uses),
            servers_up=deps.fleet.is_up(d.slug) if deps.fleet is not None else d.is_ghost,
            skills=skills,
            docs=d.docs(),
            agents=agents,
            problems=[
                *d.problems,
                *(deps.fleet.failures(d.slug) if deps.fleet is not None else []),
            ],
            watches=[watch_info(w) for w in getattr(d, "watches", {}).values()],
            hooks=[hook_info(h) for h in getattr(d, "hooks", {}).values()],
        )

    def hook_info(h) -> HookInfo:
        intake = deps.hooks
        st = intake.state.get(h.path) if intake is not None else None
        return HookInfo(
            name=h.name,
            path=h.path,
            verify=h.verify,
            events=list(h.events),
            on=intake is not None and h.path in intake.hooks,
            off_reason=intake.off.get(h.path) if intake is not None else "the listener is off",
            accepted=dict(st.accepted) if st else {},
            rejected=dict(st.rejected) if st else {},
            last=st.last if st else None,
        )

    def watch_info(w) -> WatchInfo:
        st = deps.watches.state.get(w.source) if deps.watches is not None else None
        return WatchInfo(
            name=w.name,
            server=w.server,
            read=w.read,
            every_secs=w.every,
            steps=["check", *(["jev"] if w.jev else []), *(["model"] if w.model else [])],
            response=w.response,
            urgency=w.urgency,
            last_run=st.last_run if st else None,
            last_ok=st.last_ok if st else None,
            error=st.error if st else None,
            seen=st.seen if st else 0,
            open=st.open if st else 0,
        )

    @app.get("/departments", response_model=list[DepartmentView], tags=["departments"])
    async def departments() -> list[DepartmentView]:
        if deps.departments is None:
            return []
        deps.departments.refresh()
        return [department_view(d) for d in deps.departments.list()]

    @app.get("/departments/{slug}", response_model=DepartmentView, tags=["departments"])
    async def department(slug: str) -> DepartmentView:
        d = deps.departments.get(slug) if deps.departments is not None else None
        if d is None:
            raise HTTPException(404, f"no department {slug!r}")
        return department_view(d)

    @app.post("/departments/{slug}/rules", response_model=DepartmentView, tags=["departments"])
    async def add_rule(slug: str, req: RuleAdd) -> DepartmentView:
        """Append a rule to the department's AGENTS.md, verbatim, through the gate: the
        operator's own click, audited like the spoken form (ADR 0024)."""
        from ghost_core.department_tools import SOURCE as DEPARTMENTS
        from ghost_core.department_tools import AddRule, DepartmentTarget

        d = deps.departments.get(slug) if deps.departments is not None else None
        if d is None:
            raise HTTPException(404, f"no department {slug!r}")
        if deps.department_tools is None:
            raise HTTPException(503, "departments are not available")
        try:
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability=DEPARTMENTS,
                operation="add_rule",
                target=DepartmentTarget(slug=d.slug, title=d.title),
                changes=ChangeSet(steps=(AddRule(text=req.text.strip()),)),
                rationale="a rule added in the desktop app",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not applied")
        deps.departments.refresh()
        return department_view(deps.departments.get(slug))

    @app.get("/departments/{slug}/journal", response_model=list[JournalEntry], tags=["departments"])
    async def department_journal(
        slug: str, limit: int = Query(30, ge=1, le=200)
    ) -> list[JournalEntry]:
        """What the department's agents did, newest first, from the audit."""
        d = deps.departments.get(slug) if deps.departments is not None else None
        if d is None:
            raise HTTPException(404, f"no department {slug!r}")
        if deps.department_tools is None:
            return []
        return [JournalEntry(**e) for e in deps.department_tools.journal(slug, limit=limit)]

    @app.post("/departments", response_model=DepartmentView, tags=["departments"])
    async def create_department(req: DepartmentCreate) -> DepartmentView:
        """A new department folder: the operator's action, with a short AGENTS.md, an
        empty mcp.json and its department.json. The model never calls this."""
        if deps.departments is None:
            raise HTTPException(503, "departments are not available")
        from ghost_core.departments import DepartmentError

        try:
            d = deps.departments.create(req.name, aliases=req.aliases, voice=req.voice)
        except DepartmentError as e:
            raise HTTPException(409, str(e)) from None
        if deps.conversations is not None:
            deps.conversations.sync()  # its own agent
        deps.control.reload_policy()  # its levels join the gate
        return department_view(d)

    # --- skills (ADR 0023) ------------------------------------------------------------
    UI_SESSION = "ui"  # the desktop's own session id for audited edits (as conversations.py)

    def skill_info(skill) -> SkillInfo:
        servers = set(deps.mcp.servers) if deps.mcp is not None else set()
        return SkillInfo(
            name=skill.name,
            description=skill.description,
            source=skill.source,
            path=str(skill.path),
            requires=list(skill.requires),
            missing=skill.missing(servers),
            files=skill.files(),
            problem=skill.problem,
        )

    @app.get("/skills", response_model=list[SkillInfo], tags=["skills"])
    async def skills() -> list[SkillInfo]:
        if deps.skills is None:
            return []
        deps.skills.refresh()
        return [skill_info(s) for s in deps.skills.all()]

    @app.get("/skills/{name}", response_model=SkillBody, tags=["skills"])
    async def skill_body(name: str) -> SkillBody:
        skill = deps.skills.get(name) if deps.skills is not None else None
        if skill is None:
            raise HTTPException(404, f"no skill {name!r}")
        body = skill.body()
        return SkillBody(name=skill.name, body=body[:20000], files=skill.files())

    async def skill_change(op: str, args: dict[str, Any]) -> SkillInfo | None:
        """Add or remove through the gate at quiet: the operator's own click, audited."""
        if deps.skills is None:
            raise HTTPException(503, "skills are not available")
        operation = next((o for o in deps.skills.operations() if o.name == op), None)
        if operation is None:
            raise HTTPException(500, f"no skills operation {op}")
        try:
            prepared = await operation.prepare(args)
        except LookupError as e:
            raise HTTPException(404, str(e)) from None
        except ValueError as e:
            raise HTTPException(409, str(e)) from None
        try:
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability="skills",
                operation=op,
                target=prepared.target,
                changes=prepared.changes,
                rationale=f"{op} from the desktop app",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not applied")
        deps.skills.refresh()
        skill = deps.skills.get(prepared.target.name)
        return skill_info(skill) if skill else None

    @app.post("/skills", response_model=SkillInfo, tags=["skills"])
    async def add_skill(req: SkillAdd) -> SkillInfo:
        info = await skill_change(
            "add", {"path": req.path, "name": req.name, "replace": req.replace}
        )
        if info is None:
            raise HTTPException(500, "the skill was copied but could not be read back")
        return info

    @app.delete("/skills/{name}", response_model=Ack, tags=["skills"])
    async def remove_skill(name: str) -> Ack:
        await skill_change("remove", {"name": name})
        return Ack(ok=True, detail=None)

    # --- sources: what the gate knows (ADR 0022) ---------------------------------

    @app.get("/sources", response_model=list[SourceInfo], tags=["sources"])
    async def sources() -> list[SourceInfo]:
        autonomy = deps.gate.autonomy

        def ops(name: str, operations) -> list[OperationInfo]:
            return [
                OperationInfo(
                    name=op.name,
                    description=op.description,
                    risk=op.risk,
                    autonomy=autonomy.resolve(name, op.name, risk=op.risk).value,
                    properties=op.properties,
                    required=list(op.required),
                )
                for op in operations
            ]

        out = []
        if deps.conversations is not None:
            # The agents' own operations (ADR 0014): built in, always on, and tunable in
            # the policy file like any server's.
            from ghost_core.conversations import SOURCE as CONVERSATIONS

            out.append(
                SourceInfo(
                    name=CONVERSATIONS,
                    kind="built-in",
                    description="Your agents: hire, retire, persona, voice, memory (ADR 0014)",
                    operations=ops(CONVERSATIONS, deps.conversations.operations())
                    + [
                        OperationInfo(
                            name="switch",
                            description="Transfer the operator to a department or an agent.",
                            risk="low",
                            autonomy=autonomy.resolve(CONVERSATIONS, "switch", risk="low").value,
                            properties={"name": {"type": "string"}},
                            required=["name"],
                        )
                    ],
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description)
                        for t in deps.conversations.read_tools()
                    ],
                )
            )
        if deps.department_tools is not None:
            from ghost_core.department_tools import SOURCE as DEPARTMENTS

            out.append(
                SourceInfo(
                    name=DEPARTMENTS,
                    kind="built-in",
                    description="Your departments: a rule added, a shared fact (ADR 0024)",
                    operations=ops(DEPARTMENTS, deps.department_tools.operations()),
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description)
                        for t in deps.department_tools.read_tools()
                    ],
                )
            )
        listed = {s.name for s in deps.sources}
        if deps.mcp is not None:
            # A declared server that did not connect still has operations the policy
            # names; show them (from the declaration alone) so their levels can be set.
            for name, spec in deps.mcp.specs.items():
                if name in listed or name in deps.mcp.servers:
                    continue
                out.append(
                    SourceInfo(
                        name=name,
                        kind="server",
                        description=f"Not connected: {deps.mcp.failed.get(name, 'unknown')}",
                        operations=[
                            OperationInfo(
                                name=op,
                                description=w.description or f"{w.tool} ({op})",
                                risk=w.risk,
                                autonomy=autonomy.resolve(name, op, risk=w.risk).value,  # type: ignore[arg-type]
                                properties={},
                                required=list(w.target),
                            )
                            for op, w in spec.writes.items()
                        ],
                        scope_kind=spec.scope.kind if spec.scope else None,
                        scope_items=list(spec.scope.items) if spec.scope else [],
                    )
                )
        for src in deps.sources:
            if deps.conversations is not None and src.name == "conversations":
                continue
            department = src.name.rpartition("/")[0] or None
            policy = deps.gate.policy_for(department)
            out.append(
                SourceInfo(
                    name=src.name,
                    kind="server",
                    description=(src.prompt.split(". ", 1)[0] + ".") if src.prompt else src.name,
                    operations=[
                        OperationInfo(
                            name=op.name,
                            description=op.description,
                            risk=op.risk,
                            autonomy=policy.resolve(src.name, op.name, risk=op.risk).value,
                            properties=op.properties,
                            required=list(op.required),
                        )
                        for op in src.operations
                    ],
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description) for t in src.read_tools
                    ],
                    scope_kind=src.scope_kind,
                    scope_items=list(src.scope_items())
                    if src.scope_kind and src.scope_items
                    else [],
                    department=department,
                )
            )
        return out

    # --- policy ---------------------------------------------------------------------

    def policy_view() -> PolicyView:
        text = deps.policy_path.read_text() if deps.policy_path.exists() else ""
        p = AutonomyPolicy.parse(text, source=str(deps.policy_path)) if text else AutonomyPolicy()
        return PolicyView(
            path=str(deps.policy_path),
            exists=deps.policy_path.exists(),
            text=text,
            default=p.default.value if p.default else None,
            skills={s: {op: lv.value for op, lv in ops.items()} for s, ops in p.skills.items()},
            targets={
                s: {t: {op: lv.value for op, lv in ops.items()} for t, ops in ts.items()}
                for s, ts in p.targets.items()
            },
            routes=routes_view(p.routes),
        )

    def routes_view(r: Any) -> RoutesView:
        from ghost_core.routes import RoutePolicy

        r = r or RoutePolicy()
        engine = getattr(deps.control, "routes", None)
        available = ["digest", *(getattr(engine, "routes", {}) or {})]
        return RoutesView(
            enabled=list(r.enabled),
            escalate_after=r.escalate_after,
            quiet_hours=(
                f"{r.quiet_hours[0]:%H:%M}-{r.quiet_hours[1]:%H:%M}" if r.quiet_hours else None
            ),
            batch_secs=r.batch_secs,
            calls_per_hour=r.calls_per_hour,
            calls_per_day=r.calls_per_day,
            carry=r.carry,
            greeting=r.greeting,
            urgency=dict(r.urgency),
            available=[a for a in available if a in RouteName.__args__],  # type: ignore[attr-defined]
        )

    @app.get("/policy", response_model=PolicyView, tags=["policy"])
    async def get_policy() -> PolicyView:
        try:
            return policy_view()
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    @app.put("/policy", response_model=PolicyResult, tags=["policy"])
    async def put_policy(update: PolicyUpdate) -> PolicyResult:
        if update.text is not None:
            try:
                policy = AutonomyPolicy.parse(update.text, source="request")
            except ValueError as e:
                raise HTTPException(422, str(e)) from None
            text = update.text
        else:
            current = policy_view()
            try:
                policy = AutonomyPolicy.from_plain(
                    default=update.default if update.default is not None else current.default,
                    skills=update.skills if update.skills is not None else current.skills,
                    targets=update.targets if update.targets is not None else current.targets,
                )
            except ValueError as e:
                raise HTTPException(422, str(e)) from None
            # The levels editor doesn't know the rest of [defaults]: keep it as it was.
            kept = AutonomyPolicy.parse(current.text) if current.text else AutonomyPolicy()
            routes = kept.routes
            if update.routes is not None:
                from ghost_core.routes import RouteError, RoutePolicy

                raw = update.routes.model_dump(exclude={"available"})
                if raw.get("quiet_hours") is None:
                    raw.pop("quiet_hours")
                try:
                    routes = RoutePolicy.parse(raw)
                except RouteError as e:
                    raise HTTPException(422, str(e)) from None
            policy = dataclasses.replace(policy, pending_ttl=kept.pending_ttl, routes=routes)
            text = policy.to_toml()
        deps.policy_path.parent.mkdir(parents=True, exist_ok=True)
        deps.policy_path.write_text(text if text.endswith("\n") else text + "\n")
        deps.control.reload_policy()
        return PolicyResult(
            policy=policy_view(),
            applied=True,
        )

    # --- settings --------------------------------------------------------------------

    def settings_view() -> SettingsView:
        # ``values`` is whatever non-secret keys settings.env holds; the desktop app owns
        # the list it offers. GHOST_DEPARTMENT_ROUTING (ADR 0019) is a plain value.
        env = read_env(deps.env_path)
        values = {k: v for k, v in read_env(deps.settings_path).items() if k not in SECRET_KEYS_SET}
        return SettingsView(
            settings_path=str(deps.settings_path),
            env_path=str(deps.env_path),
            values=values,
            secrets=[
                SecretState(name=k, set=bool(env.get(k)))
                for k in [*SECRET_KEYS, *sorted(mcp_secret_names(values) - SECRET_KEYS_SET)]
            ],
            vault_managed=deps.vault_env_path.exists(),
        )

    @app.get("/settings", response_model=SettingsView, tags=["settings"])
    async def get_settings() -> SettingsView:
        return settings_view()

    @app.put("/settings", response_model=SettingsResult, tags=["settings"])
    async def put_settings(update: SettingsUpdate) -> SettingsResult:
        applied: list[str] = []
        try:
            if update.values:
                bad = [k for k in update.values if k in SECRET_KEYS_SET]
                if bad:
                    raise HTTPException(422, f"{bad} are secrets; send them under 'secrets'")
                update_env(deps.settings_path, update.values, mode=0o600)
                # settings.env is folded into env by the renderer; mirror it now so a
                # restart without the renderer still sees the new values.
                update_env(deps.env_path, update.values, mode=0o600)
                applied = deps.control.apply_settings(update.values)
            if update.secrets:
                allowed = SECRET_KEYS_SET | mcp_secret_names(read_env(deps.settings_path))
                bad = [k for k in update.secrets if k not in allowed]
                if bad:
                    raise HTTPException(422, f"unknown secrets {bad}")
                update_env(deps.env_path, update.secrets, mode=0o600)
                logger.info(f"settings: secrets updated: {sorted(update.secrets)}")
        except ValueError as e:
            raise HTTPException(422, str(e)) from None
        pending_values = [k for k in (update.values or {}) if k not in applied]
        return SettingsResult(
            settings=settings_view(),
            restart_required=bool(pending_values) or bool(update.secrets),
            applied=applied,
        )

    # --- devices + setup ----------------------------------------------------------------

    @app.get("/devices", response_model=DevicesView, tags=["setup"])
    async def devices() -> DevicesView:
        import sounddevice as sd

        raw = sd.query_devices()
        try:
            din, dout = sd.default.device
        except Exception:  # noqa: BLE001
            din, dout = -1, -1
        items = [
            DeviceInfo(
                index=i,
                name=d["name"],
                max_input_channels=int(d["max_input_channels"]),
                max_output_channels=int(d["max_output_channels"]),
                default_samplerate=float(d["default_samplerate"]),
                is_default_input=(i == din),
                is_default_output=(i == dout),
            )
            for i, d in enumerate(raw)
        ]
        return DevicesView(
            devices=items,
            input_device=deps.input_device,
            output_device=deps.output_device,
            echo_cancel_active=_unit_active("ghost-echo-cancel.service"),
            echo_cancel_source_present=any(d["name"] == "ghost.mic" for d in raw),
        )

    @app.post("/setup/echo-cancel", response_model=Ack, tags=["setup"])
    async def setup_echo_cancel(uninstall: bool = False) -> Ack:
        script = deps.infra_dir / "pipewire" / "install.sh"
        if not script.exists():
            raise HTTPException(500, f"installer not found at {script}")
        args = [str(script)] + (["--uninstall"] if uninstall else [])
        proc = await asyncio.create_subprocess_exec(
            *args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
        )
        out, _ = await proc.communicate()
        detail = out.decode(errors="replace")[-2000:]
        return Ack(ok=proc.returncode == 0, detail=detail)

    @app.get("/setup/models", response_model=ModelsView, tags=["setup"])
    async def models() -> ModelsView:
        return ModelsView(
            ready=st.models_ready,
            cache_dir=str(deps.cache_dir),
            kokoro_present=(deps.cache_dir / "kokoro" / "kokoro-v1.0.onnx").exists(),
            stt_model=deps.stt_model,
        )

    # --- events --------------------------------------------------------------------------

    @app.get("/events/schema", response_model=Event, tags=["events"], include_in_schema=True)
    async def events_schema() -> Any:
        """Documents the payloads sent on the /events WebSocket. Not meant to be called."""
        raise HTTPException(404, "use the /events WebSocket")

    @app.websocket("/events")
    async def events(ws: WebSocket) -> None:
        await ws.accept()
        q = st.bus.subscribe()
        try:
            await ws.send_text(Status.model_dump_json(status()))  # first message: plain Status
            while True:
                await ws.send_text(await q.get())
        except WebSocketDisconnect:
            pass
        finally:
            st.bus.unsubscribe(q)

    return app


SECRET_KEYS_SET = frozenset(
    (
        "ANTHROPIC_AUTH_TOKEN",
        "GITHUB_TOKEN",
        "TYPESAFE_API_KEY",
        "OPENROUTER_API_KEY",
        "HF_TOKEN",
        "ELEVENLABS_API_KEY",  # the cloud voice (GHOST_TTS_ENGINE=elevenlabs)
    )
)
SECRET_KEYS = tuple(sorted(SECRET_KEYS_SET))


def _unit_active(unit: str) -> bool:
    try:
        r = subprocess.run(
            ["systemctl", "--user", "is-active", unit], capture_output=True, text=True, timeout=3
        )
        return r.stdout.strip() == "active"
    except (OSError, subprocess.TimeoutExpired):
        return False


__all__ = [
    "Controller",
    "Deps",
    "PolicyError",
    "audit_page",
    "create_app",
    "outcome_view",
    "pending_view",
    "status_snapshot",
]
