//! Ghost is an app (ADR 0045): Ghost's own functions as one MCP server, served to its own
//! agents in process.
//!
//! The daemon makes one [`GhostServer`] over its shared state and [`attach`]es it: a
//! `tokio::io::duplex` pair, the server on one end and the ordinary app client on the
//! other, joined to [`Apps`] under the name `ghost`. From there it is an app like any
//! other: its tools are offered as `ghost_<tool>`, a read-only one is a read, every other
//! one is a change through the gate, and the operator's `ghost.*` rules apply. Two things
//! differ, because the text is Ghost's own and not an outside system's: descriptions reach
//! the model whole, and results are not trimmed (the tools cap themselves).
//!
//! The handler is `Clone` over one `Arc` of state and keeps nothing per connection, so the
//! same one is served to outside clients over HTTP (ADR 0048: `clients`, `outside`,
//! `outside_tools`). Nothing here opens a listener; the daemon's does.

pub mod agents;
pub mod apps_tools;
pub mod clients;
pub mod docs;
pub mod more;
pub mod outputs_tools;
pub mod outside;
pub mod outside_tools;
pub mod questions;
pub mod recall;
pub mod schedules;
pub mod settings;
#[cfg(test)]
mod tests;
pub mod tools_search;
pub mod watches;
#[cfg(test)]
mod wide_tests;

pub use more::{CORE, Late, More, NOT_FOR_OUTSIDE, asks_by_default, ready};

use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::apps::{App, AppError, Apps, GHOST_APP, RmcpSession, init_kind};
use crate::routes::Urgency;
use crate::store::{Item, ProposalQuery, Store};
use crate::time::{iso, parse_iso, utcnow};
use crate::waiting::{self, ItemDraft, WaitingList};

use docs::{Docs, Read, SEARCH_RESULTS};

/// The app's name: the prefix of its tools, and the source its rules and audit rows name.
pub const NAME: &str = GHOST_APP;
/// The source of an agent's own items on the waiting list: `agent:<slug>`. An outside
/// client's are under `client:<name>` (`clients::CLIENT_SOURCE`, ADR 0048).
pub const AGENT_SOURCE: &str = "agent:";
pub const DETAIL_MAX: usize = 500;
pub const SUMMARY_MAX: usize = 240;
pub const AUDIT_DEFAULT: i64 = 10;
pub const AUDIT_MAX: i64 = 50;
pub const SNOOZE_MAX_MINUTES: i64 = 7 * 24 * 60;
/// How long before a server that stopped is started again, when the start itself fails.
const RESTART_PAUSE: Duration = Duration::from_secs(1);

/// What the model is told when it has these tools: the app's paragraph of the system
/// prompt, in place of the one a connected app gets.
pub const PROMPT: &str = "Ghost is an app too, and you have its tools. For a question about what Ghost is or how it \
works (apps, permissions, agents, skills, the waiting list, routes, watches, the phone), look it \
up with ghost_docs_search and answer from that, not from memory. Most of Ghost's own tools \
(watches, apps, quiet, permissions, outputs, skills, your memory, asking another agent) are not \
listed until you find them with ghost_tools_search; check there and the docs before saying Ghost \
can't do something. To \
tell the operator something later, or when they are away, use ghost_notify, and ghost_resolve \
once the matter is done. When the operator is here, just ask them aloud; to ask something they \
may answer later, use ghost_ask. When they answer a question Ghost put to them, record it with \
ghost_answer in their exact words.";

/// What the daemon alone knows for `status`: the version and commit, the phase, the
/// session and its origin, the agent, the model connection's provider and model (never a
/// key). Built from the control API's own `Status`.
pub type StatusFn = Arc<dyn Fn() -> Map<String, Value> + Send + Sync>;

/// The state the tools read and write: all of it shared, none of it per connection.
pub struct Parts {
    pub store: Arc<Store>,
    pub waiting: Arc<WaitingList>,
    /// Weak: the apps hold this server's client.
    pub apps: Weak<Apps>,
    pub status: StatusFn,
}

