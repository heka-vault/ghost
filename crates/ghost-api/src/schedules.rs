//! The schedules (ADR 0047), for the operator: list, add, change, pause, resume, remove,
//! run one now, and read what its runs said.
//!
//! These are the operator's own edits in their own desktop session, so a schedule made
//! here is attributed to the operator. What a schedule does when it runs still goes
//! through the gate, as an agent's work does. An agent's own way in is Ghost's app
//! (`ghost_schedule_manage`), never these routes: the model has no route to the control API.

use axum::Json;
use axum::extract::{Path as PathArg, State};
use chrono::{DateTime, Utc};
use ghost_core::schedules::scheduler::Scheduler;
use ghost_core::schedules::{
    Check, Job, JobEdit, Run, RunStatus, Schedule, ScheduleError, Written, when,
};
use ghost_core::time::utcnow;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::Qs;
use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};
use crate::models::{Ack, Urgency};

/// Which of the three kinds a schedule is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleKind {
    /// Once, at a time.
    At,
    /// Every interval.
    Every,
    /// A cron expression in a time zone.
    Cron,
}

/// Where a run is, or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleRunStatus {
    Queued,
    Running,
    Ok,
    /// Its check returned what it returned last time: the agent did not run.
    Unchanged,
    Failed,
    TimedOut,
    Cancelled,
    /// Ghost stopped while it ran; it was not run again.
    Interrupted,
    /// Ghost was not running at its time and came back too late for it.
    Missed,
}

impl From<RunStatus> for ScheduleRunStatus {
    fn from(s: RunStatus) -> Self {
        match s {
            RunStatus::Queued => Self::Queued,
            RunStatus::Running => Self::Running,
            RunStatus::Ok => Self::Ok,
            RunStatus::Unchanged => Self::Unchanged,
            RunStatus::Failed => Self::Failed,
            RunStatus::TimedOut => Self::TimedOut,
            RunStatus::Cancelled => Self::Cancelled,
            RunStatus::Interrupted => Self::Interrupted,
            RunStatus::Missed => Self::Missed,
        }
    }
}

/// The hours of the day a schedule may run in, in its time zone (`08:00` to `18:00`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleHours {
    pub start: String,
    pub end: String,
}

/// A look that runs first, with no model: one of a connected app's read-only tools. The
/// agent runs only when what it returns has changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleCheck {
    pub app: String,
    pub read: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

/// One schedule: what an agent does on its own, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleInfo {
    pub id: String,
    pub name: String,
    /// The agent's slug.
    pub agent: String,
    pub instruction: String,
    #[schema(inline)]
    pub kind: ScheduleKind,
    /// The time of a one-off.
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
    /// An interval as written: `30m`.
    #[serde(default)]
    pub every: Option<String>,
    /// Five cron fields.
    #[serde(default)]
    pub cron: Option<String>,
    pub time_zone: String,
    /// The schedule in a few words: `every 30m`, `0 9 * * 1-5 (Europe/London)`.
    pub summary: String,
    pub enabled: bool,
    /// `operator`, or `agent:<slug>`.
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub next_run: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
    #[serde(default)]
    #[schema(inline)]
    pub last_outcome: Option<ScheduleRunStatus>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// A run of it is waiting or going now.
    pub running: bool,
    #[serde(default)]
    pub active_hours: Option<ScheduleHours>,
    /// A run missed by less than this is run once when Ghost is back.
    pub grace_minutes: u64,
    pub max_runtime_minutes: u64,
    pub max_turns: u32,
    pub delete_after_run: bool,
    pub skills: Vec<String>,
    #[schema(inline)]
    pub urgency: Urgency,
    /// Whether a run that fails tells the operator.
    pub notify_failure: bool,
    #[serde(default)]
    pub check: Option<ScheduleCheck>,
}

/// A new schedule, or a change to one: what is left out stays as it is (or takes its
/// default in a new one). When is exactly one of `at`, `every` and `cron`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// The agent's slug; Ghost when left out of a new schedule.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    /// Once: `2026-11-12T09:00` in the schedule's time zone, a time with an offset, or a
    /// delay such as `20m`.
    #[serde(default)]
    pub at: Option<String>,
    /// `30m`, `2h`, `1d`.
    #[serde(default)]
    pub every: Option<String>,
    /// Five fields: `0 9 * * 1-5`.
    #[serde(default)]
    pub cron: Option<String>,
    /// An IANA name; the machine's when left out.
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub active_hours: Option<ScheduleHours>,
    /// Run at any hour again.
    #[serde(default)]
    pub clear_active_hours: bool,
    #[serde(default)]
    pub grace_minutes: Option<u64>,
    #[serde(default)]
    pub max_runtime_minutes: Option<u64>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub delete_after_run: Option<bool>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    #[schema(inline)]
    pub urgency: Option<Urgency>,
    #[serde(default)]
    pub notify_failure: Option<bool>,
    #[serde(default)]
    pub check: Option<ScheduleCheck>,
    /// Run every time again, without looking first.
    #[serde(default)]
    pub clear_check: bool,
}

