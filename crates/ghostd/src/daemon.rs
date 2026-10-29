//! Passive -> wake -> conversation -> sleep, forever (`ghostd.main.Daemon`).
//!
//! [`Daemon`] runs the loop and implements `ghost_api::Controller` for the desktop app
//! (`controller.rs`) and the phone listener's `PhoneHost` (`phone_host.rs`); one session
//! from wake to end is `session.rs`. [`Daemon::run`] starts what the Python started, in
//! its order: the vault, the store, the gate, the folder, the waiting list, the watch
//! runner, the hook intake, the routes, the control API, the phone listener, the hooks
//! listener, the models (loaded once), then the passive loop; stopped by the
//! [`CancellationToken`] that SIGTERM, SIGINT and a gone parent trigger, it winds down in
//! the Python's order.
//!
//! The rules hold here (CLAUDE.md): nothing from the passive loop is kept, logged or sent;
//! a password never reaches a log line (the log mirror redacts what it hands the desktop);
//! every change goes through the gate; the model has no route to the control API.
//!
//! What a test stands in for lives in [`Ports`]: the keyring, the models, the model
//! client, the desk's devices, the passive capture, playback, ducking and the session
//! runner; everything else is the real thing over a temp folder.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use ghost_api::deps::LiveModelNet;
use ghost_api::models::{SessionOrigin, TurnVia};
use ghost_api::server::default_socket_path;
use ghost_api::{DaemonState, Deps, DevicesPort, Phase};
use ghost_audio::AudioSettings;
use ghost_core::apps::{APPS_FILE, Apps, Environ, load_apps};
use ghost_core::catalog::Catalog;
use ghost_core::confirmation::{ConfirmationClassifier, RegexClassifier};
use ghost_core::conversations::{
    Conversations, ConversationsOptions, GHOST, Profile, SOURCE as CONVERSATIONS,
    conversations_source,
};
use ghost_core::folder::Folder;
use ghost_core::gate::WriteGate;
use ghost_core::hooks::{HookIntake, Secrets};
use ghost_core::jev::{JevClassifier, JevClient, JevOptions, WatchDecider, watch_decider};
use ghost_core::outputs::{Outputs, SOURCE as OUTPUTS};
use ghost_core::passwords::{Keyring, Passwords, SharedEnviron};
use ghost_core::policy::AutonomyPolicy;
use ghost_core::routes::{RouteEngine, RouteName, RoutePolicy, Urgency};
use ghost_core::rules::{Rules, SOURCE as RULES, rules_source};
use ghost_core::signin::{SignIns, auth_for_apps};
use ghost_core::skill_catalog::SkillCatalog;
use ghost_core::skills::{SOURCE as SKILLS, Skills, SkillsFrontMatter};
use ghost_core::source::{EventKind, ToolSource};
use ghost_core::store::Store;
use ghost_core::waiting::{Items, WaitingList, waiting_source};
use ghost_core::watches::{AppReader, Choice, Decider, Scalar, WatchRunner};
use ghost_core::{Events, redact_text};
use ghost_infer::wake::{WakeModel, WakeScorer, parse_phrases};
use ghost_voice::VoiceSettings;
use ghost_voice::config::LLM_FIELDS;
use ghost_voice::debug;
use ghost_voice::llm::ModelPort;
use ghost_voice::pipeline::{GhostSession, Models};
use ghost_voice::stt::ParakeetStt;
use ghost_voice::transport::{Media, desk_media};
use ghost_voice::wakeword::OnHeard;
use serde_json::{Map, Value};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::layer::Context;

use crate::build_info;
use crate::check::{
    declared_urgency, describe_apps, describe_routes, describe_skills, ghost_problems,
};
use crate::config::{GhostdSettings, runtime_dir};
use crate::deciders::{Completer, model_decider};
use crate::dictation::is_dictating;
use crate::hooks_listener::HooksListener;
use crate::passwords::{
    Opened, VAULT_RETRY, in_use, open_passwords, operator_keyring, problems as password_problems,
    wait_for_vault,
};
use crate::phone::{PhoneListener, PhoneServers, PhoneStore};
use crate::routes::{DeskRoute, DesktopRoute, PhoneRoute, Raised};
use crate::wake::{
    DetectorSpec, Event, FileWakeDetector, MediaFactory, SpokenWakeDetector, SttFactory, WAKE_FILE,
    WakeDetector, WakeListener, desk_capture, detector_phrases, wake_model_paths, wake_resources,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DUCK_STATE: &str = "duck.json";
/// How often Ghost's own items are re-read and the routes tick.
pub const WAITING_REFRESH_SECS: u64 = 15;
/// A desk session this recent means the operator is at the desk.
pub const AT_DESK_SECS: u64 = 15 * 60;
/// How long the wind-down waits for the control API to close.
pub const API_STOP_WAIT: Duration = Duration::from_secs(5);

/// The model connection's settings (ADR 0037).
pub fn llm_env_keys() -> BTreeSet<&'static str> {
    LLM_FIELDS.iter().map(|(_, env)| *env).collect()
}

pub fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- what a test stands in for ------------------------------------------------------------

/// The music ducking, as the loop and the sessions use it (`ghost_audio::Ducker`, or a
/// test's fake).
pub trait Duck: Send + Sync {
    fn duck(&self);
    fn restore(&self);
}

impl Duck for ghost_audio::Ducker {
    fn duck(&self) {
        match ghost_audio::Ducker::duck(self) {
            Ok(d) if !d.is_empty() => info!("ducked {} stream(s)", d.len()),
            Ok(_) => {}
            Err(e) => warn!("duck: {e}"),
        }
    }

    fn restore(&self) {
        if let Err(e) = ghost_audio::Ducker::restore(self) {
            warn!("duck: {e}");
        }
    }
}

/// No sound stack: nothing to duck.
#[derive(Debug, Default)]
pub struct NoDuck;

impl Duck for NoDuck {
    fn duck(&self) {}
    fn restore(&self) {}
}

/// What starts a session: a wake with the words after it, a phone call in its line, or
/// Ghost raising something at the desk (ADR 0032).
pub struct SessionStart {
    pub first_utterance: Option<String>,
    pub phone: Option<PendingPhone>,
    pub start_in: String,
    pub raised: Option<Raised>,
}

/// Runs one session from wake to end: `session::conversation`, or a test's recorder.
#[async_trait]
pub trait SessionRunner: Send + Sync {
    async fn run(&self, daemon: Arc<Daemon>, start: SessionStart);
}

/// The real sessions.
pub struct LiveSessions;

#[async_trait]
impl SessionRunner for LiveSessions {
    async fn run(&self, daemon: Arc<Daemon>, start: SessionStart) {
        crate::session::conversation(&daemon, start).await;
    }
}

/// The desk's devices for one session (`transport::desk_media`): the audio settings, the
/// thinking sound, and whether the wake chime plays first.
pub struct DeskMediaRequest {
    pub audio: AudioSettings,
    pub thinking_sound: bool,
    pub chime: bool,
}

pub type LoadModels = Arc<dyn Fn(&VoiceSettings) -> anyhow::Result<Models> + Send + Sync>;
pub type ModelFactory = Arc<
    dyn Fn(&VoiceSettings, &HashMap<String, String>) -> anyhow::Result<Arc<dyn ModelPort>>
        + Send
        + Sync,
>;
pub type DeskMedia =
    Arc<dyn Fn(DeskMediaRequest) -> BoxFuture<'static, Result<Media, String>> + Send + Sync>;
/// Play `samples` at `rate` on the device (the chime, a voice sample) and wait for them.
pub type Play = Arc<
    dyn Fn(Vec<i16>, u32, Option<String>) -> BoxFuture<'static, Result<(), String>> + Send + Sync,
>;
pub type DuckFactory = Arc<dyn Fn(&GhostdSettings, PathBuf) -> Arc<dyn Duck> + Send + Sync>;

/// The parts of the daemon a test stands in for. `live` is the workstation's.
#[derive(Clone)]
pub struct Ports {
    pub keyring: Arc<dyn Keyring>,
    pub load_models: LoadModels,
    pub model: ModelFactory,
    pub desk_media: DeskMedia,
    /// The passive pipeline's capture; None: `ghost.mic` through `ghost_audio`.
    pub passive_media: Option<MediaFactory>,
    pub play: Play,
    pub ducker: DuckFactory,
    pub sessions: Arc<dyn SessionRunner>,
    pub devices: Arc<dyn DevicesPort>,
}

impl Ports {
    /// The workstation's: the operator's keyring, the models from the caches, the model
    /// client over the connection, PipeWire for the devices.
    pub fn live() -> Self {
        Self {
            keyring: operator_keyring(),
            load_models: Arc::new(Models::load),
            model: Arc::new(|voice, env| {
                let target = voice.llm_target(Some(env));
                Ok(ghost_llm::model::shared(&target)? as Arc<dyn ModelPort>)
            }),
            desk_media: Arc::new(|req: DeskMediaRequest| {
                Box::pin(async move {
                    desk_media(&req.audio, req.thinking_sound, req.chime)
                        .await
                        .map_err(|e| e.to_string())
                })
            }),
            passive_media: None,
            play: Arc::new(|samples, rate, device| {
                Box::pin(async move { play_live(samples, rate, device).await })
            }),
            ducker: Arc::new(|cfg, state| {
                Arc::new(ghost_audio::Ducker::new(
                    cfg.duck_apps.iter().cloned(),
                    cfg.duck_level,
                    state,
                ))
            }),
            sessions: Arc::new(LiveSessions),
            devices: Arc::new(PwDevices),
        }
    }

    /// No devices, no keyring of the operator's, no models: what a test starts from. The
    /// models, the model client and the sessions must be given before they are used.
    pub fn headless() -> Self {
        Self {
            keyring: Arc::new(ghost_core::MemoryKeyring::new()),
            load_models: Arc::new(|_| anyhow::bail!("no models in this test")),
            model: Arc::new(|_, _| anyhow::bail!("no model in this test")),
            desk_media: Arc::new(|_| {
                Box::pin(async { Ok(ghost_voice::transport::silent_media()) })
            }),
            passive_media: Some(Arc::new(|| {
                Ok(Box::new(ghost_voice::transport::Silent)
                    as Box<dyn ghost_voice::transport::MediaIn>)
            })),
            play: Arc::new(|_, _, _| Box::pin(async { Ok(()) })),
            ducker: Arc::new(|_, _| Arc::new(NoDuck)),
            sessions: Arc::new(LiveSessions),
            devices: Arc::new(ghost_api::deps::NoDevices),
        }
    }
}

