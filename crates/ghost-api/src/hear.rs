//! The Hearing test (ADR 0042): the operator records takes of their own voice, checks each
//! one, and scores how well Ghost hears them, from the desktop app.
//!
//! The recorder holds the microphone while it is open: Ghost is not listening for its name
//! then. A take is held (the key down to the key up, half a second kept either side), ends
//! on silence after a countdown, or runs the plan's time. Each take is checked at once:
//! what Ghost heard against the line, on the screen only. Takes are played through Ghost's
//! own speaker; no audio crosses this API. While the recorder is open, the `hear` event
//! carries the mic's level about twenty times a second; never at any other time, and never
//! passive audio (ADR 0013, ADR 0010).

use async_trait::async_trait;
use axum::Json;
use axum::extract::{Path as PathArg, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::Qs;
use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, OptionalBody, check_len};
use crate::models::Ack;

// --- what the recorder is doing ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HearState {
    /// Not open: Ghost has the mic.
    Closed,
    /// Waiting for the mic to be handed over.
    Opening,
    /// Open, between takes.
    Ready,
    /// Counting down to a take that starts on its own.
    Countdown,
    Recording,
    /// Keeping the half second after the release.
    Stopping,
    /// Reading the take for its check.
    Checking,
}

/// How a take ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeEnding {
    /// The operator holds the key: the release ends it.
    Hold,
    /// After a countdown; ends two seconds after the last word (across the room).
    Quiet,
    /// A silent take: the plan's seconds.
    Timed,
}

/// A score run in progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearScoring {
    pub id: String,
    pub session: String,
    pub channel: String,
    pub done: usize,
    pub total: usize,
}

/// The recorder now. Also the `hear` event's body, with the level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearView {
    pub state: HearState,
    #[schema(required)]
    pub session: Option<String>,
    /// The take being recorded or checked.
    #[schema(required)]
    pub take: Option<String>,
    #[schema(required)]
    pub ending: Option<TakeEnding>,
    /// Seconds left in a countdown.
    #[schema(required)]
    pub countdown: Option<u32>,
    /// Seconds into the take.
    #[serde(default)]
    pub elapsed: f64,
    /// The raw array recorded beside `ghost.mic`, by node name; None when there is none.
    #[schema(required)]
    pub raw_device: Option<String>,
    /// Why the recorder can't open now, for the operator ("Ghost is in a session").
    #[schema(required)]
    pub unavailable: Option<String>,
    /// A run being scored.
    #[schema(required)]
    pub scoring: Option<HearScoring>,
    /// Whether a take is playing back.
    #[serde(default)]
    pub playing: bool,
    /// The take the last check is for (`GET /hear/check`), while the recorder is open.
    #[schema(required)]
    pub checked: Option<String>,
    /// The last thing that went wrong (a take that could not be checked, a run that
    /// failed), for the operator; cleared by the next success.
    #[schema(required)]
    pub problem: Option<String>,
}

impl HearView {
    pub fn closed(unavailable: Option<String>) -> Self {
        Self {
            state: HearState::Closed,
            session: None,
            take: None,
            ending: None,
            countdown: None,
            elapsed: 0.0,
            raw_device: None,
            unavailable,
            scoring: None,
            playing: false,
            checked: None,
            problem: None,
        }
    }
}

// --- the check -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HearWordOp {
    Same,
    Misheard,
    Missed,
    Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearWord {
    pub op: HearWordOp,
    #[schema(required)]
    pub said: Option<String>,
    #[schema(required)]
    pub heard: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearWarning {
    /// `too_quiet`, `clipping`, `no_speech`, `speech_in_silence`, `started_early`,
    /// `cut_off`, `missed_end`.
    pub kind: String,
    pub text: String,
    /// The take is wrong, not just worth a look: redo it.
    pub redo: bool,
}

/// A take's check: shown once, never kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearCheck {
    pub session: String,
    pub take: String,
    pub seconds: f64,
    pub rms: f64,
    pub peak: f64,
    /// Where the VAD hears speech, seconds into the take.
    #[schema(required)]
    pub speech_start: Option<f64>,
    #[schema(required)]
    pub speech_end: Option<f64>,
    /// What Ghost heard, as the score compares it: lowercase, numbers as digits.
    pub heard: String,
    pub words: Vec<HearWord>,
    pub matched: usize,
    pub said: usize,
    pub warnings: Vec<HearWarning>,
}

