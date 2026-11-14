//! Error reports (ADR 0067): the operator's switch, and the last reports this install sent,
//! exactly as they were sent but for the project's key.
//!
//! The operator's only: the phone listener, the listener for outside tools and Ghost's own
//! app serve none of this, and no tool, model, app or run can turn reports on or off or read
//! them (ADR 0013, ADR 0048).

use async_trait::async_trait;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};

/// One report this install sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct KeptReportView {
    /// RFC 3339.
    pub sent_at: String,
    /// `panic`, `page`, `failure` or `test`.
    pub kind: String,
    /// `daemon`, `shell`, `floating` or `page`.
    pub component: String,
    /// The exception's type and message, as sent.
    pub title: String,
    /// The request's body exactly as sent, the project's key shown as a placeholder.
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReportsView {
    /// This build can send error reports at all: a released build carries the project's
    /// key; a build from the checkout does not.
    pub available: bool,
    /// The operator's switch: on unless they turned it off.
    pub send: bool,
    /// This install's random id while reports are on.
    #[schema(required)]
    pub install_id: Option<String>,
    /// Reports waiting to be sent.
    pub pending: usize,
    /// The last reports sent, newest first (at most 20).
    pub kept: Vec<KeptReportView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportsChoice {
    /// Send error reports. Off sends nothing, keeps nothing waiting, and deletes the
    /// install's id.
    pub send: bool,
}

/// What the daemon does for error reports (`ghostd::reports`).
#[async_trait]
pub trait ReportsPort: Send + Sync {
    fn view(&self) -> ReportsView;
    fn set(&self, send: bool) -> Result<ReportsView, String>;
}

fn port(deps: &SharedDeps) -> ApiResult<&dyn ReportsPort> {
    deps.reports
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("error reports are not available in this daemon"))
}

/// The switch and the last reports sent.
#[utoipa::path(get, path = "/reports", operation_id = "reports_view_reports_get",
    tag = "reports", summary = "Error reports",
    responses((status = 200, description = "Successful Response", body = ReportsView)))]
async fn reports_view(State(deps): State<SharedDeps>) -> ApiResult<Json<ReportsView>> {
    Ok(Json(port(&deps)?.view()))
}

/// Send error reports, or not.
#[utoipa::path(put, path = "/reports/choice", operation_id = "reports_choice_reports_choice_put",
    tag = "reports", summary = "Send error reports", request_body = ReportsChoice,
    responses((status = 200, description = "Successful Response", body = ReportsView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn reports_choice(
    State(deps): State<SharedDeps>,
    Body(req): Body<ReportsChoice>,
) -> ApiResult<Json<ReportsView>> {
    port(&deps)?
        .set(req.send)
        .map(Json)
        .map_err(ApiError::internal)
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(reports_view))
        .routes(routes!(reports_choice))
}
