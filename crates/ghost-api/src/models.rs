//! The wire contract. Everything the desktop and mobile apps see is one of these.
//!
//! `packages/api/openapi.json` is generated from these types and `packages/client-ts` from
//! that. Change a type here, regenerate both. Field names, defaults and the value strings
//! are the Python daemon's, so the apps read either daemon the same way. Every field is
//! serialized, defaults included, as pydantic did.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;
use zeroize::Zeroizing;

pub const API_VERSION: &str = "1";

/// A write-only value: read from a request, never serialized, never shown by `Debug`.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(d)?)))
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// The value, at its point of use only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(**********)")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        subtle_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Where the daemon is. The tray tooltip and the phone's Home read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Models loading; the socket is up so the UI can show progress.
    Starting,
    /// Waiting for the wake phrase; idle audio stays on the machine.
    Passive,
    /// In a session, the operator's turn.
    Listening,
    /// Model or tool working.
    Thinking,
    /// Ghost talking.
    Speaking,
    /// In a session, waiting to be addressed by name again (ADR 0021).
    Hold,
    /// Ghost greeted the operator itself and waits for a reply (ADR 0032).
    Asking,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PendingProposal {
    pub id: String,
    /// What it changes: an app, or Ghost's own (a `ToolSource` name).
    pub source: String,
    pub operation: String,
    pub target: String,
    pub summary: String,
    pub rationale: String,
    pub expires_at: DateTime<Utc>,
    pub expires_in_secs: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BuildKind {
    #[default]
    Dev,
    Deploy,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Launcher {
    MakeDev,
    MakeStart,
}

/// Which kind of build the running daemon is (docs/releasing.md): `dev` runs from a
/// checkout, `deploy` from a `make deploy` runtime, `release` from the bundle. The commit
/// is the full hash when known; `dirty` is a checkout that had uncommitted changes when
/// the daemon started. `launcher` is how a checkout's daemon was started, when the make
/// target says so (`GHOST_LAUNCHER`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct Build {
    #[serde(default)]
    #[schema(inline)]
    #[schema(required)]
    pub kind: BuildKind,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub dirty: bool,
    #[serde(default)]
    #[schema(inline)]
    pub launcher: Option<Launcher>,
}

/// Where a live session's audio is: the desk's mic and speaker, or the paired phone over
/// WebRTC (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    Desk,
    Phone,
}

fn api_version() -> String {
    API_VERSION.to_string()
}

/// `GET /status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Status {
    pub phase: Phase,
    #[schema(required)]
    pub session_id: Option<String>,
    pub models_ready: bool,
    pub uptime_secs: f64,
    /// YYYY.M.N, the same in every copy (`scripts/version.py`).
    pub version: String,
    #[serde(default)]
    #[schema(required)]
    pub build: Build,
    #[serde(default = "api_version")]
    #[schema(required)]
    pub api_version: String,
    pub wake_mode: String,
    /// The apps connected now (ADR 0034).
    #[serde(default)]
    #[schema(required)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub pending: Option<PendingProposal>,
    /// Slug of the agent the session is with (ADR 0014).
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(default)]
    #[schema(inline)]
    pub session_origin: Option<SessionOrigin>,
    /// Ghost has decided to hang up: its goodbye may still be playing.
    #[serde(default)]
    #[schema(required)]
    pub ending: bool,
    /// Items on the waiting list the operator has still to hear (ADR 0029).
    #[serde(default)]
    #[schema(required)]
    pub waiting: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TurnVia {
    #[default]
    Voice,
    Text,
    Interceptor,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Turn {
    #[schema(inline)]
    pub role: TurnRole,
    pub text: String,
    pub session_id: String,
    pub at: DateTime<Utc>,
    #[serde(default)]
    #[schema(inline)]
    #[schema(required)]
    pub via: TurnVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeStatus {
    Success,
    Partial,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OutcomeView {
    pub proposal_id: String,
    pub target: String,
    #[schema(inline)]
    pub status: OutcomeStatus,
    pub applied: Vec<String>,
    #[serde(default)]
    pub failed_step: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub state_now: Option<String>,
    pub summary: String,
}

// --- events over the WebSocket ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeVia {
    Voice,
    Ui,
    Autonomous,
}

/// What the event stream carries, discriminated by `type`. Never a passive transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    Status {
        status: Status,
    },
    Turn {
        turn: Turn,
    },
    /// `pending` None when it was resolved or cancelled.
    Pending {
        pending: Option<PendingProposal>,
    },
    Outcome {
        outcome: OutcomeView,
        via: OutcomeVia,
    },
    Log {
        level: String,
        message: String,
        at: DateTime<Utc>,
    },
    /// The session moved to another conversation, or one was created, changed or removed.
    Conversation {
        active: Option<String>,
        #[serde(default)]
        changed: Option<String>,
    },
    /// The waiting list changed: read `GET /waiting` for it.
    Waiting {
        count: u64,
    },
    /// An app was connected, disconnected or changed state: read `GET /apps`. With
    /// `sign_in_url`, the app is waiting for the operator to sign in there.
    Apps {
        #[serde(default)]
        changed: Option<String>,
        #[serde(default)]
        sign_in_url: Option<String>,
    },
}

