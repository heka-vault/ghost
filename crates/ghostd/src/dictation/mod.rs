//! Ghost's own dictation (ADR 0077, amended 2026-11-25): the operator holds or taps the
//! dictation key (`key`, through the desktop's shortcut portal, `portal`) or runs `ghostd
//! dictate`, speaks, and the Parakeet Ghost already has reads the audio, a piece at each
//! pause while they speak. From the key the text is tidied by the model when the operator
//! allows it (`review`) and typed into the window that has the keyboard (`typing`); from a
//! command, and whenever typing can't be done, it ends on the clipboard.
//!
//! - [`Dictation`] is the one coordinator: start, stop, toggle and cancel are serialized
//!   (`ops`), each dictation has a generation, and a read that finishes after its dictation
//!   was cancelled does nothing. Recording stops by itself at [`LIMIT`].
//! - The audio barrier is the passive loop's: dictation asks it for the microphone with a
//!   [`Lease`], as the Hearing test does, and the loop stops the wake listener before it
//!   grants it (its capture closed, its pipeline, pre-roll and reads dropped, and the kept
//!   takes' array tape stopped and cleared), and builds a fresh one only once dictation's
//!   own capture has closed. So no dictated frame reaches the wake detector, the passive VAD,
//!   Parakeet's wake check, a session's speech-to-text or the realtime uplink, and no kept
//!   take holds one.
//! - The words leave this module only for the window they are typed into, the clipboard,
//!   the operator's own desktop as the live view (`Live`, on its own channel, never the
//!   event bus) and, by the operator's switch, their model for the tidy: no log line, no
//!   event on the bus, no store, no `debug::audio`, no report. What the API, the CLI and the
//!   notification get is kinds and numbers ([`ghost_api::dictation`]). The audio and the text
//!   are wiped when dropped (`Zeroizing`).

pub mod cleanup;
pub mod clipboard;
pub mod desk;
pub mod key;
pub mod portal;
pub mod review;
pub mod typing;

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_api::Phase;
use ghost_api::dictation::{
    DictationLiveView, DictationOutcome, DictationPort, DictationReason, DictationResult,
    DictationScene, DictationState, DictationView,
};
use ghost_voice::stt::{SharedRecognizer, TRAILING_SECS, dithered};
use ghost_voice::transport::MediaIn;
use tokio::sync::{Notify, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use zeroize::Zeroizing;

pub use clipboard::{Clipboard, NoClipboard, XClipboard};
pub use review::{NoReviewer, Reviewer};

use crate::daemon::{Daemon, lock};

/// The longest a dictation records, held, locked by a tap or run from a command; it then
/// stops by itself and what was said is read (two minutes to start, 2026-11-25).
pub const LIMIT: Duration = Duration::from_secs(120);
/// A piece is read at a pause only once it is at least this long; shorter ones wait for the
/// next pause or the end.
pub const PIECE_MIN: Duration = Duration::from_millis(600);
/// How often the live view's level is sent while recording (about fifteen a second).
pub const LEVEL_EVERY: Duration = Duration::from_millis(66);
/// The rate the capture gives and Parakeet reads.
pub const RATE: u32 = 16_000;
/// How long a start waits for the passive loop to let go of the microphone.
pub const LEASE_WAIT: Duration = Duration::from_secs(3);
/// How long a start waits for the microphone's first frame.
pub const FIRST_FRAME_WAIT: Duration = Duration::from_secs(3);

// --- the microphone, taken from the passive loop ---------------------------------------------

/// The passive loop's side of a lease: say the microphone is free, then wait to get it back.
pub struct Lease {
    pub granted: oneshot::Sender<()>,
    pub release: oneshot::Receiver<()>,
}

/// The microphone while dictation holds it; dropped, the passive loop has it back.
pub struct MicHold(Option<oneshot::Sender<()>>);

impl MicHold {
    /// A hold with no passive loop behind it (a test's).
    pub fn free() -> Self {
        Self(None)
    }
}

impl Drop for MicHold {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// One lease at a time, asked for by dictation and taken by the passive loop.
#[derive(Default)]
pub struct Leases {
    q: Mutex<Option<(u64, Lease)>>,
    asked: std::sync::atomic::AtomicU64,
    notify: Notify,
}

/// Withdraws a lease nobody took when the ask ends (granted, timed out or cancelled), so a
/// later loop never grants a stale one.
struct Withdraw<'a>(&'a Leases, u64);

impl Drop for Withdraw<'_> {
    fn drop(&mut self) {
        let mut q = lock(&self.0.q);
        if q.as_ref().is_some_and(|(id, _)| *id == self.1) {
            q.take();
        }
    }
}

impl Leases {
    /// Ask for the microphone; `None` when the loop did not let go within `wait`. Dropping
    /// the future withdraws the ask; a grant that raced it is given straight back.
    pub async fn ask(&self, wait: Duration) -> Option<MicHold> {
        let (granted_tx, granted_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let id = self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *lock(&self.q) = Some((
            id,
            Lease {
                granted: granted_tx,
                release: release_rx,
            },
        ));
        let _withdraw = Withdraw(self, id);
        self.notify.notify_one();
        let hold = MicHold(Some(release_tx));
        match tokio::time::timeout(wait, granted_rx).await {
            Ok(Ok(())) => Some(hold),
            _ => None,
        }
    }

    /// The next lease asked for: the passive loop waits on this beside the wake.
    pub async fn next(&self) -> Lease {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some((_, l)) = lock(&self.q).take() {
                return l;
            }
            notified.await;
        }
    }
}

// --- what the coordinator stands on ---------------------------------------------------------

/// Who started a dictation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `ghostd dictate`, the control API: ends on the clipboard, refused while not listening.
    Command,
    /// The dictation key, seen through the desktop's shortcut portal: a real key press. It may
    /// open the microphone while not listening, for that dictation only (ADR 0057 as amended
    /// 2026-11-25), and only it types into a window.
    Key,
}

/// The daemon's side, or a test's.
#[async_trait]
pub trait Host: Send + Sync {
    /// Why a dictation cannot start now, if it cannot.
    fn refusal(&self, origin: Origin) -> Option<DictationReason>;
    /// Why a dictation that has been opening its microphone must not record after all:
    /// listening was turned off (a command's), or Ghost is stopping. Asked again just
    /// before recording.
    fn still_allowed(&self, origin: Origin) -> Option<DictationReason>;
    /// The microphone from the passive loop.
    async fn take_mic(&self) -> Result<MicHold, DictationReason>;
    /// Dictation's own capture on the session's microphone.
    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason>;
    /// The shared Parakeet.
    fn recognizer(&self) -> Option<SharedRecognizer>;
    /// A speech detector to cut pieces at pauses, fresh for each dictation. None: one whole
    /// read at the end.
    fn pauses(&self) -> Option<Box<dyn Pauses>> {
        None
    }
}

