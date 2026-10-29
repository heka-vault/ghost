//! The active agent inside one session (ADR 0014, ADR 0035).
//!
//! `ConversationRuntime` knows which agent the session is in, builds the system prompt
//! from what every agent reaches plus Ghost's rules (`AGENTS.md`) and the agent's persona,
//! seeds the LLM context from its memory, notes and recent turns, records what is said, and
//! performs a transfer: Ghost's voice announces it, a short tone plays, the TTS voice
//! changes, the context is replaced, and the agent greets with a one-line recap. A transfer
//! sets aside any proposal still waiting for a yes.
//!
//! Compaction persistence lives here too: when the session's summarizer applies a summary,
//! the text is written to `memory.md` through the gate as a quiet, audited write.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_core::conversations::models::{CompactionSource, Role};
use ghost_core::conversations::{Conversations, GHOST, Profile, SOURCE, SUMMARY_TEMPLATE};
use ghost_core::folder::read_rules;
use ghost_core::gate::Request;
use ghost_core::models::{ConversationTarget, SetMemory};
use ghost_core::source::BoxFuture;
use ghost_core::{Autonomy, Change, ChangeSet, Target, ToolSource, WriteGate};
use ghost_llm::{Message, Tools};
use serde_json::{Map, Value};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::frames::{Audio, Frame};
use crate::prompt::{Persona, PromptLines, build_system_prompt_with};
use crate::turns::TurnControl;

pub const HANDOFF_NOTE: &str = "[system] The operator was just transferred to you from {origin}.{outcome} \
Greet them in one short sentence as yourself and say in a few words where this \
conversation left off, or that it is new. Then handle whatever they say next as \
usual, including asking to be transferred somewhere else.{handoff}";

pub type Reach = Arc<dyn Fn(&Profile) -> Vec<ToolSource> + Send + Sync>;
pub type OnSwitch = Arc<dyn Fn(Profile) -> BoxFuture<'static, ()> + Send + Sync>;
pub type ToolSchemas = Arc<dyn Fn(&[ToolSource]) -> Tools + Send + Sync>;

/// Two quick rising notes, like a line connecting. 16-bit mono.
pub fn transfer_tone(sample_rate: u32) -> Vec<i16> {
    let rate = f64::from(sample_rate);
    let mut out: Vec<f64> = Vec::new();
    for (freq, length) in [(740.0_f64, 0.07_f64), (988.0, 0.11)] {
        let n = (rate * length) as usize;
        for i in 0..n {
            let t = i as f64 / rate;
            let env = (t / 0.004).min(1.0) * (-t / (length * 0.45)).exp();
            out.push(env * (2.0 * std::f64::consts::PI * freq * t).sin());
        }
        out.extend(std::iter::repeat_n(0.0, (rate * 0.03) as usize));
    }
    let peak = out.iter().fold(0.0_f64, |m, v| m.max(v.abs())).max(1e-6);
    out.iter()
        .map(|v| (v * 0.35 / peak * 32767.0) as i16)
        .collect()
}

pub struct ConversationRuntime {
    conversations: Arc<Conversations>,
    gate: Arc<WriteGate>,
    session_id: String,
    fragments: Vec<String>,
    reach: Option<Reach>,
    /// Set by the tool dispatcher: the schemas for a list of sources, for a transfer.
    tool_schemas: Mutex<Option<ToolSchemas>>,
    sample_rate: u32,
    keep: usize,
    on_switch: Option<OnSwitch>,
    active: Mutex<Profile>,
    /// Follows the marker frames through the pipeline.
    speaking_as: Mutex<String>,
    /// Cleared from the start of a transfer until the new agent has greeted; a typed turn
    /// that arrives in that window waits (bounded), or it would run on the old context and
    /// race the greeting.
    settled: watch::Sender<bool>,
    turns: Mutex<Option<Arc<TurnControl>>>,
    lines: PromptLines,
}