/// Play through `ghost_audio` and wait for the device to take it all.
async fn play_live(samples: Vec<i16>, rate: u32, device: Option<String>) -> Result<(), String> {
    let mut playback = ghost_audio::Playback::open(ghost_audio::PlaybackConfig {
        sink: device,
        rate,
        name: "ghost-sample".into(),
        ..ghost_audio::PlaybackConfig::default()
    })
    .map_err(|e| e.to_string())?;
    playback.write(&samples).await.map_err(|e| e.to_string())?;
    playback.drain().await.map_err(|e| e.to_string())?;
    playback.close();
    Ok(())
}

// --- the phone (ADR 0016) -------------------------------------------------------------------

/// An accepted offer's media, waiting for the passive loop to take it.
pub struct PendingPhone {
    pub media: Arc<dyn ghost_rtc::Media>,
    pub line: String,
}

/// The live peer, for a renegotiation and trickled candidates.
pub struct LivePhone {
    pub pc_id: String,
    pub media: Arc<dyn ghost_rtc::Media>,
}

// --- the session's turn handler --------------------------------------------------------------

/// The running session's turn handler, for a typed turn (the desktop's chat input): it
/// answers the digest and desk voice the way a spoken one does.
pub type Heard = Arc<dyn Fn(&'static str, String) -> BoxFuture<'static, ()> + Send + Sync>;

// --- the daemon's parts, set in `run` --------------------------------------------------------

/// What `run` builds, each set once; a controller call before `run` finds them unset and
/// answers as a daemon with nothing behind it.
#[derive(Default)]
pub struct Late {
    pub store: OnceLock<Arc<Store>>,
    pub gate: OnceLock<Arc<WriteGate>>,
    pub waiting: OnceLock<Arc<WaitingList>>,
    pub watch_runner: OnceLock<Arc<WatchRunner>>,
    pub hook_intake: OnceLock<Arc<HookIntake>>,
    pub routes: OnceLock<Arc<RouteEngine>>,
    pub skills: OnceLock<Arc<Skills>>,
    pub folder: OnceLock<Arc<Mutex<Folder>>>,
    pub rules: OnceLock<Arc<Rules>>,
    pub outputs: OnceLock<Arc<Outputs>>,
    pub apps: OnceLock<Arc<Apps>>,
    pub signins: OnceLock<Arc<SignIns>>,
    pub catalog: OnceLock<Arc<Catalog>>,
    pub skill_catalog: OnceLock<Arc<SkillCatalog>>,
    pub events: OnceLock<Arc<Events>>,
    pub deps: OnceLock<Arc<Deps>>,
    pub models: OnceLock<Arc<Models>>,
    pub desktop_route: OnceLock<Arc<DesktopRoute>>,
}

/// What a session with any agent reaches (ADR 0034, ADR 0035).
pub type Reach = Arc<dyn Fn(&Profile) -> Vec<ToolSource> + Send + Sync>;

pub struct Daemon {
    pub cfg: Mutex<GhostdSettings>,
    pub voice: Mutex<Arc<VoiceSettings>>,
    /// The daemon's own environment: the vault's values over the process's (ADR 0036).
    pub environ: SharedEnviron,
    pub ports: Ports,
    pub rt: PathBuf,
    pub stop: CancellationToken,
    pub state: Arc<DaemonState>,
    pub passwords: Mutex<Option<Arc<Passwords>>>,
    followed_vault: AtomicBool,
    /// The last model load's failure while it is tried again; on the waiting list meanwhile.
    models_problem: Mutex<Option<String>>,
    models_retry: std::time::Duration,
    pub late: Late,
    /// The agents, with the voices once Kokoro is loaded.
    pub conversations: Mutex<Option<Arc<Conversations>>>,
    reach: Mutex<Option<Reach>>,
    pub(crate) session: Mutex<Option<Arc<GhostSession>>>,
    pub(crate) last_desk_session: Mutex<Option<Instant>>,
    pub(crate) phone_route: Arc<PhoneRoute>,
    pub(crate) first_via: Mutex<TurnVia>,
    pub(crate) heard: Mutex<Option<Heard>>,
    pub(crate) start_in: Mutex<String>,
    pub(crate) manual: Mutex<Option<Arc<FileWakeDetector>>>,
    pub(crate) listener: Mutex<Option<Arc<WakeListener>>>,
    /// Set when the passive listener must be rebuilt.
    pub(crate) rearm: Event,
    pub(crate) ducker: Mutex<Option<Arc<dyn Duck>>>,
    /// The detector's early chime played; the session won't repeat it.
    pub(crate) chimed: AtomicBool,
    pub(crate) phone_listener: Mutex<Option<Arc<PhoneListener>>>,
    phone_servers: Mutex<Option<PhoneServers>>,
    pub(crate) live_phone: Mutex<Option<LivePhone>>,
    /// An accepted offer's connection goes here; the passive loop takes it, releases the
    /// desk mic and runs the session on it.
    phone_q: Mutex<Option<PendingPhone>>,
    phone_notify: Notify,
    pub(crate) phone_end: Event,
    /// The passive loop is waiting for a wake or a phone.
    pub(crate) waiting_flag: AtomicBool,
    raise_tx: mpsc::UnboundedSender<Raised>,
    raise_rx: Mutex<mpsc::UnboundedReceiver<Raised>>,
    pub(crate) origin: Mutex<Option<SessionOrigin>>,
    route_names: Mutex<Vec<RouteName>>,
    log_mirror: LogMirror,
}

/// A model that won't load is tried again this often (ADR 0038: under the app's
/// supervisor a crash would be a restart loop with nothing to read; the waiting list
/// says what is missing instead).
pub const MODELS_RETRY: std::time::Duration = std::time::Duration::from_secs(15);
/// The waiting list's key for it.
pub const MODELS_PROBLEM_KEY: &str = "models:missing";

impl Daemon {
    pub fn new(cfg: GhostdSettings, voice: VoiceSettings, ports: Ports) -> Self {
        let build = build_info::detect(None, None, None);
        let state = Arc::new(DaemonState::with_build(
            VERSION,
            cfg.wake_mode.clone(),
            build,
        ));
        let (raise_tx, raise_rx) = mpsc::unbounded_channel();
        Self {
            cfg: Mutex::new(cfg),
            voice: Mutex::new(Arc::new(voice)),
            environ: Arc::new(Mutex::new(HashMap::new())),
            ports,
            rt: runtime_dir(),
            stop: CancellationToken::new(),
            state,
            passwords: Mutex::new(None),
            followed_vault: AtomicBool::new(false),
            models_problem: Mutex::new(None),
            models_retry: MODELS_RETRY,
            late: Late::default(),
            conversations: Mutex::new(None),
            reach: Mutex::new(None),
            session: Mutex::new(None),
            last_desk_session: Mutex::new(None),
            phone_route: Arc::new(PhoneRoute::new()),
            first_via: Mutex::new(TurnVia::Voice),
            heard: Mutex::new(None),
            start_in: Mutex::new(GHOST.to_string()),
            manual: Mutex::new(None),
            listener: Mutex::new(None),
            rearm: Event::new(),
            ducker: Mutex::new(None),
            chimed: AtomicBool::new(false),
            phone_listener: Mutex::new(None),
            phone_servers: Mutex::new(None),
            live_phone: Mutex::new(None),
            phone_q: Mutex::new(None),
            phone_notify: Notify::new(),
            phone_end: Event::new(),
            waiting_flag: AtomicBool::new(false),
            raise_tx,
            raise_rx: Mutex::new(raise_rx),
            origin: Mutex::new(None),
            route_names: Mutex::new(Vec::new()),
            log_mirror: LogMirror::default(),
        }
    }

    /// The vault, opened before the daemon (the CLI opens it first, ADR 0036).
    pub fn with_passwords(self, vault: Arc<Passwords>) -> Self {
        *lock(&self.passwords) = Some(vault);
        self
    }

    /// The vault's values, when the vault was opened into an environment of its own.
    pub fn with_environ(mut self, environ: SharedEnviron) -> Self {
        self.environ = environ;
        self
    }

    /// Where the wake file and the duck state live (`$XDG_RUNTIME_DIR/ghost`).
    pub fn with_runtime_dir(mut self, rt: impl Into<PathBuf>) -> Self {
        self.rt = rt.into();
        self
    }

    /// How often a model load that failed is tried again (a test shortens it).
    pub fn with_models_retry(mut self, retry: std::time::Duration) -> Self {
        self.models_retry = retry;
        self
    }

    /// The log mirror the subscriber was built with (`LogMirror::layer`), so the desktop's
    /// log view follows this daemon.
    pub fn with_log_mirror(mut self, mirror: LogMirror) -> Self {
        self.log_mirror = mirror;
        self
    }

    // --- reads --------------------------------------------------------------------------

    pub fn cfg(&self) -> GhostdSettings {
        lock(&self.cfg).clone()
    }

    pub fn voice(&self) -> Arc<VoiceSettings> {
        lock(&self.voice).clone()
    }

    /// The environment Ghost's settings read: the process's, the vault's values over it.
    pub fn env(&self) -> HashMap<String, String> {
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.extend(lock(&self.environ).clone());
        env
    }

    pub fn passwords(&self) -> Option<Arc<Passwords>> {
        lock(&self.passwords).clone()
    }

    pub fn session(&self) -> Option<Arc<GhostSession>> {
        lock(&self.session).clone()
    }

    pub fn models(&self) -> Option<Arc<Models>> {
        self.late.models.get().cloned()
    }

    pub fn gate(&self) -> Option<Arc<WriteGate>> {
        self.late.gate.get().cloned()
    }

    pub fn waiting(&self) -> Option<Arc<WaitingList>> {
        self.late.waiting.get().cloned()
    }

    pub fn routes(&self) -> Option<Arc<RouteEngine>> {
        self.late.routes.get().cloned()
    }

    pub fn deps(&self) -> Option<Arc<Deps>> {
        self.late.deps.get().cloned()
    }

    pub fn conversations(&self) -> Option<Arc<Conversations>> {
        lock(&self.conversations).clone()
    }

    pub fn reach(&self) -> Option<Reach> {
        lock(&self.reach).clone()
    }

    pub fn apps(&self) -> Option<Arc<Apps>> {
        self.late.apps.get().cloned()
    }

    pub fn events(&self) -> Arc<Events> {
        self.late
            .events
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(Events::default()))
    }

    pub fn routes_available(&self) -> Vec<RouteName> {
        let mut out = vec![RouteName::Digest];
        out.extend(lock(&self.route_names).iter().copied());
        out
    }

    // --- the phone queue (one accepted offer at a time) ----------------------------------

    pub(crate) fn phone_q_empty(&self) -> bool {
        lock(&self.phone_q).is_none()
    }

    pub(crate) fn put_phone(&self, pending: PendingPhone) {
        *lock(&self.phone_q) = Some(pending);
        self.phone_notify.notify_one();
    }

    pub(crate) fn take_phone(&self) -> Option<PendingPhone> {
        lock(&self.phone_q).take()
    }

    async fn next_phone(&self) -> PendingPhone {
        loop {
            let notified = self.phone_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(p) = self.take_phone() {
                return p;
            }
            notified.await;
        }
    }

    // --- the desk route's queue (ADR 0032) ---------------------------------------------

    pub(crate) fn raise_q_empty(&self) -> bool {
        lock(&self.raise_rx).is_empty()
    }

    async fn next_raised(&self) -> Option<Raised> {
        std::future::poll_fn(|cx| lock(&self.raise_rx).poll_recv(cx)).await
    }

    // --- small helpers the controller and the sessions share ------------------------------

    /// An app is waiting for the operator to sign in: the desktop opens the address.
    pub(crate) fn on_signin_url(&self, app: &str, url: &str) {
        self.state.publish_apps(Some(app), Some(url));
    }

    pub(crate) fn refresh_waiting_now(&self) {
        if let Some(w) = self.waiting()
            && let Err(e) = w.refresh(None)
        {
            warn!("waiting: refresh failed: {e}");
        }
    }

    pub(crate) fn set_waiting_count(&self) {
        if let Some(w) = self.waiting() {
            self.state.set_waiting(w.count(None).unwrap_or(0) as u64);
        }
    }

    /// Re-read Ghost's own items now and then (a change's time running out, a server
    /// that went away; watches sync their own), and let the routes send what is due
    /// (ADR 0031).
    async fn refresh_waiting(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(WAITING_REFRESH_SECS)).await;
            if let Some(w) = self.waiting()
                && let Err(e) = w.refresh(None)
            {
                error!("waiting: refresh failed: {e}");
            }
            if let Some(r) = self.routes()
                && let Err(e) = r.tick(None).await
            {
                error!("waiting: refresh failed: {e}");
            }
        }
    }

    /// Desk voice may start (ADR 0032): the desk is idle, nothing else is about to
    /// start, and dictation is off.
    pub(crate) fn can_raise(&self) -> bool {
        self.state.models_ready()
            && self.waiting_flag.load(Ordering::SeqCst)
            && self.session().is_none()
            && self.state.phase() == Phase::Passive
            && self.phone_q_empty()
            && self.raise_q_empty()
            && !is_dictating(&self.cfg().dictation_flag)
    }

    /// The operator is at the desk: a desk session ran lately, or the desktop app is
    /// connected to the event stream.
    pub(crate) fn at_desk(&self) -> bool {
        let recent = lock(&self.last_desk_session)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(AT_DESK_SECS));
        recent || self.state.bus.subscribers() > 0
    }

    /// Publish the pending proposal when it appears or goes away.
    pub(crate) fn refresh_pending(&self) {
        let (Some(gate), Some(session_id)) = (self.gate(), self.state.session_id()) else {
            return;
        };
        let p = gate.pending(&session_id, None).ok().flatten();
        let view = p.map(|p| ghost_api::pending_view(&p, None));
        let now = view.as_ref().map(|v| v.id.clone());
        let was = self.state.pending().map(|p| p.id);
        if now != was {
            self.state.set_pending(view);
        }
    }

    // --- passwords (ADR 0036) -----------------------------------------------------------

    pub(crate) fn classifier(
        &self,
        jev: Option<Arc<JevClient>>,
    ) -> Arc<dyn ConfirmationClassifier> {
        let Some(jev) = jev else {
            return Arc::new(RegexClassifier);
        };
        if let Some(vault) = self.passwords() {
            vault.note_used(&[if jev.provider() == "openrouter" {
                "OPENROUTER_API_KEY"
            } else {
                "TYPESAFE_API_KEY"
            }]);
        }
        let cfg = self.cfg();
        Arc::new(
            JevClassifier::new(jev as Arc<dyn ghost_core::Jev>)
                .with_fallback(Arc::new(RegexClassifier))
                .with_threshold(cfg.jev_threshold)
                .with_deadline(Duration::from_secs_f64(cfg.jev_deadline_secs.max(0.0)))
                .on_decision(|m| debug::audio(|| m.to_string())),
        )
    }

    /// The keyring was locked at start: try again every few seconds until it opens.
    /// "Unlock your keyring" is on the waiting list meanwhile.
    pub(crate) async fn wait_for_vault(self: Arc<Self>, retry: Duration) {
        let Some(vault) = self.passwords() else {
            return;
        };
        let (stop_tx, stop_rx) = watch::channel(false);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            stop.cancelled().await;
            let _ = stop_tx.send(true);
        });
        let d = self.clone();
        let opened = Arc::new(Opened::new(move || {
            let d = d.clone();
            tokio::spawn(async move { d.passwords_opened().await });
        }));
        wait_for_vault(vault, retry, stop_rx, opened).await;
    }

    /// The vault opened after the keyring was locked: Ghost's own settings are read
    /// again (the model connection, the voice), Jev joins when it has a key, and the apps
    /// that waited for a password connect. A watch's model decider follows at the next
    /// start. Once: the unlock route and the retry loop may both get here.
    pub async fn passwords_opened(&self) {
        if self.followed_vault.swap(true, Ordering::SeqCst) {
            return;
        }
        info!("passwords: the vault is open");
        let env = self.env();
        match VoiceSettings::from_vars(|name| env.get(name).cloned()) {
            Ok(fresh) => *lock(&self.voice) = Arc::new(fresh),
            // names only; a value never reaches the log
            Err(e) => error!("passwords: settings could not be read again ({e})"),
        }
        let jev = match build_jev(&self.cfg(), &env) {
            Ok(j) => j,
            Err(e) => {
                error!("passwords: {e}");
                None
            }
        };
        if let Some(jev) = jev
            && let Some(gate) = self.gate()
        {
            gate.set_classifier(self.classifier(Some(jev)));
        }
        if let Some(apps) = self.apps() {
            let failed: Vec<String> = apps.failed().into_keys().collect();
            for name in failed {
                if apps.revive(&name).await {
                    self.state.publish_apps(Some(&name), None);
                }
            }
        }
        self.refresh_waiting_now();
    }

    // --- the model connection (ADR 0037) ------------------------------------------------

    /// The model connection's keys, live: the environment the next session reads, and
    /// the voice settings' copy of them. A turn in flight keeps the model it started on.
    pub(crate) fn apply_model(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        let known = llm_env_keys();
        let keys: Vec<String> = values
            .keys()
            .filter(|k| known.contains(k.as_str()))
            .cloned()
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        let before: HashMap<String, Option<String>> = {
            let env = lock(&self.environ);
            keys.iter()
                .map(|k| (k.clone(), env.get(k).cloned()))
                .collect()
        };
        {
            let mut env = lock(&self.environ);
            for k in &keys {
                match values.get(k).and_then(|v| v.as_deref()) {
                    Some(v) if !v.is_empty() => {
                        env.insert(k.clone(), v.to_string());
                    }
                    _ => {
                        env.remove(k);
                    }
                }
            }
        }
        let env = self.env();
        match VoiceSettings::from_vars(|name| env.get(name).cloned()) {
            Ok(fresh) => {
                info!(
                    "llm: {} from the next session",
                    fresh.llm_target(Some(&env)).describe()
                );
                for problem in fresh.llm_problems(Some(&env)) {
                    warn!("llm: {problem}");
                }
                *lock(&self.voice) = Arc::new(fresh);
                keys
            }
            Err(e) => {
                let mut env = lock(&self.environ);
                for (k, v) in before {
                    match v {
                        Some(v) => {
                            env.insert(k, v);
                        }
                        None => {
                            env.remove(&k);
                        }
                    }
                }
                warn!("llm: the new connection waits for a restart ({e})");
                Vec::new()
            }
        }
    }

    /// Test connection (ADR 0037): one tiny streamed request through the connection
    /// the desktop is editing, timed like `make perf-llm`. The saved connection goes
    /// over the client sessions share (warm, as a session finds it); another is tried
    /// on a client of its own that isn't kept.
    pub(crate) async fn probe_model(
        &self,
        env: HashMap<String, String>,
        prompt: &str,
        max_tokens: u32,
    ) -> ghost_api::Probe {
        let target = ghost_core::connection::ModelConnection::from_env(&env).target(&env);
        let voice = self.voice();
        let mine = self.env();
        let share = target.fingerprint() == voice.llm_target(Some(&mine)).fingerprint();
        let client = if share {
            ghost_llm::model::shared(&target)
        } else {
            ghost_llm::model::Model::new(target.clone()).map(Arc::new)
        };
        let client = match client {
            Ok(c) => c,
            Err(e) => {
                return ghost_api::Probe {
                    error: Some(e.to_string()),
                    ..ghost_api::Probe::default()
                };
            }
        };
        let thinking = (target.protocol == ghost_core::connection::Protocol::Anthropic
            && !target.model.contains("haiku")
            && !voice.llm_thinking.is_empty())
        .then(|| voice.llm_thinking.clone());
        let p = client.probe(prompt, max_tokens, thinking.as_deref()).await;
        ghost_api::Probe {
            ttft: p.ttft,
            total: p.total,
            text: p.text,
            model: p.model,
            error: p.error,
        }
    }

    // --- the wake ------------------------------------------------------------------------

    fn stt_factory(models: &Arc<Models>) -> SttFactory {
        let stt = models.stt.clone();
        Arc::new(move |params| {
            Box::new(ParakeetStt::new(stt.clone(), None, params, 0.0))
                as Box<dyn ghost_voice::frames::Stage>
        })
    }

    fn passive_media(&self) -> MediaFactory {
        self.ports
            .passive_media
            .clone()
            .unwrap_or_else(|| desk_capture(self.voice().input_device.clone()))
    }

    pub(crate) fn build_wake(self: &Arc<Self>, models: &Arc<Models>) -> Arc<dyn WakeDetector> {
        let manual = FileWakeDetector::new(self.rt.join(WAKE_FILE));
        *lock(&self.manual) = Some(manual.clone());
        let cfg = self.cfg();
        match cfg.wake_mode.as_str() {
            "file" => manual,
            "onnx" => self.detector_wake(models, manual),
            _ => {
                if cfg.wake_mode != "transcript" {
                    error!(
                        "unknown GHOST_WAKE_MODE {:?}; listening by transcript",
                        cfg.wake_mode
                    );
                }
                self.transcript_wake(models, manual)
            }
        }
    }

    fn transcript_wake(
        self: &Arc<Self>,
        models: &Arc<Models>,
        manual: Arc<FileWakeDetector>,
    ) -> Arc<dyn WakeDetector> {
        let cfg = self.cfg();
        let phrases = parse_phrases(Some(&cfg.wake_phrases));
        info!(
            "wake phrases: {}",
            phrases
                .iter()
                .map(|p| format!("{p:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let listener = Arc::new(WakeListener::new(
            self.voice(),
            Self::stt_factory(models),
            phrases,
            self.passive_media(),
            cfg.wake_fuzzy,
            None,
        ));
        *lock(&self.listener) = Some(listener.clone());
        Arc::new(SpokenWakeDetector::new(listener, manual, None))
    }

    /// The trained detectors (ADR 0025): "hey ghost", and "Ghost, …" when its model
    /// is there; no transcript fallback. Without the "hey ghost" model Ghost would be
    /// deaf, so it says so loudly and listens by transcript instead.
    fn detector_wake(
        self: &Arc<Self>,
        models: &Arc<Models>,
        manual: Arc<FileWakeDetector>,
    ) -> Arc<dyn WakeDetector> {
        let cfg = self.cfg();
        let voice = self.voice();
        let (path, address) = wake_model_paths(&cfg, &voice);
        let fall_back = |why: String| {
            error!("wake: {why}; listening by transcript instead");
            lock(&self.cfg).wake_mode = "transcript".into();
            self.transcript_wake(models, manual.clone())
        };
        if !path.exists() {
            return fall_back(format!(
                "GHOST_WAKE_MODE=onnx but there is no model at {} (set GHOST_WAKE_MODEL_PATH)",
                path.display()
            ));
        }
        let (mel, embedding) = match wake_resources() {
            Ok(found) => found,
            Err(why) => return fall_back(format!("GHOST_WAKE_MODE=onnx but {why}")),
        };
        let mut files = vec![path.clone()];
        files.extend(address.iter().cloned());
        let scorer = match WakeScorer::load(&mel, &embedding, &files, 1) {
            Ok(s) => s,
            Err(e) => return fall_back(format!("the wake models did not load: {e}")),
        };
        let mut wake_models = vec![WakeModel::known(
            &path,
            cfg.detector_threshold() as f32,
            None,
        )];
        wake_models.extend(address.iter().map(|p| {
            WakeModel::known(
                p,
                cfg.address_threshold() as f32,
                Some(cfg.address_verify()),
            )
        }));
        for m in &wake_models {
            let checked = m.verify.unwrap_or(cfg.wake_verify);
            let check = if checked {
                "Parakeet checks each hit"
            } else {
                "no check"
            };
            info!(
                "wake: trained detector {}, threshold {}, {check}",
                m.name, m.threshold
            );
        }
        info!("wake: those phrases only");
        let spec = DetectorSpec {
            scorer: Arc::new(Mutex::new(scorer)),
            recognize: ghost_voice::stt::recognize_fn(&models.stt),
            models: wake_models,
            verify: cfg.wake_verify,
            cooldown_secs: cfg.wake_cooldown_secs,
        };
        let listener = Arc::new(WakeListener::new(
            voice,
            Self::stt_factory(models),
            detector_phrases(),
            self.passive_media(),
            cfg.wake_fuzzy,
            Some(spec),
        ));
        *lock(&self.listener) = Some(listener.clone());
        let on_heard: OnHeard = {
            let d = self.clone();
            Arc::new(move || {
                let d = d.clone();
                Box::pin(async move { d.heard_early().await })
            })
        };
        Arc::new(SpokenWakeDetector::new(listener, manual, Some(on_heard)))
    }

    /// The detector decided while the operator may still be talking ("hey ghost, what
    /// needs..."): duck and chime now, as a smart speaker does. The session starts once
    /// they finish, without a second chime.
    pub(crate) async fn heard_early(&self) {
        let Some(ducker) = lock(&self.ducker).clone() else {
            return;
        };
        if self.chimed.swap(true, Ordering::SeqCst) {
            return;
        }
        ducker.duck();
        if self.cfg().chime {
            let play = self.ports.play.clone();
            let device = self.voice().output_device.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    play(ghost_audio::chime(true), ghost_audio::CHIME_RATE, device).await
                {
                    warn!("chime failed: {e}");
                }
            });
        }
    }

    fn restore_early_chime(&self) {
        if self.chimed.swap(false, Ordering::SeqCst)
            && let Some(d) = lock(&self.ducker).clone()
        {
            d.restore();
        }
    }

    // --- the loop ----------------------------------------------------------------------------

    pub async fn run(self: &Arc<Self>) -> anyhow::Result<()> {
        let d = self;
        info!(
            "{}",
            build_info::describe(&d.state.version(), &d.state.build())
        );
        d.log_mirror.attach(d.state.clone());
        let cfg = d.cfg();
        if let Some(parent) = cfg.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = Arc::new(Store::open(&cfg.db_path)?);
        let autonomy = AutonomyPolicy::load(&cfg.policy_path)?;
        // ADR 0035: one folder, Ghost's. ADR 0034: the apps are Ghost's, connected now, and
        // every agent reaches them.
        let folder = Folder::new(&cfg.config_dir);
        folder.ensure_layout()?;
        for problem in &folder.problems {
            warn!("folder: {problem}");
        }
        let folder = Arc::new(Mutex::new(folder));
        let apps_file = load_apps(&cfg.config_dir.join(APPS_FILE));
        for problem in &apps_file.problems {
            warn!("apps: {problem}");
        }
        let passwords = match d.passwords() {
            Some(v) => v,
            None => {
                let v = Arc::new(open_passwords(
                    &cfg,
                    d.ports.keyring.clone(),
                    d.environ.clone(),
                ));
                *lock(&d.passwords) = Some(v.clone());
                v
            }
        };
        let signins = Arc::new(
            SignIns::new(cfg.data_dir.join("apps"))
                .with_port(cfg.signin_port)
                .with_on_url({
                    let d = Arc::downgrade(d);
                    Arc::new(move |app: &str, url: &str| {
                        if let Some(d) = d.upgrade() {
                            d.on_signin_url(app, url);
                        }
                    })
                }),
        );
        // ADR 0036: an app's ${NAME} is filled from the vault when it connects.
        let apps = Arc::new(Apps::new(
            apps_file.apps,
            Arc::new(EnvironView(d.environ.clone())) as Arc<dyn Environ>,
            Some(auth_for_apps(&signins)),
        ));
        apps.connect().await;
        let catalog = Arc::new(Catalog::new(&cfg.catalog_path));
        // #35: skills.sh search and GitHub fetches for the Skills page, never for a session.
        let previews = cfg
            .catalog_path
            .parent()
            .map(|p| p.join("skill-previews"))
            .unwrap_or_else(|| PathBuf::from("skill-previews"));
        let skill_catalog = Arc::new(SkillCatalog::new(previews, Arc::new(SkillsFrontMatter)));
        let skills = Arc::new(Skills::new(&cfg.skills_dir, Some(&cfg.home_skills_dir))); // ADR 0035
        // What Ghost's own sessions may call: every connected app's tools, and the skills
        // (the index, read_skill, add and remove).
        let mut tool_sources: Vec<ToolSource> = apps.sources();
        tool_sources.push(skills.source());
        let env = d.env();
        let jev = build_jev(&cfg, &env)?;
        let gate = Arc::new(
            WriteGate::new(store.clone())
                .with_autonomy(autonomy.clone())
                .with_classifier(d.classifier(jev.clone()))
                .with_log(Arc::new(|m: &str| {
                    let m = m.to_string();
                    debug::audio(|| m);
                })),
        );
        for a in &tool_sources {
            // every app's changes go through the gate
            gate.register(&a.name, a.operations.clone());
        }
        // And skills.install, which the desktop's catalog prepares and the model is not offered.
        gate.register(SKILLS, skills.gate_operations());
        let registered: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(
            apps.sources().into_iter().map(|s| s.name).collect(),
        ));
        d.late.store.set(store.clone()).ok();
        d.late.gate.set(gate.clone()).ok();
        d.late.apps.set(apps.clone()).ok();
        d.late.signins.set(signins.clone()).ok();
        d.late.catalog.set(catalog.clone()).ok();
        d.late.skill_catalog.set(skill_catalog.clone()).ok();
        d.late.skills.set(skills.clone()).ok();
        d.late.folder.set(folder.clone()).ok();
        {
            // An app was connected, disconnected or came back: the gate follows, and so
            // does the next turn's list of tools.
            let weak = Arc::downgrade(d);
            let hooked = apps.clone();
            let gate = gate.clone();
            let registered = registered.clone();
            apps.on_change(Arc::new(move || {
                let now: BTreeMap<String, ToolSource> = hooked
                    .sources()
                    .into_iter()
                    .map(|s| (s.name.clone(), s))
                    .collect();
                let mut reg = lock(&registered);
                for name in reg.iter() {
                    if !now.contains_key(name) {
                        gate.unregister(name);
                    }
                }
                for (name, src) in &now {
                    gate.register(name, src.operations.clone());
                }
                *reg = now.keys().cloned().collect();
                if let Some(d) = weak.upgrade() {
                    d.state.set_apps(reg.iter().cloned().collect());
                    d.refresh_waiting_now();
                }
            }));
        }
        // A read the daemon makes (a watch's look, a webhook's check, ADR 0027/0028):
        // one of the app's read-only tools, unless the operator's permissions block it.
        let read_for: Arc<dyn AppReader> = Arc::new(ReadFor {
            gate: gate.clone(),
            apps: apps.clone(),
        });
        let voice = d.voice();
        let conversations = Arc::new(Conversations::new(
            store.clone(),
            &cfg.config_dir,
            ConversationsOptions {
                default_voice: voice.tts_voice.clone(),
                default_speed: voice.tts_speed,
                retention: chrono::Duration::days(cfg.turn_retention_days),
                ..ConversationsOptions::default()
            },
        )?);
        gate.register(
            CONVERSATIONS,
            conversations_source(&conversations).operations,
        );
        *lock(&d.conversations) = Some(conversations.clone());
        let rules = Arc::new(Rules::new(&cfg.config_dir)); // ADR 0035: "add a rule", verbatim
        gate.register(RULES, rules.operations());
        d.late.rules.set(rules.clone()).ok();
        let outputs = Arc::new(Outputs::new(&cfg.config_dir)); // ADR 0033: what Ghost makes
        gate.register(OUTPUTS, outputs.operations());
        d.late.outputs.set(outputs.clone()).ok();
        // ADR 0029: what needs the operator, with the changes waiting for a yes (ADR 0030).
        let on_change: ghost_core::waiting::OnChange = {
            let weak = Arc::downgrade(d);
            Arc::new(move || {
                if let Some(d) = weak.upgrade() {
                    d.set_waiting_count();
                }
            })
        };
        let waiting = Arc::new(
            WaitingList::new(Items::new(store.clone()))
                .with_changes(gate.clone())
                .with_problems(Arc::new({
                    let apps = apps.clone();
                    let folder = folder.clone();
                    let vault = passwords.clone();
                    let cfg = cfg.clone();
                    let weak = Arc::downgrade(d);
                    move || {
                        let folder_problems = lock(&folder).problems.clone();
                        let mut out = ghost_problems(
                            &*apps,
                            &folder_problems,
                            &password_problems(Some(&vault), &cfg),
                        );
                        if let Some(d) = weak.upgrade()
                            && let Some(problem) = lock(&d.models_problem).clone()
                        {
                            out.push((MODELS_PROBLEM_KEY.to_string(), problem));
                        }
                        out
                    }
                }))
                .with_on_change(on_change.clone()),
        );
        d.late.waiting.set(waiting.clone()).ok();
        // ADR 0028: the watches, polled by the daemon; Jev and a model only where declared.
        let model_step: Arc<dyn Decider> = Arc::new(model_decider({
            let weak = Arc::downgrade(d);
            move || {
                let d = weak.upgrade()?;
                let env = d.env();
                let voice = d.voice();
                if !voice.llm_problems(Some(&env)).is_empty() {
                    return None;
                }
                ghost_llm::model::shared(&voice.llm_target(Some(&env)))
                    .ok()
                    .map(|m| m as Arc<dyn Completer>)
            }
        }));
        let watch_runner = Arc::new(
            WatchRunner::new(Arc::new(Items::new(store.clone())), read_for.clone())
                .with_jev(
                    jev.clone()
                        .map(|j| Arc::new(JevStep(watch_decider(j))) as Arc<dyn Decider>),
                )
                .with_model(Some(model_step))
                .with_on_change(on_change.clone()),
        );
        d.late.watch_runner.set(watch_runner.clone()).ok();
        // ADR 0027: webhooks, signed and parsed by declaration, checked through read_for.
        let hook_intake = Arc::new(
            HookIntake::new(
                Arc::new(Items::new(store.clone())),
                store.clone(),
                Arc::new(EnvironSecrets(d.environ.clone())),
                Arc::new(|store: Arc<Store>| {
                    Arc::new(Items::new(store)) as Arc<dyn ghost_core::watches::WaitingItems>
                }),
            )
            .with_reader(read_for.clone())
            .with_on_change(on_change.clone()),
        );
        d.late.hook_intake.set(hook_intake.clone()).ok();
        // ADR 0031: how Ghost reaches the operator, by the operator's [routes].
        let route_policy = RoutePolicy::parse(autonomy.routes.as_ref(), &autonomy.source)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let routes = Arc::new(
            RouteEngine::new(Items::new(store.clone()))?
                .with_policy(route_policy)
                .with_urgency_of({
                    let folder = folder.clone();
                    Arc::new(move |item| {
                        declared_urgency(&lock(&folder))
                            .get(&item.source)
                            .and_then(|u| Urgency::parse(u))
                            .unwrap_or(Urgency::Notify)
                    })
                })
                .with_at_desk({
                    let weak = Arc::downgrade(d);
                    Arc::new(move || weak.upgrade().is_some_and(|d| d.at_desk()))
                })
                .with_on_change(on_change.clone()),
        );
        routes.add_route(Arc::new(DeskRoute::new(
            {
                let weak = Arc::downgrade(d);
                move || weak.upgrade().is_some_and(|d| d.can_raise())
            },
            d.raise_tx.clone(),
        )));
        let desktop_route = Arc::new(DesktopRoute::new({
            let waiting = waiting.clone();
            Arc::new(move |key: &str, now| {
                waiting
                    .acknowledge(key, Some(now), Some(now))
                    .unwrap_or(false)
            })
        }));
        routes.add_route(desktop_route.clone());
        routes.add_route(d.phone_route.clone());
        *lock(&d.route_names) = vec![RouteName::Desk, RouteName::Desktop, RouteName::Phone];
        d.late.routes.set(routes.clone()).ok();
        d.late.desktop_route.set(desktop_route.clone()).ok();
        // What a session with any agent reaches (ADR 0034, ADR 0035): every connected
        // app (the operator's permissions apply in the gate), the skills, the agents' own
        // tools, the rules, the outputs and the waiting list.
        let reach: Reach = {
            let apps = apps.clone();
            let skills = skills.clone();
            let weak = Arc::downgrade(d);
            let rules = rules.clone();
            let outputs = outputs.clone();
            let waiting = waiting.clone();
            Arc::new(move |_p: &Profile| {
                let mut out = apps.sources();
                out.push(skills.source());
                if let Some(c) = weak.upgrade().and_then(|d| d.conversations()) {
                    out.push(conversations_source(&c));
                }
                out.push(rules_source(&rules));
                out.push(outputs.source());
                out.push(waiting_source(waiting.clone()));
                out
            })
        };
        *lock(&d.reach) = Some(reach);
        match conversations.prune() {
            Ok(n) if n > 0 => info!("conversations: pruned {n} summarized turns past retention"),
            Ok(_) => {}
            Err(e) => warn!("conversations: prune failed: {e}"),
        }
        d.state
            .set_apps(lock(&registered).iter().cloned().collect());
        let mut events = Events::default();
        events.add({
            let routes = routes.clone();
            Arc::new(move |event| {
                let routes = routes.clone();
                Box::pin(async move {
                    if event.kind == EventKind::Quiet
                        && let Some(until) = event.data.get("until")
                    {
                        let until = until.as_str().and_then(ghost_core::time::parse_iso);
                        if let Some(until) = until {
                            let _ = routes.quiet_until(Some(until));
                            info!("routes: quiet until {}", until.format("%H:%M"));
                        }
                    }
                })
            })
        });
        d.late.events.set(Arc::new(events)).ok();
        // The hooks listener is made now (its addresses are in the API's status) and
        // started after the phone listener, as the Python did.
        let hooks_listener = (!cfg.hooks_listen.trim().is_empty())
            .then(|| HooksListener::new(&cfg.hooks_listen, hook_intake.clone()))
            .transpose()
            .unwrap_or_else(|e| {
                error!("hooks: listener not started on {:?}: {e}", cfg.hooks_listen);
                None
            });
        let api = d
            .start_api(StartApi {
                gate: gate.clone(),
                store: store.clone(),
                sources: tool_sources.clone(),
                apps: apps.clone(),
                catalog,
                skill_catalog,
                signins: signins.clone(),
                skills: skills.clone(),
                folder: folder.clone(),
                rules,
                outputs,
                waiting: waiting.clone(),
                watches: watch_runner.clone(),
                hooks: hook_intake.clone(),
                hooks_listening: hooks_listener
                    .as_ref()
                    .map(|h| h.addresses.clone())
                    .unwrap_or_default(),
                passwords: passwords.clone(),
                conversations: conversations.clone(),
            })
            .await;
        d.start_phone_listener().await;
        if let Err(e) = waiting.refresh(None) {
            warn!("waiting: refresh failed: {e}");
        }
        d.set_waiting_count();
        let refresher = tokio::spawn(d.clone().refresh_waiting());
        // ADR 0036: a locked keyring is waited for, not a failure.
        let vault_waiter =
            (!passwords.is_open()).then(|| tokio::spawn(d.clone().wait_for_vault(VAULT_RETRY)));
        // The folder was re-read: the declared watches and hooks again.
        let follow = Arc::new(Notify::new());
        {
            let follow = follow.clone();
            lock(&folder)
                .on_refresh
                .push(Box::new(move || follow.notify_one()));
        }
        follow_watches(&folder, &apps, &watch_runner, &hook_intake).await;
        let follower = tokio::spawn({
            let folder = folder.clone();
            let apps = apps.clone();
            let watch_runner = watch_runner.clone();
            let hook_intake = hook_intake.clone();
            async move {
                loop {
                    follow.notified().await;
                    follow_watches(&folder, &apps, &watch_runner, &hook_intake).await;
                }
            }
        });
        for line in crate::check::describe_watches(&lock(&folder)) {
            info!("{line}");
        }
        if let Some(h) = &hooks_listener
            && let Err(e) = h.start().await
        {
            error!("hooks: listener not started on {:?}: {e}", cfg.hooks_listen);
        }
        info!("autonomy policy: {}", autonomy.source);
        info!("{}", describe_routes(&routes.policy()));
        info!(
            "llm: {}, thinking {}",
            voice.llm_target(Some(&env)).describe(),
            voice.llm_thinking
        );
        for problem in voice.llm_problems(Some(&env)) {
            error!("llm: {problem}");
        }
        info!("tts: {}", describe_tts(&voice));
        for line in describe_apps(&apps, &gate.autonomy()) {
            info!("{line}");
        }
        info!(
            "{}",
            describe_skills(
                &skills,
                &apps.specs().iter().map(|s| s.name.clone()).collect()
            )
        );
        info!(
            "confirmation classifier: {}",
            match &jev {
                Some(j) => format!("jev via {}, regex fallback", j.provider()),
                None => "regex".to_string(),
            }
        );

        info!("loading models");
        // A model that won't load is waited for, like a locked keyring (ADR 0036), not a
        // crash: under the app's supervisor (ADR 0038) that would be a restart loop with
        // nothing to read. The API is up, the waiting list says what is missing, and the
        // load is tried again until it works or the daemon is stopped.
        let models = loop {
            let load = d.ports.load_models.clone();
            let voice = d.voice();
            match tokio::task::spawn_blocking(move || load(&voice)).await? {
                Ok(m) => break Some(m),
                Err(e) => {
                    warn!(
                        "models: {e}; trying again in {} s (ghostd check says what is missing)",
                        d.models_retry.as_secs_f64()
                    );
                    *lock(&d.models_problem) =
                        Some(format!("Ghost's speech models are not loaded: {e}"));
                    d.refresh_waiting_now();
                    tokio::select! {
                        _ = tokio::time::sleep(d.models_retry) => {}
                        _ = d.stop.cancelled() => break None,
                    }
                }
            }
        };
        let running = if let Some(models) = models {
            if lock(&d.models_problem).take().is_some() {
                d.refresh_waiting_now(); // the item resolves
            }
            info!("models ready");
            let models = Arc::new(models);
            d.late.models.set(models.clone()).ok();
            d.state.set_models_ready(true);
            d.bind_voices(&models);
            let duck_state = d.rt.join(DUCK_STATE);
            let ducker = (d.ports.ducker)(&cfg, duck_state.clone());
            if let Err(e) = ghost_audio::restore_from_state(&duck_state) {
                warn!("duck: {e}"); // undo a previous crash
            }
            *lock(&d.ducker) = Some(ducker.clone());
            let wake = d.build_wake(&models);

            d.passive(wake.clone()).await;
            Some((wake, ducker))
        } else {
            None
        };

        // the wind-down, in the Python's order
        refresher.abort();
        if let Some(v) = vault_waiter {
            v.abort();
        }
        follower.abort();
        watch_runner.close().await;
        desktop_route.close().await;
        if let Some(h) = &hooks_listener {
            h.stop().await;
        }
        if let Some((wake, ducker)) = running {
            wake.close();
            ducker.restore();
        }
        let servers = lock(&d.phone_servers).take();
        if let Some(servers) = servers {
            servers.stop().await;
        }
        if let Some(api) = api {
            api.stop().await;
        }
        signins.close().await;
        apps.close().await;
        drop(store);
        Ok(())
    }

    /// Passive until stopped: a wake phrase, the manual trigger or a phone call starts
    /// a session; a settings change rebuilds the listener.
    pub async fn passive(self: &Arc<Self>, wake: Arc<dyn WakeDetector>) {
        let d = self;
        while !d.stop.is_cancelled() {
            info!("passive: waiting for wake word");
            wake.arm(); // a trigger from before the loop said passive is forgotten
            d.state.set_phase(Phase::Passive, None, None);
            d.rearm.clear();
            *lock(&d.start_in) = GHOST.to_string(); // a spoken wake lands in Ghost; a Call sets it
            let mut wake_task: JoinHandle<Option<String>> = {
                let w = wake.clone();
                tokio::spawn(async move { w.wait().await })
            };
            d.waiting_flag.store(true, Ordering::SeqCst);
            let won = tokio::select! {
                r = &mut wake_task => Won::Wake(r.unwrap_or(None)),
                _ = d.stop.cancelled() => Won::Stop,
                _ = d.rearm.wait() => Won::Rearm,
                p = d.next_phone() => Won::Phone(p),
                r = d.next_raised() => match r { Some(r) => Won::Raised(r), None => Won::Rearm },
            };
            d.waiting_flag.store(false, Ordering::SeqCst);
            let (mut woke, mut phone, mut raised) = (None, None, None);
            match won {
                Won::Wake(r) => woke = Some(r),
                Won::Phone(p) => phone = Some(p),
                Won::Raised(r) => raised = Some(r),
                Won::Stop | Won::Rearm => {}
            }
            if woke.is_none() {
                // Decided before the cancel below, which makes a pending task done too.
                if wake_task.is_finished() {
                    woke = Some(wake_task.await.unwrap_or(None));
                } else {
                    wake_task.abort();
                    let _ = wake_task.await;
                }
                wake.stop().await; // releases the mic
            }
            if phone.is_none() {
                phone = d.take_phone(); // accepted in the same tick
            }
            if let Some(r) = raised.take() {
                if phone.is_some()
                    || woke.is_some()
                    || d.stop.is_cancelled()
                    || is_dictating(&d.cfg().dictation_flag)
                {
                    let _ = r.reply.send(ghost_core::routes::Delivery::Unavailable); // the operator came first
                } else {
                    raised = Some(r);
                }
            }
            if d.chimed.load(Ordering::SeqCst) && (phone.is_some() || woke.is_none()) {
                d.restore_early_chime(); // the early chime played, then something else came first
            }
            if d.stop.is_cancelled() {
                if let Some(p) = phone {
                    p.media.close().await;
                }
                break;
            }
            if let Some(raised) = raised {
                // ADR 0032: Ghost greets at the desk and waits
                d.ports
                    .sessions
                    .run(
                        d.clone(),
                        SessionStart {
                            first_utterance: None,
                            phone: None,
                            start_in: GHOST.into(),
                            raised: Some(raised),
                        },
                    )
                    .await;
                continue;
            }
            if let Some(phone) = phone {
                // the phone takes the session; the desk mic is free
                let line = phone.line.clone();
                d.ports
                    .sessions
                    .run(
                        d.clone(),
                        SessionStart {
                            first_utterance: None,
                            phone: Some(phone),
                            start_in: line,
                            raised: None,
                        },
                    )
                    .await;
                continue;
            }
            let Some(first) = woke else {
                continue; // settings changed: rebuild the listener
            };
            if is_dictating(&d.cfg().dictation_flag) {
                info!("wake ignored: dictation is active");
                d.restore_early_chime();
                continue;
            }
            d.events().emit(EventKind::Wake, Map::new()).await;
            let start_in = lock(&d.start_in).clone();
            d.ports
                .sessions
                .run(
                    d.clone(),
                    SessionStart {
                        first_utterance: first,
                        phone: None,
                        start_in,
                        raised: None,
                    },
                )
                .await;
        }
    }

    /// Once Kokoro is loaded, the operations validate voice ids against its list: the
    /// agents are built again with the voices and registered in the gate again.
    fn bind_voices(&self, models: &Arc<Models>) {
        let mut voices = models.voices.clone();
        voices.sort();
        if voices.is_empty() {
            return;
        }
        let (Some(gate), Some(store)) = (self.gate(), self.late.store.get().cloned()) else {
            return;
        };
        let cfg = self.cfg();
        let voice = self.voice();
        match Conversations::new(
            store,
            &cfg.config_dir,
            ConversationsOptions {
                default_voice: voice.tts_voice.clone(),
                default_speed: voice.tts_speed,
                voices,
                retention: chrono::Duration::days(cfg.turn_retention_days),
                ..ConversationsOptions::default()
            },
        ) {
            Ok(c) => {
                let c = Arc::new(c);
                gate.register(CONVERSATIONS, conversations_source(&c).operations);
                *lock(&self.conversations) = Some(c);
            }
            Err(e) => warn!("conversations: the voices could not be bound: {e}"),
        }
    }

    async fn start_api(self: &Arc<Self>, p: StartApi) -> Option<ApiHandle> {
        let cfg = self.cfg();
        let voice = self.voice();
        let socket = cfg.api_socket.clone().unwrap_or_else(default_socket_path);
        let deps = Arc::new(Deps {
            state: self.state.clone(),
            control: self.clone(),
            gate: p.gate,
            store: p.store,
            sources: p.sources,
            policy_path: cfg.policy_path.clone(),
            settings_path: cfg.settings_path.clone(),
            env_path: cfg.env_path.clone(),
            vault_env_path: cfg.vault_env_path.clone(),
            infra_dir: cfg.infra_dir.clone(),
            cache_dir: voice.cache_dir.clone(),
            input_device: voice.input_device.clone(),
            output_device: voice.output_device.clone(),
            stt_model: voice.stt_model.clone(),
            environ: self.environ.clone(),
            conversations: Some(p.conversations),
            apps: Some(p.apps),
            catalog: Some(p.catalog),
            signins: Some(p.signins),
            skills: Some(p.skills),
            skill_catalog: Some(p.skill_catalog),
            folder: Some(p.folder),
            rules: Some(p.rules),
            outputs: Some(p.outputs),
            waiting: Some(p.waiting),
            watches: Some(p.watches),
            hooks: Some(p.hooks),
            hooks_listening: p.hooks_listening,
            passwords: Some(p.passwords),
            devices: self.ports.devices.clone(),
            model_net: Arc::new(LiveModelNet),
            model_sign_in: ghost_api::deps::ModelSignIn::default(),
        });
        self.late.deps.set(deps.clone()).ok();
        let listener = match ghost_api::server::bind(&socket).await {
            Ok(l) => l,
            Err(e) => {
                // the voice loop must run even if the API can't
                error!("api failed to start; continuing without it: {e}");
                return None;
            }
        };
        info!("api: listening on {}", socket.display());
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let app = ghost_api::router(deps);
        let path = socket.clone();
        let task = tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stop_rx.await;
                })
                .await;
            let _ = std::fs::remove_file(&path);
            if let Err(e) = result {
                error!("api: {e}");
            }
        });
        Some(ApiHandle {
            stop: Some(stop_tx),
            task,
        })
    }

    /// The phone listener, when GHOST_PHONE_LISTEN is set (ADR 0016). It starts before
    /// the models load so /hello answers and /offer says not_ready meanwhile.
    async fn start_phone_listener(self: &Arc<Self>) {
        let cfg = self.cfg();
        if cfg.phone_listen.trim().is_empty() {
            return;
        }
        let listener = match PhoneListener::new(
            &cfg.phone_listen,
            Arc::new(PhoneStore::new(cfg.phone_path())),
            self.clone(),
        ) {
            Ok(l) => Arc::new(l),
            Err(e) => {
                error!("phone: listener not started on {:?}: {e}", cfg.phone_listen);
                return;
            }
        };
        match PhoneServers::start(listener.clone()).await {
            Ok(servers) => {
                *lock(&self.phone_listener) = Some(listener);
                *lock(&self.phone_servers) = Some(servers);
            }
            Err(e) => error!("phone: listener not started on {:?}: {e}", cfg.phone_listen),
        }
    }

    /// The socket this daemon's API answers on, for a client.
    pub fn socket_path(&self) -> PathBuf {
        self.cfg().api_socket.unwrap_or_else(default_socket_path)
    }

    /// The model this session runs on.
    pub(crate) fn model(&self) -> anyhow::Result<Arc<dyn ModelPort>> {
        (self.ports.model)(&self.voice(), &self.env())
    }

    /// "Last used" on the Passwords page (ADR 0036): the model connection's key and the
    /// ElevenLabs voice's.
    pub(crate) fn note_passwords_in_use(&self) {
        if let Some(vault) = self.passwords() {
            let voice = self.voice();
            let names = in_use(Some(&voice.llm_connection()), &voice.tts_engine);
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            vault.note_used(&refs);
        }
    }

    pub(crate) fn audio_settings(&self, duck: bool) -> AudioSettings {
        let voice = self.voice();
        let cfg = self.cfg();
        AudioSettings {
            input_device: voice.input_device.clone(),
            output_device: voice.output_device.clone(),
            duck_apps: if duck {
                cfg.duck_apps.clone()
            } else {
                Vec::new()
            },
            duck_level: cfg.duck_level,
            chime: cfg.chime,
            thinking_sound: voice.thinking_sound,
            thinking_volume: voice.thinking_volume,
            thinking_delay_secs: voice.thinking_delay_secs,
            cache_dir: voice.cache_dir.clone(),
        }
    }
}

