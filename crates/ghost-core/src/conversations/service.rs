//! The agents facade (ADR 0014, ADR 0035): the registry read from Ghost's `agents/`
//! folder, name resolution, what a session loads on resume, turn and summary records,
//! retention, and the operations the gate registers under the `conversations` source name.
//!
//! Switching is not here: it is a session action in the voice pipeline. Everything that
//! changes a row or a file is an operation below, so it runs at the operator's level
//! and is audited.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tracing::warn;

use crate::conversations::files::{ConversationFiles, read_agent_json};
use crate::conversations::models::{
    Compaction, CompactionSource, GHOST, MAX_JOB, MAX_MEMORY, MAX_NAME, MAX_NOTE, MAX_PERSONA,
    Profile, Role, TurnRecord, new_id, slugify,
};
use crate::conversations::writer::{ConversationWriter, agent_map};
use crate::levels::Risk;
use crate::models::{
    AppendNote, Change, ChangeSet, ConversationTarget, Create, Rename, SetMemory, SetNotes,
    SetPersona, SetVoice, Target, py_repr, py_str,
};
use crate::source::{Operation, Prepared, ReadTool, ToolSource};
use crate::store::{Store, StoreError};
use crate::time::{iso, utcnow};

/// The gate, policy and audit key for the agents' operations.
pub const SOURCE: &str = "conversations";
pub const DEFAULT_VOICE: &str = "af_heart";
pub const SUMMARY_TEMPLATE: &str = "What this conversation has covered so far: {summary}";
pub const NEXT_SESSION: &str = "(next session)";
pub const RECORD_END: &str = "(end of the record)";
/// Earlier turns arrive as a record, not as replayed chat: replayed, the model sees its own
/// past claims with none of the tool calls behind them and "corrects" lookups that were real.
/// Sessions are marked and the record's end is spelled out: unmarked, a request that opened
/// a session ("Ghost, let's work on …") read as the tail of the last one's goodbye, and
/// Ghost said "talk soon" and hung up (2026-10-24).
pub const RECENT_TEMPLATE: &str = "The record of what was said earlier in this conversation, oldest first, with \
(next session) where one session ended and a later one began. The lookups and changes \
in it really happened; their results are not repeated here, so look again if you need \
current details. A goodbye in it ended that earlier session, not this one. What \
follows (end of the record) is live.\n\n{record}\n(end of the record)";

/// Ghost's own `agents/` folder name under its config folder (ADR 0035).
const AGENTS_DIR: &str = "agents";

const STOP: [&str; 15] = [
    "the",
    "a",
    "an",
    "to",
    "me",
    "with",
    "team",
    "office",
    "desk",
    "agent",
    "people",
    "folks",
    "group",
    "conversation",
    "thread",
];

#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("no agent {}", py_repr(.0))]
    NoAgent(String),
    #[error("no agent called {}", py_repr(.0))]
    NoAgentCalled(String),
    #[error("no agent {} to seed from", py_repr(.0))]
    NoSeed(String),
    #[error("Ghost's memory is private; a handoff from Ghost is a summary you write")]
    GhostMemoryPrivate,
    #[error("there is already an agent called {}", py_repr(.0))]
    AlreadyExists(String),
    #[error("a name is required")]
    NameRequired,
    #[error("that name is too long")]
    NameTooLong,
    #[error("that persona is too long")]
    PersonaTooLong,
    #[error("keep the job to a line")]
    JobTooLong,
    #[error("that memory is too long")]
    MemoryTooLong,
    #[error("aliases must be a list")]
    AliasesNotAList,
    #[error("{} is not an installed voice", py_repr(.0))]
    NotInstalledVoice(String),
    #[error("{} is not a voice id", py_repr(.0))]
    NotAVoiceId(String),
    #[error("speed must be a number")]
    SpeedNotANumber,
    #[error("speed must be between 0.5 and 2")]
    SpeedOutOfRange,
    #[error("Ghost stays")]
    GhostStays,
    #[error("Ghost cannot be deleted")]
    GhostCannotBeDeleted,
    #[error("{0} is already retired")]
    AlreadyRetired(String),
    #[error("{0} is not retired")]
    NotRetired(String),
    #[error("nothing to remember")]
    NothingToRemember,
    #[error("that note is too long; keep it to a sentence or two")]
    NoteTooLong,
    #[error("no note matches that")]
    NoNoteMatches,
    #[error("a summary is required")]
    SummaryRequired,
    #[error("expected a list of names")]
    ExpectedNames,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, ConversationError>;

/// One LLM message standing in for the past.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// What a session loads when it enters an agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumeContext {
    pub profile: Profile,
    pub persona: String,
    pub memory: String,
    pub notes: Vec<String>,
    pub recent: Vec<TurnRecord>,
}

impl ResumeContext {
    /// The LLM messages that stand in for the past: the rolling summary in the same
    /// shape the in-session summarizer uses, the pinned notes, then the record of the
    /// turns not yet covered by a summary.
    pub fn messages(&self) -> Vec<Message> {
        let mut out = Vec::new();
        if !self.memory.trim().is_empty() {
            out.push(Message {
                role: "user".into(),
                content: SUMMARY_TEMPLATE.replace("{summary}", self.memory.trim()),
            });
        }
        if !self.notes.is_empty() {
            out.push(Message {
                role: "user".into(),
                content: format!(
                    "Things the operator asked you to remember, verbatim:\n- {}",
                    self.notes.join("\n- ")
                ),
            });
        }
        let mut lines: Vec<String> = Vec::new();
        let mut session: Option<&str> = None;
        for t in &self.recent {
            if !lines.is_empty() && session != Some(t.session_id.as_str()) {
                lines.push(NEXT_SESSION.into());
            }
            session = Some(t.session_id.as_str());
            match t.role {
                Role::User => lines.push(format!("Operator: {}", t.text)),
                Role::Assistant => lines.push(format!("You: {}", t.text)),
                Role::Call => lines.push(format!("(you called {})", t.text)),
                Role::System => {}
            }
        }
        if !lines.is_empty() {
            out.push(Message {
                role: "user".into(),
                content: RECENT_TEMPLATE.replace("{record}", &lines.join("\n")),
            });
        }
        merge_adjacent(out)
    }
}

fn merge_adjacent(msgs: Vec<Message>) -> Vec<Message> {
    let mut merged: Vec<Message> = Vec::new();
    for m in msgs {
        match merged.last_mut() {
            Some(last) if last.role == m.role => {
                last.content = format!("{}\n\n{}", last.content, m.content);
            }
            _ => merged.push(m),
        }
    }
    merged
}

/// The keyword arguments `Conversations` took in Python, with their defaults.
#[derive(Debug, Clone)]
pub struct ConversationsOptions {
    pub default_voice: String,
    pub default_speed: f64,
    pub voices: Vec<String>,
    pub recent_limit: i64,
    pub retention: Duration,
}

impl Default for ConversationsOptions {
    fn default() -> Self {
        Self {
            default_voice: DEFAULT_VOICE.into(),
            default_speed: 1.0,
            voices: Vec::new(),
            recent_limit: 40,
            retention: Duration::days(90),
        }
    }
}

pub struct Conversations {
    store: Arc<Store>,
    pub config_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub files: Arc<ConversationFiles>,
    pub writer: Arc<ConversationWriter>,
    pub default_voice: String,
    pub default_speed: f64,
    voices: Vec<String>,
    recent_limit: i64,
    retention: Duration,
}

impl Conversations {
    /// The folders under `config_dir/agents/` are the registry; the store mirrors them
    /// and holds what only Ghost writes. `sync` reads the folders; every write below
    /// keeps both in step.
    pub fn new(
        store: Arc<Store>,
        config_dir: impl AsRef<Path>,
        options: ConversationsOptions,
    ) -> Result<Self> {
        let config_dir = expanduser(config_dir.as_ref());
        let agents_dir = config_dir.join(AGENTS_DIR);
        let locate_root = agents_dir.clone();
        let files = Arc::new(ConversationFiles::new(Arc::new(move |slug: &str| {
            locate_root.join(slug)
        })));
        let writer = Arc::new(ConversationWriter::new(
            store.clone(),
            files.clone(),
            agents_dir.clone(),
        ));
        let this = Self {
            store,
            config_dir,
            agents_dir,
            files,
            writer,
            default_voice: options.default_voice,
            default_speed: options.default_speed,
            voices: options.voices,
            recent_limit: options.recent_limit,
            retention: options.retention,
        };
        this.sync()?;
        Ok(this)
    }

