//! The control API (ADR 0013): a small HTTP app on a user-owned Unix socket, mode 0600,
//! no TCP port. The desktop app and the `ghostd` CLI are its clients; the model has no
//! route to it, and nothing passive crosses it.
//!
//! `models` holds the types; `packages/api/openapi.json` stays the contract the TypeScript
//! client is generated from, and `openapi::document` generates it from these types.
//!
//! What never passes through here: passive audio or passive transcripts (the idle-audio
//! rule), a password's value (write-only, ADR 0036), or a way for the model to reach it. A
//! Confirm click in the operator's own desktop session is confirmation evidence, recorded
//! as `classifier: ui`.

pub mod app;
pub mod apps;
pub mod client;
pub mod controller;
pub mod conversations;
pub mod deps;
pub mod error;
pub mod model;
pub mod models;
pub mod openapi;
pub mod passwords;
pub mod server;
pub mod skill_catalog;
pub mod state;

pub use app::{
    audit_page, create_app, outcome_view, pending_view, permissions_view, status_snapshot,
    waiting_view,
};
pub use client::{ClientError, GhostClient, Reply};
pub use controller::{Controller, PhoneConnection, Probe, Refusal, Unavailable};
pub use conversations::{conversation_detail, conversation_view};
pub use deps::{AppsPort, Deps, DevicesPort, ModelNet, SharedDeps, SignInPort, UI_SESSION};
pub use error::{ApiError, ApiResult, HTTPValidationError, ValidationError};
pub use models::{
    AuditPage, Build, Phase, PhoneAddress, PhoneDevice, PhonePairing, PhoneView, Status,
    WaitingView,
};
pub use server::{bind, default_socket_path, serve};
pub use state::{DaemonState, EventBus};

/// The routes over `deps`, as `axum::serve` takes them.
pub fn router(deps: SharedDeps) -> axum::Router {
    create_app(deps).0
}
