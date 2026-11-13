//! The realtime voice's settings and what it costs (ADR 0063): plain data the daemon, the
//! control API and the voice pipeline all read. The voice itself is `ghost_voice::realtime`.

pub mod settings;
pub mod spend;

pub use settings::{RealtimeSettings, provider_spec};
pub use spend::{Ledger, Prices, Spend, Usage};