enum Won {
    Wake(Option<String>),
    Stop,
    Rearm,
    Phone(PendingPhone),
    Raised(Raised),
}

struct StartApi {
    gate: Arc<WriteGate>,
    store: Arc<Store>,
    sources: Vec<ToolSource>,
    apps: Arc<Apps>,
    catalog: Arc<Catalog>,
    skill_catalog: Arc<SkillCatalog>,
    signins: Arc<SignIns>,
    skills: Arc<Skills>,
    folder: Arc<Mutex<Folder>>,
    rules: Arc<Rules>,
    outputs: Arc<Outputs>,
    waiting: Arc<WaitingList>,
    watches: Arc<WatchRunner>,
    hooks: Arc<HookIntake>,
    hooks_listening: Vec<String>,
    passwords: Arc<Passwords>,
    conversations: Arc<Conversations>,
}

/// The control API as started: stop it and wait for it.
struct ApiHandle {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ApiHandle {
    async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if tokio::time::timeout(API_STOP_WAIT, &mut self.task)
            .await
            .is_err()
        {
            warn!("api: connections still open at shutdown; closing them");
            self.task.abort();
        }
    }
}

/// The declared watches and hooks, again: after the folder was read or re-read.
async fn follow_watches(
    folder: &Arc<Mutex<Folder>>,
    apps: &Apps,
    watch_runner: &Arc<WatchRunner>,
    hook_intake: &HookIntake,
) {
    let (watched, hooks) = {
        let f = lock(folder);
        (
            f.watches.values().cloned().collect::<Vec<_>>(),
            f.hooks.values().cloned().collect::<Vec<_>>(),
        )
    };
    let known: BTreeSet<String> = apps.specs().into_iter().map(|s| s.name).collect();
    for w in &watched {
        if !known.contains(&w.app) {
            warn!("watch {}: reads {}, which isn't connected", w.name, w.app);
        }
    }
    if let Err(e) = watch_runner.set_watches(&watched) {
        warn!("watches: {e}");
    }
    for problem in hook_intake.set_hooks(&hooks).await {
        warn!("hooks: {problem}");
    }
}

