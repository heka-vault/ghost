//! The dictation key, hold or tap (ADR 0077 as amended 2026-11-25): one pure state machine
//! from the key's presses and releases to "start" and "stop", with no clock of its own, so
//! every case is a test.
//!
//! - **Hold:** recording starts at the press; a release after [`HOLD`] or more stops it.
//! - **Tap:** a release sooner than [`HOLD`] locks recording on; the next press stops it, and
//!   its release is ignored.
//! - The portal repeats the press every 30 ms after 500 ms while the key is held and sends one
//!   release (measured on GNOME 50.1): a press while the key is down is ignored.
//! - A release is decided only after [`GRACE`], so a release followed at once by a press (X11
//!   auto-repeat sends those pairs) is not taken for a tap or a stop.
//! - A press within [`DEBOUNCE`] of the last stop is ignored, so a bounce does not start
//!   another dictation.
//! - When the dictation ends on its own (the limit, a cancel, a refusal), [`KeyMachine::ended`]
//!   resets the machine; a key still down is then waited out, not taken as a new press.

use std::time::Duration;

/// A press held this long or longer is push-to-talk; a shorter one is a tap that locks.
pub const HOLD: Duration = Duration::from_millis(300);
/// How long a release waits for a press that would undo it.
pub const GRACE: Duration = Duration::from_millis(50);
/// Presses this soon after a stop are ignored.
pub const DEBOUNCE: Duration = Duration::from_millis(30);

/// What the key did, with the time it was seen (any fixed origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down(Duration),
    Up(Duration),
    /// No key event, only time passing: decides a release whose grace is over.
    Tick(Duration),
}

/// What the dictation should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle {
        /// When the last dictation was stopped by the key, for the debounce.
        stopped: Option<Duration>,
    },
    /// The key is down and recording; `since` is the press.
    Held { since: Duration },
    /// The key came up at `at`, `since` after its press; decided once the grace is over.
    Releasing { since: Duration, at: Duration },
    /// A tap locked recording on; the key is up.
    Locked,
    /// The key is down but means nothing until it comes up (the press that stopped a locked
    /// dictation, or a key still held when the dictation ended by itself).
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMachine {
    state: State,
}

impl Default for KeyMachine {
    fn default() -> Self {
        Self {
            state: State::Idle { stopped: None },
        }
    }
}