// --- the waiting list (ADR 0029, ADR 0030) ----------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WaitingState {
    New,
    Delivered,
    Acknowledged,
    Snoozed,
}

/// One item that needs the operator. `title` comes from an outside system: show it as
/// text, never as markup. `change` is set for a change waiting for a yes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WaitingItemView {
    pub key: String,
    pub source: String,
    pub title: String,
    #[serde(default)]
    pub link: Option<String>,
    #[schema(inline)]
    pub state: WaitingState,
    pub first_seen: DateTime<Utc>,
    pub changed_at: DateTime<Utc>,
    pub age_secs: i64,
    #[serde(default)]
    pub delivered_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub delivered_via: Option<String>,
    #[serde(default)]
    pub acknowledged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub snoozed_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub change: Option<PendingProposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WaitingView {
    pub items: Vec<WaitingItemView>,
    /// How many still need the operator to hear them.
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ItemKey {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SnoozeRequest {
    pub key: String,
    /// 1 to a week.
    pub minutes: i64,
}

// --- requests ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct WakeRequest {
    /// Words to treat as the first turn.
    #[serde(default)]
    pub text: Option<String>,
    /// The line the session opens in; Ghost when unset.
    #[serde(default)]
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SayRequest {
    /// 1 to 4000 characters.
    pub text: String,
    /// With no session running, the line the new one opens in; Ghost when unset.
    #[serde(default)]
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Ack {
    pub ok: bool,
    #[serde(default)]
    pub detail: Option<String>,
}

impl Ack {
    pub fn ok() -> Self {
        Self {
            ok: true,
            detail: None,
        }
    }

    pub fn no(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: Some(detail.into()),
        }
    }
}

// --- audit -------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AuditEventView {
    pub at: DateTime<Utc>,
    pub event: String,
    #[serde(default)]
    pub detail: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AuditEntry {
    pub id: String,
    pub session_id: String,
    /// An app, or Ghost's own (conversations, skills, ...).
    pub source: String,
    pub operation: String,
    pub target: String,
    pub summary: String,
    pub autonomy: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub events: Vec<AuditEventView>,
    /// The agent it was proposed with (ADR 0014); colours the entry.
    #[serde(default)]
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AuditPage {
    pub items: Vec<AuditEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

// --- sources + policy ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OperationInfo {
    pub name: String,
    pub description: String,
    pub risk: String,
    /// By name alone; a rule with argument patterns may decide some calls.
    pub autonomy: String,
    pub properties: Map<String, Value>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReadToolInfo {
    pub name: String,
    pub description: String,
}

// --- apps and permissions (ADR 0034) --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Allow,
    Ask,
    Block,
    Quiet,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Allow => "allow",
            Level::Ask => "ask",
            Level::Block => "block",
            Level::Quiet => "quiet",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "block" => Some(Self::Block),
            "quiet" => Some(Self::Quiet),
            _ => None,
        }
    }
}

/// One of an app's tools, and what Ghost's own permissions say about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppAction {
    pub name: String,
    /// The app's, first sentence: untrusted text, shown to the operator only.
    pub description: String,
    /// Read-only, by the app's own word; anything else is a change.
    pub reads: bool,
    // For a change; a read is allow or block. (Not a doc comment: utoipa would wrap the
    // inlined enum in an allOf with a description object, which the TS client reads as an
    // intersection with Record<string, never>.)
    #[schema(inline)]
    pub level: Level,
    /// The rule that names just this tool ("github.issue_write").
    pub rule: String,
    /// A rule with argument patterns may decide some calls.
    #[serde(default)]
    #[schema(required)]
    pub conditional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    SignIn,
    Key,
    Local,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    Connected,
    Connecting,
    NeedsSignIn,
    Failed,
    Stopped,
}

