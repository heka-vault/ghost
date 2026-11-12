//! The tools only an outside client has (ADR 0048), merged into the server's router by
//! [`GhostServer::for_outside`]: who the agents are, a written turn with one, a sentence
//! at the desk, and a wait for what changed. Ghost's own agents are not offered them.

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::clients::{CLIENT_SOURCE, shown};
use super::outside::{Outside, Said, WAIT_DEFAULT_SECS, WAIT_MAX_SECS};
use super::{GhostServer, Parts, clip, reply, source_key};
use crate::routes::Urgency;
use crate::waiting::{self, ItemDraft};

/// The longest sentence `say` speaks.
pub const SAY_MAX: usize = 300;
/// The longest turn `agent_ask` hands an agent.
pub const ASK_MAX: usize = 4_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AgentAsk {
    /// The agent, by the name agents_list gives.
    pub agent: String,
    /// What to ask it.
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Say {
    /// One sentence for the operator.
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EventsWait {
    /// The cursor the last call returned; leave out to wait for what happens from now.
    #[serde(default)]
    pub since: Option<u64>,
    /// How long to wait, in seconds (30 unless given, 60 at most).
    #[serde(default)]
    pub timeout_s: Option<f64>,
}

fn parsed<T: serde::de::DeserializeOwned>(args: &Map<String, Value>) -> Result<T, String> {
    let own: Map<String, Value> = args
        .iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    serde_json::from_value(Value::Object(own)).map_err(|e| e.to_string())
}

impl Outside {
    /// One of the changes only outside clients have, after the gate let it through.
    /// `from` is the caller, `client:<name>`.
    pub(super) async fn run(
        &self,
        parts: &Parts,
        tool: &str,
        from: &str,
        args: &Map<String, Value>,
    ) -> Result<Value, String> {
        if !from.starts_with(CLIENT_SOURCE) {
            return Err("this needs to know which client is calling".into());
        }
        match tool {
            "agent_ask" => self.agent_ask(parts, from, parsed(args)?).await,
            "say" => self.say(parts, from, parsed(args)?).await,
            other => Err(format!("unknown operation {other}")),
        }
    }

    async fn agent_ask(&self, parts: &Parts, from: &str, p: AgentAsk) -> Result<Value, String> {
        let text = p.text.trim();
        if text.is_empty() {
            return Err("ask it something".into());
        }
        if text.chars().count() > ASK_MAX {
            return Err(format!("a question is {ASK_MAX} characters at most"));
        }
        let wanted = p.agent.trim().to_lowercase();
        let agents = parts
            .store
            .list_conversations(false, false)
            .map_err(|e| e.to_string())?;
        let agent = agents
            .iter()
            .find(|a| a.slug == wanted || a.name.to_lowercase() == wanted)
            .ok_or_else(|| {
                let names: Vec<&str> = agents.iter().map(|a| a.slug.as_str()).collect();
                format!(
                    "no agent called {}; there are: {}",
                    p.agent.trim(),
                    names.join(", ")
                )
            })?;
        let answer = self.runner.ask(agent, text, from).await?;
        Ok(json!({"agent": agent.slug, "answer": answer}))
    }

    /// The sentence is an item first, so it is never lost; then, if the desk is free,
    /// Ghost says it.
    async fn say(&self, parts: &Parts, from: &str, p: Say) -> Result<Value, String> {
        let text = clip(&p.text, SAY_MAX);
        if text.is_empty() {
            return Err("say what?".into());
        }
        let name = from.strip_prefix(CLIENT_SOURCE).unwrap_or(from);
        let line = clip(&format!("From {}: {text}", shown(name)), waiting::TITLE_MAX);
        let key = source_key(from, &text);
        let mut fields = Map::new();
        fields.insert("urgency".into(), json!(Urgency::Notify.as_str()));
        parts
            .waiting
            .observe(
                from,
                &ItemDraft::new(key.clone(), line.clone()).fields(fields),
            )
            .map_err(|e| e.to_string())?;
        let item = parts
            .waiting
            .items
            .get(&key)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the item was not kept".to_string())?;
        Ok(match self.desk.say(&line, &item).await {
            Said::Speaking => json!({
                "key": key,
                "status": "saying it at the desk",
                "note": "events_wait says whether the operator answered; it stays on the \
                         waiting list until they do.",
            }),
            Said::Busy(why) => json!({
                "key": key,
                "status": "on the waiting list",
                "reason": why,
            }),
        })
    }
}

#[tool_router(router = outside_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Ghost's agents: each one's name and job, and whether the operator is in a session with it now.",
        annotations(read_only_hint = true)
    )]
    fn agents_list(&self) -> CallToolResult {
        reply((|| {
            let status = (self.parts.status)();
            let open = status
                .get("session")
                .and_then(|s| s.get("open"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let with = status.get("agent").and_then(Value::as_str).unwrap_or("");
            let agents: Vec<Value> = self
                .parts
                .store
                .list_conversations(false, false)
                .map_err(|e| e.to_string())?
                .iter()
                .map(|a| {
                    json!({
                        "agent": a.slug,
                        "name": a.name,
                        "job": a.job,
                        "in_session": open && a.slug == with,
                    })
                })
                .collect();
            Ok(json!({"agents": agents}))
        })())
    }

    #[tool(
        description = "Give one of Ghost's agents a written turn and get its written answer. Nothing is spoken at the desk."
    )]
    async fn agent_ask(&self, Parameters(p): Parameters<AgentAsk>) -> CallToolResult {
        reply(
            self.run_outside("agent_ask", json!({"agent": p.agent, "text": p.text}))
                .await,
        )
    }

    #[tool(
        description = "Have Ghost say one sentence to the operator at the desk now. If the desk is busy or it is a quiet time, it goes on the waiting list instead, as notify does."
    )]
    async fn say(&self, Parameters(p): Parameters<Say>) -> CallToolResult {
        reply(self.run_outside("say", json!({"text": p.text})).await)
    }

    #[tool(
        description = "Wait for something to change: an item on the waiting list added, acknowledged, snoozed or resolved, a change the operator answered, a session starting or ending. Returns the events and a cursor to pass as since next time; empty when the time runs out.",
        annotations(read_only_hint = true)
    )]
    async fn events_wait(&self, Parameters(p): Parameters<EventsWait>) -> CallToolResult {
        reply(self.do_events_wait(p).await)
    }
}

