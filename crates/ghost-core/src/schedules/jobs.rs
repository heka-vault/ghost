//! The schedules and their runs, in SQLite (`schedules`, `schedule_runs`).
//!
//! Everything that decides whether a run happens is here and takes the time as an
//! argument: what is due, what was missed, what a restart finds half done, and how often a
//! watch may act. A run's row is written before the run starts and the schedule's next
//! time is moved in the same step, so nothing is run twice.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;
use tokio::sync::Notify;
use tracing::warn;

use super::when::{self, ActiveHours, Schedule, Written};
use super::{
    Check, DEFAULT_GRACE_SECS, INSTRUCTION_MAX, Job, JobOptions, MAX_JOBS, MAX_RUNTIME_SECS,
    MAX_TURNS, NAME_MAX, Run, RunSpec, RunStatus, clip, source,
};
use crate::conversations::models::new_id;
use crate::redact::redact_text;
use crate::routes::Urgency;
use crate::store::{Store, StoreError};
use crate::time::{iso, parse_iso};

/// How many runs are kept per schedule, and per watch.
pub const RUNS_KEPT: i64 = 200;
/// A watch acts at most this often in an hour unless it says otherwise.
pub const ACT_PER_HOUR: u32 = 12;

const JOB_COLS: &str = "id, name, agent, instruction, schedule, options, enabled, created_by, \
    created_at, updated_at, next_run_at, last_run_at, last_outcome, last_error, check_hash";
const RUN_COLS: &str = "id, job, source, item_key, agent, name, request, status, scheduled_for, \
    started_at, finished_at, answer, error, delivery, attempts, model_calls, tool_calls";

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// What was asked for does not hold: a schedule that doesn't read, an unknown agent.
    #[error("{0}")]
    Invalid(String),
    #[error("there is no schedule {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, ScheduleError>;

fn invalid(msg: impl Into<String>) -> ScheduleError {
    ScheduleError::Invalid(msg.into())
}

/// What Ghost knows of an agent by its slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Known,
    Retired,
    Unknown,
}

pub type Agents = Arc<dyn Fn(&str) -> AgentState + Send + Sync>;

/// A change to a schedule, or a new one: every field left `None` stays as it is (or takes
/// its default in a new schedule).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobEdit {
    pub name: Option<String>,
    pub agent: Option<String>,
    pub instruction: Option<String>,
    pub when: Option<Written>,
    /// `Some(None)` clears them.
    pub active_hours: Option<Option<(String, String)>>,
    pub grace_minutes: Option<u64>,
    pub max_runtime_minutes: Option<u64>,
    pub max_turns: Option<u32>,
    pub delete_after_run: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub urgency: Option<String>,
    pub notify_failure: Option<bool>,
    /// `Some(None)` clears it.
    pub check: Option<Option<Check>>,
}

/// What `claim_due` found: the runs now queued, and the ones whose time passed while
/// Ghost was off for longer than their grace.
#[derive(Debug, Default)]
pub struct Claimed {
    pub queued: Vec<Run>,
    pub missed: Vec<Run>,
}

pub struct Jobs {
    store: Arc<Store>,
    default_tz: String,
    agents: Option<Agents>,
    /// Told when a run was queued or a schedule changed, so the scheduler looks now.
    wake: Notify,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// One piece of work at a time per agent, asks first (`super::turns`).
    turns: Arc<super::turns::AgentTurns>,
}

fn col_dt(raw: Option<String>) -> Option<DateTime<Utc>> {
    raw.as_deref().and_then(parse_iso)
}

fn opt_iso(dt: Option<DateTime<Utc>>) -> Option<String> {
    dt.as_ref().map(iso)
}

fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 80
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn job_row(r: &Row<'_>) -> rusqlite::Result<std::result::Result<Job, String>> {
    let id: String = r.get(0)?;
    let name: String = r.get(1)?;
    let schedule: String = r.get(4)?;
    let options: String = r.get(5)?;
    let parsed = serde_json::from_str::<Schedule>(&schedule)
        .map_err(|e| e.to_string())
        .and_then(|s| s.check().map(|_| s).map_err(|e| e.to_string()));
    let schedule = match parsed {
        Ok(s) => s,
        Err(e) => return Ok(Err(format!("schedule {name} ({id}) does not read: {e}"))),
    };
    let options: JobOptions = match serde_json::from_str(&options) {
        Ok(o) => o,
        Err(e) => {
            return Ok(Err(format!(
                "schedule {name} ({id}): its options do not read: {e}"
            )));
        }
    };
    Ok(Ok(Job {
        id,
        name,
        agent: r.get(2)?,
        instruction: r.get(3)?,
        schedule,
        options,
        enabled: r.get::<_, i64>(6)? != 0,
        created_by: r.get(7)?,
        created_at: col_dt(r.get(8)?).unwrap_or_default(),
        updated_at: col_dt(r.get(9)?).unwrap_or_default(),
        next_run_at: col_dt(r.get(10)?),
        last_run_at: col_dt(r.get(11)?),
        last_outcome: r.get(12)?,
        last_error: r.get(13)?,
        check_hash: r.get(14)?,
    }))
}

fn run_row(r: &Row<'_>) -> rusqlite::Result<Option<Run>> {
    let request: String = r.get(6)?;
    let status: String = r.get(7)?;
    let (Ok(spec), Some(status)) = (
        serde_json::from_str::<RunSpec>(&request),
        RunStatus::parse(&status),
    ) else {
        return Ok(None);
    };
    Ok(Some(Run {
        id: r.get(0)?,
        job: r.get(1)?,
        source: r.get(2)?,
        item_key: r.get(3)?,
        agent: r.get(4)?,
        name: r.get(5)?,
        spec,
        status,
        scheduled_for: col_dt(r.get(8)?).unwrap_or_default(),
        started_at: col_dt(r.get(9)?),
        finished_at: col_dt(r.get(10)?),
        answer: r.get(11)?,
        error: r.get(12)?,
        delivery: r.get(13)?,
        attempts: r.get::<_, i64>(14)?.max(0) as u32,
        model_calls: r.get::<_, i64>(15)?.max(0) as u32,
        tool_calls: r.get::<_, i64>(16)?.max(0) as u32,
    }))
}