/// A connected app: how it is reached, whether it is up, and what it can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppView {
    /// Its short name: the rules' and the tools' prefix.
    pub name: String,
    pub title: String,
    #[schema(inline)]
    pub kind: AppKind,
    #[schema(inline)]
    pub state: AppState,
    /// Not connected, or tools not offered, and why; never a secret.
    #[serde(default)]
    pub problem: Option<String>,
    /// The catalog entry it came from.
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub actions: Vec<AppAction>,
    /// Waiting for the operator's browser.
    #[serde(default)]
    pub sign_in_url: Option<String>,
    /// The environment names it needs (secret values never shown).
    #[serde(default)]
    #[schema(required)]
    pub settings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogSetting {
    pub key: String,
    pub label: String,
    #[serde(default)]
    #[schema(required)]
    pub secret: bool,
    #[serde(default = "default_true")]
    #[schema(required)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub choices: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogOptionKind {
    SignIn,
    Key,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HostOwner {
    Maker,
    Publisher,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Registry {
    Npm,
    Pypi,
    Oci,
}

/// One way to connect: sign in, an API key, or run on this computer. Where it runs: a
/// remote's `host`, and whose host that is; a package's `registry` and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogOption {
    pub id: String,
    #[schema(inline)]
    pub kind: CatalogOptionKind,
    pub label: String,
    #[serde(default)]
    #[schema(required)]
    pub settings: Vec<CatalogSetting>,
    /// A remote's host: what Ghost sends the app goes there.
    #[serde(default)]
    pub host: Option<String>,
    /// The product maker's host, the publisher's own, or someone else's.
    #[serde(default)]
    #[schema(inline)]
    pub host_owner: Option<HostOwner>,
    #[serde(default)]
    #[schema(inline)]
    pub registry: Option<Registry>,
    /// Its name there: "posthog-mcp".
    #[serde(default)]
    pub package: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verified {
    Github,
    Domain,
}

/// Who published an entry, as the registry checked it. Who published it, not whether it
/// is safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogPublisher {
    /// "PostHog", "mcparmory.com"
    pub name: String,
    #[schema(inline)]
    pub verified: Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MakerEvidence {
    Publisher,
    Listed,
    Remote,
    Repository,
    Website,
}

/// The entry is the product maker's own, and what agreed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogMaker {
    pub product: String,
    #[schema(inline)]
    pub evidence: Vec<MakerEvidence>,
}

/// An app in the catalog. Its text is the registry's: shown to the operator, never to the
/// model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CatalogEntryView {
    /// The registry's name, reverse-DNS.
    pub name: String,
    pub title: String,
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub featured: bool,
    #[serde(default)]
    #[schema(required)]
    pub options: Vec<CatalogOption>,
    /// The short name it would be connected under.
    pub suggested_name: String,
    /// The names it is connected under already.
    #[serde(default)]
    #[schema(required)]
    pub connected: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub publisher: Option<CatalogPublisher>,
    #[serde(default)]
    pub maker: Option<CatalogMaker>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub product_maker: Option<String>,
    /// In search results: other publishers' entries for the same product, folded under
    /// the maker's own. Always empty on `GET /catalog/{name}`.
    #[serde(default)]
    #[schema(no_recursion)]
    #[schema(required)]
    pub others: Vec<CatalogEntryView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CatalogPage {
    pub entries: Vec<CatalogEntryView>,
    /// Results, counting a maker's own with what folds under it as one.
    pub total: i64,
    #[serde(default)]
    pub fetched_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[schema(required)]
    pub refreshing: bool,
    #[serde(default)]
    pub problem: Option<String>,
}

/// Connect an app: from the catalog (`catalog` and `option`), from a URL, or from another
/// client's pasted config. Secret values go to Ghost's vault, never into apps.json, and
/// never come back out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct AppConnect {
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub option: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::BTreeMap<String, String>>)]
    pub values: Option<Map<String, Value>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub replace: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppConnectResult {
    pub apps: Vec<AppView>,
    #[serde(default)]
    pub sign_in_url: Option<String>,
    /// What the operator should know about a pasted config: keys Ghost does not read.
    #[serde(default)]
    #[schema(required)]
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Mode {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "read-only")]
    ReadOnly,
}

/// `[permissions]` in policy.toml: everything runs unless a rule says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PermissionsView {
    pub path: String,
    #[schema(inline)]
    pub mode: Mode,
    #[serde(default)]
    #[schema(required)]
    pub block: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub ask: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub quiet: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub allow: Vec<String>,
}

/// One toggle: move `rule` into `level`'s list, or out of every list (None).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RuleSet {
    pub rule: String,
    #[serde(default)]
    #[schema(inline)]
    pub level: Option<Level>,
}

