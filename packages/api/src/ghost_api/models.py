"""The wire contract. Everything the desktop and mobile apps see is one of these.

``packages/api/openapi.json`` is generated from this module (``make api-schema``) and
``packages/client-ts`` is generated from that. Change a model here, regenerate both.
"""

from __future__ import annotations

import enum
from datetime import datetime
from typing import Annotated, Any, Literal

from pydantic import BaseModel, Field, SecretStr

API_VERSION = "1"


class Phase(enum.StrEnum):
    STARTING = "starting"  # models loading; the socket is up so the UI can show progress
    PASSIVE = "passive"  # waiting for the wake phrase; idle audio stays on the machine
    LISTENING = "listening"  # in a session, operator's turn
    THINKING = "thinking"  # model or tool working
    SPEAKING = "speaking"  # Ghost talking
    HOLD = "hold"  # in a session, waiting to be addressed by name again (ADR 0021)
    ASKING = "asking"  # Ghost greeted the operator itself and waits for a reply (ADR 0032)


class PendingProposal(BaseModel):
    id: str
    source: str  # what it changes: an app, or Ghost's own (ghost_core.source.ToolSource)
    operation: str
    target: str
    summary: str
    rationale: str
    expires_at: datetime
    expires_in_secs: float


class Build(BaseModel):
    """Which kind of build the running daemon is (docs/releasing.md): ``dev`` runs from a
    checkout, ``deploy`` from a ``make deploy`` runtime, ``release`` from the .deb. The
    commit is the full hash when known; ``dirty`` is a checkout that had uncommitted
    changes when the daemon started. ``launcher`` is how a checkout's daemon was started,
    when the make target says so (``GHOST_LAUNCHER``): ``make-dev`` for ``make dev`` and
    ``make daemon`` (restarted with ``make reload``), ``make-start`` for ``make start``
    (``make restart``); anything else (a unit running the checkout, ``uv run``) is None."""

    kind: Literal["dev", "deploy", "release"] = "dev"
    commit: Annotated[str, Field(pattern=r"^[0-9a-f]{7,40}$")] | None = None
    dirty: bool = False
    launcher: Literal["make-dev", "make-start"] | None = None


class Status(BaseModel):
    phase: Phase
    session_id: str | None
    models_ready: bool
    uptime_secs: float
    version: str  # YYYY.M.N, the same in every copy (scripts/version.py)
    build: Build = Build()
    api_version: str = API_VERSION
    wake_mode: str
    apps: list[str] = []  # the apps connected now (ADR 0034)
    pending: PendingProposal | None = None
    conversation: str | None = None  # slug of the agent the session is with (ADR 0014)
    # Where the live session's audio is: the workstation's mic and speaker, or the paired
    # phone over WebRTC (ADR 0016). None when there is no session.
    session_origin: Literal["desk", "phone"] | None = None
    # Ghost has decided to hang up: its goodbye may still be playing, and the session ends
    # as soon as it has. False outside a session.
    ending: bool = False
    # How many items on the waiting list still need the operator to hear them (ADR 0029).
    waiting: int = 0


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


class WaitingEvent(BaseModel):
    """The waiting list changed: read ``GET /waiting`` for it."""

    type: Literal["waiting"] = "waiting"
    count: int


class AppsEvent(BaseModel):
    """An app was connected, disconnected or changed state: read ``GET /apps``. With
    ``sign_in_url``, the app is waiting for the operator to sign in there; the desktop opens
    it in the browser."""

    type: Literal["apps"] = "apps"
    changed: str | None = None
    sign_in_url: str | None = None


Event = Annotated[
    StatusEvent
    | TurnEvent
    | PendingEvent
    | OutcomeEvent
    | LogEvent
    | ConversationEvent
    | WaitingEvent
    | AppsEvent,
    Field(discriminator="type"),
]


# --- the waiting list (ADR 0029, ADR 0030) ---------------------------------------


class WaitingItemView(BaseModel):
    """One item that needs the operator. ``title`` comes from an outside system: show
    it as text, never as markup. ``change`` is set for a change waiting for a yes."""

    key: str
    source: str
    title: str
    link: str | None = None
    state: Literal["new", "delivered", "acknowledged", "snoozed"]
    first_seen: datetime
    changed_at: datetime
    age_secs: int
    delivered_at: datetime | None = None
    delivered_via: str | None = None
    acknowledged_at: datetime | None = None
    snoozed_until: datetime | None = None
    change: PendingProposal | None = None


