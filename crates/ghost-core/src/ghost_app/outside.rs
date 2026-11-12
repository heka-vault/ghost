//! Ghost's own app, served to outside clients (ADR 0048): what a call from a tool on this
//! computer goes through before it reaches the same server Ghost's agents use.
//!
//! In process a change reaches the server through the session's dispatcher and the gate.
//! A call from outside has no session, so [`GhostServer::dispatch`] does the equivalent:
//!
//! - the caller is the client the listener authenticated ([`ClientId`], in the request's
//!   own extensions), named `client:<name>`; no argument is believed, and a `_`-led one
//!   the client wrote is dropped;
//! - every client has a budget of calls a minute, and a smaller one of things said to the
//!   operator;
//! - a read is checked against the operator's `block` rules and logged at debug level;
//! - every other tool is a change through the gate's own `submit`, as `ghost.<tool>`:
//!   `block` refuses, `ask` leaves it on the waiting list for the operator's yes and the
//!   client is told it is waiting, anything else runs, and the audit has a row for it
//!   under the session `client:<name>`.
//!
//! The server keeps nothing per connection: the limits, the events and the gate are all in
//! one shared [`Outside`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListResourcesResult, ListToolsResult,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ResultType,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::clients::{CLIENT_SOURCE, ClientId, client_source, shown};
use super::{Caller, GhostServer, NAME, Parts, SUMMARY_MAX, clip, reply};
use crate::apps::CALLER_ARG;
use crate::conversations::models::Profile;
use crate::gate::{Request, WriteGate};
use crate::models::{
    AppChange, AppSnapshot, AppTarget, Change, ChangeSet, OutcomeStatus, Snapshot, Target,
};
use crate::source::{Operation, Prepared, Writer};
use crate::store::Item;
use crate::waiting::{self, Response, State, WaitingList};

/// Calls a client may make in a minute, reads and changes together.
pub const CALLS_PER_MINUTE: usize = 60;
/// Of those, how many may tell the operator something (`notify`, `say`).
pub const NOTIFIES_PER_MINUTE: usize = 10;
/// The longest `events_wait` holds a request.
pub const WAIT_MAX_SECS: f64 = 60.0;
pub const WAIT_DEFAULT_SECS: f64 = 30.0;
/// How many events are kept for a client that comes back for them.
pub const EVENTS_KEPT: usize = 256;
/// The tools that tell the operator something: the smaller budget.
pub const TELLING: [&str; 2] = ["notify", "say"];
/// What an outside client is told about Ghost when it connects.
pub const INSTRUCTIONS: &str = "Ghost is the operator's voice assistant on this computer. Use notify to tell the operator \
something (it goes on their waiting list and reaches them by voice, on the desktop or on the \
phone), resolve once it no longer matters, and events_wait to wait for them to acknowledge \
it. ask puts a question to the operator; events_wait says when it is settled and question \
reads the answer, which is theirs alone. status, docs_search and docs_read say what Ghost is and how it is now. A change the \
operator's permissions ask about answers \"waiting for the operator\": it has not run yet.";

pub const DOCS_URI: &str = "ghost://docs/";
pub const STATUS_URI: &str = "ghost://status";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- limits ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub calls: usize,
    pub notifies: usize,
    pub window: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            calls: CALLS_PER_MINUTE,
            notifies: NOTIFIES_PER_MINUTE,
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Default)]
struct Hits {
    calls: VecDeque<Instant>,
    notifies: VecDeque<Instant>,
}

/// A sliding window per client.
struct Limiter {
    limits: Limits,
    hits: Mutex<HashMap<String, Hits>>,
}

impl Limiter {
    fn take(&self, client: &str, telling: bool) -> Result<(), String> {
        let now = Instant::now();
        let Limits {
            calls,
            notifies,
            window,
        } = self.limits;
        let mut all = lock(&self.hits);
        let hits = all.entry(client.to_string()).or_default();
        for q in [&mut hits.calls, &mut hits.notifies] {
            while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
                q.pop_front();
            }
        }
        let wait = |q: &VecDeque<Instant>| {
            q.front()
                .map_or(0, |t| {
                    window.saturating_sub(now.duration_since(*t)).as_secs()
                })
                .max(1)
        };
        if hits.calls.len() >= calls {
            return Err(format!(
                "Too many calls: a client may make {calls} a minute. Try again in {} s.",
                wait(&hits.calls)
            ));
        }
        if telling && hits.notifies.len() >= notifies {
            return Err(format!(
                "Too many things told to the operator: a client may send {notifies} a minute. \
                 Try again in {} s.",
                wait(&hits.notifies)
            ));
        }
        hits.calls.push_back(now);
        if telling {
            hits.notifies.push_back(now);
        }
        Ok(())
    }
}

