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
    HOLD = "hold"  # in a session, waiting to be addressed by name again (ADR 0021)


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
    servers: list[str]  # the MCP servers connected at start (ADR 0022)
    pending: PendingProposal | None = None
    conversation: str | None = None  # slug of the conversation the session is in (ADR 0014)
    department: str | None = None  # ADR 0023
    toolkit: str | None = None
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
    conversation: str | None = Field(
        default=None, description="The line the session opens in; Ghost when unset."
    )


class SayRequest(BaseModel):
    text: str = Field(min_length=1, max_length=4000)
    conversation: str | None = Field(
        default=None,
        description="With no session running, the line the new one opens in; Ghost when unset.",
    )


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
    conversation: str | None = None  # the thread it was proposed in (ADR 0014); colours the entry
    department: str | None = None  # ADR 0023
    toolkit: str | None = None


class AuditPage(BaseModel):
    items: list[AuditEntry]
    total: int
    limit: int
    offset: int


# --- sources + policy ---------------------------------------------------------


class OperationInfo(BaseModel):
    name: str
    description: str
    risk: str
    autonomy: str  # resolved at the source level; targets may tighten it
    properties: dict[str, Any]
    required: list[str]


class ReadToolInfo(BaseModel):
    name: str
    description: str


class McpToolInfo(BaseModel):
    name: str
    description: str
    offered: bool  # read-only (or listed under reads): the model may call it today


class McpWriteInfo(BaseModel):
    """A declared write: the policy operation, the tool it calls, and its level."""

    operation: str
    tool: str
    risk: str
    autonomy: str  # resolved at the server level; a target rule may still tighten it
    asked_by: list[str] = []
    target: list[str] = []
    read: str | None = None  # the read that shows the target; None: no stale check
    offered: bool  # the tool exists on the server and the level is not deny
    properties: dict[str, Any] = {}
    required: list[str] = []


class McpScopeInfo(BaseModel):
    kind: str
    arguments: list[str]
    items: list[str] = []


class McpServerInfo(BaseModel):
    """One MCP server from the policy file (ADR 0022) and what Ghost got from it."""

    name: str
    transport: str  # "http" or "stdio"
    connected: bool
    error: str | None = None  # why it is not connected; never a secret
    tools: list[McpToolInfo] = []
    writes: list[McpWriteInfo] = []
    scope: McpScopeInfo | None = None


class SkillInfo(BaseModel):
    """A skill Ghost can read (ADR 0023): a SKILL.md folder, Ghost's own or a project's."""

    name: str
    description: str
    source: str  # "ghost" for Ghost's own directory, else the project root it sits under
    path: str
    requires: list[str] = []  # "mcp:<server>" names it needs
    missing: list[str] = []  # the required servers that are not connected
    files: list[str] = []  # the files beside SKILL.md


class SkillBody(BaseModel):
    name: str
    body: str  # SKILL.md without its front matter, trimmed
    files: list[str] = []


class SkillAdd(BaseModel):
    path: str = Field(min_length=1, description="A folder on this machine holding SKILL.md.")
    name: str | None = None
    replace: bool = False


class GrantView(BaseModel):
    """A toolkit's grant on one server (ADR 0023). null lists mean everything."""

    server: str
    reads: list[str] | None = None
    writes: list[str] | None = None
    items: list[str] | None = None
    levels: dict[str, str] = {}


class ToolkitView(BaseModel):
    name: str
    description: str = ""
    skills: list[str] = []
    grants: list[GrantView] = []
    problems: list[str] = []  # what the file does not allow or know about, from check_toolkits


class ToolkitUpdate(BaseModel):
    description: str = ""
    skills: list[str] = []
    grants: list[GrantView] = []


class TemplateView(BaseModel):
    """A department template (ADR 0023): shipped with Ghost or the operator's own."""

    name: str
    title: str
    description: str = ""
    source: str  # "shipped" or "ghost"
    path: str
    aliases: list[str] = []
    voice: str | None = None
    toolkits: list[str] = []
    skills: list[str] = []
    needs: list[str] = []  # what is missing for it to work as declared


