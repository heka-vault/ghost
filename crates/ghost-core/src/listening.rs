//! Not listening (ADR 0055): the operator's switch that closes Ghost's microphone. While it
//! is off no capture stream is open, so nothing reaches the VAD, the wake detector or
//! Parakeet, and nothing is transcribed. It is one state with an optional end ("for an
//! hour"), kept in the store's settings like do-not-disturb so it holds over a restart.
//!
//! Only the operator turns it off and on: the desktop, the tray, the control API (the
//! phone, a keyboard shortcut) and, to turn it off only, their own words in a session.
//! No tool of any app or agent reaches it.

use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::{DateTime, Duration, Utc};
use tokio::sync::watch;

use crate::time::{iso, parse_iso};
use crate::waiting::Items;

/// The setting's key: empty when listening, `off` until turned back on, or the end's time.
pub const KEY: &str = "not_listening";
const OFF: &str = "off";
/// The longest "for …" the switch takes: a day. Longer is "until I turn it back on".
pub const MOST_MINUTES: u32 = 24 * 60;

/// Whether Ghost may open its microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Listening {
    #[default]
    On,
    /// Not listening; `until` when it ends by itself, `None` until the operator turns it on.
    Off { until: Option<DateTime<Utc>> },
}

impl Listening {
    /// What holds at `now`: an `Off` whose end has passed is `On`.
    pub fn at(self, now: DateTime<Utc>) -> Self {
        match self {
            Self::Off { until: Some(t) } if t <= now => Self::On,
            other => other,
        }
    }

    pub fn is_off(self, now: DateTime<Utc>) -> bool {
        matches!(self.at(now), Self::Off { .. })
    }

    fn encode(self) -> String {
        match self {
            Self::On => String::new(),
            Self::Off { until: None } => OFF.into(),
            Self::Off { until: Some(t) } => iso(&t),
        }
    }

    fn decode(v: Option<&str>) -> Self {
        match v.map(str::trim) {
            None | Some("") => Self::On,
            Some(OFF) => Self::Off { until: None },
            // a value this build does not read is the careful one: off
            Some(t) => Self::Off {
                until: parse_iso(t),
            },
        }
    }
}

/// The switch, shared by the daemon's passive loop, its sessions and the control API.
/// `changed` wakes whoever waits on it; the store, once attached, keeps it.
pub struct Mic {
    tx: watch::Sender<Listening>,
    items: OnceLock<Items>,
    gate: Mutex<()>,
}

impl Default for Mic {
    fn default() -> Self {
        Self::new()
    }
}

impl Mic {
    pub fn new() -> Self {
        Self {
            tx: watch::Sender::new(Listening::On),
            items: OnceLock::new(),
            gate: Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the switch back from the store and keeps it there from now on. A switch set
    /// before the store opened (nothing should, but a click may) wins over the stored one.
    pub fn attach(&self, items: Items) -> crate::waiting::Result<Listening> {
        let _g = self.lock();
        let stored = Listening::decode(items.setting(KEY)?.as_deref()).at(Utc::now());
        let now = *self.tx.borrow();
        let _ = self.items.set(items.clone());
        let keep = if now == Listening::On { stored } else { now };
        items.set_setting(KEY, &keep.encode())?;
        self.tx.send_replace(keep);
        Ok(keep)
    }

    /// The switch as set, an end that has passed included (see [`Mic::now`]).
    pub fn get(&self) -> Listening {
        *self.tx.borrow()
    }

    /// The switch as it holds now.
    pub fn now(&self) -> Listening {
        self.get().at(Utc::now())
    }

    pub fn is_off(&self) -> bool {
        self.now() != Listening::On
    }

    /// Turn listening off, for `minutes` (at most a day) or until turned back on.
    pub fn off(&self, minutes: Option<u32>) -> crate::waiting::Result<Listening> {
        let until = minutes
            .filter(|m| *m > 0)
            .map(|m| Utc::now() + Duration::minutes(i64::from(m.min(MOST_MINUTES))))
            // whole seconds, as the store keeps it
            .and_then(|t| DateTime::from_timestamp(t.timestamp(), 0));
        self.set(Listening::Off { until })
    }

    /// Turn listening off until `until`.
    pub fn off_until(&self, until: DateTime<Utc>) -> crate::waiting::Result<Listening> {
        self.set(Listening::Off { until: Some(until) })
    }

    pub fn on(&self) -> crate::waiting::Result<Listening> {
        self.set(Listening::On)
    }

    fn set(&self, to: Listening) -> crate::waiting::Result<Listening> {
        let _g = self.lock();
        if let Some(items) = self.items.get() {
            items.set_setting(KEY, &to.encode())?;
        }
        self.tx.send_replace(to);
        Ok(to)
    }

    /// A receiver that sees every change.
    pub fn watch(&self) -> watch::Receiver<Listening> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Arc;

    fn items() -> Items {
        Items::new(Arc::new(Store::in_memory().unwrap()))
    }

    #[test]
    fn off_holds_over_a_restart_and_on_clears_it() {
        let store = items();
        let mic = Mic::new();
        mic.attach(store.clone()).unwrap();
        assert!(!mic.is_off());
        mic.off(None).unwrap();
        // the daemon starts again over the same store
        let again = Mic::new();
        assert_eq!(
            again.attach(store.clone()).unwrap(),
            Listening::Off { until: None }
        );
        assert!(again.is_off());
        again.on().unwrap();
        let third = Mic::new();
        assert_eq!(third.attach(store).unwrap(), Listening::On);
    }

    #[test]
    fn a_timed_off_ends_by_itself_and_is_kept_with_its_end() {
        let store = items();
        let mic = Mic::new();
        mic.attach(store.clone()).unwrap();
        let set = mic.off(Some(30)).unwrap();
        let Listening::Off { until: Some(t) } = set else {
            panic!("{set:?}")
        };
        assert!(t > Utc::now() + Duration::minutes(29));
        assert!(set.is_off(Utc::now()));
        assert!(!set.is_off(t + Duration::seconds(1)));
        assert_eq!(Mic::new().attach(store).unwrap(), set);
    }

    #[test]
    fn an_end_that_passed_while_ghost_was_down_is_listening() {
        let store = items();
        store
            .set_setting(KEY, &iso(&(Utc::now() - Duration::minutes(1))))
            .unwrap();
        assert_eq!(Mic::new().attach(store).unwrap(), Listening::On);
    }

    #[test]
    fn more_than_a_day_is_a_day_and_an_unreadable_value_is_off() {
        let mic = Mic::new();
        let Listening::Off { until: Some(t) } = mic.off(Some(100_000)).unwrap() else {
            panic!()
        };
        assert!(t <= Utc::now() + Duration::minutes(i64::from(MOST_MINUTES)));
        assert_eq!(
            Listening::decode(Some("soon")),
            Listening::Off { until: None }
        );
    }

    #[test]
    fn a_click_before_the_store_opened_is_kept() {
        let mic = Mic::new();
        mic.off(None).unwrap();
        let store = items();
        assert_eq!(
            mic.attach(store.clone()).unwrap(),
            Listening::Off { until: None }
        );
        assert_eq!(store.setting(KEY).unwrap().as_deref(), Some(OFF));
    }

    #[tokio::test]
    async fn a_change_wakes_the_watcher() {
        let mic = Mic::new();
        let mut rx = mic.watch();
        mic.off(None).unwrap();
        rx.changed().await.unwrap();
        assert!(rx.borrow().is_off(Utc::now()));
    }
}
