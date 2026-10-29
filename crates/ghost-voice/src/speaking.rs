//! Whether Ghost's voice is at the speaker right now, as the output stage knows it.
//!
//! `BotStarted` and `BotStopped` travel up the pipeline as frames, and a frame waits in
//! each stage's inbox behind whatever that stage is doing. The TTS stage renders a reply a
//! clause at a time inside one frame handler, so the `BotStarted` it forwards reaches the
//! user aggregator only once the whole reply is rendered: seconds, for a long answer. In
//! that window the barge-in rule took Ghost for silent, and the first half second of each
//! reply, which the echo canceller lets through while it converges, opened a turn and cut
//! the reply off: Ghost heard itself and took it for the operator. Pipecat carried these
//! frames out of band, as system frames.
//!
//! This is the fact itself, by handle: set by the output stage as it writes the first
//! sample of a reply, cleared when the last has played or the turn was cut. The barge-in
//! rule, the interim schedule, the echo filter, the confirmation processor and the
//! interceptors read it next to the frames; the frames still carry the event to the taps
//! and the observer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct Speaking(Arc<AtomicBool>);

impl Speaking {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, on: bool) {
        self.0.store(on, Ordering::SeqCst);
    }

    pub fn is_on(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_one_fact_shared_by_handle() {
        let a = Speaking::new();
        let b = a.clone();
        assert!(!b.is_on());
        a.set(true);
        assert!(b.is_on());
        b.set(false);
        assert!(!a.is_on());
    }
}
