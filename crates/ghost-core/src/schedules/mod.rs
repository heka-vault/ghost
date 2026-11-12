//! Schedules (ADR 0047): agents that work while the operator is away.
//!
//! A **schedule** is one job: an agent, an instruction, and when (`at` a time, `every`
//! interval, or a `cron` expression in a time zone). A **run** is one execution of it: the
//! agent's prompt adapted for writing, the same tools through the same gate, no microphone
//! and no speaker. A run also starts when the operator asks for one now, and when a watch
//! declares the `act` response (ADR 0029).
//!
//! This module is the part with no model in it: the job model and its SQLite rows
//! ([`Jobs`]), the time arithmetic ([`when`]), and the [`Scheduler`] that claims what is
//! due, runs one job at a time per agent through an [`AgentRunner`], and delivers the
//! result as an item on the waiting list under `agent:<slug>`. The runner itself is
//! `ghost_voice::headless`, which the daemon hands in.
//!
//! The rules that hold here as everywhere: every change a run makes goes through the gate;
//! a change that would ask has nobody to answer and is set aside on the waiting list (ADR
//! 0030); what an app returns is data; nothing here touches the microphone.
//!
//! The ideas are OpenClaw's cron jobs and Hermes Agent's (both MIT): the three schedule
//! kinds, a run recorded before it starts so a restart neither runs nor delivers it twice,
//! one catch-up run inside a grace window and never a backlog, `[SILENT]` to say nothing,
//! and a cheap check hashed first so the model runs only when something changed.

pub mod jobs;
pub mod scheduler;
#[cfg(test)]
mod tests;
pub mod when;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

pub use jobs::{JobEdit, Jobs, ScheduleError};
pub use scheduler::{Scheduler, SchedulerParts};
pub use when::{ActiveHours, Schedule, Written};

/// Who made a schedule when it was the operator (the desktop); an agent is `agent:<slug>`.
pub const OPERATOR: &str = "operator";
/// A run whose final answer is exactly this delivers nothing (Hermes Agent's marker).
pub const SILENT: &str = "[SILENT]";
/// How a run's session id starts: what tells its turns and its changes from a session's.
pub const RUN_SESSION: &str = "run-";
/// How a run's turns start in the agent's transcript.
pub const RUN_MARK: &str = "[scheduled run";

pub const DEFAULT_GRACE_SECS: u64 = 2 * 3600;
pub const DEFAULT_MAX_RUNTIME_SECS: u64 = 600;
pub const MAX_RUNTIME_SECS: u64 = 3600;
pub const DEFAULT_MAX_TURNS: u32 = 30;
pub const MAX_TURNS: u32 = 100;
pub const NAME_MAX: usize = 80;
pub const INSTRUCTION_MAX: usize = 4000;
pub const MAX_JOBS: usize = 200;
/// A run's answer as the waiting list carries it; the whole of it is in the transcript.
pub const DETAIL_MAX: usize = 2000;
/// What the transcript and the runs keep of an answer.
pub const ANSWER_MAX: usize = 16_000;

/// A run's session id.
pub fn run_session(run_id: &str) -> String {
    format!("{RUN_SESSION}{run_id}")
}

/// Whether a session id is a run's.
pub fn is_run_session(session_id: &str) -> bool {
    session_id.starts_with(RUN_SESSION)
}

/// The line a run's turn opens with in the transcript: `[scheduled run: morning check]`.
pub fn run_mark(name: &str) -> String {
    format!("{RUN_MARK}: {name}]")
}

/// `agent:<slug>`, as `created_by` holds an agent.
pub fn by_agent(slug: &str) -> String {
    format!("agent:{slug}")
}

/// A cheap look that runs first, with no model (monitor mode): one of a connected app's
/// read-only tools. The agent runs only when what it returns has changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub app: String,
    pub read: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

/// What a schedule carries besides when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobOptions {
    /// The zone its active hours and a written time are read in.
    pub tz: String,
    pub active_hours: Option<ActiveHours>,
    /// A run missed by less than this is run once when Ghost is back; later, not at all.
    pub grace_secs: u64,
    pub max_runtime_secs: u64,
    pub max_turns: u32,
    /// A one-off is removed once it has run.
    pub delete_after_run: bool,
    /// Skills whose text the run starts with.
    pub skills: Vec<String>,
    /// How loud the result may get: digest, notify or call (ADR 0031).
    pub urgency: String,
    /// Whether a run that fails tells the operator.
    pub notify_failure: bool,
    pub check: Option<Check>,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            tz: "UTC".into(),
            active_hours: None,
            grace_secs: DEFAULT_GRACE_SECS,
            max_runtime_secs: DEFAULT_MAX_RUNTIME_SECS,
            max_turns: DEFAULT_MAX_TURNS,
            delete_after_run: false,
            skills: Vec::new(),
            urgency: "notify".into(),
            notify_failure: true,
            check: None,
        }
    }
}

/// One schedule, as its row holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub name: String,
    /// The agent's slug.
    pub agent: String,
    pub instruction: String,
    pub schedule: Schedule,
    pub options: JobOptions,
    pub enabled: bool,
    /// `operator`, `agent:<slug>`, or `client:<name>` for an outside tool (ADR 0048).
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<String>,
    pub last_error: Option<String>,
    /// What the check returned last time, hashed.
    pub check_hash: Option<String>,
}

