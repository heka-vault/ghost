//! Kept wake takes, the daemon's side (ADR 0065, `ghost_core::wake_takes`).
//!
//! - [`Keeper`] is what the wake processor hands each decided hit to: it adds the input's
//!   name and, when the array's own stream is captured beside the one the detector hears,
//!   the same moment from it, and writes the take off the async runtime.
//! - [`ArrayTape`] is that second stream: the raw array under `ghost.mic` (the capture
//!   target of `echo-cancel.conf`, as the Hearing test finds it), kept in memory for the
//!   last 20 s and only while the passive listener runs ([`TapedWake`]). It is never
//!   opened while listening is off (ADR 0057): the passive loop does not wait then.
//! - [`WakeTakesDesk`] serves the control API's routes: the list, the choice, labels,
//!   deleting, and playing a take through Ghost's own speaker.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_api::wake_takes::{
    WakeTakeLabel, WakeTakeOutcome, WakeTakeView, WakeTakesChoice, WakeTakesError, WakeTakesPort,
    WakeTakesResult, WakeTakesView,
};
use ghost_audio::{Capture, CaptureConfig};
use ghost_core::time::iso;
use ghost_core::wake_takes::{
    Choice, Label, MAX_BYTES, MAX_TAKES, NewTake, Outcome, RATE, TakeMeta, Takes,
};
use ghost_voice::wakeword::TakeKeeper;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::daemon::{Daemon, lock};
use crate::wake::WakeDetector;

/// How much of the array's stream is kept in memory: more than any take.
pub const TAPE_SECS: f64 = 20.0;
const RAW_CHANNELS: u16 = 2;

// --- the array's stream beside the detector's ----------------------------------------------

struct Ring {
    buf: VecDeque<i16>,
    channels: u16,
    newest: Option<Instant>,
}

