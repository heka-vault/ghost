//! Developer's live microphone meter (ADR 0073, amended 2026-11-14): the lease the meter view
//! holds, and the task that reads `ghost_voice::meter` about fifteen times a second while it
//! is held and publishes each reading as an `Event::Meter`.
//!
//! The view asks for [`METER_LEASE_SECS`] at a time and asks again while it is open; when
//! it stops asking (closed, hidden, the window gone), the meter closes by itself, the
//! stages stop measuring, and one last reading says so. It opens no microphone: it reads
//! what the passive listener or a desk session already measures, and with listening off
//! (ADR 0057) there is nothing, which it says. Nothing is kept: each reading is what came
//! since the one before. The operator's control API only (`PUT /developer/meter`); no tool,
//! run, outside client or phone route reaches it, and the phone's stream and the outside
//! listener forward no `meter` event.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The lease's clock: tokio's, so a test can run it in paused time.
type LeaseInstant = tokio::time::Instant;

use ghost_api::live::{
    METER_LEASE_SECS, METER_PER_SECOND, MeterDecisionKind, MeterDecisionView, MeterLeaseView,
    MeterPort, MeterState, MeterView, RoomGateView, RoomGateWhy,
};
use ghost_voice::meter::{self, Decision, Reading, Source};

use crate::daemon::Daemon;

/// A frame older than this means nothing is listening now.
const STALE_SECS: f64 = 0.5;
/// How often the room gate's plan (the microphone's case) is read again.
const PLAN_EVERY: Duration = Duration::from_secs(2);

/// What the publisher needs to know besides the reading: listening off, and whether a desk
/// session here would run the room gate and why.
pub type Facts = Arc<dyn Fn() -> (bool, (bool, RoomGateWhy)) + Send + Sync>;
pub type Publish = Arc<dyn Fn(MeterView) + Send + Sync>;
/// Whether a desk session here runs the room gate, and why.
pub type Planned = (bool, RoomGateWhy);

/// Whether a desk session on this microphone would run the room gate, and why.
pub fn planned_gate(setting: Option<bool>, on_this_microphone: bool) -> (bool, RoomGateWhy) {
    match setting {
        Some(on) => (on, RoomGateWhy::Setting),
        None if on_this_microphone => (true, RoomGateWhy::ThisMicrophone),
        None => (false, RoomGateWhy::OnChipArray),
    }
}

fn f32_of(x: Option<f64>) -> Option<f32> {
    x.map(|v| ((v * 10.0).round() / 10.0) as f32)
}

fn decision_view(ago: f64, d: Decision) -> MeterDecisionView {
    let mut v = MeterDecisionView {
        kind: MeterDecisionKind::Taken,
        ago_ms: (ago * 1000.0).round().clamp(0.0, f64::from(u32::MAX)) as u32,
        words: None,
        dbfs: None,
        person: None,
        secs: None,
        score: None,
        woke: None,
    };
    match d {
        Decision::Taken { words, dbfs } => {
            v.words = Some(u32::try_from(words).unwrap_or(u32::MAX));
            v.dbfs = f32_of(Some(dbfs));
        }
        Decision::NoWords { secs, dbfs } => {
            v.kind = MeterDecisionKind::NoWords;
            v.secs = f32_of(Some(secs));
            v.dbfs = f32_of(Some(dbfs));
        }
        Decision::HeldRoom { dbfs, person } => {
            v.kind = MeterDecisionKind::HeldRoom;
            v.dbfs = f32_of(Some(dbfs));
            v.person = f32_of(person);
        }
        Decision::LetGo { secs, dbfs } => {
            v.kind = MeterDecisionKind::LetGo;
            v.secs = f32_of(Some(secs));
            v.dbfs = f32_of(Some(dbfs));
        }
        Decision::TakenByName => v.kind = MeterDecisionKind::TakenByName,
        Decision::Echo => v.kind = MeterDecisionKind::Echo,
        Decision::TooShort => v.kind = MeterDecisionKind::TooShort,
        Decision::BargeIn => v.kind = MeterDecisionKind::BargeIn,
        Decision::Wake { score, woke } => {
            v.kind = MeterDecisionKind::Wake;
            v.score = Some((score * 100.0).round() / 100.0);
            v.woke = woke;
        }
        Decision::NearMiss { score } => {
            v.kind = MeterDecisionKind::NearMiss;
            v.score = Some((score * 100.0).round() / 100.0);
        }
    }
    v
}

