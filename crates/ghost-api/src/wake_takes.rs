//! Kept wake takes (ADR 0064): the sound of each time Ghost thought it heard its name, kept
//! on this computer to learn the operator's voice, and the operator's labels on them.
//!
//! These routes carry no audio: the list is metadata (the score, the time, the input, the
//! detector's score curve, what Parakeet read), and a take is played through Ghost's own
//! speaker on the desktop's request, as the Hearing test plays its takes. They are the
//! operator's only: the phone listener, the listener for outside tools and Ghost's own app
//! serve none of this (ADR 0013, ADR 0048).

use async_trait::async_trait;
use axum::Json;
use axum::extract::{Path as PathArg, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};
use crate::models::Ack;

/// The operator's choice: one switch, on unless they turned it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WakeTakesChoice {
    /// Keep the sound of each wake attempt (both streams), its score curve and what
    /// Parakeet read.
    pub keep: bool,
    /// The retraining counts an accepted wake with no label as the operator calling Ghost.
    #[serde(default)]
    pub unlabelled_accepted_are_me: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WakeTakeOutcome {
    /// Ghost woke.
    Accepted,
    /// The detector scored it and Parakeet's check turned it down.
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WakeTakeLabel {
    /// Me calling Ghost.
    Me,
    /// Not a wake.
    Not,
    /// Me calling Ghost, and I had called just before and it missed.
    MeMissedBefore,
}

/// One kept take, without its audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WakeTakeView {
    pub id: String,
    /// RFC 3339.
    pub at: String,
    pub outcome: WakeTakeOutcome,
    pub model: String,
    pub score: f32,
    pub threshold: f32,
    /// The input the detector heard.
    pub input: String,
    /// The array's own stream kept beside it, by node name.
    #[schema(required)]
    pub array: Option<String>,
    pub secs: f64,
    /// Where in the take the frame that scored ended, in seconds.
    pub hit_at: f64,
    /// The detector's score every 80 ms; the last one's window ended at `scores_end`.
    pub scores: Vec<f32>,
    pub scores_end: f64,
    /// What Parakeet read for it.
    #[schema(required)]
    pub reading: Option<String>,
    #[schema(required)]
    pub label: Option<WakeTakeLabel>,
    pub bytes: u64,
}

/// Everything kept, and the choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WakeTakesView {
    pub choice: WakeTakesChoice,
    pub count: usize,
    pub bytes: u64,
    pub max_count: usize,
    pub max_bytes: u64,
    /// The take playing, if any.
    #[schema(required)]
    pub playing: Option<String>,
    /// Newest first.
    pub takes: Vec<WakeTakeView>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct WakeTakeLabelRequest {
    /// None clears the label.
    #[serde(default)]
    pub label: Option<WakeTakeLabel>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct WakeTakePlayRequest {
    /// Play the array's own stream instead of the one the detector heard.
    #[serde(default)]
    pub array: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WakeTakesDeleted {
    pub deleted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTakesError {
    NotFound(String),
    Failed(String),
}

impl From<WakeTakesError> for ApiError {
    fn from(e: WakeTakesError) -> Self {
        match e {
            WakeTakesError::NotFound(m) => ApiError::not_found(m),
            WakeTakesError::Failed(m) => ApiError::internal(m),
        }
    }
}

pub type WakeTakesResult<T> = Result<T, WakeTakesError>;

/// What the daemon does for kept wake takes (`ghostd::wake_takes`).
#[async_trait]
pub trait WakeTakesPort: Send + Sync {
    fn view(&self) -> WakeTakesView;
    fn set_choice(&self, choice: WakeTakesChoice) -> WakeTakesResult<WakeTakesView>;
    fn label(&self, id: &str, label: Option<WakeTakeLabel>) -> WakeTakesResult<WakeTakeView>;
    fn delete(&self, id: &str) -> WakeTakesResult<()>;
    fn delete_all(&self) -> WakeTakesResult<usize>;
    /// Through Ghost's own speaker.
    async fn play(&self, id: &str, array: bool) -> WakeTakesResult<()>;
    async fn stop_playing(&self);
}

fn port(deps: &SharedDeps) -> ApiResult<&dyn WakeTakesPort> {
    deps.wake_takes
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("kept wake takes are not available in this daemon"))
}

fn check_id(id: &str) -> ApiResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::not_found("no such take"))
    }
}