/// Who is calling. In process the session's dispatcher names the agent in `_agent`
/// (`crate::apps::CALLER_ARG`), over anything the model wrote. A listener for outside
/// clients uses `Named`, from what it authenticated, so an argument is never believed: an
/// agent's slug, or `client:<name>` for an outside client (ADR 0048).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Caller {
    #[default]
    Session,
    Named(String),
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocsSearch {
    /// A few words about what to find.
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DocsRead {
    /// readme, glossary, architecture, apps, connect-ghost, tools, skills, folder,
    /// performance or phone-protocol.
    pub name: String,
    /// A heading of that document; leave out for its list of headings.
    #[serde(default)]
    pub section: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Notify {
    /// What the operator should know, in one line.
    pub title: String,
    /// A sentence or two more, if needed.
    #[serde(default)]
    pub detail: Option<String>,
    /// digest, notify (the default) or call.
    #[serde(default)]
    pub urgency: Option<String>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Resolve {
    /// The key ghost_notify returned.
    pub key: String,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitingAck {
    /// Keys from waiting_list.
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitingSnooze {
    /// Keys from waiting_list.
    pub keys: Vec<String>,
    /// How long to stay quiet about them.
    pub minutes: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AuditRecent {
    /// How many, newest first (10 unless given, 50 at most).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Only changes since this UTC date or time (2026-11-05, or 2026-11-05T14:00:00Z).
    #[serde(default)]
    pub since: Option<String>,
}

/// The source an agent's items are kept under.
pub fn agent_source(slug: &str) -> String {
    format!("{AGENT_SOURCE}{slug}")
}

/// The key of an item an agent put on the waiting list: the agent and what the title
/// says, so the same thing said again is the same item. (Not a *note*: an agent's notes
/// are its `notes.md`.)
pub fn item_key(slug: &str, title: &str) -> String {
    source_key(&agent_source(slug), title)
}

/// The same for any source that tells the operator things: `agent:<slug>` or
/// `client:<name>`.
pub fn source_key(source: &str, title: &str) -> String {
    let said = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let digest = hex::encode(Sha256::digest(said.as_bytes()));
    format!("{source}:{}", &digest[..10])
}

/// How loud an agent's item may get (ADR 0031): what the agent asked for with it. An
/// outside client's the same. None for an item that is neither's.
pub fn urgency_of(item: &Item) -> Option<Urgency> {
    if !item.source.starts_with(AGENT_SOURCE) && !item.source.starts_with(clients::CLIENT_SOURCE) {
        return None;
    }
    Some(
        item.fields
            .get("urgency")
            .and_then(Value::as_str)
            .and_then(Urgency::parse)
            .unwrap_or(Urgency::Notify),
    )
}

fn clip(text: &str, max: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        t
    } else {
        let mut cut: String = t.chars().take(max - 1).collect();
        cut.push('…');
        cut
    }
}

/// A date (`2026-11-05`, its first moment) or a time, UTC.
fn parse_when(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    parse_iso(s).or_else(|| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|n| n.and_utc())
    })
}

fn reply(result: Result<Value, String>) -> CallToolResult {
    match result {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
    }
}

/// Ghost's own functions as an MCP server.
#[derive(Clone)]
pub struct GhostServer {
    parts: Arc<Parts>,
    docs: &'static Docs,
    caller: Caller,
    /// The schedules and their two tools (ADR 0047), when the daemon gives them.
    schedules: Option<Arc<crate::schedules::Jobs>>,
    /// The questions and their tools (ADR 0051), when the daemon gives them.
    questions: Option<Arc<crate::questions::Questions>>,
    tool_router: ToolRouter<Self>,
    /// What only a server for outside clients has (ADR 0048).
    outside: Option<Arc<outside::Outside>>,
    /// What the wider tools reach (ADR 0052), each when the daemon gives it.
    more: Arc<More>,
    /// How many `ask_agent` turns each session has running (ADR 0052: three at most).
    asking: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

#[tool_router]
impl GhostServer {
    pub fn new(parts: Parts) -> Self {
        Self {
            parts: Arc::new(parts),
            docs: Docs::built_in(),
            caller: Caller::Session,
            schedules: None,
            questions: None,
            tool_router: Self::tool_router() + Self::tools_search_router() + Self::recall_router(),
            outside: None,
            more: Arc::new(More::default()),
            asking: Arc::default(),
        }
    }

    /// The same server for one authenticated caller (an outside client, later).
    pub fn for_caller(&self, caller: Caller) -> Self {
        Self {
            caller,
            ..self.clone()
        }
    }

    /// The tools this server offers, as the client lists them.
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    /// The source a call's items are kept under, `agent:<slug>` or `client:<name>`: never
    /// what a caller from outside wrote as an argument.
    fn agent(&self, said: Option<String>) -> Result<String, String> {
        let who = match &self.caller {
            Caller::Named(who) => who.clone(),
            Caller::Session => said.unwrap_or_default(),
        };
        let (kind, slug) = match who.strip_prefix(clients::CLIENT_SOURCE) {
            Some(name) => (clients::CLIENT_SOURCE, name),
            None => (AGENT_SOURCE, who.as_str()),
        };
        let ok = !slug.is_empty()
            && slug.len() <= 80
            && slug
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if ok {
            Ok(format!("{kind}{slug}"))
        } else {
            Err("this needs to know which agent is calling, and nothing said".into())
        }
    }

    fn apps_view(&self) -> Vec<Value> {
        let Some(apps) = self.parts.apps.upgrade() else {
            return Vec::new();
        };
        apps.specs()
            .into_iter()
            .map(|spec| {
                let live = apps.app(&spec.name).filter(|a| a.connected());
                let mut row = Map::new();
                row.insert("name".into(), json!(spec.name));
                row.insert("title".into(), json!(spec.shown()));
                row.insert("state".into(), json!(apps.state(&spec.name)));
                row.insert(
                    "reads".into(),
                    json!(live.as_ref().map_or(0, |a| a.reads().len())),
                );
                row.insert(
                    "changes".into(),
                    json!(live.as_ref().map_or(0, |a| a.changes().len())),
                );
                if let Some(problem) = apps.problem(&spec.name) {
                    row.insert("problem".into(), json!(problem));
                }
                Value::Object(row)
            })
            .collect()
    }

    fn do_docs_search(&self, p: &DocsSearch) -> Result<Value, String> {
        let hits = self.docs.search(&p.query, SEARCH_RESULTS);
        if hits.is_empty() {
            return Ok(json!({
                "results": [],
                "documents": self.docs.documents(),
                "note": "Nothing matched. Try other words, or read a document's headings with ghost_docs_read.",
            }));
        }
        let results: Vec<Value> = hits
            .into_iter()
            .map(|h| json!({"document": h.document, "heading": h.heading, "excerpt": h.excerpt}))
            .collect();
        Ok(json!({
            "results": results,
            "note": "Read a whole section with ghost_docs_read (name, section).",
        }))
    }

    fn do_docs_read(&self, p: &DocsRead) -> Result<Value, String> {
        match self
            .docs
            .read(&p.name, p.section.as_deref())
            .map_err(|e| e.to_string())?
        {
            Read::Outline { document, headings } => {
                Ok(json!({"document": document, "sections": headings}))
            }
            Read::Section {
                document,
                heading,
                text,
                cut,
            } => {
                let mut out = json!({"document": document, "section": heading, "text": text});
                if cut {
                    out["cut"] = json!(true);
                    out["note"] = json!(format!(
                        "This is the first {} characters of the section; the rest was cut. \
                         Search for what is missing.",
                        docs::SECTION_MAX
                    ));
                }
                Ok(out)
            }
        }
    }

    fn do_status(&self) -> Result<Value, String> {
        let mut out = (self.parts.status)();
        let now = utcnow();
        let items = self
            .parts
            .waiting
            .refresh(Some(now))
            .map_err(|e| e.to_string())?;
        let apps: Vec<Value> = self
            .apps_view()
            .into_iter()
            .map(|a| json!({"name": a["name"], "title": a["title"], "state": a["state"]}))
            .collect();
        out.insert("apps".into(), Value::Array(apps));
        out.insert(
            "waiting".into(),
            json!(items.iter().filter(|i| i.needs_alerting(Some(now))).count()),
        );
        // What Ghost itself reports: the waiting list's own source (an app not connected,
        // a locked keyring, a file in Ghost's folder that doesn't read).
        let problems: Vec<&str> = items
            .iter()
            .filter(|i| i.source == waiting::GHOST)
            .map(|i| i.title.as_str())
            .collect();
        out.insert("problems".into(), json!(problems));
        Ok(Value::Object(out))
    }

    fn do_notify(&self, p: Notify) -> Result<Value, String> {
        let source = self.agent(p.agent)?;
        let title = clip(&p.title, waiting::TITLE_MAX);
        if title.is_empty() {
            return Err("an item needs a title".into());
        }
        let urgency = match p
            .urgency
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            None => Urgency::Notify,
            Some(u) => Urgency::parse(&u.to_lowercase())
                .ok_or_else(|| format!("urgency is digest, notify or call, not {u}"))?,
        };
        let mut fields = Map::new();
        fields.insert("urgency".into(), json!(urgency.as_str()));
        if let Some(detail) = p.detail.as_deref().map(|d| clip(d, DETAIL_MAX))
            && !detail.is_empty()
        {
            fields.insert("detail".into(), json!(detail));
        }
        let key = source_key(&source, &title);
        // An outside client's item says who it is from, on every route (ADR 0048).
        let title = match source.strip_prefix(clients::CLIENT_SOURCE) {
            Some(name) => clip(
                &format!("From {}: {title}", clients::shown(name)),
                waiting::TITLE_MAX,
            ),
            None => title,
        };
        let changed = self
            .parts
            .waiting
            .observe(&source, &ItemDraft::new(key.clone(), title).fields(fields))
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "key": key,
            "status": if changed { "on the waiting list" } else { "already on the waiting list" },
        }))
    }

    fn do_resolve(&self, p: Resolve) -> Result<Value, String> {
        let source = self.agent(p.agent)?;
        let done = self
            .parts
            .waiting
            .resolve(&source, p.key.trim())
            .map_err(|e| e.to_string())?;
        if done {
            Ok(json!({"key": p.key.trim(), "status": "resolved"}))
        } else {
            Err(format!(
                "{} is not an open item of yours; an agent resolves only what it put there",
                p.key.trim()
            ))
        }
    }

    fn marked(
        &self,
        keys: &[String],
        mark: impl Fn(&str) -> waiting::Result<bool>,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        if keys.is_empty() {
            return Err("name at least one key from waiting_list".into());
        }
        let (mut done, mut missing) = (Vec::new(), Vec::new());
        for key in keys {
            if mark(key.trim()).map_err(|e| e.to_string())? {
                done.push(key.trim().to_string());
            } else {
                missing.push(key.trim().to_string());
            }
        }
        Ok((done, missing))
    }

    fn do_waiting_ack(&self, p: &WaitingAck) -> Result<Value, String> {
        let (done, missing) =
            self.marked(&p.keys, |k| self.parts.waiting.acknowledge(k, None, None))?;
        Ok(json!({"acknowledged": done, "not_found": missing}))
    }

    fn do_waiting_snooze(&self, p: &WaitingSnooze) -> Result<Value, String> {
        if !(1..=SNOOZE_MAX_MINUTES).contains(&p.minutes) {
            return Err(format!("minutes is 1 to {SNOOZE_MAX_MINUTES}"));
        }
        let until = utcnow() + chrono::Duration::minutes(p.minutes);
        let (done, missing) = self.marked(&p.keys, |k| self.parts.waiting.snooze(k, until))?;
        Ok(json!({"snoozed": done, "until": iso(&until), "not_found": missing}))
    }

    fn do_audit_recent(&self, p: &AuditRecent) -> Result<Value, String> {
        let limit = p.limit.unwrap_or(AUDIT_DEFAULT).clamp(1, AUDIT_MAX);
        let since = match p.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(parse_when(s).ok_or_else(|| {
                format!("since is a UTC date or time (2026-11-05 or 2026-11-05T14:00:00Z), not {s}")
            })?),
        };
        // What backs GET /audit: the proposals, newest first.
        let (rows, total) = self
            .parts
            .store
            .list_proposals(&ProposalQuery::page(limit, 0))
            .map_err(|e| e.to_string())?;
        let changes: Vec<Value> = rows
            .iter()
            .filter(|p| since.is_none_or(|s| p.created_at >= s))
            .map(|p| {
                json!({
                    "when": iso(&p.created_at),
                    "agent": p.conversation,
                    "source": p.ability,
                    "operation": p.operation,
                    "target": p.target.spoken(),
                    "summary": clip(&p.preview, SUMMARY_MAX),
                    "level": p.autonomy,
                    "outcome": p.status.as_str(),
                })
            })
            .collect();
        Ok(json!({"changes": changes, "total_recorded": total, "now": iso(&utcnow())}))
    }

    #[tool(
        description = "Search Ghost's own documentation: what Ghost is and how it works. Returns the best sections with a short excerpt each.",
        annotations(read_only_hint = true)
    )]
    fn docs_search(&self, Parameters(p): Parameters<DocsSearch>) -> CallToolResult {
        reply(self.do_docs_search(&p))
    }

    #[tool(
        description = "Read one section of Ghost's documentation, or a document's headings when no section is named.",
        annotations(read_only_hint = true)
    )]
    fn docs_read(&self, Parameters(p): Parameters<DocsRead>) -> CallToolResult {
        reply(self.do_docs_read(&p))
    }

    #[tool(
        description = "How Ghost is now: version, phase, the session and its agent, the connected apps, how much is waiting, the model connection, and any problem Ghost reports.",
        annotations(read_only_hint = true)
    )]
    fn status(&self) -> CallToolResult {
        reply(self.do_status())
    }

    #[tool(
        description = "Put an item for the operator on the waiting list, to reach them later or when they are away. The same title again is the same item. Returns its key."
    )]
    fn notify(&self, Parameters(p): Parameters<Notify>) -> CallToolResult {
        reply(self.do_notify(p))
    }

    #[tool(description = "Clear an item you put on the waiting list, once the matter is done.")]
    fn resolve(&self, Parameters(p): Parameters<Resolve>) -> CallToolResult {
        reply(self.do_resolve(p))
    }

    #[tool(
        description = "Mark waiting-list items as heard by the operator, so they stop alerting until they change."
    )]
    fn waiting_ack(&self, Parameters(p): Parameters<WaitingAck>) -> CallToolResult {
        reply(self.do_waiting_ack(&p))
    }

    #[tool(description = "Stay quiet about waiting-list items for some minutes.")]
    fn waiting_snooze(&self, Parameters(p): Parameters<WaitingSnooze>) -> CallToolResult {
        reply(self.do_waiting_snooze(&p))
    }

    #[tool(
        description = "Recent changes from the audit: what was proposed, by which agent, when, and how it ended. For 'what did you do yesterday?'.",
        annotations(read_only_hint = true)
    )]
    fn audit_recent(&self, Parameters(p): Parameters<AuditRecent>) -> CallToolResult {
        reply(self.do_audit_recent(&p))
    }

    #[tool(
        description = "The apps connected to Ghost: each one's state and how many reads and changes it offers.",
        annotations(read_only_hint = true)
    )]
    fn apps_list(&self) -> CallToolResult {
        reply(Ok(json!({"apps": self.apps_view()})))
    }
}