/// Whether a frame is speech, for cutting pieces at pauses: true from the start of speech
/// until a pause long enough to end it (the detector's own stop time, about half a second).
pub trait Pauses: Send {
    fn speaking(&mut self, frame: &[i16]) -> bool;
}

/// Types into the window that has the keyboard (the RemoteDesktop portal in the daemon).
#[async_trait]
pub trait Typer: Send + Sync {
    /// Type `text`, printable ASCII only ([`typing::plan`] made it). An error is the reason
    /// it couldn't, and the text goes to the clipboard.
    async fn type_text(&self, text: &str) -> Result<(), DictationReason>;
}

/// No typing: the clipboard.
pub struct NoTyper;

#[async_trait]
impl Typer for NoTyper {
    async fn type_text(&self, _text: &str) -> Result<(), DictationReason> {
        Err(DictationReason::TypingFailed)
    }
}

/// The operator's own desktop, for the live view (`DaemonState::publish_dictation`).
pub trait Live: Send + Sync {
    fn show(&self, view: DictationLiveView);
}

pub struct NoLive;

impl Live for NoLive {
    fn show(&self, _view: DictationLiveView) {}
}

/// The default output muted while recording, and its earlier state put back.
pub trait Muter: Send + Sync {
    fn mute(&self);
    fn restore(&self);
}

pub struct NoMute;

impl Muter for NoMute {
    fn mute(&self) {}
    fn restore(&self) {}
}

/// What the coordinator uses beside its host: each has a default that does nothing.
pub struct Parts {
    pub typer: Arc<dyn Typer>,
    pub reviewer: Arc<dyn Reviewer>,
    pub live: Arc<dyn Live>,
    pub muter: Arc<dyn Muter>,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            typer: Arc::new(NoTyper),
            reviewer: Arc::new(NoReviewer),
            live: Arc::new(NoLive),
            muter: Arc::new(NoMute),
        }
    }
}

/// What the operator sees when nobody is at a terminal: Ghost's fixed sentences and counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Note {
    Recording,
    Ended(DictationResult),
}

pub trait Notifier: Send + Sync {
    fn show(&self, note: Note);
}

/// No notifications (a test's, or a desk with no display).
pub struct NoNotes;

impl Notifier for NoNotes {
    fn show(&self, _note: Note) {}
}

/// A desktop notification through `notify-send`, as the desktop route sends them, replaced
/// in place from start to end and transient, so none stays in the list.
#[derive(Default)]
pub struct DesktopNotes {
    id: Arc<Mutex<Option<u32>>>,
}

impl Notifier for DesktopNotes {
    fn show(&self, note: Note) {
        let (title, body, expire) = match note {
            Note::Recording => (
                "Dictating",
                format!(
                    "Speak, then run the shortcut again to stop. Up to {} seconds.",
                    LIMIT.as_secs()
                ),
                LIMIT.as_millis() + 5_000,
            ),
            Note::Ended(r) => ("Dictation", r.sentence(), 4_000),
        };
        let id = self.id.clone();
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            let mut cmd = tokio::process::Command::new(crate::routes::NOTIFY_SEND);
            cmd.args([
                "--app-name=Ghost",
                "--urgency=low",
                "--transient",
                "--icon=audio-input-microphone",
                "--print-id",
            ])
            .arg(format!("--expire-time={expire}"));
            if let Some(n) = *lock(&id) {
                cmd.arg(format!("--replace-id={n}"));
            }
            let out = cmd
                .arg(title)
                .arg(body)
                .env_clear()
                .envs(crate::passwords::child_env()) // never a password (ADR 0036)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await;
            if let Ok(out) = out
                && let Ok(n) = String::from_utf8_lossy(&out.stdout).trim().parse::<u32>()
            {
                *lock(&id) = Some(n);
            }
        });
    }
}

// --- the coordinator ----------------------------------------------------------------------

/// How a recording ended.
#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
    Stopped,
    Limit,
    MicrophoneEnded,
}

struct Captured {
    samples: Zeroizing<Vec<i16>>,
    end: End,
    /// The pieces read at pauses while recording, in order; each is the cleaned text.
    pieces: Vec<tokio::task::JoinHandle<Option<Zeroizing<String>>>>,
    /// Where the last piece ended: the rest is read at the end.
    cut: usize,
}

/// What a start holds once the first frame is in. Fields drop in this order: the capture,
/// then the lease (the passive loop is told only after the capture is gone), then the first
/// frame, wiped.
struct Opened {
    media: Box<dyn MediaIn>,
    _hold: MicHold,
    first: Zeroizing<Vec<i16>>,
}

/// Close what a start or a recording held, off the runtime: dropping a capture joins its
/// thread, which may take a while. The live-capture registry, not this, is what says when
/// it is gone.
fn close_off(mut o: Opened) {
    o.media.close();
    drop(tokio::task::spawn_blocking(move || drop(o)));
}

/// A start between the request and the first frame.
struct Starting {
    generation: u64,
    cancel: CancellationToken,
}

struct Recording {
    generation: u64,
    origin: Origin,
    since: Instant,
    first_frame_ms: u64,
    stop: CancellationToken,
    /// What was recorded, sent by the recording task as it stops; dropped unread (and wiped)
    /// when the dictation is cancelled.
    captured: oneshot::Receiver<Captured>,
}

enum Stage {
    Idle,
    Starting(Starting),
    Recording(Recording),
    Reading {
        generation: u64,
    },
    /// Read and committed; the clipboard is being set. A cancel waits for it (bounded by the
    /// clipboard's own bounds): it is too late to keep the text off the clipboard.
    Writing {
        generation: u64,
    },
}

impl Stage {
    fn generation(&self) -> Option<u64> {
        match self {
            Stage::Idle => None,
            Stage::Starting(s) => Some(s.generation),
            Stage::Recording(r) => Some(r.generation),
            Stage::Reading { generation } | Stage::Writing { generation } => Some(*generation),
        }
    }
}

struct Inner {
    generation: u64,
    /// Who started the current (or last) generation: what the end shows and says.
    origin: Origin,
    phase: Stage,
    last: Option<DictationResult>,
}

/// Why a start did not reach its first frame.
enum Unstarted {
    Refused(DictationReason),
    Cancelled,
}

/// The longest a cancel waits for a clipboard write already under way.
const WRITING_WAIT: Duration = Duration::from_secs(6);

/// One operation's settling, whatever way it ends: dropped while armed (an early return
/// that forgot, or a panic unwinding), it puts the stage back to idle if the stage is still
/// this operation's, with the result `Cancelled (Failed)`.
struct Settle {
    me: Weak<Dictation>,
    generation: u64,
    armed: bool,
}