/// The schedules, the zone a time is read in when none is named, and what is wrong with
/// any of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SchedulesView {
    pub schedules: Vec<ScheduleInfo>,
    pub time_zone: String,
    pub problems: Vec<String>,
}

/// One run of an agent with nobody there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleRunInfo {
    pub id: String,
    /// The schedule's id; none for a watch that acted.
    #[serde(default)]
    pub schedule: Option<String>,
    /// `schedule`, `run_now`, or `watch:<name>`.
    pub source: String,
    pub agent: String,
    pub name: String,
    #[schema(inline)]
    pub status: ScheduleRunStatus,
    pub scheduled_for: DateTime<Utc>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// What the agent answered.
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// What came of the answer: delivered, silent, empty, by_agent, failure,
    /// failure_not_told or none.
    #[serde(default)]
    pub delivery: Option<String>,
    /// Model calls tried again after the model could not be reached.
    pub retries: u32,
    pub model_calls: u32,
    pub tool_calls: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScheduleRunsView {
    pub runs: Vec<ScheduleRunInfo>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RunsQuery {
    #[param(minimum = 1, maximum = 200, default = 20)]
    pub limit: Option<i64>,
}

fn urgency(u: &str) -> Urgency {
    match u {
        "digest" => Urgency::Digest,
        "call" => Urgency::Call,
        _ => Urgency::Notify,
    }
}

fn urgency_str(u: Urgency) -> &'static str {
    match u {
        Urgency::Digest => "digest",
        Urgency::Notify => "notify",
        Urgency::Call => "call",
    }
}

pub fn schedule_info(job: &Job, running: bool) -> ScheduleInfo {
    let (kind, at, every, cron) = match &job.schedule {
        Schedule::At { at } => (ScheduleKind::At, Some(*at), None, None),
        Schedule::Every { secs, .. } => (
            ScheduleKind::Every,
            None,
            Some(when::say_duration(*secs)),
            None,
        ),
        Schedule::Cron { expr, .. } => (ScheduleKind::Cron, None, None, Some(expr.clone())),
    };
    ScheduleInfo {
        id: job.id.clone(),
        name: job.name.clone(),
        agent: job.agent.clone(),
        instruction: job.instruction.clone(),
        kind,
        at,
        every,
        cron,
        time_zone: job.options.tz.clone(),
        summary: job.schedule.say(&job.options.tz),
        enabled: job.enabled,
        created_by: job.created_by.clone(),
        created_at: job.created_at,
        updated_at: job.updated_at,
        next_run: job.next_run_at.filter(|_| job.enabled),
        last_run: job.last_run_at,
        last_outcome: job
            .last_outcome
            .as_deref()
            .and_then(RunStatus::parse)
            .map(Into::into),
        last_error: job.last_error.clone(),
        running,
        active_hours: job.options.active_hours.as_ref().map(|h| ScheduleHours {
            start: h.start.clone(),
            end: h.end.clone(),
        }),
        grace_minutes: job.options.grace_secs / 60,
        max_runtime_minutes: job.options.max_runtime_secs / 60,
        max_turns: job.options.max_turns,
        delete_after_run: job.options.delete_after_run,
        skills: job.options.skills.clone(),
        urgency: urgency(&job.options.urgency),
        notify_failure: job.options.notify_failure,
        check: job.options.check.as_ref().map(|c| ScheduleCheck {
            app: c.app.clone(),
            read: c.read.clone(),
            args: c.args.clone(),
        }),
    }
}

pub fn run_info(run: &Run) -> ScheduleRunInfo {
    ScheduleRunInfo {
        id: run.id.clone(),
        schedule: run.job.clone(),
        source: run.source.clone(),
        agent: run.agent.clone(),
        name: run.name.clone(),
        status: run.status.into(),
        scheduled_for: run.scheduled_for,
        started_at: run.started_at,
        finished_at: run.finished_at,
        answer: run.answer.clone(),
        error: run.error.clone(),
        delivery: run.delivery.clone(),
        retries: run.attempts,
        model_calls: run.model_calls,
        tool_calls: run.tool_calls,
    }
}

