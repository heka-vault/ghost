//! Removing Ghost from this computer (#147, ADR 0070): what it left, by kind, with sizes, for
//! Settings › General. The list only: the removal itself is `ghostd uninstall`, which the app
//! runs once it has quit (it cannot remove the storage of a window that is still open).
//!
//! The operator's only: the phone listener, the listener for outside tools and Ghost's own app
//! serve none of this, and no tool, model, app or run can list, start or choose it.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult};

/// One thing Ghost left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LeftoverItemView {
    pub key: String,
    /// What it is, in the person's words.
    pub what: String,
    /// The files and folders it is, home shown as `~`; only those there now.
    pub paths: Vec<String>,
    /// Bytes on disk.
    pub size: u64,
    /// Something the person does themselves, or a fact.
    #[schema(required)]
    pub note: Option<String>,
}

/// One kind the person chooses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LeftoverCategoryView {
    /// `running`, `other-tools`, `speech-models`, `app-storage`, `data` or `vault`.
    pub id: String,
    pub title: String,
    pub sentence: String,
    /// For what cannot be had back.
    #[schema(required)]
    pub warning: Option<String>,
    /// The person's own words and keys: not chosen unless they say so.
    pub persons: bool,
    pub size: u64,
    /// What is there now (empty: nothing of this kind is left).
    pub items: Vec<LeftoverItemView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UninstallView {
    pub categories: Vec<LeftoverCategoryView>,
    /// `deb`, `appimage` or `checkout`.
    pub install: String,
    /// The one line that finishes it: `sudo apt remove heka-ghost-desktop`, or the AppImage
    /// to delete.
    pub finish: String,
}

/// What the daemon lists (`ghostd::uninstall`).
pub trait UninstallPort: Send + Sync {
    fn view(&self) -> UninstallView;
}

/// What Ghost left on this computer, by kind.
#[utoipa::path(get, path = "/uninstall", operation_id = "uninstall_view_uninstall_get",
    tag = "uninstall", summary = "What Ghost left on this computer",
    responses((status = 200, description = "Successful Response", body = UninstallView)))]
async fn uninstall_view(State(deps): State<SharedDeps>) -> ApiResult<Json<UninstallView>> {
    let port = deps
        .uninstall
        .clone()
        .ok_or_else(|| ApiError::unavailable("this daemon cannot list what Ghost left"))?;
    // Sizes walk the folders: off the runtime's threads.
    tokio::task::spawn_blocking(move || port.view())
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new().routes(routes!(uninstall_view))
}