/// Everything past the tools themselves is in `outside`: who is calling, the gate for a
/// call that had no session, and the resources.
#[tool_handler(router = self.tool_router, name = "ghost")]
impl ServerHandler for GhostServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.info()
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        self.dispatch(request, context).await
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        Ok(self.tool_list(&context))
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, rmcp::ErrorData> {
        Ok(self.resource_list())
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, rmcp::ErrorData> {
        Ok(self.resource(&request, &context)?.into())
    }
}

/// The running app. Dropping it stops the task that keeps it served.
pub struct Attached {
    pub app: Arc<App>,
    keeper: tokio::task::JoinHandle<()>,
}

impl Attached {
    /// Stop serving: before the apps close, so the server is not started again.
    pub fn stop(&self) {
        self.keeper.abort();
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.keeper.abort();
    }
}

/// One pipe: the server on one end, the app's client on the other.
async fn serve(app: &App, server: GhostServer) -> Result<tokio::task::JoinHandle<()>, AppError> {
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let serving = tokio::spawn(async move {
        match server.serve(server_io).await {
            Ok(running) => {
                let _ = running.waiting().await;
            }
            Err(e) => warn!("app {NAME}: its server did not start: {e}"),
        }
    });
    let running = ().serve(client_io).await.map_err(|e| AppError::Connect(init_kind(&e)))?;
    app.attach(Arc::new(RmcpSession::new(running))).await?;
    Ok(serving)
}

/// Start Ghost's own app and join it to `apps` as `ghost`. It needs no vault, no network
/// and no sign-in; it is not in `apps.json`, so it is never an app that "isn't connected"
/// on the waiting list. A server that stops is started again on the same `App`, so the
/// gate's operations keep working.
pub async fn attach(apps: &Arc<Apps>, server: GhostServer) -> Result<Attached, AppError> {
    let app = Arc::new(App::own(NAME, "Ghost"));
    let mut serving = serve(&app, server.clone()).await?;
    apps.attach(app.clone());
    info!(
        "app {NAME}: {} tools ({} reads, {} changes), in process",
        app.tools().len(),
        app.reads().len(),
        app.changes().len()
    );
    let kept = app.clone();
    let keeper = tokio::spawn(async move {
        loop {
            let _ = (&mut serving).await;
            warn!("app {NAME}: its server stopped; starting it again");
            loop {
                match serve(&kept, server.clone()).await {
                    Ok(s) => {
                        serving = s;
                        break;
                    }
                    Err(e) => {
                        warn!("app {NAME}: not started: {}", e.reason());
                        tokio::time::sleep(RESTART_PAUSE).await;
                    }
                }
            }
        }
    });
    Ok(Attached { app, keeper })
}
