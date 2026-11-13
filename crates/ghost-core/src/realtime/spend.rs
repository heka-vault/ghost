//! What the realtime voice costs: each response's usage priced from the provider's list,
//! a session's total, and the day's in a small file beside Ghost's data (never the audio).
//!
//! The estimate is the provider's own token counts times the prices Ghost was built with
//! ([`super::settings::PRICES_AS_OF`]); the provider's bill is the truth. A day is the
//! machine's local date. The file holds numbers only:
//! `{"day": "2026-11-12", "usd": 0.42, "sessions": 3, "last_session_usd": 0.11}`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "realtime-spend.json";

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    pub text_in: f64,
    pub text_cached: f64,
    pub text_out: f64,
    pub audio_in: f64,
    pub audio_cached: f64,
    pub audio_out: f64,
}

/// One response's tokens, cached input apart from the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub text_in: u64,
    pub audio_in: u64,
    pub text_cached: u64,
    pub audio_cached: u64,
    pub text_out: u64,
    pub audio_out: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.text_in += other.text_in;
        self.audio_in += other.audio_in;
        self.text_cached += other.text_cached;
        self.audio_cached += other.audio_cached;
        self.text_out += other.text_out;
        self.audio_out += other.audio_out;
    }

    /// The estimate in US dollars.
    pub fn usd(&self, p: &Prices) -> f64 {
        let m = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
        m(self.text_in, p.text_in)
            + m(self.text_cached, p.text_cached)
            + m(self.text_out, p.text_out)
            + m(self.audio_in, p.audio_in)
            + m(self.audio_cached, p.audio_cached)
            + m(self.audio_out, p.audio_out)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    pub day: String,
    pub usd: f64,
    pub sessions: u64,
    pub last_session_usd: f64,
}

/// The day's spend, kept in one file and read on every change, so the control API and the
/// session see the same numbers.
pub struct Ledger {
    path: PathBuf,
    lock: Mutex<()>,
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl Ledger {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Today's spend; a file from another day reads as nothing yet today, keeping the
    /// last session's figure.
    pub fn read(&self, day: &str) -> Spend {
        let kept: Spend = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if kept.day == day {
            kept
        } else {
            Spend {
                day: day.to_string(),
                last_session_usd: kept.last_session_usd,
                ..Spend::default()
            }
        }
    }

    fn write(&self, s: &Spend) {
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string(s)
            && std::fs::write(&tmp, text).is_ok()
        {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// A realtime session opened.
    pub fn session_started(&self, day: &str) -> Spend {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.read(day);
        s.sessions += 1;
        s.last_session_usd = 0.0;
        self.write(&s);
        s
    }

    /// Add one response's estimate; the day's total after it.
    pub fn add(&self, day: &str, usd: f64) -> Spend {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.read(day);
        s.usd += usd.max(0.0);
        s.last_session_usd += usd.max(0.0);
        self.write(&s);
        s
    }

    /// Whether `cap` is reached today.
    pub fn over(&self, day: &str, cap: Option<f64>) -> bool {
        cap.is_some_and(|c| self.read(day).usd >= c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::settings::OPENAI;

    #[test]
    fn a_minute_each_way_on_the_mini_model_costs_about_three_cents() {
        // 600 audio tokens a minute heard, 1,200 a minute spoken (the provider's rates).
        let p = OPENAI.models[0].prices;
        let u = Usage {
            audio_in: 600,
            audio_out: 1200,
            ..Usage::default()
        };
        let usd = u.usd(&p);
        assert!((usd - 0.03).abs() < 0.001, "{usd}");
        // Cached input is priced as cached.
        let cached = Usage {
            audio_cached: 1_000_000,
            ..Usage::default()
        };
        assert!((cached.usd(&p) - 0.30).abs() < 1e-9);
    }

    #[test]
    fn the_day_adds_up_and_starts_again_tomorrow() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::new(dir.path());
        l.session_started("2026-11-12");
        l.add("2026-11-12", 0.25);
        let s = l.add("2026-11-12", 0.5);
        assert!((s.usd - 0.75).abs() < 1e-9);
        assert_eq!(s.sessions, 1);
        assert!(l.over("2026-11-12", Some(0.75)));
        assert!(!l.over("2026-11-12", Some(1.0)));
        assert!(!l.over("2026-11-12", None));
        let tomorrow = l.read("2026-11-13");
        assert_eq!(tomorrow.usd, 0.0);
        assert!((tomorrow.last_session_usd - 0.75).abs() < 1e-9);
        // The file holds numbers only.
        let text = std::fs::read_to_string(l.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 4);
    }
}
