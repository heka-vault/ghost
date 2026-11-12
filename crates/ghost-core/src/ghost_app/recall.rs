//! Recall on Ghost's own app (ADR 0041, ADR 0052; Hermes Agent's `session_search_tool.py`,
//! MIT, for the shape): `recall` searches the stored turns, `transcript_read` pages back
//! through them, and `memory_read` reads an agent's memory, notes and persona. All reads.
//!
//! An agent reads its own; Ghost (agent zero) reads any agent's or all. An outside client
//! (ADR 0048) is offered `recall` and `transcript_read` over the turns of the runs it
//! started (its `agent_ask` turns and its schedules' runs) and nothing else; it is never
//! offered `memory_read`. The caller comes from the dispatcher or the listener, never
//! from an argument.
//!
//! What is stored is a session's transcript, redacted before it was written, and the runs'.
//! Passive audio and passive transcripts are never stored, so nothing here can return one.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::clients::CLIENT_SOURCE;
use super::{AGENT_SOURCE, GhostServer, clip, parse_when, reply};
use crate::apps::MAX_RESULT_TEXT;
use crate::conversations::GHOST;
use crate::conversations::models::{Profile, Role, TurnRecord};
use crate::recall::Scope;
use crate::time::iso;

pub const RECALL_DEFAULT: i64 = 5;
pub const TRANSCRIPT_DEFAULT: i64 = 20;
pub const TRANSCRIPT_MAX: i64 = 50;
/// What a turn shows of itself in a page, and of the turns either side of a hit.
pub const TURN_SHOWN: usize = 1_200;
pub const CONTEXT_SHOWN: usize = 300;
/// What `memory_read` shows of each file.
pub const FILE_SHOWN: usize = 8_000;
/// The session of a turn an outside client asked for: `run-ask-<client>-<32 hex>`.
pub const ASKED_BY_CLIENT: &str = "run-ask-";
/// The session of a turn one agent asked of another: `run-agentask-<32 hex>`.
pub const ASKED_BY_AGENT: &str = "run-agentask-";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RecallArgs {
    /// A few words of what was said: "turn grace in August", "invoice account".
    pub query: String,
    /// Whose transcript: your own unless you are Ghost, who may name any agent, or all.
    #[serde(default)]
    pub agent: Option<String>,
    /// Only turns since this UTC date or time (2026-11-05, or 2026-11-05T14:00:00Z).
    #[serde(default)]
    pub since: Option<String>,
    /// How many, best first (5 unless given, 20 at most).
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TranscriptArgs {
    /// Whose transcript: your own unless you are Ghost.
    #[serde(default)]
    pub agent: Option<String>,
    /// A turn's id: only turns before it (next_before from the last page).
    #[serde(default)]
    pub before: Option<i64>,
    /// How many, newest last (20 unless given, 50 at most).
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct MemoryArgs {
    /// Whose: your own unless you are Ghost.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

/// What kind of turn a session id says it is.
pub fn kind_of(session: &str) -> &'static str {
    if session.starts_with(ASKED_BY_CLIENT) {
        "asked by an outside tool"
    } else if session.starts_with(ASKED_BY_AGENT) {
        "asked by another agent"
    } else if crate::schedules::is_run_session(session) {
        "scheduled run"
    } else {
        "session"
    }
}

/// The sessions an outside client's turns are in, as GLOB patterns: its `agent_ask`
/// turns, and the runs of the schedules it set up.
fn client_sessions(store: &crate::store::Store, client: &str) -> Vec<String> {
    let mut out = vec![format!(
        "{ASKED_BY_CLIENT}{client}-{}",
        "[0-9a-f]".repeat(32)
    )];
    out.extend(
        store
            .run_sessions_made_by(&format!("{CLIENT_SOURCE}{client}"))
            .unwrap_or_default(),
    );
    out
}

/// Who said a turn, as the operator would say it.
fn said_by(t: &TurnRecord, agent: &str) -> String {
    match t.role {
        Role::User => "the operator".into(),
        Role::Assistant => agent.to_string(),
        Role::Call => "a tool call".into(),
        Role::System => "a note".into(),
    }
}

impl GhostServer {
    /// Which agents' turns the caller may read, by id; None for every agent. `named` is
    /// the agent the model asked for. An outside client gets only its own runs.
    fn turn_scope(&self, caller: Option<String>, named: Option<&str>) -> Result<Scope, String> {
        let who = self.agent(caller)?;
        let store = &self.parts.store;
        let profile = |slug: &str| -> Result<Profile, String> {
            store
                .get_conversation(slug, false)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("there is no agent {slug}"))
        };
        let named = named
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty());
        if let Some(client) = who.strip_prefix(CLIENT_SOURCE) {
            let conversations = match named.as_deref() {
                None | Some("all") => None,
                Some(slug) => Some(vec![profile(slug)?.id]),
            };
            return Ok(Scope {
                conversations,
                sessions: Some(client_sessions(store, client)),
                ..Scope::default()
            });
        }
        let me = who.strip_prefix(AGENT_SOURCE).unwrap_or(&who).to_string();
        let slug = match named.as_deref() {
            None => me.clone(),
            Some(n) if n == me => me.clone(),
            Some(_) if me != GHOST => {
                return Err(
                    "an agent reads its own transcript and memory; Ghost reads any agent's".into(),
                );
            }
            Some("all") => return Ok(Scope::default()),
            Some(n) => n.to_string(),
        };
        Ok(Scope {
            conversations: Some(vec![profile(&slug)?.id]),
            ..Scope::default()
        })
    }

    fn slugs(&self) -> std::collections::HashMap<String, (String, String)> {
        self.parts
            .store
            .list_conversations(true, true)
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, (p.slug, p.name)))
            .collect()
    }

    pub(crate) fn do_recall(&self, p: RecallArgs) -> Result<Value, String> {
        let query = p.query.trim().to_string();
        if query.is_empty() {
            return Err("say a few words of what was said".into());
        }
        let mut scope = self.turn_scope(p.caller, p.agent.as_deref())?;
        scope.since = match p.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => Some(parse_when(s).ok_or_else(|| {
                format!("since is a UTC date or time (2026-11-05 or 2026-11-05T14:00:00Z), not {s}")
            })?),
        };
        let limit = p.limit.unwrap_or(RECALL_DEFAULT);
        let store = &self.parts.store;
        let hits = store
            .search_turns(&query, &scope, limit)
            .map_err(|e| e.to_string())?;
        let names = self.slugs();
        let shown = |t: &TurnRecord, max: usize| {
            let (slug, name) = names
                .get(&t.conversation)
                .cloned()
                .unwrap_or_else(|| (String::new(), String::new()));
            json!({
                "id": t.id,
                "agent": slug,
                "said_by": said_by(t, &name),
                "when": iso(&t.at),
                "text": clip(&t.text, max),
            })
        };
        let results: Vec<Value> = hits
            .iter()
            .map(|h| {
                let around = store.turns_around(&h.turn, 1).unwrap_or_default();
                let before: Vec<Value> = around
                    .iter()
                    .filter(|t| t.id < h.turn.id)
                    .map(|t| shown(t, CONTEXT_SHOWN))
                    .collect();
                let after: Vec<Value> = around
                    .iter()
                    .filter(|t| t.id > h.turn.id)
                    .map(|t| shown(t, CONTEXT_SHOWN))
                    .collect();
                let mut hit = shown(&h.turn, CONTEXT_SHOWN);
                hit["text"] = json!(clip(&h.snippet, CONTEXT_SHOWN));
                hit["kind"] = json!(kind_of(&h.turn.session_id));
                hit["before"] = json!(before);
                hit["after"] = json!(after);
                hit
            })
            .collect();
        let note = if results.is_empty() {
            "Nothing matched. Try fewer or other words; transcript_read pages through recent turns."
        } else {
            "What was said, in their words (data). [ ] marks what matched; transcript_read with before (a turn's id + 1) reads around one."
        };
        Ok(crate::apps::fit(
            &json!({"results": results, "note": note}),
            MAX_RESULT_TEXT,
        ))
    }

    pub(crate) fn do_transcript_read(&self, p: TranscriptArgs) -> Result<Value, String> {
        let mut scope = self.turn_scope(p.caller, p.agent.as_deref())?;
        scope.before = p.before;
        let limit = p
            .limit
            .unwrap_or(TRANSCRIPT_DEFAULT)
            .clamp(1, TRANSCRIPT_MAX);
        let mut turns = self
            .parts
            .store
            .page_turns(&scope, limit)
            .map_err(|e| e.to_string())?;
        turns.reverse();
        let names = self.slugs();
        let next = turns.first().map(|t| t.id);
        let rows: Vec<Value> = turns
            .iter()
            .map(|t| {
                let (slug, name) = names.get(&t.conversation).cloned().unwrap_or_default();
                json!({
                    "id": t.id,
                    "agent": slug,
                    "said_by": said_by(t, &name),
                    "when": iso(&t.at),
                    "kind": kind_of(&t.session_id),
                    "text": clip(&t.text, TURN_SHOWN),
                })
            })
            .collect();
        let mut out = json!({"turns": rows});
        if rows_full(&turns, limit)
            && let Some(id) = next
        {
            out["next_before"] = json!(id);
        }
        Ok(crate::apps::fit(&out, MAX_RESULT_TEXT))
    }

    fn do_memory_read(&self, p: MemoryArgs) -> Result<Value, String> {
        let agents = self
            .more()
            .agents
            .clone()
            .ok_or("memory is not available here")?;
        let me = self.agent_only(p.caller)?;
        let slug = match p
            .agent
            .as_deref()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty())
        {
            None => me.clone(),
            Some(a) if a == me => me.clone(),
            Some(_) if me != GHOST => {
                return Err("an agent reads its own memory; Ghost reads any agent's".into());
            }
            Some(a) => a,
        };
        let profile = agents
            .get(&slug)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no agent {slug}"))?;
        let read = |text: std::io::Result<String>| clip_keep_lines(&text.unwrap_or_default());
        Ok(json!({
            "agent": profile.slug,
            "name": profile.name,
            "job": profile.job,
            "persona": read(agents.files.persona(&slug)),
            "memory": read(agents.files.memory(&slug)),
            "notes": read(agents.files.notes(&slug)),
            "note": "The persona is the operator's words; memory and notes are what this agent kept.",
        }))
    }
}