// --- events ---------------------------------------------------------------------------------

struct Log {
    /// The number the next event gets; the first is 1.
    next: u64,
    ring: VecDeque<(u64, Value)>,
}

/// What changed, numbered, for `events_wait`: a short ring every client reads from its own
/// cursor. Nothing here is a log line, a transcript or audio; [`Feed`] decides what goes
/// in.
pub struct EventLog {
    log: Mutex<Log>,
    changed: tokio::sync::Notify,
}

impl Default for EventLog {
    fn default() -> Self {
        Self {
            log: Mutex::new(Log {
                next: 1,
                ring: VecDeque::with_capacity(EVENTS_KEPT),
            }),
            changed: tokio::sync::Notify::new(),
        }
    }
}

impl EventLog {
    pub fn push(&self, event: Value) {
        {
            let mut log = lock(&self.log);
            let n = log.next;
            log.next += 1;
            if log.ring.len() >= EVENTS_KEPT {
                log.ring.pop_front();
            }
            log.ring.push_back((n, event));
        }
        self.changed.notify_waiters();
    }

    /// The number of the newest event; 0 before the first.
    pub fn cursor(&self) -> u64 {
        lock(&self.log).next - 1
    }

    /// The events after `cursor`. A cursor from before a restart (ahead of this log) reads
    /// from the start.
    pub fn since(&self, cursor: u64) -> Vec<(u64, Value)> {
        let log = lock(&self.log);
        let cursor = if cursor >= log.next { 0 } else { cursor };
        log.ring
            .iter()
            .filter(|(n, _)| *n > cursor)
            .cloned()
            .collect()
    }

    /// The events after `since` (now, when None), waiting up to `timeout` for the first.
    /// Returns them with the cursor to pass next time.
    pub async fn wait(&self, since: Option<u64>, timeout: Duration) -> (Vec<(u64, Value)>, u64) {
        let since = since.unwrap_or_else(|| self.cursor());
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let events = self.since(since);
            if let Some((last, _)) = events.last() {
                let cursor = *last;
                return (events, cursor);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let now = self.cursor();
                // A cursor from before a restart starts over.
                return (Vec::new(), if since > now { now } else { since });
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    source: String,
    title: String,
    state: &'static str,
    changed_at: chrono::DateTime<chrono::Utc>,
}

fn state_word(item: &Item) -> &'static str {
    match item.state(None) {
        State::Snoozed => "snoozed",
        State::Acknowledged => "acknowledged",
        State::New | State::Delivered => "waiting",
    }
}

/// Turns what the daemon's event bus says into the events an outside client may wait on:
/// the waiting list (an item added, changed, acknowledged, snoozed or resolved, found by
/// comparing the list with the last look), a change's outcome, and a session starting or
/// ending. Everything else the bus carries (log lines, turns, the voice's level, the
/// Hearing test, an app's sign-in address) is dropped here.
pub struct Feed {
    log: Arc<EventLog>,
    waiting: Arc<WaitingList>,
    seen: Mutex<HashMap<String, Seen>>,
    session: Mutex<bool>,
}

impl Feed {
    pub fn new(log: Arc<EventLog>, waiting: Arc<WaitingList>) -> Self {
        let feed = Self {
            log,
            waiting,
            seen: Mutex::new(HashMap::new()),
            session: Mutex::new(false),
        };
        *lock(&feed.seen) = feed.look(); // what is there at the start is not news
        feed
    }

    fn look(&self) -> HashMap<String, Seen> {
        self.waiting
            .items
            .open(Some(Response::Tell))
            .unwrap_or_default()
            .into_iter()
            .map(|i| {
                let seen = Seen {
                    state: state_word(&i),
                    source: i.source,
                    title: i.title,
                    changed_at: i.changed_at,
                };
                (i.key, seen)
            })
            .collect()
    }

    /// The waiting list may have changed: say how.
    pub fn waiting_changed(&self) {
        let now = self.look();
        let mut events = Vec::new();
        {
            let mut seen = lock(&self.seen);
            let item = |change: &str, key: &str, s: &Seen| json!({"type": "waiting", "change": change, "key": key, "source": s.source, "title": s.title});
            for (key, s) in &now {
                match seen.get(key) {
                    None => events.push(item("added", key, s)),
                    Some(was) if was.changed_at != s.changed_at || was.title != s.title => {
                        events.push(item("changed", key, s));
                    }
                    Some(was) if was.state != s.state => events.push(item(s.state, key, s)),
                    Some(_) => {}
                }
            }
            for (key, was) in seen.iter() {
                if !now.contains_key(key) {
                    events.push(item("resolved", key, was));
                }
            }
            *seen = now;
        }
        for e in events {
            self.log.push(e);
        }
    }