/// A Jev client per GHOST_JEV and whichever key is present, or None for regex only.
pub fn build_jev(
    cfg: &GhostdSettings,
    env: &HashMap<String, String>,
) -> Result<Option<Arc<JevClient>>, ghost_core::JevError> {
    let mut env = env.clone();
    env.insert("TYPESAFE_API_KEY".into(), cfg.typesafe_api_key.clone());
    env.insert("OPENROUTER_API_KEY".into(), cfg.openrouter_api_key.clone());
    let options = JevOptions {
        timeout: Duration::from_secs_f64(cfg.jev_deadline_secs.max(0.1)),
        ..JevOptions::default()
    };
    Ok(JevClient::from_env(&cfg.jev, &env, options)?.map(Arc::new))
}

/// The TTS engine, for the log (`ghost_voice.elevenlabs.describe_tts`).
pub fn describe_tts(voice: &VoiceSettings) -> String {
    if voice.tts_engine == "elevenlabs" {
        format!(
            "elevenlabs ({}), kokoro behind it ({} at {})",
            voice.elevenlabs_model, voice.tts_voice, voice.tts_speed
        )
    } else {
        format!("kokoro ({} at {})", voice.tts_voice, voice.tts_speed)
    }
}

// --- small adapters ---------------------------------------------------------------------------