class WaitingView(BaseModel):
    items: list[WaitingItemView]
    count: int  # how many still need the operator to hear them


class ItemKey(BaseModel):
    key: str


class SnoozeRequest(BaseModel):
    key: str
    minutes: int = Field(ge=1, le=7 * 24 * 60)


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
    source: str  # an app, or Ghost's own (conversations, skills, ...)
    operation: str
    target: str
    summary: str
    autonomy: str
    status: str
    created_at: datetime
    events: list[AuditEventView]
    conversation: str | None = None  # the agent it was proposed with (ADR 0014); colours the entry


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
    autonomy: str  # by name alone; a rule with argument patterns may decide some calls
    properties: dict[str, Any]
    required: list[str]


class ReadToolInfo(BaseModel):
    name: str
    description: str


# --- apps and permissions (ADR 0034) ---------------------------------------------

Level = Literal["allow", "ask", "block", "quiet"]


class AppAction(BaseModel):
    """One of an app's tools, and what Ghost's own permissions say about it."""

    name: str
    description: str  # the app's, first sentence: untrusted text, shown to the operator only
    reads: bool  # read-only, by the app's own word; anything else is a change
    level: Level  # for a change; a read is allow or block
    rule: str  # the rule that names just this tool ("github.issue_write")
    conditional: bool = False  # a rule with argument patterns may decide some calls


class AppView(BaseModel):
    """A connected app: how it is reached, whether it is up, and what it can do."""

    name: str  # its short name: the rules' and the tools' prefix
    title: str
    kind: Literal["sign_in", "key", "local", "custom"]
    state: Literal["connected", "connecting", "needs_sign_in", "failed", "stopped"]
    problem: str | None = None  # not connected, or tools not offered, and why; never a secret
    catalog: str | None = None  # the catalog entry it came from
    icon: str | None = None
    actions: list[AppAction] = []
    sign_in_url: str | None = None  # waiting for the operator's browser
    settings: list[str] = []  # the environment names it needs (secret values never shown)


class CatalogSetting(BaseModel):
    key: str
    label: str
    secret: bool = False
    required: bool = True
    default: str | None = None
    choices: list[str] = []


class CatalogOption(BaseModel):
    """One way to connect: sign in, an API key, or run on this computer. Where it runs:
    a remote's ``host``, and whose host that is; a package's ``registry`` and name."""

    id: str
    kind: Literal["sign_in", "key", "local"]
    label: str
    settings: list[CatalogSetting] = []
    host: str | None = None  # a remote's host: what Ghost sends the app goes there
    # The product maker's host, the publisher's own, or someone else's (a gateway, a
    # shared host). "maker" only when Ghost knows the product's maker.
    host_owner: Literal["maker", "publisher", "other"] | None = None
    registry: Literal["npm", "pypi", "oci"] | None = None  # a package's registry
    package: str | None = None  # its name there: "posthog-mcp"


class CatalogPublisher(BaseModel):
    """Who published an entry, as the registry checked it: the GitHub account it signed in
    with (``io.github.PostHog/…``), or the domain it proved it owns (``com.mcparmory/…``).
    Who published it, not whether it is safe."""

    name: str  # "PostHog", "mcparmory.com"
    verified: Literal["github", "domain"]


class CatalogMaker(BaseModel):
    """The entry is the product maker's own, and what agreed: the publisher's name is the
    product's (``publisher``) or Ghost's own list says so (``listed``); it connects to the
    maker's host (``remote``); its repository is theirs; its website is theirs."""

    product: str  # "PostHog"
    evidence: list[Literal["publisher", "listed", "remote", "repository", "website"]]


class CatalogEntryView(BaseModel):
    """An app in the catalog. Its text is the registry's: shown to the operator, never
    to the model."""

    name: str  # the registry's name, reverse-DNS
    title: str
    description: str
    version: str
    website: str | None = None
    repository: str | None = None
    icon: str | None = None
    featured: bool = False
    options: list[CatalogOption] = []
    suggested_name: str  # the short name it would be connected under
    connected: list[str] = []  # the names it is connected under already
    updated_at: datetime | None = None  # when the registry last saw this version change
    publisher: CatalogPublisher | None = None
    maker: CatalogMaker | None = None  # set when it is the product maker's own
    # The product it is for, as its maker names it, when Ghost knows the maker ("PostHog"
    # for pipeworx-io's "Posthog"); and that maker's own entry, when it isn't this one.
    product: str | None = None
    product_maker: str | None = None
    # In search results: other publishers' entries for the same product, folded under the
    # maker's own. Always empty on GET /catalog/{name}.
    others: list[CatalogEntryView] = []


