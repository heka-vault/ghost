//! The live microphone meter (ADR 0073, amended 2026-11-14): levels and decisions by kind,
//! for Settings › Developer's meter only, while it is open.
//!
//! The stages that already measure a frame's level or decide about a segment (the passive
//! listener's VAD, the session's speech-in stage and room gate, the echo filter, the barge-in
//! rule, the wake detector) report here. While the meter is closed, which is always unless
//! the operator has Developer's meter view open, every function is one relaxed atomic load
//! and returns: nothing is measured, formatted or kept.
//!
//! What it can hold is closed by its types: a level in dBFS, whether the VAD hears speech,
//! the room gate's three levels, and a [`Decision`], an enum of fixed kinds with numbers.
//! There is no field for text, so no transcript, word or name can reach the meter, in a
//! session or out of one; and no audio, since it keeps one number per frame. Nothing is kept
//! past the next read (`take`, about fifteen a second): no history.
//!
//! The microphone is never opened for it: it reads only what a pipeline that is already
//! listening measures. With listening off (ADR 0057) nothing is listening, so nothing comes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static OPEN: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State::new());

/// At most this many decisions wait for the next read; older ones go.
pub const DECISIONS_KEPT: usize = 32;
/// The room gate's levels are read at most this often (they sort ten seconds of levels).
const ROOM_EVERY_SECS: f64 = 0.1;

/// Where the audio being measured is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The passive listener, waiting for the wake: local models only (ADR 0010).
    Passive,
    /// A desk session.
    Session,
}

/// The room gate's levels as it sees them now (ADR 0072), dBFS.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RoomLevels {
    /// The room's loud level: the 90th percentile outside the person's speech.
    pub room: Option<f64>,
    /// The person's level, once heard.
    pub person: Option<f64>,
    /// The line under which a segment is the room's.
    pub line: Option<f64>,
}

/// What Ghost decided about a stretch of sound, by kind only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// Read as the person's and passed on to the turn (`words` how many).
    Taken { words: usize, dbfs: f64 },
    /// Read and nothing came of it: no words in it.
    NoWords { secs: f64, dbfs: f64 },
    /// Held as the room: below the person's level (`person`), at `dbfs`.
    HeldRoom { dbfs: f64, person: Option<f64> },
    /// A held segment let go at its end.
    LetGo { secs: f64, dbfs: f64 },
    /// A held segment that began with Ghost's name, so taken.
    TakenByName,
    /// Dropped as Ghost's own voice coming back.
    Echo,
    /// Heard over Ghost, too short to interrupt it on its own.
    TooShort,
    /// The person talked over Ghost and Ghost stopped.
    BargeIn,
    /// The wake detector scored a hit: `woke` once decided (Parakeet's check may refuse it).
    Wake { score: f32, woke: Option<bool> },
    /// The best score of a stretch that stayed under the detector's threshold.
    NearMiss { score: f32 },
}

/// One read of the meter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    pub source: Option<Source>,
    /// The loudest 20 ms frame since the last read, dBFS.
    pub peak: Option<f64>,
    /// The mean level since the last read, dBFS (of the frames' levels).
    pub level: Option<f64>,
    /// The VAD heard speech at the last frame.
    pub speaking: bool,
    /// The room gate's levels; None where it does not run (the on-chip array, the passive
    /// listener).
    pub room: Option<RoomLevels>,
    /// Whether the last frame came from a stage with a room gate.
    pub room_gate: bool,
    /// Seconds since the last frame; None when none came since the meter opened.
    pub since_frame: Option<f64>,
    /// What was decided since the last read, oldest first, each with when (seconds ago).
    pub decisions: Vec<(f64, Decision)>,
}

struct State {
    source: Option<Source>,
    peak: Option<f64>,
    sum: f64,
    n: u32,
    speaking: bool,
    room: Option<RoomLevels>,
    room_gate: bool,
    room_at: Option<Instant>,
    last_frame: Option<Instant>,
    decisions: Vec<(Instant, Decision)>,
}