impl Job {
    /// Whether `agent` may change it: its own (it runs as that agent, or that agent made
    /// it), and Ghost may change any.
    pub fn managed_by(&self, agent: &str) -> bool {
        agent == crate::conversations::GHOST
            || self.agent == agent
            || self.created_by == by_agent(agent)
    }
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Recorded, waiting for its agent to be free.
    Queued,
    Running,
    Ok,
    /// Monitor mode: the check returned what it returned last time; no model was called.
    Unchanged,
    Failed,
    TimedOut,
    Cancelled,
    /// Ghost stopped while it ran. It is not run again.
    Interrupted,
    /// Ghost was not running at its time, and came back after the grace window.
    Missed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Ok => "ok",
            RunStatus::Unchanged => "unchanged",
            RunStatus::Failed => "failed",
            RunStatus::TimedOut => "timed_out",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Interrupted => "interrupted",
            RunStatus::Missed => "missed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "ok" => RunStatus::Ok,
            "unchanged" => RunStatus::Unchanged,
            "failed" => RunStatus::Failed,
            "timed_out" => RunStatus::TimedOut,
            "cancelled" => RunStatus::Cancelled,
            "interrupted" => RunStatus::Interrupted,
            "missed" => RunStatus::Missed,
            _ => return None,
        })
    }

    pub fn finished(self) -> bool {
        !matches!(self, RunStatus::Queued | RunStatus::Running)
    }

    /// A run that ended without doing its work.
    pub fn failed(self) -> bool {
        matches!(
            self,
            RunStatus::Failed | RunStatus::TimedOut | RunStatus::Interrupted | RunStatus::Missed
        )
    }
}

/// What came of a run's result.
pub mod delivery {
    /// On the waiting list.
    pub const DELIVERED: &str = "delivered";
    /// The answer was `[SILENT]`.
    pub const SILENT: &str = "silent";
    /// The answer was empty.
    pub const EMPTY: &str = "empty";
    /// The agent told the operator itself with `ghost_notify`.
    pub const BY_AGENT: &str = "by_agent";
    /// A failure, on the waiting list.
    pub const FAILURE: &str = "failure";
    /// A failure, with telling switched off.
    pub const FAILURE_QUIET: &str = "failure_not_told";
    /// Nothing to deliver (unchanged, cancelled).
    pub const NONE: &str = "none";
}

/// What a run was started for.
pub mod source {
    pub const SCHEDULE: &str = "schedule";
    /// The operator, or an agent, asked for it now.
    pub const RUN_NOW: &str = "run_now";
    /// `watch:<name>`: a watch's `act`.
    pub const WATCH: &str = "watch:";
}

/// What a run is given, kept with its row so a queued run survives a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSpec {
    pub instruction: String,
    /// Typed fields, as data: a watch's item, never a raw body.
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub skills: Vec<String>,
    pub max_turns: u32,
    pub max_runtime_secs: u64,
    pub urgency: String,
    pub notify_failure: bool,
}

/// One run, as its row holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: String,
    /// The schedule's id; None for a watch's act.
    pub job: Option<String>,
    pub source: String,
    pub item_key: Option<String>,
    pub agent: String,
    pub name: String,
    pub spec: RunSpec,
    pub status: RunStatus,
    pub scheduled_for: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub delivery: Option<String>,
    /// Model calls tried again after the model could not be reached.
    pub attempts: u32,
    pub model_calls: u32,
    pub tool_calls: u32,
}

impl Run {
    pub fn session_id(&self) -> String {
        run_session(&self.id)
    }
}

/// One run, as the runner is asked for it.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub run_id: String,
    pub agent: String,
    /// The schedule's name, or the watch's.
    pub name: String,
    pub instruction: String,
    /// Typed fields given as data.
    pub data: Option<Value>,
    /// What a check found changed, when the schedule has one.
    pub changed: Option<String>,
    pub skills: Vec<String>,
    pub max_turns: u32,
    pub max_runtime: Duration,
    pub cancel: CancellationToken,
    /// Tools by the name the model sees that this run is not offered and may not call:
    /// a follow-up to an answer is not offered `ghost_answer` or `ghost_ask` (ADR 0051).
    pub withheld: Vec<String>,
}

/// How a run ended, as the runner saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The model stopped calling tools: `answer` is its last words.
    Done,
    /// The model could not be reached or broke off; worth trying again.
    Transient(String),
    /// It cannot work as asked (no such agent, a refused request).
    Failed(String),
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub ended: Ended,
    pub answer: String,
    pub model_calls: u32,
    pub tool_calls: u32,
    /// Model calls tried again after the model could not be reached.
    pub retries: u32,
    /// The agent told the operator itself (`ghost_notify`) during the run.
    pub notified: bool,
    /// Changes that needed a yes and were set aside on the waiting list (ADR 0030).
    pub set_aside: Vec<String>,
}

impl RunOutcome {
    pub fn ended(ended: Ended) -> Self {
        Self {
            ended,
            answer: String::new(),
            model_calls: 0,
            tool_calls: 0,
            retries: 0,
            notified: false,
            set_aside: Vec::new(),
        }
    }
}

/// What runs an agent with nobody there: `ghost_voice::headless::HeadlessRunner` in the
/// daemon, a fake in this crate's tests.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(&self, request: RunRequest) -> RunOutcome;
}

/// Text cut to `max` characters, with an ellipsis when it was longer.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}
