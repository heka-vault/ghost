//! Agents working together, and an agent writing a skill (ADR 0052), on Ghost's own app.
//!
//! `ask_agent` hands another agent a written task and returns its written answer (OpenClaw's
//! `sessions_send` and Hermes Agent's `delegate_tool`, MIT, for the idea). It is the run with
//! nobody there (ADR 0047) that an outside tool's `agent_ask` is: the asked agent's own
//! prompt, memory and tools, the same gate (a change at `ask` is set aside, never made), 30
//! model calls and ten minutes at the most, and the turn kept in the asked agent's
//! transcript marked as asked by the agent that asked. Bounded twice more: an agent asked
//! this way cannot ask a third (depth 1), and a session has three such turns running at
//! the most. It is a change through the gate, so the operator's permissions can stop it
//! and the audit shows it. (Named `ask_agent`, not `agent_ask`: that name is an outside
//! client's tool under the same `ghost.` rules, and the gate keeps one operation per name.)
//!
//! `skill_write` makes or replaces a `SKILL.md` folder in Ghost's own skills folder from
//! what an agent learned. It writes the one file and nothing else, so it can never place a
//! script; the name and the result are checked as `skills.add` checks them. It waits for
//! the operator's yes unless their permissions say otherwise.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::outside_tools::ASK_MAX;
use super::recall::ASKED_BY_AGENT;
use super::{AGENT_SOURCE, GhostServer, reply};

/// How many `ask_agent` turns one session may have running at once.
pub const ASKING_MOST: usize = 3;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AskAgent {
    /// The agent to ask, by name or slug (list_agents gives them).
    pub agent: String,
    /// The task or question, written to stand alone: the agent sees nothing else of this conversation.
    pub text: String,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
    #[serde(default, rename = "_session")]
    #[schemars(skip)]
    pub session: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SkillWrite {
    /// The skill's name: lowercase letters, digits, dots and dashes (invoice-reminders).
    pub name: String,
    /// One line: what kind of task it is for.
    pub description: String,
    /// The instructions, in Markdown: what to do, in what order, and what to watch for.
    pub body: String,
    /// Replace a skill of that name you or another agent wrote before.
    #[serde(default)]
    pub replace: Option<bool>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

/// A slot of the session's three; given back when dropped.
struct Slot {
    asking: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    session: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut all = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = all.get_mut(&self.session) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                all.remove(&self.session);
            }
        }
    }
}

impl GhostServer {
    fn take_slot(&self, session: &str) -> Result<Slot, String> {
        let mut all = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let n = all.entry(session.to_string()).or_insert(0);
        if *n >= ASKING_MOST {
            return Err(format!(
                "{ASKING_MOST} agents are already working on this conversation's tasks; wait for one to answer"
            ));
        }
        *n += 1;
        Ok(Slot {
            asking: self.asking.clone(),
            session: session.to_string(),
        })
    }

    pub(crate) async fn do_ask_agent(&self, p: AskAgent) -> Result<Value, String> {
        let me = self.agent_only(p.caller.clone())?;
        let runner = self
            .more()
            .runner
            .clone()
            .ok_or("asking another agent is not available here")?;
        let session = p.session.clone().unwrap_or_default();
        if session.starts_with(ASKED_BY_AGENT) {
            return Err(
                "you were asked by another agent: answer it yourself; an agent asked this way \
                 cannot ask a third"
                    .into(),
            );
        }
        let text = p.text.trim();
        if text.is_empty() {
            return Err("say what the agent is to do".into());
        }
        if text.chars().count() > ASK_MAX {
            return Err(format!("a task is {ASK_MAX} characters at most"));
        }
        let wanted = p.agent.trim().to_lowercase();
        let agents = self
            .parts
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
        if agent.slug == me {
            return Err("that is you; do it yourself".into());
        }
        let _slot = self.take_slot(if session.is_empty() { &me } else { &session })?;
        let answer = runner
            .ask(agent, text, &format!("{AGENT_SOURCE}{me}"))
            .await?;
        Ok(json!({
            "agent": agent.slug,
            "answer": answer,
            "note": "Its answer, in its words. What it changed is in the audit; a change that needed the operator's yes is on their waiting list.",
        }))
    }

    pub(crate) fn do_skill_write(&self, p: SkillWrite) -> Result<Value, String> {
        let me = self.agent_only(p.caller.clone())?;
        let skills = self
            .more()
            .skills
            .clone()
            .ok_or("skills are not available here")?;
        let skill = skills
            .write_skill(
                &p.name,
                &p.description,
                &p.body,
                p.replace.unwrap_or(false),
                &me,
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "skill": skill.name,
            "description": skill.description,
            "status": "written",
            "note": "Every agent sees it in its list of skills from its next step.",
        }))
    }
}

#[tool_router(router = agents_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Give another of the operator's agents a written task and get its written answer. It works with its own memory and tools, through the same permissions; it cannot ask a third agent."
    )]
    async fn ask_agent(&self, Parameters(p): Parameters<AskAgent>) -> CallToolResult {
        reply(self.do_ask_agent(p).await)
    }
}

#[tool_router(router = skills_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Write a skill from what you learned: a page of instructions every agent can read for a kind of task. Only the page: never a script. The operator says yes first unless their permissions say otherwise."
    )]
    fn skill_write(&self, Parameters(p): Parameters<SkillWrite>) -> CallToolResult {
        reply(self.do_skill_write(p))
    }
}