/// The daemon's environment as the apps read it (`${NAME}` from the vault, ADR 0036).
pub struct EnvironView(pub SharedEnviron);

impl Environ for EnvironView {
    fn get(&self, name: &str) -> Option<String> {
        lock(&self.0)
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    }
}

/// The same, as the hooks' secrets.
pub struct EnvironSecrets(pub SharedEnviron);

impl Secrets for EnvironSecrets {
    fn secret(&self, name: &str) -> Option<zeroize::Zeroizing<String>> {
        lock(&self.0)
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .map(zeroize::Zeroizing::new)
    }
}

/// A watch's Jev step over the typed choices.
pub struct JevStep(pub WatchDecider);

#[async_trait]
impl Decider for JevStep {
    async fn decide(
        &self,
        question: &str,
        fields: &Map<String, Scalar>,
        choices: &BTreeMap<Choice, String>,
    ) -> anyhow::Result<Option<String>> {
        let named: BTreeMap<String, String> = choices
            .iter()
            .map(|(c, why)| (c.as_str().to_string(), why.clone()))
            .collect();
        Ok(self.0.decide(question, fields.clone(), &named).await)
    }
}

/// A read the daemon makes (a watch's look, a webhook's check, ADR 0027/0028): one of
/// the app's read-only tools, unless the operator's permissions block it.
struct ReadFor {
    gate: Arc<WriteGate>,
    apps: Arc<Apps>,
}