class CatalogPage(BaseModel):
    entries: list[CatalogEntryView]  # results; another publisher's for a product may fold
    total: int  # results, counting a maker's own with what folds under it as one
    fetched_at: datetime | None = None  # when Ghost's copy was last refreshed
    refreshing: bool = False
    problem: str | None = None


class AppConnect(BaseModel):
    """Connect an app: from the catalog (``catalog`` and ``option``), from a URL, or from
    another client's pasted config. Secret values go to Ghost's environment, never into
    apps.json, and never come back out."""

    catalog: str | None = None
    option: str | None = None
    name: str | None = Field(default=None, max_length=40)
    values: dict[str, str] | None = None
    url: str | None = None
    config: str | None = Field(default=None, max_length=20000)
    replace: bool | None = None


class AppConnectResult(BaseModel):
    apps: list[AppView]
    sign_in_url: str | None = None
    # What the operator should know about a pasted config: keys Ghost does not read.
    problems: list[str] = []


class PermissionsView(BaseModel):
    """``[permissions]`` in policy.toml: everything runs unless a rule says otherwise."""

    path: str
    mode: Literal["open", "ask", "read-only"]
    block: list[str] = []
    ask: list[str] = []
    quiet: list[str] = []
    allow: list[str] = []


class RuleSet(BaseModel):
    """One toggle: move ``rule`` into ``level``'s list, or out of every list (None)."""

    rule: str = Field(min_length=1, max_length=300)
    level: Level | None = None


class PermissionsUpdate(BaseModel):
    """Whole lists, the mode, or one rule moved; what is not given is kept."""

    mode: Literal["open", "ask", "read-only"] | None = None
    block: list[str] | None = None
    ask: list[str] | None = None
    quiet: list[str] | None = None
    allow: list[str] | None = None
    set: RuleSet | None = None


class SkillOriginView(BaseModel):
    """Where one of Ghost's skills was installed from (#35), as its lock file records it."""

    source: str  # "anthropics/skills", a GitHub repository
    source_url: str
    skill_path: str  # its SKILL.md in the repository
    commit: str  # the commit it was installed at
    folder_hash: str  # the folder's git tree hash at that commit
    ref: str | None = None  # the branch or tag it follows; None: the default branch
    installed_at: str | None = None


class SkillInfo(BaseModel):
    """A skill Ghost can read (ADR 0023): a SKILL.md folder, Ghost's own or the operator's."""

    name: str
    description: str
    source: str  # "ghost" for Ghost's own folder, "home" for the operator's ~/.agents/skills
    path: str
    requires: list[str] = []  # the apps it says it needs
    missing: list[str] = []  # the ones that are not connected
    files: list[str] = []  # the files beside SKILL.md
    problem: str | None = None  # why it is not offered to the model (ADR 0024), if it is not
    origin: SkillOriginView | None = None  # installed from GitHub through the catalog


class SkillListing(BaseModel):
    """A skill skills.sh lists (#35). Its text is the directory's: shown to the operator,
    never to the model."""

    id: str  # "anthropics/skills/pdf"
    source: str  # "anthropics/skills"
    skill: str  # "pdf"
    name: str
    installs: int
    installed: str | None = None  # the name of Ghost's skill installed from it, if one is


class SkillSearchPage(BaseModel):
    query: str
    skills: list[SkillListing] = []
    problem: str | None = None  # skills.sh failed or answered in a shape Ghost doesn't know
    fetched_at: datetime | None = None


class SkillPreviewRequest(BaseModel):
    source: str = Field(
        min_length=1,
        max_length=300,
        description="owner/repo, owner/repo/skill, or a github.com address of a skill's folder.",
    )
    skill: str | None = Field(default=None, max_length=128, description="The skill's id in it.")


class SkillFileView(BaseModel):
    path: str
    bytes: int
    script: bool  # a file an interpreter runs, or one that was executable upstream