impl Settle {
    fn new(me: &Weak<Dictation>, generation: u64) -> Self {
        Self {
            me: me.clone(),
            generation,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Settle {
    fn drop(&mut self) {
        if self.armed
            && let Some(d) = self.me.upgrade()
        {
            d.failed(self.generation);
        }
    }
}

pub struct Dictation {
    host: Arc<dyn Host>,
    clipboard: Arc<dyn Clipboard>,
    notes: Arc<dyn Notifier>,
    parts: Parts,
    limit: Duration,
    /// Each change of stage, one at a time. Never held across a capture's open or close, a
    /// read, or the clipboard.
    ops: tokio::sync::Mutex<()>,
    inner: Mutex<Inner>,
    /// Each dictation's result as it is decided, by generation.
    done: watch::Sender<Option<(u64, DictationResult)>>,
    me: Weak<Dictation>,
}

impl Dictation {
    pub fn new(
        host: Arc<dyn Host>,
        clipboard: Arc<dyn Clipboard>,
        notes: Arc<dyn Notifier>,
        limit: Duration,
    ) -> Arc<Self> {
        Self::with_parts(host, clipboard, notes, Parts::default(), limit)
    }

    pub fn with_parts(
        host: Arc<dyn Host>,
        clipboard: Arc<dyn Clipboard>,
        notes: Arc<dyn Notifier>,
        parts: Parts,
        limit: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            host,
            clipboard,
            notes,
            parts,
            limit,
            ops: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                generation: 0,
                origin: Origin::Command,
                phase: Stage::Idle,
                last: None,
            }),
            done: watch::channel(None).0,
            me: me.clone(),
        })
    }

    fn state_of(stage: &Stage) -> DictationState {
        match stage {
            Stage::Idle => DictationState::Idle,
            Stage::Starting(_) => DictationState::Starting,
            Stage::Recording(_) => DictationState::Recording,
            Stage::Reading { .. } | Stage::Writing { .. } => DictationState::Reading,
        }
    }

    pub fn state(&self) -> DictationState {
        Self::state_of(&lock(&self.inner).phase)
    }

    fn view_with(&self, result: Option<DictationResult>) -> DictationView {
        let inner = lock(&self.inner);
        let (recording_secs, first_frame_ms) = match &inner.phase {
            Stage::Recording(r) => (
                Some(r.since.elapsed().as_secs_f64()),
                Some(r.first_frame_ms),
            ),
            _ => (None, None),
        };
        DictationView {
            state: Self::state_of(&inner.phase),
            limit_secs: self.limit.as_secs_f64(),
            recording_secs,
            first_frame_ms,
            result,
        }
    }

    pub fn view(&self) -> DictationView {
        let last = lock(&self.inner).last;
        self.view_with(last)
    }

    /// Each result as it is decided: the key's loop watches it for a dictation that ended
    /// by itself.
    pub fn decided(&self) -> watch::Receiver<Option<(u64, DictationResult)>> {
        self.done.subscribe()
    }

    /// The result decided for `generation`, if it was.
    fn result_of(&self, generation: u64) -> Option<DictationResult> {
        self.done
            .borrow()
            .filter(|(g, _)| *g == generation)
            .map(|(_, r)| r)
    }

    /// Decide `generation`'s result: remembered, sent to whoever waits, shown.
    fn decide(&self, generation: u64, r: DictationResult) {
        let origin = {
            let mut inner = lock(&self.inner);
            inner.last = Some(r);
            inner.origin
        };
        self.done.send_replace(Some((generation, r)));
        self.ended(origin, r);
    }

    /// How a dictation ended, shown: the live view's last state, and a notification unless
    /// the key's own scene says it (ADR 0077 as amended 2026-11-25: from the key, only what
    /// went wrong and the clipboard's "Press Ctrl+V" are notified; a typed text, nothing
    /// heard and a cancel are the scene's).
    fn ended(&self, origin: Origin, r: DictationResult) {
        use DictationOutcome as O;
        let quiet = origin == Origin::Key
            && match r.outcome {
                O::Typed | O::NothingHeard => true,
                O::Cancelled => r.reason != Some(DictationReason::Failed),
                O::OnClipboard | O::NotOnClipboard | O::Refused => false,
            };
        if !quiet {
            self.notes.show(Note::Ended(r));
        }
        let scene = match r.outcome {
            O::Typed => DictationScene::Typed,
            O::OnClipboard => DictationScene::Clipboard,
            O::NothingHeard => DictationScene::NothingHeard,
            O::NotOnClipboard | O::Cancelled | O::Refused => DictationScene::Stopped,
        };
        self.parts.live.show(DictationLiveView {
            scene,
            key: origin == Origin::Key,
            level: None,
            text: None,
            seconds: r.seconds,
            limit_secs: self.limit.as_secs_f64(),
            sentence: Some(r.sentence()),
        });
    }

    /// The live view while recording or writing.
    fn live(
        &self,
        origin: Origin,
        scene: DictationScene,
        level: Option<f32>,
        text: Option<String>,
        seconds: f64,
    ) {
        self.parts.live.show(DictationLiveView {
            scene,
            key: origin == Origin::Key,
            level,
            text,
            seconds,
            limit_secs: self.limit.as_secs_f64(),
            sentence: None,
        });
    }

    /// An operation of `generation` failed (a panic, or an exit that did not settle): the
    /// stage goes back to idle if it is still that operation's.
    fn failed(&self, generation: u64) {
        let ours = {
            let mut inner = lock(&self.inner);
            let ours = inner.phase.generation() == Some(generation);
            if ours {
                inner.phase = Stage::Idle;
            }
            ours
        };
        if ours {
            warn!("dictation: an operation failed; nothing was kept");
            self.decide(
                generation,
                DictationResult::cancelled(DictationReason::Failed),
            );
        }
    }

    /// A refusal: said and answered.
    fn refuse(&self, origin: Origin, reason: DictationReason) -> DictationView {
        let r = DictationResult::refused(reason);
        info!("dictation: refused ({reason:?})");
        self.ended(origin, r);
        self.view_with(Some(r))
    }

    /// Open the microphone and record, from a command. The dictation is the coordinator's
    /// from here: the start runs as its own task, so dropping this request (a client gone)
    /// changes nothing.
    pub async fn start(&self) -> DictationView {
        self.start_from(Origin::Command).await
    }

    /// Open the microphone and record, from the dictation key.
    pub async fn start_key(&self) -> DictationView {
        self.start_from(Origin::Key).await
    }

    async fn start_from(&self, origin: Origin) -> DictationView {
        let (generation, cancel) = {
            let _op = self.ops.lock().await;
            match self.state() {
                DictationState::Idle => {}
                DictationState::Starting | DictationState::Recording => {
                    return self.refuse(origin, DictationReason::AlreadyDictating);
                }
                DictationState::Reading => {
                    return self.refuse(origin, DictationReason::StillReading);
                }
            }
            if let Some(r) = self.host.refusal(origin) {
                return self.refuse(origin, r);
            }
            if self.host.recognizer().is_none() {
                return self.refuse(origin, DictationReason::NoSpeechModel);
            }
            let cancel = CancellationToken::new();
            let mut inner = lock(&self.inner);
            inner.generation += 1;
            inner.origin = origin;
            inner.phase = Stage::Starting(Starting {
                generation: inner.generation,
                cancel: cancel.clone(),
            });
            (inner.generation, cancel)
        };
        self.live(origin, DictationScene::Starting, None, None, 0.0);
        let Some(me) = self.me.upgrade() else {
            return self.view();
        };
        match tokio::spawn(me.run_start(generation, cancel, origin)).await {
            Ok(v) => v,
            Err(_) => self.view_with(self.result_of(generation)),
        }
    }

    async fn run_start(
        self: Arc<Self>,
        generation: u64,
        cancel: CancellationToken,
        origin: Origin,
    ) -> DictationView {
        let mut settle = Settle::new(&self.me, generation);
        let t0 = Instant::now();
        let acquired = self.acquire(&cancel, t0).await;
        let _op = self.ops.lock().await;
        let current = matches!(&lock(&self.inner).phase,
            Stage::Starting(s) if s.generation == generation)
            && !cancel.is_cancelled();
        let (opened, leased, open_ms) = match acquired {
            Ok(a) if current => a,
            Ok((o, _, _)) => {
                close_off(o);
                settle.disarm();
                return self.view_with(self.result_of(generation));
            }
            Err(why) => {
                settle.disarm();
                return self.unstarted(generation, why);
            }
        };
        // Asked again just before recording: listening may have gone off meanwhile.
        if let Some(why) = self.host.still_allowed(origin) {
            close_off(opened);
            settle.disarm();
            return self.unstarted(generation, Unstarted::Refused(why));
        }
        let first_frame_ms = t0.elapsed().as_millis() as u64;
        info!(
            "dictation: recording; first frame {first_frame_ms} ms after the start (the \
             microphone handed over in {} ms, the capture open in {} ms)",
            leased.as_millis(),
            open_ms.as_millis()
        );
        let stop = CancellationToken::new();
        let (captured_tx, captured_rx) = oneshot::channel();
        // Muted for the recording only: `record` puts it back as it stops, however it stops.
        self.parts.muter.mute();
        tokio::spawn(record(
            opened,
            Recorder {
                stop: stop.clone(),
                limit: self.limit,
                me: self.me.clone(),
                generation,
                origin,
                pauses: self.host.pauses(),
                model: self.host.recognizer(),
            },
            captured_tx,
        ));
        lock(&self.inner).phase = Stage::Recording(Recording {
            generation,
            origin,
            since: t0,
            first_frame_ms,
            stop,
            captured: captured_rx,
        });
        settle.disarm();
        if origin == Origin::Command {
            self.notes.show(Note::Recording); // the key's scene says it
        }
        self.live(origin, DictationScene::Listening, Some(0.0), None, 0.0);
        self.view_with(None)
    }

    /// The microphone from the passive loop, the capture, and its first frame. A cancel
    /// interrupts the wait for the loop and for the first frame; an open in flight is not
    /// interrupted, and what it opens is closed as soon as it returns. Whatever this gives up
    /// is closed off the runtime (`close_off`); the registry says when it is gone.
    async fn acquire(
        &self,
        cancel: &CancellationToken,
        t0: Instant,
    ) -> Result<(Opened, Duration, Duration), Unstarted> {
        let hold = tokio::select! {
            h = self.host.take_mic() => h.map_err(Unstarted::Refused)?,
            _ = cancel.cancelled() => return Err(Unstarted::Cancelled),
        };
        let leased = t0.elapsed();
        let media = self.host.open().await.map_err(Unstarted::Refused)?;
        let open_ms = t0.elapsed() - leased;
        let mut opened = Opened {
            media,
            _hold: hold,
            first: Zeroizing::new(Vec::new()),
        };
        if cancel.is_cancelled() {
            close_off(opened);
            return Err(Unstarted::Cancelled);
        }
        let first = tokio::select! {
            f = tokio::time::timeout(FIRST_FRAME_WAIT, opened.media.next_frame()) => f,
            _ = cancel.cancelled() => {
                close_off(opened);
                return Err(Unstarted::Cancelled);
            }
        };
        match first {
            Ok(Some(f)) => {
                opened.first = Zeroizing::new(f.samples);
                Ok((opened, leased, open_ms))
            }
            _ => {
                close_off(opened);
                Err(Unstarted::Refused(DictationReason::MicrophoneFailed))
            }
        }
    }

    /// A start that did not record (the caller holds `ops`): the answer is the refusal, or
    /// the cancel's own result.
    fn unstarted(&self, generation: u64, why: Unstarted) -> DictationView {
        let ours = {
            let mut inner = lock(&self.inner);
            let ours = matches!(&inner.phase, Stage::Starting(s) if s.generation == generation);
            if ours {
                inner.phase = Stage::Idle;
            }
            ours
        };
        match why {
            Unstarted::Refused(r) if ours => {
                info!("dictation: refused ({r:?})");
                self.decide(generation, DictationResult::refused(r));
                self.view_with(Some(DictationResult::refused(r)))
            }
            _ => self.view_with(self.result_of(generation)),
        }
    }

    /// Stop recording and read; the answer comes once the text is on the clipboard. Runs as
    /// the coordinator's own task, like a start. A stop while the microphone is still opening
    /// cancels the start.
    pub async fn stop(&self) -> DictationView {
        let Some(me) = self.me.upgrade() else {
            return self.view();
        };
        match tokio::spawn(me.finish(None)).await {
            Ok(v) => v,
            Err(_) => self.view(),
        }
    }

    /// Start when idle, stop when starting or recording, refuse while reading.
    pub async fn toggle(&self) -> DictationView {
        match self.state() {
            DictationState::Idle => self.start().await,
            DictationState::Starting | DictationState::Recording => self.stop().await,
            DictationState::Reading => self.refuse(Origin::Command, DictationReason::StillReading),
        }
    }

    async fn finish(self: Arc<Self>, only: Option<u64>) -> DictationView {
        enum Next {
            NotDictating,
            Unchanged,
            CancelStart,
            Wait(u64),
            Read(Recording),
        }
        let mut done = self.done.subscribe();
        let next = {
            let _op = self.ops.lock().await;
            let mut inner = lock(&self.inner);
            match std::mem::replace(&mut inner.phase, Stage::Idle) {
                Stage::Idle if only.is_some() => Next::Unchanged,
                Stage::Idle => Next::NotDictating,
                Stage::Starting(s) => {
                    inner.phase = Stage::Starting(s);
                    if only.is_some() {
                        Next::Unchanged
                    } else {
                        Next::CancelStart
                    }
                }
                stage @ (Stage::Reading { .. } | Stage::Writing { .. }) => {
                    let g = stage.generation().unwrap_or(0);
                    inner.phase = stage;
                    Next::Wait(g)
                }
                Stage::Recording(r) if only.is_some_and(|g| g != r.generation) => {
                    inner.phase = Stage::Recording(r);
                    Next::Unchanged
                }
                Stage::Recording(r) => {
                    inner.phase = Stage::Reading {
                        generation: r.generation,
                    };
                    Next::Read(r)
                }
            }
        };
        let rec = match next {
            Next::NotDictating => {
                return self.view_with(Some(DictationResult::refused(
                    DictationReason::NotDictating,
                )));
            }
            Next::Unchanged => return self.view(),
            Next::CancelStart => return self.cancel(DictationReason::Operator).await,
            Next::Wait(generation) => {
                let r = done
                    .wait_for(|d| d.is_some_and(|(g, _)| g >= generation))
                    .await
                    .ok()
                    .and_then(|d| *d)
                    .map(|(_, r)| r);
                return self.view_with(r);
            }
            Next::Read(rec) => rec,
        };
        let generation = rec.generation;
        let origin = rec.origin;
        let mut settle = Settle::new(&self.me, generation);
        rec.stop.cancel();
        // What was recorded: the capture closes on its own (the registry tracks it).
        let captured = rec.captured.await.ok();
        let failed = captured.is_none();
        let captured = captured.unwrap_or(Captured {
            samples: Zeroizing::new(Vec::new()),
            end: End::Stopped,
            pieces: Vec::new(),
            cut: 0,
        });
        let seconds = captured.samples.len() as f64 / f64::from(RATE);
        self.live(origin, DictationScene::Writing, None, None, seconds);
        let mut result = DictationResult {
            seconds,
            first_frame_ms: Some(rec.first_frame_ms),
            ..DictationResult::of(DictationOutcome::NothingHeard, None)
        };
        result.reason = match captured.end {
            End::Limit => Some(DictationReason::Limit),
            End::MicrophoneEnded => Some(DictationReason::MicrophoneEnded),
            End::Stopped => None,
        };
        let mut read_failed = failed;
        // The rest since the last piece, read now; then the pieces read while recording, in
        // order, and the rest after them.
        let Captured {
            samples,
            pieces,
            cut,
            ..
        } = captured;
        let tail = match self.host.recognizer() {
            Some(model) => {
                let read = tokio::task::spawn_blocking(move || {
                    let rest = &samples[cut.min(samples.len())..];
                    let out = if !rest.is_empty() {
                        let (text, queue, took) = read(&model, rest);
                        (cleanup::clean(&text), queue, took)
                    } else {
                        (
                            Zeroizing::new(String::new()),
                            Duration::ZERO,
                            Duration::ZERO,
                        )
                    };
                    drop(samples); // wiped here, off the runtime
                    out
                })
                .await;
                match read {
                    Ok((text, queue, took)) => {
                        result.queue_ms = Some(queue.as_millis() as u64);
                        result.read_ms = Some(took.as_millis() as u64);
                        text
                    }
                    Err(_) => {
                        read_failed = true;
                        Zeroizing::new(String::new())
                    }
                }
            }
            None => Zeroizing::new(String::new()),
        };
        let mut parts: Vec<Zeroizing<String>> = Vec::with_capacity(pieces.len() + 1);
        for p in pieces {
            match p.await {
                Ok(Some(t)) if !t.is_empty() => parts.push(t),
                Ok(_) => {}
                Err(_) => read_failed = true,
            }
        }
        if !tail.is_empty() {
            parts.push(tail);
        }
        let text = joined(&parts);
        drop(parts);
        if read_failed {
            drop(text);
            settle.disarm();
            self.failed(generation);
            return self.view_with(self.result_of(generation));
        }
        // From the key, the model's tidy, by the operator's switch: still in Reading, so a
        // cancel meanwhile drops it all.
        let text = if origin == Origin::Key && !text.is_empty() {
            self.tidy(text).await
        } else {
            text
        };
        // The commit: only while this dictation is still the one being read. `ops` is not
        // held while the text is typed or the clipboard set; the stage says Writing meanwhile.
        {
            let _op = self.ops.lock().await;
            let mut inner = lock(&self.inner);
            if !matches!(inner.phase, Stage::Reading { generation: g } if g == generation) {
                drop(inner);
                drop(text); // wiped; a cancelled read does nothing
                settle.disarm();
                return self.view_with(self.result_of(generation));
            }
            inner.phase = Stage::Writing { generation };
        }
        if !text.is_empty() {
            result.chars = text.chars().count();
            let (outcome, why) = self.deliver(origin, text).await;
            result.outcome = outcome;
            if why.is_some() {
                result.reason = why;
            }
        }
        info!(
            "dictation: {:?}{}; {:.1} s recorded, the rest read in {} ms after {} ms in the \
             queue, {} characters",
            result.outcome,
            result
                .reason
                .map(|r| format!(" ({r:?})"))
                .unwrap_or_default(),
            result.seconds,
            result.read_ms.unwrap_or(0),
            result.queue_ms.unwrap_or(0),
            result.chars
        );
        {
            let mut inner = lock(&self.inner);
            if matches!(inner.phase, Stage::Writing { generation: g } if g == generation) {
                inner.phase = Stage::Idle;
            }
        }
        settle.disarm();
        self.decide(generation, result);
        self.view_with(Some(result))
    }

    /// The model's tidy, within [`review::WAIT`], taken only when [`review::accept`] says so.
    async fn tidy(&self, text: Zeroizing<String>) -> Zeroizing<String> {
        let asked = tokio::time::timeout(review::WAIT, self.parts.reviewer.review(&text)).await;
        match asked {
            Ok(Some(tidied)) if review::accept(&text, &tidied) => {
                info!("dictation: tidied by the model");
                cleanup::sanitize(&tidied)
            }
            Ok(Some(_)) => {
                info!("dictation: the model changed too much; typing what was read");
                text
            }
            Ok(None) => text,
            Err(_) => {
                info!("dictation: the model took too long; typing what was read");
                text
            }
        }
    }

    /// Where the text goes: typed from the key when every character has a key and the typing
    /// works, the clipboard otherwise, and always from a command.
    async fn deliver(
        &self,
        origin: Origin,
        text: Zeroizing<String>,
    ) -> (DictationOutcome, Option<DictationReason>) {
        if origin == Origin::Key {
            match typing::plan(&text) {
                typing::Plan::Type(t) => match self.parts.typer.type_text(&t).await {
                    Ok(()) => return (DictationOutcome::Typed, None),
                    Err(why) => {
                        info!("dictation: couldn't type it ({why:?}); the clipboard instead");
                        return self
                            .to_clipboard(text, Some(DictationReason::TypingFailed))
                            .await;
                    }
                },
                typing::Plan::Clipboard => {
                    return self.to_clipboard(text, Some(DictationReason::NoKeys)).await;
                }
            }
        }
        self.to_clipboard(text, None).await
    }

    async fn to_clipboard(
        &self,
        text: Zeroizing<String>,
        why: Option<DictationReason>,
    ) -> (DictationOutcome, Option<DictationReason>) {
        let clipboard = self.clipboard.clone();
        match tokio::task::spawn_blocking(move || clipboard.set(text)).await {
            Ok(Ok(())) => (DictationOutcome::OnClipboard, why),
            Ok(Err(e)) => (DictationOutcome::NotOnClipboard, Some(e)),
            Err(_) => (
                DictationOutcome::NotOnClipboard,
                Some(DictationReason::ClipboardFailed),
            ),
        }
    }

    /// Drop the dictation, starting, recording or being read: nothing reaches the
    /// clipboard. Returns at once: what it had open closes on its own, and the passive loop
    /// waits for that through the live-capture registry. A dictation already writing to the
    /// clipboard is too late to stop: the cancel waits for it (bounded) and says how it ended.
    pub async fn cancel(&self, why: DictationReason) -> DictationView {
        let mut done = self.done.subscribe();
        let writing = {
            let _op = self.ops.lock().await;
            let mut inner = lock(&self.inner);
            match std::mem::replace(&mut inner.phase, Stage::Idle) {
                Stage::Idle => {
                    drop(inner);
                    return self.view_with(Some(DictationResult::refused(
                        DictationReason::NotDictating,
                    )));
                }
                Stage::Writing { generation } => {
                    inner.phase = Stage::Writing { generation };
                    Some(generation)
                }
                stage => {
                    let generation = stage.generation().unwrap_or(0);
                    match stage {
                        Stage::Starting(s) => s.cancel.cancel(),
                        Stage::Recording(r) => r.stop.cancel(), // its samples are dropped unread
                        _ => {}
                    }
                    drop(inner);
                    let r = DictationResult::cancelled(why);
                    info!("dictation: cancelled ({why:?})");
                    self.decide(generation, r);
                    None
                }
            }
        };
        match writing {
            Some(generation) => {
                let r = tokio::time::timeout(
                    WRITING_WAIT,
                    done.wait_for(|d| d.is_some_and(|(g, _)| g >= generation)),
                )
                .await
                .ok()
                .and_then(|d| d.ok().and_then(|d| *d))
                .map(|(_, r)| r);
                self.view_with(r)
            }
            None => {
                let last = lock(&self.inner).last;
                self.view_with(last)
            }
        }
    }

    /// Cancel if anything is going on, a start included; nothing to say otherwise.
    pub async fn end(&self, why: DictationReason) {
        if self.state() != DictationState::Idle {
            self.cancel(why).await;
        }
    }
}