    // --- registry ------------------------------------------------------------

    /// An agent's folder: `agents/<slug>/` in Ghost's folder (ADR 0035).
    pub fn dir_for(&self, slug: &str) -> PathBuf {
        self.agents_dir.join(slug)
    }

    /// Read `agents/` into the store: a folder is an agent (its `agent.json` is its
    /// identity), Ghost is made when it is missing, and a row whose folder is gone is
    /// marked deleted. Returns the problems found.
    pub fn sync(&self) -> Result<Vec<String>> {
        let mut problems: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        // Deleted rows are kept (slug is unique): a folder made again under an old slug
        // takes its row and id back instead of colliding with it.
        let mut rows: BTreeMap<String, Profile> = self
            .store
            .list_conversations(true, true)?
            .into_iter()
            .map(|p| (p.slug.clone(), p))
            .collect();
        let mut ids: BTreeSet<String> = rows.values().map(|p| p.id.clone()).collect();
        ids.insert(self.ensure_ghost_in(&mut rows)?.id); // agents/ exists from here on
        let mut folders: Vec<PathBuf> = std::fs::read_dir(&self.agents_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let file_name = folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let slug = file_name.to_lowercase();
            if file_name != slug {
                problems.push(format!(
                    "{}: an agent folder's name is not lower-case; not read",
                    folder.display()
                ));
                continue;
            }
            let existing = rows.get(&slug).cloned();
            let data = match read_agent_json(&folder) {
                Some(d) => d,
                None => match &existing {
                    Some(e) => agent_map(e),
                    None => {
                        problems.push(format!("{}: no agent.json; not an agent", folder.display()));
                        continue;
                    }
                },
            };
            // The row's id wins over agent.json (an edited or restored file); an id
            // another agent already has (a copied folder) is a new agent.
            let file_id = data.get("id").filter(|v| truthy(v)).map(py_str);
            let agent_id = match (&existing, file_id) {
                (Some(e), _) => e.id.clone(),
                (None, Some(id)) if !ids.contains(&id) => id,
                _ => new_id(),
            };
            ids.insert(agent_id.clone());
            let now = utcnow();
            let name = match data.get("name").filter(|v| truthy(v)) {
                Some(v) => py_str(v),
                None => match &existing {
                    Some(e) => e.name.clone(),
                    None => title_case(&slug.replace('-', " ")),
                },
            };
            let p = Profile {
                id: agent_id,
                slug: slug.clone(),
                name,
                aliases: aliases_of(&data)
                    .into_iter()
                    .filter(|a| !a.trim().is_empty())
                    .collect(),
                job: data
                    .get("job")
                    .filter(|v| truthy(v))
                    .map(py_str)
                    .unwrap_or_default()
                    .chars()
                    .take(MAX_JOB)
                    .collect(),
                voice: data.get("voice").filter(|v| truthy(v)).map(py_str),
                speed: data.get("speed").filter(|v| !v.is_null()).and_then(as_f64),
                created_at: existing.as_ref().map_or(now, |e| e.created_at),
                updated_at: existing.as_ref().map_or(now, |e| e.updated_at),
                last_active_at: existing.as_ref().and_then(|e| e.last_active_at),
                retired_at: existing.as_ref().and_then(|e| e.retired_at),
                deleted_at: None,
            };
            if existing.as_ref() != Some(&p) {
                self.store.upsert_conversation(&p)?;
            }
            if Value::Object(data) != p.agent_json() {
                // a hand-made folder gets its id written back
                self.files.write_agent(&slug, &agent_map(&p))?;
            }
            seen.insert(slug);
        }
        for (slug, p) in &rows {
            if !seen.contains(slug) && p.deleted_at.is_none() {
                warn!("agents: {slug}'s folder is gone; marked deleted");
                let mut q = p.clone();
                q.deleted_at = Some(utcnow());
                self.store.upsert_conversation(&q)?;
            }
        }
        for problem in &problems {
            warn!("agents: {problem}");
        }
        Ok(problems)
    }

