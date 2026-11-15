//! Ghost's own dictation (ADR 0077): the operator starts it, speaks, stops it, and the
//! Parakeet Ghost already has reads the audio; in this slice the text ends on the clipboard.
//! Not nerd-dictation, the operator's outside tool, which `nerd_dictation` only notices.
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
//! - The words never leave this module but for the clipboard: no log line, no event, no
//!   store, no `debug::audio`, no report. What the API, the CLI and the notification get is
//!   kinds and numbers ([`ghost_api::dictation`]). The audio and the text are wiped when
//!   dropped (`Zeroizing`).

pub mod cleanup;
pub mod clipboard;

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_api::Phase;
use ghost_api::dictation::{
    DictationOutcome, DictationPort, DictationReason, DictationResult, DictationState,
    DictationView,
};
use ghost_voice::frames::Audio;
use ghost_voice::stt::{SharedRecognizer, TRAILING_SECS, dithered};
use ghost_voice::transport::MediaIn;
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use zeroize::Zeroizing;

pub use clipboard::{Clipboard, NoClipboard, XClipboard};

use crate::daemon::{Daemon, lock};

/// The longest a dictation records in this slice: one whole read, no pieces.
pub const LIMIT: Duration = Duration::from_secs(30);
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
    q: Mutex<Option<Lease>>,
    notify: Notify,
}

impl Leases {
    /// Ask for the microphone; `None` when the loop did not let go within `wait`.
    pub async fn ask(&self, wait: Duration) -> Option<MicHold> {
        let (granted_tx, granted_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *lock(&self.q) = Some(Lease {
            granted: granted_tx,
            release: release_rx,
        });
        self.notify.notify_one();
        let hold = MicHold(Some(release_tx));
        match tokio::time::timeout(wait, granted_rx).await {
            Ok(Ok(())) => Some(hold),
            _ => {
                lock(&self.q).take();
                None
            }
        }
    }

    /// The next lease asked for: the passive loop waits on this beside the wake.
    pub async fn next(&self) -> Lease {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(l) = lock(&self.q).take() {
                return l;
            }
            notified.await;
        }
    }
}

// --- what the coordinator stands on ---------------------------------------------------------

/// The daemon's side, or a test's.
#[async_trait]
pub trait Host: Send + Sync {
    /// Why a dictation cannot start now, if it cannot.
    fn refusal(&self) -> Option<DictationReason>;
    /// The microphone from the passive loop.
    async fn take_mic(&self) -> Result<MicHold, DictationReason>;
    /// Dictation's own capture on the session's microphone.
    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason>;
    /// The shared Parakeet.
    fn recognizer(&self) -> Option<SharedRecognizer>;
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
}

struct Recording {
    generation: u64,
    since: Instant,
    first_frame_ms: u64,
    stop: CancellationToken,
    task: JoinHandle<Captured>,
}

enum Stage {
    Idle,
    Recording(Recording),
    Reading { generation: u64 },
}

struct Inner {
    generation: u64,
    phase: Stage,
    last: Option<DictationResult>,
}