/// What a recording runs with.
struct Recorder {
    stop: CancellationToken,
    limit: Duration,
    me: Weak<Dictation>,
    generation: u64,
    origin: Origin,
    pauses: Option<Box<dyn Pauses>>,
    model: Option<SharedRecognizer>,
}

/// The words read so far, a slot per piece in order, for the live view. Wiped when dropped.
#[derive(Default)]
struct Heard {
    slots: Vec<Option<Zeroizing<String>>>,
}

impl Heard {
    fn text(&self) -> Option<String> {
        let parts: Vec<&str> = self
            .slots
            .iter()
            .flatten()
            .map(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Puts the output's sound back when the recording ends, however it ends.
struct Unmute(Weak<Dictation>);

impl Drop for Unmute {
    fn drop(&mut self) {
        if let Some(d) = self.0.upgrade() {
            d.parts.muter.restore();
        }
    }
}

/// The pieces' texts joined, sanitized; wiped when dropped.
fn joined(parts: &[Zeroizing<String>]) -> Zeroizing<String> {
    let len = parts.iter().map(|p| p.len() + 1).sum();
    let mut all = Zeroizing::new(String::with_capacity(len));
    for p in parts {
        if !all.is_empty() {
            all.push(' ');
        }
        all.push_str(p);
    }
    cleanup::sanitize(&all)
}

/// A frame's level, 0.0 to 1.0: its RMS in dBFS from -50 (0.0) to 0 (1.0).
fn level_of(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / frame.len() as f64).sqrt() / 32768.0;
    if rms <= 0.0 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 50.0) / 50.0).clamp(0.0, 1.0) as f32
}