class SkillPreview(BaseModel):
    """A skill fetched from GitHub and staged, for the operator to read before installing.
    Everything in it but the hashes is the source's text: untrusted."""

    token: str  # what install takes; good for 30 minutes
    name: str
    description: str
    source: str
    source_url: str
    skill_path: str
    commit: str
    folder_hash: str
    ref: str | None = None
    skill_md: str  # the whole SKILL.md, front matter included
    files: list[SkillFileView] = []  # the files beside SKILL.md
    scripts: list[str] = []  # the ones ghost-scripts could run, through the gate
    installed: Literal["ghost", "home"] | None = None  # a skill of that name already
    update_of: str | None = None  # the installed skill this is a newer version of


class SkillInstall(BaseModel):
    token: str = Field(min_length=1, max_length=64)
    name: str | None = None
    replace: bool = False


class SkillUpdateView(BaseModel):
    """Whether an installed skill's source has moved on, checked when the page asks."""

    name: str
    source: str
    commit: str  # installed
    latest_commit: str | None = None
    update: bool = False  # the skill's folder changed upstream
    problem: str | None = None  # the check failed, or the source no longer has it


class SkillBody(BaseModel):
    name: str
    body: str  # SKILL.md without its front matter, trimmed
    files: list[str] = []


class SkillAdd(BaseModel):
    path: str = Field(min_length=1, description="A folder on this machine holding SKILL.md.")
    name: str | None = None
    replace: bool = False


class WatchInfo(BaseModel):
    """A watch in watches.json (ADR 0028, ADR 0035) and how its last look went."""

    name: str
    app: str
    read: str
    every_secs: int
    steps: list[str]  # "check", then "jev" and "model" where declared
    response: Literal["tell", "record"]
    urgency: Literal["digest", "notify", "call"]
    last_run: datetime | None = None
    last_ok: datetime | None = None
    error: str | None = None
    seen: int = 0  # results the last look kept
    open: int = 0  # items it holds open now


class HookInfo(BaseModel):
    """A webhook in hooks.json (ADR 0027, ADR 0035): on or off, and what came of
    deliveries since the daemon started. Counts only; never a body."""

    name: str
    path: str  # POST here on the webhook listener
    verify: Literal["github", "hmac"]
    events: list[str]
    on: bool
    off_reason: str | None = None
    accepted: dict[str, int] = {}  # item, resolved, ignored
    rejected: dict[str, int] = {}  # by reason
    last: datetime | None = None


class OutputInfo(BaseModel):
    """A file Ghost made in its out/ folder (ADR 0033). Ghost never runs it."""

    name: str
    path: str
    bytes: int
    modified: datetime


class WatchingView(BaseModel):
    """What Ghost watches (ADR 0035): the watches in watches.json, the webhooks in
    hooks.json, where the webhook listener is, and what is wrong in those files or left
    in Ghost's folder from an older layout."""

    watches: list[WatchInfo] = []
    hooks: list[HookInfo] = []
    listening: list[str] = []  # the webhook listener's addresses; empty when it is off
    problems: list[str] = []


class RulesView(BaseModel):
    """Ghost's AGENTS.md (ADR 0035): the rules in every agent's prompt."""

    path: str  # the file the operator opens
    text: str


class RuleAdd(BaseModel):
    """A rule appended to Ghost's AGENTS.md, verbatim (ADR 0035)."""

    text: str = Field(min_length=1, max_length=1000)


class SourceInfo(BaseModel):
    """A source of tools the gate knows: Ghost's own (built in) or a connected app."""

    name: str
    kind: str  # "built-in" or "app"
    description: str
    operations: list[OperationInfo] = []
    read_tools: list[ReadToolInfo] = []


RouteName = Literal["digest", "desk", "desktop", "phone", "call", "sms", "whatsapp"]


class RoutesView(BaseModel):
    """``[routes]`` in the policy file (ADR 0031): how Ghost reaches the operator."""

    enabled: list[RouteName]
    escalate_after: int  # minutes
    quiet_hours: str | None = None  # "22:00-07:00"
    batch_secs: int
    calls_per_hour: int
    calls_per_day: int
    carry: Literal["count", "title"]
    greeting: str
    urgency: dict[str, Literal["digest", "notify", "call"]] = {}
    # The routes this daemon has something behind; the others can be listed but do nothing yet.
    available: list[RouteName] = []


