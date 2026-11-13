//! The realtime voice's routes (ADR 0062): what is chosen, what turning it on changes, the
//! day's spend, and the connection's check.
//!
//! The mode and its settings are saved like any setting, through `PUT /settings` (the
//! `GHOST_VOICE_MODE` and `GHOST_REALTIME_*` values in settings.env); its key is a password
//! in Ghost's vault, saved on its own (`PUT /passwords/{name}`) and never shown. Only the
//! operator reaches these routes: no tool, app, model or outside tool has a way to them.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use ghost_core::realtime::settings::{PRICES_AS_OF, PROVIDERS, disclosure};
use ghost_core::realtime::{RealtimeSettings, Spend};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RealtimeModelView {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RealtimeProviderView {
    pub id: String,
    pub title: String,
    pub models: Vec<RealtimeModelView>,
    pub default_model: String,
    pub voices: Vec<String>,
    pub default_voice: String,
    /// The password's name it reads by default.
    pub password: String,
    /// What turning it on changes, in plain words, shown before it is on.
    pub disclosure: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RealtimeSpendView {
    /// The local date the figures are for.
    pub day: String,
    /// Today's estimate in US dollars.
    pub usd: f64,
    pub sessions: u64,
    /// The last realtime session's estimate in US dollars.
    pub last_session_usd: f64,
    /// When the prices the estimate uses were read from the provider.
    pub prices_as_of: String,
    /// Whether Ghost knows the chosen model's prices (an unknown model has no estimate).
    pub priced: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RealtimeCheckView {
    pub checked_at: chrono::DateTime<chrono::Utc>,
    pub provider: String,
    pub model: String,
    pub reachable: bool,
    /// In plain words, never with the key.
    pub error: Option<String>,
    pub connect_ms: Option<u64>,
    /// From asking for a line to its first audio.
    pub first_audio_ms: Option<u64>,
    /// Whether a request was handed to Ghost's agent; null when not tried.
    pub tools: Option<bool>,
    /// What the check itself cost, estimated, in US dollars.
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RealtimeView {
    /// `local` (the default) or `realtime`.
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub voice: String,
    /// The password's name; whether the vault holds it.
    pub password: String,
    pub password_set: bool,
    pub daily_cap_usd: Option<f64>,
    pub providers: Vec<RealtimeProviderView>,
    pub spend: RealtimeSpendView,
    pub last_check: Option<RealtimeCheckView>,
    /// A saved setting that does not read, in plain words.
    pub problem: Option<String>,
}

pub fn spend_view(s: Spend, priced: bool) -> RealtimeSpendView {
    RealtimeSpendView {
        day: s.day,
        usd: s.usd,
        sessions: s.sessions,
        last_session_usd: s.last_session_usd,
        prices_as_of: PRICES_AS_OF.to_string(),
        priced,
    }
}

fn env_of(deps: &Deps) -> HashMap<String, String> {
    let mut env = deps.environ();
    for (k, v) in deps.settings() {
        if ghost_core::realtime::settings::KEYS.contains(&k.as_str()) {
            env.insert(k, v);
        }
    }
    env
}

pub fn realtime_view(deps: &Deps) -> RealtimeView {
    let env = env_of(deps);
    let (settings, problem) = match RealtimeSettings::from_vars(|k| env.get(k).cloned()) {
        Ok(s) => (s, None),
        Err(e) => (RealtimeSettings::default(), Some(e)),
    };
    let password_set = deps
        .passwords
        .as_ref()
        .is_some_and(|v| v.is_open() && v.contains(&settings.password))
        || env.get(&settings.password).is_some_and(|v| !v.is_empty());
    let priced = settings.prices().is_some();
    RealtimeView {
        mode: if settings.on { "realtime" } else { "local" }.into(),
        provider: settings.provider.clone(),
        model: settings.model.clone(),
        voice: settings.voice.clone(),
        password: settings.password.clone(),
        password_set,
        daily_cap_usd: settings.daily_cap_usd,
        providers: PROVIDERS
            .iter()
            .map(|p| RealtimeProviderView {
                id: p.id.into(),
                title: p.title.into(),
                models: p
                    .models
                    .iter()
                    .map(|m| RealtimeModelView {
                        id: m.id.into(),
                        title: m.title.into(),
                    })
                    .collect(),
                default_model: p.default_model.into(),
                voices: p.voices.iter().map(|v| v.to_string()).collect(),
                default_voice: p.default_voice.into(),
                password: p.password.into(),
                disclosure: disclosure(p),
            })
            .collect(),
        spend: spend_view(deps.control.realtime_spend().unwrap_or_default(), priced),
        last_check: deps.control.last_realtime_check(),
        problem,
    }
}

/// The realtime voice: whether it is chosen, what turning it on changes, today's spend.
#[utoipa::path(get, path = "/voice/realtime", operation_id = "realtime_voice_get", tag = "voice",
    summary = "Realtime Voice",
    description = "The realtime voice (ADR 0062): the mode (local by default), the provider, model,\nvoice and password's name, whether the vault holds it, the daily cap, what turning it on\nchanges in plain words, today's spend estimate and the latest check. Saved through\nPUT /settings; the key through the passwords.",
    responses((status = 200, description = "Successful Response", body = RealtimeView)))]
async fn get_realtime(State(deps): State<SharedDeps>) -> Json<RealtimeView> {
    Json(realtime_view(&deps))
}

/// Check the realtime connection with the saved settings.
#[utoipa::path(post, path = "/voice/realtime/check", operation_id = "realtime_voice_check_post", tag = "voice",
    summary = "Realtime Voice Check",
    description = "Check the realtime connection with the saved settings (ADR 0062): whether it\nanswers, the time to its first audio for a fixed line, and whether a fixed request is\nhanded to Ghost's agent. Nothing of the operator's is sent; the key is read from the\nvault by the daemon and never shown. Costs a fraction of a cent.",
    responses((status = 200, description = "Successful Response", body = RealtimeCheckView),
              (status = 503, description = "Ghost can't check the realtime voice now")))]
async fn check_realtime(State(deps): State<SharedDeps>) -> ApiResult<Json<RealtimeCheckView>> {
    deps.control
        .check_realtime()
        .await
        .map(Json)
        .ok_or_else(|| ApiError::unavailable("Ghost can't check the realtime voice right now"))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_realtime))
        .routes(routes!(check_realtime))
}