    /// Agent zero, where every wake lands: its row and its folder, made when either is
    /// missing. Its folder is then read like any agent's.
    fn ensure_ghost_in(&self, rows: &mut BTreeMap<String, Profile>) -> Result<Profile> {
        let existing = rows.get(GHOST).cloned();
        let folder = self.agents_dir.join(GHOST);
        if let Some(e) = &existing
            && e.deleted_at.is_none()
            && folder.is_dir()
        {
            return Ok(e.clone());
        }
        let data = read_agent_json(&folder).unwrap_or_default();
        let id = data
            .get("id")
            .filter(|v| truthy(v))
            .map(py_str)
            .or_else(|| existing.as_ref().map(|e| e.id.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(new_id);
        let name = data
            .get("name")
            .filter(|v| truthy(v))
            .map(py_str)
            .unwrap_or_else(|| "Ghost".into());
        let mut aliases = aliases_of(&data);
        if aliases.is_empty() {
            aliases = vec!["ghost".into(), "primary".into(), "main line".into()];
        }
        let mut p = Profile::new(GHOST, name);
        p.id = id;
        p.aliases = aliases;
        if let Some(e) = &existing {
            p.created_at = e.created_at;
        }
        self.store.upsert_conversation(&p)?;
        if !folder.join(crate::conversations::files::AGENT).is_file() {
            self.files.write_agent(GHOST, &agent_map(&p))?;
        }
        rows.insert(GHOST.into(), p.clone());
        Ok(p)
    }

    pub fn ensure_ghost(&self) -> Result<Profile> {
        if let Some(p) = self.store.get_conversation(GHOST, false)? {
            return Ok(p);
        }
        self.sync()?;
        self.store
            .get_conversation(GHOST, false)?
            .ok_or_else(|| ConversationError::NoAgent(GHOST.into()))
    }

    pub fn get(&self, slug: &str) -> Result<Option<Profile>> {
        Ok(self.store.get_conversation(slug, false)?)
    }

    pub fn by_id(&self, id: &str) -> Result<Option<Profile>> {
        Ok(self.store.get_conversation_by_id(id)?)
    }

    /// Live agents, Ghost first. Retired ones are left out unless asked for: they are
    /// readable, not routable.
    pub fn list(&self, include_retired: bool) -> Result<Vec<Profile>> {
        Ok(self.store.list_conversations(include_retired, false)?)
    }

    /// The agent the operator most likely means by `spoken`: exact slug, then any name
    /// or alias, then word overlap with filler words removed. A retired agent is not a
    /// match unless `include_retired` (to bring it back).
    pub fn resolve(&self, spoken: &str, include_retired: bool) -> Result<Option<Profile>> {
        let text = spoken.trim().to_lowercase();
        if text.is_empty() {
            return Ok(None);
        }
        let profiles = self.list(include_retired)?;
        let slug = slugify(&text);
        if let Some(p) = profiles.iter().find(|p| p.slug == slug) {
            return Ok(Some(p.clone()));
        }
        if let Some(p) = profiles
            .iter()
            .find(|p| p.spoken_names().iter().any(|n| n.to_lowercase() == text))
        {
            return Ok(Some(p.clone()));
        }
        let want = words(&text);
        if want.is_empty() {
            return Ok(None);
        }
        let mut best: Option<(usize, &Profile)> = None;
        for p in &profiles {
            let mut cand = BTreeSet::new();
            for n in p.spoken_names() {
                cand.extend(words(&n.to_lowercase()));
            }
            let score = want.intersection(&cand).count();
            if score > 0 && best.is_none_or(|(b, _)| score > b) {
                best = Some((score, p));
            }
        }
        Ok(best.map(|(_, p)| p.clone()))
    }

    pub fn touch(&self, slug: &str) -> Result<()> {
        if let Some(p) = self.get(slug)? {
            self.store.touch_conversation(&p.id)?;
        }
        Ok(())
    }

    pub fn voices(&self) -> &[String] {
        &self.voices
    }

    /// The voice heard: the agent's own, else Ghost's, else the default (ADR 0035).
    pub fn voice_for(&self, p: &Profile) -> Result<(String, f64)> {
        let ghost = if p.is_ghost() {
            Some(p.clone())
        } else {
            self.get(GHOST)?
        };
        let voice = p
            .voice
            .clone()
            .or_else(|| ghost.as_ref().and_then(|g| g.voice.clone()))
            .unwrap_or_else(|| self.default_voice.clone());
        let speed = p
            .speed
            .or_else(|| ghost.as_ref().and_then(|g| g.speed))
            .unwrap_or(self.default_speed);
        Ok((voice, speed))
    }

    /// A voice no live agent uses yet, so a new agent sounds like itself. The default
    /// when every installed voice is taken (or none is known).
    pub fn unused_voice(&self) -> Result<String> {
        let mut taken: BTreeSet<String> = self
            .list(true)?
            .into_iter()
            .filter_map(|p| p.voice)
            .collect();
        taken.insert(self.default_voice.clone());
        Ok(self
            .voices
            .iter()
            .find(|v| !taken.contains(*v))
            .cloned()
            .unwrap_or_else(|| self.default_voice.clone()))
    }

    // --- what a session loads -------------------------------------------------

    pub fn resume(&self, slug: &str) -> Result<ResumeContext> {
        let p = self
            .get(slug)?
            .ok_or_else(|| ConversationError::NoAgent(slug.into()))?;
        Ok(ResumeContext {
            persona: self.files.persona(slug)?,
            memory: self.files.memory(slug)?,
            notes: self.files.note_lines(slug)?,
            recent: self.store.recent_turns(&p.id, self.recent_limit, true)?,
            profile: p,
        })
    }

    // --- transcript ------------------------------------------------------------

    fn id(&self, slug: &str) -> Result<String> {
        self.store
            .get_conversation(slug, true)?
            .map(|p| p.id)
            .ok_or_else(|| ConversationError::NoAgent(slug.into()))
    }

    pub fn record(
        &self,
        slug: &str,
        session_id: &str,
        role: Role,
        text: &str,
    ) -> Result<Option<i64>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.store.add_turn(
            &self.id(slug)?,
            session_id,
            role,
            text,
        )?))
    }

    /// Move the turns of one session from `src` to `dst` (save this as ...).
    pub fn carry(&self, session_id: &str, src: &str, dst: &str) -> Result<usize> {
        Ok(self
            .store
            .move_session_turns(session_id, &self.id(src)?, &self.id(dst)?)?)
    }

    pub fn turns(&self, slug: &str, limit: i64) -> Result<Vec<TurnRecord>> {
        Ok(self.store.recent_turns(&self.id(slug)?, limit, false)?)
    }

    pub fn compactions(&self, slug: &str) -> Result<Vec<Compaction>> {
        Ok(self.store.compactions(&self.id(slug)?, 50)?)
    }

    pub fn last_turn_id(&self, slug: &str) -> Result<Option<i64>> {
        Ok(self.store.last_turn_id(&self.id(slug)?)?)
    }

    /// Drop summarized turns older than the retention window.
    pub fn prune(&self) -> Result<usize> {
        Ok(self.store.prune_turns(utcnow() - self.retention)?)
    }

    // --- operations for the gate --------------------------------------------------

    pub fn read_tools(self: &Arc<Self>) -> Vec<ReadTool> {
        let this = self.clone();
        vec![ReadTool {
            name: "list_agents".into(),
            description: "The agents, with names, aliases and jobs: who the operator can be \
transferred to. Retired agents are not listed."
                .into(),
            properties: json!({}),
            required: Vec::new(),
            handler: Arc::new(move |_args| {
                let this = this.clone();
                Box::pin(async move {
                    let agents: Vec<Value> = this
                        .list(false)?
                        .iter()
                        .map(|p| {
                            json!({
                                "name": p.name,
                                "slug": p.slug,
                                "aliases": p.aliases,
                                "job": if p.job.is_empty() { Value::Null } else { json!(p.job) },
                                "last_active": p.last_active_at.as_ref().map(iso),
                            })
                        })
                        .collect();
                    let mut out = Map::new();
                    out.insert("agents".into(), Value::Array(agents));
                    Ok(out)
                })
            }),
            operation: String::new(),
        }]
    }

    pub fn operations(self: &Arc<Self>) -> Vec<Operation> {
        let voice_prop = {
            let mut prop = json!({"type": "string", "description": "A Kokoro voice id."});
            if !self.voices.is_empty() {
                prop["enum"] = json!(self.voices);
            }
            prop
        };
        let conv = json!({
            "type": "string",
            "description": "Which agent, by name. Omit for the one you are in.",
        });
        vec![
            self.op(
                "create",
                "Hire a new agent: a conversation with its own persona and job. Use it \
at once, without asking, when the operator says 'save this as X', 'start \
an agent for X' or 'hire someone for X'; it switches them to it. Always \
write a persona and a one-line job. Leave voice out to speak in Ghost's.",
                json!({
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                    "job": {
                        "type": "string",
                        "description": "what it is for, in one line (brand design, the Q3 audit)",
                    },
                    "voice": voice_prop,
                    "speed": {"type": "number"},
                    "persona": {
                        "type": "string",
                        "description": "Who this agent is, in two or three sentences, written from what \
the conversation has been about: its subject, how the operator \
talks about it, and a manner that fits (brisk for ops, careful for \
money, warm for people). Distinct from the other agents; never a \
generic assistant. The operator edits it later.",
                    },
                    "seed_from": {
                        "type": "string",
                        "description": "Slug of a retired agent whose memory becomes this one's first \
memory, as a handoff.",
                    },
                    "memory": {
                        "type": "string",
                        "description": "The new agent's first memory: a summary you write of this \
session so far, when the operator hands this conversation to a \
new agent ('save this as X'). Facts, decisions, open items.",
                    },
                }),
                &["name"],
                Self::prepare_create,
                Risk::Low,
                &["hire", "start", "save"],
            ),
            self.op(
                "retire",
                "Retire an agent: it leaves the roster and voice routing but keeps its \
transcript and memory, and can be rehired. Nothing is deleted.",
                json!({"conversation": conv}),
                &[],
                Self::prepare_retire,
                Risk::Low,
                &["retire", "fire", "let go"],
            ),
            self.op(
                "rehire",
                "Bring a retired agent back onto the roster.",
                json!({"conversation": conv}),
                &[],
                Self::prepare_rehire,
                Risk::Low,
                &["rehire", "bring back"],
            ),
            self.op(
                "rename",
                "Rename an agent, change what it answers to, or restate its job.",
                json!({
                    "conversation": conv,
                    "name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}},
                    "job": {"type": "string"},
                }),
                &[],
                Self::prepare_rename,
                Risk::Low,
                &[],
            ),
            self.op(
                "set_voice",
                "Change an agent's voice or speaking speed; no voice means Ghost's.",
                json!({
                    "conversation": conv,
                    "voice": voice_prop,
                    "speed": {"type": "number", "description": "0.5 to 2.0"},
                }),
                &[],
                Self::prepare_set_voice,
                Risk::Low,
                &[],
            ),
            self.op(
                "set_persona",
                "Set who this agent is, in the operator's own words, verbatim. Never paraphrase.",
                json!({"conversation": conv, "text": {"type": "string"}}),
                &["text"],
                Self::prepare_set_persona,
                Risk::Medium,
                &[],
            ),
            self.op(
                "remember",
                "Pin something the operator asked you to remember, in their words. \
Never summarized away.",
                json!({"conversation": conv, "text": {"type": "string"}}),
                &["text"],
                Self::prepare_remember,
                Risk::Low,
                &[],
            ),
            self.op(
                "forget",
                "Drop a pinned note that contains the given words.",
                json!({"conversation": conv, "text": {"type": "string"}}),
                &["text"],
                Self::prepare_forget,
                Risk::Medium,
                &[],
            ),
            self.op(
                "compact",
                "Replace this agent's rolling memory with a fresh summary you write: \
facts, decisions, open items. Only when the operator asks.",
                json!({"conversation": conv, "summary": {"type": "string"}}),
                &["summary"],
                Self::prepare_compact,
                Risk::Low,
                &[],
            ),
            self.op(
                "reset",
                "Forget everything an agent knows: its memory, notes and transcript. \
Keeps the name, job and voice.",
                json!({"conversation": conv}),
                &[],
                Self::prepare_reset,
                Risk::High,
                &[],
            ),
            self.op(
                "delete",
                "Remove an agent entirely, its folder included. Retire is the usual way.",
                json!({"conversation": conv}),
                &[],
                Self::prepare_delete,
                Risk::High,
                &[],
            ),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn op(
        self: &Arc<Self>,
        name: &str,
        description: &str,
        properties: Value,
        required: &[&str],
        prepare: fn(&Conversations, &Map<String, Value>) -> Result<Prepared>,
        risk: Risk,
        asked_by: &[&str],
    ) -> Operation {
        let this = self.clone();
        Operation {
            name: name.into(),
            description: description.into(),
            properties,
            required: required.iter().map(|s| s.to_string()).collect(),
            prepare: Arc::new(move |args| {
                let this = this.clone();
                Box::pin(async move { Ok(prepare(&this, &args)?) })
            }),
            writer: self.writer.clone(),
            risk,
            allowed: Arc::new(ConversationWriter::allowed),
            asked_by: asked_by.iter().map(|s| s.to_string()).collect(),
            tool_name: None,
        }
    }

    fn prepare_create(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let name = name_arg(args.get("name"))?;
        let slug = slugify(&name);
        if self.store.get_conversation(&slug, true)?.is_some() {
            return Err(ConversationError::AlreadyExists(name));
        }
        let voice = voice_arg(args.get("voice"), &self.voices)?;
        let persona = args.get("persona").map(py_str_or_empty).unwrap_or_default();
        let persona = persona.trim().to_string();
        if persona.chars().count() > MAX_PERSONA {
            return Err(ConversationError::PersonaTooLong);
        }
        let job = normalize(&args.get("job").map(py_str_or_empty).unwrap_or_default());
        if job.chars().count() > MAX_JOB {
            return Err(ConversationError::JobTooLong);
        }
        let mut memory = normalize(&args.get("memory").map(py_str_or_empty).unwrap_or_default());
        let seed = args
            .get("seed_from")
            .map(py_str_or_empty)
            .unwrap_or_default();
        let seed = seed.trim();
        if !seed.is_empty() {
            memory = self.handoff_memory(seed)?;
        }
        if memory.chars().count() > MAX_MEMORY {
            return Err(ConversationError::MemoryTooLong);
        }
        Ok(Prepared {
            target: Target::Conversation(ConversationTarget { slug }),
            changes: ChangeSet::one(Change::Create(Create {
                name: name.clone(),
                aliases: aliases_arg(args.get("aliases"))?,
                job,
                voice,
                speed: speed_arg(args.get("speed"), None)?,
                persona,
                memory,
            })),
            rationale: format!("hire {name}"),
            asked_by: Vec::new(),
        })
    }

    fn prepare_retire(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        if p.is_ghost() {
            return Err(ConversationError::GhostStays);
        }
        if p.retired() {
            return Err(ConversationError::AlreadyRetired(p.name));
        }
        Ok(prepared(&p.slug, Change::Retire, ""))
    }

    fn prepare_rehire(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, true)?;
        if !p.retired() {
            return Err(ConversationError::NotRetired(p.name));
        }
        Ok(prepared(&p.slug, Change::Rehire, ""))
    }

    fn prepare_rename(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let name = match args.get("name") {
            Some(v) => name_arg(Some(v))?,
            None => name_arg(Some(&Value::String(p.name.clone())))?,
        };
        let aliases = match args.get("aliases") {
            Some(v) => aliases_arg(Some(v))?,
            None => p.aliases.clone(),
        };
        let job = match args.get("job") {
            Some(v) if !v.is_null() => Some(normalize(&py_str(v))),
            _ => None,
        };
        if let Some(j) = &job
            && j.chars().count() > MAX_JOB
        {
            return Err(ConversationError::JobTooLong);
        }
        Ok(prepared(
            &p.slug,
            Change::Rename(Rename { name, aliases, job }),
            "",
        ))
    }

    fn prepare_set_voice(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let voice = match args.get("voice") {
            Some(v) => voice_arg(Some(v), &self.voices)?,
            None => p.voice.clone(),
        };
        let speed = speed_arg(args.get("speed"), p.speed)?;
        Ok(prepared(
            &p.slug,
            Change::SetVoice(SetVoice { voice, speed }),
            "",
        ))
    }

    fn prepare_set_persona(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let text = args.get("text").map(py_str_or_empty).unwrap_or_default();
        let text = text.trim().to_string();
        if text.chars().count() > MAX_PERSONA {
            return Err(ConversationError::PersonaTooLong);
        }
        Ok(prepared(
            &p.slug,
            Change::SetPersona(SetPersona { text }),
            "",
        ))
    }

    fn prepare_remember(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let text = normalize(&args.get("text").map(py_str_or_empty).unwrap_or_default());
        if text.is_empty() {
            return Err(ConversationError::NothingToRemember);
        }
        if text.chars().count() > MAX_NOTE {
            return Err(ConversationError::NoteTooLong);
        }
        Ok(prepared(
            &p.slug,
            Change::AppendNote(AppendNote { text }),
            "",
        ))
    }

    fn prepare_forget(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let needle =
            normalize(&args.get("text").map(py_str_or_empty).unwrap_or_default()).to_lowercase();
        let lines = self.files.note_lines(&p.slug)?;
        let keep: Vec<String> = lines
            .iter()
            .filter(|ln| !needle.is_empty() && !ln.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        if keep.len() == lines.len() {
            return Err(ConversationError::NoNoteMatches);
        }
        let dropped = lines.len() - keep.len();
        Ok(prepared(
            &p.slug,
            Change::SetNotes(SetNotes {
                text: keep.join("\n"),
            }),
            &format!("forget {dropped} note(s)"),
        ))
    }

    fn prepare_compact(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        let text = args.get("summary").map(py_str_or_empty).unwrap_or_default();
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err(ConversationError::SummaryRequired);
        }
        Ok(prepared(
            &p.slug,
            Change::SetMemory(SetMemory {
                text,
                through_turn_id: self.last_turn_id(&p.slug)?,
                source: CompactionSource::Manual,
            }),
            "",
        ))
    }

    fn prepare_reset(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, false)?;
        Ok(prepared(&p.slug, Change::Reset, ""))
    }

    fn prepare_delete(&self, args: &Map<String, Value>) -> Result<Prepared> {
        let p = self.target(args, true)?;
        if p.is_ghost() {
            return Err(ConversationError::GhostCannotBeDeleted);
        }
        Ok(prepared(&p.slug, Change::Delete, ""))
    }

    /// The memory a new agent starts with when seeded from `seed` (a slug or a name):
    /// that agent's current `memory.md`. Ghost's memory stays private to Ghost (ADR
    /// 0014), so it can never be the seed.
    pub fn handoff_memory(&self, seed: &str) -> Result<String> {
        let p = match self.get(seed)? {
            Some(p) => Some(p),
            None => self.resolve(seed, true)?,
        };
        let p = p.ok_or_else(|| ConversationError::NoSeed(seed.into()))?;
        if p.is_ghost() {
            return Err(ConversationError::GhostMemoryPrivate);
        }
        Ok(self.files.memory(&p.slug)?)
    }

    /// The agent an operation is about: `conversation` by name, else the active one the
    /// caller put in `_active`.
    fn target(&self, args: &Map<String, Value>, include_retired: bool) -> Result<Profile> {
        if let Some(spoken) = args.get("conversation").filter(|v| truthy(v)) {
            let spoken = py_str(spoken);
            return self
                .resolve(&spoken, include_retired)?
                .ok_or(ConversationError::NoAgentCalled(spoken));
        }
        let slug = args
            .get("_active")
            .filter(|v| truthy(v))
            .map(py_str)
            .unwrap_or_else(|| GHOST.into());
        self.get(&slug)?.ok_or(ConversationError::NoAgent(slug))
    }
}