impl KeyMachine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the machine thinks a dictation is going on (held, releasing or locked).
    pub fn active(&self) -> bool {
        matches!(
            self.state,
            State::Held { .. } | State::Releasing { .. } | State::Locked
        )
    }

    /// When a [`KeyEvent::Tick`] should come, if one is needed: the end of a release's grace.
    pub fn deadline(&self) -> Option<Duration> {
        match self.state {
            State::Releasing { at, .. } => Some(at + GRACE),
            _ => None,
        }
    }

    pub fn on(&mut self, ev: KeyEvent) -> Option<Action> {
        // A release whose grace is over is decided first, whatever comes next.
        if let State::Releasing { since, at } = self.state {
            let now = match ev {
                KeyEvent::Down(t) | KeyEvent::Up(t) | KeyEvent::Tick(t) => t,
            };
            match ev {
                // A press inside the grace: the release was auto-repeat's, the hold goes on.
                KeyEvent::Down(t) if t < at + GRACE => {
                    self.state = State::Held { since };
                    return None;
                }
                _ if now >= at + GRACE => {
                    let decided = self.decide_release(since, at);
                    // The event that made the decision still counts on its own.
                    return match (decided, ev) {
                        (Some(a), _) => Some(a),
                        (None, KeyEvent::Tick(_)) => None,
                        (None, ev) => self.on(ev),
                    };
                }
                _ => return None,
            }
        }
        match (self.state, ev) {
            (_, KeyEvent::Tick(_)) => None,
            (State::Idle { stopped }, KeyEvent::Down(t)) => {
                if stopped.is_some_and(|s| t < s + DEBOUNCE) {
                    return None;
                }
                self.state = State::Held { since: t };
                Some(Action::Start)
            }
            (State::Idle { .. }, KeyEvent::Up(_)) => None,
            // The portal's repeats while the key is held.
            (State::Held { .. }, KeyEvent::Down(_)) => None,
            (State::Held { since }, KeyEvent::Up(t)) => {
                self.state = State::Releasing { since, at: t };
                None
            }
            (State::Locked, KeyEvent::Down(t)) => {
                self.state = State::Waiting;
                let _ = t;
                Some(Action::Stop)
            }
            (State::Locked, KeyEvent::Up(_)) => None,
            (State::Waiting, KeyEvent::Down(_)) => None,
            (State::Waiting, KeyEvent::Up(t)) => {
                self.state = State::Idle { stopped: Some(t) };
                None
            }
            (State::Releasing { .. }, _) => None, // handled above
        }
    }

    fn decide_release(&mut self, since: Duration, at: Duration) -> Option<Action> {
        if at.saturating_sub(since) >= HOLD {
            // The stop happens when the grace ends; the debounce counts from there.
            self.state = State::Idle {
                stopped: Some(at + GRACE),
            };
            Some(Action::Stop)
        } else {
            self.state = State::Locked;
            None
        }
    }

    /// The dictation ended without the key (the limit, a cancel, a refusal, a failure): back
    /// to idle, or wait out a key that is still down. `now` starts the debounce.
    pub fn ended(&mut self, now: Duration) {
        self.state = match self.state {
            State::Held { .. } | State::Waiting => State::Waiting,
            _ => State::Idle { stopped: Some(now) },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Feed events; collect the actions, ticking at each deadline that passes before the next
    /// event, as the driver does.
    fn run(m: &mut KeyMachine, events: &[KeyEvent]) -> Vec<(u64, Action)> {
        let mut out = Vec::new();
        for &ev in events {
            let t = match ev {
                KeyEvent::Down(t) | KeyEvent::Up(t) | KeyEvent::Tick(t) => t,
            };
            if let Some(d) = m.deadline()
                && d <= t
                && let Some(a) = m.on(KeyEvent::Tick(d))
            {
                out.push((d.as_millis() as u64, a));
            }
            if let Some(a) = m.on(ev) {
                out.push((t.as_millis() as u64, a));
            }
        }
        out
    }

    use KeyEvent::{Down, Tick, Up};

    #[test]
    fn a_hold_starts_at_the_press_and_stops_after_the_grace() {
        let mut m = KeyMachine::new();
        let got = run(&mut m, &[Down(ms(0)), Up(ms(1200)), Tick(ms(1300))]);
        assert_eq!(got, [(0, Action::Start), (1250, Action::Stop)]);
        assert!(!m.active());
    }

    #[test]
    fn the_portals_repeats_while_held_are_ignored() {
        // GNOME 50.1: a press, a repeat after 500 ms, then one every 30 ms, one release.
        let mut events = vec![Down(ms(0))];
        let mut t = 500;
        while t < 6200 {
            events.push(Down(ms(t)));
            t += 30;
        }
        events.push(Up(ms(6203)));
        events.push(Tick(ms(6300)));
        let mut m = KeyMachine::new();
        assert_eq!(
            run(&mut m, &events),
            [(0, Action::Start), (6253, Action::Stop)]
        );
    }

    #[test]
    fn a_tap_locks_and_the_next_press_stops() {
        let mut m = KeyMachine::new();
        // Taps of 65 to 110 ms, as measured.
        let got = run(
            &mut m,
            &[
                Down(ms(0)),
                Up(ms(77)),
                Tick(ms(200)),
                Down(ms(4000)),
                Up(ms(4090)),
                Tick(ms(4200)),
            ],
        );
        assert_eq!(got, [(0, Action::Start), (4000, Action::Stop)]);
        assert!(!m.active());
    }

    #[test]
    fn the_press_that_stops_a_locked_dictation_does_not_start_another_when_held() {
        let mut m = KeyMachine::new();
        let got = run(
            &mut m,
            &[
                Down(ms(0)),
                Up(ms(80)),
                Tick(ms(200)),
                Down(ms(3000)),
                Down(ms(3500)), // held, repeating
                Down(ms(3530)),
                Up(ms(5000)),
                Tick(ms(5100)),
            ],
        );
        assert_eq!(got, [(0, Action::Start), (3000, Action::Stop)]);
    }

    #[test]
    fn a_release_then_press_inside_the_grace_is_one_hold() {
        // X11 auto-repeat: release and press pairs while the key is physically held.
        let mut m = KeyMachine::new();
        let got = run(
            &mut m,
            &[
                Down(ms(0)),
                Up(ms(600)),
                Down(ms(610)),
                Up(ms(640)),
                Down(ms(645)),
                Up(ms(1500)),
                Tick(ms(1600)),
            ],
        );
        assert_eq!(got, [(0, Action::Start), (1550, Action::Stop)]);
    }

    #[test]
    fn the_hold_threshold_is_measured_from_press_to_release_not_to_the_decision() {
        let mut m = KeyMachine::new();
        // Released at 290 ms: a tap, even though the decision comes at 340 ms.
        let got = run(&mut m, &[Down(ms(0)), Up(ms(290)), Tick(ms(400))]);
        assert_eq!(got, [(0, Action::Start)]);
        assert!(m.active(), "locked");
        let mut m = KeyMachine::new();
        let got = run(&mut m, &[Down(ms(0)), Up(ms(300)), Tick(ms(400))]);
        assert_eq!(got, [(0, Action::Start), (350, Action::Stop)]);
    }

    #[test]
    fn a_press_right_after_a_stop_is_a_bounce() {
        let mut m = KeyMachine::new();
        let got = run(
            &mut m,
            &[
                Down(ms(0)),
                Up(ms(1000)),
                Tick(ms(1050)),
                Down(ms(1060)), // 10 ms after the stop: ignored
                Up(ms(1070)),
                Down(ms(2000)),
            ],
        );
        assert_eq!(
            got,
            [
                (0, Action::Start),
                (1050, Action::Stop),
                (2000, Action::Start)
            ]
        );
    }

    #[test]
    fn a_dictation_that_ends_while_the_key_is_held_waits_for_the_release() {
        // The limit, or a refusal at the start: the held key is not a new press.
        let mut m = KeyMachine::new();
        assert_eq!(m.on(Down(ms(0))), Some(Action::Start));
        m.ended(ms(100));
        assert_eq!(m.on(Down(ms(600))), None, "a repeat");
        assert_eq!(m.on(Up(ms(900))), None);
        assert_eq!(m.on(Down(ms(2000))), Some(Action::Start));
    }

    #[test]
    fn a_locked_dictation_that_ends_by_itself_leaves_the_key_free() {
        let mut m = KeyMachine::new();
        run(&mut m, &[Down(ms(0)), Up(ms(80)), Tick(ms(200))]);
        assert!(m.active());
        m.ended(ms(120_000)); // the two-minute limit
        assert!(!m.active());
        assert_eq!(m.on(Down(ms(121_000))), Some(Action::Start));
    }

    #[test]
    fn a_release_whose_grace_passed_unnoticed_is_decided_by_the_next_event() {
        // No tick came (a busy runtime): the next press decides the release first.
        let mut m = KeyMachine::new();
        assert_eq!(m.on(Down(ms(0))), Some(Action::Start));
        assert_eq!(m.on(Up(ms(1000))), None);
        assert_eq!(m.on(Down(ms(5000))), Some(Action::Stop));
        // That press was consumed by the decision only; it is not a new start.
        assert!(!m.active());
        let mut m = KeyMachine::new();
        m.on(Down(ms(0)));
        m.on(Up(ms(50))); // a tap, undecided
        // The next press, long after: the tap is decided (locked), then the press stops it.
        assert_eq!(m.on(Down(ms(3000))), Some(Action::Stop));
    }

    #[test]
    fn a_stray_release_or_tick_when_idle_does_nothing() {
        let mut m = KeyMachine::new();
        assert_eq!(m.on(Up(ms(10))), None);
        assert_eq!(m.on(Tick(ms(20))), None);
        assert!(!m.active());
    }
}