impl GhostServer {
    /// A change's own body, when it is reached with the caller already named.
    async fn run_outside(&self, tool: &str, args: Value) -> Result<Value, String> {
        let outside = self.outside()?;
        let from = self.agent(None)?;
        let args = args.as_object().cloned().unwrap_or_default();
        outside.run(&self.parts, tool, &from, &args).await
    }

    async fn do_events_wait(&self, p: EventsWait) -> Result<Value, String> {
        let outside = self.outside()?;
        let me = self.agent(None)?;
        let secs = p.timeout_s.unwrap_or(WAIT_DEFAULT_SECS);
        if !secs.is_finite() || secs < 0.0 {
            return Err("timeout_s is a number of seconds, 60 at most".into());
        }
        let timeout = Duration::from_secs_f64(secs.min(WAIT_MAX_SECS));
        let (events, cursor) = outside.events.wait(p.since, timeout).await;
        let events: Vec<Value> = events
            .into_iter()
            .map(|(n, mut e)| {
                if let Some(o) = e.as_object_mut() {
                    o.insert("n".into(), json!(n));
                    // An item's words are its source's, and a change's are its session's: a
                    // client reads its own, and of another's only that it happened.
                    let kind = o
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if kind == "waiting" || kind == "change" {
                        let mine = o.get("source").and_then(Value::as_str) == Some(me.as_str());
                        o.insert("yours".into(), json!(mine));
                        if !mine {
                            o.remove("title");
                            o.remove("target");
                            o.remove("summary");
                        }
                        if kind == "change" {
                            o.remove("source");
                        }
                    }
                }
                e
            })
            .collect();
        Ok(json!({"timed_out": events.is_empty(), "events": events, "cursor": cursor}))
    }
}