fn prepared(slug: &str, change: Change, rationale: &str) -> Prepared {
    Prepared {
        target: Target::Conversation(ConversationTarget { slug: slug.into() }),
        changes: ChangeSet::one(change),
        rationale: rationale.into(),
        asked_by: Vec::new(),
    }
}

/// `~` and `~/...` as Python's `expanduser`.
fn expanduser(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if (s == "~" || s.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(s.trim_start_matches("~/").trim_start_matches('~'));
    }
    p.to_path_buf()
}

/// Python's truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Bool(true) => true,
    }
}

/// `str(value or "")`.
fn py_str_or_empty(v: &Value) -> String {
    if truthy(v) { py_str(v) } else { String::new() }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// `data.get("aliases", [])` as strings.
fn aliases_of(data: &Map<String, Value>) -> Vec<String> {
    match data.get("aliases") {
        Some(Value::Array(items)) => items.iter().map(py_str).collect(),
        _ => Vec::new(),
    }
}

/// Python's `str.title()`.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphabetic() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// `" ".join(text.split())`.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `[a-z0-9]+` runs of a lower-cased text, filler words removed.
fn words(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            if !STOP.contains(&cur.as_str()) {
                out.insert(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    out
}

fn name_arg(value: Option<&Value>) -> Result<String> {
    let name = normalize(&value.map(py_str_or_empty).unwrap_or_default());
    if name.is_empty() {
        return Err(ConversationError::NameRequired);
    }
    if name.chars().count() > MAX_NAME {
        return Err(ConversationError::NameTooLong);
    }
    Ok(name)
}

fn aliases_arg(value: Option<&Value>) -> Result<Vec<String>> {
    let items: Vec<String> = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(s)) => s.split([',', ';']).map(String::from).collect(),
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        Some(_) => return Err(ConversationError::AliasesNotAList),
    };
    let mut out: Vec<String> = Vec::new();
    for v in items {
        let s = normalize(&v);
        if !s.is_empty() && !out.iter().any(|o| o.to_lowercase() == s.to_lowercase()) {
            out.push(s);
        }
    }
    out.truncate(12);
    Ok(out)
}

fn voice_arg(value: Option<&Value>, voices: &[String]) -> Result<Option<String>> {
    let v = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Ok(None),
        Some(v) => py_str(v).trim().to_string(),
    };
    if !voices.is_empty() && !voices.contains(&v) {
        return Err(ConversationError::NotInstalledVoice(v));
    }
    if !is_voice_id(&v) {
        return Err(ConversationError::NotAVoiceId(v));
    }
    Ok(Some(v))
}