// --- the plan ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HearKind {
    /// A request.
    Speech,
    /// A word or two Ghost must not drop.
    Reply,
    /// Nothing said.
    Silence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearPlanTake {
    pub id: String,
    pub kind: HearKind,
    /// The line; " ... " is a pause of about a second.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Names Ghost must hear, each in the line.
    #[serde(default)]
    pub terms: Vec<String>,
    /// A silent take's instruction.
    #[serde(default)]
    pub say: Option<String>,
    /// A silent take's length.
    #[serde(default)]
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearBlock {
    pub block: String,
    pub condition: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub instruction: String,
    /// Across the room: takes start after a countdown and end on silence.
    #[serde(default)]
    pub away: bool,
    /// Music on the desk speakers.
    #[serde(default)]
    pub music: bool,
    pub takes: Vec<HearPlanTake>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearPlan {
    pub version: u32,
    #[serde(default)]
    pub about: String,
    pub blocks: Vec<HearBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearPlanView {
    pub plan: HearPlan,
    /// The operator's own plan; false: the one Ghost ships.
    pub edited: bool,
}

// --- sessions ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearSessionSummary {
    pub name: String,
    pub recorded: usize,
    pub skipped: usize,
    pub raw: usize,
    /// Takes whose check asked for a redo.
    pub flagged: usize,
    pub seconds: f64,
    #[schema(required)]
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HearTakeState {
    Recorded,
    Skipped,
}

/// A take in a session, as its manifest line says: the line asked for, never what was
/// heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearTake {
    pub id: String,
    pub block: String,
    pub kind: HearKind,
    pub text: String,
    pub state: HearTakeState,
    #[schema(required)]
    pub seconds: Option<f64>,
    #[schema(required)]
    pub peak: Option<f64>,
    #[schema(required)]
    pub speech_start: Option<f64>,
    #[schema(required)]
    pub speech_end: Option<f64>,
    #[schema(required)]
    pub ending: Option<String>,
    /// The check's warnings by kind.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The take has a raw-array file.
    pub raw: bool,
    #[schema(required)]
    pub time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearSession {
    pub name: String,
    pub takes: Vec<HearTake>,
}

// --- runs and their reports ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearRunWer {
    #[schema(required)]
    pub stage: Option<f64>,
    #[schema(required)]
    pub turn: Option<f64>,
    #[schema(required)]
    pub file: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearRunInfo {
    pub id: String,
    pub session: String,
    /// `aec` (`ghost.mic`) or `raw` (the array under it).
    pub channel: String,
    pub created: String,
    pub commit: String,
    pub takes: usize,
    pub wer: HearRunWer,
    /// The operator's note: what this run was for.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HearVerdict {
    Better,
    Same,
    Worse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearRow {
    pub cells: Vec<String>,
    /// A comparison row's verdict.
    #[schema(required)]
    pub verdict: Option<HearVerdict>,
    /// The take a row is about, `<id>|<source>`.
    #[schema(required)]
    pub take: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearTable {
    pub columns: Vec<String>,
    pub rows: Vec<HearRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearFact {
    pub name: String,
    pub value: String,
}

/// A part of a report section, in order. Text may hold `code` in backticks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HearPart {
    Text {
        text: String,
    },
    Table(HearTable),
    Facts {
        facts: Vec<HearFact>,
    },
    /// The pass rule's verdict for `stage` against the baseline.
    Pass {
        pass: bool,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearSection {
    /// 2 for a section, 3 for one inside the section before.
    pub level: u8,
    pub title: String,
    pub parts: Vec<HearPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearReport {
    pub meta: Vec<HearFact>,
    pub intro: String,
    pub sections: Vec<HearSection>,
    /// The pass rule, when compared against a baseline.
    #[schema(required)]
    pub pass: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HearRun {
    pub info: HearRunInfo,
    #[schema(required)]
    pub baseline: Option<HearRunInfo>,
    pub report: HearReport,
}

// --- the port ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HearError {
    /// The recorder is not open, or a take is already going: 409.
    Conflict(String),
    /// No such take, session or run: 404.
    NotFound(String),
    /// What the operator sent is wrong: 422.
    Invalid(String),
    /// Ghost can't now (a session, no models yet): 503.
    Unavailable(String),
    Failed(String),
}

impl From<HearError> for ApiError {
    fn from(e: HearError) -> Self {
        match e {
            HearError::Conflict(m) => ApiError::conflict(m),
            HearError::NotFound(m) => ApiError::not_found(m),
            HearError::Invalid(m) => ApiError::unprocessable(m),
            HearError::Unavailable(m) => ApiError::unavailable(m),
            HearError::Failed(m) => ApiError::internal(m),
        }
    }
}

pub type HearResult<T> = Result<T, HearError>;

/// What the daemon does for the Hearing test (`ghostd::hear`).
#[async_trait]
pub trait HearPort: Send + Sync {
    fn view(&self) -> HearView;
    /// Take the mic from the passive loop; `session`: carry one on. Unset: today's
    /// newest session, or a new one; `new`: a new one.
    async fn open(&self, session: Option<String>, new: bool) -> HearResult<HearView>;
    /// Give the mic back.
    async fn close(&self) -> HearView;
    async fn start_take(&self, take: &str, ending: TakeEnding) -> HearResult<HearView>;
    /// End a held take: waits for the half second after, then the check.
    async fn stop_take(&self) -> HearResult<HearCheck>;
    async fn cancel_take(&self) -> HearResult<HearView>;
    /// The last take's check, while the recorder is open.
    fn last_check(&self) -> Option<HearCheck>;
    async fn skip_take(&self, take: &str) -> HearResult<HearSession>;

    fn plan(&self) -> HearResult<HearPlanView>;
    fn save_plan(&self, plan: HearPlan) -> HearResult<HearPlanView>;
    fn reset_plan(&self) -> HearResult<HearPlanView>;

    fn sessions(&self) -> Vec<HearSessionSummary>;
    fn session(&self, name: &str) -> HearResult<HearSession>;
    fn delete_session(&self, name: &str) -> HearResult<()>;

    /// Play a take through Ghost's own output; `channel` `aec` or `raw`.
    async fn play(&self, session: &str, take: &str, channel: &str) -> HearResult<()>;
    async fn stop_playing(&self);

    /// Score a session; progress on the `hear` event.
    async fn start_run(&self, session: &str, channel: &str) -> HearResult<HearRunInfo>;
    fn runs(&self) -> Vec<HearRunInfo>;
    fn run(&self, id: &str, baseline: Option<&str>) -> HearResult<HearRun>;
    fn note_run(&self, id: &str, note: &str) -> HearResult<HearRunInfo>;
    fn delete_run(&self, id: &str) -> HearResult<()>;
}

fn port(deps: &SharedDeps) -> ApiResult<&dyn HearPort> {
    deps.hear
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("the Hearing test is not available in this daemon"))
}

// --- the routes ----------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearOpenRequest {
    /// Carry this session on. Unset: today's newest session, or a new one.
    #[serde(default)]
    pub session: Option<String>,
    /// Start a new session, even if there is one from today.
    #[serde(default)]
    pub new: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearTakeRequest {
    pub take: String,
    pub ending: TakeEnding,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearSkipRequest {
    pub take: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearPlayRequest {
    pub session: String,
    pub take: String,
    /// `aec` (the default) or `raw`.
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearRunRequest {
    pub session: String,
    /// `aec` (the default) or `raw`.
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HearNoteRequest {
    pub note: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HearRunQuery {
    /// Another run's id to compare against.
    pub baseline: Option<String>,
}

fn channel(c: Option<&str>) -> ApiResult<String> {
    match c.unwrap_or("aec") {
        c @ ("aec" | "raw") => Ok(c.to_string()),
        other => Err(ApiError::unprocessable(format!(
            "channel is aec or raw, not {other:?}"
        ))),
    }
}

/// The recorder now.
#[utoipa::path(get, path = "/hear", operation_id = "hear_view_hear_get", tag = "hear",
    summary = "Hearing test", responses((status = 200, description = "Successful Response", body = HearView)))]
async fn hear_view(State(deps): State<SharedDeps>) -> ApiResult<Json<HearView>> {
    Ok(Json(port(&deps)?.view()))
}

/// Open the recorder: it takes the mic, and Ghost stops listening for its name until it
/// closes.
#[utoipa::path(post, path = "/hear/open", operation_id = "hear_open_hear_open_post", tag = "hear",
    summary = "Open the recorder", request_body = Option<HearOpenRequest>,
    responses((status = 200, description = "Successful Response", body = HearView),
              (status = 409, description = "Busy"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_open(
    State(deps): State<SharedDeps>,
    OptionalBody(req): OptionalBody<HearOpenRequest>,
) -> ApiResult<Json<HearView>> {
    let (session, new) = req.map_or((None, false), |r| (r.session, r.new));
    if let Some(s) = &session {
        check_len(&["body", "session"], s, 1, 64)?;
    }
    if new && session.is_some() {
        return Err(ApiError::unprocessable("new or session, not both"));
    }
    Ok(Json(port(&deps)?.open(session, new).await?))
}

/// Close the recorder: Ghost has the mic again.
#[utoipa::path(post, path = "/hear/close", operation_id = "hear_close_hear_close_post", tag = "hear",
    summary = "Close the recorder", responses((status = 200, description = "Successful Response", body = HearView)))]
async fn hear_close(State(deps): State<SharedDeps>) -> ApiResult<Json<HearView>> {
    Ok(Json(port(&deps)?.close().await))
}

/// Start a take.
#[utoipa::path(post, path = "/hear/take/start", operation_id = "hear_start_take_hear_take_start_post",
    tag = "hear", summary = "Start a take", request_body = HearTakeRequest,
    responses((status = 200, description = "Successful Response", body = HearView),
              (status = 404, description = "No such take"), (status = 409, description = "Not open, or a take is going"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_start_take(
    State(deps): State<SharedDeps>,
    Body(req): Body<HearTakeRequest>,
) -> ApiResult<Json<HearView>> {
    check_len(&["body", "take"], &req.take, 1, 16)?;
    Ok(Json(port(&deps)?.start_take(&req.take, req.ending).await?))
}

/// End a held take; the answer is its check.
#[utoipa::path(post, path = "/hear/take/stop", operation_id = "hear_stop_take_hear_take_stop_post",
    tag = "hear", summary = "End a take",
    responses((status = 200, description = "Successful Response", body = HearCheck),
              (status = 409, description = "No take is going")))]
async fn hear_stop_take(State(deps): State<SharedDeps>) -> ApiResult<Json<HearCheck>> {
    Ok(Json(port(&deps)?.stop_take().await?))
}

/// Throw the take away.
#[utoipa::path(post, path = "/hear/take/cancel", operation_id = "hear_cancel_take_hear_take_cancel_post",
    tag = "hear", summary = "Cancel a take",
    responses((status = 200, description = "Successful Response", body = HearView),
              (status = 409, description = "No take is going")))]
async fn hear_cancel_take(State(deps): State<SharedDeps>) -> ApiResult<Json<HearView>> {
    Ok(Json(port(&deps)?.cancel_take().await?))
}

/// The last take's check, while the recorder is open.
#[utoipa::path(get, path = "/hear/check", operation_id = "hear_check_hear_check_get", tag = "hear",
    summary = "The last check",
    responses((status = 200, description = "Successful Response", body = HearCheck),
              (status = 404, description = "No check")))]
async fn hear_check(State(deps): State<SharedDeps>) -> ApiResult<Json<HearCheck>> {
    port(&deps)?
        .last_check()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no take has been checked"))
}

/// Skip a take in the open session.
#[utoipa::path(post, path = "/hear/take/skip", operation_id = "hear_skip_take_hear_take_skip_post",
    tag = "hear", summary = "Skip a take", request_body = HearSkipRequest,
    responses((status = 200, description = "Successful Response", body = HearSession),
              (status = 404, description = "No such take"), (status = 409, description = "Not open"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_skip_take(
    State(deps): State<SharedDeps>,
    Body(req): Body<HearSkipRequest>,
) -> ApiResult<Json<HearSession>> {
    check_len(&["body", "take"], &req.take, 1, 16)?;
    Ok(Json(port(&deps)?.skip_take(&req.take).await?))
}

/// The take list: the operator's, or the one Ghost ships.
#[utoipa::path(get, path = "/hear/plan", operation_id = "hear_plan_hear_plan_get", tag = "hear",
    summary = "The take list", responses((status = 200, description = "Successful Response", body = HearPlanView)))]
async fn hear_plan(State(deps): State<SharedDeps>) -> ApiResult<Json<HearPlanView>> {
    Ok(Json(port(&deps)?.plan()?))
}

/// Save the operator's take list; refused, with why, when it doesn't hold together.
#[utoipa::path(put, path = "/hear/plan", operation_id = "hear_save_plan_hear_plan_put", tag = "hear",
    summary = "Save the take list", request_body = HearPlan,
    responses((status = 200, description = "Successful Response", body = HearPlanView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_save_plan(
    State(deps): State<SharedDeps>,
    Body(plan): Body<HearPlan>,
) -> ApiResult<Json<HearPlanView>> {
    Ok(Json(port(&deps)?.save_plan(plan)?))
}

/// Back to the take list Ghost ships.
#[utoipa::path(delete, path = "/hear/plan", operation_id = "hear_reset_plan_hear_plan_delete", tag = "hear",
    summary = "Reset the take list", responses((status = 200, description = "Successful Response", body = HearPlanView)))]
async fn hear_reset_plan(State(deps): State<SharedDeps>) -> ApiResult<Json<HearPlanView>> {
    Ok(Json(port(&deps)?.reset_plan()?))
}

/// Every session, newest first.
#[utoipa::path(get, path = "/hear/sessions", operation_id = "hear_sessions_hear_sessions_get", tag = "hear",
    summary = "Sessions", responses((status = 200, description = "Successful Response", body = Vec<HearSessionSummary>)))]
async fn hear_sessions(State(deps): State<SharedDeps>) -> ApiResult<Json<Vec<HearSessionSummary>>> {
    Ok(Json(port(&deps)?.sessions()))
}

/// A session's takes.
#[utoipa::path(get, path = "/hear/sessions/{name}", operation_id = "hear_session_hear_sessions__name__get",
    tag = "hear", summary = "A session", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = HearSession),
              (status = 404, description = "No such session")))]
