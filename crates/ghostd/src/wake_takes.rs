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
}

impl ArrayTape {
    pub fn new(node: String) -> Arc<Self> {
        Self::with_opener(node, live_opener())
    }

    /// The same tape over another opener (a test's).
    pub fn with_opener(node: String, opener: TapeOpener) -> Arc<Self> {
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
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    /// Open the capture, unless it is open. A tape that starts holds nothing from before:
    /// the ring is cleared and moves to a new epoch, so a capture told to stop that still
    /// had a frame in hand cannot add it. The capture opens off the runtime; the task owns
    /// it and closes it (dropping it joins its thread) before it ends.
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
        let (node, ring, opener, token) = (
            self.node.clone(),
            self.ring.clone(),
            self.opener.clone(),
            stop.clone(),
        );
        // A stopping capture left in the slot is not waited on here: its writes are already
        // refused, and `stop` is what dictation waits on (it joins the newest).
        *task = Some(Running {
            stop,
            task: tokio::spawn(async move {
                // Never dropped mid-open: a cancel waits for the open, then closes what opened.
                let opened = tokio::task::spawn_blocking(move || opener(&node)).await;
                let mut cap = match opened {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => {
                        warn!("wake takes: the array's stream did not open: {e}");
                        return;
                    }
                    Err(_) => return,
                };
                loop {
                    let frame = tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
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
                drop(cap); // the capture's thread is joined here
            }),
        });
    }

    /// Refuse the running capture's writes and forget what the tape held, at once, and tell
    /// the capture to close; it stays in its slot for `stop` to wait on.
    fn halt(&self) {
        lock(&self.ring).turn();
        if let Some(running) = lock(&self.task).as_ref() {
            running.stop.cancel();
        }
    }

    /// Close the capture and forget what it held, and return only once the capture is
    /// closed: an open in flight is waited for and closed too. The passive loop awaits this
    /// before it gives the microphone to dictation (ADR 0077).
    pub async fn stop(&self) {
        self.halt();
        let running = lock(&self.task).take();
        if let Some(running) = running {
            let _ = running.task.await;
        }
    }

    /// The same without waiting: shutdown, where nothing can wait.
    pub fn stop_now(&self) {
        self.halt();
        if let Some(running) = lock(&self.task).take() {
            running.task.abort();
        }
    }

    /// No capture of the tape's is open or opening.
    pub fn idle(&self) -> bool {
        lock(&self.task)
            .as_ref()
            .is_none_or(|t| t.task.is_finished())
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

/// A wait dropped mid-way (the passive loop chose something else): the tape's writes are
/// refused at once; `stop`, which the loop always awaits next, waits for its capture.
struct StopOnDrop(Arc<ArrayTape>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.halt();
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
        self.inner.stop().await;
        self.tape.stop().await;
    }

    fn quiet(&self) -> bool {
        self.inner.quiet() && self.tape.idle()
    }

    fn close(&self) {
        self.tape.stop_now();
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
    fn the_tape_holds_a_whole_accepted_take_written_after_a_long_request() {
        // 12 s before the hit and 1.5 s after, written when the operator finishes up to
        // 12 s after the hit: the array's copy is as long as the detector's.
        let tape = ArrayTape::new("n".into());
        tape.push(
            &vec![3i16; 40 * 16_000 * 2],
            Instant::now() + Duration::from_secs(5),
        );
        let (_, pcm) = tape.slice(13.5, 10.5).unwrap();
        assert_eq!(pcm.len(), (13.5 * 16_000.0) as usize * 2);
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
        tape.stop_now();
        assert!(tape.slice(1.0, 0.0).is_none());
    }

    /// A tape capture for tests: a frame of `value` every 5 ms, slow to open and to close,
    /// counting how many are open.
    struct SlowMic {
        open: Arc<std::sync::atomic::AtomicIsize>,
        value: i16,
    }

    #[async_trait]
    impl ghost_voice::transport::MediaIn for SlowMic {
        async fn next_frame(&mut self) -> Option<ghost_voice::frames::Audio> {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some(ghost_voice::frames::Audio::new(vec![self.value; 160], RATE))
        }
    }

    impl Drop for SlowMic {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(50)); // as a capture joins its thread
            self.open.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn slow_tape(
        open: Arc<std::sync::atomic::AtomicIsize>,
        opens: Arc<AtomicUsize>,
    ) -> Arc<ArrayTape> {
        ArrayTape::with_opener(
            "n".into(),
            Arc::new(move |_node: &str| {
                opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150)); // a slow open
                open.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // An open finished: counted down from the opens begun.
                opens.fetch_add(1 << 16, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(SlowMic {
                    open: open.clone(),
                    value: 7,
                })
                    as Box<dyn ghost_voice::transport::MediaIn>)
            }),
        )
    }

    use std::sync::atomic::AtomicUsize;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_the_tape_waits_for_an_open_in_flight_and_its_close() {
        // Finding 2: the passive loop hands the microphone over only after this returns, so
        // when it returns no capture of the tape's is open or about to open.
        let open = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let opens = Arc::new(AtomicUsize::new(0));
        let tape = slow_tape(open.clone(), opens.clone());
        tape.start();
        tokio::time::sleep(Duration::from_millis(30)).await; // the open is in flight
        assert_eq!(
            opens.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "begun, not finished"
        );
        tape.stop().await;
        assert_eq!(
            opens.load(std::sync::atomic::Ordering::SeqCst),
            1 + (1 << 16),
            "stop returned with an open still in flight"
        );
        assert_eq!(
            open.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a capture is open"
        );
        assert!(tape.idle());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            open.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "one opened after"
        );
        assert!(tape.slice(1.0, 0.0).is_none(), "the tape kept something");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_running_tape_stops_with_its_capture_closed_and_nothing_written_after() {
        let open = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let tape = slow_tape(open.clone(), Arc::new(AtomicUsize::new(0)));
        tape.start();
        for _ in 0..100 {
            if tape.slice(0.01, 0.0).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(tape.slice(0.01, 0.0).is_some(), "the tape records");
        tape.stop().await;
        assert_eq!(open.load(std::sync::atomic::Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            tape.slice(1.0, 0.0).is_none(),
            "a frame was written after the stop"
        );
        // a stop told while a wait is dropped refuses writes at once, before the join
        tape.start();
        tokio::time::sleep(Duration::from_millis(250)).await;
        tape.halt();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(tape.slice(1.0, 0.0).is_none(), "written after the halt");
        tape.stop().await;
        assert_eq!(open.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