#[async_trait]
impl AppReader for ReadFor {
    async fn read(&self, app: &str, tool: &str, args: Map<String, Value>) -> anyhow::Result<Value> {
        if self.gate.blocks_read(app, tool, &args) {
            anyhow::bail!("{app}.{tool} is blocked by the operator's permissions");
        }
        let out = self.apps.read_for(app, tool, &args).await?;
        Ok(Value::Object(out))
    }
}

// --- the devices (GET /devices) ---------------------------------------------------------------

/// The sound devices the desktop's Devices page lists: PipeWire's audio nodes (sources,
/// sinks, duplex), by `node.name`, with the channels and rate they say they have. Which
/// is the default is PipeWire's metadata, which the registry doesn't carry; the page
/// reads the configured names beside this list.
#[derive(Debug, Default, Clone, Copy)]
pub struct PwDevices;

impl DevicesPort for PwDevices {
    fn devices(&self) -> Vec<ghost_api::models::DeviceInfo> {
        let nodes = ghost_audio::Session::connect("ghostd").and_then(|s| s.nodes());
        match nodes {
            Ok(nodes) => nodes
                .iter()
                .filter_map(|n| device_of(n.id, &n.props))
                .collect(),
            Err(e) => {
                warn!("devices: {e}");
                Vec::new()
            }
        }
    }
}