impl Dictation {
    /// Whether `generation` is the one recording now.
    fn recording(&self, generation: u64) -> bool {
        matches!(&lock(&self.inner).phase, Stage::Recording(r) if r.generation == generation)
    }
}

/// Read one piece on the shared Parakeet while the recording goes on; its cleaned text goes
/// into its slot and the live view.
#[allow(clippy::too_many_arguments)]
fn read_piece(
    model: SharedRecognizer,
    piece: Zeroizing<Vec<i16>>,
    index: usize,
    heard: Arc<Mutex<Heard>>,
    me: Weak<Dictation>,
    generation: u64,
    origin: Origin,
    since: Instant,
) -> tokio::task::JoinHandle<Option<Zeroizing<String>>> {
    tokio::spawn(async move {
        let text = tokio::task::spawn_blocking(move || {
            let (text, _, _) = read(&model, &piece);
            drop(piece); // wiped here, off the runtime
            cleanup::clean(&text)
        })
        .await
        .ok()?;
        let now = {
            let mut h = lock(&heard);
            if h.slots.len() <= index {
                h.slots.resize_with(index + 1, || None);
            }
            h.slots[index] = Some(text.clone());
            h.text()
        };
        if let Some(d) = me.upgrade()
            && d.recording(generation)
        {
            d.live(
                origin,
                DictationScene::Listening,
                None,
                now,
                since.elapsed().as_secs_f64(),
            );
        }
        Some(text)
    })
}

