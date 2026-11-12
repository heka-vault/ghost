//! What a session did between its turns, for the desktop's conversation screen (ADR 0058).
//!
//! `GET /activity?sessions=a,b` reads the activity the store kept beside the transcript
//! (calls, a handover, a cut, a failed turn: names only, never words), so a reload or an
//! earlier session shows them. `GET /calls/{id}` is what one call of the live session
//! returned, as the model was given it: fetched when the operator opens the call, never
//! pushed with an event, and only for the last calls, kept in memory.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, HTTPValidationError};
use crate::models::{ActivityKind, ActivityView, CallPreview};

/// How many sessions one request may ask about.
pub const SESSIONS_PER_REQUEST: usize = 40;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ActivityQuery {
    /// Session ids, comma separated, at most 40.
    pub sessions: String,
}

fn kind_of(k: &str) -> Option<ActivityKind> {
    Some(match k {
        "call" => ActivityKind::Call,
        "result" => ActivityKind::Result,
        "handover" => ActivityKind::Handover,
        "cut" => ActivityKind::Cut,
        "error" => ActivityKind::Error,
        _ => return None,
    })
}

/// The kinds the store keeps: what the record shows between turns. The operator's turn
/// opening and closing is live only.
pub fn kept(kind: ActivityKind) -> Option<&'static str> {
    Some(match kind {
        ActivityKind::Call => "call",
        ActivityKind::Result => "result",
        ActivityKind::Handover => "handover",
        ActivityKind::Cut => "cut",
        ActivityKind::Error => "error",
        ActivityKind::YouStarted | ActivityKind::YouStopped => return None,
    })
}

/// What the sessions did, oldest first per session: calls, handovers, cuts and failed
/// turns, by name. Kept as long as the session's transcript.
#[utoipa::path(get, path = "/activity", operation_id = "activity_activity_get", tag = "session",
    summary = "Activity",
    description = "What the sessions did between their turns, oldest first per session: calls, handovers, cuts and failed turns, by name. Kept as long as the session's transcript.",
    params(ActivityQuery),
    responses((status = 200, description = "Successful Response", body = Vec<ActivityView>),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn activity(
    State(deps): State<SharedDeps>,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<Vec<ActivityView>>> {
    let sessions: Vec<String> = q
        .sessions
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if sessions.len() > SESSIONS_PER_REQUEST {
        return Err(ApiError::invalid(
            &["query", "sessions"],
            "at most 40 sessions",
            "too_long",
        ));
    }
    let rows = deps
        .store
        .session_activity(&sessions)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .filter_map(|r| {
                Some(ActivityView {
                    kind: kind_of(&r.kind)?,
                    session_id: r.session_id,
                    at: r.at,
                    name: r.name,
                    id: r.call_id,
                    names: r.names,
                    secs: r.secs,
                })
            })
            .collect(),
    ))
}

/// What one call of the live session returned, as the model was given it (bounded and
/// redacted), cut to 2,000 characters. 404 once it is no longer kept.
#[utoipa::path(get, path = "/calls/{id}", operation_id = "call_calls_id_get", tag = "session",
    summary = "Call",
    description = "What one call of the live session returned, as the model was given it (bounded and redacted), cut to 2,000 characters. 404 once it is no longer kept.",
    params(("id" = String, Path, description = "The call's id, as `activity` carries it")),
    responses((status = 200, description = "Successful Response", body = CallPreview),
              (status = 404, description = "Not kept")))]
async fn call(
    State(deps): State<SharedDeps>,
    Path(id): Path<String>,
) -> ApiResult<Json<CallPreview>> {
    deps.state
        .call(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("that call's result is no longer kept"))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(activity))
        .routes(routes!(call))
}
