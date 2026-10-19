"""The wire contract. Everything the desktop and mobile apps see is one of these.

``packages/api/openapi.json`` is generated from this module (``make api-schema``) and
``packages/client-ts`` is generated from that. Change a model here, regenerate both.
"""

from __future__ import annotations

import enum
from datetime import datetime
from typing import Annotated, Any, Literal

from pydantic import BaseModel, Field

API_VERSION = "1"


class Phase(enum.StrEnum):
    STARTING = "starting"  # models loading; the socket is up so the UI can show progress
    PASSIVE = "passive"  # waiting for the wake phrase; idle audio stays on the machine
    LISTENING = "listening"  # in a session, operator's turn
    THINKING = "thinking"  # model or tool working
    SPEAKING = "speaking"  # Ghost talking


class PendingProposal(BaseModel):
    id: str
    ability: str
    operation: str
    target: str
    summary: str
    rationale: str
    expires_at: datetime
    expires_in_secs: float


class Status(BaseModel):
    phase: Phase
    session_id: str | None
    models_ready: bool
    uptime_secs: float
    version: str
    api_version: str = API_VERSION
    wake_mode: str
    abilities: list[str]
    pending: PendingProposal | None = None
    conversation: str | None = None  # slug of the conversation the session is in (ADR 0014)
    # Where the live session's audio is: the workstation's mic and speaker, or the paired
    # phone over WebRTC (ADR 0016). None when there is no session.
    session_origin: Literal["desk", "phone"] | None = None
    # Ghost has decided to hang up: its goodbye may still be playing, and the session ends
    # as soon as it has. False outside a session.
    ending: bool = False


class Turn(BaseModel):
    role: Literal["user", "assistant", "system"]
    text: str
    session_id: str
    at: datetime
    via: Literal["voice", "text", "interceptor", "system"] = "voice"


class OutcomeView(BaseModel):
    proposal_id: str
    target: str
    status: Literal["success", "partial", "failed"]
    applied: list[str]
    failed_step: str | None = None
    error: str | None = None
    state_now: str | None = None
    summary: str


# --- events over the WebSocket ---------------------------------------------------


class StatusEvent(BaseModel):
    type: Literal["status"] = "status"
    status: Status


class TurnEvent(BaseModel):
    type: Literal["turn"] = "turn"
    turn: Turn


class PendingEvent(BaseModel):
    type: Literal["pending"] = "pending"
    pending: PendingProposal | None  # None when it was resolved or cancelled


class OutcomeEvent(BaseModel):
    type: Literal["outcome"] = "outcome"
    outcome: OutcomeView
    via: Literal["voice", "ui", "autonomous"]


class LogEvent(BaseModel):
    type: Literal["log"] = "log"
    level: str
    message: str
    at: datetime


class ConversationEvent(BaseModel):
    """The session moved to another conversation, or one was created, changed or removed."""

    type: Literal["conversation"] = "conversation"
    active: str | None
    changed: str | None = None  # slug that was created/edited/deleted, if any


Event = Annotated[
    StatusEvent | TurnEvent | PendingEvent | OutcomeEvent | LogEvent | ConversationEvent,
    Field(discriminator="type"),
]


# --- requests -------------------------------------------------------------------


class WakeRequest(BaseModel):
    text: str | None = Field(default=None, description="Words to treat as the first turn.")


class SayRequest(BaseModel):
    text: str = Field(min_length=1, max_length=4000)


class Ack(BaseModel):
    ok: bool
    detail: str | None = None


# --- audit ----------------------------------------------------------------------


class AuditEventView(BaseModel):
    at: datetime
    event: str
    detail: dict[str, Any] | None = None


class AuditEntry(BaseModel):
    id: str
    session_id: str
    ability: str
    operation: str
    target: str
    summary: str
    autonomy: str
    status: str
    created_at: datetime
    events: list[AuditEventView]


class AuditPage(BaseModel):
    items: list[AuditEntry]
    total: int
    limit: int
    offset: int


# --- abilities + policy ---------------------------------------------------------


class OperationInfo(BaseModel):
    name: str
    description: str
    risk: str
    autonomy: str  # resolved at the ability level; targets may tighten it
    properties: dict[str, Any]
    required: list[str]


class ReadToolInfo(BaseModel):
    name: str
    description: str


class AbilityInfo(BaseModel):
    name: str
    version: str
    description: str
    enabled: bool
    loaded: bool
    passive: bool
    operations: list[OperationInfo] = []
    read_tools: list[ReadToolInfo] = []
    interceptors: list[str] = []
    slots: list[str] = []
    listeners: int = 0
    commands: list[str] = []