impl Jobs {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            default_tz: when::machine_tz(),
            agents: None,
            wake: Notify::new(),
            on_change: Mutex::new(None),
            turns: super::turns::AgentTurns::new(),
        }
    }

    /// Which agents are working with nobody there, shared by the scheduler and every ask.
    pub fn turns(&self) -> &Arc<super::turns::AgentTurns> {
        &self.turns
    }

    /// The zone a schedule is read in when it names none; the machine's unless given.
    pub fn with_default_tz(mut self, tz: &str) -> Self {
        self.default_tz = tz.to_string();
        self
    }

    /// How a slug is checked: a schedule for an agent Ghost doesn't have is refused.
    pub fn with_agents(mut self, agents: Agents) -> Self {
        self.agents = Some(agents);
        self
    }

    /// Runs after a schedule or a run changed, so the apps can be told.
    pub fn on_change(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    pub fn default_tz(&self) -> &str {
        &self.default_tz
    }

    /// Resolves when something was queued or changed.
    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn changed(&self) {
        let f = self
            .on_change
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(f) = f {
            f();
        }
    }

    /// What Ghost knows of the agent a schedule or a watch names.
    pub fn agent_state(&self, slug: &str) -> AgentState {
        match &self.agents {
            Some(f) => f(slug),
            None => AgentState::Known,
        }
    }

    fn check_agent(&self, slug: &str) -> Result<()> {
        if !slug_ok(slug) {
            return Err(invalid("a schedule names its agent by slug"));
        }
        match self.agent_state(slug) {
            AgentState::Known => Ok(()),
            AgentState::Retired => Err(invalid(format!(
                "the agent {slug} is retired; rehire it first"
            ))),
            AgentState::Unknown => Err(invalid(format!("there is no agent {slug}"))),
        }
    }

    // --- reading -------------------------------------------------------------------

    fn rows(&self) -> Result<Vec<std::result::Result<Job, String>>> {
        let conn = self.store.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM schedules ORDER BY created_at, rowid"
        ))?;
        let rows = st.query_map([], job_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every schedule that reads, oldest first. One that doesn't is left out here and
    /// named by [`Jobs::problems`].
    pub fn list(&self) -> Result<Vec<Job>> {
        Ok(self.rows()?.into_iter().filter_map(|r| r.ok()).collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<Job>> {
        let conn = self.store.conn();
        let row = conn
            .query_row(
                &format!("SELECT {JOB_COLS} FROM schedules WHERE id = ?"),
                [id],
                job_row,
            )
            .optional()?;
        Ok(row.and_then(|r| r.ok()))
    }

    fn must(&self, id: &str) -> Result<Job> {
        self.get(id)?
            .ok_or_else(|| ScheduleError::NotFound(id.to_string()))
    }

    /// A schedule by its id, or by its name when exactly one has it (an agent is told
    /// "stop the morning check").
    pub fn find(&self, id_or_name: &str) -> Result<Option<Job>> {
        let wanted = id_or_name.trim();
        if let Some(job) = self.get(wanted)? {
            return Ok(Some(job));
        }
        let lower = wanted.to_lowercase();
        let mut named: Vec<Job> = self
            .list()?
            .into_iter()
            .filter(|j| j.name.to_lowercase() == lower)
            .collect();
        Ok(if named.len() == 1 { named.pop() } else { None })
    }

    /// What is wrong with the stored schedules, as (key, sentence): one that does not
    /// read, one for an agent Ghost doesn't have, one whose time never comes.
    pub fn problems(&self) -> Vec<(String, String)> {
        let rows = match self.rows() {
            Ok(r) => r,
            Err(e) => return vec![("schedules".into(), format!("The schedules: {e}"))],
        };
        let mut out = Vec::new();
        for (i, row) in rows.into_iter().enumerate() {
            match row {
                Err(problem) => out.push((format!("schedule:unreadable:{i}"), problem)),
                Ok(job) => {
                    let said = match self.agent_state(&job.agent) {
                        AgentState::Known => continue,
                        AgentState::Retired => "is retired",
                        AgentState::Unknown => "is not an agent Ghost has",
                    };
                    if job.enabled {
                        out.push((
                            format!("schedule:{}:agent", job.id),
                            format!(
                                "The schedule {} is for {}, which {said}; it will not run",
                                job.name, job.agent
                            ),
                        ));
                    }
                }
            }
        }
        out
    }

    // --- writing -------------------------------------------------------------------

    fn options_from(
        &self,
        base: JobOptions,
        edit: &JobEdit,
        schedule: &Schedule,
    ) -> Result<JobOptions> {
        let mut o = base;
        if let Schedule::Cron { tz, .. } = schedule {
            o.tz = tz.clone();
        } else if let Some(tz) = edit
            .when
            .as_ref()
            .and_then(|w| w.tz.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            o.tz = when::parse_tz(tz)
                .map_err(|e| invalid(e.to_string()))?
                .name()
                .to_string();
        }
        if let Some(hours) = &edit.active_hours {
            o.active_hours = match hours {
                None => None,
                Some((start, end)) => {
                    Some(ActiveHours::parse(start, end).map_err(|e| invalid(e.to_string()))?)
                }
            };
        }
        if let Some(m) = edit.grace_minutes {
            if m > 7 * 24 * 60 {
                return Err(invalid("the grace window is a week at the most"));
            }
            o.grace_secs = m * 60;
        }
        if let Some(m) = edit.max_runtime_minutes {
            if !(1..=MAX_RUNTIME_SECS / 60).contains(&m) {
                return Err(invalid(format!(
                    "a run takes from 1 to {} minutes",
                    MAX_RUNTIME_SECS / 60
                )));
            }
            o.max_runtime_secs = m * 60;
        }
        if let Some(t) = edit.max_turns {
            if !(1..=MAX_TURNS).contains(&t) {
                return Err(invalid(format!("a run takes from 1 to {MAX_TURNS} turns")));
            }
            o.max_turns = t;
        }
        if let Some(d) = edit.delete_after_run {
            o.delete_after_run = d;
        }
        if let Some(skills) = &edit.skills {
            if skills.len() > 8 || skills.iter().any(|s| s.trim().is_empty() || s.len() > 80) {
                return Err(invalid("skills is up to eight skill names"));
            }
            o.skills = skills.iter().map(|s| s.trim().to_string()).collect();
        }
        if let Some(u) = &edit.urgency {
            let u = u.trim().to_lowercase();
            if Urgency::parse(&u).is_none() {
                return Err(invalid("urgency is digest, notify or call"));
            }
            o.urgency = u;
        }
        if let Some(n) = edit.notify_failure {
            o.notify_failure = n;
        }
        if let Some(check) = &edit.check {
            if let Some(c) = check
                && (c.app.trim().is_empty() || c.read.trim().is_empty())
            {
                return Err(invalid(
                    "a check names an app and one of its read-only tools",
                ));
            }
            o.check = check.clone();
        }
        Ok(o)
    }

    fn text(value: &str, what: &str, max: usize) -> Result<String> {
        let t = value.trim();
        if t.is_empty() {
            return Err(invalid(format!("a schedule needs {what}")));
        }
        if t.chars().count() > max {
            return Err(invalid(format!("{what} is {max} characters at the most")));
        }
        Ok(t.to_string())
    }

    /// A new schedule. `created_by` is `operator`, `agent:<slug>` or `client:<name>` (an
    /// outside tool, ADR 0048).
    pub fn create(&self, edit: &JobEdit, created_by: &str, now: DateTime<Utc>) -> Result<Job> {
        let instruction = Self::text(
            edit.instruction.as_deref().unwrap_or(""),
            "an instruction",
            INSTRUCTION_MAX,
        )?;
        let name = match edit
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            Some(n) => Self::text(n, "a name", NAME_MAX)?,
            None => clip(
                &instruction.split_whitespace().collect::<Vec<_>>().join(" "),
                48,
            ),
        };
        let agent = edit
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or(crate::conversations::GHOST)
            .to_string();
        self.check_agent(&agent)?;
        let written = edit
            .when
            .as_ref()
            .ok_or_else(|| invalid("a schedule needs a time: at, every or cron"))?;
        let schedule =
            Schedule::parse(written, now, &self.default_tz).map_err(|e| invalid(e.to_string()))?;
        let base = JobOptions {
            tz: self.default_tz.clone(),
            grace_secs: DEFAULT_GRACE_SECS,
            // A one-off is gone once it has run, unless asked to stay (OpenClaw's default).
            delete_after_run: schedule.is_once(),
            ..JobOptions::default()
        };
        let options = self.options_from(base, edit, &schedule)?;
        let next = when::next_run(&schedule, options.active_hours.as_ref(), &options.tz, now)
            .ok_or_else(|| invalid("that time has passed, or never comes"))?;
        if self.count()? >= MAX_JOBS {
            return Err(invalid(format!(
                "there are {MAX_JOBS} schedules already; remove one first"
            )));
        }
        let job = Job {
            id: new_id()[..12].to_string(),
            name,
            agent,
            instruction,
            schedule,
            options,
            enabled: true,
            created_by: created_by.to_string(),
            created_at: now,
            updated_at: now,
            next_run_at: Some(next),
            last_run_at: None,
            last_outcome: None,
            last_error: None,
            check_hash: None,
        };
        self.store.conn().execute(
            &format!("INSERT INTO schedules ({JOB_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
            params![
                job.id,
                redact_text(&job.name),
                job.agent,
                redact_text(&job.instruction),
                serde_json::to_string(&job.schedule).unwrap_or_default(),
                serde_json::to_string(&job.options).unwrap_or_default(),
                1,
                job.created_by,
                iso(&job.created_at),
                iso(&job.updated_at),
                opt_iso(job.next_run_at),
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
            ],
        )?;
        self.wake();
        self.changed();
        self.must(&job.id)
    }

    fn count(&self) -> Result<usize> {
        let n: i64 = self
            .store
            .conn()
            .query_row("SELECT COUNT(*) FROM schedules", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Change a schedule. A new time, new active hours or a new check start it afresh.
    pub fn update(&self, id: &str, edit: &JobEdit, now: DateTime<Utc>) -> Result<Job> {
        let mut job = self.must(id)?;
        if let Some(name) = &edit.name {
            job.name = Self::text(name, "a name", NAME_MAX)?;
        }
        if let Some(instruction) = &edit.instruction {
            job.instruction = Self::text(instruction, "an instruction", INSTRUCTION_MAX)?;
        }
        if let Some(agent) = edit
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            self.check_agent(agent)?;
            job.agent = agent.to_string();
        }
        let mut retime = false;
        if let Some(written) = &edit.when {
            job.schedule = Schedule::parse(written, now, &job.options.tz)
                .map_err(|e| invalid(e.to_string()))?;
            retime = true;
        }
        retime |= edit.active_hours.is_some();
        let check_changed = edit.check.is_some();
        job.options = self.options_from(job.options.clone(), edit, &job.schedule)?;
        if retime {
            let next = when::next_run(
                &job.schedule,
                job.options.active_hours.as_ref(),
                &job.options.tz,
                now,
            )
            .ok_or_else(|| invalid("that time has passed, or never comes"))?;
            job.next_run_at = Some(next);
        }
        let conn = self.store.conn();
        conn.execute(
            "UPDATE schedules SET name = ?, agent = ?, instruction = ?, schedule = ?, options = ?, \
             next_run_at = ?, updated_at = ? WHERE id = ?",
            params![
                redact_text(&job.name),
                job.agent,
                redact_text(&job.instruction),
                serde_json::to_string(&job.schedule).unwrap_or_default(),
                serde_json::to_string(&job.options).unwrap_or_default(),
                opt_iso(job.next_run_at),
                iso(&now),
                id
            ],
        )?;
        if check_changed {
            // Another check: what the old one returned is nothing to compare with.
            conn.execute(
                "UPDATE schedules SET check_hash = NULL, check_last = NULL WHERE id = ?",
                [id],
            )?;
        }
        drop(conn);
        self.wake();
        self.changed();
        self.must(id)
    }

    /// Pause or resume. Resuming counts from now: what it would have run while paused is
    /// not run.
    pub fn set_enabled(&self, id: &str, enabled: bool, now: DateTime<Utc>) -> Result<Job> {
        let job = self.must(id)?;
        let next = if enabled {
            let next = when::next_run(
                &job.schedule,
                job.options.active_hours.as_ref(),
                &job.options.tz,
                now,
            );
            if next.is_none() {
                return Err(invalid(
                    "its time has passed; give it a new one to resume it",
                ));
            }
            next
        } else {
            job.next_run_at
        };
        self.store.conn().execute(
            "UPDATE schedules SET enabled = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
            params![enabled as i64, opt_iso(next), iso(&now), id],
        )?;
        self.wake();
        self.changed();
        self.must(id)
    }

    /// Remove a schedule. A run of it still waiting is cancelled; its past runs go too.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let conn = self.store.conn();
        let n = conn.execute("DELETE FROM schedules WHERE id = ?", [id])?;
        if n > 0 {
            conn.execute(
                "DELETE FROM schedule_runs WHERE job = ? AND status != 'running'",
                [id],
            )?;
        }
        drop(conn);
        if n > 0 {
            self.changed();
        }
        Ok(n > 0)
    }

    // --- runs ----------------------------------------------------------------------

    fn spec_of(job: &Job) -> RunSpec {
        RunSpec {
            instruction: job.instruction.clone(),
            data: None,
            skills: job.options.skills.clone(),
            max_turns: job.options.max_turns,
            max_runtime_secs: job.options.max_runtime_secs,
            urgency: job.options.urgency.clone(),
            notify_failure: job.options.notify_failure,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_run(
        &self,
        job: Option<&str>,
        source: &str,
        item_key: Option<&str>,
        agent: &str,
        name: &str,
        spec: &RunSpec,
        status: RunStatus,
        scheduled_for: DateTime<Utc>,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Run> {
        let id = new_id()[..12].to_string();
        let finished = status.finished().then(|| iso(&now));
        let conn = self.store.conn();
        conn.execute(
            "INSERT INTO schedule_runs (id, job, source, item_key, agent, name, request, status, \
             scheduled_for, finished_at, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                job,
                source,
                item_key,
                agent,
                redact_text(name),
                redact_text(&serde_json::to_string(spec).unwrap_or_default()),
                status.as_str(),
                iso(&scheduled_for),
                finished,
                error.map(redact_text),
            ],
        )?;
        // Keep the newest of each schedule's, and of each watch's.
        match job {
            Some(job) => conn.execute(
                "DELETE FROM schedule_runs WHERE job = ?1 AND status NOT IN ('queued', 'running') \
                 AND id NOT IN (SELECT id FROM schedule_runs WHERE job = ?1 \
                 ORDER BY scheduled_for DESC, rowid DESC LIMIT ?2)",
                params![job, RUNS_KEPT],
            )?,
            None => conn.execute(
                "DELETE FROM schedule_runs WHERE job IS NULL AND source = ?1 \
                 AND status NOT IN ('queued', 'running') \
                 AND id NOT IN (SELECT id FROM schedule_runs WHERE job IS NULL AND source = ?1 \
                 ORDER BY scheduled_for DESC, rowid DESC LIMIT ?2)",
                params![source, RUNS_KEPT],
            )?,
        };
        drop(conn);
        self.must_run(&id)
    }

    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        let conn = self.store.conn();
        Ok(conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM schedule_runs WHERE id = ?"),
                [id],
                run_row,
            )
            .optional()?
            .flatten())
    }

    fn must_run(&self, id: &str) -> Result<Run> {
        self.run(id)?
            .ok_or_else(|| ScheduleError::NotFound(id.to_string()))
    }

    fn runs_where(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Run>> {
        let conn = self.store.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM schedule_runs WHERE {clause}"
        ))?;
        let rows = st.query_map(args, run_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    /// A schedule's runs, newest first.
    pub fn runs(&self, job: &str, limit: i64) -> Result<Vec<Run>> {
        self.runs_where(
            "job = ? ORDER BY scheduled_for DESC, rowid DESC LIMIT ?",
            &[&job, &limit.clamp(1, RUNS_KEPT)],
        )
    }

    /// The runs of every schedule and watch, newest first.
    pub fn recent_runs(&self, limit: i64) -> Result<Vec<Run>> {
        self.runs_where(
            "1 = 1 ORDER BY scheduled_for DESC, rowid DESC LIMIT ?",
            &[&limit.clamp(1, RUNS_KEPT)],
        )
    }

    /// The runs scheduled in `[since, until)`, newest first (ADR 0060): one day of them,
    /// whatever ran since.
    pub fn runs_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Run>> {
        self.runs_where(
            "scheduled_for >= ? AND scheduled_for < ? ORDER BY scheduled_for DESC, rowid DESC LIMIT ?",
            &[&iso(&since), &iso(&until), &limit.clamp(1, RUNS_KEPT)],
        )
    }

    /// The runs waiting for their agent, oldest first.
    pub fn queued(&self) -> Result<Vec<Run>> {
        self.runs_where("status = 'queued' ORDER BY scheduled_for, rowid", &[])
    }

    fn open_run_of(&self, job: &str) -> Result<Option<Run>> {
        Ok(self
            .runs_where(
                "job = ? AND status IN ('queued', 'running') ORDER BY rowid LIMIT 1",
                &[&job],
            )?
            .pop())
    }

    /// Run a schedule now, whatever its time and whether or not it is paused. One already
    /// waiting or running is the answer: there is never a second beside it.
    pub fn request_run(&self, id: &str, now: DateTime<Utc>) -> Result<Run> {
        let job = self.must(id)?;
        if let Some(open) = self.open_run_of(id)? {
            return Ok(open);
        }
        let run = self.insert_run(
            Some(id),
            source::RUN_NOW,
            None,
            &job.agent,
            &job.name,
            &Self::spec_of(&job),
            RunStatus::Queued,
            now,
            None,
            now,
        )?;
        self.wake();
        self.changed();
        Ok(run)
    }

    /// Queue a run for a watch's `act`, unless the watch has acted on this item before
    /// (`once_per_key`) or has acted `per_hour` times in the last hour. `Err` says why not.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_act(
        &self,
        watch_source: &str,
        item_key: &str,
        agent: &str,
        name: &str,
        spec: &RunSpec,
        per_hour: u32,
        once_per_key: bool,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<Run, String>> {
        {
            let conn = self.store.conn();
            if once_per_key {
                let seen: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM schedule_runs WHERE source = ? AND item_key = ?",
                    params![watch_source, item_key],
                    |r| r.get(0),
                )?;
                if seen > 0 {
                    return Ok(Err("it has acted on this item already".into()));
                }
            }
            let recent: i64 = conn.query_row(
                "SELECT COUNT(*) FROM schedule_runs WHERE source = ? AND scheduled_for > ?",
                params![watch_source, iso(&(now - Duration::hours(1)))],
                |r| r.get(0),
            )?;
            if recent >= i64::from(per_hour) {
                return Ok(Err(format!(
                    "it has acted {recent} times in the last hour, its limit"
                )));
            }
        }
        let run = self.insert_run(
            None,
            watch_source,
            Some(item_key),
            agent,
            name,
            spec,
            RunStatus::Queued,
            now,
            None,
            now,
        )?;
        self.wake();
        self.changed();
        Ok(Ok(run))
    }

    /// What is due at `now`: each due schedule gets one queued run and its next time
    /// moves past `now`, so there is one run however long Ghost was off, never a backlog.
    /// One due for longer than its grace window is recorded as missed and not run. One
    /// with a run still waiting or running is not given another, and one whose agent is
    /// gone or retired is left alone.
    pub fn claim_due(&self, now: DateTime<Utc>) -> Result<Claimed> {
        let mut out = Claimed::default();
        for job in self.list()? {
            let Some(due) = job.next_run_at.filter(|t| job.enabled && *t <= now) else {
                continue;
            };
            if self.agent_state(&job.agent) != AgentState::Known {
                // Nobody to run it (`problems` says so). Should the agent come back, its
                // time is caught up or missed like any other.
                continue;
            }
            let next = when::next_run(
                &job.schedule,
                job.options.active_hours.as_ref(),
                &job.options.tz,
                now,
            );
            let late = (now - due).num_seconds().max(0) as u64;
            let spec = Self::spec_of(&job);
            if late > job.options.grace_secs {
                let why = format!(
                    "Ghost was not running at {}; it came back {} later, past the {} it may be late by",
                    due.format("%Y-%m-%d %H:%M UTC"),
                    when::say_duration(late - late % 60),
                    when::say_duration(job.options.grace_secs)
                );
                out.missed.push(self.insert_run(
                    Some(&job.id),
                    source::SCHEDULE,
                    None,
                    &job.agent,
                    &job.name,
                    &spec,
                    RunStatus::Missed,
                    due,
                    Some(&why),
                    now,
                )?);
                self.store.conn().execute(
                    "UPDATE schedules SET last_outcome = 'missed', last_error = ?, next_run_at = ?, \
                     enabled = ? WHERE id = ?",
                    params![why, opt_iso(next), next.is_some() as i64, job.id],
                )?;
                continue;
            }
            if self.open_run_of(&job.id)?.is_none() {
                out.queued.push(self.insert_run(
                    Some(&job.id),
                    source::SCHEDULE,
                    None,
                    &job.agent,
                    &job.name,
                    &spec,
                    RunStatus::Queued,
                    due,
                    None,
                    now,
                )?);
            }
            self.store.conn().execute(
                "UPDATE schedules SET next_run_at = ? WHERE id = ?",
                params![opt_iso(next), job.id],
            )?;
        }
        if !(out.queued.is_empty() && out.missed.is_empty()) {
            self.changed();
        }
        Ok(out)
    }

    /// What a start finds from before: a run that was going when Ghost stopped is
    /// interrupted, and is not run again; one still waiting past its grace is missed.
    pub fn recover(&self, now: DateTime<Utc>) -> Result<Vec<Run>> {
        let mut out = Vec::new();
        for run in self.runs_where("status = 'running' ORDER BY rowid", &[])? {
            self.finish(
                &run.id,
                RunStatus::Interrupted,
                None,
                Some("Ghost stopped while it was running; it was not run again"),
                None,
                (0, 0),
                now,
            )?;
            out.push(self.must_run(&run.id)?);
        }
        for run in self.queued()? {
            let grace = match &run.job {
                Some(id) => self
                    .get(id)?
                    .map_or(DEFAULT_GRACE_SECS, |j| j.options.grace_secs),
                None => DEFAULT_GRACE_SECS,
            };
            if (now - run.scheduled_for).num_seconds() > grace as i64 {
                self.finish(
                    &run.id,
                    RunStatus::Missed,
                    None,
                    Some("Ghost was off past the time it could still have run"),
                    None,
                    (0, 0),
                    now,
                )?;
                out.push(self.must_run(&run.id)?);
            }
        }
        Ok(out)
    }

    /// A queued run starts. False when it is not queued any more (cancelled, removed).
    pub fn start(&self, run_id: &str, now: DateTime<Utc>) -> Result<bool> {
        let n = self.store.conn().execute(
            "UPDATE schedule_runs SET status = 'running', started_at = ? \
             WHERE id = ? AND status = 'queued'",
            params![iso(&now), run_id],
        )?;
        if n > 0 {
            self.changed();
        }
        Ok(n > 0)
    }

    /// How many times the model was asked again after it could not be reached.
    pub fn set_retries(&self, run_id: &str, retries: u32) -> Result<()> {
        self.store.conn().execute(
            "UPDATE schedule_runs SET attempts = ? WHERE id = ?",
            params![retries, run_id],
        )?;
        Ok(())
    }

    /// A run's result, recorded once: a run that already has one keeps it.
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &self,
        run_id: &str,
        status: RunStatus,
        answer: Option<&str>,
        error: Option<&str>,
        delivery: Option<&str>,
        (model_calls, tool_calls): (u32, u32),
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.store.conn().execute(
            "UPDATE schedule_runs SET status = ?, finished_at = ?, answer = ?, error = ?, \
             delivery = ?, model_calls = ?, tool_calls = ? \
             WHERE id = ? AND status IN ('queued', 'running')",
            params![
                status.as_str(),
                iso(&now),
                answer.map(|a| redact_text(&clip(a, super::ANSWER_MAX))),
                error.map(|e| redact_text(&clip(e, 500))),
                delivery,
                model_calls,
                tool_calls,
                run_id
            ],
        )?;
        if n > 0 {
            self.changed();
        }
        Ok(n > 0)
    }

    pub fn set_delivery(&self, run_id: &str, delivery: &str) -> Result<()> {
        self.store.conn().execute(
            "UPDATE schedule_runs SET delivery = ? WHERE id = ?",
            params![delivery, run_id],
        )?;
        Ok(())
    }

    /// Only the newest `unchanged` look of a schedule is kept: a check every five minutes
    /// is not three hundred rows a day.
    pub fn drop_older_unchanged(&self, job: &str, keep: &str) -> Result<()> {
        self.store.conn().execute(
            "DELETE FROM schedule_runs WHERE job = ? AND status = 'unchanged' AND id != ?",
            params![job, keep],
        )?;
        Ok(())
    }

    /// What a schedule's last run came to. A one-off that has run is removed when it says
    /// so, else paused.
    pub fn note(
        &self,
        job_id: &str,
        status: RunStatus,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Some(job) = self.get(job_id)? else {
            return Ok(());
        };
        self.store.conn().execute(
            "UPDATE schedules SET last_run_at = ?, last_outcome = ?, last_error = ? WHERE id = ?",
            params![
                iso(&now),
                status.as_str(),
                error.map(|e| redact_text(&clip(e, 500))),
                job_id
            ],
        )?;
        if job.schedule.is_once() && job.next_run_at.is_none() {
            if job.options.delete_after_run && status == RunStatus::Ok {
                // Its runs are kept: what it said is still to be read.
                self.store
                    .conn()
                    .execute("DELETE FROM schedules WHERE id = ?", [job_id])?;
            } else {
                self.store
                    .conn()
                    .execute("UPDATE schedules SET enabled = 0 WHERE id = ?", [job_id])?;
            }
        }
        self.changed();
        Ok(())
    }

    /// What the schedule's check returned last time: its hash, and the text (bounded).
    pub fn check_state(&self, job_id: &str) -> Result<(Option<String>, Option<String>)> {
        let conn = self.store.conn();
        Ok(conn
            .query_row(
                "SELECT check_hash, check_last FROM schedules WHERE id = ?",
                [job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, None)))
    }

    pub fn set_check_state(&self, job_id: &str, hash: &str, last: &str) -> Result<()> {
        self.store.conn().execute(
            "UPDATE schedules SET check_hash = ?, check_last = ? WHERE id = ?",
            params![hash, redact_text(last), job_id],
        )?;
        Ok(())
    }

    /// A stored row as JSON would break `list`; `ghostd check` and the tests write one.
    #[doc(hidden)]
    pub fn corrupt_for_test(&self, id: &str, schedule: &Value) {
        if let Err(e) = self.store.conn().execute(
            "UPDATE schedules SET schedule = ? WHERE id = ?",
            params![schedule.to_string(), id],
        ) {
            warn!("schedules: {e}");
        }
    }
}