/// One reading as the event carries it. `planned` is what a desk session here would do.
pub fn view_of(r: Reading, listening_off: bool, planned: (bool, RoomGateWhy)) -> MeterView {
    let fresh = r.since_frame.is_some_and(|s| s <= STALE_SECS);
    let state = match (listening_off, fresh, r.source) {
        (true, _, _) => MeterState::ListeningOff,
        (false, true, Some(Source::Passive)) => MeterState::Passive,
        (false, true, Some(Source::Session)) => MeterState::Session,
        _ => MeterState::Quiet,
    };
    let live = matches!(state, MeterState::Passive | MeterState::Session);
    let room = r.room.unwrap_or_default();
    let room_gate = match state {
        MeterState::Session => RoomGateView {
            on: r.room_gate,
            why: if r.room_gate == planned.0 {
                planned.1
            } else if r.room_gate {
                RoomGateWhy::ThisMicrophone
            } else {
                RoomGateWhy::OnChipArray
            },
            room: f32_of(room.room),
            person: f32_of(room.person),
            line: f32_of(room.line),
        },
        _ => RoomGateView {
            on: planned.0,
            why: RoomGateWhy::NoSession,
            room: None,
            person: None,
            line: None,
        },
    };
    MeterView {
        open: true,
        state,
        level: if live { f32_of(r.level) } else { None },
        peak: if live { f32_of(r.peak) } else { None },
        speaking: live && r.speaking,
        room_gate,
        decisions: if listening_off {
            Vec::new()
        } else {
            r.decisions
                .into_iter()
                .map(|(ago, d)| decision_view(ago, d))
                .collect()
        },
    }
}

/// The closing reading: nothing measured.
fn closed() -> MeterView {
    MeterView {
        open: false,
        state: MeterState::Quiet,
        level: None,
        peak: None,
        speaking: false,
        room_gate: RoomGateView {
            on: false,
            why: RoomGateWhy::NoSession,
            room: None,
            person: None,
            line: None,
        },
        decisions: Vec::new(),
    }
}

struct Lease {
    until: Option<LeaseInstant>,
    running: bool,
}

/// The control API's side, and the publishing task.
pub struct MeterDesk {
    /// The daemon it serves; the task ends with it. None for a test's desk.
    daemon: Option<Weak<Daemon>>,
    lease: Arc<Mutex<Lease>>,
    publish: Publish,
    facts: Facts,
}

impl MeterDesk {
    pub fn new(d: &Arc<Daemon>) -> Self {
        let state = d.state.clone();
        let weak = Arc::downgrade(d);
        let plan: Arc<Mutex<Option<(Instant, Planned)>>> = Arc::default();
        let facts: Facts = Arc::new(move || {
            let Some(d) = weak.upgrade() else {
                return (true, (false, RoomGateWhy::NoSession));
            };
            let off = d.state.not_listening().is_some();
            let mut p = plan.lock().unwrap_or_else(|e| e.into_inner());
            let planned = match *p {
                Some((at, v)) if at.elapsed() < PLAN_EVERY => v,
                _ => {
                    let v = planned_gate(d.voice().room_gate, d.mic_plan().room_gate());
                    *p = Some((Instant::now(), v));
                    v
                }
            };
            (off, planned)
        });
        let mut desk = Self::with(Arc::new(move |v| state.publish_meter(v)), facts);
        desk.daemon = Some(Arc::downgrade(d));
        desk
    }

    /// A desk over anything, for tests.
    pub fn with(publish: Publish, facts: Facts) -> Self {
        Self {
            daemon: None,
            lease: Arc::new(Mutex::new(Lease {
                until: None,
                running: false,
            })),
            publish,
            facts,
        }
    }

    fn lease_view(until: Option<LeaseInstant>) -> MeterLeaseView {
        MeterLeaseView {
            open: until.is_some(),
            until: until.map(|u| {
                (chrono::Utc::now()
                    + chrono::Duration::from_std(u.saturating_duration_since(LeaseInstant::now()))
                        .unwrap_or_default())
                .to_rfc3339()
            }),
            per_second: METER_PER_SECOND,
        }
    }

    fn spawn(&self) {
        let lease = self.lease.clone();
        let publish = self.publish.clone();
        let facts = self.facts.clone();
        let daemon = self.daemon.clone();
        tokio::spawn(async move {
            let every = Duration::from_millis(1000 / u64::from(METER_PER_SECOND));
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let open = {
                    let mut l = lease.lock().unwrap_or_else(|e| e.into_inner());
                    let gone = daemon.as_ref().is_some_and(|d| d.upgrade().is_none());
                    let open = !gone && l.until.is_some_and(|u| LeaseInstant::now() < u);
                    if !open {
                        l.until = None;
                        l.running = false;
                        meter::set_open(false);
                    }
                    open
                };
                if !open {
                    publish(closed());
                    return;
                }
                let (off, planned) = facts();
                publish(view_of(meter::take(), off, planned));
            }
        });
    }
}

