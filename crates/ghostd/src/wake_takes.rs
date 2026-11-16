//! Kept wake takes, the daemon's side (ADR 0065, `ghost_core::wake_takes`).
//!
//! - [`Keeper`] is what the wake processor hands each decided hit to: it adds the input's
//!   name and, when the array's own stream is captured beside the one the detector hears,
//!   the same moment from it, and writes the take off the async runtime.
//! - [`ArrayTape`] is that second stream: the raw array under `ghost.mic` (the capture
//!   target of `echo-cancel.conf`, as the Hearing test finds it), kept in memory for the
//!   last 30 s and only while the passive listener runs ([`TapedWake`]). It is never
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
use ghost_audio::live::{Holder, Registry};
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

/// How much of the array's stream is kept in memory: more than any take and the longest
/// request after its hit (an accepted take is written when the operator finishes, up to
/// `MAX_LISTEN_SECS` after it), so the array's copy is as long as the detector's.
pub const TAPE_SECS: f64 = 30.0;
const RAW_CHANNELS: u16 = 2;

// --- the array's stream beside the detector's ----------------------------------------------

struct Ring {
    buf: VecDeque<i16>,
    channels: u16,
    newest: Option<Instant>,
    /// Which capture may write: each start and each stop moves it on, so a frame from a
    /// capture that was told to stop is refused here, under the ring's own lock.
    epoch: u64,
}

impl Ring {
    /// Add the newest samples, keeping the last `TAPE_SECS`.
    fn add(&mut self, samples: &[i16], at: Instant) {
        let keep = (TAPE_SECS * f64::from(RATE)) as usize * usize::from(self.channels);
        self.buf.extend(samples.iter().copied());
        let over = self.buf.len().saturating_sub(keep);
        self.buf.drain(..over);
        self.newest = Some(at);
    }

    /// Forget everything and refuse the writer of before; the new epoch.
    fn turn(&mut self) -> u64 {
        self.buf.clear();
        self.newest = None;
        self.epoch += 1;
        self.epoch
    }
}

/// Opens the array's raw stream: the live one through `ghost_audio`, or a test's.
pub type TapeOpener =
    Arc<dyn Fn(&str) -> Result<Box<dyn ghost_voice::transport::MediaIn>, String> + Send + Sync>;

/// The live array: every channel of the raw node, into memory.
fn live_opener() -> TapeOpener {
    Arc::new(|node: &str| {
        Capture::open(CaptureConfig {
            node: Some(node.to_string()),
            channels: RAW_CHANNELS,
            ring_seconds: 2.0,
            name: "ghost-wake-array".into(),
            description: "Ghost (kept wake takes, the array)".into(),
            ..CaptureConfig::default()
        })
        .map(|c| Box::new(crate::wake::CaptureIn(c)) as Box<dyn ghost_voice::transport::MediaIn>)
        .map_err(|e| e.to_string())
    })
}

struct Running {
    stop: tokio_util::sync::CancellationToken,
    task: JoinHandle<()>,
}

pub struct ArrayTape {
    node: String,
    ring: Arc<Mutex<Ring>>,
    task: Mutex<Option<Running>>,
    opener: TapeOpener,
    /// Counts the tape's captures with every other desk capture (ADR 0077); None in a test
    /// that does not look.
    captures: Option<Arc<Registry>>,
}

impl ArrayTape {
    pub fn new(node: String, captures: Option<Arc<Registry>>) -> Arc<Self> {
        Self::with_opener(node, live_opener(), captures)
    }

    /// The same tape over another opener (a test's).
    pub fn with_opener(
        node: String,
        opener: TapeOpener,
        captures: Option<Arc<Registry>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            node,
            ring: Arc::new(Mutex::new(Ring {
                buf: VecDeque::new(),
                channels: RAW_CHANNELS,
                newest: None,
                epoch: 0,
            })),
            task: Mutex::new(None),
            opener,
            captures,
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    /// Open the capture, unless it is open. A tape that starts holds nothing from before:
    /// the ring is cleared and moves to a new epoch, so a capture told to stop that still
    /// had a frame in hand cannot add it. The capture opens off the runtime and is counted
    /// in the live-capture registry from before its open until after it is closed: the token
    /// goes into the blocking open and comes back with the capture, so even a task that was
    /// retired while it opened is counted until what it opened is gone.
    pub fn start(self: &Arc<Self>) {
        let mut task = lock(&self.task);
        if task
            .as_ref()
            .is_some_and(|t| !t.stop.is_cancelled() && !t.task.is_finished())
        {
            return;
        }
        let epoch = lock(&self.ring).turn();
        let stop = tokio_util::sync::CancellationToken::new();
        let (node, ring, opener, cancel) = (
            self.node.clone(),
            self.ring.clone(),
            self.opener.clone(),
            stop.clone(),
        );
        let token = self.captures.as_ref().map(|r| r.take(Holder::Tape));
        // A retired task left running is not waited on (as before ADR 0077): its writes are
        // refused by the epoch, and the registry counts its capture until it is gone.
        *task = Some(Running {
            stop,
            task: tokio::spawn(async move {
                let opened = tokio::task::spawn_blocking(move || (opener(&node), token)).await;
                let (mut cap, _token) = match opened {
                    Ok((Ok(c), t)) => (c, t),
                    Ok((Err(e), _)) => {
                        warn!("wake takes: the array's stream did not open: {e}");
                        return;
                    }
                    Err(_) => return,
                };
                loop {
                    let frame = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        f = cap.next_frame() => f,
                    };
                    let Some(frame) = frame else { break };
                    let mut r = lock(&ring);
                    if r.epoch != epoch {
                        break; // told to stop while this frame was in hand
                    }
                    r.add(&frame.samples, Instant::now());
                }
                cap.close();
                drop(cap); // the capture's thread is joined here; the token goes after it
            }),
        });
    }