async fn hear_session(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<HearSession>> {
    Ok(Json(port(&deps)?.session(&name)?))
}

/// Delete a session and its audio.
#[utoipa::path(delete, path = "/hear/sessions/{name}", operation_id = "hear_delete_session_hear_sessions__name__delete",
    tag = "hear", summary = "Delete a session", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such session"), (status = 409, description = "Open in the recorder")))]
async fn hear_delete_session(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    port(&deps)?.delete_session(&name)?;
    Ok(Json(Ack::ok()))
}

/// Play a take through Ghost's own speaker.
#[utoipa::path(post, path = "/hear/play", operation_id = "hear_play_hear_play_post", tag = "hear",
    summary = "Play a take", request_body = HearPlayRequest,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such take"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_play(
    State(deps): State<SharedDeps>,
    Body(req): Body<HearPlayRequest>,
) -> ApiResult<Json<Ack>> {
    let ch = channel(req.channel.as_deref())?;
    port(&deps)?.play(&req.session, &req.take, &ch).await?;
    Ok(Json(Ack::ok()))
}

/// Stop playing.
#[utoipa::path(post, path = "/hear/play/stop", operation_id = "hear_stop_playing_hear_play_stop_post",
    tag = "hear", summary = "Stop playing", responses((status = 200, description = "Successful Response", body = Ack)))]