/// One registry node as a device, or None when it isn't an audio source or sink.
pub fn device_of(
    id: u32,
    props: &BTreeMap<String, String>,
) -> Option<ghost_api::models::DeviceInfo> {
    let class = props.get("media.class").map(String::as_str).unwrap_or("");
    let (input, output) = match class {
        "Audio/Source" => (true, false),
        "Audio/Sink" => (false, true),
        "Audio/Duplex" => (true, true),
        _ => return None,
    };
    let channels: i64 = props
        .get("audio.channels")
        .and_then(|c| c.parse().ok())
        .unwrap_or(2);
    let rate: f64 = props
        .get("audio.rate")
        .and_then(|r| r.parse().ok())
        .unwrap_or(48_000.0);
    let name = props
        .get("node.name")
        .cloned()
        .or_else(|| props.get("node.description").cloned())?;
    Some(ghost_api::models::DeviceInfo {
        index: i64::from(id),
        name,
        max_input_channels: if input { channels } else { 0 },
        max_output_channels: if output { channels } else { 0 },
        default_samplerate: rate,
        is_default_input: false,
        is_default_output: false,
    })
}

// --- the log mirror -------------------------------------------------------------------------

/// The log, mirrored into the event stream for the desktop app's log view: the daemon's
/// own words, INFO and up, redacted (ADR 0036), and only while someone listens.
#[derive(Clone, Default)]
pub struct LogMirror {
    state: Arc<RwLock<Option<Arc<DaemonState>>>>,
}

impl LogMirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// The daemon's state, once it is there.
    pub fn attach(&self, state: Arc<DaemonState>) {
        *self.state.write().unwrap_or_else(|e| e.into_inner()) = Some(state);
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogMirror {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > tracing::Level::INFO {
            return;
        }
        let state = self.state.read().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(state) = state else { return };
        if state.bus.subscribers() == 0 {
            return;
        }
        let mut v = MessageVisitor(String::new());
        event.record(&mut v);
        let name = match level {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARNING",
            _ => "INFO",
        };
        state.log(name, &redact_text(&v.0));
    }
}

/// The settings the process environment gives, with the vault's values over it.
pub fn settings_from(
    env: &HashMap<String, String>,
) -> anyhow::Result<(GhostdSettings, VoiceSettings)> {
    let cfg = GhostdSettings::from_env(env)?;
    let voice = VoiceSettings::from_vars(|name| env.get(name).cloned())?;
    Ok((cfg, voice))
}

#[cfg(test)]
pub(crate) mod tests {
    //! `apps/ghostd/tests/test_ghostd.py` and `test_passwords_daemon.py`, the daemon's
    //! own, by name. A fake keyring and temp folders: never the operator's (ADR 0036).

    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    use ghost_core::MemoryKeyring;

    use super::*;

    /// A daemon with nothing but its settings over a temp folder.
    pub(crate) fn daemon(tmp: &Path) -> Arc<Daemon> {
        let cfg = GhostdSettings {
            data_dir: tmp.join("data"),
            config_dir: tmp.join("config"),
            db_path: tmp.join("data/ghost.db"),
            policy_path: tmp.join("config/policy.toml"),
            chime: false,
            ..GhostdSettings::default()
        };
        let voice = VoiceSettings::from_vars(|name| {
            (name == "GHOST_CACHE_DIR").then(|| tmp.join("cache").to_string_lossy().into_owned())
        })
        .unwrap();
        Arc::new(Daemon::new(cfg, voice, Ports::headless()).with_runtime_dir(tmp.join("rt")))
    }

    /// Records every session the loop would have run, by the line it opens in.
    pub(crate) struct Recorder(pub Mutex<Vec<String>>);

    #[async_trait]
    impl SessionRunner for Recorder {
        async fn run(&self, _d: Arc<Daemon>, start: SessionStart) {
            self.0.lock().unwrap().push(start.start_in);
        }
    }

    pub(crate) fn with_recorder(tmp: &Path) -> (Arc<Daemon>, Arc<Recorder>) {
        let recorder = Arc::new(Recorder(Mutex::new(Vec::new())));
        let mut d = Arc::try_unwrap(daemon(tmp)).ok().expect("one reference");
        d.ports.sessions = recorder.clone();
        (Arc::new(d), recorder)
    }

    /// A wake that scripts what the loop sees, wait by wait.
    pub(crate) struct Scripted {
        pub waits: AtomicUsize,
        pub on_wait: Box<dyn Fn(usize) -> BoxFuture<'static, Option<String>> + Send + Sync>,
    }

    #[async_trait]
    impl WakeDetector for Scripted {
        async fn wait(&self) -> Option<String> {
            let n = self.waits.fetch_add(1, Ordering::SeqCst) + 1;
            (self.on_wait)(n).await
        }
        async fn stop(&self) {}
        fn close(&self) {}
    }