class PolicyView(BaseModel):
    path: str
    exists: bool
    text: str
    permissions: PermissionsView
    routes: RoutesView | None = None


class PolicyUpdate(BaseModel):
    """Either the raw TOML, or the routes rendered into it (the permissions have their own
    route, ``PUT /permissions``)."""

    text: str | None = None
    routes: RoutesView | None = None


class PolicyResult(BaseModel):
    policy: PolicyView
    applied: bool  # reloaded live


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
    values: dict[str, str]  # non-secret settings, as written in settings.env
    # Ghost's own passwords and the apps', set or not (ADR 0036); /passwords has the rest
    secrets: list[SecretState]


class SettingsUpdate(BaseModel):
    values: dict[str, str | None] | None = None  # None value deletes the key
    # write-only, into the vault (ADR 0036); None deletes. PUT /passwords/{name} is the
    # Passwords page's own way.
    secrets: dict[str, str | None] | None = None


class SettingsResult(BaseModel):
    settings: SettingsView
    applied: list[str] = []  # keys the daemon picked up without a restart
    restart_required: bool = False  # a pending value or a secret: the service must restart


# --- passwords (ADR 0036) -------------------------------------------------------
# A password goes in and never comes back out: these say what it is called, what uses it
# and when it was last used. No route returns a value.


class VaultView(BaseModel):
    """Ghost's vault: one encrypted file, its key in the operator's keyring."""

    state: Literal["open", "locked", "unavailable", "broken"]
    problem: str | None = None  # why it isn't open, in the operator's words
    path: str


class PasswordInfo(BaseModel):
    name: str  # what config files call it: ${NAME}
    set: bool | None  # in the vault; None while the vault is closed (locked): unknown
    used_by: list[str] = []  # "The model connection", "GitHub" (an app), "Webhook ci"
    ghost: bool = False  # one Ghost itself reads; a new value applies at its next start
    added: datetime | None = None
    updated: datetime | None = None
    last_used: datetime | None = None  # the last time Ghost filled it in


class ImportSourceView(BaseModel):
    kind: Literal["heka_vault", "file"]  # heka-vault's config, or a plaintext file
    path: str


class PasswordsView(BaseModel):
    vault: VaultView
    passwords: list[PasswordInfo]
    importable: list[ImportSourceView] = []  # what an import would take; empty after one


class PasswordSet(BaseModel):
    value: SecretStr  # write-only; never echoed, not even in a validation error


class PasswordResult(BaseModel):
    passwords: PasswordsView
    restart_required: bool = False  # Ghost's own: the service must restart to use it


class PasswordImportResult(BaseModel):
    imported: list[str] = []  # names now in the vault
    kept: list[str] = []  # already in the vault; left as they were
    settings: list[str] = []  # non-secret settings moved to settings.env
    removed: list[str] = []  # plaintext files deleted
    notes: list[str] = []
    passwords: PasswordsView
    restart_required: bool = False


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
    """An agent (ADR 0014, ADR 0035): a conversation with a persona and a job."""

    id: str
    slug: str
    name: str
    aliases: list[str]
    job: str = ""
    voice: str  # the voice heard: its own, else Ghost's
    speed: float
    own_voice: bool = False  # True when the voice is the agent's own
    created_at: datetime
    last_active_at: datetime | None
    retired_at: datetime | None = None  # set: off the roster and voice routing, still readable
    active: bool  # the running session is with it
    is_ghost: bool


class ConversationDetail(ConversationView):
    persona: str
    memory: str
    notes: list[str]
    turn_count: int
    compactions: int
    dir: str


class ConversationCreate(BaseModel):
    """Hire an agent."""

    name: str = Field(min_length=1, max_length=80)
    aliases: list[str] = []
    job: str = Field(default="", max_length=200)
    voice: str | None = None  # None: Ghost's voice
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str = Field(default="", max_length=4000)
    # Slug of a retired agent whose memory.md becomes this one's first memory, as a
    # handoff. Never Ghost.
    seed_from: str | None = None