    /// One event from the daemon's bus, as the JSON the control API's event stream sends.
    pub fn on_bus(&self, event: &Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("waiting" | "pending") => self.waiting_changed(),
            Some("outcome") => {
                let o = &event["outcome"];
                // Whose change it was: the session it was proposed in. A client is told
                // what came of its own; of anyone else's, only that one ended.
                let session = o["proposal_id"]
                    .as_str()
                    .and_then(|id| self.waiting.items.store().get_proposal(id).ok().flatten())
                    .map(|p| p.session_id)
                    .unwrap_or_default();
                self.log.push(json!({
                    "type": "change",
                    "id": o["proposal_id"],
                    "outcome": o["status"],
                    "source": session,
                    "target": o["target"],
                    "summary": clip(o["summary"].as_str().unwrap_or_default(), SUMMARY_MAX),
                }));
                self.waiting_changed(); // a change that was waiting is no longer
            }
            // A question was asked or settled (ADR 0051): who asked it, its state, and for
            // the asker alone its words and the answer (`events_wait` strips them for others).
            Some("question") => {
                let q = &event["question"];
                if q["kind"] == "change" {
                    return; // a change's own events say what came of it
                }
                self.log.push(json!({
                    "type": "question",
                    "key": q["key"],
                    "source": q["source"],
                    "state": q["state"],
                    "title": q["question"],
                    "answer": q["answer"],
                    "choice": q["choice"],
                }));
            }
            Some("status") => {
                let s = &event["status"];
                let phase = s["phase"].as_str().unwrap_or_default();
                let open = s["session_id"].is_string() && !matches!(phase, "passive" | "starting");
                let mut was = lock(&self.session);
                if open != *was {
                    *was = open;
                    self.log.push(json!({
                        "type": "session",
                        "state": if open { "started" } else { "ended" },
                        "origin": s["session_origin"],
                        "agent": s["conversation"],
                    }));
                }
            }
            _ => {}
        }
    }
}

// --- what the daemon plugs in -----------------------------------------------------------------

/// Runs one written turn with an agent, with no audio: what `agent_ask` needs. The daemon
/// plugs in its headless run (ADR 0047: `ghostd::outside::AskRunner`); without one
/// [`NoRunner`] says so. The turn stops when the request it was made for does
/// (`crate::apps::current_cancel`).
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// `text` as a turn for `agent`, from `from` (`client:<name>`); the agent's written
    /// answer. Never spoken at the desk.
    async fn ask(&self, agent: &Profile, text: &str, from: &str) -> Result<String, String>;
}

/// No way to run a turn without a voice session yet.
pub struct NoRunner;

#[async_trait]
impl AgentRunner for NoRunner {
    async fn ask(&self, agent: &Profile, _text: &str, _from: &str) -> Result<String, String> {
        Err(format!(
            "Not available yet: Ghost cannot give {} a written turn without a voice session. \
             Use notify to leave the operator a message instead.",
            agent.name
        ))
    }
}

/// What came of asking the desk to speak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// Ghost is saying it now; whether the operator answers is an event later.
    Speaking,
    /// The desk is not free, or it is a quiet time: why, in a few words.
    Busy(String),
}

/// Ghost's voice at the desk for one sentence (ADR 0032's desk voice): spoken only when
/// the desk is idle and the operator let Ghost speak first.
#[async_trait]
pub trait DeskVoice: Send + Sync {
    async fn say(&self, line: &str, item: &Item) -> Said;
}

/// No desk: everything is left on the waiting list.
pub struct NoDesk;

#[async_trait]
impl DeskVoice for NoDesk {
    async fn say(&self, _line: &str, _item: &Item) -> Said {
        Said::Busy("the desk voice is not available".into())
    }
}

// --- the shared state -------------------------------------------------------------------------

/// What a server for outside clients has beyond [`Parts`]: the gate its changes go
/// through, the limits, the events, and the daemon's own parts for `agent_ask` and `say`.
pub struct Outside {
    pub(super) gate: Arc<WriteGate>,
    pub events: Arc<EventLog>,
    limiter: Limiter,
    pub(super) runner: Arc<dyn AgentRunner>,
    pub(super) desk: Arc<dyn DeskVoice>,
    calls: Once,
}

// --- a change sent twice --------------------------------------------------------------------

/// The `_meta` key of a tool call that may be sent again (`ghostd mcp serve` sets it on
/// every call it forwards): the same key from the same client within [`ONCE_KEPT`] gets
/// the first call's result, and the change is not made twice. The phone's signed `POST`s
/// do the same by their `id` (ADR 0051).
pub const IDEMPOTENCY_KEY: &str = "ghost/idempotency-key";
/// How long a call's result is kept for a resend, from when it finished.
pub const ONCE_KEPT: Duration = Duration::from_secs(300);
const KEY_MAX: usize = 128;