/// Whole lists, the mode, or one rule moved; what is not given is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct PermissionsUpdate {
    #[serde(default)]
    #[schema(inline)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub block: Option<Vec<String>>,
    #[serde(default)]
    pub ask: Option<Vec<String>>,
    #[serde(default)]
    pub quiet: Option<Vec<String>>,
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub set: Option<RuleSet>,
}

// --- skills ------------------------------------------------------------------------------

/// Where one of Ghost's skills was installed from (#35), as its lock file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillOriginView {
    pub source: String,
    pub source_url: String,
    pub skill_path: String,
    pub commit: String,
    pub folder_hash: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub installed_at: Option<String>,
}

/// A skill Ghost can read (ADR 0023): a SKILL.md folder, Ghost's own or the operator's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// "ghost" for Ghost's own folder, "home" for the operator's ~/.agents/skills.
    pub source: String,
    pub path: String,
    #[serde(default)]
    #[schema(required)]
    pub requires: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub missing: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub files: Vec<String>,
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub origin: Option<SkillOriginView>,
}

/// A skill skills.sh lists (#35). Its text is the directory's: shown to the operator,
/// never to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillListing {
    pub id: String,
    pub source: String,
    pub skill: String,
    pub name: String,
    pub installs: i64,
    #[serde(default)]
    pub installed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SkillSearchPage {
    pub query: String,
    #[serde(default)]
    #[schema(required)]
    pub skills: Vec<SkillListing>,
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillPreviewRequest {
    /// owner/repo, owner/repo/skill, or a github.com address of a skill's folder.
    pub source: String,
    #[serde(default)]
    pub skill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillFileView {
    pub path: String,
    pub bytes: u64,
    /// A file an interpreter runs, or one that was executable upstream.
    pub script: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SkillPlace {
    Ghost,
    Home,
}

/// A skill fetched from GitHub and staged, for the operator to read before installing.
/// Everything in it but the hashes is the source's text: untrusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillPreview {
    /// What install takes; good for 30 minutes.
    pub token: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub source_url: String,
    pub skill_path: String,
    pub commit: String,
    pub folder_hash: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// The whole SKILL.md, front matter included.
    pub skill_md: String,
    #[serde(default)]
    #[schema(required)]
    pub files: Vec<SkillFileView>,
    /// The ones ghost-scripts could run, through the gate.
    #[serde(default)]
    #[schema(required)]
    pub scripts: Vec<String>,
    #[serde(default)]
    #[schema(inline)]
    pub installed: Option<SkillPlace>,
    #[serde(default)]
    pub update_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillInstall {
    pub token: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub replace: bool,
}

/// Whether an installed skill's source has moved on, checked when the page asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillUpdateView {
    pub name: String,
    pub source: String,
    pub commit: String,
    #[serde(default)]
    pub latest_commit: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub update: bool,
    #[serde(default)]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillBody {
    pub name: String,
    /// SKILL.md without its front matter, trimmed.
    pub body: String,
    #[serde(default)]
    #[schema(required)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillAdd {
    /// A folder on this machine holding SKILL.md.
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub replace: bool,
}

// --- watching ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Response {
    Tell,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Digest,
    Notify,
    Call,
}

/// A watch in watches.json (ADR 0028, ADR 0035) and how its last look went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WatchInfo {
    pub name: String,
    pub app: String,
    pub read: String,
    pub every_secs: i64,
    /// "check", then "jev" and "model" where declared.
    pub steps: Vec<String>,
    #[schema(inline)]
    pub response: Response,
    #[schema(inline)]
    pub urgency: Urgency,
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_ok: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
    /// Results the last look kept.
    #[serde(default)]
    #[schema(required)]
    pub seen: i64,
    /// Items it holds open now.
    #[serde(default)]
    #[schema(required)]
    pub open: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verify {
    Github,
    Hmac,
}

/// A webhook in hooks.json (ADR 0027, ADR 0035): on or off, and what came of deliveries
/// since the daemon started. Counts only; never a body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HookInfo {
    pub name: String,
    /// POST here on the webhook listener.
    pub path: String,
    #[schema(inline)]
    pub verify: Verify,
    pub events: Vec<String>,
    pub on: bool,
    #[serde(default)]
    pub off_reason: Option<String>,
    /// item, resolved, ignored
    #[serde(default)]
    #[schema(required, value_type = std::collections::BTreeMap<String, i64>)]
    pub accepted: Map<String, Value>,
    /// By reason.
    #[serde(default)]
    #[schema(required, value_type = std::collections::BTreeMap<String, i64>)]
    pub rejected: Map<String, Value>,
    #[serde(default)]
    pub last: Option<DateTime<Utc>>,
}

