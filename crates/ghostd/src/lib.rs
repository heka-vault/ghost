//! Ghost's daemon, as a library the `ghostd` binary assembles (W8 of heka-vault/ghost#39):
//! its settings, what build it is, the phone's way in, the webhook listener, the listener
//! for outside tools, the routes
//! it runs itself, the vault's daemon side, the small pieces the wake loop reads, and the
//! command line's `check`, `push` and `latency`.

pub mod build_info;
pub mod check;
pub mod config;
pub mod controller;
pub mod daemon;
pub mod deciders;
pub mod dictation;
pub mod ghost_app;
pub mod hear;
pub mod hooks_listener;
pub mod latency;
pub mod listening;
pub mod llm_cli;
pub mod mcp;
pub mod microphone;
pub mod model_check;
pub mod outside;
pub mod passwords;
pub mod phone;
pub mod phone_host;
/// Pausing the players for a session (MPRIS).
pub mod players;
pub mod push;
pub mod reply_duck;
pub mod reports;
pub mod routes;
pub mod schedules;
pub mod session;
pub mod setup_cli;
pub mod speech_models;
pub mod wake;
pub mod wake_detectors;
pub mod wake_takes;

pub use config::GhostdSettings;
