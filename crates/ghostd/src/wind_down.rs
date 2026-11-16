//! The daemon's runtime, and how long its end may take.
//!
//! Dropping a Tokio runtime waits for every blocking task still running, however long. A
//! capture whose close stalls (its thread is joined in a blocking task, ADR 0077; an open
//! in flight is one too) would then keep a daemon that was told to stop from ever exiting.
//! The daemon's own wind-down is awaited inside its future; what is left after it is only
//! work nobody waits for, and that gets [`LIMIT`], then the process goes.

use std::{future::Future, time::Duration};

/// What detached blocking work is given once the daemon's own wind-down has returned.
pub const LIMIT: Duration = Duration::from_secs(5);

/// Run `future` on a multi-threaded runtime, then give blocking tasks still running
/// `limit` to end before returning without them.
pub fn block_on<T>(
    future: impl Future<Output = anyhow::Result<T>>,
    limit: Duration,
) -> anyhow::Result<T> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let out = runtime.block_on(future);
    runtime.shutdown_timeout(limit);
    out
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    use super::block_on;

    /// A blocking task that never ends (a capture whose thread will not join) does not
    /// keep the daemon from returning. The gate stays closed until after the return.
    #[test]
    fn a_blocking_task_that_never_ends_does_not_hold_the_exit() {
        let (gate, wait) = mpsc::channel::<()>();
        let started = Instant::now();
        let out = block_on(
            async move {
                drop(tokio::task::spawn_blocking(move || {
                    let _ = wait.recv();
                }));
                tokio::task::yield_now().await;
                Ok(7)
            },
            Duration::from_millis(200),
        );
        assert_eq!(out.unwrap(), 7);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the exit waited for a task nobody waits on: {:?}",
            started.elapsed()
        );
        drop(gate); // only now may the stuck task end
    }

    #[test]
    fn the_daemons_error_is_returned() {
        let out: anyhow::Result<()> =
            block_on(async { anyhow::bail!("no") }, Duration::from_millis(50));
        assert!(out.is_err());
    }
}
