//! Wake detection (`ghostd.wake` and `ghost_voice.wake`'s listener).
//!
//! Passive state: microphone frames stay on this machine. [`SpokenWakeDetector`] runs the
//! passive listener: by transcript (the default, ADR 0010: local VAD and Parakeet, every
//! transcript without a wake phrase dropped), or by the trained "hey ghost" detector with
//! Parakeet checking its hits (`GHOST_WAKE_MODE=onnx`, ADR 0025). [`FileWakeDetector`] is
//! the development trigger: `ghostd wake` (or SIGUSR1); every mode honours it.
//!
//! [`WakeListener`] runs a fresh passive pipeline each time `wait()` is called, until a
//! phrase matches: the capture, the VAD, then Parakeet and the phrase matcher, or the
//! detector. Everything is rebuilt per wait, and the microphone is released before the
//! session opens it. Transcripts end at the matcher: they are never forwarded, kept, logged
//! (except under `GHOST_DEBUG_AUDIO`, through `debug::audio`) or sent anywhere.
//!
//! `wait()` returns what the operator said after the wake phrase, or `None`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ghost_infer::wake::{ADDRESS_MODEL_GLOB, DEFAULT_MODEL_NAME, WakeMatch, match_wake};
use ghost_infer::{VadAnalyzer, VadParams, VadState};
use ghost_voice::VoiceSettings;
use ghost_voice::debug;
use ghost_voice::frames::{Direction, Frame, Out, Stage};
use ghost_voice::pipeline::Pipeline;
use ghost_voice::transport::MediaIn;
use ghost_voice::wakeword::{
    DETECTOR_PHRASE, Detector, DetectorWakeProcessor, OnHeard, OnMatch, Recognize, Scorer,
};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::config::GhostdSettings;

/// The manual trigger's file in the runtime dir.
pub const WAKE_FILE: &str = "wake";
/// How often the file trigger looks for its file.
pub const FILE_POLL: Duration = Duration::from_millis(200);
/// A pipeline that ended on its own is rebuilt after this, doubling up to `BACKOFF_MAX`.
pub const BACKOFF: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long `stop` waits for the passive pipeline's stages to finish.
pub const STOP_WAIT: Duration = Duration::from_secs(5);

fn lock<T: ?Sized>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- an asyncio.Event -------------------------------------------------------------------

/// `asyncio.Event`: set, cleared, waited on by any number of tasks; a wait after a set
/// returns at once.
#[derive(Default)]
pub struct Event {
    set: AtomicBool,
    notify: Notify,
}