/// Record until stopped, the limit (in samples, or in time for a capture that stalls), or
/// the microphone going away; cut a piece at each pause after speech and read it while
/// recording goes on; send the level to the live view; send what was recorded, then close
/// the capture and give the microphone back, and if it ended by itself, finish the
/// dictation. The buffer is the whole limit from the start, so it never grows and leaves no
/// copy behind; it is wiped when dropped. A panic here settles the stage (`Settle`), and
/// unwinding drops the capture before the lease; the output's sound is put back either way.
async fn record(mut opened: Opened, r: Recorder, captured: oneshot::Sender<Captured>) {
    let Recorder {
        stop,
        limit,
        me,
        generation,
        origin,
        mut pauses,
        model,
    } = r;
    let mut settle = Settle::new(&me, generation);
    let _unmute = Unmute(me.clone());
    let max = (limit.as_secs_f64() * f64::from(RATE)) as usize;
    let piece_min = (PIECE_MIN.as_secs_f64() * f64::from(RATE)) as usize;
    let deadline = tokio::time::Instant::now() + limit;
    let since = Instant::now();
    let mut samples: Zeroizing<Vec<i16>> = Zeroizing::new(Vec::with_capacity(max));
    let take = |frame: &[i16], samples: &mut Vec<i16>| {
        let room = max - samples.len();
        samples.extend_from_slice(&frame[..frame.len().min(room)]);
    };
    take(&opened.first, &mut samples);
    opened.first = Zeroizing::new(Vec::new()); // the old one is wiped as it drops
    let heard = Arc::new(Mutex::new(Heard::default()));
    let mut pieces = Vec::new();
    let mut cut = 0usize;
    let mut speech = false;
    let mut shown = Instant::now();
    let mut end = End::Stopped;
    loop {
        if samples.len() >= max {
            end = End::Limit;
            break;
        }
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = tokio::time::sleep_until(deadline) => {
                end = End::Limit;
                break;
            }
            f = opened.media.next_frame() => match f {
                Some(f) => {
                    let f = Zeroizing::new(f.samples);
                    take(&f, &mut samples);
                    if let Some(p) = pauses.as_mut() {
                        if p.speaking(&f) {
                            speech = true;
                        } else if speech && samples.len() - cut >= piece_min {
                            // A pause after speech: this piece is read now.
                            speech = false;
                            if let Some(m) = &model {
                                let piece = Zeroizing::new(samples[cut..].to_vec());
                                pieces.push(read_piece(
                                    m.clone(),
                                    piece,
                                    pieces.len(),
                                    heard.clone(),
                                    me.clone(),
                                    generation,
                                    origin,
                                    since,
                                ));
                                cut = samples.len();
                            }
                        }
                    }
                    if shown.elapsed() >= LEVEL_EVERY {
                        shown = Instant::now();
                        if let Some(d) = me.upgrade() {
                            let text = lock(&heard).text();
                            d.live(
                                origin,
                                DictationScene::Listening,
                                Some(level_of(&f)),
                                text,
                                since.elapsed().as_secs_f64(),
                            );
                        }
                    }
                }
                None => {
                    end = End::MicrophoneEnded;
                    break;
                }
            },
        }
    }
    // A cancelled dictation dropped the receiver: the samples go, wiped, and the pieces
    // still being read are dropped unread.
    let _ = captured.send(Captured {
        samples,
        end,
        pieces,
        cut,
    });
    close_off(opened);
    settle.disarm();
    if end != End::Stopped {
        if end == End::MicrophoneEnded {
            warn!("dictation: the microphone went away; reading what was said");
        }
        if let Some(d) = me.upgrade() {
            tokio::spawn(d.finish(Some(generation)));
        }
    }
}