/// A file Ghost made in its out/ folder (ADR 0033). Ghost never runs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OutputInfo {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub modified: DateTime<Utc>,
}

/// What Ghost watches (ADR 0035): the watches, the webhooks, where the webhook listener
/// is, and what is wrong in those files or left in Ghost's folder from an older layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, ToSchema)]
pub struct WatchingView {
    #[serde(default)]
    #[schema(required)]
    pub watches: Vec<WatchInfo>,
    #[serde(default)]
    #[schema(required)]
    pub hooks: Vec<HookInfo>,
    /// The webhook listener's addresses; empty when it is off.
    #[serde(default)]
    #[schema(required)]
    pub listening: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub problems: Vec<String>,
}

/// Ghost's AGENTS.md (ADR 0035): the rules in every agent's prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RulesView {
    /// The file the operator opens.
    pub path: String,
    pub text: String,
}

/// A rule appended to Ghost's AGENTS.md, verbatim (ADR 0035).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RuleAdd {
    pub text: String,
}

/// A source of tools the gate knows: Ghost's own (built in) or a connected app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SourceInfo {
    pub name: String,
    /// "built-in" or "app"
    pub kind: String,
    pub description: String,
    #[serde(default)]
    #[schema(required)]
    pub operations: Vec<OperationInfo>,
    #[serde(default)]
    #[schema(required)]
    pub read_tools: Vec<ReadToolInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RouteName {
    Digest,
    Desk,
    Desktop,
    Phone,
    Call,
    Sms,
    Whatsapp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Carry {
    Count,
    Title,
}

/// `[routes]` in the policy file (ADR 0031): how Ghost reaches the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RoutesView {
    #[schema(inline)]
    pub enabled: Vec<RouteName>,
    /// Minutes.
    pub escalate_after: i64,
    /// "22:00-07:00"
    #[serde(default)]
    pub quiet_hours: Option<String>,
    pub batch_secs: i64,
    pub calls_per_hour: i64,
    pub calls_per_day: i64,
    #[schema(inline)]
    pub carry: Carry,
    pub greeting: String,
    #[serde(default)]
    #[schema(inline)]
    #[schema(required)]
    pub urgency: std::collections::BTreeMap<String, Urgency>,
    /// The routes this daemon has something behind; the others can be listed but do
    /// nothing yet.
    #[serde(default)]
    #[schema(inline)]
    #[schema(required)]
    pub available: Vec<RouteName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PolicyView {
    pub path: String,
    pub exists: bool,
    pub text: String,
    pub permissions: PermissionsView,
    #[serde(default)]
    pub routes: Option<RoutesView>,
}

/// Either the raw TOML, or the routes rendered into it (the permissions have their own
/// route, `PUT /permissions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct PolicyUpdate {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub routes: Option<RoutesView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PolicyResult {
    pub policy: PolicyView,
    /// Reloaded live.
    pub applied: bool,
}

// --- settings ------------------------------------------------------------------------------