fn edit_of(req: ScheduleRequest) -> JobEdit {
    let timed =
        req.at.is_some() || req.every.is_some() || req.cron.is_some() || req.time_zone.is_some();
    JobEdit {
        name: req.name,
        agent: req.agent,
        instruction: req.instruction,
        when: timed.then_some(Written {
            at: req.at,
            every: req.every,
            cron: req.cron,
            tz: req.time_zone,
        }),
        active_hours: if req.clear_active_hours {
            Some(None)
        } else {
            req.active_hours.map(|h| Some((h.start, h.end)))
        },
        grace_minutes: req.grace_minutes,
        max_runtime_minutes: req.max_runtime_minutes,
        max_turns: req.max_turns,
        delete_after_run: req.delete_after_run,
        skills: req.skills,
        urgency: req.urgency.map(|u| urgency_str(u).to_string()),
        notify_failure: req.notify_failure,
        check: if req.clear_check {
            Some(None)
        } else {
            req.check.map(|c| {
                Some(Check {
                    app: c.app,
                    read: c.read,
                    args: c.args,
                })
            })
        },
    }
}

fn scheduler(deps: &SharedDeps) -> ApiResult<&std::sync::Arc<Scheduler>> {
    deps.schedules
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("schedules are not running"))
}

fn refused(e: ScheduleError) -> ApiError {
    match e {
        ScheduleError::NotFound(_) => ApiError::not_found(e.to_string()),
        ScheduleError::Invalid(_) => ApiError::unprocessable(e.to_string()),
        other => ApiError::new(500, other.to_string()),
    }
}

fn running(s: &Scheduler, job: &str) -> bool {
    s.jobs()
        .runs(job, 1)
        .ok()
        .and_then(|r| r.into_iter().next())
        .is_some_and(|r| !r.status.finished())
}

fn info(s: &Scheduler, job: &Job) -> ScheduleInfo {
    schedule_info(job, running(s, &job.id))
}

/// The schedules, oldest first.
#[utoipa::path(get, path = "/schedules", operation_id = "schedules_schedules_get", tag = "schedules",
    summary = "Schedules",
    description = "What each agent does on its own, and when: every schedule with its next run and how\nthe last one ended, and what is wrong with any of them (ADR 0047).",
    responses((status = 200, description = "Successful Response", body = SchedulesView),
              (status = 503, description = "Schedules are not running")))]
async fn schedules(State(deps): State<SharedDeps>) -> ApiResult<Json<SchedulesView>> {
    let s = scheduler(&deps)?;
    let jobs = s.jobs().list().map_err(refused)?;
    Ok(Json(SchedulesView {
        schedules: jobs.iter().map(|j| info(s, j)).collect(),
        time_zone: s.jobs().default_tz().to_string(),
        problems: s.jobs().problems().into_iter().map(|(_, p)| p).collect(),
    }))
}

/// Add a schedule. It is the operator's.
#[utoipa::path(post, path = "/schedules", operation_id = "add_schedule_schedules_post", tag = "schedules",
    summary = "Add a schedule", request_body = ScheduleRequest,
    responses((status = 200, description = "Successful Response", body = ScheduleInfo),
              (status = 422, description = "Validation Error", body = HTTPValidationError),
              (status = 503, description = "Schedules are not running")))]
async fn add_schedule(
    State(deps): State<SharedDeps>,
    Body(req): Body<ScheduleRequest>,
) -> ApiResult<Json<ScheduleInfo>> {
    let s = scheduler(&deps)?;
    let job = s
        .jobs()
        .create(&edit_of(req), ghost_core::schedules::OPERATOR, utcnow())
        .map_err(refused)?;
    Ok(Json(info(s, &job)))
}

/// One schedule.
#[utoipa::path(get, path = "/schedules/{schedule_id}", operation_id = "schedule_schedules__schedule_id__get",
    tag = "schedules", summary = "A schedule", params(("schedule_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = ScheduleInfo),
              (status = 404, description = "No such schedule")))]
async fn schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<ScheduleInfo>> {
    let s = scheduler(&deps)?;
    let job = s
        .jobs()
        .get(&id)
        .map_err(refused)?
        .ok_or_else(|| ApiError::not_found(format!("there is no schedule {id}")))?;
    Ok(Json(info(s, &job)))
}

/// Change a schedule: what is left out stays as it is.
#[utoipa::path(patch, path = "/schedules/{schedule_id}", operation_id = "edit_schedule_schedules__schedule_id__patch",
    tag = "schedules", summary = "Change a schedule", params(("schedule_id" = String, Path,)),
    request_body = ScheduleRequest,
    responses((status = 200, description = "Successful Response", body = ScheduleInfo),
              (status = 404, description = "No such schedule"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn edit_schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Body(req): Body<ScheduleRequest>,
) -> ApiResult<Json<ScheduleInfo>> {
    let s = scheduler(&deps)?;
    let job = s
        .jobs()
        .update(&id, &edit_of(req), utcnow())
        .map_err(refused)?;
    Ok(Json(info(s, &job)))
}

/// Remove a schedule, with its runs.
#[utoipa::path(delete, path = "/schedules/{schedule_id}", operation_id = "remove_schedule_schedules__schedule_id__delete",
    tag = "schedules", summary = "Remove a schedule", params(("schedule_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such schedule")))]