impl Event {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self) {
        self.set.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn clear(&self) {
        self.set.store(false, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        loop {
            if self.is_set() {
                return;
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

// --- the detectors ---------------------------------------------------------------------

/// What the passive loop waits on: the wake, with any words spoken after it.
#[async_trait]
pub trait WakeDetector: Send + Sync {
    /// Return when the wake word fires, with any words spoken after it.
    async fn wait(&self) -> Option<String>;
    /// Release the microphone: after a `wait` that was dropped (the loop chose something
    /// else) the pipeline may still be running.
    async fn stop(&self);
    fn close(&self);
}

/// The manual trigger: the wake file, SIGUSR1, or `trigger()` from the control API
/// (optionally with words to treat as the first turn).
pub struct FileWakeDetector {
    path: PathBuf,
    event: Event,
    text: Mutex<Option<String>>,
}

impl FileWakeDetector {
    /// `path` is the wake file; SIGUSR1 is watched by a task of its own (a runtime is
    /// needed) and ignored where the signal can't be registered.
    pub fn new(path: impl Into<PathBuf>) -> Arc<Self> {
        let this = Arc::new(Self {
            path: path.into(),
            event: Event::new(),
            text: Mutex::new(None),
        });
        if let Ok(mut usr1) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
        {
            let weak = Arc::downgrade(&this);
            tokio::spawn(async move {
                while usr1.recv().await.is_some() {
                    let Some(d) = weak.upgrade() else { break };
                    d.event.set();
                }
            });
        }
        this
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn trigger(&self, text: Option<String>) {
        *lock(&self.text) = text;
        self.event.set();
    }

    /// Until the file appears, the signal arrives or `trigger` is called.
    pub async fn wait(&self) -> Option<String> {
        let _ = std::fs::remove_file(&self.path);
        self.event.clear();
        *lock(&self.text) = None;
        while !self.event.is_set() {
            if self.path.exists() {
                let _ = std::fs::remove_file(&self.path);
                return None;
            }
            let _ = tokio::time::timeout(FILE_POLL, self.event.wait()).await;
        }
        lock(&self.text).take()
    }

    pub fn close(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[async_trait]
impl WakeDetector for FileWakeDetector {
    async fn wait(&self) -> Option<String> {
        FileWakeDetector::wait(self).await
    }

    async fn stop(&self) {}

    fn close(&self) {
        FileWakeDetector::close(self);
    }
}

/// The passive listener hears the wake phrase; the file/SIGUSR1 trigger still works
/// alongside. `on_heard` runs when the trained detector has decided, before the operator
/// finishes speaking (the early chime); transcript wake never calls it.
pub struct SpokenWakeDetector {
    listener: Arc<WakeListener>,
    manual: Arc<FileWakeDetector>,
    on_heard: Option<OnHeard>,
}

impl SpokenWakeDetector {
    pub fn new(
        listener: Arc<WakeListener>,
        manual: Arc<FileWakeDetector>,
        on_heard: Option<OnHeard>,
    ) -> Self {
        Self {
            listener,
            manual,
            on_heard,
        }
    }
}

#[async_trait]
impl WakeDetector for SpokenWakeDetector {
    async fn wait(&self) -> Option<String> {
        let spoken = self.listener.wait(self.on_heard.clone());
        let manual = self.manual.wait();
        tokio::pin!(spoken);
        tokio::pin!(manual);
        let out = tokio::select! {
            m = &mut spoken => (!m.remainder.is_empty()).then_some(m.remainder),
            t = &mut manual => t,
        };
        // Whether a phrase matched or the file fired: release the microphone before
        // anything else runs.
        self.listener.stop().await;
        out
    }

    async fn stop(&self) {
        self.listener.stop().await;
    }

    fn close(&self) {
        self.manual.close();
    }
}

// --- the passive pipeline's stages ------------------------------------------------------

/// The passive pipeline's source: the microphone's frames go in; when the source closes
/// (the device vanished) the pipeline ends, so the listener rebuilds it.
struct PassiveInput {
    media: Option<Box<dyn MediaIn>>,
    token: CancellationToken,
    task: Option<JoinHandle<()>>,
}

#[async_trait]
impl Stage for PassiveInput {
    fn name(&self) -> &'static str {
        "audio_in"
    }

    async fn started(&mut self, out: &Out) {
        let Some(mut media) = self.media.take() else {
            return;
        };
        let out = out.clone();
        let token = self.token.clone();
        self.task = Some(tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    f = media.next_frame() => f,
                };
                match frame {
                    Some(frame) => out.push(Frame::AudioIn(frame)).await,
                    None => {
                        out.push(Frame::End).await;
                        break;
                    }
                }
            }
            media.close();
        }));
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Up {
            return;
        }
        if matches!(frame, Frame::End | Frame::Cancel)
            && let Some(t) = self.task.take()
        {
            t.abort();
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

fn epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Silero on the passive audio: `VadStarted` before the frame that began the speech,
/// `VadStopped` after the one that ended it, the same order as a session's STT stage.
/// Without a model (none loaded) nothing is said and the audio passes through.
pub struct VadStage {
    vad: Option<VadAnalyzer>,
    params: VadParams,
    speaking: bool,
}

impl VadStage {
    pub fn new(vad: Option<VadAnalyzer>, params: VadParams) -> Self {
        Self {
            vad,
            params,
            speaking: false,
        }
    }
}

#[async_trait]
impl Stage for VadStage {
    fn name(&self) -> &'static str {
        "vad"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Down
            && let Frame::AudioIn(audio) = &frame
            && let Some(vad) = &mut self.vad
        {
            let after = vad.analyze(&audio.samples, epoch()).unwrap_or(vad.state());
            if !self.speaking && after == VadState::Speaking {
                self.speaking = true;
                debug::audio(|| "passive: speech started".to_string());
                out.push(Frame::VadStarted {
                    start_secs: self.params.start_secs,
                })
                .await;
            }
            let stopped = self.speaking && after == VadState::Quiet;
            out.forward(frame, direction).await;
            if stopped {
                self.speaking = false;
                debug::audio(|| "passive: speech stopped, transcribing".to_string());
                out.push(Frame::VadStopped {
                    stop_secs: self.params.stop_secs,
                })
                .await;
            }
            return;
        }
        out.forward(frame, direction).await;
    }
}

/// Last stage of the passive pipeline in transcript mode. Transcripts end here.
pub struct WakeMatchStage {
    phrases: Vec<String>,
    on_match: OnMatch,
    fuzzy: f64,
}

impl WakeMatchStage {
    pub fn new(phrases: Vec<String>, on_match: OnMatch, fuzzy: f64) -> Self {
        Self {
            phrases,
            on_match,
            fuzzy,
        }
    }
}

#[async_trait]
impl Stage for WakeMatchStage {
    fn name(&self) -> &'static str {
        "wake_match"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::Transcript(text) | Frame::Interim(text) if direction == Direction::Down => {
                if let Some(m) = match_wake(text, &self.phrases, self.fuzzy) {
                    (self.on_match)(m).await;
                } else {
                    debug::audio(|| format!("passive: no wake phrase in {text:?}"));
                }
                // transcripts are never forwarded or kept
            }
            Frame::AudioIn(_) => {} // passive audio ends here too
            _ => out.forward(frame, direction).await,
        }
    }
}

// --- the listener -----------------------------------------------------------------------

/// Where the passive pipeline's audio comes from: `ghost.mic` through `ghost_audio`, or a
/// test's device. Built per pipeline, so the microphone is released between waits.
pub type MediaFactory = Arc<dyn Fn() -> Result<Box<dyn MediaIn>, String> + Send + Sync>;
/// The speech-to-text stage for transcript mode, over the VAD's parameters: Parakeet on the
/// models the daemon loaded, or a fake.
pub type SttFactory = Arc<dyn Fn(VadParams) -> Box<dyn Stage> + Send + Sync>;
/// Silero for one pipeline, or None (no file: the tests that don't feed audio).
pub type VadFactory = Arc<dyn Fn(VadParams) -> Option<VadAnalyzer> + Send + Sync>;

/// The trained detectors' parts, shared across every pipeline the listener builds (the
/// scorer's sessions are loaded once).
#[derive(Clone)]
pub struct DetectorSpec {
    pub scorer: Arc<Mutex<dyn Scorer>>,
    pub recognize: Recognize,
    pub models: Vec<ghost_infer::WakeModel>,
    pub verify: bool,
    pub cooldown_secs: f64,
}

/// The listener's view of a shared scorer.
struct SharedScorer(Arc<Mutex<dyn Scorer>>);

impl Scorer for SharedScorer {
    fn score(&mut self, window: &[i16]) -> ghost_infer::Result<Vec<(String, f32)>> {
        lock(&self.0).score(window)
    }
}

impl DetectorSpec {
    fn detector(&self) -> Detector {
        Detector {
            scorer: Box::new(SharedScorer(self.scorer.clone())),
            recognize: self.recognize.clone(),
            models: self.models.clone(),
            verify: self.verify,
            cooldown_secs: self.cooldown_secs,
        }
    }
}

/// One passive pipeline, built and not yet run: what the tests look at.
pub struct Built {
    pub pipeline: Pipeline,
    pub token: CancellationToken,
    /// The VAD's parameters: the listener's own stop, longer than a session's.
    pub vad_params: VadParams,
    pub stage_names: Vec<&'static str>,
}

struct Running {
    token: CancellationToken,
    done: watch::Receiver<bool>,
}

/// The passive listener: a fresh pipeline each `wait`, until a phrase matches.
pub struct WakeListener {
    settings: Arc<VoiceSettings>,
    stt_factory: SttFactory,
    vad_factory: VadFactory,
    media_factory: MediaFactory,
    phrases: Mutex<Vec<String>>,
    fuzzy: Mutex<f64>,
    detector: Option<DetectorSpec>,
    on_heard: Mutex<Option<OnHeard>>,
    running: Mutex<Option<Running>>,
    builds: AtomicUsize,
    backoff: Duration,
}

impl WakeListener {
    pub fn new(
        settings: Arc<VoiceSettings>,
        stt_factory: SttFactory,
        phrases: Vec<String>,
        media_factory: MediaFactory,
        fuzzy: f64,
        detector: Option<DetectorSpec>,
    ) -> Self {
        let silero = settings.silero_model.clone();
        let vad_factory: VadFactory =
            Arc::new(move |params| match ghost_infer::Silero::load(&silero) {
                Ok(m) => Some(VadAnalyzer::new(m, params)),
                Err(e) => {
                    tracing::warn!("Silero: {e}; the passive listener hears no speech");
                    None
                }
            });
        Self {
            settings,
            stt_factory,
            vad_factory,
            media_factory,
            phrases: Mutex::new(phrases),
            fuzzy: Mutex::new(fuzzy),
            detector,
            on_heard: Mutex::new(None),
            running: Mutex::new(None),
            builds: AtomicUsize::new(0),
            backoff: BACKOFF,
        }
    }

    /// The VAD's model, for a test that feeds its own frames.
    pub fn with_vad_factory(mut self, vad_factory: VadFactory) -> Self {
        self.vad_factory = vad_factory;
        self
    }

    /// The first retry's wait after a pipeline ended on its own.
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Change what the next passive pipeline listens for. Takes effect when the listener
    /// is next (re)built, so the daemon re-arms it after a settings change.
    pub fn set_phrases(&self, phrases: Vec<String>, fuzzy: Option<f64>) {
        *lock(&self.phrases) = phrases;
        if let Some(f) = fuzzy {
            *lock(&self.fuzzy) = f;
        }
    }

    pub fn phrases(&self) -> Vec<String> {
        lock(&self.phrases).clone()
    }

    pub fn fuzzy(&self) -> f64 {
        *lock(&self.fuzzy)
    }

    /// How many pipelines have been built.
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }

    /// A pipeline is running now.
    pub fn is_running(&self) -> bool {
        lock(&self.running).is_some()
    }

    /// The passive pipeline: the capture, the VAD, then Parakeet and the phrase matcher,
    /// or the detector. The detector decides on sound, so the VAD only says when the
    /// operator paused: the session's short stop lets the check run at once, and the
    /// processor waits out the rest of `wake_vad_stop_secs` before it ends their
    /// utterance.
    pub fn build(&self, on_match: OnMatch) -> Result<Built, String> {
        let s = &self.settings;
        let stop = if self.detector.is_some() {
            s.vad_stop_secs
        } else {
            s.wake_vad_stop_secs
        };
        let vad_params = VadParams {
            confidence: s.vad_confidence,
            start_secs: s.vad_start_secs,
            stop_secs: stop,
            min_volume: s.vad_min_volume,
        };
        let media = (self.media_factory)()?;
        let token = CancellationToken::new();
        let mut stages: Vec<Box<dyn Stage>> = vec![
            Box::new(PassiveInput {
                media: Some(media),
                token: token.clone(),
                task: None,
            }),
            Box::new(VadStage::new((self.vad_factory)(vad_params), vad_params)),
        ];
        match &self.detector {
            Some(spec) => stages.push(Box::new(DetectorWakeProcessor::new(
                spec.detector(),
                on_match,
                lock(&self.on_heard).clone(),
                self.fuzzy(),
                (f64::from(s.wake_vad_stop_secs) - f64::from(stop)).max(0.0),
            ))),
            None => {
                stages.push((self.stt_factory)(vad_params));
                stages.push(Box::new(WakeMatchStage::new(
                    self.phrases(),
                    on_match,
                    self.fuzzy(),
                )));
            }
        }
        let stage_names = stages.iter().map(|s| s.name()).collect();
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Built {
            pipeline: Pipeline::new(stages, None),
            token,
            vad_params,
            stage_names,
        })
    }

    /// Until a phrase matches. `on_heard` runs when the detector has decided, before the
    /// operator finishes speaking (detector mode only; transcript wake decides at the end
    /// of the utterance, so it never calls it).
    pub async fn wait(&self, on_heard: Option<OnHeard>) -> WakeMatch {
        *lock(&self.on_heard) = on_heard;
        let (tx, mut rx) = mpsc::channel::<WakeMatch>(1);
        let mut backoff = self.backoff;
        loop {
            let on_match: OnMatch = {
                let tx = tx.clone();
                Arc::new(move |m: WakeMatch| {
                    let tx = tx.clone();
                    Box::pin(async move {
                        if tx.try_send(m.clone()).is_ok() {
                            info!("wake phrase: {:?}", m.phrase);
                        }
                    })
                })
            };
            let built = match self.build(on_match) {
                Ok(b) => b,
                Err(e) => {
                    error!("passive listener could not start: {e}; retry in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            };
            let (done_tx, done_rx) = watch::channel(false);
            let token = built.token.clone();
            let run = built.pipeline.run(token.clone());
            tokio::spawn(async move {
                run.await;
                let _ = done_tx.send(true);
            });
            *lock(&self.running) = Some(Running {
                token,
                done: done_rx.clone(),
            });
            let mut done = done_rx;
            let matched = tokio::select! {
                m = rx.recv() => m,
                _ = done.wait_for(|d| *d) => None,
            };
            self.stop().await;
            if let Some(m) = matched {
                return m;
            }
            // The pipeline ended on its own (device gone, stage failure). Rebuild it
            // rather than sit in "passive" with nobody reading the microphone.
            error!(
                "passive listener stopped without a wake phrase; retry in {}s",
                backoff.as_secs_f64()
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    /// Cancel the running pipeline and wait for its stages, so the microphone is free.
    pub async fn stop(&self) {
        let running = lock(&self.running).take();
        let Some(Running { token, mut done }) = running else {
            return;
        };
        token.cancel();
        if tokio::time::timeout(STOP_WAIT, done.wait_for(|d| *d))
            .await
            .is_err()
        {
            error!("passive listener did not stop in {STOP_WAIT:?}");
        }
    }
}

// --- what the daemon builds the listener from --------------------------------------------

/// The "hey ghost" model and the "Ghost, …" ones there are, as set or in the voice cache.
pub fn wake_model_paths(cfg: &GhostdSettings, voice: &VoiceSettings) -> (PathBuf, Vec<PathBuf>) {
    let wake_dir = voice.cache_dir.join("wake");
    let hey = cfg
        .wake_model_path
        .clone()
        .unwrap_or_else(|| wake_dir.join(DEFAULT_MODEL_NAME));
    let address: Vec<PathBuf> = match &cfg.wake_address_model_path {
        Some(p) => vec![p.clone()],
        None => {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&wake_dir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| matches_glob(p, ADDRESS_MODEL_GLOB))
                        .collect()
                })
                .unwrap_or_default();
            found.sort();
            found
        }
    };
    let address = address
        .into_iter()
        .filter(|p| p.exists() && *p != hey)
        .collect();
    (hey, address)
}

/// `ghost_*.onnx` and the like: a prefix, a `*`, a suffix.
fn matches_glob(path: &Path, glob: &str) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match glob.split_once('*') {
        Some((prefix, suffix)) => {
            name.starts_with(prefix)
                && name.ends_with(suffix)
                && name.len() >= prefix.len() + suffix.len()
        }
        None => name == glob,
    }
}

/// Where livekit-wakeword's feature models are (`melspectrogram.onnx`,
/// `embedding_model.onnx`): the voice cache's `wake/` folder, else the Python venv's copy.
pub fn wake_resources(voice: &VoiceSettings) -> Option<PathBuf> {
    let cached = voice.cache_dir.join("wake");
    if cached.join("melspectrogram.onnx").is_file() {
        return Some(cached);
    }
    let site = ghost_voice::config::site_packages(&|name| std::env::var(name).ok())?;
    let dir = site.join("livekit/wakeword/resources");
    dir.join("melspectrogram.onnx").is_file().then_some(dir)
}

/// The capture from the desk's microphone, as the passive pipeline takes it.
pub struct CaptureIn(pub ghost_audio::Capture);

#[async_trait]
impl MediaIn for CaptureIn {
    async fn next_frame(&mut self) -> Option<ghost_voice::frames::Audio> {
        let f = self.0.next_frame().await?;
        Some(ghost_voice::frames::Audio::new(f.samples, f.rate))
    }
}

/// `ghost.mic` (or `GHOST_INPUT_DEVICE`) through `ghost_audio`, opened per pipeline.
pub fn desk_capture(input_device: Option<String>) -> MediaFactory {
    Arc::new(move || {
        let capture = ghost_audio::Capture::open(ghost_audio::CaptureConfig {
            node: input_device.clone().or_else(|| Some("ghost.mic".into())),
            ..ghost_audio::CaptureConfig::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Box::new(CaptureIn(capture)) as Box<dyn MediaIn>)
    })
}

/// The detector phrase, for the listener in detector mode.
pub fn detector_phrases() -> Vec<String> {
    vec![DETECTOR_PHRASE.to_string()]
}

#[cfg(test)]
mod tests {
    //! `packages/voice/tests/test_wake.py` (the listener lifecycle) and
    //! `test_wakeword.py` (the detector pipeline), by name.

    use std::sync::atomic::AtomicUsize;

    use ghost_voice::frames::Audio;

    use super::*;

    const PHRASES: [&str; 4] = [
        "hey ghost",
        "ghost you there",
        "ghost are you there",
        "okay ghost",
    ];

    /// A microphone that never says anything: nothing touches a sound device.
    struct Inert;

    #[async_trait]
    impl MediaIn for Inert {
        async fn next_frame(&mut self) -> Option<Audio> {
            std::future::pending().await
        }
    }

    /// A stage that stands in for Parakeet and does nothing.
    struct Nothing;

    #[async_trait]
    impl Stage for Nothing {
        fn name(&self) -> &'static str {
            "stt"
        }
        async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
            out.forward(frame, direction).await;
        }
    }

    fn settings(get: impl Fn(&str) -> Option<String>) -> Arc<VoiceSettings> {
        Arc::new(VoiceSettings::from_vars(get).unwrap())
    }

    fn phrases() -> Vec<String> {
        PHRASES.iter().map(|p| p.to_string()).collect()
    }

    fn no_vad() -> VadFactory {
        Arc::new(|_| None)
    }

    fn listener(media: MediaFactory, stt: SttFactory) -> WakeListener {
        WakeListener::new(settings(|_| None), stt, phrases(), media, 0.0, None)
            .with_vad_factory(no_vad())
            .with_backoff(Duration::ZERO)
    }

    fn inert_media() -> MediaFactory {
        Arc::new(|| Ok(Box::new(Inert) as Box<dyn MediaIn>))
    }

    fn nothing_stt() -> SttFactory {
        Arc::new(|_| Box::new(Nothing) as Box<dyn Stage>)
    }

    #[tokio::test(start_paused = true)]
    async fn test_passive_worker_has_no_idle_timeout() {
        // Pipecat cancelled an idle worker after five minutes by default; a wake listener
        // is idle by definition and must sit for hours: one pipeline, never rebuilt, and
        // nothing in it that times a silent operator out.
        let l = Arc::new(listener(inert_media(), nothing_stt()));
        let built = l.build(Arc::new(|_| Box::pin(async {}))).unwrap();
        assert_eq!(
            built.stage_names,
            vec!["audio_in", "vad", "stt", "wake_match"]
        );
        let waiting = {
            let l = l.clone();
            tokio::spawn(async move { l.wait(None).await })
        };
        tokio::time::advance(Duration::from_secs(6 * 3600)).await;
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert_eq!(l.builds(), 2); // the one above, and the one waiting
        assert!(l.is_running());
        waiting.abort();
        l.stop().await;
        assert!(!l.is_running());
    }

    #[tokio::test]
    async fn test_passive_vad_has_its_own_stop() {
        // The listener waits longer for the end of speech than a turn does, so a comma
        // after "hey ghost" doesn't split the phrase from what follows; the session's VAD
        // keeps its own.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let vad: VadFactory = {
            let seen = seen.clone();
            Arc::new(move |p| {
                seen.lock().unwrap().push(p);
                None
            })
        };
        let l = WakeListener::new(
            settings(|name| (name == "GHOST_WAKE_VAD_STOP_SECS").then(|| "0.7".to_string())),
            nothing_stt(),
            phrases(),
            inert_media(),
            0.0,
            None,
        )
        .with_vad_factory(vad);
        let built = l.build(Arc::new(|_| Box::pin(async {}))).unwrap();
        assert_eq!(built.vad_params.stop_secs, 0.7);
        assert_eq!(seen.lock().unwrap()[0].stop_secs, 0.7);
        assert_eq!(VoiceSettings::default().vad_stop_secs, 0.2);
    }

    /// Yields `frames` once, then waits forever.
    struct Frames(Vec<Vec<i16>>);

    #[async_trait]
    impl MediaIn for Frames {
        async fn next_frame(&mut self) -> Option<Audio> {
            if self.0.is_empty() {
                std::future::pending::<()>().await;
            }
            Some(Audio::new(self.0.remove(0), 16_000))
        }
    }

    /// A source that closed at once: the device vanished.
    struct Gone;

    #[async_trait]
    impl MediaIn for Gone {
        async fn next_frame(&mut self) -> Option<Audio> {
            None
        }
    }

    /// Hears "hey ghost hi" in any audio.
    struct HearsHi;

    #[async_trait]
    impl Stage for HearsHi {
        fn name(&self) -> &'static str {
            "stt"
        }
        async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
            if matches!(frame, Frame::AudioIn(_)) {
                out.push(Frame::Transcript("hey ghost hi".into())).await;
                return;
            }
            out.forward(frame, direction).await;
        }
    }

    #[tokio::test]
    async fn test_listener_rebuilds_when_pipeline_ends_without_match() {
        // A pipeline that exits on its own (device vanished, stage failure) is replaced,
        // so the daemon never reports "passive" with nobody reading the microphone.
        let runs = Arc::new(AtomicUsize::new(0));
        let media: MediaFactory = {
            let runs = runs.clone();
            Arc::new(move || {
                let n = runs.fetch_add(1, Ordering::SeqCst);
                Ok(if n == 0 {
                    Box::new(Gone) as Box<dyn MediaIn> // the first pipeline dies quietly
                } else {
                    Box::new(Frames(vec![vec![0; 320]]))
                })
            })
        };
        let l = listener(media, Arc::new(|_| Box::new(HearsHi) as Box<dyn Stage>));
        let m = tokio::time::timeout(Duration::from_secs(5), l.wait(None))
            .await
            .unwrap();
        assert_eq!(m.remainder, "hi");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(l.builds(), 2);
        assert!(!l.is_running());
    }

    struct NoScore;

    impl Scorer for NoScore {
        fn score(&mut self, _window: &[i16]) -> ghost_infer::Result<Vec<(String, f32)>> {
            Ok(vec![("hey_ghost".into(), 0.0)])
        }
    }

    #[tokio::test]
    async fn test_the_listener_builds_the_detector_pipeline() {
        // Detector mode replaces the transcript of every utterance: no STT in the
        // pipeline, and the session's short VAD stop so the check can run at the first
        // pause.
        let built_stt = Arc::new(AtomicUsize::new(0));
        let stt: SttFactory = {
            let built = built_stt.clone();
            Arc::new(move |_| {
                built.fetch_add(1, Ordering::SeqCst);
                Box::new(Nothing) as Box<dyn Stage>
            })
        };
        let spec = DetectorSpec {
            scorer: Arc::new(Mutex::new(NoScore)),
            recognize: Arc::new(|_| String::new()),
            models: vec![ghost_infer::WakeModel::known(
                Path::new("hey_ghost.onnx"),
                0.5,
                None,
            )],
            verify: true,
            cooldown_secs: 2.0,
        };
        let l = WakeListener::new(
            settings(|_| None),
            stt,
            detector_phrases(),
            inert_media(),
            0.9,
            Some(spec),
        )
        .with_vad_factory(no_vad());
        let built = l.build(Arc::new(|_| Box::pin(async {}))).unwrap();
        assert!(built.stage_names.contains(&"wake"));
        assert!(!built.stage_names.contains(&"stt"));
        assert_eq!(built_stt.load(Ordering::SeqCst), 0);
        assert_eq!(
            built.vad_params.stop_secs,
            VoiceSettings::default().vad_stop_secs
        );
    }

    #[tokio::test]
    async fn the_file_trigger_fires_on_the_file_or_a_call_with_words() {
        let dir = tempfile::tempdir().unwrap();
        let d = FileWakeDetector::new(dir.path().join(WAKE_FILE));
        let waiting = {
            let d = d.clone();
            tokio::spawn(async move { d.wait().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(d.path(), "").unwrap();
        assert_eq!(waiting.await.unwrap(), None);
        assert!(!d.path().exists());
        let waiting = {
            let d = d.clone();
            tokio::spawn(async move { d.wait().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        d.trigger(Some("what needs me".into()));
        assert_eq!(waiting.await.unwrap(), Some("what needs me".into()));
    }

    #[tokio::test]
    async fn the_spoken_detector_takes_the_file_trigger_too_and_stops_the_listener() {
        let dir = tempfile::tempdir().unwrap();
        let manual = FileWakeDetector::new(dir.path().join(WAKE_FILE));
        let l = Arc::new(listener(inert_media(), nothing_stt()));
        let d = SpokenWakeDetector::new(l.clone(), manual.clone(), None);
        let waiting = tokio::spawn(async move { WakeDetector::wait(&d).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(l.is_running());
        manual.trigger(Some("hi".into()));
        assert_eq!(waiting.await.unwrap(), Some("hi".into()));
        assert!(!l.is_running()); // the microphone was released
    }

    #[test]
    fn wake_model_paths_find_the_openers_beside_hey_ghost() {
        let dir = tempfile::tempdir().unwrap();
        let wake = dir.path().join("wake");
        std::fs::create_dir_all(&wake).unwrap();
        for n in [
            "hey_ghost.onnx",
            "ghost_need.onnx",
            "ghost_address.onnx",
            "other.onnx",
        ] {
            std::fs::write(wake.join(n), "").unwrap();
        }
        let voice = VoiceSettings::from_vars(|name| {
            (name == "GHOST_CACHE_DIR").then(|| dir.path().to_string_lossy().into_owned())
        })
        .unwrap();
        let cfg = GhostdSettings::default();
        let (hey, address) = wake_model_paths(&cfg, &voice);
        assert_eq!(hey, wake.join("hey_ghost.onnx"));
        assert_eq!(
            address,
            vec![
                wake.join("ghost_address.onnx"),
                wake.join("ghost_need.onnx")
            ]
        );
    }

    #[tokio::test]
    async fn an_event_set_before_the_wait_returns_at_once() {
        let e = Event::new();
        e.set();
        tokio::time::timeout(Duration::from_millis(100), e.wait())
            .await
            .unwrap();
        e.clear();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), e.wait())
                .await
                .is_err()
        );
    }
}