type Outcome = Result<Value, String>;

enum Slot {
    /// Still running: a resend waits for it.
    Going(tokio::sync::watch::Receiver<Option<Outcome>>),
    Done(Instant, Outcome),
}

/// What each client's keyed calls came to.
#[derive(Default)]
struct Once {
    slots: Mutex<HashMap<(String, String), Slot>>,
}

/// The first call with a key: what it came to is recorded here, for a resend.
pub(super) struct First {
    key: (String, String),
    tx: tokio::sync::watch::Sender<Option<Outcome>>,
}

enum Begun {
    First(First),
    Again(tokio::sync::watch::Receiver<Option<Outcome>>),
    Done(Outcome),
}

impl Once {
    fn begin(&self, client: &str, key: &str) -> Begun {
        let mut slots = lock(&self.slots);
        slots.retain(|_, s| match s {
            // A first call whose task is gone without a result is not waited on.
            Slot::Going(rx) => rx.has_changed().is_ok(),
            Slot::Done(at, _) => at.elapsed() < ONCE_KEPT,
        });
        let k = (client.to_string(), key.to_string());
        match slots.get(&k) {
            Some(Slot::Going(rx)) => Begun::Again(rx.clone()),
            Some(Slot::Done(_, out)) => Begun::Done(out.clone()),
            None => {
                let (tx, rx) = tokio::sync::watch::channel(None);
                slots.insert(k.clone(), Slot::Going(rx));
                Begun::First(First { key: k, tx })
            }
        }
    }

    fn finish(&self, first: First, out: &Outcome) {
        lock(&self.slots).insert(first.key, Slot::Done(Instant::now(), out.clone()));
        let _ = first.tx.send(Some(out.clone()));
    }
}

/// Cancelled when the HTTP response a request is answered on is dropped: the tool closed
/// the connection. The listener (`ghostd::outside`) puts it in the request's extensions.
/// On the protocol's stateful sessions the server keeps a call running when its
/// connection goes, so this, not the call's future being dropped, is what stops a change
/// that takes long (`agent_ask`) when the tool that asked for it is gone.
#[derive(Clone, Debug)]
pub struct RequestGone(pub CancellationToken);

fn request_gone(context: &RequestContext<RoleServer>) -> Option<CancellationToken> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<RequestGone>())
        .map(|g| g.0.clone())
}

/// The key a client sent with a call, when it is a usable one.
fn idempotency_key(
    request: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Option<String> {
    request
        .meta
        .as_ref()
        .and_then(|m| m.get(IDEMPOTENCY_KEY).cloned())
        .or_else(|| context.meta.get(IDEMPOTENCY_KEY).cloned())
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|k| (1..=KEY_MAX).contains(&k.chars().count()))
}