pub struct ArrayTape {
    node: String,
    ring: Arc<Mutex<Ring>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ArrayTape {
    pub fn new(node: String) -> Arc<Self> {
        Arc::new(Self {
            node,
            ring: Arc::new(Mutex::new(Ring {
                buf: VecDeque::new(),
                channels: RAW_CHANNELS,
                newest: None,
            })),
            task: Mutex::new(None),
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    /// Open the capture, unless it is open.
    pub fn start(self: &Arc<Self>) {
        let mut task = lock(&self.task);
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let node = self.node.clone();
        let ring = self.ring.clone();
        *task = Some(tokio::spawn(async move {
            let opened = tokio::task::spawn_blocking(move || {
                Capture::open(CaptureConfig {
                    node: Some(node),
                    channels: RAW_CHANNELS,
                    ring_seconds: 2.0,
                    name: "ghost-wake-array".into(),
                    description: "Ghost (kept wake takes, the array)".into(),
                    ..CaptureConfig::default()
                })
            })
            .await;
            let mut cap = match opened {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    warn!("wake takes: the array's stream did not open: {e}");
                    return;
                }
                Err(_) => return,
            };
            let keep = (TAPE_SECS * f64::from(RATE)) as usize * usize::from(RAW_CHANNELS);
            while let Some(frame) = cap.next_frame().await {
                let mut r = lock(&ring);
                r.buf.extend(frame.samples.iter().copied());
                let over = r.buf.len().saturating_sub(keep);
                r.buf.drain(..over);
                r.newest = Some(Instant::now());
            }
        }));
    }

    /// Close the capture and forget what it held.
    pub fn stop(&self) {
        if let Some(t) = lock(&self.task).take() {
            t.abort();
        }
        let mut r = lock(&self.ring);
        r.buf.clear();
        r.newest = None;
    }

    /// `push` for tests: as the capture loop does.
    pub fn push(&self, samples: &[i16], at: Instant) {
        let mut r = lock(&self.ring);
        r.buf.extend(samples.iter().copied());
        r.newest = Some(at);
    }

    /// `secs` of the array's stream ending `ago` seconds before now, interleaved: as much
    /// of it as the tape holds.
    pub fn slice(&self, secs: f64, ago: f64) -> Option<(u16, Vec<i16>)> {
        let r = lock(&self.ring);
        let newest = r.newest?;
        let ch = usize::from(r.channels);
        let since = Instant::now()
            .saturating_duration_since(newest)
            .as_secs_f64();
        let back = ((ago - since).max(0.0) * f64::from(RATE)).round() as usize * ch;
        let want = (secs * f64::from(RATE)) as usize * ch;
        let end = r.buf.len().checked_sub(back)?;
        let start = end.saturating_sub(want);
        let out: Vec<i16> = r.buf.range(start..end).copied().collect();
        (!out.is_empty()).then_some((r.channels, out))
    }
}

// --- the keeper -----------------------------------------------------------------------------

pub struct Keeper {
    takes: Arc<Takes>,
    input: String,
    array: Option<Arc<ArrayTape>>,
}

impl Keeper {
    pub fn new(takes: Arc<Takes>, input: String, array: Option<Arc<ArrayTape>>) -> Self {
        Self {
            takes,
            input,
            array,
        }
    }
}

impl TakeKeeper for Keeper {
    fn wants(&self) -> bool {
        self.takes.wants()
    }

    fn keep(&self, mut take: NewTake, tail_secs: f64) {
        take.input = self.input.clone();
        if let Some(tape) = &self.array {
            let secs = take.audio.len() as f64 / f64::from(RATE);
            if let Some((ch, pcm)) = tape.slice(secs, tail_secs) {
                take.array = Some((tape.node().to_string(), ch, pcm));
            }
        }
        let takes = self.takes.clone();
        let write = move || match takes.keep(take) {
            Ok(Some(m)) => info!("{}", Takes::describe(&m)),
            Ok(None) => {}
            Err(e) => warn!("wake takes: a take could not be kept: {e}"),
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(write);
            }
            Err(_) => write(),
        }
    }
}

// --- the tape opens and closes with the passive listener ------------------------------------

/// The wake detector with the array's tape running while it waits.
pub struct TapedWake {
    inner: Arc<dyn WakeDetector>,
    tape: Arc<ArrayTape>,
}

impl TapedWake {
    pub fn new(inner: Arc<dyn WakeDetector>, tape: Arc<ArrayTape>) -> Self {
        Self { inner, tape }
    }
}

struct StopOnDrop(Arc<ArrayTape>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

#[async_trait]
impl WakeDetector for TapedWake {
    fn arm(&self) {
        self.inner.arm();
    }

    async fn wait(&self) -> Option<String> {
        self.tape.start();
        let _stop = StopOnDrop(self.tape.clone());
        self.inner.wait().await
    }

    async fn stop(&self) {
        self.tape.stop();
        self.inner.stop().await;
    }

    fn close(&self) {
        self.tape.stop();
        self.inner.close();
    }
}

// --- the control API's side -----------------------------------------------------------------

fn outcome_view(o: Outcome) -> WakeTakeOutcome {
    match o {
        Outcome::Accepted => WakeTakeOutcome::Accepted,
        Outcome::Refused => WakeTakeOutcome::Refused,
    }
}

fn label_view(l: Label) -> WakeTakeLabel {
    match l {
        Label::Me => WakeTakeLabel::Me,
        Label::Not => WakeTakeLabel::Not,
        Label::MeMissedBefore => WakeTakeLabel::MeMissedBefore,
    }
}

fn label_of(l: WakeTakeLabel) -> Label {
    match l {
        WakeTakeLabel::Me => Label::Me,
        WakeTakeLabel::Not => Label::Not,
        WakeTakeLabel::MeMissedBefore => Label::MeMissedBefore,
    }
}

pub fn take_view(m: TakeMeta) -> WakeTakeView {
    WakeTakeView {
        id: m.id,
        at: iso(&m.at),
        outcome: outcome_view(m.outcome),
        model: m.model,
        score: m.score,
        threshold: m.threshold,
        input: m.input,
        array: m.array,
        secs: m.secs,
        hit_at: m.hit_at,
        scores: m.scores,
        scores_end: m.scores_end,
        reading: m.reading,
        label: m.label.map(label_view),
        bytes: m.bytes,
    }
}

fn not_found(e: std::io::Error) -> WakeTakesError {
    if e.kind() == std::io::ErrorKind::NotFound {
        WakeTakesError::NotFound("no such take".into())
    } else {
        WakeTakesError::Failed(e.to_string())
    }
}

pub struct WakeTakesDesk {
    takes: Arc<Takes>,
    d: Weak<Daemon>,
    playing: Mutex<Option<(String, JoinHandle<()>)>>,
}

impl WakeTakesDesk {
    pub fn new(takes: Arc<Takes>, d: Weak<Daemon>) -> Arc<Self> {
        Arc::new(Self {
            takes,
            d,
            playing: Mutex::new(None),
        })
    }
}

#[async_trait]
impl WakeTakesPort for WakeTakesDesk {
    fn view(&self) -> WakeTakesView {
        let c = self.takes.choice();
        let list = self.takes.list();
        let playing = lock(&self.playing)
            .as_ref()
            .filter(|(_, h)| !h.is_finished())
            .map(|(id, _)| id.clone());
        WakeTakesView {
            choice: WakeTakesChoice {
                keep: c.keep,
                unlabelled_accepted_are_me: c.unlabelled_accepted_are_me,
            },
            count: list.len(),
            bytes: list.iter().map(|t| t.bytes).sum(),
            max_count: MAX_TAKES,
            max_bytes: MAX_BYTES,
            playing,
            takes: list.into_iter().map(take_view).collect(),
        }
    }

    fn set_choice(&self, c: WakeTakesChoice) -> WakeTakesResult<WakeTakesView> {
        self.takes
            .set_choice(Choice {
                keep: c.keep,
                unlabelled_accepted_are_me: c.unlabelled_accepted_are_me,
            })
            .map_err(|e| WakeTakesError::Failed(e.to_string()))?;
        info!("wake takes: keeping {}", if c.keep { "on" } else { "off" });
        Ok(self.view())
    }

    fn label(&self, id: &str, label: Option<WakeTakeLabel>) -> WakeTakesResult<WakeTakeView> {
        self.takes
            .label(id, label.map(label_of))
            .map(take_view)
            .map_err(not_found)
    }

    fn delete(&self, id: &str) -> WakeTakesResult<()> {
        match self.takes.delete(id) {
            Ok(true) => Ok(()),
            Ok(false) => Err(WakeTakesError::NotFound("no such take".into())),
            Err(e) => Err(not_found(e)),
        }
    }

    fn delete_all(&self) -> WakeTakesResult<usize> {
        let n = self
            .takes
            .delete_all()
            .map_err(|e| WakeTakesError::Failed(e.to_string()))?;
        info!("wake takes: deleted all {n}");
        Ok(n)
    }

    async fn play(&self, id: &str, array: bool) -> WakeTakesResult<()> {
        let d = self
            .d
            .upgrade()
            .ok_or_else(|| WakeTakesError::Failed("Ghost is stopping".into()))?;
        let pcm = self.takes.audio(id, array).map_err(not_found)?;
        let play = d.ports.play.clone();
        let device = d.voice().output_device.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = play(pcm, RATE, device).await {
                warn!("wake takes: playing a take failed: {e}");
            }
        });
        if let Some((_, old)) = lock(&self.playing).replace((id.to_string(), handle)) {
            old.abort();
        }
        Ok(())
    }

    async fn stop_playing(&self) {
        if let Some((_, h)) = lock(&self.playing).take() {
            h.abort();
        }
    }
}

/// Give a take a moment to be written: for tests.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(n: usize) -> NewTake {
        NewTake {
            audio: vec![1; n],
            hit: n / 2,
            accepted: true,
            model: "hey_ghost".into(),
            score: 0.7,
            threshold: 0.6,
            ..NewTake::default()
        }
    }

    #[tokio::test]
    async fn the_keeper_names_the_input_and_takes_the_array_beside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let takes = Arc::new(Takes::new(tmp.path()));
        let tape = ArrayTape::new("alsa_input.array".into());
        // 3 s of the array, the newest just now
        tape.push(&vec![5i16; 3 * 16_000 * 2], Instant::now());
        let k = Keeper::new(takes.clone(), "ghost.mic".into(), Some(tape));
        k.keep(take(16_000), 0.0);
        for _ in 0..40 {
            if !takes.list().is_empty() {
                break;
            }
            settle().await;
        }
        let m = &takes.list()[0];
        assert_eq!(m.input, "ghost.mic");
        assert_eq!(m.array.as_deref(), Some("alsa_input.array"));
        assert_eq!(takes.audio(&m.id, true).unwrap().len(), 16_000);
    }

    #[tokio::test]
    async fn off_keeps_nothing_through_the_keeper() {
        let tmp = tempfile::tempdir().unwrap();
        let takes = Arc::new(Takes::new(tmp.path()));
        takes
            .set_choice(Choice {
                keep: false,
                ..Choice::default()
            })
            .unwrap();
        let k = Keeper::new(takes.clone(), "ghost.mic".into(), None);
        assert!(!k.wants());
        k.keep(take(16_000), 0.0);
        settle().await;
        settle().await;
        assert!(!tmp.path().join(ghost_core::wake_takes::FOLDER).exists());
    }

    #[test]
    fn the_tape_slices_back_from_the_take_end() {
        let tape = ArrayTape::new("n".into());
        let mut pcm = vec![0i16; 4 * 16_000 * 2];
        // the last second is 9s
        for s in pcm[3 * 16_000 * 2..].iter_mut() {
            *s = 9;
        }
        // newest "now" whatever the test's own pace (a later instant reads as no time since)
        tape.push(&pcm, Instant::now() + Duration::from_secs(5));
        let (ch, last) = tape.slice(1.0, 0.0).unwrap();
        assert_eq!(ch, 2);
        assert_eq!(last.len(), 32_000);
        assert!(last.iter().all(|&s| s == 9));
        // the take ended a second before the newest sample: the second before that
        let (_, before) = tape.slice(1.0, 1.0).unwrap();
        assert!(before.iter().all(|&s| s == 0));
        tape.stop();
        assert!(tape.slice(1.0, 0.0).is_none());
    }
}
