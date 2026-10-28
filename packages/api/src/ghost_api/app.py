"""The FastAPI app. Local only, over a Unix socket; the daemon owns the state and
implements ``Controller``; this module only translates HTTP into calls on it.

What never passes through here: passive audio or passive transcripts (the idle-audio
rule), secret values (write-only), or a way for the model to reach any of it.
"""

from __future__ import annotations

import asyncio
import os
import subprocess
from collections.abc import Sequence
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Protocol

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from ghost_core import (
    AutonomyPolicy,
    GateError,
    Outcome,
    Proposal,
    Store,
    ToolSource,
    WriteGate,
)
from ghost_core.outputs import list_outputs
from ghost_core.policy import PolicyError, write_routes
from loguru import logger

from ghost_api.files import read_env, update_env
from ghost_api.models import (
    API_VERSION,
    Ack,
    AuditEntry,
    AuditEventView,
    AuditPage,
    DeviceInfo,
    DevicesView,
    Event,
    HookInfo,
    ItemKey,
    ModelsView,
    OperationInfo,
    OutcomeView,
    OutputInfo,
    PendingProposal,
    PermissionsView,
    PhonePairing,
    PhoneView,
    PolicyResult,
    PolicyUpdate,
    PolicyView,
    ReadToolInfo,
    RouteName,
    RoutesView,
    RuleAdd,
    RulesView,
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
    WatchingView,
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
        apps: Any = None,  # ghost_core.apps.Apps (ADR 0034)
        catalog: Any = None,  # ghost_core.catalog.Catalog
        signins: Any = None,  # ghost_core.signin.SignIns
        skills: Any = None,  # ghost_core.skills.Skills
        folder: Any = None,  # ghost_core.folder.GhostFolder (ADR 0035)
        rules: Any = None,  # ghost_core.rules.Rules
        outputs: Any = None,  # ghost_core.outputs.Outputs
        waiting: Any = None,  # ghost_core.waiting.WaitingList
        watches: Any = None,  # ghost_core.watches.WatchRunner
        hooks: Any = None,  # ghost_core.hooks.HookIntake
        hooks_listening: Sequence[str] = (),  # the webhook listener's addresses, when on
    ) -> None:
        self.waiting = waiting
        self.watches = watches
        self.hooks = hooks
        self.hooks_listening = list(hooks_listening)
        self.conversations = conversations
        self.apps = apps
        self.catalog = catalog
        self.signins = signins
        self.skills = skills
        self.folder = folder
        self.rules = rules
        self.outputs = outputs
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


APPS_ENV = "apps.env"  # app secrets, beside env: the vault renderer rewrites env whole


def apps_env_path(deps: Deps) -> Path:
    """Where the apps' secrets are kept (ADR 0034): 0600, never in the vault's rendering."""
    return deps.env_path.parent / APPS_ENV


def app_secret_names(values: dict[str, str], apps: Any = None) -> set[str]:
    """The secret names the connected apps refer to (``${NAME}`` in apps.json, ADR 0034),
    and any the operator listed in GHOST_APP_SECRETS (a comma list; GHOST_MCP_SECRETS is
    its older name): settable from the desktop like the built-in ones, never readable."""
    raw = (values.get("GHOST_APP_SECRETS") or "") + "," + (values.get("GHOST_MCP_SECRETS") or "")
    names = {n.strip() for n in raw.split(",") if n.strip() and n.strip().isupper()}
    if apps is not None:
        for spec in apps.specs.values():
            names.update(spec.secrets())
    return names


def permissions_view(p: AutonomyPolicy, path: Path) -> PermissionsView:
    """``[permissions]`` as the desktop edits it (ADR 0034)."""
    return PermissionsView(
        path=str(path),
        mode=p.permissions.mode.value,  # type: ignore[arg-type]
        **p.permissions.rules(),
    )