impl Outside {
    pub fn new(gate: Arc<WriteGate>) -> Self {
        Self {
            gate,
            events: Arc::new(EventLog::default()),
            limiter: Limiter {
                limits: Limits::default(),
                hits: Mutex::new(HashMap::new()),
            },
            runner: Arc::new(NoRunner),
            desk: Arc::new(NoDesk),
            calls: Once::default(),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limiter.limits = limits;
        self
    }

    pub fn with_events(mut self, events: Arc<EventLog>) -> Self {
        self.events = events;
        self
    }

    pub fn with_runner(mut self, runner: Arc<dyn AgentRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn with_desk(mut self, desk: Arc<dyn DeskVoice>) -> Self {
        self.desk = desk;
        self
    }
}

/// The gate's writer for a change only outside clients have (`agent_ask`, `say`): nothing
/// to read first, then the tool itself. What it answered is the read-back, once.
struct OutsideWriter {
    parts: Arc<Parts>,
    outside: Weak<Outside>,
    answers: Mutex<HashMap<String, Map<String, Value>>>,
}

#[async_trait]
impl Writer for OutsideWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        Ok(Snapshot::App(AppSnapshot {
            version: String::new(),
            checked: false,
            result: lock(&self.answers).remove(&target.key()),
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        _idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let Change::App(c) = change else {
            anyhow::bail!("not a change to Ghost's own app");
        };
        let Some(outside) = self.outside.upgrade() else {
            anyhow::bail!("Ghost is not serving outside clients now");
        };
        let from = c
            .arguments
            .get(CALLER_ARG)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let answer = outside
            .run(&self.parts, &c.tool, from, &c.arguments)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let answer = match answer {
            Value::Object(m) => m,
            other => Map::from_iter([("result".to_string(), other)]),
        };
        lock(&self.answers).insert(target.key(), answer);
        Ok(())
    }
}

impl GhostServer {
    /// The same server for outside clients: the tools only they have are added, and every
    /// call goes through [`GhostServer::dispatch`]'s checks.
    pub fn for_outside(&self, outside: Outside) -> Self {
        let mut tool_router = self.tool_router.clone();
        tool_router.merge(Self::outside_router());
        // Finding a connected app's tool (ADR 0046) is for an agent, which can call it. An
        // outside tool cannot, and what the search returns is the apps' own text. And what
        // ADR 0052 keeps from outside tools: an agent's memory, the operator's settings and
        // permissions, the apps' own text, and every change to apps, watches, quiet,
        // outputs and skills (`more::NOT_FOR_OUTSIDE`).
        for name in super::NOT_FOR_OUTSIDE {
            tool_router.remove_route(name);
        }
        // Only the operator answers a question (ADR 0051): an outside tool never can.
        tool_router.remove_route("answer");
        let server = Self {
            tool_router,
            outside: Some(Arc::new(outside)),
            ..self.clone()
        };
        server.register_operations();
        server
    }

    pub fn is_outside(&self) -> bool {
        self.outside.is_some()
    }

    /// The events `events_wait` reads; the daemon's [`Feed`] writes them.
    pub fn events(&self) -> Option<Arc<EventLog>> {
        self.outside.as_ref().map(|o| o.events.clone())
    }

    pub(super) fn outside(&self) -> Result<&Arc<Outside>, String> {
        self.outside
            .as_ref()
            .ok_or_else(|| "this tool is for outside clients".to_string())
    }

    /// The changes only outside clients have, as the gate's operations under `ghost`.
    fn outside_operations(&self) -> Vec<Operation> {
        let Some(outside) = &self.outside else {
            return Vec::new();
        };
        let writer: Arc<dyn Writer> = Arc::new(OutsideWriter {
            parts: self.parts.clone(),
            outside: Arc::downgrade(outside),
            answers: Mutex::new(HashMap::new()),
        });
        Self::outside_router()
            .list_all()
            .into_iter()
            .filter(|t| !is_read(t))
            .map(|t| {
                let name = t.name.to_string();
                let tool = name.clone();
                let mut op = Operation::new(
                    name,
                    t.description.clone().unwrap_or_default().to_string(),
                    Arc::new(move |args: Map<String, Value>| {
                        let tool = tool.clone();
                        Box::pin(async move {
                            Ok(Prepared {
                                target: Target::App(AppTarget {
                                    app: NAME.to_string(),
                                    tool: tool.clone(),
                                    fields: Map::new(),
                                    title: "Ghost".to_string(),
                                }),
                                changes: ChangeSet::one(Change::App(AppChange {
                                    tool,
                                    arguments: args,
                                })),
                                rationale: String::new(),
                                asked_by: Vec::new(),
                            })
                        })
                    }),
                    writer.clone(),
                );
                op.properties = t
                    .input_schema
                    .get("properties")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                op
            })
            .collect()
    }

    /// Put those operations in the gate, where the operator's `ghost.*` rules and the
    /// audit already are. Called again whenever the gate's `ghost` operations were
    /// replaced (an app connected or went away), so a change that was waiting for a yes
    /// still runs.
    pub fn register_operations(&self) {
        let Some(outside) = &self.outside else {
            return;
        };
        for op in self.outside_operations() {
            if outside.gate.operation(NAME, &op.name).is_err() {
                outside.gate.add(NAME, op);
            }
        }
    }

    /// Which tools a client is shown: not one an unconditional `block` names.
    pub(super) fn offered(&self) -> Vec<Tool> {
        let all = self.tool_router.list_all();
        match &self.outside {
            None => all,
            Some(o) => all
                .into_iter()
                .filter(|t| o.gate.offers(NAME, &t.name, is_read(t)))
                .collect(),
        }
    }

    /// One tool call. In process it goes straight to the tool: the session's dispatcher
    /// and the gate were before it. From outside, see the module's words.
    pub(super) async fn dispatch(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(outside) = &self.outside else {
            return self
                .tool_router
                .call(ToolCallContext::new(self, request, context))
                .await;
        };
        let client = client_of(&context)?;
        let name = request.name.to_string();
        let Some(tool) = self.tool_router.get(&name).cloned() else {
            return Err(ErrorData::invalid_params("tool not found", None));
        };
        // A change sent again with the key it was first sent with gets the first one's
        // result, and is not made again (`say` is not said twice). A read is safe to
        // repeat and is not kept.
        let first = match idempotency_key(&request, &context).filter(|_| !is_read(&tool)) {
            None => None,
            Some(key) => match outside.calls.begin(&client, &key) {
                Begun::Done(out) => return Ok(reply(out).into()),
                Begun::Again(mut rx) => {
                    debug!("outside: {client} sent {NAME}.{name} again; the first call answers");
                    let out = tokio::select! {
                        got = rx.wait_for(Option::is_some) => match got {
                            Ok(v) => v.clone().unwrap_or_else(|| Err("the first call did not finish".into())),
                            Err(_) => Err("the first call did not finish".into()),
                        },
                        () = context.ct.cancelled() => Err("cancelled".into()),
                    };
                    return Ok(reply(out).into());
                }
                Begun::First(first) => Some(first),
            },
        };
        if let Err(e) = outside
            .limiter
            .take(&client, TELLING.contains(&name.as_str()))
        {
            let out: Outcome = Err(e);
            if let Some(first) = first {
                outside.calls.finish(first, &out);
            }
            return Ok(reply(out).into());
        }
        let args = request.arguments.clone().unwrap_or_default();
        if is_read(&tool) {
            if outside.gate.blocks_read(NAME, &name, &args) {
                return Ok(reply(Err(blocked(&name))).into());
            }
            debug!("outside: {client} read {NAME}.{name}");
            let named = self.for_caller(Caller::Named(client_source(&client)));
            return named
                .tool_router
                .call(ToolCallContext::new(&named, request, context))
                .await;
        }
        // A change runs to its outcome whatever becomes of the request: the gate is not
        // left with a proposal half made when the client goes away. What may take long
        // (`agent_ask`, a call to an app) stops on this token instead, when the client
        // cancels the request or drops the connection.
        let stop = CancellationToken::new();
        let gone = stop.clone().drop_guard();
        let work = {
            let (me, outside) = (self.clone(), outside.clone());
            tokio::spawn(crate::apps::with_cancel(stop.clone(), async move {
                let out = me.change(&outside, &client, &name, args).await;
                // Recorded by the task, so a resend gets it even when this request is gone.
                if let Some(first) = first {
                    outside.calls.finish(first, &out);
                }
                out
            }))
        };
        tokio::pin!(work);
        let gone_away = request_gone(&context).unwrap_or_default();
        let out = tokio::select! {
            out = &mut work => out,
            () = context.ct.cancelled() => {
                stop.cancel();
                work.await
            }
            () = gone_away.cancelled() => {
                debug!("outside: a change's connection closed; stopping it");
                stop.cancel();
                work.await
            }
        };
        let _ = gone.disarm();
        Ok(reply(out.unwrap_or_else(|e| Err(format!("the call did not finish: {e}")))).into())
    }

    /// A change from an outside client, through the gate.
    async fn change(
        &self,
        outside: &Outside,
        client: &str,
        tool: &str,
        mut args: Map<String, Value>,
    ) -> Result<Value, String> {
        let who = client_source(client);
        self.register_operations();
        let op = outside
            .gate
            .operation(NAME, tool)
            .map_err(|_| "Ghost's own app is not running; try again in a moment".to_string())?;
        // The caller is who the listener authenticated, over anything the client wrote.
        args.retain(|k, _| !k.starts_with('_'));
        args.insert(CALLER_ARG.into(), json!(who));
        let prepared = (op.prepare)(args).await.map_err(|e| e.to_string())?;
        let req = Request::new(
            &who,
            NAME,
            tool,
            prepared.target,
            prepared.changes,
            &format!("{}, an outside client", shown(client)),
        );
        // `asked_by` stays empty: nothing a client sends is ever the operator's yes.
        let sub = outside.gate.submit(req).await.map_err(|e| e.to_string())?;
        let Some(outcome) = sub.outcome else {
            // `ask`: it waits on the waiting list, not on this client.
            let _ = outside.gate.set_aside(&who, "waiting for the operator");
            let _ = self.parts.waiting.refresh(None);
            return Ok(json!({
                "status": "waiting for the operator",
                "id": sub.proposal.id,
                "key": waiting::change_key(&sub.proposal.id),
                "note": "The operator's permissions ask before this runs. It has not run; it is \
                         on their waiting list, and events_wait says when it is answered.",
            }));
        };
        match outcome.status {
            OutcomeStatus::Success => Ok(match outcome.readback {
                Some(Snapshot::App(AppSnapshot {
                    result: Some(answer),
                    ..
                })) => Value::Object(answer),
                _ => json!({"status": "done"}),
            }),
            _ => Err(outcome
                .error
                .unwrap_or_else(|| "nothing changed".to_string())),
        }
    }

    pub(super) fn info(&self) -> ServerConfig {
        let about = Implementation::new(NAME, env!("CARGO_PKG_VERSION"));
        if self.outside.is_some() {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_resources()
                    .build(),
            )
            .with_server_info(about)
            .with_instructions(INSTRUCTIONS)
        } else {
            // In process the resources are not advertised: Ghost offers an app's resources
            // to its agents as two more read tools (ADR 0046), and the agents already have
            // `docs_read` and `status`. Two tools on every turn for nothing new (ADR 0018).
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(about)
        }
    }

    pub(super) fn tool_list(&self, context: &RequestContext<RoleServer>) -> ListToolsResult {
        let hints = context
            .protocol_version()
            .is_some_and(|v| v >= rmcp::model::ProtocolVersion::V_2026_07_28);
        ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: self.offered(),
            meta: None,
            next_cursor: None,
            ttl_ms: hints.then_some(0),
            cache_scope: hints.then_some(rmcp::model::CacheScope::Public),
        }
    }

    /// A read that is not a tool call (a resource): the same client, budget and `block`
    /// rules as the tool it stands for.
    fn resource_read(
        &self,
        context: &RequestContext<RoleServer>,
        tool: &str,
        args: &Map<String, Value>,
    ) -> Result<(), ErrorData> {
        let Some(outside) = &self.outside else {
            return Ok(());
        };
        let client = client_of(context)?;
        outside
            .limiter
            .take(&client, false)
            .map_err(|e| ErrorData::invalid_request(e, None))?;
        if outside.gate.blocks_read(NAME, tool, args) {
            return Err(ErrorData::invalid_request(blocked(tool), None));
        }
        debug!("outside: {client} read a resource of {NAME}.{tool}");
        Ok(())
    }

    /// The documentation and the status, as resources: `ghost://docs/<name>` and
    /// `ghost://status`.
    pub(super) fn resource_list(&self) -> ListResourcesResult {
        let offers = |tool: &str| {
            self.outside
                .as_ref()
                .is_none_or(|o| o.gate.offers(NAME, tool, true))
        };
        let mut resources = Vec::new();
        if offers("status") {
            resources.push(
                Resource::new(STATUS_URI, "status")
                    .with_title("Ghost's status")
                    .with_description(
                        "How Ghost is now: version, phase, the session and its agent, the apps, \
                         how much is waiting.",
                    )
                    .with_mime_type("application/json"),
            );
        }
        if offers("docs_read") {
            for name in self.docs.documents() {
                resources.push(
                    Resource::new(format!("{DOCS_URI}{name}"), name)
                        .with_title(format!("Ghost's documentation: {name}"))
                        .with_mime_type("text/markdown"),
                );
            }
        }
        ListResourcesResult {
            result_type: Some(ResultType::COMPLETE),
            resources,
            meta: None,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
        }
    }

    pub(super) fn resource(
        &self,
        request: &ReadResourceRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let uri = request.uri.as_str();
        let (text, mime) = if uri == STATUS_URI {
            self.resource_read(context, "status", &Map::new())?;
            let status = self
                .do_status()
                .map_err(|e| ErrorData::internal_error(e, None))?;
            (status.to_string(), "application/json")
        } else if let Some(name) = uri.strip_prefix(DOCS_URI) {
            let args = Map::from_iter([("name".to_string(), json!(name))]);
            self.resource_read(context, "docs_read", &args)?;
            let text = super::docs::whole(name).ok_or_else(|| {
                ErrorData::resource_not_found(format!("no document named {name}"), None)
            })?;
            (text.to_string(), "text/markdown")
        } else {
            return Err(ErrorData::resource_not_found(
                format!("no resource {uri}"),
                None,
            ));
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some(mime.to_string()),
                text,
                meta: None,
            },
        ]))
    }
}