async fn remove_schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let s = scheduler(&deps)?;
    if !s.jobs().remove(&id).map_err(refused)? {
        return Err(ApiError::not_found(format!("there is no schedule {id}")));
    }
    Ok(Json(Ack {
        ok: true,
        detail: None,
    }))
}

/// Pause a schedule: it keeps its time and does not run.
#[utoipa::path(post, path = "/schedules/{schedule_id}/pause", operation_id = "pause_schedule_schedules__schedule_id__pause_post",
    tag = "schedules", summary = "Pause a schedule", params(("schedule_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = ScheduleInfo),
              (status = 404, description = "No such schedule")))]
async fn pause_schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<ScheduleInfo>> {
    let s = scheduler(&deps)?;
    let job = s
        .jobs()
        .set_enabled(&id, false, utcnow())
        .map_err(refused)?;
    Ok(Json(info(s, &job)))
}

/// Resume a schedule, counting from now: what it would have run while paused is not run.
#[utoipa::path(post, path = "/schedules/{schedule_id}/resume", operation_id = "resume_schedule_schedules__schedule_id__resume_post",
    tag = "schedules", summary = "Resume a schedule", params(("schedule_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = ScheduleInfo),
              (status = 404, description = "No such schedule"),
              (status = 422, description = "Its time has passed", body = HTTPValidationError)))]
async fn resume_schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<ScheduleInfo>> {
    let s = scheduler(&deps)?;
    let job = s.jobs().set_enabled(&id, true, utcnow()).map_err(refused)?;
    Ok(Json(info(s, &job)))
}

/// Run a schedule now, whatever its time and whether or not it is paused. The answer is
/// the run; one already waiting or going is returned instead of a second.
#[utoipa::path(post, path = "/schedules/{schedule_id}/run", operation_id = "run_schedule_schedules__schedule_id__run_post",
    tag = "schedules", summary = "Run a schedule now", params(("schedule_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = ScheduleRunInfo),
              (status = 404, description = "No such schedule")))]
async fn run_schedule(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<ScheduleRunInfo>> {
    let s = scheduler(&deps)?;
    let run = s.jobs().request_run(&id, utcnow()).map_err(refused)?;
    Ok(Json(run_info(&run)))
}

/// A schedule's runs, newest first, with what each answered.
#[utoipa::path(get, path = "/schedules/{schedule_id}/runs", operation_id = "schedule_runs_schedules__schedule_id__runs_get",
    tag = "schedules", summary = "A schedule's runs", params(("schedule_id" = String, Path,), RunsQuery),
    responses((status = 200, description = "Successful Response", body = ScheduleRunsView),
              (status = 404, description = "No such schedule"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn schedule_runs(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Qs(q): Qs<RunsQuery>,
) -> ApiResult<Json<ScheduleRunsView>> {
    let s = scheduler(&deps)?;
    if s.jobs().get(&id).map_err(refused)?.is_none() {
        return Err(ApiError::not_found(format!("there is no schedule {id}")));
    }
    let runs = s.jobs().runs(&id, q.limit.unwrap_or(20)).map_err(refused)?;
    Ok(Json(ScheduleRunsView {
        runs: runs.iter().map(run_info).collect(),
    }))
}

/// The runs of every schedule and of every watch that acted, newest first.
#[utoipa::path(get, path = "/runs", operation_id = "runs_runs_get", tag = "schedules",
    summary = "Recent runs", params(RunsQuery),
    responses((status = 200, description = "Successful Response", body = ScheduleRunsView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn runs(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<RunsQuery>,
) -> ApiResult<Json<ScheduleRunsView>> {
    let s = scheduler(&deps)?;
    let runs = s
        .jobs()
        .recent_runs(q.limit.unwrap_or(20))
        .map_err(refused)?;
    Ok(Json(ScheduleRunsView {
        runs: runs.iter().map(run_info).collect(),
    }))
}

/// Stop a run that is waiting or going.
#[utoipa::path(post, path = "/runs/{run_id}/cancel", operation_id = "cancel_run_runs__run_id__cancel_post",
    tag = "schedules", summary = "Stop a run", params(("run_id" = String, Path,)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such run, or it has ended")))]
async fn cancel_run(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let s = scheduler(&deps)?;
    if !s.cancel(&id) {
        return Err(ApiError::not_found(format!(
            "there is no run {id} waiting or going"
        )));
    }
    Ok(Json(Ack {
        ok: true,
        detail: None,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(schedules, add_schedule))
        .routes(routes!(schedule, edit_schedule, remove_schedule))
        .routes(routes!(pause_schedule))
        .routes(routes!(resume_schedule))
        .routes(routes!(run_schedule))
        .routes(routes!(schedule_runs))
        .routes(routes!(runs))
        .routes(routes!(cancel_run))
}