async fn hear_stop_playing(State(deps): State<SharedDeps>) -> ApiResult<Json<Ack>> {
    port(&deps)?.stop_playing().await;
    Ok(Json(Ack::ok()))
}

/// Every score run, newest first.
#[utoipa::path(get, path = "/hear/runs", operation_id = "hear_runs_hear_runs_get", tag = "hear",
    summary = "Score runs", responses((status = 200, description = "Successful Response", body = Vec<HearRunInfo>)))]
async fn hear_runs(State(deps): State<SharedDeps>) -> ApiResult<Json<Vec<HearRunInfo>>> {
    Ok(Json(port(&deps)?.runs()))
}

/// Score a session. The run goes on in the daemon; its progress is on the `hear` event.
#[utoipa::path(post, path = "/hear/runs", operation_id = "hear_start_run_hear_runs_post", tag = "hear",
    summary = "Score a session", request_body = HearRunRequest,
    responses((status = 200, description = "Successful Response", body = HearRunInfo),
              (status = 404, description = "No such session"), (status = 409, description = "A run is going"),
              (status = 503, description = "Not now"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_start_run(
    State(deps): State<SharedDeps>,
    Body(req): Body<HearRunRequest>,
) -> ApiResult<Json<HearRunInfo>> {
    let ch = channel(req.channel.as_deref())?;
    Ok(Json(port(&deps)?.start_run(&req.session, &ch).await?))
}

/// A run's report, against another run when `baseline` is given.
#[utoipa::path(get, path = "/hear/runs/{id}", operation_id = "hear_run_hear_runs__id__get", tag = "hear",
    summary = "A run's report", params(("id" = String, Path), HearRunQuery),
    responses((status = 200, description = "Successful Response", body = HearRun),
              (status = 404, description = "No such run")))]
