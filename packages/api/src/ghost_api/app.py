"""The FastAPI app. Local only, over a Unix socket; the daemon owns the state and
implements ``Controller``; this module only translates HTTP into calls on it.

What never passes through here: passive audio or passive transcripts (the idle-audio
rule), secret values (write-only), or a way for the model to reach any of it.
"""

from __future__ import annotations

import asyncio
import re
import subprocess
from collections.abc import Sequence
from datetime import datetime
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
from ghost_core.conversations import ConversationTarget
from ghost_core.policy import PolicyError
from loguru import logger

from ghost_api.files import read_env, update_env
from ghost_api.models import (
    API_VERSION,
    Ack,
    AuditEntry,
    AuditEventView,
    AuditPage,
    ConversationDetail,
    DeviceInfo,
    DevicesView,
    Event,
    GrantView,
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
    SayRequest,
    SecretState,
    SettingsResult,
    SettingsUpdate,
    SettingsView,
    SkillAdd,
    SkillBody,
    SkillInfo,
    SourceInfo,
    Status,
    TemplateApply,
    TemplateView,
    ToolkitUpdate,
    ToolkitView,
    WakeRequest,
)
from ghost_api.state import DaemonState, now


class Controller(Protocol):
    """What the daemon lets the API do. Every method is safe to call at any time."""

    async def wake(self, first_utterance: str | None, conversation: str | None = None) -> bool: ...
    async def sleep(self) -> bool: ...
    async def say(self, text: str, conversation: str | None = None) -> bool: ...
    async def confirm_pending(self) -> Outcome | None: ...
    async def cancel_pending(self) -> bool: ...
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
        mcp: Any = None,  # ghost_core.mcp.Servers
        skills: Any = None,  # ghost_core.skills.Skills
        templates: Any = None,  # ghost_core.templates.Templates
    ) -> None:
        self.conversations = conversations
        self.mcp = mcp
        self.skills = skills
        self.templates = templates
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
        session_origin=st.session_origin,  # type: ignore[arg-type]
        ending=st.ending,
    )


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
            toolkit=p.toolkit,
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

    @app.get("/mcp", response_model=list[McpServerInfo], tags=["mcp"])
    async def mcp_servers() -> list[McpServerInfo]:
        servers = deps.mcp
        if servers is None:
            return []
        autonomy = deps.gate.autonomy
        out = []
        for name, spec in servers.specs.items():
            live = servers.servers.get(name)
            offered = {t.name for t in live.offered()} if live else set()
            ops = {o.name: o for o in live.operations()} if live else {}
            writes = []
            for op_name, w in spec.writes.items():
                level = autonomy.resolve(name, op_name, risk=w.risk)  # type: ignore[arg-type]
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
            out.append(
                McpServerInfo(
                    name=name,
                    transport=spec.transport,
                    connected=live is not None,
                    error=servers.failed.get(name),
                    tools=[
                        McpToolInfo(
                            name=t.name, description=t.description, offered=t.name in offered
                        )
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
                )
            )
        return out

    # --- toolkits (ADR 0023) ----------------------------------------------------------

    def toolkit_views(policy: AutonomyPolicy) -> list[ToolkitView]:
        skill_names = [s.name for s in deps.skills.list()] if deps.skills is not None else None
        problems = policy.check_toolkits(skills=skill_names)
        return [
            ToolkitView(
                name=name,
                description=t.description,
                skills=list(t.skills),
                grants=[
                    GrantView(
                        server=g.server,
                        reads=list(g.reads) if g.reads is not None else None,
                        writes=list(g.writes) if g.writes is not None else None,
                        items=list(g.items) if g.items is not None else None,
                        levels=dict(g.levels),
                    )
                    for g in t.grants.values()
                ],
                problems=[m for m in problems if m.startswith(f"toolkit {name}:")],
            )
            for name, t in policy.toolkits.items()
        ]

    @app.get("/toolkits", response_model=list[ToolkitView], tags=["toolkits"])
    async def toolkits() -> list[ToolkitView]:
        try:
            return toolkit_views(AutonomyPolicy.load(deps.policy_path))
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    def save_toolkits(toolkits: dict) -> AutonomyPolicy:
        """Rewrite the policy file with these toolkits and reload the gate: the designer's
        save. Levels, targets and servers are carried over unchanged."""
        try:
            current = AutonomyPolicy.load(deps.policy_path)
        except ValueError as e:
            raise HTTPException(422, f"the policy file cannot be read: {e}") from None
        policy = AutonomyPolicy.from_plain(
            default=current.default.value if current.default else None,
            skills={
                s: {op: lv.value for op, lv in ops.items()} for s, ops in current.skills.items()
            },
            targets={
                s: {t: {op: lv.value for op, lv in ops.items()} for t, ops in ts.items()}
                for s, ts in current.targets.items()
            },
            mcp=current.mcp,
            toolkits=toolkits,
        )
        deps.policy_path.parent.mkdir(parents=True, exist_ok=True)
        deps.policy_path.write_text(policy.to_toml())
        deps.control.reload_policy()
        return AutonomyPolicy.load(deps.policy_path)

    @app.put("/toolkits/{name}", response_model=ToolkitView, tags=["toolkits"])
    async def put_toolkit(name: str, update: ToolkitUpdate) -> ToolkitView:
        from ghost_core.toolkits import Grant, ToolkitSpec

        if not re.match(r"^[a-z0-9][a-z0-9_-]{0,39}$", name):
            raise HTTPException(422, "a toolkit name is letters, digits, _ or -")
        try:
            current = AutonomyPolicy.load(deps.policy_path)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None
        spec = ToolkitSpec(
            name=name,
            description=update.description,
            skills=tuple(update.skills),
            grants={
                g.server: Grant(
                    server=g.server,
                    reads=tuple(g.reads) if g.reads is not None else None,
                    writes=tuple(g.writes) if g.writes is not None else None,
                    items=tuple(g.items) if g.items is not None else None,
                    levels=dict(g.levels),
                )
                for g in update.grants
            },
        )
        for level in (lv for g in spec.grants.values() for lv in g.levels.values()):
            if level not in ("deny", "confirm", "announce", "quiet"):
                raise HTTPException(422, f"{level!r} is not a level")
        policy = save_toolkits({**current.toolkits, name: spec})
        view = next(v for v in toolkit_views(policy) if v.name == name)
        return view

    @app.delete("/toolkits/{name}", response_model=Ack, tags=["toolkits"])
    async def delete_toolkit(name: str) -> Ack:
        try:
            current = AutonomyPolicy.load(deps.policy_path)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None
        if name not in current.toolkits:
            raise HTTPException(404, f"no toolkit {name!r}")
        save_toolkits({k: v for k, v in current.toolkits.items() if k != name})
        return Ack(ok=True, detail=None)

    # --- department templates (ADR 0023) ---------------------------------------------

    def template_view(t) -> TemplateView:
        policy = deps.gate.autonomy
        servers = list(deps.mcp.servers) if deps.mcp is not None else []
        return TemplateView(
            name=t.name,
            title=t.title,
            description=t.description,
            source=t.source,
            path=str(t.path),
            aliases=list(t.aliases),
            voice=t.voice,
            toolkits=list(t.toolkits),
            skills=[p.name for p in t.skills],
            needs=t.needs(list(policy.toolkits), servers),
        )

    @app.get("/templates", response_model=list[TemplateView], tags=["templates"])
    async def templates() -> list[TemplateView]:
        if deps.templates is None:
            return []
        deps.templates.refresh()
        return [template_view(t) for t in deps.templates.list()]

    @app.post("/templates/{name}/apply", response_model=ConversationDetail, tags=["templates"])
    async def apply_template(name: str, req: TemplateApply) -> ConversationDetail:
        """The desktop's import: the template's toolkits into the policy file, its skills
        into Ghost's skills, and a department line with its persona, each one audited."""
        from ghost_core.conversations.models import Create, slugify
        from ghost_core.skills import AddSkill, SkillTarget

        from ghost_api.conversations import conversation_detail

        if deps.templates is None or deps.conversations is None:
            raise HTTPException(503, "templates are not available")
        t = deps.templates.get(name)
        if t is None:
            raise HTTPException(404, f"no template {name!r}")
        current = AutonomyPolicy.load(deps.policy_path)
        missing = {k: v for k, v in t.toolkit_specs.items() if k not in current.toolkits}
        if missing:
            save_toolkits({**current.toolkits, **missing})
        if deps.skills is not None:
            for folder in t.skills:
                if deps.skills.get(folder.name) is not None:
                    continue
                try:
                    await deps.gate.apply_internal(
                        session_id=UI_SESSION,
                        ability="skills",
                        operation="add",
                        target=SkillTarget(name=folder.name),
                        changes=ChangeSet(steps=(AddSkill(source_path=str(folder)),)),
                        rationale=f"from the {t.title} template",
                    )
                except (GateError, PolicyError, ValueError) as e:
                    raise HTTPException(409, f"skill {folder.name}: {e}") from None
            deps.skills.refresh()
        convs = deps.conversations
        title = (req.name or t.title).strip()
        slug = slugify(title)
        if convs.get(slug) is not None:
            raise HTTPException(409, f"there is already a line called {title!r}")
        try:
            toolkits = convs.validate_toolkits(list(t.toolkits))
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability="conversations",
                operation="create",
                target=ConversationTarget(slug=slug),
                changes=ChangeSet(
                    steps=(
                        Create(
                            name=title,
                            aliases=t.aliases,
                            voice=t.voice or convs.unused_voice(),
                            speed=convs.default_speed,
                            persona=t.persona,
                            toolkits=toolkits,
                        ),
                    )
                ),
                rationale=f"from the {t.title} template",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not applied")
        return conversation_detail(convs, convs.get(slug), st.conversation)

    @app.post("/conversations/{slug}/export", response_model=TemplateView, tags=["templates"])
    async def export_template(slug: str) -> TemplateView:
        """Write a department back out as one of the operator's own templates."""
        from ghost_core.templates import export_department, load_template

        if deps.templates is None or deps.conversations is None or deps.templates.own is None:
            raise HTTPException(503, "templates are not available")
        p = deps.conversations.get(slug)
        if p is None or p.is_ghost or not p.is_department:
            raise HTTPException(404, f"{slug!r} is not a department")
        policy = deps.gate.autonomy
        specs = {k: v for k, v in policy.toolkits.items() if k in p.toolkits}
        skill_names = (
            {s for k in p.toolkits for s in policy.toolkits.get(k).skills} if p.toolkits else set()
        )
        folders = [
            s.path for s in (deps.skills.list() if deps.skills else []) if s.name in skill_names
        ]
        folder = export_department(
            deps.templates.own,
            name=p.slug,
            title=p.name,
            description="",
            aliases=p.aliases,
            voice=p.voice,
            persona=deps.conversations.files.persona(p.slug),
            toolkits=p.toolkits,
            toolkit_specs=specs,
            skill_folders=folders,
        )
        deps.templates.refresh()
        t = load_template(folder, "ghost")
        if t is None:
            raise HTTPException(500, "the template was written but cannot be read back")
        return template_view(t)

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
        )

    @app.get("/skills", response_model=list[SkillInfo], tags=["skills"])
    async def skills() -> list[SkillInfo]:
        if deps.skills is None:
            return []
        deps.skills.refresh()
        return [skill_info(s) for s in deps.skills.list()]

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
            # The lines' own operations (ADR 0014): built in, always on, and tunable in
            # the policy file like any server's.
            from ghost_core.conversations import SOURCE as CONVERSATIONS

            out.append(
                SourceInfo(
                    name=CONVERSATIONS,
                    kind="built-in",
                    description="Your lines: persona, voice, memory, scope (ADR 0014)",
                    operations=ops(CONVERSATIONS, deps.conversations.operations())
                    + [
                        OperationInfo(
                            name="switch",
                            description="Patch the operator through to another line.",
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
            out.append(
                SourceInfo(
                    name=src.name,
                    kind="server",
                    description=(src.prompt.split(". ", 1)[0] + ".") if src.prompt else src.name,
                    operations=ops(src.name, src.operations),
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description) for t in src.read_tools
                    ],
                    scope_kind=src.scope_kind,
                    scope_items=list(src.scope_items())
                    if src.scope_kind and src.scope_items
                    else [],
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
        )

    @app.get("/policy", response_model=PolicyView, tags=["policy"])
    async def get_policy() -> PolicyView:
        try:
            return policy_view()
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    @app.put("/policy", response_model=PolicyResult, tags=["policy"])
    async def put_policy(update: PolicyUpdate) -> PolicyResult:
        try:
            before = AutonomyPolicy.load(deps.policy_path)  # the servers it declares
        except ValueError:
            before = AutonomyPolicy()  # a broken file: the text path below can repair it
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
                    mcp=before.mcp,
                    toolkits=before.toolkits,
                )
            except ValueError as e:
                raise HTTPException(422, str(e)) from None
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
        # the list it offers. GHOST_CROSS_DEPARTMENT_ANSWERS, GHOST_DEPARTMENT_ROUTING and
        # GHOST_GITHUB_TOKENS (ADR 0019) are plain values: the token map names
        # environment variables, and only those variables' values are secrets.
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