impl State {
    const fn new() -> Self {
        Self {
            source: None,
            peak: None,
            sum: 0.0,
            n: 0,
            speaking: false,
            room: None,
            room_gate: false,
            room_at: None,
            last_frame: None,
            decisions: Vec::new(),
        }
    }
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Open or close the meter. Closing forgets everything it held.
pub fn set_open(on: bool) {
    OPEN.store(on, Ordering::SeqCst);
    *state() = State::new();
}

/// The meter is open: the stages report.
#[inline]
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// A frame of `samples` reached a stage that listens. `room` reads the room gate's levels
/// (only called when they are due); None from a stage with no gate.
pub fn frame(
    source: Source,
    samples: &[i16],
    speaking: bool,
    room: Option<&dyn Fn() -> RoomLevels>,
) {
    if !is_open() || samples.is_empty() {
        return;
    }
    let level = crate::stt::level_dbfs(samples);
    let now = Instant::now();
    let mut s = state();
    if s.source != Some(source) {
        // The audio moved between the passive listener and a session: start the read over.
        s.peak = None;
        s.sum = 0.0;
        s.n = 0;
        s.room = None;
    }
    s.source = Some(source);
    s.peak = Some(s.peak.map_or(level, |p| p.max(level)));
    s.sum += level;
    s.n += 1;
    s.speaking = speaking;
    s.room_gate = room.is_some();
    s.last_frame = Some(now);
    match room {
        Some(read) => {
            let due = s
                .room_at
                .is_none_or(|t| now.duration_since(t).as_secs_f64() >= ROOM_EVERY_SECS);
            if due {
                s.room = Some(read());
                s.room_at = Some(now);
            }
        }
        None => s.room = None,
    }
}

/// Ghost decided about a stretch of sound.
pub fn decided(d: Decision) {
    if !is_open() {
        return;
    }
    let mut s = state();
    if s.decisions.len() >= DECISIONS_KEPT {
        s.decisions.remove(0);
    }
    s.decisions.push((Instant::now(), d));
}

/// What the meter holds now, and forget it: the levels start over and the decisions go.
pub fn take() -> Reading {
    let now = Instant::now();
    let mut s = state();
    let r = Reading {
        source: s.source,
        peak: s.peak,
        level: (s.n > 0).then(|| s.sum / f64::from(s.n)),
        speaking: s.speaking,
        room: s.room,
        room_gate: s.room_gate,
        since_frame: s.last_frame.map(|t| now.duration_since(t).as_secs_f64()),
        decisions: s
            .decisions
            .drain(..)
            .map(|(at, d)| (now.duration_since(at).as_secs_f64(), d))
            .collect(),
    };
    s.peak = None;
    s.sum = 0.0;
    s.n = 0;
    r
}

/// The meter's process-wide state is shared by every test that opens it.
#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closed_it_holds_nothing() {
        let _g = TEST_LOCK.lock().await;
        set_open(false);
        frame(Source::Session, &[1000; 320], true, None);
        decided(Decision::Echo);
        let r = take();
        assert_eq!(r.source, None);
        assert!(r.decisions.is_empty() && r.peak.is_none());
    }

    #[tokio::test]
    async fn open_it_reads_levels_and_forgets_them_once_read() {
        let _g = TEST_LOCK.lock().await;
        set_open(true);
        frame(Source::Passive, &[0; 320], false, None);
        frame(Source::Passive, &[3277; 320], true, None); // -20 dBFS
        decided(Decision::NearMiss { score: 0.31 });
        let r = take();
        assert_eq!(r.source, Some(Source::Passive));
        assert!((r.peak.unwrap() + 20.0).abs() < 0.1, "{r:?}");
        assert!(r.speaking);
        assert!(!r.room_gate && r.room.is_none());
        assert_eq!(r.decisions.len(), 1);
        let again = take();
        assert!(again.decisions.is_empty() && again.peak.is_none());
        set_open(false);
    }

    #[tokio::test]
    async fn the_room_gate_is_read_at_most_ten_times_a_second() {
        let _g = TEST_LOCK.lock().await;
        set_open(true);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let read = || {
            calls.fetch_add(1, Ordering::SeqCst);
            RoomLevels {
                room: Some(-50.0),
                person: Some(-25.0),
                line: Some(-33.0),
            }
        };
        for _ in 0..50 {
            frame(Source::Session, &[500; 320], false, Some(&read));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let r = take();
        assert!(r.room_gate);
        assert_eq!(r.room.unwrap().person, Some(-25.0));
        set_open(false);
    }

    #[tokio::test]
    async fn a_burst_keeps_only_the_newest_decisions() {
        let _g = TEST_LOCK.lock().await;
        set_open(true);
        for _ in 0..(DECISIONS_KEPT + 10) {
            decided(Decision::TooShort);
        }
        decided(Decision::BargeIn);
        let r = take();
        assert_eq!(r.decisions.len(), DECISIONS_KEPT);
        assert_eq!(r.decisions.last().unwrap().1, Decision::BargeIn);
        set_open(false);
    }
}