/// Every kept take (no audio) and the choice.
#[utoipa::path(get, path = "/wake/takes", operation_id = "wake_takes_view_wake_takes_get",
    tag = "wake", summary = "Kept wake takes",
    responses((status = 200, description = "Successful Response", body = WakeTakesView)))]
async fn wake_takes_view(State(deps): State<SharedDeps>) -> ApiResult<Json<WakeTakesView>> {
    Ok(Json(port(&deps)?.view()))
}

/// Keep wake takes, or not. Turning it off keeps what is there: delete it to remove it.
#[utoipa::path(put, path = "/wake/takes/choice", operation_id = "wake_takes_choice_wake_takes_choice_put",
    tag = "wake", summary = "Keep wake takes", request_body = WakeTakesChoice,
    responses((status = 200, description = "Successful Response", body = WakeTakesView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn wake_takes_choice(
    State(deps): State<SharedDeps>,
    Body(req): Body<WakeTakesChoice>,
) -> ApiResult<Json<WakeTakesView>> {
    Ok(Json(port(&deps)?.set_choice(req)?))
}

/// Label a take.
#[utoipa::path(put, path = "/wake/takes/{id}/label", operation_id = "wake_take_label_wake_takes__id__label_put",
    tag = "wake", summary = "Label a take", params(("id" = String, Path)), request_body = WakeTakeLabelRequest,
    responses((status = 200, description = "Successful Response", body = WakeTakeView),
              (status = 404, description = "No such take"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn wake_take_label(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Body(req): Body<WakeTakeLabelRequest>,
) -> ApiResult<Json<WakeTakeView>> {
    check_id(&id)?;
    Ok(Json(port(&deps)?.label(&id, req.label)?))
}

/// Delete a take: every file of it.
#[utoipa::path(delete, path = "/wake/takes/{id}", operation_id = "wake_take_delete_wake_takes__id__delete",
    tag = "wake", summary = "Delete a take", params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such take")))]
async fn wake_take_delete(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    check_id(&id)?;
    port(&deps)?.delete(&id)?;
    Ok(Json(Ack::ok()))
}

/// Delete every kept take, and the folder.
#[utoipa::path(delete, path = "/wake/takes", operation_id = "wake_takes_delete_all_wake_takes_delete",
    tag = "wake", summary = "Delete every take",
    responses((status = 200, description = "Successful Response", body = WakeTakesDeleted)))]
async fn wake_takes_delete_all(
    State(deps): State<SharedDeps>,
) -> ApiResult<Json<WakeTakesDeleted>> {
    let deleted = port(&deps)?.delete_all()?;
    Ok(Json(WakeTakesDeleted { deleted }))
}

/// Play a take through Ghost's own speaker. No audio crosses this API.
#[utoipa::path(post, path = "/wake/takes/{id}/play", operation_id = "wake_take_play_wake_takes__id__play_post",
    tag = "wake", summary = "Play a take", params(("id" = String, Path)), request_body = Option<WakeTakePlayRequest>,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such take"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn wake_take_play(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    crate::error::OptionalBody(req): crate::error::OptionalBody<WakeTakePlayRequest>,
) -> ApiResult<Json<Ack>> {
    check_id(&id)?;
    port(&deps)?.play(&id, req.is_some_and(|r| r.array)).await?;
    Ok(Json(Ack::ok()))
}

/// Stop playing.
#[utoipa::path(post, path = "/wake/takes/play/stop", operation_id = "wake_takes_stop_wake_takes_play_stop_post",
    tag = "wake", summary = "Stop playing",
    responses((status = 200, description = "Successful Response", body = Ack)))]
async fn wake_takes_stop(State(deps): State<SharedDeps>) -> ApiResult<Json<Ack>> {
    port(&deps)?.stop_playing().await;
    Ok(Json(Ack::ok()))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(wake_takes_view, wake_takes_delete_all))
        .routes(routes!(wake_takes_choice))
        .routes(routes!(wake_take_label))
        .routes(routes!(wake_take_delete))
        .routes(routes!(wake_take_play))
        .routes(routes!(wake_takes_stop))
}