async fn hear_run(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Qs(q): Qs<HearRunQuery>,
) -> ApiResult<Json<HearRun>> {
    Ok(Json(port(&deps)?.run(&id, q.baseline.as_deref())?))
}

/// Note what a run was for.
#[utoipa::path(put, path = "/hear/runs/{id}/note", operation_id = "hear_note_run_hear_runs__id__note_put",
    tag = "hear", summary = "Note a run", params(("id" = String, Path)), request_body = HearNoteRequest,
    responses((status = 200, description = "Successful Response", body = HearRunInfo),
              (status = 404, description = "No such run"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn hear_note_run(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Body(req): Body<HearNoteRequest>,
) -> ApiResult<Json<HearRunInfo>> {
    check_len(&["body", "note"], &req.note, 0, 500)?;
    Ok(Json(port(&deps)?.note_run(&id, &req.note)?))
}

/// Delete a run.
#[utoipa::path(delete, path = "/hear/runs/{id}", operation_id = "hear_delete_run_hear_runs__id__delete",
    tag = "hear", summary = "Delete a run", params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such run")))]
async fn hear_delete_run(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    port(&deps)?.delete_run(&id)?;
    Ok(Json(Ack::ok()))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(hear_view))
        .routes(routes!(hear_open))
        .routes(routes!(hear_close))
        .routes(routes!(hear_start_take))
        .routes(routes!(hear_stop_take))
        .routes(routes!(hear_cancel_take))
        .routes(routes!(hear_check))
        .routes(routes!(hear_skip_take))
        .routes(routes!(hear_plan, hear_save_plan, hear_reset_plan))
        .routes(routes!(hear_sessions))
        .routes(routes!(hear_session, hear_delete_session))
        .routes(routes!(hear_play))
        .routes(routes!(hear_stop_playing))
        .routes(routes!(hear_runs, hear_start_run))
        .routes(routes!(hear_run, hear_delete_run))
        .routes(routes!(hear_note_run))
}
