//! What a source of tools gives Ghost: a prompt paragraph, typed read tools, write
//! operations for the gate, and the per-turn hooks Ghost's own parts use.
//!
//! Sources are core: the agents (ADR 0014), Ghost's fixed intents, and every app the
//! operator connects (ADR 0034). There is no package mechanism; a `ToolSource` is built
//! in code from what an app or a core module offers.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::levels::Risk;
use crate::models::{Change, ChangeSet, Snapshot, Target};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ToolResult = Map<String, Value>;
pub type ToolHandler =
    Arc<dyn Fn(Map<String, Value>) -> BoxFuture<'static, anyhow::Result<ToolResult>> + Send + Sync>;
pub type PrepareFn =
    Arc<dyn Fn(Map<String, Value>) -> BoxFuture<'static, anyhow::Result<Prepared>> + Send + Sync>;
pub type AllowedFn = Arc<dyn Fn(&Target) -> bool + Send + Sync>;

/// A typed, read-only function the model may call directly.
#[derive(Clone)]
pub struct ReadTool {
    pub name: String,
    pub description: String,
    pub properties: Value,
    pub required: Vec<String>,
    pub handler: ToolHandler,
    /// The name a permission rule knows it by (an app's own tool name); the tool's when "".
    pub operation: String,
}

/// What an operation hands to the gate: a target, a typed change set, and why.
/// `asked_by` are the words that ask for this call, when they depend on it (an app's
/// tool, ADR 0034); empty means the operation's own.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub target: Target,
    pub changes: ChangeSet,
    pub rationale: String,
    pub asked_by: Vec<String>,
}

/// The one interface the gate uses to touch the world. Provided by a source.
#[async_trait]
pub trait Writer: Send + Sync {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot>;
    /// Apply one change. `idempotency_key` is unique per proposal; a writer that creates
    /// something (a comment, a message) must embed it so a retry never creates a second
    /// copy.
    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        idempotency_key: &str,
    ) -> anyhow::Result<()>;
}

/// A named change. The model supplies `properties`; `prepare` validates them into a
/// `Prepared`; the gate applies it through `writer` at the level the operator's
/// permissions give it (ADR 0034). `risk` is the source's own note, shown, never the
/// level. `allowed` is the source's own limit (an agent's folder); the gate checks it
/// before anything else. `tool_name` set: the model calls it as its own tool under that
/// name (an app's change); unset, through `change`.
#[derive(Clone)]
pub struct Operation {
    pub name: String,
    pub description: String,
    pub properties: Value,
    pub required: Vec<String>,
    pub prepare: PrepareFn,
    pub writer: Arc<dyn Writer>,
    pub risk: Risk,
    pub allowed: AllowedFn,
    /// Words in the operator's request that name this operation ("close", "shut"). When
    /// a confirm-level proposal follows a request that plainly asks for it on that
    /// target, the request is the yes (`request`). Empty: always ask.
    pub asked_by: Vec<String>,
    pub tool_name: Option<String>,
}

impl Operation {
    /// An operation with the defaults: medium risk, every target allowed, no words.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        prepare: PrepareFn,
        writer: Arc<dyn Writer>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            properties: Value::Object(Map::new()),
            required: Vec::new(),
            prepare,
            writer,
            risk: Risk::Medium,
            allowed: Arc::new(|_| true),
            asked_by: Vec::new(),
            tool_name: None,
        }
    }
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation")
            .field("name", &self.name)
            .field("risk", &self.risk)
            .finish()
    }
}

// --- per-turn hooks -----------------------------------------------------------------

/// What the operator just said, with the context an interceptor needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurn {
    pub text: String,
    /// A confirm-level change is waiting for a yes.
    pub pending: bool,
    /// Ghost's previous reply.
    pub last_assistant: String,
    /// Said over Ghost: the turn cut its reply off. A hold phrase then means "stop
    /// talking", not "I'm turning away" (ADR 0021).
    pub barged_in: bool,
}

/// What an interceptor does with the turn. `HangUp` ends the session; `Hold` keeps it
/// while the operator talks to someone else (ADR 0021); `Cancel` drops a pending change;
/// `Quiet` is do-not-disturb until `until` (ADR 0031), the operator's own words, never the
/// model's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    HangUp,
    Hold,
    Cancel,
    Quiet,
}

/// An interceptor took the turn. The model never sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Handled {
    pub say: Option<String>,
    pub action: Option<Action>,
    pub until: Option<DateTime<Utc>>,
}

/// Looks at a user turn before the model does. `Some` takes it, `None` passes. Runs
/// inside a session only; never on the passive path.
#[async_trait]
pub trait TurnInterceptor: Send + Sync {
    fn name(&self) -> &str;
    async fn on_user_turn(&self, turn: &UserTurn) -> Option<Handled>;
}

/// Where in the voice pipeline a processor sits (Ghost's own, by slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Raw mic frames in session.
    AudioIn,
    /// Transcription frames.
    AfterStt,
    /// Aggregated user turn.
    BeforeLlm,
    /// Model text and tool frames.
    AfterLlm,
    /// Sentences to speak.
    BeforeTts,
    /// TTS audio.
    AudioOut,
}

// --- lifecycle ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Wake,
    SessionStart,
    SessionEnd,
    Outcome,
    Error,
    Quiet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    pub data: Map<String, Value>,
}

pub type EventListener = Arc<dyn Fn(Event) -> BoxFuture<'static, ()> + Send + Sync>;

/// One source of tools, as the pipeline and the gate take it. `name` is the key the
/// permissions, the gate registry and the audit use (`github`, `conversations`);
/// `title` is how the operator knows it ("GitHub").
#[derive(Clone, Default)]
pub struct ToolSource {
    pub name: String,
    pub prompt: String,
    pub read_tools: Vec<ReadTool>,
    pub operations: Vec<Operation>,
    pub interceptors: Vec<Arc<dyn TurnInterceptor>>,
    pub listeners: Vec<EventListener>,
    pub title: String,
    /// A connected app (`apps.json`), not one of Ghost's in-tree sources: a call to it
    /// leaves the machine or runs a program, and is what Ghost acknowledges aloud before
    /// the answer (`ghost_voice::acks`).
    pub app: bool,
}

impl ToolSource {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
}
