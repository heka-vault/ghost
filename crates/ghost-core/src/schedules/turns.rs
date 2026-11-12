//! One piece of work at a time per agent, with nobody there: a scheduled run, a watch's
//! act, a follow-up to an answer, another agent's `ask_agent` or an outside tool's
//! `agent_ask`. Each writes the agent's transcript and may write its memory, so two of
//! them side by side would interleave both. A live voice session is not in this queue: a
//! run beside it shares nothing mutable with it (ADR 0047).
//!
//! Asks go first, since someone is waiting on the answer: while an ask waits for an agent,
//! the scheduler starts no run of it. An ask waits [`ASK_WAIT`] at the most and is then
//! refused with [`Busy`], so a run that is long, or two agents asking each other from
//! their own runs, cannot hold a caller for ever.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;

/// The longest an ask waits for its agent to be free.
pub const ASK_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Agent {
    busy: bool,
    /// Asks waiting for this agent: a run is not started while there are any.
    asks: usize,
}

/// The agents that are working, and the asks waiting for them.
#[derive(Default)]
pub struct AgentTurns {
    agents: Mutex<HashMap<String, Agent>>,
    /// Told whenever an agent is free again.
    freed: Notify,
    /// Also told then: the scheduler, so a run that waited starts.
    on_free: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// An ask waited [`ASK_WAIT`] and the agent was still working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub waited: Duration,
}

/// The agent's turn, until it is dropped.
pub struct Turn {
    turns: Arc<AgentTurns>,
    agent: String,
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.turns.release(&self.agent);
    }
}

impl AgentTurns {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Agent>> {
        self.agents.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What else to tell when an agent is free (the scheduler's wake).
    pub fn on_free(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_free.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Whether the agent is working now.
    pub fn busy(&self, agent: &str) -> bool {
        self.lock().get(agent).is_some_and(|a| a.busy)
    }

    /// A run's turn, if the agent is free and no ask is waiting for it. The caller hands
    /// it back with [`AgentTurns::release`].
    pub fn try_run(&self, agent: &str) -> bool {
        let mut agents = self.lock();
        let a = agents.entry(agent.to_string()).or_default();
        if a.busy || a.asks > 0 {
            return false;
        }
        a.busy = true;
        true
    }

    /// An ask's turn: at once if the agent is free, else once it is, ahead of any run,
    /// within `wait`.
    pub async fn ask(self: &Arc<Self>, agent: &str, wait: Duration) -> Result<Turn, Busy> {
        {
            let mut agents = self.lock();
            let a = agents.entry(agent.to_string()).or_default();
            if !a.busy {
                a.busy = true;
                return Ok(self.turn(agent));
            }
            a.asks += 1;
        }
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Registered before the look, so a release in between is not missed.
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut agents = self.lock();
                let a = agents.entry(agent.to_string()).or_default();
                if !a.busy {
                    a.busy = true;
                    a.asks -= 1;
                    return Ok(self.turn(agent));
                }
            }
            if tokio::time::timeout_at(deadline, freed).await.is_err() {
                let mut agents = self.lock();
                if let Some(a) = agents.get_mut(agent) {
                    a.asks -= 1;
                }
                drop(agents);
                // A run held back for this ask may start now.
                self.tell();
                return Err(Busy { waited: wait });
            }
        }
    }

    fn turn(self: &Arc<Self>, agent: &str) -> Turn {
        Turn {
            turns: self.clone(),
            agent: agent.to_string(),
        }
    }

    /// The agent is free again.
    pub fn release(&self, agent: &str) {
        {
            let mut agents = self.lock();
            if let Some(a) = agents.get_mut(agent) {
                a.busy = false;
                if a.asks == 0 {
                    agents.remove(agent);
                }
            }
        }
        self.tell();
    }

    fn tell(&self) {
        self.freed.notify_waiters();
        let f = self
            .on_free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(f) = f {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_run_waits_for_an_ask_and_an_ask_goes_before_a_run() {
        let t = AgentTurns::new();
        // A run has the agent: an ask waits for it.
        assert!(t.try_run("ledger"));
        let asking = tokio::spawn({
            let t = t.clone();
            async move { t.ask("ledger", Duration::from_secs(5)).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        // While the ask waits, no other run starts, even once the agent is free.
        t.release("ledger");
        let turn = asking.await.unwrap().expect("its turn");
        assert!(t.busy("ledger"));
        assert!(!t.try_run("ledger"));
        drop(turn);
        assert!(!t.busy("ledger"));
        assert!(t.try_run("ledger"));
        // Another agent is not held up by this one.
        assert!(t.try_run("ghost"));
    }

    #[tokio::test]
    async fn an_ask_held_too_long_is_told_the_agent_is_busy() {
        let t = AgentTurns::new();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        t.on_free({
            let woken = woken.clone();
            Arc::new(move || {
                woken.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        });
        assert!(t.try_run("ledger"));
        let e = t
            .ask("ledger", Duration::from_millis(50))
            .await
            .err()
            .expect("busy");
        assert_eq!(e.waited, Duration::from_millis(50));
        // The scheduler was told, and the ask no longer holds runs back.
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        t.release("ledger");
        assert!(t.try_run("ledger"));
    }

    #[tokio::test]
    async fn two_asks_of_one_agent_take_turns() {
        let t = AgentTurns::new();
        let first = t.ask("ledger", Duration::from_secs(5)).await.unwrap();
        let second = tokio::spawn({
            let t = t.clone();
            async move { t.ask("ledger", Duration::from_secs(5)).await.map(|_| ()) }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!second.is_finished());
        drop(first);
        second.await.unwrap().unwrap();
    }
}
