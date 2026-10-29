//! The wakeup from PipeWire's thread to tokio: an `eventfd`. The process callback writes
//! one byte-count to it, a non-blocking syscall that takes no user-space lock, so the
//! realtime thread can never wait on the daemon's thread (a `tokio::sync::Notify` locks a
//! mutex when a waiter is registered, and under build load that showed as PipeWire xruns on
//! 2026-10-28). The tokio side reads it through `AsyncFd`, registered lazily on the first
//! wait so the stream can be opened outside a runtime.

use std::io;
use std::os::fd::OwnedFd;

use rustix::event::{EventfdFlags, eventfd};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// The realtime side: `signal` never blocks and never allocates.
#[derive(Debug)]
pub struct Wake {
    fd: OwnedFd,
}

impl Wake {
    /// An eventfd and the waiter on its other end.
    pub fn pair() -> io::Result<(Wake, Waiter)> {
        let fd = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let other = fd.try_clone()?;
        Ok((
            Wake { fd },
            Waiter {
                fd: Some(other),
                async_fd: None,
            },
        ))
    }

    /// Wake the waiter. Safe from a realtime callback.
    #[inline]
    pub fn signal(&self) {
        let _ = rustix::io::write(&self.fd, &1u64.to_ne_bytes());
    }
}

/// The tokio side.
#[derive(Debug)]
pub struct Waiter {
    fd: Option<OwnedFd>,
    async_fd: Option<AsyncFd<OwnedFd>>,
}

impl Waiter {
    /// Resolves after the next `signal`, or at once when one happened since the last
    /// wait. An error means the fd is gone; treat it as the stream having stopped.
    pub async fn wait(&mut self) -> io::Result<()> {
        if self.async_fd.is_none() {
            let fd = self
                .fd
                .take()
                .ok_or_else(|| io::Error::other("the waiter's fd was taken"))?;
            // SAFETY: the `OwnedFd` moves into the `AsyncFd`, which owns it until it is dropped
            // and deregistered; nothing else can close it in between.
            let afd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) }?;
            self.async_fd = Some(afd);
        }
        let afd = self.async_fd.as_ref().expect("just set");
        loop {
            let mut guard = afd.readable().await?;
            let mut count = [0u8; 8];
            match guard.try_io(|inner| {
                rustix::io::read(inner.get_ref(), &mut count).map_err(io::Error::from)
            }) {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_signal_before_the_wait_is_not_lost() {
        let (wake, mut waiter) = Wake::pair().unwrap();
        wake.signal();
        wake.signal();
        tokio::time::timeout(Duration::from_millis(100), waiter.wait())
            .await
            .expect("woken at once")
            .unwrap();
        // Both signals were folded into one wake; nothing is pending now.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), waiter.wait())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_signal_from_another_thread_wakes() {
        let (wake, mut waiter) = Wake::pair().unwrap();
        let wake = std::sync::Arc::new(wake);
        let t = std::thread::spawn({
            let wake = wake.clone();
            move || {
                std::thread::sleep(Duration::from_millis(30));
                wake.signal();
            }
        });
        tokio::time::timeout(Duration::from_millis(500), waiter.wait())
            .await
            .expect("woken")
            .unwrap();
        t.join().unwrap();
    }
}
