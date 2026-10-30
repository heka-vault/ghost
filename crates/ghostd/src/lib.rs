//! Ghost's daemon, as a library the `ghostd` binary assembles (W8 of heka-vault/ghost#39):
//! its settings, what build it is, the phone's way in, the webhook listener, the routes
//! it runs itself, the vault's daemon side, the small pieces the wake loop reads, and the
//! command line's `check`, `push` and `latency`.

pub mod build_info;
pub mod check;
pub mod config;
pub mod controller;
pub mod daemon;
pub mod deciders;
pub mod dictation;
pub mod hooks_listener;
pub mod latency;
pub mod passwords;
pub mod phone;
pub mod phone_host;
/// Pausing the players for a session (MPRIS).
pub mod players;
pub mod push;
pub mod routes;
pub mod session;
pub mod wake;

pub use config::GhostdSettings;
