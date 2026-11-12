//! The limits an app is held to (ADR 0046), with the defaults the leading agents use, and
//! the settings that move them. A setting that is not a whole number above zero is
//! ignored and the default stands.
//!
//! | Setting | Default | What |
//! |---|---|---|
//! | `GHOST_APP_TIMEOUT` | 300 | seconds one call may take |
//! | `GHOST_APP_CONNECT_TIMEOUT` | 60 | seconds connecting may take |
//! | `GHOST_APP_RESULT_CHARS` | 24000 | characters of one result the model reads |
//! | `GHOST_APP_TOOLS_MAX` | 40 | apps' tools offered by name before the rest are found by search |
//! | `GHOST_APP_TOOLS_BYTES` | 30000 | the same, by the size of their schemas |
//! | `GHOST_APP_TOOLS_KEPT` | 12 | how many of the most used stay offered by name past that |
//!
//! An app's own `timeout`, `connect_timeout` and `result_chars` in `apps.json` win.

use std::time::Duration;

/// One call (both leaders' default; it was 20).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Connecting (both leaders' default; it was 15).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// One whole result, after redaction (it was 1,200 per string and 30 items per list).
pub const RESULT_CHARS: usize = 24_000;
/// A list in a result, within the budget.
pub const MAX_RESULT_ITEMS: usize = 200;
/// A tool's description as the model reads it (it was the first sentence, 240).
pub const DESCRIPTION_MAX: usize = 1_024;
/// What an app says about itself when it connects, in its paragraph of the prompt.
pub const INSTRUCTIONS_MAX: usize = 2_000;
/// How long the operator has to answer an app's question.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(120);
/// An app over HTTP is pinged this often, when it answers pings.
pub const KEEPALIVE: Duration = Duration::from_secs(180);
pub const TOOLS_MAX: usize = 40;
pub const TOOLS_BYTES: usize = 30_000;
pub const TOOLS_KEPT: usize = 12;

/// How a connection that died is brought back in the background: `tries` attempts, the
/// wait doubling from `base` up to `cap`, each off by up to `jitter` either way, then one
/// attempt every `park`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backoff {
    pub tries: u32,
    pub base: Duration,
    pub cap: Duration,
    pub park: Duration,
    pub jitter: f64,
    /// How often a live connection is looked at.
    pub watch: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            tries: 5,
            base: Duration::from_secs(1),
            cap: Duration::from_secs(60),
            park: Duration::from_secs(300),
            jitter: 0.2,
            watch: Duration::from_secs(5),
        }
    }
}

impl Backoff {
    /// The wait before attempt `n` (from 0), before the jitter.
    pub fn wait(&self, n: u32) -> Duration {
        if n >= self.tries {
            return self.park;
        }
        let doubled = self.base.saturating_mul(1u32 << n.min(16));
        doubled.min(self.cap)
    }

    /// The wait with its jitter: `roll` is in 0..1.
    pub fn jittered(&self, n: u32, roll: f64) -> Duration {
        let wait = self.wait(n).as_secs_f64();
        let off = (roll.clamp(0.0, 1.0) * 2.0 - 1.0) * self.jitter;
        Duration::from_secs_f64((wait * (1.0 + off)).max(0.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub call_timeout: Duration,
    pub connect_timeout: Duration,
    pub result_chars: usize,
    pub ask_timeout: Duration,
    pub keepalive: Duration,
    pub tools_max: usize,
    pub tools_bytes: usize,
    pub tools_kept: usize,
    pub backoff: Backoff,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            call_timeout: CALL_TIMEOUT,
            connect_timeout: CONNECT_TIMEOUT,
            result_chars: RESULT_CHARS,
            ask_timeout: ASK_TIMEOUT,
            keepalive: KEEPALIVE,
            tools_max: TOOLS_MAX,
            tools_bytes: TOOLS_BYTES,
            tools_kept: TOOLS_KEPT,
            backoff: Backoff::default(),
        }
    }
}

impl Limits {
    /// The defaults, moved by the settings `var` has.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let number = |name: &str| -> Option<u64> {
            var(name)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|n| *n > 0)
        };
        let mut l = Self::default();
        if let Some(n) = number("GHOST_APP_TIMEOUT") {
            l.call_timeout = Duration::from_secs(n);
        }
        if let Some(n) = number("GHOST_APP_CONNECT_TIMEOUT") {
            l.connect_timeout = Duration::from_secs(n);
        }
        if let Some(n) = number("GHOST_APP_RESULT_CHARS") {
            l.result_chars = n as usize;
        }
        if let Some(n) = number("GHOST_APP_TOOLS_MAX") {
            l.tools_max = n as usize;
        }
        if let Some(n) = number("GHOST_APP_TOOLS_BYTES") {
            l.tools_bytes = n as usize;
        }
        if let Some(n) = number("GHOST_APP_TOOLS_KEPT") {
            l.tools_kept = n as usize;
        }
        l
    }

    /// From Ghost's own environment (`settings.env`): none of these is a password.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn the_defaults_are_the_leaders_and_a_setting_moves_one() {
        let l = Limits::from_vars(|_| None);
        assert_eq!(l, Limits::default());
        assert_eq!(
            (l.call_timeout.as_secs(), l.connect_timeout.as_secs()),
            (300, 60)
        );
        assert_eq!(
            (l.result_chars, l.tools_max, l.tools_bytes),
            (24_000, 40, 30_000)
        );
        let l = Limits::from_vars(|name| match name {
            "GHOST_APP_TIMEOUT" => Some("45".into()),
            "GHOST_APP_RESULT_CHARS" => Some(" 8000 ".into()),
            "GHOST_APP_CONNECT_TIMEOUT" => Some("soon".into()), // not a number: the default
            "GHOST_APP_TOOLS_MAX" => Some("0".into()),          // not above zero: the default
            _ => None,
        });
        assert_eq!(l.call_timeout.as_secs(), 45);
        assert_eq!(l.result_chars, 8_000);
        assert_eq!(l.connect_timeout, CONNECT_TIMEOUT);
        assert_eq!(l.tools_max, TOOLS_MAX);
    }

    #[test]
    fn the_wait_doubles_to_the_cap_then_parks() {
        let b = Backoff::default();
        let secs: Vec<u64> = (0..7).map(|n| b.wait(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 300, 300]);
        let long = Backoff {
            tries: 9,
            ..Backoff::default()
        };
        assert_eq!(long.wait(8).as_secs(), 60);
        assert_eq!(b.jittered(3, 0.5).as_secs_f64(), 8.0);
        assert!((b.jittered(3, 0.0).as_secs_f64() - 6.4).abs() < 1e-9);
        assert!((b.jittered(3, 1.0).as_secs_f64() - 9.6).abs() < 1e-9);
    }
}
