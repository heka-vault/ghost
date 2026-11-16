//! Which desk captures are alive, counted where they live (ADR 0077).
//!
//! A [`Registry`] is made by the daemon and handed to every part that opens a capture on
//! the desk's microphone. Each takes a [`CaptureToken`] *before* it starts opening (so an
//! open still in flight counts) and keeps it with the capture until the capture is gone: a
//! [`Capture`](crate::Capture) holds it as its last field (`Capture::with_token`), so it is
//! released only after the capture's thread is joined; anything else holding a capture
//! keeps the token beside it and declared after it, for the same order.
//!
//! "Is any desk capture alive?" then has one answer whoever opened it, including a capture
//! whose owner gave up waiting for it (a pipeline stage left to finish on its own, a tape
//! task retired while it was still opening). Nothing here waits by itself: the counts are
//! read, and [`Registry::until`] is awaited only by whoever must know.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

/// Who opened a desk capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Holder {
    /// The passive listener's pipeline (the wake).
    Wake,
    /// The kept wake takes' raw array tape (ADR 0065).
    Tape,
    /// A desk session's microphone.
    Session,
    /// The Hearing test (ADR 0042).
    Hearing,
    /// Dictation (ADR 0077).
    Dictation,
}

const HOLDERS: usize = 5;

impl Holder {
    fn index(self) -> usize {
        match self {
            Holder::Wake => 0,
            Holder::Tape => 1,
            Holder::Session => 2,
            Holder::Hearing => 3,
            Holder::Dictation => 4,
        }
    }
}

/// The live desk captures, by holder.
#[derive(Debug)]
pub struct Registry {
    counts: watch::Sender<[usize; HOLDERS]>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            counts: watch::channel([0; HOLDERS]).0,
        }
    }
}

impl Registry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Count a capture about to be opened by `holder`; it stops counting when the token is
    /// dropped, which its holder does only after the capture itself is gone.
    pub fn take(self: &Arc<Self>, holder: Holder) -> CaptureToken {
        self.counts.send_modify(|c| c[holder.index()] += 1);
        CaptureToken {
            registry: self.clone(),
            holder,
        }
    }

    /// Captures alive now, whoever holds them.
    pub fn alive(&self) -> usize {
        self.counts.borrow().iter().sum()
    }

    /// Captures alive now held by `holder`.
    pub fn alive_for(&self, holder: Holder) -> usize {
        self.counts.borrow()[holder.index()]
    }

    /// Wait until `done` holds for the counts (by holder), or `wait` passes: true if it held.
    pub async fn until(&self, wait: Duration, done: impl Fn(&Counts) -> bool) -> bool {
        let mut rx = self.counts.subscribe();
        let check = |c: &[usize; HOLDERS]| done(&Counts(*c));
        tokio::time::timeout(wait, rx.wait_for(check)).await.is_ok()
    }

    /// The counts now.
    pub fn counts(&self) -> Counts {
        Counts(*self.counts.borrow())
    }
}

/// A snapshot of the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts([usize; HOLDERS]);

impl Counts {
    pub fn total(&self) -> usize {
        self.0.iter().sum()
    }
    pub fn of(&self, holder: Holder) -> usize {
        self.0[holder.index()]
    }
}

/// One desk capture alive, or opening. Dropped last.
#[derive(Debug)]
pub struct CaptureToken {
    registry: Arc<Registry>,
    holder: Holder,
}

impl CaptureToken {
    pub fn holder(&self) -> Holder {
        self.holder
    }
}

impl Drop for CaptureToken {
    fn drop(&mut self) {
        let i = self.holder.index();
        self.registry
            .counts
            .send_modify(|c| c[i] = c[i].saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_token_counts_until_dropped_and_the_wait_sees_it_go() {
        let r = Registry::new();
        let a = r.take(Holder::Tape);
        let b = r.take(Holder::Dictation);
        assert_eq!((r.alive(), r.alive_for(Holder::Tape)), (2, 1));
        assert!(!r.until(Duration::from_millis(20), |c| c.total() == 0).await);
        let waiting = {
            let r = r.clone();
            tokio::spawn(async move { r.until(Duration::from_secs(2), |c| c.total() == 0).await })
        };
        drop(a);
        assert_eq!(r.counts().of(Holder::Tape), 0);
        drop(b);
        assert!(waiting.await.unwrap());
        assert_eq!(r.alive(), 0);
    }
}
