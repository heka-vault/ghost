//! The speech models the bundle does not carry (ADR 0066): Parakeet, which hears, and
//! Kokoro, which speaks, 2.9 GB together. The app downloads them in its first run and in
//! Settings, through these routes, with progress and a clear failure; a person with the
//! .deb never needs a script from the checkout.
//!
//! Each file is pinned by SHA-256 (`ghostd::speech_models`); nothing here carries audio,
//! a transcript or a password, and the routes are the operator's only: the phone listener,
//! the listener for outside tools and Ghost's own app serve none of this.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult};

/// One file this computer does not have yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SpeechModelFile {
    /// `Parakeet` (hearing) or `Kokoro` (the voice).
    pub model: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpeechModelsFetchState {
    /// Nothing asked for since the daemon started.
    Idle,
    Fetching,
    /// The last download stopped; `error` says why. What was fetched whole is kept, and a
    /// file cut off part way resumes where it stopped.
    Failed,
    /// Every file is here and matched its pin.
    Done,
}

/// The download, as it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SpeechModelsFetch {
    pub state: SpeechModelsFetchState,
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// The file being fetched.
    #[schema(required)]
    pub file: Option<String>,
    /// In plain words, when it failed.
    #[schema(required)]
    pub error: Option<String>,
}

impl SpeechModelsFetch {
    pub fn idle() -> Self {
        Self {
            state: SpeechModelsFetchState::Idle,
            done_bytes: 0,
            total_bytes: 0,
            file: None,
            error: None,
        }
    }
}

/// What this computer has, and the download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SpeechModelsView {
    /// The daemon has loaded its speech models: Ghost can hear and speak.
    pub ready: bool,
    /// The files that are not here yet; empty when nothing is missing.
    pub missing: Vec<SpeechModelFile>,
    pub missing_bytes: u64,
    /// Where they go.
    pub folder: String,
    /// Free space there, when it can be read.
    #[schema(required)]
    pub free_bytes: Option<u64>,
    pub fetch: SpeechModelsFetch,
}

/// What the daemon does for the speech models (`ghostd::speech_models`).
pub trait SpeechModelsPort: Send + Sync {
    fn view(&self) -> SpeechModelsView;
    /// Start the download of what is missing, in the background; a download already
    /// running is left to run. Err in plain words when it cannot start.
    fn fetch(&self) -> Result<SpeechModelsView, String>;
}

fn port(deps: &SharedDeps) -> ApiResult<&dyn SpeechModelsPort> {
    deps.speech_models
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("the speech models are not managed by this daemon"))
}

/// What is missing, and the download.
#[utoipa::path(get, path = "/speech-models", operation_id = "speech_models_view_speech_models_get",
    tag = "speech-models", summary = "The speech models this computer has",
    responses((status = 200, description = "Successful Response", body = SpeechModelsView)))]
async fn speech_models_view(State(deps): State<SharedDeps>) -> ApiResult<Json<SpeechModelsView>> {
    Ok(Json(port(&deps)?.view()))
}

/// Download what is missing, each file checked against its pin before it is used.
#[utoipa::path(post, path = "/speech-models/fetch", operation_id = "speech_models_fetch_speech_models_fetch_post",
    tag = "speech-models", summary = "Download the speech models",
    responses((status = 200, description = "Successful Response", body = SpeechModelsView),
              (status = 409, description = "It cannot start")))]
async fn speech_models_fetch(State(deps): State<SharedDeps>) -> ApiResult<Json<SpeechModelsView>> {
    port(&deps)?.fetch().map(Json).map_err(ApiError::conflict)
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(speech_models_view))
        .routes(routes!(speech_models_fetch))
}
