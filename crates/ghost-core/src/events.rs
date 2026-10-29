//! Fire-and-forget lifecycle events for a source's listeners.

use std::time::Duration;

use serde_json::{Map, Value};
use tracing::warn;

use crate::source::{Event, EventKind, EventListener};

pub struct Events {
    listeners: Vec<EventListener>,
    timeout: Duration,
}

impl Default for Events {
    fn default() -> Self {
        Self::new(Vec::new(), Duration::from_secs(2))
    }
}

impl Events {
    pub fn new(listeners: Vec<EventListener>, timeout: Duration) -> Self {
        Self { listeners, timeout }
    }

    pub fn add(&mut self, listener: EventListener) {
        self.listeners.push(listener);
    }

    /// Call every listener with a deadline so none of them can stall speech.
    pub async fn emit(&self, kind: EventKind, data: Map<String, Value>) {
        if self.listeners.is_empty() {
            return;
        }
        let event = Event { kind, data };
        let calls = self.listeners.iter().enumerate().map(|(i, listener)| {
            let event = event.clone();
            async move {
                if tokio::time::timeout(self.timeout, listener(event))
                    .await
                    .is_err()
                {
                    warn!("listener {i} for {kind:?} timed out");
                }
            }
        });
        futures::future::join_all(calls).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn every_listener_is_called_and_a_slow_one_is_cut_off() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut events = Events::new(Vec::new(), Duration::from_millis(20));
        let c = count.clone();
        events.add(Arc::new(move |_| {
            let c = c.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            })
        }));
        let c = count.clone();
        events.add(Arc::new(move |_| {
            let c = c.clone();
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                c.fetch_add(10, Ordering::SeqCst);
            })
        }));
        let started = std::time::Instant::now();
        events.emit(EventKind::Wake, Map::new()).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