def pending_view(p: Proposal, at: datetime | None = None) -> PendingProposal:
    at = at or now()
    return PendingProposal(
        id=p.id,
        source=p.ability,  # the store's column keeps its older name
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
        build=st.build,
        wake_mode=st.wake_mode,
        apps=sorted(n for n in deps.apps.apps if deps.apps.is_up(n))
        if deps.apps is not None
        else [],
        pending=pending,
        conversation=st.conversation,
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
    source: str | None = None,
    status: str | None = None,
    session_id: str | None = None,
) -> AuditPage:
    """A page of the audit log, newest first. Shared by the control API and the phone."""
    rows, total = store.list_proposals(
        limit=limit, offset=offset, ability=source, status=status, session_id=session_id
    )
    items = [
        AuditEntry(
            id=p.id,
            session_id=p.session_id,
            source=p.ability,
            operation=p.operation,
            target=p.target.spoken(),
            summary=p.preview,
            autonomy=p.autonomy,
            status=p.status.value,
            created_at=p.created_at,
            conversation=p.conversation,
            events=[
                AuditEventView(at=at, event=ev, detail=_event_detail(detail))
                for at, ev, detail in store.events(p.id)
            ],
        )
        for p in rows
    ]
    return AuditPage(items=items, total=total, limit=limit, offset=offset)


