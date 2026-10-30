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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Audio resuming after this much silence at the speaker is an onset: the echo canceller
/// has drifted and leaks the next half second or so (measured 2026-10-29/30: the first
/// ~0.5 s of each reply and of each clause after a gap come back at full level).
const GAP: Duration = Duration::from_millis(300);

#[derive(Debug, Default)]
struct Inner {
    on: AtomicBool,
    last_audio: Mutex<Option<Instant>>,
    onset: Mutex<Option<Instant>>,
    /// A turn's close is waiting (the grace, or the longer silence): the VAD re-arms
    /// quicker for the operator going on (`stt`).
    closing: AtomicBool,
}

#[derive(Clone, Debug, Default)]
pub struct Speaking(Arc<Inner>);

impl Speaking {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, on: bool) {
        self.0.on.store(on, Ordering::SeqCst);
    }

    pub fn is_on(&self) -> bool {
        self.0.on.load(Ordering::SeqCst)
    }

    /// A chunk of Ghost's voice was written to the device. After a gap, that is an onset.
    pub fn audio(&self) {
        let now = Instant::now();
        let mut last = self.0.last_audio.lock().unwrap();
        if last.is_none_or(|t| now.duration_since(t) > GAP) {
            *self.0.onset.lock().unwrap() = Some(now);
        }
        *last = Some(now);
    }

    /// When Ghost's voice last (re)started after a gap, if ever.
    pub fn onset(&self) -> Option<Instant> {
        *self.0.onset.lock().unwrap()
    }

    pub fn set_closing(&self, on: bool) {
        self.0.closing.store(on, Ordering::SeqCst);
    }

    pub fn is_closing(&self) -> bool {
        self.0.closing.load(Ordering::SeqCst)
    }

    /// Tests: an onset at a chosen time.
    pub fn mark_onset_at(&self, at: Instant) {
        *self.0.onset.lock().unwrap() = Some(at);
        *self.0.last_audio.lock().unwrap() = Some(at);
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
