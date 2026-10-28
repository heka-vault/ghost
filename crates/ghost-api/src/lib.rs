//! The control API (ADR 0013): a small HTTP app on a user-owned Unix socket, mode 0600,
//! no TCP port. The desktop app and the `ghostd` CLI are its clients; the model has no
//! route to it, and nothing passive crosses it.
//!
//! `models` holds the types; `packages/api/openapi.json` stays the contract the TypeScript
//! client is generated from, and W3 of heka-vault/ghost#39 generates it from these types.

pub mod models;
pub mod server;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::RwLock;

use crate::models::{Phase, Status};

/// What the daemon knows about itself, read by every route that reports it.
#[derive(Debug)]
pub struct DaemonState {
    pub phase: Phase,
    pub session_id: Option<String>,
    pub models_ready: bool,
    pub version: String,
    pub wake_mode: String,
    pub apps: Vec<String>,
    pub conversation: Option<String>,
    pub waiting: u64,
    started: Instant,
}

impl DaemonState {
    pub fn new(version: impl Into<String>, wake_mode: impl Into<String>) -> Self {
        Self {
            phase: Phase::Starting,
            session_id: None,
            models_ready: false,
            version: version.into(),
            wake_mode: wake_mode.into(),
            apps: Vec::new(),
            conversation: None,
            waiting: 0,
            started: Instant::now(),
        }
    }

    pub fn uptime_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

/// The one place a `Status` is built, as in the Python daemon: the control API, its event
/// stream and the phone's `/hello` all read it.
pub fn status_snapshot(st: &DaemonState) -> Status {
    Status {
        phase: st.phase,
        session_id: st.session_id.clone(),
        models_ready: st.models_ready,
        uptime_secs: st.uptime_secs(),
        version: st.version.clone(),
        wake_mode: st.wake_mode.clone(),
        apps: st.apps.clone(),
        pending: None,
        conversation: st.conversation.clone(),
        session_origin: None,
        ending: false,
        waiting: st.waiting,
    }
}

pub type SharedState = Arc<RwLock<DaemonState>>;

async fn get_status(State(state): State<SharedState>) -> Json<Status> {
    Json(status_snapshot(&*state.read().await))
}

/// The routes. Only `/status` so far; the rest arrive with W3.
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/status", get(get_status))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_daemon_is_starting_with_nothing_loaded() {
        let st = DaemonState::new("2026.10.0", "transcript");
        let s = status_snapshot(&st);
        assert_eq!(s.phase, Phase::Starting);
        assert!(!s.models_ready);
        assert_eq!(s.session_id, None);
        assert_eq!(s.version, "2026.10.0");
        assert_eq!(s.waiting, 0);
    }

    #[test]
    fn status_serializes_the_way_the_typescript_client_reads_it() {
        let st = DaemonState::new("2026.10.0", "onnx");
        let v: serde_json::Value = serde_json::to_value(status_snapshot(&st)).unwrap();
        assert_eq!(v["phase"], "starting");
        assert_eq!(v["session_id"], serde_json::Value::Null);
        assert_eq!(v["models_ready"], false);
        assert_eq!(v["wake_mode"], "onnx");
        assert_eq!(v["apps"], serde_json::json!([]));
        assert_eq!(v["ending"], false);
        assert_eq!(v["waiting"], 0);
        assert!(v["uptime_secs"].is_number());
    }
}