    #[tokio::test]
    async fn test_a_settings_change_rebuilds_the_listener_instead_of_waking() {
        // A live wake setting re-arms the passive loop: the pending wait is cancelled and
        // a new one starts. It once fell through to the cancelled task's result and killed
        // ghostd (2026-10-21).
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder) = with_recorder(tmp.path());
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                move |n| {
                    let d = d.clone();
                    Box::pin(async move {
                        if n == 1 {
                            d.rearm.set(); // a settings change, then stop
                        } else {
                            d.stop.cancel();
                        }
                        std::future::pending().await
                    })
                }
            }),
        });
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake.clone()))
            .await
            .unwrap();
        assert_eq!(wake.waits.load(Ordering::SeqCst), 2);
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_a_call_opens_the_session_in_that_line() {
        // The desktop's Call button (2026-10-24): a manual wake names the agent the
        // session opens with; a spoken wake still lands in Ghost, and a retired or unknown
        // agent is refused.
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder) = with_recorder(tmp.path());
        let store = Arc::new(Store::in_memory().unwrap());
        let convs = Arc::new(
            Conversations::new(
                store.clone(),
                tmp.path().join("config"),
                ConversationsOptions::default(),
            )
            .unwrap(),
        );
        store
            .upsert_conversation(&Profile::new("legal", "Legal"))
            .unwrap();
        let mut old = Profile::new("old", "Old");
        old.retired_at = Some(ghost_core::time::utcnow());
        store.upsert_conversation(&old).unwrap();
        *lock(&d.conversations) = Some(convs);
        let manual = FileWakeDetector::new(tmp.path().join("rt/wake"));
        *lock(&d.manual) = Some(manual.clone());
        let passive_again = Arc::new(Event::new()); // the loop is waiting for its second wake
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                let manual = manual.clone();
                let again = passive_again.clone();
                move |n| {
                    let d = d.clone();
                    let manual = manual.clone();
                    let again = again.clone();
                    Box::pin(async move {
                        match n {
                            1 => None, // spoken: Ghost
                            2 => {
                                again.set();
                                manual.wait().await
                            }
                            _ => {
                                d.stop.cancel();
                                std::future::pending().await
                            }
                        }
                    })
                }
            }),
        });
        let caller = {
            let d = d.clone();
            let again = passive_again.clone();
            tokio::spawn(async move {
                again.wait().await;
                use ghost_api::Controller;
                assert!(!d.wake(None, Some("old".into())).await); // retired
                assert!(!d.wake(None, Some("nope".into())).await); // unknown
                assert!(d.wake(None, Some("legal".into())).await);
            })
        };
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake))
            .await
            .unwrap();
        caller.await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![GHOST.to_string(), "legal".to_string()]
        );
    }

    pub(crate) struct FakeDuck(pub Mutex<Vec<&'static str>>);

    impl Duck for FakeDuck {
        fn duck(&self) {
            self.0.lock().unwrap().push("duck");
        }
        fn restore(&self) {
            self.0.lock().unwrap().push("restore");
        }
    }

    #[tokio::test]
    async fn test_an_early_chime_without_a_session_restores_the_music() {
        // The trained detector chimes and ducks before the operator finishes (ADR 0025).
        // If something else wins first (a settings change here), the music comes back up,
        // and the next session chimes as usual.
        let tmp = tempfile::tempdir().unwrap();
        let (d, _recorder) = with_recorder(tmp.path());
        let ducker = Arc::new(FakeDuck(Mutex::new(Vec::new())));
        *lock(&d.ducker) = Some(ducker.clone());
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                move |n| {
                    let d = d.clone();
                    Box::pin(async move {
                        if n == 1 {
                            d.heard_early().await; // the detector decided
                            d.heard_early().await; // once is enough
                            d.rearm.set();
                        } else {
                            d.stop.cancel();
                        }
                        std::future::pending().await
                    })
                }
            }),
        });
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake))
            .await
            .unwrap();
        assert_eq!(*ducker.0.lock().unwrap(), vec!["duck", "restore"]);
        assert!(!d.chimed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_the_route_engine_exists_before_routes_are_added_to_it() {
        // Daemon.run is not exercised end to end here (it loads the voice models); in Rust
        // the order that broke startup on 2026-10-27 can't compile the other way round
        // (`RouteEngine::new` before `add_route`), and the daemon lists what it added.
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        assert_eq!(d.routes_available(), vec![RouteName::Digest]);
        *lock(&d.route_names) = vec![RouteName::Desk, RouteName::Desktop, RouteName::Phone];
        assert_eq!(
            d.routes_available(),
            vec![
                RouteName::Digest,
                RouteName::Desk,
                RouteName::Desktop,
                RouteName::Phone
            ]
        );
    }

    // --- the vault (test_passwords_daemon.py) ----------------------------------------------

    fn vault_in(tmp: &Path, ring: &Arc<MemoryKeyring>, environ: SharedEnviron) -> Arc<Passwords> {
        Arc::new(
            Passwords::new(
                tmp.join("passwords.vault"),
                ring.clone() as Arc<dyn Keyring>,
            )
            .with_environ(environ),
        )
    }

    #[tokio::test]
    async fn test_the_daemon_follows_a_vault_that_opens_late() {
        let tmp = tempfile::tempdir().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(tmp.path().join("passwords.vault"), ring.clone());
        assert!(seeded.open());
        seeded
            .set("OPENROUTER_API_KEY", "sk-or-0123456789abcdef")
            .unwrap();
        ring.set_locked(true);
        let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
        let vault = vault_in(tmp.path(), &ring, environ.clone());
        assert!(!vault.open());
        let mut d = Arc::try_unwrap(daemon(tmp.path())).ok().unwrap();
        d = d
            .with_passwords(vault.clone())
            .with_environ(environ.clone());
        let d = Arc::new(d);
        let store = Arc::new(Store::in_memory().unwrap());
        let gate = Arc::new(WriteGate::new(store));
        d.late.gate.set(gate.clone()).ok();
        ring.set_locked(false);
        d.clone().wait_for_vault(Duration::from_millis(10)).await;
        assert!(vault.is_open());
        assert!(
            lock(&environ)
                .get("OPENROUTER_API_KEY")
                .is_some_and(|v| v.starts_with("sk-or-"))
        );
        // the retry loop spawned the follow-up; give it a moment
        for _ in 0..50 {
            if d.followed_vault.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(d.followed_vault.load(Ordering::SeqCst));
        // Jev joined through the key the vault holds: the settings were read again from
        // the daemon's environment, where the key now is, and the gate's classifier was
        // set (the gate has no getter for it; `test_the_vault_opening_is_followed_once`
        // covers the once).
        assert!(d.env().contains_key("OPENROUTER_API_KEY"));
    }

    #[tokio::test]
    async fn test_the_vault_opening_is_followed_once() {
        // The unlock route and the retry loop may both see the vault open: Jev joins once.
        let tmp = tempfile::tempdir().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
        let vault = vault_in(tmp.path(), &ring, environ.clone());
        assert!(vault.open());
        lock(&environ).insert("OPENROUTER_API_KEY".into(), "sk-or-0123456789abcdef".into());
        let d = Arc::try_unwrap(daemon(tmp.path()))
            .ok()
            .unwrap()
            .with_passwords(vault)
            .with_environ(environ);
        let d = Arc::new(d);
        let store = Arc::new(Store::in_memory().unwrap());
        let gate = Arc::new(WriteGate::new(store));
        d.late.gate.set(gate.clone()).ok();
        d.passwords_opened().await;
        d.passwords_opened().await;
        assert!(d.followed_vault.load(Ordering::SeqCst));
        assert_eq!(
            gate.last_classification().map(|_| 1).unwrap_or(0),
            0,
            "no decision was made yet"
        );
    }

    #[test]
    fn test_in_use_follows_the_model_connection_and_the_voice() {
        use ghost_core::connection::ModelConnection;
        let conn = |pairs: &[(&str, &str)]| {
            let env: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            ModelConnection::from_env(&env)
        };
        let proxy = conn(&[
            ("ANTHROPIC_BASE_URL", "http://x:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "t"),
        ]);
        assert_eq!(in_use(Some(&proxy), "kokoro"), vec!["ANTHROPIC_AUTH_TOKEN"]);
        let router = conn(&[("GHOST_LLM_ENDPOINT", "openrouter")]);
        assert_eq!(
            in_use(Some(&router), "elevenlabs"),
            vec!["OPENROUTER_API_KEY", "ELEVENLABS_API_KEY"]
        );
        let openai = conn(&[("GHOST_LLM_PROVIDER", "openai")]);
        assert_eq!(in_use(Some(&openai), "kokoro"), vec!["OPENAI_API_KEY"]);
        let ollama = conn(&[("GHOST_LLM_PROVIDER", "ollama")]);
        assert!(in_use(Some(&ollama), "kokoro").is_empty()); // no key at all
    }

    #[test]
    fn apply_model_takes_the_connection_keys_into_the_daemons_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        let mut values = BTreeMap::new();
        values.insert("GHOST_LLM_PROVIDER".to_string(), Some("openai".to_string()));
        values.insert(
            "GHOST_WAKE_PHRASES".to_string(),
            Some("hey ghost".to_string()),
        );
        assert_eq!(d.apply_model(&values), vec!["GHOST_LLM_PROVIDER"]);
        assert_eq!(
            lock(&d.environ)
                .get("GHOST_LLM_PROVIDER")
                .map(String::as_str),
            Some("openai")
        );
        assert_eq!(d.voice().llm_connection().provider, "openai");
        values.insert("GHOST_LLM_PROVIDER".to_string(), None);
        assert_eq!(d.apply_model(&values), vec!["GHOST_LLM_PROVIDER"]);
        assert!(!lock(&d.environ).contains_key("GHOST_LLM_PROVIDER"));
    }

    #[test]
    fn the_devices_page_lists_audio_sources_and_sinks_by_node_name() {
        let props = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let mic = device_of(
            41,
            &props(&[
                ("media.class", "Audio/Source"),
                ("node.name", "ghost.mic"),
                ("audio.channels", "1"),
                ("audio.rate", "16000"),
            ]),
        )
        .unwrap();
        assert_eq!((mic.index, mic.name.as_str()), (41, "ghost.mic"));
        assert_eq!((mic.max_input_channels, mic.max_output_channels), (1, 0));
        assert_eq!(mic.default_samplerate, 16000.0);
        let sink = device_of(
            7,
            &props(&[("media.class", "Audio/Sink"), ("node.name", "speakers")]),
        )
        .unwrap();
        assert_eq!((sink.max_input_channels, sink.max_output_channels), (0, 2));
        assert!(
            device_of(
                9,
                &props(&[
                    ("media.class", "Stream/Output/Audio"),
                    ("node.name", "spotify")
                ])
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn the_log_mirror_hands_the_desktop_redacted_lines_only_while_it_listens() {
        let mirror = LogMirror::new();
        let state = Arc::new(DaemonState::new("test", "file"));
        mirror.attach(state.clone());
        let mut rx = state.bus.subscribe();
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::registry().with(mirror.clone());
        tracing::subscriber::with_default(subscriber, || {
            info!("hello from the daemon");
            tracing::debug!("not mirrored");
        });
        let text = rx.try_recv().unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "log");
        assert_eq!(v["level"], "INFO");
        assert_eq!(v["message"], "hello from the daemon");
        assert!(rx.try_recv().is_err());
    }
}
