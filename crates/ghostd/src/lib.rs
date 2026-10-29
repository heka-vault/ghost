//! Ghost's daemon, as a library the `ghostd` binary assembles (W8 of heka-vault/ghost#39):
//! its settings, what build it is, the phone's way in, the vault's daemon side, and the
//! small pieces the wake loop reads.

pub mod build_info;
pub mod config;
pub mod deciders;
pub mod dictation;
pub mod passwords;
pub mod phone;

pub use config::GhostdSettings;