impl MeterPort for MeterDesk {
    fn set(&self, open: bool) -> MeterLeaseView {
        let mut l = self.lease.lock().unwrap_or_else(|e| e.into_inner());
        if !open {
            // The task sees it at its next tick, closes the stages' door and says so.
            l.until = None;
            if !l.running {
                meter::set_open(false);
            }
            return Self::lease_view(None);
        }
        let until = LeaseInstant::now() + Duration::from_secs(METER_LEASE_SECS);
        l.until = Some(until);
        if !l.running {
            l.running = true;
            meter::set_open(true);
            drop(l);
            self.spawn();
        }
        Self::lease_view(Some(until))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(source: Source, since: f64) -> Reading {
        Reading {
            source: Some(source),
            peak: Some(-30.04),
            level: Some(-41.26),
            speaking: true,
            room: None,
            room_gate: false,
            since_frame: Some(since),
            decisions: vec![(
                0.1,
                Decision::Wake {
                    score: 0.734,
                    woke: Some(true),
                },
            )],
        }
    }

    #[test]
    fn listening_off_shows_nothing_measured() {
        let v = view_of(
            reading(Source::Passive, 0.0),
            true,
            (true, RoomGateWhy::ThisMicrophone),
        );
        assert_eq!(v.state, MeterState::ListeningOff);
        assert!(v.level.is_none() && v.peak.is_none() && !v.speaking);
        assert!(v.decisions.is_empty());
    }

    #[test]
    fn on_the_array_the_session_says_the_room_gate_is_off_and_why() {
        let planned = planned_gate(None, false);
        assert_eq!(planned, (false, RoomGateWhy::OnChipArray));
        let v = view_of(reading(Source::Session, 0.02), false, planned);
        assert_eq!(v.state, MeterState::Session);
        assert!(!v.room_gate.on);
        assert_eq!(v.room_gate.why, RoomGateWhy::OnChipArray);
        assert_eq!(v.level, Some(-41.3));
        // A setting outranks the microphone.
        assert_eq!(
            planned_gate(Some(false), true),
            (false, RoomGateWhy::Setting)
        );
    }

    #[test]
    fn passive_audio_gives_levels_and_wake_scores_and_a_stale_one_is_quiet() {
        let v = view_of(
            reading(Source::Passive, 0.02),
            false,
            planned_gate(None, true),
        );
        assert_eq!(v.state, MeterState::Passive);
        assert_eq!(v.room_gate.why, RoomGateWhy::NoSession);
        assert_eq!(v.decisions[0].kind, MeterDecisionKind::Wake);
        assert_eq!(v.decisions[0].score, Some(0.73));
        assert_eq!(v.decisions[0].woke, Some(true));
        let stale = view_of(
            reading(Source::Passive, 3.0),
            false,
            planned_gate(None, true),
        );
        assert_eq!(stale.state, MeterState::Quiet);
        assert!(stale.level.is_none());
    }

    fn desk() -> (MeterDesk, Arc<Mutex<Vec<MeterView>>>) {
        let seen: Arc<Mutex<Vec<MeterView>>> = Arc::default();
        let s = seen.clone();
        let desk = MeterDesk::with(
            Arc::new(move |v| s.lock().unwrap().push(v)),
            Arc::new(|| (false, (true, RoomGateWhy::ThisMicrophone))),
        );
        (desk, seen)
    }

    #[tokio::test(start_paused = true)]
    async fn it_publishes_about_fifteen_a_second_and_closes_when_the_view_stops_asking() {
        let (desk, seen) = desk();
        let lease = desk.set(true);
        assert!(lease.open && lease.until.is_some());
        assert!(meter::is_open());
        tokio::time::sleep(Duration::from_secs(1)).await;
        let n = seen.lock().unwrap().len();
        assert!((14..=17).contains(&n), "{n} readings in a second");
        // Not asked again: closed by itself after the lease.
        tokio::time::sleep(Duration::from_secs(METER_LEASE_SECS)).await;
        let all = seen.lock().unwrap().clone();
        assert!(!all.last().unwrap().open, "the last reading says it closed");
        assert!(!meter::is_open(), "the stages stopped measuring");
        let count = all.len();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(seen.lock().unwrap().len(), count, "nothing after it closed");
    }

    #[tokio::test(start_paused = true)]
    async fn closing_stops_it_at_the_next_tick() {
        let (desk, seen) = desk();
        desk.set(true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let lease = desk.set(false);
        assert!(!lease.open);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let all = seen.lock().unwrap().clone();
        assert!(!all.last().unwrap().open);
        assert!(!meter::is_open());
    }
}