impl ConversationRuntime {
    /// `reach(agent)` gives the sources a session with that agent gets (every connected
    /// app and Ghost's own, ADR 0035); None means every session gets `fragments` and
    /// whatever tools were registered.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversations: Arc<Conversations>,
        gate: Arc<WriteGate>,
        session_id: &str,
        fragments: Vec<String>,
        sample_rate: u32,
        keep_messages: usize,
        start: &str,
        on_switch: Option<OnSwitch>,
        reach: Option<Reach>,
    ) -> Arc<Self> {
        let p = conversations
            .get(start)
            .ok()
            .flatten()
            .or_else(|| conversations.ensure_ghost().ok())
            .expect("Ghost exists");
        let _ = conversations.touch(&p.slug);
        let (settled, _) = watch::channel(true);
        Arc::new(Self {
            conversations,
            gate,
            session_id: session_id.to_string(),
            fragments,
            reach,
            tool_schemas: Mutex::new(None),
            sample_rate,
            keep: keep_messages,
            on_switch,
            speaking_as: Mutex::new(p.slug.clone()),
            active: Mutex::new(p),
            settled,
            turns: Mutex::new(None),
            lines: PromptLines::default(),
        })
    }

    pub fn with_turns(self: Arc<Self>, turns: Arc<TurnControl>) -> Arc<Self> {
        *self.turns.lock().unwrap() = Some(turns);
        self
    }

    pub fn set_tool_schemas(&self, schemas: ToolSchemas) {
        *self.tool_schemas.lock().unwrap() = Some(schemas);
    }

    /// The schemas for a list of sources, as the dispatcher builds them.
    pub fn tool_schemas(&self, sources: &[ToolSource]) -> Option<Tools> {
        self.tool_schemas
            .lock()
            .unwrap()
            .as_ref()
            .map(|f| f(sources))
    }

    pub fn conversations(&self) -> &Arc<Conversations> {
        &self.conversations
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn active(&self) -> Profile {
        self.active.lock().unwrap().clone()
    }

    pub fn has_reach(&self) -> bool {
        self.reach.is_some()
    }

    pub fn settled(&self) -> bool {
        *self.settled.borrow()
    }

    /// Wait for a transfer in flight to land (bounded).
    pub async fn wait_settled(&self, within: Duration) {
        let mut rx = self.settled.subscribe();
        let _ = tokio::time::timeout(within, async {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
    }

    // --- what the session is built from -----------------------------------------

    /// The sources a session with this agent reaches (ADR 0034, ADR 0035): read on
    /// every call, so an app connected since is seen at once.
    pub fn sources_for(&self, profile: Option<&Profile>) -> Vec<ToolSource> {
        match &self.reach {
            None => Vec::new(),
            Some(reach) => match profile {
                Some(p) => reach(p),
                None => reach(&self.active()),
            },
        }
    }

    /// Ghost's listing: each other agent by name and job (ADR 0035).
    pub fn roster(&self) -> String {
        self.conversations
            .list(false)
            .unwrap_or_default()
            .iter()
            .filter(|p| !p.is_ghost())
            .map(|p| {
                if p.job.is_empty() {
                    format!("- {}", p.name)
                } else {
                    format!("- {}: {}", p.name, p.job)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn system_prompt(&self, profile: Option<&Profile>) -> String {
        let p = profile.cloned().unwrap_or_else(|| self.active());
        let fragments: Vec<String> = match &self.reach {
            None => self.fragments.clone(),
            Some(_) => self
                .sources_for(Some(&p))
                .into_iter()
                .map(|s| s.prompt)
                .filter(|f| !f.is_empty())
                .collect(),
        };
        let fragments: Vec<String> = fragments.into_iter().filter(|f| !f.is_empty()).collect();
        let persona = self
            .conversations
            .files
            .persona(&p.slug)
            .unwrap_or_default();
        let rules = read_rules(&self.conversations.config_dir);
        let roster = if p.is_ghost() {
            self.roster()
        } else {
            String::new()
        };
        build_system_prompt_with(
            &fragments,
            &Persona {
                persona: &persona,
                name: &p.name,
                is_ghost: p.is_ghost(),
                job: &p.job,
                rules: &rules,
                roster: &roster,
            },
            self.lines,
        )
    }

    pub fn initial_messages(&self, profile: Option<&Profile>) -> Vec<Message> {
        let p = profile.cloned().unwrap_or_else(|| self.active());
        match self.conversations.resume(&p.slug) {
            Ok(ctx) => ctx
                .messages()
                .into_iter()
                .map(|m| match m.role.as_str() {
                    "assistant" => Message::assistant(m.content),
                    "system" => Message::system(m.content),
                    _ => Message::user(m.content),
                })
                .collect(),
            Err(e) => {
                warn!("conversation {}: no context: {e}", p.slug);
                Vec::new()
            }
        }
    }

    /// Set the TTS voice for the agent the session opens in.
    pub fn start_frames(&self) -> Vec<Frame> {
        vec![self.voice_frame(&self.active())]
    }

    // --- records --------------------------------------------------------------------

    /// User turns belong to the agent the session is in now; assistant text belongs to
    /// the one that was speaking when it was generated (see the marker).
    pub fn record(&self, role: &str, text: &str) {
        let Some(r) = Role::parse(role) else { return };
        let active = self.active().slug;
        let slug = if role == "assistant" {
            self.speaking_as.lock().unwrap().clone()
        } else {
            active.clone()
        };
        if let Err(e) = self.conversations.record(&slug, &self.session_id, r, text) {
            warn!("conversation {slug}: turn not recorded: {e}");
        }
        if role == "assistant" && slug == active && !self.settled() {
            self.settled.send_replace(true); // the new agent has greeted; turns may flow again
        }
    }

    pub fn on_marker(&self, slug: &str) {
        *self.speaking_as.lock().unwrap() = slug.to_string();
    }

    pub fn record_call(&self, name: &str, args: &Map<String, Value>) {
        let mut keys: Vec<&String> = args.keys().filter(|k| !k.starts_with('_')).collect();
        keys.sort();
        let shown: Vec<String> = keys
            .iter()
            .map(|k| format!("{k}={}", short(&args[*k])))
            .collect();
        self.record("call", &format!("{name}({})", shown.join(", ")));
    }

    /// Save-as: what was said in this session so far moves to `target`.
    pub fn carry_session_turns(&self, target: &Profile) -> usize {
        self.conversations
            .carry(&self.session_id, &self.active().slug, &target.slug)
            .unwrap_or(0)
    }

    // --- transfer --------------------------------------------------------------------

    /// An agent by name (ADR 0035).
    pub fn resolve(&self, spoken: &str) -> Option<Profile> {
        self.conversations.resolve(spoken, false).ok().flatten()
    }

    pub fn switch_allowed(&self) -> bool {
        self.gate.autonomy().resolve(SOURCE, "switch", None) != Autonomy::Deny
    }

    fn turn_token(&self) -> CancellationToken {
        self.turns
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(CancellationToken::new, |t| t.current())
    }

    /// Perform the transfer and return the frames that carry it out, in order: the
    /// announce line, the tone, the voice change, the marker, the new system prompt, the
    /// new tool list, and the context replacement that makes the agent greet. The caller
    /// feeds them in at the pipeline source once any in-flight tool result has been
    /// appended to the context (`settled` is cleared until then). `outcome` is what just
    /// happened (a created agent), told to the new agent so it does not redo it.
    pub async fn switch(&self, target: &Profile, handoff: &str, outcome: &str) -> Vec<Frame> {
        let origin = self.active();
        if target.slug == origin.slug {
            return Vec::new();
        }
        if matches!(self.gate.pending(&self.session_id, None), Ok(Some(_))) {
            // Set aside, not cancelled: it waits on the waiting list (ADR 0030).
            let _ = self
                .gate
                .set_aside(&self.session_id, "switched conversation");
        }
        self.settled.send_replace(false);
        let line = if target.is_ghost() {
            "Back to Ghost.".to_string()
        } else {
            format!("Transferring you to {}.", target.name)
        };
        self.record("system", &format!("transferred to {}", target.name));
        *self.active.lock().unwrap() = target.clone();
        let _ = self.conversations.touch(&target.slug);
        self.record("system", &format!("transferred from {}", origin.name));
        if let Some(on_switch) = &self.on_switch {
            on_switch(target.clone()).await;
        }
        let note = HANDOFF_NOTE
            .replace("{origin}", &origin.name)
            .replace(
                "{outcome}",
                &if outcome.trim().is_empty() {
                    String::new()
                } else {
                    format!(" {}", outcome.trim())
                },
            )
            .replace(
                "{handoff}",
                &if handoff.trim().is_empty() {
                    String::new()
                } else {
                    format!(" Handoff from {}: {}", origin.name, handoff.trim())
                },
            );
        let mut messages = self.initial_messages(Some(target));
        messages.push(Message::user(note));
        let mut frames = vec![
            Frame::Speak(line), // the tap records it
            Frame::AudioOut {
                audio: Audio::new(transfer_tone(self.sample_rate), self.sample_rate),
                turn: self.turn_token(),
            },
            self.voice_frame(target),
            Frame::Marker(target.slug.clone()),
            Frame::SystemPrompt(self.system_prompt(Some(target))),
        ];
        if self.reach.is_some()
            && let Some(tools) = self.tool_schemas(&self.sources_for(Some(target)))
        {
            frames.push(Frame::SetTools(tools));
        }
        frames.push(Frame::ContextUpdate {
            messages,
            run_llm: true,
        });
        frames
    }

    /// The transfer frames are in the pipeline. `settled` is set when the new agent's
    /// greeting has been generated (see `record`); a caller that could not queue the
    /// frames releases it here so nothing waits forever.
    pub fn transfer_done(&self) {
        if *self.speaking_as.lock().unwrap() == self.active().slug {
            self.settled.send_replace(true);
        }
    }

    pub fn voice_frame(&self, p: &Profile) -> Frame {
        let (voice, speed) = self
            .conversations
            .voice_for(p)
            .unwrap_or_else(|_| (self.conversations.default_voice.clone(), 1.0));
        Frame::VoiceChange { voice, speed }
    }

    // --- compaction --------------------------------------------------------------------

    /// Called after the session's summarizer replaced older messages with a summary. The
    /// summary is the first non-system message; persist it as the agent's memory and mark
    /// the turns it stands in for. Which turns is approximate: everything but the last
    /// `keep_messages` spoken turns, which errs toward keeping more raw.
    pub async fn on_summary_applied(&self, messages: &[Message]) {
        let Some(summary) = summary_text(messages) else {
            return;
        };
        let slug = self.active().slug;
        let recent = match self.conversations.resume(&slug) {
            Ok(ctx) => ctx.recent,
            Err(_) => Vec::new(),
        };
        let spoken: Vec<_> = recent
            .iter()
            .filter(|t| matches!(t.role, Role::User | Role::Assistant))
            .collect();
        let through = if spoken.len() > self.keep {
            Some(spoken[spoken.len() - self.keep - 1].id)
        } else {
            None
        };
        let mut req = Request::new(
            &self.session_id,
            SOURCE,
            "compact",
            Target::Conversation(ConversationTarget { slug: slug.clone() }),
            ChangeSet::one(Change::SetMemory(SetMemory {
                text: summary.clone(),
                through_turn_id: through,
                source: CompactionSource::Auto,
            })),
            "automatic compaction",
        );
        req.conversation = Some(slug.clone());
        match self.gate.apply_internal(req).await {
            Ok(out) => info!(
                "compacted {slug}: {}, memory {} chars",
                crate::confirmation::status_str(out.status),
                summary.len()
            ),
            Err(e) => warn!("compaction not persisted: {e}"),
        }
    }
}

fn summary_text(messages: &[Message]) -> Option<String> {
    let prefix = SUMMARY_TEMPLATE.split("{summary}").next().unwrap_or("");
    for m in messages {
        let content = match m {
            Message::System(_) => continue,
            Message::User(t) => t.as_str(),
            Message::Assistant { text, .. } => text.as_str(),
            Message::Tool { content, .. } => content.as_str(),
        };
        return content
            .strip_prefix(prefix)
            .map(|rest| rest.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    None
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() <= 40 {
        s
    } else {
        format!("{}...", s.chars().take(37).collect::<String>())
    }
}

/// Where an agent's slug comes from after `create`: the target's scope.
pub fn created_slug(target: &Target) -> String {
    target.scope()
}

/// The slug of the agent a session starts in when none is named.
pub fn default_start() -> &'static str {
    GHOST
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_tone_is_short_mono_pcm() {
        let data = transfer_tone(24000);
        assert!((0.15 * 24000.0) < data.len() as f64 && (data.len() as f64) < 0.4 * 24000.0);
    }
}