/// One whole read on the shared Parakeet, as `ghost_voice::stt::recognize` makes it (the
/// trailing silence a segment gets, the dither, the trim), with the wait for the model and
/// the read timed apart. Every buffer here is sized once and wiped when dropped: the f32
/// copy, the dithered copy, and what Parakeet returned. Parakeet's own buffers inside the
/// model are not ours to wipe. Its error is a kind, never words, and is not logged here.
fn read(model: &SharedRecognizer, samples: &[i16]) -> (Zeroizing<String>, Duration, Duration) {
    let trailing = (TRAILING_SECS * f64::from(RATE)) as usize;
    let mut audio: Zeroizing<Vec<f32>> =
        Zeroizing::new(Vec::with_capacity(samples.len() + trailing));
    audio.extend(samples.iter().map(|&s| f32::from(s) / 32768.0));
    audio.extend(std::iter::repeat_n(0.0f32, trailing));
    let asked = Instant::now();
    let mut guard = model.lock().unwrap_or_else(|e| e.into_inner());
    let queue = asked.elapsed();
    let began = Instant::now();
    let audio = Zeroizing::new(dithered(&audio));
    let text = match guard.transcribe(&audio) {
        Ok(t) => {
            let t = Zeroizing::new(t);
            let trimmed = t.trim();
            let mut out = Zeroizing::new(String::with_capacity(trimmed.len()));
            out.push_str(trimmed);
            out
        }
        Err(_) => {
            warn!("dictation: Parakeet failed to read it");
            Zeroizing::new(String::new())
        }
    };
    (text, queue, began.elapsed())
}

/// The control API's port.
pub struct DictationDesk(pub Arc<Dictation>);

#[async_trait]
impl DictationPort for DictationDesk {
    fn view(&self) -> DictationView {
        self.0.view()
    }
    async fn start(&self) -> DictationView {
        self.0.start().await
    }
    async fn stop(&self) -> DictationView {
        self.0.stop().await
    }
    async fn toggle(&self) -> DictationView {
        self.0.toggle().await
    }
    async fn cancel(&self) -> DictationView {
        self.0.cancel(DictationReason::Operator).await
    }
}

// --- the command line ------------------------------------------------------------------------

/// What `ghostd dictate` prints for an answer, and its exit code: 0 recording or done, 1 a
/// refusal or nothing written down. Ghost's own sentences and numbers, never words.
pub fn describe(v: &DictationView) -> (String, i32) {
    match (v.state, v.result) {
        (DictationState::Recording, None) => (
            format!(
                "Dictating: speak, then `ghostd dictate stop` (or the shortcut again). Up to {:.0} \
                 seconds. The microphone was ready {} ms after the request.",
                v.limit_secs,
                v.first_frame_ms.unwrap_or(0)
            ),
            0,
        ),
        (DictationState::Reading, None) => ("Writing it down…".into(), 0),
        (DictationState::Starting, None) => ("Opening the microphone…".into(), 0),
        (DictationState::Idle, None) => ("Not dictating.".into(), 0),
        (_, Some(r)) => {
            let mut line = r.sentence();
            if r.read_ms.is_some() {
                line.push_str(&format!(
                    " ({:.1} s recorded; read in {} ms after {} ms waiting for the speech model; \
                     the microphone was ready {} ms after the start)",
                    r.seconds,
                    r.read_ms.unwrap_or(0),
                    r.queue_ms.unwrap_or(0),
                    r.first_frame_ms.unwrap_or(0)
                ));
            }
            let ok = matches!(r.outcome, DictationOutcome::OnClipboard)
                || (v.state == DictationState::Recording);
            (line, if ok { 0 } else { 1 })
        }
    }
}