class TemplateApply(BaseModel):
    name: str | None = None  # the department's name; default: the template's title


class SourceInfo(BaseModel):
    """A source of tools the gate knows: the lines (built in) or an MCP server. The
    policy screen edits levels per operation from this list."""

    name: str
    kind: str  # "built-in" or "server"
    description: str
    operations: list[OperationInfo] = []
    read_tools: list[ReadToolInfo] = []
    scope_kind: str | None = None  # what a thread can be scoped to for it (ADR 0019)
    scope_items: list[str] = []  # the whole allowlist, in Target.scope() form


class PolicyView(BaseModel):
    path: str
    exists: bool
    text: str
    default: str | None
    skills: dict[str, dict[str, str]]
    targets: dict[str, dict[str, dict[str, str]]]


class PolicyUpdate(BaseModel):
    """Either the raw TOML, or structured fields that are rendered to TOML."""

    text: str | None = None
    default: str | None = None
    skills: dict[str, dict[str, str]] | None = None
    targets: dict[str, dict[str, dict[str, str]]] | None = None


class PolicyResult(BaseModel):
    policy: PolicyView
    applied: bool  # autonomy levels reloaded live


# --- settings -------------------------------------------------------------------

SECRET_KEYS = (
    "ANTHROPIC_AUTH_TOKEN",
    "GITHUB_TOKEN",
    "TYPESAFE_API_KEY",
    "OPENROUTER_API_KEY",
    "HF_TOKEN",
    "ELEVENLABS_API_KEY",
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
    applied: list[str] = []  # keys the daemon picked up without a restart
    restart_required: bool = False  # a pending value or a secret: the service must restart


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
    # ADR 0019: per ability, the items this thread may touch ({"github_issues":
    # ["acme/app"]}); an ability absent here is unscoped. Always empty for Ghost.
    scope: dict[str, list[str]] = {}
    created_at: datetime
    last_active_at: datetime | None
    settled_at: datetime | None = None  # set: done, out of voice routing, still readable
    active: bool  # the running session is in it
    is_ghost: bool
    # ADR 0023: a department (no ``department``) and its toolkits, or a topic line in one.
    toolkits: list[str] = []
    department: str | None = None
    is_department: bool = False


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
    voice: str | None = None  # None: a voice no other thread uses
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str = Field(default="", max_length=4000)
    scope: dict[str, list[str]] = {}
    # Slug of a thread (usually a settled one) whose memory.md becomes this one's first
    # memory, as a handoff. Never Ghost.
    seed_from: str | None = None
    department: str | None = None  # ADR 0023: start it as a topic line in this department
    toolkits: list[str] = []  # ADR 0023: a new department's toolkits (none: everything)


class ToolkitsUpdate(BaseModel):
    toolkits: list[str] = []


class ConversationUpdate(BaseModel):
    """Each field present is applied as its own audited write."""

    name: str | None = Field(default=None, min_length=1, max_length=80)
    aliases: list[str] | None = None
    voice: str | None = None
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str | None = Field(default=None, max_length=4000)
    memory: str | None = Field(default=None, max_length=20000)
    notes: list[str] | None = None
    scope: dict[str, list[str]] | None = None  # the whole map; {} makes it unscoped
    seed_from: str | None = None  # replace the memory with that thread's memory.md


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
    address: str  # the first GHOST_PHONE_LISTEN address, host:port; what the operator types
    addresses: list[str] = Field(default_factory=list)  # every address the listener serves
    expires_at: datetime


class PhoneDevice(BaseModel):
    id: str
    name: str
    paired_at: datetime
    last_seen: datetime | None


class PhoneView(BaseModel):
    address: str | None  # None when the listener is off
    addresses: list[str] = Field(default_factory=list)  # LAN first, then the tunnel (ADR 0020)
    device: PhoneDevice | None
