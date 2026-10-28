//! The daemon exits when the shell that started it is gone (ADR 0038), so a crashed shell
//! leaves no orphan on the microphone. The shell says who it is in `GHOST_PARENT_PID`;
//! without it (the user unit, `make dev`, a terminal) nothing here is armed, and `run` has
//! no parent to depend on.
//!
//! Two mechanisms, since each has a gap: `prctl(PR_SET_PDEATHSIG, SIGTERM)` is immediate
//! but misses a parent that died before it was set and is tied to the parent's thread;
//! a poll of the parent pid every few seconds catches the rest.

use std::time::Duration;

use rustix::process::{Pid, Signal, getppid, set_parent_process_death_signal};
use tokio::sync::Notify;
use tracing::{info, warn};

pub const POLL: Duration = Duration::from_secs(3);

/// `GHOST_PARENT_PID`, when the launcher set one.
pub fn expected_parent() -> Option<u32> {
    std::env::var("GHOST_PARENT_PID").ok()?.trim().parse().ok()
}

/// The parent is not `expected`: it died and the daemon was reparented.
pub fn parent_gone(expected: u32) -> bool {
    getppid().map(Pid::as_raw_nonzero).map(|p| p.get() as u32) != Some(expected)
}

/// Arm both mechanisms when a parent is expected. `gone` is notified when it is.
pub fn watch(gone: &'static Notify) {
    let Some(expected) = expected_parent() else {
        return;
    };
    match set_parent_process_death_signal(Some(Signal::TERM)) {
        Ok(()) => info!("parent {expected}: stopping when it is gone"),
        Err(e) => warn!("parent death signal: {e}"),
    }
    if parent_gone(expected) {
        warn!("parent {expected} is already gone; stopping");
        gone.notify_one();
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL).await;
            if parent_gone(expected) {
                warn!("parent {expected} is gone; stopping");
                gone.notify_one();
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_is_gone_when_it_is_not_the_expected_one() {
        let me = getppid().unwrap().as_raw_nonzero().get() as u32;
        assert!(!parent_gone(me));
        assert!(parent_gone(me + 100_000));
    }
}