pub const SECRET_KEYS: [&str; 6] = [
    "ANTHROPIC_AUTH_TOKEN",
    "GITHUB_TOKEN",
    "TYPESAFE_API_KEY",
    "OPENROUTER_API_KEY",
    "HF_TOKEN",
    "ELEVENLABS_API_KEY",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SecretState {
    pub name: String,
    pub set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SettingsView {
    pub settings_path: String,
    /// Non-secret settings, as written in settings.env.
    pub values: std::collections::BTreeMap<String, String>,
    /// Ghost's own passwords and the apps', set or not (ADR 0036); /passwords has the rest.
    pub secrets: Vec<SecretState>,
}

/// `values`: a None value deletes the key. `secrets`: write-only, into the vault (ADR
/// 0036); None deletes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub struct SettingsUpdate {
    #[serde(default)]
    pub values: Option<std::collections::BTreeMap<String, Option<String>>>,
    #[serde(default)]
    pub secrets: Option<std::collections::BTreeMap<String, Option<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SettingsResult {
    pub settings: SettingsView,
    /// Keys the daemon picked up without a restart.
    #[serde(default)]
    #[schema(required)]
    pub applied: Vec<String>,
    /// A pending value or a secret: the service must restart.
    #[serde(default)]
    #[schema(required)]
    pub restart_required: bool,
}

// --- passwords (ADR 0036) ------------------------------------------------------------------
// A password goes in and never comes back out: these say what it is called, what uses it
// and when it was last used. No route returns a value.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VaultState {
    Open,
    Locked,
    Unavailable,
    Broken,
}

/// Ghost's vault: one encrypted file, its key in the operator's keyring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VaultView {
    #[schema(inline)]
    pub state: VaultState,
    /// Why it isn't open, in the operator's words.
    #[serde(default)]
    pub problem: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PasswordInfo {
    /// What config files call it: ${NAME}
    pub name: String,
    /// In the vault; None while the vault is closed (locked): unknown.
    #[schema(required)]
    pub set: Option<bool>,
    /// "The model connection", "GitHub" (an app), "Webhook ci"
    #[serde(default)]
    #[schema(required)]
    pub used_by: Vec<String>,
    /// One Ghost itself reads; a new value applies at its next start.
    #[serde(default)]
    #[schema(required)]
    pub ghost: bool,
    #[serde(default)]
    pub added: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated: Option<DateTime<Utc>>,
    /// The last time Ghost filled it in.
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceKind {
    HekaVault,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportSourceView {
    #[schema(inline)]
    pub kind: ImportSourceKind,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PasswordsView {
    pub vault: VaultView,
    pub passwords: Vec<PasswordInfo>,
    /// What an import would take; empty after one.
    #[serde(default)]
    #[schema(required)]
    pub importable: Vec<ImportSourceView>,
}

/// Write-only; never echoed, not even in a validation error.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PasswordSet {
    #[schema(value_type = String, format = Password, write_only)]
    pub value: Secret,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PasswordResult {
    pub passwords: PasswordsView,
    /// Ghost's own: the service must restart to use it.
    #[serde(default)]
    #[schema(required)]
    pub restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PasswordImportResult {
    /// Names now in the vault.
    #[serde(default)]
    #[schema(required)]
    pub imported: Vec<String>,
    /// Already in the vault; left as they were.
    #[serde(default)]
    #[schema(required)]
    pub kept: Vec<String>,
    /// Non-secret settings moved to settings.env.
    #[serde(default)]
    #[schema(required)]
    pub settings: Vec<String>,
    /// Plaintext files deleted.
    #[serde(default)]
    #[schema(required)]
    pub removed: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub notes: Vec<String>,
    pub passwords: PasswordsView,
    #[serde(default)]
    #[schema(required)]
    pub restart_required: bool,
}

// --- devices + setup -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceInfo {
    pub index: i64,
    pub name: String,
    pub max_input_channels: i64,
    pub max_output_channels: i64,
    pub default_samplerate: f64,
    pub is_default_input: bool,
    pub is_default_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DevicesView {
    pub devices: Vec<DeviceInfo>,
    #[schema(required)]
    pub input_device: Option<String>,
    #[schema(required)]
    pub output_device: Option<String>,
    pub echo_cancel_active: bool,
    pub echo_cancel_source_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelsView {
    pub ready: bool,
    pub cache_dir: String,
    pub kokoro_present: bool,
    pub stt_model: String,
}

// --- conversations (ADR 0014) --------------------------------------------------------------

/// An agent (ADR 0014, ADR 0035): a conversation with a persona and a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConversationView {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub job: String,
    /// The voice heard: its own, else Ghost's.
    pub voice: String,
    pub speed: f64,
    /// True when the voice is the agent's own.
    #[serde(default)]
    #[schema(required)]
    pub own_voice: bool,
    pub created_at: DateTime<Utc>,
    #[schema(required)]
    pub last_active_at: Option<DateTime<Utc>>,
    /// Set: off the roster and voice routing, still readable.
    #[serde(default)]
    pub retired_at: Option<DateTime<Utc>>,
    /// The running session is with it.
    pub active: bool,
    pub is_ghost: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConversationDetail {
    #[serde(flatten)]
    pub view: ConversationView,
    pub persona: String,
    pub memory: String,
    pub notes: Vec<String>,
    pub turn_count: i64,
    pub compactions: i64,
    pub dir: String,
}

/// Hire an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConversationCreate {
    pub name: String,
    #[serde(default)]
    #[schema(required)]
    pub aliases: Vec<String>,
    #[serde(default)]
    #[schema(required)]
    pub job: String,
    /// None: Ghost's voice.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    #[schema(required)]
    pub persona: String,
    /// Slug of a retired agent whose memory.md becomes this one's first memory, as a
    /// handoff. Never Ghost.
    #[serde(default)]
    pub seed_from: Option<String>,
}

/// Each field present is applied as its own audited write. `voice` "" means back to
/// Ghost's voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, ToSchema)]
pub struct ConversationUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub persona: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub notes: Option<Vec<String>>,
    #[serde(default)]
    pub seed_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TurnViewRole {
    User,
    Assistant,
    Call,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TurnView {
    pub id: i64,
    pub session_id: String,
    #[schema(inline)]
    pub role: TurnViewRole,
    pub text: String,
    pub at: DateTime<Utc>,
    pub compacted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CompactionView {
    pub id: String,
    pub at: DateTime<Utc>,
    pub source: String,
    pub summary: String,
    #[schema(required)]
    pub through_turn_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VoiceInfo {
    pub id: String,
    /// "American female: Heart"
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SampleRequest {
    #[serde(default = "default_speed")]
    #[schema(required)]
    pub speed: f64,
    #[serde(default)]
    pub text: Option<String>,
}

fn default_speed() -> f64 {
    1.0
}

// --- the paired phone (ADR 0016) -----------------------------------------------------------

/// A fresh pairing code for the phone listener. Shown to the operator only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PhonePairing {
    /// Six digits.
    pub code: String,
    /// The first GHOST_PHONE_LISTEN address, host:port; what the operator types.
    pub address: String,
    /// Every address the listener serves.
    #[serde(default)]
    pub addresses: Vec<String>,
    pub expires_at: DateTime<Utc>,
    /// The QR code's payload: ghost://pair?code=…&address=…, every address in order.
    pub link: String,
}

/// The network a phone address is on (ADR 0020): the home network, or the operator's
/// WireGuard tunnel for away from home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PhoneNetwork {
    Home,
    Tunnel,
}

/// One `GHOST_PHONE_LISTEN` address and the network it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneAddress {
    pub address: String,
    /// None when Ghost can't tell: the address is on every interface (0.0.0.0), or on
    /// none of this computer's right now.
    #[schema(required)]
    #[schema(inline)]
    pub network: Option<PhoneNetwork>,
    /// Bound now; a tunnel address whose interface isn't up yet is retried.
    pub listening: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PhoneDevice {
    pub id: String,
    pub name: String,
    pub paired_at: DateTime<Utc>,
    /// The phone's last verified request.
    #[schema(required)]
    pub last_seen: Option<DateTime<Utc>>,
    /// The listen address that request came in on.
    #[serde(default)]
    pub last_address: Option<String>,
    /// And the network it came over.
    #[serde(default)]
    #[schema(inline)]
    pub last_network: Option<PhoneNetwork>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PhoneView {
    /// None when the listener is off.
    #[schema(required)]
    pub address: Option<String>,
    /// LAN first, then the tunnel (ADR 0020).
    #[serde(default)]
    pub addresses: Vec<String>,
    /// The same, with each network.
    #[serde(default)]
    pub listen: Vec<PhoneAddress>,
    #[schema(required)]
    pub device: Option<PhoneDevice>,
}

// --- the model connection (ADR 0037) -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Anthropic,
    Openai,
}

/// One way to reach a model, as the Model page offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelProviderView {
    pub id: String,
    pub title: String,
    pub blurb: String,
    #[schema(inline)]
    pub protocol: Protocol,
    /// "" for a URL of the operator's own.
    pub base_url: String,
    /// The Password's name it uses by default.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub password_optional: bool,
    /// OpenRouter: signing in gives the key.
    #[serde(default)]
    #[schema(required)]
    pub sign_in: bool,
    #[serde(default)]
    #[schema(required)]
    pub local: bool,
    /// None: the first the provider lists.
    #[serde(default)]
    pub default_model: Option<String>,
}

/// The model connection now. No key is in it: only whether its Password is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ModelConnectionView {
    pub provider: String,
    #[schema(inline)]
    pub protocol: Protocol,
    /// Where calls go.
    pub base_url: String,
    /// Set when it isn't the provider's own.
    #[serde(default)]
    pub base_url_override: Option<String>,
    /// As saved; "" means the provider's default.
    pub model: String,
    /// What is sent.
    pub wire_model: String,
    /// The name.
    #[serde(default)]
    pub password: Option<String>,
    pub password_set: bool,
    #[serde(default)]
    #[schema(required)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_secs: Option<f64>,
    /// The older setting it comes from ("GHOST_LLM_ENDPOINT=openrouter").
    #[serde(default)]
    pub legacy: Option<String>,
    #[serde(default)]
    #[schema(required)]
    pub problems: Vec<String>,
    pub ready: bool,
    /// The GHOST_LLM_* values that say this connection explicitly.
    #[serde(default)]
    #[schema(required)]
    pub values: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    #[schema(required)]
    pub providers: Vec<ModelProviderView>,
    /// OpenRouter is waiting for the browser.
    #[serde(default)]
    #[schema(required)]
    pub sign_in_pending: bool,
    /// The last sign-in didn't go through.
    #[serde(default)]
    pub sign_in_error: Option<String>,
}

/// A connection being edited: GHOST_LLM_* values over the saved ones (None removes one),
/// and a key typed but not saved yet, used for this one request and dropped.
#[derive(Debug, Clone, Deserialize, Default, ToSchema)]
pub struct ModelDraft {
    #[serde(default)]
    #[schema(required)]
    pub values: std::collections::BTreeMap<String, Option<String>>,
    /// Write-only, like `PasswordSet.value`: never echoed.
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Password, write_only, max_length = 4096)]
    pub password_value: Option<Secret>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelOptionView {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelListView {
    pub models: Vec<ModelOptionView>,
    /// The provider's default, when it lists it.
    #[serde(default)]
    pub default: Option<String>,
    /// Why the list couldn't be read; models is empty then.
    #[serde(default)]
    pub error: Option<String>,
}

/// One tiny request, timed like `make perf-llm`: request to first token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelTestResult {
    pub ok: bool,
    #[serde(default)]
    pub ttft_ms: Option<i64>,
    #[serde(default)]
    pub total_ms: Option<i64>,
    /// What was asked for.
    pub model: String,
    /// What the provider said answered, when it says.
    #[serde(default)]
    pub answered_by: Option<String>,
    /// The first words, shortened.
    #[serde(default)]
    pub reply: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// Provider, protocol and host; never a key.
    #[serde(rename = "where")]
    pub where_: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ModelSignInResult {
    /// The desktop opens it; the loopback takes the browser's way back.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_discriminated_by_type() {
        let e = Event::Waiting { count: 3 };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type": "waiting", "count": 3})
        );
        let e: Event = serde_json::from_value(
            json!({"type": "apps", "changed": "github", "sign_in_url": null}),
        )
        .unwrap();
        assert_eq!(
            e,
            Event::Apps {
                changed: Some("github".into()),
                sign_in_url: None
            }
        );
        let e = Event::Log {
            level: "info".into(),
            message: "hi".into(),
            at: Utc::now(),
        };
        assert_eq!(serde_json::to_value(&e).unwrap()["type"], "log");
    }

    #[test]
    fn a_secret_never_shows_and_is_never_serialized() {
        let s: PasswordSet = serde_json::from_value(json!({"value": "hunter2-hunter2"})).unwrap();
        assert_eq!(s.value.expose(), "hunter2-hunter2");
        assert_eq!(format!("{:?}", s.value), "Secret(**********)");
        let d: ModelDraft = serde_json::from_value(
            json!({"values": {"GHOST_LLM_MODEL": null}, "password_value": "k"}),
        )
        .unwrap();
        assert!(d.password_value.is_some());
        assert_eq!(d.values["GHOST_LLM_MODEL"], None);
    }

    #[test]
    fn the_status_and_a_detail_keep_the_python_shape() {
        let v = serde_json::to_value(Build::default()).unwrap();
        assert_eq!(
            v,
            json!({"kind": "dev", "commit": null, "dirty": false, "launcher": null})
        );
        let view = ConversationView {
            id: "i".into(),
            slug: "ledger".into(),
            name: "Ledger".into(),
            aliases: vec![],
            job: String::new(),
            voice: "af_heart".into(),
            speed: 1.0,
            own_voice: false,
            created_at: Utc::now(),
            last_active_at: None,
            retired_at: None,
            active: false,
            is_ghost: false,
        };
        let detail = ConversationDetail {
            view,
            persona: "p".into(),
            memory: "m".into(),
            notes: vec![],
            turn_count: 0,
            compactions: 0,
            dir: "/d".into(),
        };
        let v = serde_json::to_value(&detail).unwrap();
        assert_eq!(v["slug"], "ledger");
        assert_eq!(v["persona"], "p");
        let m: Mode = serde_json::from_value(json!("read-only")).unwrap();
        assert_eq!(m, Mode::ReadOnly);
        assert_eq!(
            serde_json::to_value(AppState::NeedsSignIn).unwrap(),
            "needs_sign_in"
        );
        assert_eq!(serde_json::to_value(Launcher::MakeDev).unwrap(), "make-dev");
        assert_eq!(
            serde_json::to_value(ImportSourceKind::HekaVault).unwrap(),
            "heka_vault"
        );
    }
}