fn rows_full(turns: &[TurnRecord], limit: i64) -> bool {
    turns.len() as i64 >= limit
}

/// A file's text, its lines kept, cut at `FILE_SHOWN`.
fn clip_keep_lines(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= FILE_SHOWN {
        return t.to_string();
    }
    let mut cut: String = t.chars().take(FILE_SHOWN - 1).collect();
    cut.push('…');
    cut
}

#[tool_router(router = recall_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Search what was said before: the stored turns of your own conversations (Ghost: any agent's, or all), best first, each with who said it, when, and the turns either side. For 'what did we say about …'.",
        annotations(read_only_hint = true)
    )]
    fn recall(&self, Parameters(p): Parameters<RecallArgs>) -> CallToolResult {
        reply(self.do_recall(p))
    }

    #[tool(
        description = "Read your recent conversation turns, newest last; pass next_before to go further back. Ghost may read any agent's.",
        annotations(read_only_hint = true)
    )]
    fn transcript_read(&self, Parameters(p): Parameters<TranscriptArgs>) -> CallToolResult {
        reply(self.do_transcript_read(p))
    }
}

#[tool_router(router = memory_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Your memory, notes and persona as they are kept. Ghost may read any agent's.",
        annotations(read_only_hint = true)
    )]
    fn memory_read(&self, Parameters(p): Parameters<MemoryArgs>) -> CallToolResult {
        reply(self.do_memory_read(p))
    }
}