def _event_detail(detail: dict[str, Any] | None) -> dict[str, Any] | None:
    """An audit event's detail as the API says it: the source, where the store's rows
    keep the older word (ability)."""
    if not detail or "ability" not in detail:
        return detail
    return {("source" if k == "ability" else k): v for k, v in detail.items()}


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
    from ghost_api.apps import add_app_routes

    add_app_routes(app, deps)

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
        source: str | None = None,
        status_: str | None = Query(None, alias="status"),
        session_id: str | None = None,
    ) -> AuditPage:
        """Proposals, newest first; source narrows to one app or one of Ghost's own."""
        return audit_page(
            deps.store,
            limit=limit,
            offset=offset,
            source=source,
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

    # --- what Ghost watches, its rules and what it made (ADR 0035) -----------------------
    UI_SESSION = "ui"  # the desktop's own session id for audited edits (as conversations.py)

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
            app=w.app,
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

    @app.get("/watches", response_model=WatchingView, tags=["watching"])
    async def watching() -> WatchingView:
        """The watches in watches.json and the webhooks in hooks.json, each with its last
        look and its counts, re-read now; where the webhook listener is; and what is wrong
        in those files or left over in Ghost's folder (ADR 0035)."""
        if deps.folder is None:
            return WatchingView(listening=list(deps.hooks_listening))
        deps.folder.refresh()  # the daemon follows: changed watches restart
        return WatchingView(
            watches=[watch_info(w) for w in deps.folder.watches.values()],
            hooks=[hook_info(h) for h in deps.folder.hooks.values()],
            listening=list(deps.hooks_listening),
            problems=list(deps.folder.problems),
        )

    @app.get("/rules", response_model=RulesView, tags=["rules"])
    async def get_rules() -> RulesView:
        """Ghost's AGENTS.md: the rules in every agent's prompt (ADR 0035)."""
        if deps.rules is None:
            raise HTTPException(503, "rules are not available")
        return RulesView(path=str(deps.rules.path), text=deps.rules.text())

    @app.post("/rules", response_model=OutcomeView, tags=["rules"])
    async def add_rule(req: RuleAdd) -> OutcomeView:
        """Append a rule to Ghost's AGENTS.md, verbatim, through the gate: the operator's
        own click, audited like the spoken ``rules.add`` (ADR 0035). Never written here."""
        from ghost_core.rules import SOURCE as RULES

        if deps.rules is None:
            raise HTTPException(503, "rules are not available")
        try:
            prepared = deps.rules.prepare_add(req.text)
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability=RULES,
                operation="add",
                target=prepared.target,
                changes=prepared.changes,
                rationale="a rule added in the desktop app",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not applied")
        return outcome_view(out)

    @app.get("/outputs", response_model=list[OutputInfo], tags=["outputs"])
    async def outputs() -> list[OutputInfo]:
        """The files Ghost made in its out/ folder (ADR 0033), by name."""
        if deps.outputs is None:
            return []
        return [
            OutputInfo(
                name=f["name"],
                path=f["path"],
                bytes=f["bytes"],
                modified=datetime.fromtimestamp(f["modified"]).astimezone(),
            )
            for f in list_outputs(deps.outputs.config_dir)
        ]

    # --- skills (ADR 0023) ------------------------------------------------------------

    def skill_info(skill) -> SkillInfo:
        servers = (
            {n for n in deps.apps.apps if deps.apps.is_up(n)} if deps.apps is not None else set()
        )
        return SkillInfo(
            name=skill.name,
            description=skill.description,
            source=skill.source,
            path=str(skill.path),
            requires=skill.apps(),
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

    # --- sources: what the gate knows (ADR 0034) ---------------------------------

    @app.get("/sources", response_model=list[SourceInfo], tags=["sources"])
    async def sources() -> list[SourceInfo]:
        autonomy = deps.gate.autonomy

        def ops(name: str, operations) -> list[OperationInfo]:
            return [
                OperationInfo(
                    name=op.name,
                    description=op.description,
                    risk=op.risk,
                    autonomy=autonomy.resolve(name, op.name).value,
                    properties=op.properties,
                    required=list(op.required),
                )
                for op in operations
            ]

        out = []
        if deps.conversations is not None:
            # The agents' own operations (ADR 0014): built in, always on, and named in
            # the permissions like any app's.
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
                            description="Transfer the operator to one of their agents.",
                            risk="low",
                            autonomy=autonomy.resolve(CONVERSATIONS, "switch").value,
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
        if deps.rules is not None:
            from ghost_core.rules import SOURCE as RULES

            out.append(
                SourceInfo(
                    name=RULES,
                    kind="built-in",
                    description="Your rules for every agent: a rule added (ADR 0035)",
                    operations=ops(RULES, deps.rules.operations()),
                )
            )
        apps = deps.apps.sources() if deps.apps is not None else deps.sources
        for src in apps:
            if src.name in (s.name for s in out):
                continue
            out.append(
                SourceInfo(
                    name=src.name,
                    kind="app",
                    description=src.title or src.name,
                    operations=[
                        OperationInfo(
                            name=op.name,
                            description=op.description,
                            risk=op.risk,
                            autonomy=autonomy.resolve(src.name, op.name).value,
                            properties=op.properties,
                            required=list(op.required),
                        )
                        for op in src.operations
                    ],
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description) for t in src.read_tools
                    ],
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
            permissions=permissions_view(p, deps.policy_path),
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
                AutonomyPolicy.parse(update.text, source="request")  # never a file that won't read
            except ValueError as e:
                raise HTTPException(422, str(e)) from None
            text = update.text
        else:
            current = policy_view()
            if update.routes is None:
                return PolicyResult(policy=current, applied=False)
            from ghost_core.routes import RouteError, RoutePolicy

            raw = update.routes.model_dump(exclude={"available"})
            if raw.get("quiet_hours") is None:
                raw.pop("quiet_hours")
            try:
                routes = RoutePolicy.parse(raw)
                # The routes editor doesn't know the rest of the file: only [routes]
                # changes, and everything else stays as written, comments included.
                text = write_routes(current.text, routes)
            except (RouteError, ValueError) as e:
                raise HTTPException(422, str(e)) from None
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
        # the list it offers.
        env = read_env(deps.env_path)
        values = {k: v for k, v in read_env(deps.settings_path).items() if k not in SECRET_KEYS_SET}
        env = {**read_env(apps_env_path(deps)), **env}
        return SettingsView(
            settings_path=str(deps.settings_path),
            env_path=str(deps.env_path),
            values=values,
            secrets=[
                SecretState(name=k, set=bool(env.get(k)))
                for k in [
                    *SECRET_KEYS,
                    *sorted(app_secret_names(values, deps.apps) - SECRET_KEYS_SET),
                ]
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
                allowed = SECRET_KEYS_SET | app_secret_names(
                    read_env(deps.settings_path), deps.apps
                )
                bad = [k for k in update.secrets if k not in allowed]
                if bad:
                    raise HTTPException(422, f"unknown secrets {bad}")
                own = {k: v for k, v in update.secrets.items() if k in SECRET_KEYS_SET}
                apps = {k: v for k, v in update.secrets.items() if k not in SECRET_KEYS_SET}
                if own:
                    update_env(deps.env_path, own, mode=0o600)
                if apps:  # kept out of env, which the vault renderer rewrites
                    update_env(apps_env_path(deps), apps, mode=0o600)
                    os.environ.update({k: v for k, v in apps.items() if v})
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