class PolicyView(BaseModel):
    path: str
    exists: bool
    text: str
    default: str | None
    skills: dict[str, dict[str, str]]
    targets: dict[str, dict[str, dict[str, str]]]
    enabled: list[str] | None
    passive: list[str]


class PolicyUpdate(BaseModel):
    """Either the raw TOML, or structured fields that are rendered to TOML."""

    text: str | None = None
    default: str | None = None
    skills: dict[str, dict[str, str]] | None = None
    targets: dict[str, dict[str, dict[str, str]]] | None = None
    enabled: list[str] | None = None
    passive: list[str] | None = None


class PolicyResult(BaseModel):
    policy: PolicyView
    applied: bool  # autonomy levels reloaded live
    restart_required: bool  # the enabled list changed


# --- settings -------------------------------------------------------------------

SECRET_KEYS = (
    "ANTHROPIC_AUTH_TOKEN",
    "GITHUB_TOKEN",
    "TYPESAFE_API_KEY",
    "OPENROUTER_API_KEY",
    "HF_TOKEN",
)


class SecretState(BaseModel):
    name: str
    set: bool


class SettingsView(BaseModel):
    settings_path: str
    env_path: str
    values: dict[str, str]  # non-secret settings, as written in settings.env
    secrets: list[SecretState]
    vault_managed: bool  # the operator's secrets store renders env at every start


class SettingsUpdate(BaseModel):
    values: dict[str, str | None] | None = None  # None value deletes the key
    secrets: dict[str, str | None] | None = None  # write-only; None deletes


class SettingsResult(BaseModel):
    settings: SettingsView
    restart_required: bool
    applied: list[str] = []  # keys the daemon picked up without a restart


# --- devices + setup ------------------------------------------------------------


class DeviceInfo(BaseModel):
    index: int
    name: str
    max_input_channels: int
    max_output_channels: int
    default_samplerate: float
    is_default_input: bool
    is_default_output: bool


class DevicesView(BaseModel):
    devices: list[DeviceInfo]
    input_device: str | None
    output_device: str | None
    echo_cancel_active: bool
    echo_cancel_source_present: bool


class ModelsView(BaseModel):
    ready: bool
    cache_dir: str
    kokoro_present: bool
    stt_model: str


# --- conversations (ADR 0014) -------------------------------------------------------


class ConversationView(BaseModel):
    slug: str
    name: str
    aliases: list[str]
    voice: str
    speed: float
    created_at: datetime
    last_active_at: datetime | None
    active: bool  # the running session is in it
    is_ghost: bool


class ConversationDetail(ConversationView):
    persona: str
    memory: str
    notes: list[str]
    turn_count: int
    compactions: int
    dir: str


class ConversationCreate(BaseModel):
    name: str = Field(min_length=1, max_length=80)
    aliases: list[str] = []
    voice: str | None = None
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str = Field(default="", max_length=4000)


class ConversationUpdate(BaseModel):
    """Each field present is applied as its own audited write."""

    name: str | None = Field(default=None, min_length=1, max_length=80)
    aliases: list[str] | None = None
    voice: str | None = None
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str | None = Field(default=None, max_length=4000)
    memory: str | None = Field(default=None, max_length=20000)
    notes: list[str] | None = None


class TurnView(BaseModel):
    id: int
    session_id: str
    role: Literal["user", "assistant", "call", "system"]
    text: str
    at: datetime
    compacted: bool


class CompactionView(BaseModel):
    id: str
    at: datetime
    source: str
    summary: str
    through_turn_id: int | None


class VoiceInfo(BaseModel):
    id: str
    label: str  # "American female: Heart"


class SampleRequest(BaseModel):
    speed: float = Field(default=1.0, ge=0.5, le=2.0)
    text: str | None = Field(default=None, max_length=200)


# --- the paired phone (ADR 0016) ------------------------------------------------------


class PhonePairing(BaseModel):
    """A fresh pairing code for the phone listener. Shown to the operator only."""

    code: str  # six digits
    address: str  # GHOST_PHONE_LISTEN, host:port
    expires_at: datetime


class PhoneDevice(BaseModel):
    id: str
    name: str
    paired_at: datetime
    last_seen: datetime | None


class PhoneView(BaseModel):
    address: str | None  # None when the listener is off
    device: PhoneDevice | None