/// `[a-z]{2}_[a-z0-9]+`, whole.
fn is_voice_id(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 4
        && b[0].is_ascii_lowercase()
        && b[1].is_ascii_lowercase()
        && b[2] == b'_'
        && b[3..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn speed_arg(value: Option<&Value>, default: Option<f64>) -> Result<Option<f64>> {
    let s = match value {
        None | Some(Value::Null) => return Ok(default),
        Some(Value::String(s)) if s.is_empty() => return Ok(default),
        Some(v) => as_f64(v).ok_or(ConversationError::SpeedNotANumber)?,
    };
    if !(0.5..=2.0).contains(&s) {
        return Err(ConversationError::SpeedOutOfRange);
    }
    Ok(Some((s * 100.0).round() / 100.0))
}

/// The agents' own read tools and operations as a tool source, so the gate, the policy
/// file and the audit treat them like any app's (ADR 0014, ADR 0034).
pub fn conversations_source(convs: &Arc<Conversations>) -> ToolSource {
    ToolSource {
        name: SOURCE.into(),
        prompt: String::new(),
        read_tools: convs.read_tools(),
        operations: convs.operations(),
        ..Default::default()
    }
}

/// A list argument the model may pass as a list or a comma-separated string.
pub fn names_arg(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(s) => Ok(s
            .split([',', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()),
        Value::Array(a) => Ok(a
            .iter()
            .map(py_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()),
        _ => Err(ConversationError::ExpectedNames),
    }
}

#[cfg(test)]
mod tests {
    //! Agents (ADR 0014, ADR 0035): the registry read from Ghost's agents/ folder, name
    //! resolution, resume context, and every operation through the gate.

    use super::*;
    use crate::gate::{Request, Submission, WriteGate};
    use crate::levels::Autonomy;
    use crate::models::{OutcomeStatus, Snapshot};
    use crate::policy::AutonomyPolicy;
    use crate::source::Writer;
    use tempfile::TempDir;

    const VOICES: [&str; 3] = ["af_heart", "am_adam", "bf_emma"];

    fn voices() -> Vec<String> {
        VOICES.iter().map(|s| s.to_string()).collect()
    }

    fn layout(tmp: &Path) -> PathBuf {
        let cfg = tmp.join("ghost");
        std::fs::create_dir_all(&cfg).unwrap();
        cfg
    }

    fn write(path: &Path, data: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string(&data).unwrap()).unwrap();
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn copytree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.path().is_dir() {
                copytree(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }

    fn convs_with(
        store: Arc<Store>,
        cfg: &Path,
        options: ConversationsOptions,
    ) -> Arc<Conversations> {
        Arc::new(Conversations::new(store, cfg, options).unwrap())
    }

    struct World {
        store: Arc<Store>,
        convs: Arc<Conversations>,
        gate: WriteGate,
        tmp: TempDir,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let convs = convs_with(
            store.clone(),
            &layout(tmp.path()),
            ConversationsOptions {
                default_voice: "af_heart".into(),
                voices: voices(),
                ..Default::default()
            },
        );
        let gate = WriteGate::new(store.clone()).with_autonomy(
            AutonomyPolicy::parse(
                &format!(
                    "[{SOURCE}]\nset_persona = 'announce'\nreset = 'announce'\n\
delete = 'announce'\nforget = 'announce'\n"
                ),
                "<test>",
            )
            .unwrap(),
        );
        gate.register(SOURCE, convs.operations());
        World {
            store,
            convs,
            gate,
            tmp,
        }
    }

    fn args<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    async fn run_as(
        gate: &WriteGate,
        op: &str,
        active: &str,
        mut a: Map<String, Value>,
    ) -> anyhow::Result<Submission> {
        let operation = gate.operation(SOURCE, op)?;
        a.insert("_active".into(), json!(active));
        let prepared = (operation.prepare)(a).await?;
        Ok(gate
            .submit(Request::new(
                "s1",
                SOURCE,
                op,
                prepared.target,
                prepared.changes,
                &prepared.rationale,
            ))
            .await?)
    }

    async fn run(gate: &WriteGate, op: &str, a: Map<String, Value>) -> anyhow::Result<Submission> {
        run_as(gate, op, GHOST, a).await
    }

    fn status(sub: &Submission) -> OutcomeStatus {
        sub.outcome.as_ref().unwrap().status
    }

    fn snap(sub: &Submission) -> &crate::models::ConversationSnapshot {
        match &sub.proposal.snapshot {
            Snapshot::Conversation(s) => s,
            _ => panic!("not an agent snapshot"),
        }
    }

    fn err_contains(r: anyhow::Result<Submission>, needle: &str) {
        let e = r.expect_err("expected an error").to_string();
        assert!(e.contains(needle), "{e:?} does not contain {needle:?}");
    }

    #[test]
    fn slugify_() {
        assert_eq!(slugify("Brand design (EMEA)"), "brand-design-emea");
        assert_eq!(slugify("  Legal  "), "legal");
        assert_eq!(slugify("!!!"), "agent");
    }

    #[test]
    fn ghost_is_agent_zero() {
        let w = world();
        let convs = &w.convs;
        let slugs: Vec<String> = convs
            .list(false)
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, vec![GHOST]);
        let ghost = convs.get(GHOST).unwrap().unwrap();
        assert!(ghost.name == "Ghost" && ghost.is_ghost() && ghost.voice.is_none());
        assert_eq!(
            convs.voice_for(&ghost).unwrap(),
            ("af_heart".to_string(), 1.0)
        );
        assert!(convs.files.dir(GHOST).join("agent.json").is_file());
        assert_eq!(
            convs.files.dir(GHOST),
            convs.config_dir.join("agents").join(GHOST)
        );
        // idempotent
        convs.sync().unwrap();
        assert_eq!(convs.list(false).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn hire_rename_resolve() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        let sub = run(
            gate,
            "create",
            args([
                ("name", json!("Brand")),
                ("aliases", json!(["the brand agent"])),
                ("job", json!("brand design")),
                ("persona", json!("Warm, visual, never salesy.")),
            ]),
        )
        .await
        .unwrap();
        assert!(sub.executed() && status(&sub) == OutcomeStatus::Success);
        let brand = convs.get("brand").unwrap().unwrap();
        assert_eq!(brand.job, "brand design");
        assert!(brand.voice.is_none() && convs.voice_for(&brand).unwrap().0 == "af_heart"); // Ghost's
        let folder = convs.config_dir.join("agents").join("brand"); // ADR 0035: flat, beside Ghost's
        assert!(
            folder.is_dir()
                && std::fs::read_to_string(folder.join("persona.md"))
                    .unwrap()
                    .starts_with("Warm")
        );
        let data = read_json(&folder.join("agent.json"));
        assert!(
            data["id"] == brand.id && data["job"] == "brand design" && data.get("voice").is_none()
        );

        assert_eq!(
            convs.resolve("brand", false).unwrap().unwrap().slug,
            "brand"
        );
        assert_eq!(
            convs
                .resolve("the brand agent", false)
                .unwrap()
                .unwrap()
                .slug,
            "brand"
        );
        assert_eq!(
            convs
                .resolve("transfer me to the brand agent", false)
                .unwrap()
                .unwrap()
                .slug,
            "brand"
        );
        assert!(convs.resolve("legal", false).unwrap().is_none());

        let sub = run(
            gate,
            "rename",
            args([
                ("conversation", json!("brand")),
                ("name", json!("Brand Studio")),
                ("aliases", json!(["studio"])),
                ("job", json!("the look")),
            ]),
        )
        .await
        .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        let again = convs.get("brand").unwrap().unwrap();
        assert!(again.name == "Brand Studio" && again.job == "the look"); // slug is stable
        assert_eq!(
            read_json(&folder.join("agent.json"))["name"],
            "Brand Studio"
        );
        assert_eq!(
            convs.resolve("studio people", false).unwrap().unwrap().slug,
            "brand"
        );
    }

    #[tokio::test]
    async fn an_agent_without_its_own_voice_speaks_in_ghosts() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(gate, "create", args([("name", json!("Brand"))]))
            .await
            .unwrap();
        run(
            gate,
            "set_voice",
            args([("voice", json!("am_adam")), ("speed", json!(1.2))]),
        )
        .await
        .unwrap(); // Ghost's
        let brand = convs.get("brand").unwrap().unwrap();
        assert_eq!(
            convs.voice_for(&brand).unwrap(),
            ("am_adam".to_string(), 1.2)
        );
        run(
            gate,
            "set_voice",
            args([
                ("conversation", json!("brand")),
                ("voice", json!("bf_emma")),
            ]),
        )
        .await
        .unwrap();
        let brand = convs.get("brand").unwrap().unwrap();
        assert_eq!(
            convs.voice_for(&brand).unwrap(),
            ("bf_emma".to_string(), 1.2)
        );
    }

    #[tokio::test]
    async fn hire_rejects_duplicates_and_bad_voices() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(gate, "create", args([("name", json!("Legal"))]))
            .await
            .unwrap();
        err_contains(
            run(gate, "create", args([("name", json!("legal"))])).await,
            "already an agent",
        );
        err_contains(
            run(gate, "create", args([("name", json!("Ghost"))])).await,
            "already an agent",
        );
        err_contains(
            run(
                gate,
                "create",
                args([("name", json!("Ops")), ("voice", json!("zz_nobody"))]),
            )
            .await,
            "not an installed voice",
        );
        assert!(!convs.operations().iter().any(|o| o.name == "move")); // ADR 0035
        assert!(
            !gate
                .operation(SOURCE, "create")
                .unwrap()
                .properties
                .as_object()
                .unwrap()
                .contains_key("department")
        );
    }

    #[tokio::test]
    async fn set_voice_and_persona() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        let sub = run(
            gate,
            "set_voice",
            args([("voice", json!("am_adam")), ("speed", json!(1.2))]),
        )
        .await
        .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        let ghost = convs.get(GHOST).unwrap().unwrap();
        assert!(ghost.voice.as_deref() == Some("am_adam") && ghost.speed == Some(1.2));
        assert_eq!(
            convs.voice_for(&ghost).unwrap(),
            ("am_adam".to_string(), 1.2)
        );
        let sub = run(gate, "set_voice", args([("voice", Value::Null)]))
            .await
            .unwrap(); // back to the default
        assert!(
            status(&sub) == OutcomeStatus::Success
                && convs.get(GHOST).unwrap().unwrap().voice.is_none()
        );
        let sub = run(
            gate,
            "set_persona",
            args([("text", json!("You are terse and dry."))]),
        )
        .await
        .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        assert_eq!(
            convs.files.persona(GHOST).unwrap().trim(),
            "You are terse and dry."
        );
    }

    #[tokio::test]
    async fn remember_and_forget() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(
            gate,
            "remember",
            args([("text", json!("the books close on the fifth"))]),
        )
        .await
        .unwrap();
        run(
            gate,
            "remember",
            args([("text", json!("Sam owns payroll"))]),
        )
        .await
        .unwrap();
        assert_eq!(
            convs.files.note_lines(GHOST).unwrap(),
            vec!["the books close on the fifth", "Sam owns payroll"]
        );
        let sub = run(gate, "forget", args([("text", json!("payroll"))]))
            .await
            .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        assert_eq!(
            convs.files.note_lines(GHOST).unwrap(),
            vec!["the books close on the fifth"]
        );
        assert!(
            run(gate, "forget", args([("text", json!("nothing like this"))]))
                .await
                .is_err()
        );
    }

    /// A request that opens a session follows a marked end, not the last goodbye: run
    /// together, "let's work on the wake word" got "talk soon" and a hang-up (2026-10-24).
    #[test]
    fn the_record_marks_where_each_session_ended() {
        let w = world();
        let convs = &w.convs;
        convs
            .record(GHOST, "s1", Role::User, "You can hang up now.")
            .unwrap();
        convs
            .record(
                GHOST,
                "s1",
                Role::Assistant,
                "Glad it's working, talk soon!",
            )
            .unwrap();
        convs
            .record(GHOST, "s2", Role::User, "What time is it?")
            .unwrap();
        let record = convs.resume(GHOST).unwrap().messages()[0].content.clone();
        assert!(record.ends_with(
            "Operator: You can hang up now.\nYou: Glad it's working, talk soon!\n\
(next session)\nOperator: What time is it?\n(end of the record)"
        ));
    }

    #[tokio::test]
    async fn turns_resume_compaction_and_prune() {
        let w = world();
        let (store, convs, gate) = (&w.store, &w.convs, &w.gate);
        convs
            .record(GHOST, "s1", Role::User, "what needs my sign-off")
            .unwrap();
        convs
            .record(GHOST, "s1", Role::Call, "list_issues(repo=acme/app)")
            .unwrap();
        convs
            .record(GHOST, "s1", Role::Assistant, "Two issues are waiting.")
            .unwrap();
        convs
            .record(GHOST, "s1", Role::User, "label the first one bug")
            .unwrap();
        let last = convs.last_turn_id(GHOST).unwrap();

        let ctx = convs.resume(GHOST).unwrap();
        let msgs = ctx.messages();
        // one record, from the user side: never replayed as the model's own turns
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user"]);
        let record = &msgs[0].content;
        assert!(record.starts_with(RECENT_TEMPLATE.split("{record}").next().unwrap()));
        assert!(record.contains("Operator: what needs my sign-off"));
        assert!(record.contains("(you called list_issues(repo=acme/app))"));
        assert!(record.contains("You: Two issues are waiting."));
        assert!(record.ends_with("Operator: label the first one bug\n(end of the record)"));
        // transcripts are keyed by the agent's id, not its slug
        assert_eq!(
            ctx.recent[0].conversation,
            convs.get(GHOST).unwrap().unwrap().id
        );

        // an automatic compaction through the internal path
        let out = gate
            .apply_internal(Request::new(
                "s1",
                SOURCE,
                "compact",
                Target::Conversation(ConversationTarget { slug: GHOST.into() }),
                ChangeSet::one(Change::SetMemory(SetMemory {
                    text: "Operator reviewed two issues.".into(),
                    through_turn_id: last,
                    source: CompactionSource::Auto,
                })),
                "auto",
            ))
            .await
            .unwrap();
        assert_eq!(out.status, OutcomeStatus::Success);
        assert_eq!(
            convs.files.memory(GHOST).unwrap().trim(),
            "Operator reviewed two issues."
        );
        let versions = convs.compactions(GHOST).unwrap();
        assert!(versions.len() == 1 && versions[0].through_turn_id == last);
        let ctx = convs.resume(GHOST).unwrap();
        assert!(ctx.recent.is_empty()); // all covered
        assert_eq!(
            ctx.messages()[0].content,
            SUMMARY_TEMPLATE.replace("{summary}", "Operator reviewed two issues.")
        );
        // the operator edits the file by hand; the next resume reads the edit
        convs
            .files
            .write(
                GHOST,
                "memory.md",
                "Operator reviewed two issues. Sam owes a reply.",
            )
            .unwrap();
        assert!(convs.resume(GHOST).unwrap().memory.contains("Sam owes"));

        // pruning drops only summarized turns older than the window
        assert_eq!(store.prune_turns(utcnow()).unwrap(), 4);
        assert!(convs.turns(GHOST, 200).unwrap().is_empty());
    }

    #[tokio::test]
    async fn reset_hides_turns_and_keeps_the_before_in_the_audit() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        convs.record(GHOST, "s1", Role::User, "hello").unwrap();
        run(gate, "remember", args([("text", json!("keep this"))]))
            .await
            .unwrap();
        convs
            .files
            .write(GHOST, "memory.md", "some summary")
            .unwrap();
        let sub = run(gate, "reset", args([])).await.unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        assert!(
            convs.turns(GHOST, 200).unwrap().is_empty() && convs.files.memory(GHOST).unwrap() == ""
        );
        assert!(convs.files.note_lines(GHOST).unwrap().is_empty());
        let before = snap(&sub);
        assert!(before.memory.trim() == "some summary" && before.notes.contains("keep this"));
    }

    #[tokio::test]
    async fn delete_removes_the_folder_and_hides_the_agent() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(
            gate,
            "create",
            args([("name", json!("Legal")), ("voice", json!("bf_emma"))]),
        )
        .await
        .unwrap();
        run(
            gate,
            "set_persona",
            args([
                ("conversation", json!("legal")),
                ("text", json!("Careful and formal.")),
            ]),
        )
        .await
        .unwrap();
        convs
            .record("legal", "s1", Role::User, "is this contract okay")
            .unwrap();
        let folder = convs.files.dir("legal");
        let sub = run(gate, "delete", args([("conversation", json!("legal"))]))
            .await
            .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        assert!(
            convs.get("legal").unwrap().is_none()
                && convs.resolve("legal", false).unwrap().is_none()
        );
        assert!(!folder.exists());
        let before = snap(&sub);
        assert!(
            before.voice.as_deref() == Some("bf_emma")
                && before.persona.trim() == "Careful and formal."
        );
    }

    #[tokio::test]
    async fn ghost_cannot_be_deleted_retired_or_renamed() {
        let w = world();
        let gate = &w.gate;
        err_contains(run(gate, "delete", args([])).await, "cannot be deleted");
        err_contains(run(gate, "retire", args([])).await, "stays");
        let sub = run(gate, "rename", args([("name", json!("Spectre"))]))
            .await
            .unwrap();
        let outcome = sub.outcome.as_ref().unwrap();
        assert!(
            outcome.status == OutcomeStatus::Failed
                && outcome
                    .error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("keeps its name")
        );
    }

    #[test]
    fn retention_window_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let convs = convs_with(
            store,
            &layout(tmp.path()),
            ConversationsOptions {
                retention: Duration::days(1),
                ..Default::default()
            },
        );
        convs.record(GHOST, "s", Role::User, "fresh").unwrap();
        assert_eq!(convs.prune().unwrap(), 0); // nothing summarized yet, nothing old
    }

    // --- the folders are the registry ---------------------------------------------------

    #[test]
    fn sync_reads_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = layout(tmp.path());
        let store = Arc::new(Store::in_memory().unwrap());
        let convs = convs_with(
            store,
            &cfg,
            ConversationsOptions {
                voices: voices(),
                ..Default::default()
            },
        );
        // the operator makes an agent by hand: a folder with an agent.json
        let folder = cfg.join("agents").join("brand");
        write(
            &folder.join("agent.json"),
            json!({"name": "Brand", "job": "the look"}),
        );
        std::fs::create_dir(cfg.join("agents").join("junk")).unwrap(); // no agent.json: not an agent
        let problems = convs.sync().unwrap();
        let brand = convs.get("brand").unwrap();
        assert!(brand.as_ref().is_some_and(|b| b.job == "the look"));
        assert_eq!(
            read_json(&folder.join("agent.json"))["id"],
            brand.as_ref().unwrap().id
        ); // given an id
        assert!(problems.iter().any(|p| p.contains("junk")));
        // gone from disk: marked deleted, its history hidden from the roster
        std::fs::remove_file(folder.join("agent.json")).unwrap();
        std::fs::remove_dir(&folder).unwrap();
        convs.sync().unwrap();
        assert!(convs.get("brand").unwrap().is_none());
    }

    #[test]
    fn agent_folders_keep_their_rows_when_copied_or_edited() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = layout(tmp.path());
        write(
            &cfg.join("agents").join("brand").join("agent.json"),
            json!({"name": "Brand"}),
        );
        write(
            &cfg.join("agents").join("Loud").join("agent.json"),
            json!({"name": "Loud"}),
        );
        let db = tmp.path().join("g.db");
        let convs = convs_with(
            Arc::new(Store::open(&db).unwrap()),
            &cfg,
            ConversationsOptions::default(),
        );
        let brand = convs.get("brand").unwrap().unwrap();
        assert!(convs.get("loud").unwrap().is_none()); // not lower-case: not read
        convs
            .record("brand", "s", Role::User, "hello brand")
            .unwrap();
        copytree(
            &cfg.join("agents").join("brand"),
            &cfg.join("agents").join("brand2"),
        ); // cp -r
        convs.sync().unwrap();
        assert!(
            convs.get("brand").unwrap().unwrap().id == brand.id
                && convs.turns("brand", 200).unwrap().len() == 1
        );
        let copy = convs.get("brand2").unwrap();
        assert!(copy.as_ref().is_some_and(|c| c.id != brand.id));
        assert!(convs.turns("brand2", 200).unwrap().is_empty());
        let mut data = read_json(&cfg.join("agents").join("brand").join("agent.json"));
        data["id"] = json!("a-different-id");
        write(&cfg.join("agents").join("brand").join("agent.json"), data);
        drop(convs);
        let again = convs_with(
            Arc::new(Store::open(&db).unwrap()),
            &cfg,
            ConversationsOptions::default(),
        ); // a restart with the edited file
        assert_eq!(again.get("brand").unwrap().unwrap().id, brand.id);
        assert_eq!(
            read_json(&cfg.join("agents").join("brand").join("agent.json"))["id"],
            brand.id
        );
    }

    #[test]
    fn a_folder_made_again_under_an_old_slug_takes_its_row_back() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = layout(tmp.path());
        write(
            &cfg.join("agents").join("brand").join("agent.json"),
            json!({"name": "Brand"}),
        );
        let db = tmp.path().join("g.db");
        let open = || {
            convs_with(
                Arc::new(Store::open(&db).unwrap()),
                &cfg,
                ConversationsOptions::default(),
            )
        };
        let convs = open();
        let ghost_id = convs.get(GHOST).unwrap().unwrap().id;
        let brand_id = convs.get("brand").unwrap().unwrap().id;
        drop(convs);
        std::fs::remove_dir_all(cfg.join("agents")).unwrap();
        let convs = open(); // restart: brand marked deleted, Ghost made again
        assert!(
            convs.get("brand").unwrap().is_none()
                && convs.get(GHOST).unwrap().unwrap().id == ghost_id
        );
        drop(convs);
        write(
            &cfg.join("agents").join("brand").join("agent.json"),
            json!({"name": "Brand"}),
        );
        let convs = open(); // restart again: no collision
        assert_eq!(convs.get("brand").unwrap().unwrap().id, brand_id);
    }

    /// ADR 0035: agents hired into a department under departments/ are not read; the
    /// operator hires them again (`ghostd check` names the folder).
    #[test]
    fn an_old_department_folder_is_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = layout(tmp.path());
        write(
            &cfg.join("departments")
                .join("marketing")
                .join("agents")
                .join("brand")
                .join("agent.json"),
            json!({"name": "B"}),
        );
        let convs = convs_with(
            Arc::new(Store::in_memory().unwrap()),
            &cfg,
            ConversationsOptions::default(),
        );
        let slugs: Vec<String> = convs
            .list(false)
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, vec![GHOST]);
    }

    #[tokio::test]
    async fn a_reopened_store_keeps_timestamps_and_retired_state() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ghost.db");
        let cfg = layout(tmp.path());
        let options = || ConversationsOptions {
            voices: voices(),
            ..Default::default()
        };
        let convs = convs_with(Arc::new(Store::open(&db).unwrap()), &cfg, options());
        let ops: BTreeMap<String, Operation> = convs
            .operations()
            .into_iter()
            .map(|o| (o.name.clone(), o))
            .collect();

        let prep = (ops["create"].prepare)(args([("name", json!("Brand"))]))
            .await
            .unwrap();
        convs
            .writer
            .apply(&prep.target, &prep.changes.steps()[0], "k")
            .await
            .unwrap();
        let prep = (ops["retire"].prepare)(args([("conversation", json!("brand"))]))
            .await
            .unwrap();
        convs
            .writer
            .apply(&prep.target, &prep.changes.steps()[0], "k")
            .await
            .unwrap();
        assert!(convs.get("brand").unwrap().unwrap().retired());
        drop(ops);
        drop(convs);
        let again = convs_with(Arc::new(Store::open(&db).unwrap()), &cfg, options());
        assert!(
            again.get("brand").unwrap().unwrap().retired()
                && again.resolve("brand", false).unwrap().is_none()
        );
        let slugs: Vec<String> = again
            .list(true)
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, vec![GHOST, "brand"]);
    }

    #[test]
    fn a_pre_0024_database_is_moved_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ghost.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE conversations (slug TEXT PRIMARY KEY, name TEXT NOT NULL, voice TEXT);\
                 CREATE TABLE turns (id INTEGER PRIMARY KEY, conversation TEXT);\
                 CREATE TABLE compactions (id TEXT PRIMARY KEY, conversation TEXT);\
                 INSERT INTO conversations VALUES ('ghost', 'Ghost', 'af_heart');",
            )
            .unwrap();
        }
        let store = Store::open(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let tables: Vec<String> = st
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            tables
                .iter()
                .any(|t| t.starts_with("retired_conversations_"))
        );
        assert!(store.list_conversations(false, false).unwrap().is_empty()); // the new layout starts empty
    }

    // --- retire, rehire and the handoff (ADR 0019) ----------------------------------------

    #[tokio::test]
    async fn retire_hides_from_resolve_and_the_default_list_but_stays_readable() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(
            gate,
            "create",
            args([("name", json!("Heka New")), ("voice", json!("bf_emma"))]),
        )
        .await
        .unwrap();
        convs
            .record("heka-new", "s1", Role::User, "we shipped the thing")
            .unwrap();
        convs
            .files
            .write("heka-new", "memory.md", "Shipped v1; v2 planning next.")
            .unwrap();
        err_contains(run(gate, "retire", args([])).await, "stays");
        let sub = run(gate, "retire", args([("conversation", json!("heka new"))]))
            .await
            .unwrap();
        assert!(status(&sub) == OutcomeStatus::Success && sub.autonomy == Autonomy::Announce);
        let p = convs.get("heka-new").unwrap().unwrap();
        assert!(p.retired() && p.retired_at.is_some());
        assert!(convs.resolve("heka new", false).unwrap().is_none()); // out of voice routing
        let slugs = |include_retired: bool| -> Vec<String> {
            convs
                .list(include_retired)
                .unwrap()
                .into_iter()
                .map(|q| q.slug)
                .collect()
        };
        assert_eq!(slugs(false), vec![GHOST]);
        assert_eq!(slugs(true), vec![GHOST, "heka-new"]);
        assert_eq!(
            convs.turns("heka-new", 200).unwrap()[0].text,
            "we shipped the thing"
        ); // readable
        assert_eq!(
            convs.files.memory("heka-new").unwrap().trim(),
            "Shipped v1; v2 planning next."
        );
        err_contains(
            run(gate, "retire", args([("conversation", json!("heka new"))])).await,
            "no agent called",
        ); // retired: not routable

        // a new agent for the same work is seeded from the retired one's memory
        let sub = run(
            gate,
            "create",
            args([
                ("name", json!("Heka Workshop")),
                ("seed_from", json!("heka-new")),
            ]),
        )
        .await
        .unwrap();
        assert!(
            status(&sub) == OutcomeStatus::Success
                && sub.proposal.changes.describe().contains("handoff")
        );
        assert_eq!(
            convs.files.memory("heka-workshop").unwrap().trim(),
            "Shipped v1; v2 planning next."
        );
        assert_eq!(
            convs.compactions("heka-workshop").unwrap()[0].source,
            CompactionSource::Manual
        );
        err_contains(
            run(
                gate,
                "create",
                args([("name", json!("Nope")), ("seed_from", json!(GHOST))]),
            )
            .await,
            "private",
        );
        err_contains(
            run(
                gate,
                "create",
                args([("name", json!("Nope")), ("seed_from", json!("zzz"))]),
            )
            .await,
            "no agent",
        );

        // and it can come back; rehire finds it by name even though resolve() would not
        let sub = run(gate, "rehire", args([("conversation", json!("heka new"))]))
            .await
            .unwrap();
        assert_eq!(status(&sub), OutcomeStatus::Success);
        assert_eq!(
            convs.resolve("heka new", false).unwrap().unwrap().slug,
            "heka-new"
        );
        err_contains(
            run(gate, "rehire", args([("conversation", json!("heka new"))])).await,
            "not retired",
        );
    }

    #[tokio::test]
    async fn unused_voice_prefers_one_nobody_uses() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(
            gate,
            "create",
            args([("name", json!("Brand")), ("voice", json!("am_adam"))]),
        )
        .await
        .unwrap();
        assert_eq!(convs.unused_voice().unwrap(), "bf_emma"); // the default and Brand's are taken
        let bare = convs_with(
            Arc::new(Store::in_memory().unwrap()),
            &w.tmp.path().join("bare"),
            ConversationsOptions {
                default_voice: "af_heart".into(),
                ..Default::default()
            },
        );
        assert_eq!(bare.unused_voice().unwrap(), "af_heart"); // no voice list: the default
    }

    #[tokio::test]
    async fn list_agents_tool_lists_agents_and_hides_retired() {
        let w = world();
        let (convs, gate) = (&w.convs, &w.gate);
        run(
            gate,
            "create",
            args([("name", json!("Brand")), ("job", json!("the look"))]),
        )
        .await
        .unwrap();
        run(gate, "create", args([("name", json!("Old"))]))
            .await
            .unwrap();
        run(gate, "retire", args([("conversation", json!("old"))]))
            .await
            .unwrap();
        let tool = convs.read_tools().remove(0);
        let out = (tool.handler)(Map::new()).await.unwrap();
        let listed: Vec<(String, Value)> = out["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["slug"].as_str().unwrap().to_string(), a["job"].clone()))
            .collect();
        assert_eq!(
            listed,
            vec![
                (GHOST.to_string(), Value::Null),
                ("brand".to_string(), json!("the look")),
            ]
        );
    }
}
