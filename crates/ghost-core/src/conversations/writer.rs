//! Applies agent changes: the folder under `agents/` plus the mirror row in SQLite.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::conversations::files::{ConversationFiles, MEMORY, NOTES, PERSONA};
use crate::conversations::models::{Compaction, CompactionSource, GHOST, Profile, new_id};
use crate::models::{Change, ConversationSnapshot, ConversationState, Snapshot, Target, py_repr};
use crate::source::Writer;
use crate::store::{Store, StoreError};
use crate::time::utcnow;

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("{} already exists", py_repr(.0))]
    AlreadyExists(String),
    #[error("{} already exists; pick another name", .0.display())]
    FolderExists(PathBuf),
    #[error("Ghost stays")]
    GhostStays,
    #[error("Ghost keeps its name")]
    GhostKeepsItsName,
    #[error("no agent {}", py_repr(.0))]
    NoAgent(String),
    #[error("ConversationWriter cannot apply {0}")]
    CannotApply(String),
    #[error("not an agent: {0}")]
    NotAnAgent(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct ConversationWriter {
    store: Arc<Store>,
    files: Arc<ConversationFiles>,
    agents_dir: PathBuf,
}

impl ConversationWriter {
    /// `agents_dir` is Ghost's `agents/` folder, where every agent lives.
    pub fn new(store: Arc<Store>, files: Arc<ConversationFiles>, agents_dir: PathBuf) -> Self {
        Self {
            store,
            files,
            agents_dir,
        }
    }

    pub fn allowed(target: &Target) -> bool {
        matches!(target, Target::Conversation(t) if !t.slug.is_empty())
    }

    fn slug_of(target: &Target) -> Result<&str, WriterError> {
        match target {
            Target::Conversation(t) => Ok(&t.slug),
            other => Err(WriterError::NotAnAgent(other.spoken())),
        }
    }

    fn apply_change(
        &self,
        slug: &str,
        change: &Change,
        idempotency_key: &str,
    ) -> Result<(), WriterError> {
        let store = &self.store;
        let files = &self.files;
        match change {
            Change::Create(c) => {
                if store.get_conversation(slug, true)?.is_some() {
                    return Err(WriterError::AlreadyExists(c.name.clone()));
                }
                let folder = self.agents_dir.join(slug);
                if folder.exists() {
                    return Err(WriterError::FolderExists(folder));
                }
                let mut p = Profile::new(slug, c.name.clone());
                p.aliases = c.aliases.clone();
                p.job = c.job.clone();
                p.voice = c.voice.clone();
                p.speed = c.speed;
                store.upsert_conversation(&p)?;
                files.write_agent(slug, &agent_map(&p))?;
                files.write(slug, PERSONA, &c.persona)?;
                if !c.memory.trim().is_empty() {
                    files.write(slug, MEMORY, &c.memory)?;
                    store.add_compaction(&Compaction {
                        id: new_id(),
                        conversation: p.id.clone(),
                        summary: c.memory.clone(),
                        through_turn_id: None,
                        at: utcnow(),
                        source: CompactionSource::Manual,
                    })?;
                }
            }
            Change::Retire => {
                let p = self.live(slug)?;
                if p.is_ghost() {
                    return Err(WriterError::GhostStays);
                }
                if !p.retired() {
                    let now = utcnow();
                    let mut q = p;
                    q.retired_at = Some(now);
                    q.updated_at = now;
                    self.save(&q)?;
                }
            }
            Change::Rehire => {
                let p = self.live(slug)?;
                if p.retired() {
                    let mut q = p;
                    q.retired_at = None;
                    q.updated_at = utcnow();
                    self.save(&q)?;
                }
            }
            Change::Rename(c) => {
                if slug == GHOST {
                    return Err(WriterError::GhostKeepsItsName);
                }
                let mut q = self.live(slug)?;
                q.name = c.name.clone();
                q.aliases = c.aliases.clone();
                if let Some(job) = &c.job {
                    q.job = job.clone();
                }
                q.updated_at = utcnow();
                self.save(&q)?;
            }
            Change::SetVoice(c) => {
                let mut q = self.live(slug)?;
                q.voice = c.voice.clone();
                q.speed = c.speed;
                q.updated_at = utcnow();
                self.save(&q)?;
            }
            Change::SetPersona(c) => {
                self.live(slug)?;
                files.write(slug, PERSONA, &c.text)?;
            }
            Change::AppendNote(c) => {
                self.live(slug)?;
                let current = files.notes(slug)?;
                let line = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let head = if current.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}\n", current.trim_end_matches('\n'))
                };
                files.write(slug, NOTES, &format!("{head}{line}"))?;
            }
            Change::SetNotes(c) => {
                self.live(slug)?;
                files.write(slug, NOTES, &c.text)?;
            }
            Change::SetMemory(c) => {
                let p = self.live(slug)?;
                files.write(slug, MEMORY, &c.text)?;
                let cid = new_id();
                store.add_compaction(&Compaction {
                    id: cid.clone(),
                    conversation: p.id.clone(),
                    summary: c.text.clone(),
                    through_turn_id: c.through_turn_id,
                    at: utcnow(),
                    source: c.source,
                })?;
                if let Some(through) = c.through_turn_id {
                    store.mark_compacted(&p.id, through, &cid)?;
                }
            }
            Change::Reset => {
                let p = self.live(slug)?;
                files.write(slug, MEMORY, "")?;
                files.write(slug, NOTES, "")?;
                store.tag_turns(&p.id, idempotency_key)?;
            }
            Change::Delete => {
                let p = self.live(slug)?;
                if p.is_ghost() {
                    return Err(WriterError::GhostStays);
                }
                let folder = files.dir(slug);
                let mut q = p.clone();
                q.deleted_at = Some(utcnow());
                store.upsert_conversation(&q)?;
                store.tag_turns(&p.id, idempotency_key)?;
                if folder.is_dir() {
                    std::fs::remove_dir_all(folder)?;
                }
            }
            other => return Err(WriterError::CannotApply(change_name(other))),
        }
        Ok(())
    }

    /// Identity changes land in both places: the row and the folder's agent.json.
    fn save(&self, p: &Profile) -> Result<(), WriterError> {
        self.store.upsert_conversation(p)?;
        self.files.write_agent(&p.slug, &agent_map(p))?;
        Ok(())
    }

    fn live(&self, slug: &str) -> Result<Profile, WriterError> {
        self.store
            .get_conversation(slug, false)?
            .ok_or_else(|| WriterError::NoAgent(slug.to_string()))
    }
}

