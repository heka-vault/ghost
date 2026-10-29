//! Audio debug logging, off by default.
//!
//! With `GHOST_DEBUG_AUDIO=1` Ghost logs what it hears and decides: passive and session
//! transcripts, VAD starts and stops, wake matches and misses, barge-in decisions, and
//! confirmation verdicts. The log stays on the machine (journal or terminal). Leave it off
//! in normal use: with it off, passive transcripts are never written anywhere (CLAUDE.md,
//! ADR 0010). This is the one function that may put a transcript in a log line; the
//! message is built only when the door is open, so nothing is even formatted otherwise.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{info, warn};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if on {
        warn!("GHOST_DEBUG_AUDIO is on: transcripts and audio decisions are being logged");
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Log an audio-path event when debugging is on. `msg` runs only then.
pub fn audio(msg: impl FnOnce() -> String) {
    if enabled() {
        info!("[audio] {}", msg());
    }
}
