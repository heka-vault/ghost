//! How much Ghost read and did (ADR 0060): `GET /reach` totals the calls agents made in a
//! stretch of time, by app, by agent and by session or run, as counts and sizes. Never a
//! call's arguments, its result or any words: the store has no column for them.

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, HTTPValidationError};
use crate::models::ReachView;

/// The longest stretch one request may ask about.
pub const REACH_MAX_DAYS: i64 = 31;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReachQuery {
    /// From this time (RFC 3339), inclusive.
    pub since: DateTime<Utc>,
    /// To this time, exclusive; now when left out.
    pub until: Option<DateTime<Utc>>,
    /// Only these sessions or runs (`run-<id>`), comma separated, at most 40.
    pub sessions: Option<String>,
}

/// How much Ghost read and did between two times: the calls agents made in sessions and
/// runs, totalled, by app, by agent and by session or run. Counts and sizes only.
#[utoipa::path(get, path = "/reach", operation_id = "reach_reach_get", tag = "session",
    summary = "Reach",
    description = "How much Ghost read and did between two times (at most 31 days): the calls agents made in sessions and runs, totalled, by app, by agent and by session or run (`run-<id>`). Counts and sizes only, never contents.",
    params(ReachQuery),
    responses((status = 200, description = "Successful Response", body = ReachView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn reach(
    State(deps): State<SharedDeps>,
    Query(q): Query<ReachQuery>,
) -> ApiResult<Json<ReachView>> {
    let until = q.until.unwrap_or_else(|| Utc::now() + Duration::seconds(1));
    if until <= q.since {
        return Err(ApiError::invalid(
            &["query", "until"],
            "until must be after since",
            "value_error",
        ));
    }
    if until - q.since > Duration::days(REACH_MAX_DAYS) {
        return Err(ApiError::invalid(
            &["query", "since"],
            "at most 31 days at a time",
            "value_error",
        ));
    }
    let sessions: Vec<String> = q
        .sessions
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if sessions.len() > crate::calls::SESSIONS_PER_REQUEST {
        return Err(ApiError::invalid(
            &["query", "sessions"],
            "at most 40 sessions",
            "too_long",
        ));
    }
    let rows = deps
        .store
        .reach_between(q.since, until, &sessions)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let s = ghost_core::reach::summarize(&rows);
    Ok(Json(ReachView {
        since: q.since,
        until,
        total: s.total.into(),
        by_app: s.by_app.into_iter().map(Into::into).collect(),
        by_agent: s.by_agent.into_iter().map(Into::into).collect(),
        by_session: s.by_session.into_iter().map(Into::into).collect(),
        recent: rows
            .iter()
            .rev()
            .take(crate::models::REACH_RECENT)
            .map(Into::into)
            .collect(),
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new().routes(routes!(reach))
}