/// `Profile::agent_json` as the map `write_agent` takes.
pub(crate) fn agent_map(p: &Profile) -> serde_json::Map<String, serde_json::Value> {
    match p.agent_json() {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

/// The Python class name of a change this writer does not apply.
fn change_name(c: &Change) -> String {
    let dbg = format!("{c:?}");
    dbg.split(|ch: char| !ch.is_alphanumeric())
        .next()
        .unwrap_or("Change")
        .to_string()
}

#[async_trait]
impl Writer for ConversationWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        let slug = Self::slug_of(target)?;
        let Some(p) = self.store.get_conversation(slug, true)? else {
            return Ok(Snapshot::Conversation(ConversationSnapshot::missing()));
        };
        let deleted = p.deleted_at.is_some();
        let (persona, memory, notes) = if deleted {
            (String::new(), String::new(), String::new())
        } else {
            (
                self.files.persona(&p.slug)?,
                self.files.memory(&p.slug)?,
                self.files.notes(&p.slug)?,
            )
        };
        Ok(Snapshot::Conversation(ConversationSnapshot::of(
            ConversationState {
                name: &p.name,
                aliases: &p.aliases,
                job: &p.job,
                voice: p.voice.as_deref(),
                speed: p.speed,
                persona: &persona,
                memory: &memory,
                notes: &notes,
                turns: self.store.turn_count(&p.id)?,
                deleted,
                retired: p.retired(),
            },
        )))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let slug = Self::slug_of(target)?;
        self.apply_change(slug, change, idempotency_key)?;
        Ok(())
    }
}
