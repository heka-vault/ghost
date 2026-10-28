//! The control API's types. Field names and values are the contract in
//! `packages/api/openapi.json`; a change here is a change the TypeScript client sees.

use serde::{Deserialize, Serialize};

/// Where the daemon is. The tray tooltip and the phone's Home read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Models loading; the socket is up so the UI can show progress.
    Starting,
    /// Waiting for the wake phrase; idle audio stays on the machine.
    Passive,
    /// In a session, the operator's turn.
    Listening,
    /// Model or tool working.
    Thinking,
    /// Ghost talking.
    Speaking,
    /// In a session, waiting to be addressed by name again (ADR 0021).
    Hold,
    /// Ghost greeted the operator itself and waits for a reply (ADR 0032).
    Asking,
}

/// Where a live session's audio is: the desk's mic and speaker, or the paired phone over
/// WebRTC (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    Desk,
    Phone,
}

/// A change waiting for the operator's yes (ADR 0011). Filled in with the gate (W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingProposal {
    pub id: String,
    pub summary: String,
}

/// `GET /status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub phase: Phase,
    pub session_id: Option<String>,
    pub models_ready: bool,
    pub uptime_secs: f64,
    /// YYYY.M.N, the same in every copy (`scripts/version.py`).
    pub version: String,
    pub wake_mode: String,
    /// The apps connected now (ADR 0034).
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub pending: Option<PendingProposal>,
    /// Slug of the agent the session is with (ADR 0014).
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(default)]
    pub session_origin: Option<SessionOrigin>,
    /// Ghost has decided to hang up: its goodbye may still be playing.
    #[serde(default)]
    pub ending: bool,
    /// Items on the waiting list the operator has still to hear (ADR 0029).
    #[serde(default)]
    pub waiting: u64,
}