class ConversationUpdate(BaseModel):
    """Each field present is applied as its own audited write."""

    name: str | None = Field(default=None, min_length=1, max_length=80)
    aliases: list[str] | None = None
    job: str | None = Field(default=None, max_length=200)
    voice: str | None = None  # "" means back to Ghost's voice
    speed: float | None = Field(default=None, ge=0.5, le=2.0)
    persona: str | None = Field(default=None, max_length=4000)
    memory: str | None = Field(default=None, max_length=20000)
    notes: list[str] | None = None
    seed_from: str | None = None  # replace the memory with that agent's memory.md


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
    # The QR code's payload: ghost://pair?code=…&address=…, every address in order. The phone
    # app scans it, or the phone's camera opens the app with it (docs/phone-protocol.md).
    link: str


# The network a phone address is on (ADR 0020): the home network, or the operator's
# WireGuard tunnel for away from home.
PhoneNetwork = Literal["home", "tunnel"]


class PhoneAddress(BaseModel):
    """One ``GHOST_PHONE_LISTEN`` address and the network it is on."""

    address: str
    # From the interface that holds the address: a WireGuard (or other tunnel) interface is
    # the tunnel, any other the home network. None when Ghost can't tell: the address is on
    # every interface (0.0.0.0), or on none of this computer's right now.
    network: PhoneNetwork | None
    listening: bool  # bound now; a tunnel address whose interface isn't up yet is retried


class PhoneDevice(BaseModel):
    id: str
    name: str
    paired_at: datetime
    last_seen: datetime | None  # the phone's last verified request
    last_address: str | None = None  # the listen address that request came in on
    last_network: PhoneNetwork | None = None  # and the network it came over


class PhoneView(BaseModel):
    address: str | None  # None when the listener is off
    addresses: list[str] = Field(default_factory=list)  # LAN first, then the tunnel (ADR 0020)
    listen: list[PhoneAddress] = Field(default_factory=list)  # the same, with each network
    device: PhoneDevice | None


# --- the model connection (ADR 0037) --------------------------------------------------


class ModelProviderView(BaseModel):
    """One way to reach a model, as the Model page offers it."""

    id: str
    title: str
    blurb: str
    protocol: Literal["anthropic", "openai"]
    base_url: str  # "" for a URL of the operator's own
    password: str | None = None  # the Password's name it uses by default
    password_optional: bool = False
    sign_in: bool = False  # OpenRouter: signing in gives the key
    local: bool = False
    default_model: str | None = None  # None: the first the provider lists


class ModelConnectionView(BaseModel):
    """The model connection now. No key is in it: only whether its Password is set."""

    provider: str
    protocol: Literal["anthropic", "openai"]
    base_url: str  # where calls go
    base_url_override: str | None = None  # set when it isn't the provider's own
    model: str  # as saved; "" means the provider's default
    wire_model: str  # what is sent
    password: str | None = None  # the name
    password_set: bool
    headers: dict[str, str] = {}
    timeout_secs: float | None = None
    legacy: str | None = None  # the older setting it comes from ("GHOST_LLM_ENDPOINT=openrouter")
    problems: list[str] = []
    ready: bool
    # The GHOST_LLM_* values that say this connection explicitly (for a page that saves it).
    values: dict[str, str] = {}
    providers: list[ModelProviderView] = []
    sign_in_pending: bool = False  # OpenRouter is waiting for the browser
    sign_in_error: str | None = None  # the last sign-in didn't go through


class ModelDraft(BaseModel):
    """A connection being edited: GHOST_LLM_* values over the saved ones (None removes
    one), and a key typed but not saved yet, used for this one request and dropped."""

    values: dict[str, str | None] = {}
    # write-only, like PasswordSet.value: never echoed, not even in a validation error
    password_value: SecretStr | None = Field(default=None, max_length=4096)


class ModelOptionView(BaseModel):
    id: str
    name: str | None = None


class ModelListView(BaseModel):
    models: list[ModelOptionView]
    default: str | None = None  # the provider's default, when it lists it
    error: str | None = None  # why the list couldn't be read; models is empty then


class ModelTestResult(BaseModel):
    """One tiny request, timed like ``make perf-llm``: request to first token."""

    ok: bool
    ttft_ms: int | None = None
    total_ms: int | None = None
    model: str  # what was asked for
    answered_by: str | None = None  # what the provider said answered, when it says
    reply: str | None = None  # the first words, shortened
    error: str | None = None
    where: str  # provider, protocol and host; never a key


class ModelSignInResult(BaseModel):
    url: str  # the desktop opens it; the loopback takes the browser's way back