    /// Refuse the running capture's writes and forget what the tape held, at once, and tell
    /// the capture to close.
    fn halt(&self) {
        lock(&self.ring).turn();
        if let Some(running) = lock(&self.task).as_ref() {
            running.stop.cancel();
        }
    }

    /// Close the capture and forget what it held, without waiting for it, as before ADR
    /// 0077: the capture closes in its own time, and whoever must know when it is gone (the
    /// dictation grant) asks the live-capture registry.
    pub fn stop(&self) {
        self.halt();
        lock(&self.task).take();
    }

    /// The ring's lock, held by a test to catch a frame in hand.
    #[cfg(test)]
    fn ring_for_test(&self) -> std::sync::MutexGuard<'_, Ring> {
        lock(&self.ring)
    }

    /// `push` for tests: as the capture loop does.
    pub fn push(&self, samples: &[i16], at: Instant) {
        lock(&self.ring).add(samples, at);
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

/// A wait dropped mid-way (the passive loop chose something else): the tape stops, its
/// writes refused at once.
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
        cut_at_start: m.cut_at_start,
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
        let tape = ArrayTape::new("alsa_input.array".into(), None);
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
    fn the_tape_holds_a_whole_accepted_take_written_after_a_long_request() {
        // 12 s before the hit and 1.5 s after, written when the operator finishes up to
        // 12 s after the hit: the array's copy is as long as the detector's.
        let tape = ArrayTape::new("n".into(), None);
        tape.push(
            &vec![3i16; 40 * 16_000 * 2],
            Instant::now() + Duration::from_secs(5),
        );
        let (_, pcm) = tape.slice(13.5, 10.5).unwrap();
        assert_eq!(pcm.len(), (13.5 * 16_000.0) as usize * 2);
    }

    #[test]
    fn the_tape_slices_back_from_the_take_end() {
        let tape = ArrayTape::new("n".into(), None);
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

    /// A tape capture for tests: a frame of `value` every 5 ms, slow to close, saying when
    /// it has a frame in hand.
    struct SlowMic {
        value: i16,
        handed: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ghost_voice::transport::MediaIn for SlowMic {
        async fn next_frame(&mut self) -> Option<ghost_voice::frames::Audio> {
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.handed.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(ghost_voice::frames::Audio::new(vec![self.value; 160], RATE))
        }
    }

    impl Drop for SlowMic {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(50)); // as a capture joins its thread
        }
    }

    fn slow_tape(
        open_ms: u64,
        handed: Arc<std::sync::atomic::AtomicBool>,
        captures: Option<Arc<Registry>>,
    ) -> Arc<ArrayTape> {
        ArrayTape::with_opener(
            "n".into(),
            Arc::new(move |_node: &str| {
                std::thread::sleep(Duration::from_millis(open_ms)); // a slow open
                Ok(Box::new(SlowMic {
                    value: 7,
                    handed: handed.clone(),
                })
                    as Box<dyn ghost_voice::transport::MediaIn>)
            }),
            captures,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frame_held_across_a_stop_is_refused_by_the_generation() {
        // The writer has a frame in hand and is about to take the ring when the stop comes:
        // the generation it started with no longer matches, so the frame is not added.
        let handed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tape = slow_tape(0, handed.clone(), None);
        tape.start();
        tokio::time::sleep(Duration::from_millis(50)).await; // it records
        {
            let mut ring = tape.ring_for_test();
            ring.buf.clear();
            handed.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        // Hold the ring so the writer blocks with its next frame in hand, on a thread of its
        // own: the runtime's workers stay free for the writer.
        let held = {
            let (tape, handed) = (tape.clone(), handed.clone());
            std::thread::spawn(move || {
                let mut ring = tape.ring_for_test();
                while !handed.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                std::thread::sleep(Duration::from_millis(20)); // it is waiting on the ring
                // What `stop` does, while the writer waits: a new generation, and the cancel.
                ring.turn();
            })
        };
        while !held.is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if let Some(r) = lock(&tape.task).as_ref() {
            r.stop.cancel();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            tape.ring_for_test().buf.is_empty(),
            "a frame held across the stop was added"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_tapes_captures_are_counted_until_gone_a_retired_one_too() {
        // ADR 0077: counted from before the open (an open in flight counts) until after the
        // capture is closed; a tape stopped and started again while its predecessor is still
        // opening counts both; `stop` itself waits for nothing.
        let captures = Registry::new();
        let tape = slow_tape(
            150,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Some(captures.clone()),
        );
        tape.start();
        assert_eq!(
            captures.alive_for(Holder::Tape),
            1,
            "the open in flight counts"
        );
        let began = Instant::now();
        tape.stop();
        assert!(began.elapsed() < Duration::from_millis(20), "stop waited");
        tape.start(); // the predecessor is still opening
        assert_eq!(captures.alive_for(Holder::Tape), 2);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            captures.alive_for(Holder::Tape) >= 1,
            "the running one counts"
        );
        tape.stop();
        assert!(
            captures
                .until(Duration::from_secs(2), |c| c.of(Holder::Tape) == 0)
                .await,
            "a tape capture outlived its count"
        );
        assert!(tape.slice(1.0, 0.0).is_none(), "the tape kept something");
    }
}