fn is_read(tool: &Tool) -> bool {
    tool.annotations
        .as_ref()
        .and_then(|a| a.read_only_hint)
        .unwrap_or(false)
}

fn blocked(tool: &str) -> String {
    format!("{NAME}.{tool} is blocked by the operator's permissions")
}

/// The client the listener authenticated this request as. A request that did not come
/// through the listener has none, and is refused.
fn client_of(context: &RequestContext<RoleServer>) -> Result<String, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<ClientId>())
        .map(|c| c.0.clone())
        .filter(|name| !name.is_empty() && !name.starts_with(CLIENT_SOURCE))
        .ok_or_else(|| ErrorData::invalid_request("this request names no client", None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::waiting::{ItemDraft, Items};

    fn limiter(calls: usize, notifies: usize, window: Duration) -> Limiter {
        Limiter {
            limits: Limits {
                calls,
                notifies,
                window,
            },
            hits: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn each_client_has_its_own_budget_and_telling_a_smaller_one() {
        let l = limiter(3, 1, Duration::from_millis(50));
        assert!(l.take("a", true).is_ok());
        let e = l.take("a", true).unwrap_err();
        assert!(e.contains("1 a minute"), "{e}");
        assert!(l.take("a", false).is_ok());
        assert!(l.take("a", false).is_ok());
        let e = l.take("a", false).unwrap_err();
        assert!(e.contains("3 a minute") && e.contains("Try again"), "{e}");
        // Another client is not slowed by this one.
        assert!(l.take("b", true).is_ok());
        std::thread::sleep(Duration::from_millis(60));
        assert!(l.take("a", true).is_ok());
    }

    #[tokio::test]
    async fn waiting_returns_at_once_on_an_event_and_empty_at_the_timeout() {
        let log = Arc::new(EventLog::default());
        let began = Instant::now();
        let (events, cursor) = log.wait(None, Duration::from_millis(40)).await;
        assert!(events.is_empty());
        assert_eq!(cursor, 0);
        assert!(began.elapsed() >= Duration::from_millis(40));

        let waiter = tokio::spawn({
            let log = log.clone();
            async move { log.wait(Some(0), Duration::from_secs(5)).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        log.push(json!({"type": "session", "state": "started"}));
        let (events, cursor) = waiter.await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(cursor, 1);
        // What came before the cursor is not said again; what came after is, at once.
        log.push(json!({"type": "session", "state": "ended"}));
        let (events, cursor) = log.wait(Some(1), Duration::from_secs(5)).await;
        assert_eq!(events[0].1["state"], "ended");
        assert_eq!(cursor, 2);
        // A cursor from before a restart reads from the start.
        assert_eq!(log.since(99).len(), 2);
    }

    #[test]
    fn the_ring_keeps_only_the_newest() {
        let log = EventLog::default();
        for n in 0..(EVENTS_KEPT + 10) {
            log.push(json!({"n": n}));
        }
        let all = log.since(0);
        assert_eq!(all.len(), EVENTS_KEPT);
        assert_eq!(all[0].0, 11);
        assert_eq!(log.cursor(), (EVENTS_KEPT + 10) as u64);
    }

    #[test]
    fn the_feed_says_how_the_waiting_list_changed_and_drops_what_is_not_for_outside() {
        let store = Arc::new(Store::in_memory().unwrap());
        let waiting = Arc::new(WaitingList::new(Items::new(store)));
        waiting
            .observe("watch:ci", &ItemDraft::new("ci:1", "CI failed"))
            .unwrap();
        let log = Arc::new(EventLog::default());
        let feed = Feed::new(log.clone(), waiting.clone());
        feed.on_bus(&json!({"type": "waiting", "count": 1}));
        assert!(
            log.since(0).is_empty(),
            "what was there at the start is not news"
        );

        waiting
            .observe(
                "client:cursor",
                &ItemDraft::new("client:cursor:abc", "From Cursor: done"),
            )
            .unwrap();
        feed.on_bus(&json!({"type": "waiting", "count": 2}));
        waiting
            .acknowledge("client:cursor:abc", None, None)
            .unwrap();
        feed.on_bus(&json!({"type": "waiting", "count": 1}));
        waiting
            .resolve("client:cursor", "client:cursor:abc")
            .unwrap();
        feed.on_bus(&json!({"type": "waiting", "count": 1}));
        let said: Vec<String> = log
            .since(0)
            .iter()
            .map(|(_, e)| {
                format!(
                    "{} {}",
                    e["change"].as_str().unwrap(),
                    e["key"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                "added client:cursor:abc",
                "acknowledged client:cursor:abc",
                "resolved client:cursor:abc"
            ]
        );

        // A session starting and ending, once each; never a turn, a log line or audio.
        let before = log.cursor();
        let status = |phase: &str, session: Value| {
            json!({"type": "status", "status": {"phase": phase, "session_id": session,
                   "session_origin": "desk", "conversation": "ghost"}})
        };
        feed.on_bus(&status("listening", json!("s1")));
        feed.on_bus(&status("thinking", json!("s1")));
        feed.on_bus(&status("passive", Value::Null));
        feed.on_bus(
            &json!({"type": "turn", "turn": {"role": "user", "text": "a secret thing said"}}),
        );
        feed.on_bus(&json!({"type": "log", "level": "info", "message": "x"}));
        feed.on_bus(&json!({"type": "voice", "level": 0.4}));
        feed.on_bus(&json!({"type": "apps", "sign_in_url": "https://example.com/oauth"}));
        let after = log.since(before);
        assert_eq!(after.len(), 2);
        // A change's outcome, with the session it was proposed in when the store has it.
        feed.on_bus(&json!({"type": "outcome", "via": "ui", "outcome": {
            "proposal_id": "p1", "status": "success", "target": "acme/app#7", "summary": "Applied."}}));
        let last = log.since(log.cursor() - 1);
        assert_eq!(last[0].1["type"], "change");
        assert_eq!(last[0].1["source"], "");
        assert_eq!(after[0].1["state"], "started");
        assert_eq!(after[1].1["state"], "ended");
        let all = serde_json::to_string(&log.since(0)).unwrap();
        assert!(!all.contains("a secret thing said") && !all.contains("oauth"));
    }
}