/// What `ghostd dictate status` prints: where dictation is now, then, when nothing is going
/// on, how the last one ended. `GET /dictation` carries the last result beside a dictation
/// that is recording or being read; that result is not this one's, so it is left out then
/// (#177). Exit 0 whenever the daemon answered.
pub fn describe_status(v: &DictationView) -> (String, i32) {
    let line = match (v.state, v.result) {
        (DictationState::Idle, Some(r)) => format!("Not dictating. Last time: {}", r.sentence()),
        _ => describe(&DictationView { result: None, ..*v }).0,
    };
    (line, 0)
}

/// `ghostd dictate start|stop|toggle|cancel|status`.
pub async fn cli(
    args: &[String],
    client: &ghost_api::GhostClient,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
) -> i32 {
    let usage = "ghostd dictate start|stop|toggle|cancel|status";
    let status = matches!(args.first().map(String::as_str), None | Some("status"));
    let reply = match args.first().map(String::as_str) {
        None | Some("status") => client.dictation(None).await,
        Some(verb @ ("start" | "stop" | "toggle" | "cancel")) => client.dictation(Some(verb)).await,
        Some(other) => {
            let _ = writeln!(err, "unknown: ghostd dictate {other}; {usage}");
            return 2;
        }
    };
    let view = match reply.map(serde_json::from_value::<DictationView>) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = writeln!(err, "ghostd dictate: an answer this build cannot read: {e}");
            return 1;
        }
        Err(e) => {
            let _ = writeln!(err, "ghostd dictate: {e}");
            return 1;
        }
    };
    let (line, code) = if status {
        describe_status(&view)
    } else {
        describe(&view)
    };
    let _ = writeln!(out, "{line}");
    code
}

// --- the daemon's side ----------------------------------------------------------------------

/// What dictation stands on in `ghostd`.
pub struct DaemonHost {
    pub d: Weak<Daemon>,
}

#[async_trait]
impl Host for DaemonHost {
    fn refusal(&self, origin: Origin) -> Option<DictationReason> {
        let Some(d) = self.d.upgrade() else {
            return Some(DictationReason::Stopping);
        };
        if d.stop.is_cancelled() {
            return Some(DictationReason::Stopping);
        }
        // ADR 0057 as amended 2026-11-25: only a real key press opens the microphone while
        // not listening, for that dictation; a command is refused.
        if d.mic.is_off() && origin == Origin::Command {
            return Some(DictationReason::NotListening);
        }
        if d.state.phase() == Phase::Recording {
            return Some(DictationReason::HearingTest);
        }
        if d.session().is_some()
            || d.state.phase() != Phase::Passive
            || !d.waiting_flag.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Some(DictationReason::Busy);
        }
        if d.models().is_none() {
            return Some(DictationReason::NoSpeechModel);
        }
        if (d.ports.sound_facts)().is_some_and(|f| !f.nodes.iter().any(|n| n.is_source())) {
            return Some(DictationReason::NoMicrophone);
        }
        None
    }

    fn still_allowed(&self, origin: Origin) -> Option<DictationReason> {
        let Some(d) = self.d.upgrade() else {
            return Some(DictationReason::Stopping);
        };
        if d.stop.is_cancelled() {
            Some(DictationReason::Stopping)
        } else if d.mic.is_off() && origin == Origin::Command {
            Some(DictationReason::NotListening)
        } else {
            None
        }
    }

    async fn take_mic(&self) -> Result<MicHold, DictationReason> {
        let d = self.d.upgrade().ok_or(DictationReason::Stopping)?;
        d.dictation_mic
            .ask(LEASE_WAIT)
            .await
            .ok_or(DictationReason::Busy)
    }

    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason> {
        let d = self.d.upgrade().ok_or(DictationReason::Stopping)?;
        // Counted from before the open starts until after the capture is gone (ADR 0077):
        // the passive loop listens again only when this token is released.
        let token = d.captures.take(ghost_audio::live::Holder::Dictation);
        let port = d.ports.dictation_media.clone();
        // What a session hears (ADR 0064): the plan's session input.
        let spec = d.mic_plan().session;
        let opened = tokio::task::spawn_blocking(move || -> Result<Box<dyn MediaIn>, String> {
            if let Some(f) = port {
                return f();
            }
            ghost_audio::Capture::open(ghost_audio::CaptureConfig {
                name: "ghost-dictation-capture".into(),
                description: "Ghost (dictation)".into(),
                ..ghost_audio::CaptureConfig::for_spec(&spec)
            })
            .map(|c| Box::new(crate::wake::CaptureIn(c)) as Box<dyn MediaIn>)
            .map_err(|e| e.to_string())
        })
        .await;
        match opened {
            Ok(Ok(media)) => Ok(Box::new(crate::wake::CountedIn::new(media, token))),
            Ok(Err(e)) => {
                warn!("dictation: the microphone did not open: {e}");
                Err(DictationReason::MicrophoneFailed)
            }
            Err(_) => Err(DictationReason::MicrophoneFailed),
        }
    }

    fn recognizer(&self) -> Option<SharedRecognizer> {
        self.d.upgrade()?.models().map(|m| m.stt.clone())
    }

    fn pauses(&self) -> Option<Box<dyn Pauses>> {
        let d = self.d.upgrade()?;
        let models = d.models()?;
        let silero = ghost_infer::Silero::load(&models.silero).ok()?;
        let params = ghost_infer::VadParams {
            stop_secs: PAUSE.as_secs_f32(),
            ..d.voice().vad_params()
        };
        Some(Box::new(SileroPauses {
            vad: ghost_infer::VadAnalyzer::new(silero, params),
            now: 0.0,
        }))
    }
}

/// The pause that ends a piece: half a second of quiet after speech.
pub const PAUSE: Duration = Duration::from_millis(500);

/// Silero over dictation's own frames, on the samples' own clock.
struct SileroPauses {
    vad: ghost_infer::VadAnalyzer,
    now: f64,
}

impl Pauses for SileroPauses {
    fn speaking(&mut self, frame: &[i16]) -> bool {
        self.now += frame.len() as f64 / f64::from(RATE);
        match self.vad.analyze(frame, self.now) {
            Ok(state) => state != ghost_infer::VadState::Quiet,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod barrier_tests;
#[cfg(test)]
mod key_tests;
#[cfg(test)]
mod tests;
