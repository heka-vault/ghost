"""The FastAPI app. Local only, over a Unix socket; the daemon owns the state and
implements ``Controller``; this module only translates HTTP into calls on it.

What never passes through here: passive audio or passive transcripts (the idle-audio
rule), secret values (write-only), or a way for the model to reach any of it.
"""

from __future__ import annotations

import asyncio
import subprocess
from collections.abc import Sequence
from datetime import datetime
from pathlib import Path
from typing import Any, Protocol

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from ghost_core import AutonomyPolicy, LoadedAbility, Outcome, Proposal, Store, WriteGate
from ghost_core.policy import PolicyError
from loguru import logger

from ghost_api.files import read_env, update_env
from ghost_api.models import (
    API_VERSION,
    AbilityInfo,
    Ack,
    AuditEntry,
    AuditEventView,
    AuditPage,
    DeviceInfo,
    DevicesView,
    Event,
    ModelsView,
    OperationInfo,
    OutcomeView,
    PendingProposal,
    PolicyResult,
    PolicyUpdate,
    PolicyView,
    ReadToolInfo,
    SayRequest,
    SecretState,
    SettingsResult,
    SettingsUpdate,
    SettingsView,
    Status,
    WakeRequest,
)
from ghost_api.state import DaemonState, now


class Controller(Protocol):
    """What the daemon lets the API do. Every method is safe to call at any time."""

    async def wake(self, first_utterance: str | None) -> bool: ...
    async def sleep(self) -> bool: ...
    async def say(self, text: str) -> bool: ...
    async def confirm_pending(self) -> Outcome | None: ...
    async def cancel_pending(self) -> bool: ...
    def reload_policy(self) -> AutonomyPolicy: ...
    def apply_settings(self, values: dict[str, str | None]) -> list[str]: ...  # keys live now
    def discovered_abilities(self) -> dict[str, Any]: ...  # name -> Ability


class Deps:
    def __init__(
        self,
        *,
        state: DaemonState,
        control: Controller,
        gate: WriteGate,
        store: Store,
        abilities: Sequence[LoadedAbility],
        policy_path: Path,
        settings_path: Path,
        env_path: Path,
        vault_env_path: Path,
        infra_dir: Path,
        cache_dir: Path,
        input_device: str | None,
        output_device: str | None,
        stt_model: str,
    ) -> None:
        self.state = state
        self.control = control
        self.gate = gate
        self.store = store
        self.abilities = list(abilities)
        self.policy_path = policy_path
        self.settings_path = settings_path
        self.env_path = env_path
        self.vault_env_path = vault_env_path
        self.infra_dir = infra_dir
        self.cache_dir = cache_dir
        self.input_device = input_device
        self.output_device = output_device
        self.stt_model = stt_model


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
            abilities=[a.name for a in deps.abilities],
            pending=pending,
        )

    st.set_status_provider(status)

    # --- status + control -------------------------------------------------------

    @app.get("/status", response_model=Status, tags=["session"])
    async def get_status() -> Status:
        return status()

    @app.post("/wake", response_model=Ack, tags=["session"])
    async def wake(req: WakeRequest | None = None) -> Ack:
        ok = await deps.control.wake(req.text if req else None)
        return Ack(ok=ok, detail=None if ok else "not passive")

    @app.post("/sleep", response_model=Ack, tags=["session"])
    async def sleep() -> Ack:
        ok = await deps.control.sleep()
        return Ack(ok=ok, detail=None if ok else "no session")

    @app.post("/session/say", response_model=Ack, tags=["session"])
    async def say(req: SayRequest) -> Ack:
        ok = await deps.control.say(req.text)
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

    @app.post("/undo", response_model=OutcomeView, tags=["writes"])
    async def undo() -> OutcomeView:
        if not st.session_id:
            raise HTTPException(409, "no session")
        from ghost_core import GateError

        try:
            outcome = await deps.gate.undo(st.session_id)
        except GateError as e:
            raise HTTPException(409, str(e)) from None
        view = outcome_view(outcome)
        st.outcome(view, "undo")
        return view

    # --- audit ---------------------------------------------------------------------

    @app.get("/audit", response_model=AuditPage, tags=["writes"])
    async def audit(
        limit: int = Query(50, ge=1, le=500),
        offset: int = Query(0, ge=0),
        ability: str | None = None,
        status_: str | None = Query(None, alias="status"),
        session_id: str | None = None,
    ) -> AuditPage:
        rows, total = deps.store.list_proposals(
            limit=limit, offset=offset, ability=ability, status=status_, session_id=session_id
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
                undo_of=p.undo_of,
                created_at=p.created_at,
                events=[
                    AuditEventView(at=at, event=ev, detail=detail)
                    for at, ev, detail in deps.store.events(p.id)
                ],
            )
            for p in rows
        ]
        return AuditPage(items=items, total=total, limit=limit, offset=offset)

    # --- abilities -----------------------------------------------------------------

    @app.get("/abilities", response_model=list[AbilityInfo], tags=["abilities"])
    async def abilities() -> list[AbilityInfo]:
        loaded = {a.name: a for a in deps.abilities}
        autonomy = deps.gate.autonomy
        out = []
        for name, ab in sorted(deps.control.discovered_abilities().items()):
            la = loaded.get(name)
            c = la.contributions if la else None
            out.append(
                AbilityInfo(
                    name=name,
                    version=ab.version,
                    description=ab.description,
                    enabled=la is not None,
                    loaded=la is not None,
                    passive=ab.passive,
                    operations=[
                        OperationInfo(
                            name=op.name,
                            description=op.description,
                            risk=op.risk,
                            autonomy=autonomy.resolve(name, op.name, risk=op.risk).value,
                            properties=op.properties,
                            required=list(op.required),
                        )
                        for op in (c.operations if c else [])
                    ],
                    read_tools=[
                        ReadToolInfo(name=t.name, description=t.description)
                        for t in (c.read_tools if c else [])
                    ],
                    interceptors=[i.name for i in (c.interceptors if c else [])],
                    slots=[str(s) for s in (c.processors if c else {})],
                    listeners=len(c.listeners) if c else 0,
                    commands=[cmd.name for cmd in (c.commands if c else [])],
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
            enabled=list(p.enabled) if p.enabled is not None else None,
            passive=list(p.passive),
        )

    @app.get("/policy", response_model=PolicyView, tags=["policy"])
    async def get_policy() -> PolicyView:
        try:
            return policy_view()
        except ValueError as e:
            raise HTTPException(422, str(e)) from None

    @app.put("/policy", response_model=PolicyResult, tags=["policy"])
    async def put_policy(update: PolicyUpdate) -> PolicyResult:
        before = AutonomyPolicy.load(deps.policy_path)
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
                    enabled=update.enabled if update.enabled is not None else current.enabled,
                    passive=update.passive if update.passive is not None else current.passive,
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
            restart_required=tuple(policy.enabled or ()) != tuple(before.enabled or ()),
        )

    # --- settings --------------------------------------------------------------------

    def settings_view() -> SettingsView:
        env = read_env(deps.env_path)
        values = {k: v for k, v in read_env(deps.settings_path).items() if k not in SECRET_KEYS_SET}
        return SettingsView(
            settings_path=str(deps.settings_path),
            env_path=str(deps.env_path),
            values=values,
            secrets=[SecretState(name=k, set=bool(env.get(k))) for k in SECRET_KEYS],
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
                bad = [k for k in update.secrets if k not in SECRET_KEYS_SET]
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


__all__ = ["Controller", "Deps", "PolicyError", "create_app", "outcome_view", "pending_view"]