pub struct Dictation {
    host: Arc<dyn Host>,
    clipboard: Arc<dyn Clipboard>,
    notes: Arc<dyn Notifier>,
    limit: Duration,
    /// Start, stop, toggle and cancel, one at a time.
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
        Arc::new_cyclic(|me| Self {
            host,
            clipboard,
            notes,
            limit,
            ops: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                generation: 0,
                phase: Stage::Idle,
                last: None,
            }),
            done: watch::channel(None).0,
            me: me.clone(),
        })
    }

    pub fn state(&self) -> DictationState {
        match lock(&self.inner).phase {
            Stage::Idle => DictationState::Idle,
            Stage::Recording(_) => DictationState::Recording,
            Stage::Reading { .. } => DictationState::Reading,
        }
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
            state: match inner.phase {
                Stage::Idle => DictationState::Idle,
                Stage::Recording(_) => DictationState::Recording,
                Stage::Reading { .. } => DictationState::Reading,
            },
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

    /// A refusal: said, remembered, answered.
    fn refuse(&self, reason: DictationReason) -> DictationView {
        let r = DictationResult::refused(reason);
        info!("dictation: refused ({reason:?})");
        self.notes.show(Note::Ended(r));
        self.view_with(Some(r))
    }

    /// Open the microphone and record.
    pub async fn start(&self) -> DictationView {
        let _op = self.ops.lock().await;
        match self.state() {
            DictationState::Idle => {}
            DictationState::Recording => return self.refuse(DictationReason::AlreadyDictating),
            DictationState::Reading => return self.refuse(DictationReason::StillReading),
        }
        if let Some(r) = self.host.refusal() {
            return self.refuse(r);
        }
        if self.host.recognizer().is_none() {
            return self.refuse(DictationReason::NoSpeechModel);
        }
        let t0 = Instant::now();
        let hold = match self.host.take_mic().await {
            Ok(h) => h,
            Err(r) => return self.refuse(r),
        };
        let leased = t0.elapsed();
        let mut media = match self.host.open().await {
            Ok(m) => m,
            Err(r) => return self.refuse(r), // the hold drops: the passive loop has it back
        };
        let opened = t0.elapsed();
        let first = match tokio::time::timeout(FIRST_FRAME_WAIT, media.next_frame()).await {
            Ok(Some(f)) => f,
            _ => {
                media.close();
                return self.refuse(DictationReason::MicrophoneFailed);
            }
        };
        let first_frame_ms = t0.elapsed().as_millis() as u64;
        info!(
            "dictation: recording; first frame {first_frame_ms} ms after the start (the \
             microphone handed over in {} ms, the capture open in {} ms)",
            leased.as_millis(),
            (opened - leased).as_millis()
        );
        let stop = CancellationToken::new();
        let generation = {
            let mut inner = lock(&self.inner);
            inner.generation += 1;
            inner.generation
        };
        let max = (self.limit.as_secs_f64() * f64::from(RATE)) as usize;
        let task = tokio::spawn(record(
            media,
            first,
            stop.clone(),
            max,
            hold,
            self.me.clone(),
            generation,
        ));
        lock(&self.inner).phase = Stage::Recording(Recording {
            generation,
            since: t0,
            first_frame_ms,
            stop,
            task,
        });
        self.notes.show(Note::Recording);
        self.view_with(None)
    }

    /// Stop recording and read; the answer comes once the text is on the clipboard.
    pub async fn stop(&self) -> DictationView {
        self.finish(None).await
    }

    /// Start when idle, stop when recording, refuse while reading.
    pub async fn toggle(&self) -> DictationView {
        match self.state() {
            DictationState::Idle => self.start().await,
            DictationState::Recording => self.stop().await,
            DictationState::Reading => self.refuse(DictationReason::StillReading),
        }
    }

    async fn finish(&self, only: Option<u64>) -> DictationView {
        enum Next {
            NotDictating,
            Unchanged,
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
                Stage::Reading { generation } => {
                    inner.phase = Stage::Reading { generation };
                    Next::Wait(generation)
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
        rec.stop.cancel();
        // Awaited: the capture is closed and the passive loop has the microphone back.
        let captured = rec.task.await.unwrap_or(Captured {
            samples: Zeroizing::new(Vec::new()),
            end: End::Stopped,
        });
        let seconds = captured.samples.len() as f64 / f64::from(RATE);
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
        let text = match self.host.recognizer() {
            Some(model) => {
                let samples = captured.samples;
                let read = tokio::task::spawn_blocking(move || read(&model, &samples)).await;
                match read {
                    Ok((text, queue, took)) => {
                        result.queue_ms = Some(queue.as_millis() as u64);
                        result.read_ms = Some(took.as_millis() as u64);
                        Zeroizing::new(cleanup::clean(&text))
                    }
                    Err(_) => Zeroizing::new(String::new()),
                }
            }
            None => Zeroizing::new(String::new()),
        };
        // The commit: only while this dictation is still the one being read.
        let _op = self.ops.lock().await;
        let current =
            matches!(lock(&self.inner).phase, Stage::Reading { generation: g } if g == generation);
        if !current {
            drop(text); // wiped; a cancelled read does nothing
            let r = self
                .done
                .borrow()
                .filter(|(g, _)| *g == generation)
                .map(|(_, r)| r);
            return self.view_with(r);
        }
        if !text.is_empty() {
            result.chars = text.chars().count();
            let clipboard = self.clipboard.clone();
            let set = tokio::task::spawn_blocking(move || clipboard.set(text)).await;
            match set {
                Ok(Ok(())) => result.outcome = DictationOutcome::OnClipboard,
                Ok(Err(why)) => {
                    result.outcome = DictationOutcome::NotOnClipboard;
                    result.reason = Some(why);
                }
                Err(_) => {
                    result.outcome = DictationOutcome::NotOnClipboard;
                    result.reason = Some(DictationReason::ClipboardFailed);
                }
            }
        }
        info!(
            "dictation: {:?}{}; {:.1} s recorded, read in {} ms after {} ms in the queue, {} \
             characters",
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
            inner.phase = Stage::Idle;
            inner.last = Some(result);
        }
        self.done.send_replace(Some((generation, result)));
        self.notes.show(Note::Ended(result));
        self.view_with(Some(result))
    }

    /// Drop the dictation, recorded or being read: nothing reaches the clipboard. Waits
    /// until a recording's capture is closed, so the microphone is free when it returns.
    pub async fn cancel(&self, why: DictationReason) -> DictationView {
        let _op = self.ops.lock().await;
        let was = std::mem::replace(&mut lock(&self.inner).phase, Stage::Idle);
        let (generation, rec) = match was {
            Stage::Idle => {
                return self.view_with(Some(DictationResult::refused(
                    DictationReason::NotDictating,
                )));
            }
            Stage::Recording(r) => (r.generation, Some(r)),
            Stage::Reading { generation } => (generation, None),
        };
        if let Some(rec) = rec {
            rec.stop.cancel();
            let _ = rec.task.await; // the samples are wiped as they drop
        }
        let r = DictationResult::cancelled(why);
        lock(&self.inner).last = Some(r);
        self.done.send_replace(Some((generation, r)));
        info!("dictation: cancelled ({why:?})");
        self.notes.show(Note::Ended(r));
        self.view_with(Some(r))
    }

    /// Cancel if anything is going on; nothing to say otherwise.
    pub async fn end(&self, why: DictationReason) {
        if self.state() != DictationState::Idle {
            self.cancel(why).await;
        }
    }
}

/// Record until stopped, the limit, or the microphone going away; then close the capture,
/// give the microphone back, and if it ended by itself, finish the dictation.
async fn record(
    mut media: Box<dyn MediaIn>,
    first: Audio,
    stop: CancellationToken,
    max: usize,
    hold: MicHold,
    me: Weak<Dictation>,
    generation: u64,
) -> Captured {
    let mut samples = Zeroizing::new(Vec::with_capacity(max.min(RATE as usize * 10)));
    let mut first = Zeroizing::new(first.samples);
    samples.extend_from_slice(&first);
    first.clear();
    let mut end = End::Stopped;
    loop {
        if samples.len() >= max {
            samples.truncate(max);
            end = End::Limit;
            break;
        }
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            f = media.next_frame() => match f {
                Some(f) => {
                    let f = Zeroizing::new(f.samples);
                    samples.extend_from_slice(&f);
                }
                None => {
                    end = End::MicrophoneEnded;
                    break;
                }
            },
        }
    }
    media.close();
    drop(media);
    drop(hold);
    if end != End::Stopped {
        if end == End::MicrophoneEnded {
            warn!("dictation: the microphone went away; reading what was said");
        }
        if let Some(d) = me.upgrade() {
            tokio::spawn(async move {
                d.finish(Some(generation)).await;
            });
        }
    }
    Captured { samples, end }
}

/// One whole read on the shared Parakeet, as `ghost_voice::stt::recognize` makes it (the
/// trailing silence a segment gets, the dither, the trim), with the wait for the model and
/// the read timed apart. Parakeet's error is a kind, never words, and is not logged here.
fn read(model: &SharedRecognizer, samples: &[i16]) -> (Zeroizing<String>, Duration, Duration) {
    let mut audio = Zeroizing::new(ghost_infer::audio::to_f32(samples));
    audio.extend(std::iter::repeat_n(
        0.0f32,
        (TRAILING_SECS * f64::from(RATE)) as usize,
    ));
    let asked = Instant::now();
    let mut guard = model.lock().unwrap_or_else(|e| e.into_inner());
    let queue = asked.elapsed();
    let began = Instant::now();
    let audio = Zeroizing::new(dithered(&audio));
    let text = match guard.transcribe(&audio) {
        Ok(t) => Zeroizing::new(t.trim().to_string()),
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

/// `ghostd dictate start|stop|toggle|cancel|status`.
pub async fn cli(
    args: &[String],
    client: &ghost_api::GhostClient,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
) -> i32 {
    let usage = "ghostd dictate start|stop|toggle|cancel|status";
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
    let (line, code) = describe(&view);
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
    fn refusal(&self) -> Option<DictationReason> {
        let Some(d) = self.d.upgrade() else {
            return Some(DictationReason::Stopping);
        };
        if d.stop.is_cancelled() {
            return Some(DictationReason::Stopping);
        }
        if d.mic.is_off() {
            return Some(DictationReason::NotListening); // ADR 0057 stands
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

    async fn take_mic(&self) -> Result<MicHold, DictationReason> {
        let d = self.d.upgrade().ok_or(DictationReason::Stopping)?;
        d.dictation_mic
            .ask(LEASE_WAIT)
            .await
            .ok_or(DictationReason::Busy)
    }

    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason> {
        let d = self.d.upgrade().ok_or(DictationReason::Stopping)?;
        if let Some(f) = d.ports.dictation_media.clone() {
            return f().map_err(|_| DictationReason::MicrophoneFailed);
        }
        // What a session hears (ADR 0064): the plan's session input.
        let spec = d.mic_plan().session;
        let opened = tokio::task::spawn_blocking(move || {
            ghost_audio::Capture::open(ghost_audio::CaptureConfig {
                name: "ghost-dictation-capture".into(),
                description: "Ghost (dictation)".into(),
                ..ghost_audio::CaptureConfig::for_spec(&spec)
            })
        })
        .await;
        match opened {
            Ok(Ok(c)) => Ok(Box::new(crate::wake::CaptureIn(c)) as Box<dyn MediaIn>),
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
}

#[cfg(test)]
mod barrier_tests;
#[cfg(test)]
mod tests;
