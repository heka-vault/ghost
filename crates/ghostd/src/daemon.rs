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
use ghost_core::apps::{APPS_FILE, Apps, Environ, GHOST_APP, load_apps};
use ghost_core::catalog::Catalog;
use ghost_core::confirmation::{ConfirmationClassifier, RegexClassifier};
use ghost_core::conversations::{
    Conversations, ConversationsOptions, GHOST, Profile, SOURCE as CONVERSATIONS,
    conversations_source,
};
use ghost_core::folder::Folder;
use ghost_core::gate::WriteGate;
use ghost_core::ghost_app::clients::Clients;
use ghost_core::ghost_app::outside::Outside;
use ghost_core::ghost_app::{self, GhostServer};
use ghost_core::hooks::{HookIntake, Secrets};
use ghost_core::jev::{JevClassifier, JevClient, JevOptions, WatchDecider, watch_decider};
use ghost_core::listening::{Listening, Mic};
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
use crate::routes::{CallRoute, DeskRoute, DesktopRoute, NtfyRoute, PhoneRoute, Raised};
use crate::wake::{
    DetectorSpec, Event, FileWakeDetector, MediaFactory, SpokenWakeDetector, SttFactory, WAKE_FILE,
    WakeDetector, WakeListener, desk_capture, detector_phrases, wake_model_paths, wake_resources,
};
use ghost_core::questions::{AppAsker, Question, Questions};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DUCK_STATE: &str = "duck.json";
/// How often Ghost's own items are re-read and the routes tick.
pub const WAITING_REFRESH_SECS: u64 = 15;
/// A desk session this recent means the operator is at the desk.
pub const AT_DESK_SECS: u64 = 15 * 60;
/// How long the wind-down waits for the control API to close.
pub const API_STOP_WAIT: Duration = Duration::from_secs(5);
/// How often the transcript is pruned (ADR 0041).
pub const PRUNE_EVERY: Duration = Duration::from_secs(24 * 3600);

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
    /// Typed in the desktop app with no session open (2026-11-13): a typed session, which
    /// opens no microphone and ends a short while after its reply; the passive listener
    /// keeps running beside it.
    pub typed: bool,
    /// Ends the session from outside: the operator called Ghost by name during a typed one.
    pub end: CancellationToken,
}

impl SessionStart {
    /// A session at the desk or on the phone, as a wake, a call or Ghost raising starts it.
    pub fn new(start_in: impl Into<String>) -> Self {
        Self {
            first_utterance: None,
            phone: None,
            start_in: start_in.into(),
            raised: None,
            typed: false,
            end: CancellationToken::new(),
        }
    }
}

/// After Ghost's voice stops, how long a wake beside a typed session is still taken for its
/// own voice coming back through the room (the canceller's tail).
pub const OWN_VOICE_GRACE: Duration = Duration::from_millis(1500);

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
    /// Open the microphone; `false` while not listening (ADR 0057): a typed turn's
    /// session, its input silent and no capture stream opened.
    pub mic: bool,
}

pub type LoadModels = Arc<dyn Fn(&VoiceSettings) -> anyhow::Result<Models> + Send + Sync>;
pub type ModelFactory = Arc<
    dyn Fn(&VoiceSettings, &HashMap<String, String>) -> anyhow::Result<Arc<dyn ModelPort>>
        + Send
        + Sync,
>;
/// The fast model's client when the connection names one (ADR 0050).
pub type FastModelFactory = Arc<
    dyn Fn(&VoiceSettings, &HashMap<String, String>) -> Option<Arc<dyn ModelPort>> + Send + Sync,
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
    pub fast_model: FastModelFactory,
    pub desk_media: DeskMedia,
    /// The passive pipeline's capture; None: `ghost.mic` through `ghost_audio`.
    pub passive_media: Option<MediaFactory>,
    pub play: Play,
    pub ducker: DuckFactory,
    /// The players paused for a session and resumed after (`players`).
    pub players: Arc<dyn crate::players::PlayersPort>,
    pub sessions: Arc<dyn SessionRunner>,
    pub devices: Arc<dyn DevicesPort>,
    /// The sound devices as PipeWire has them (ADR 0064); None where it can't be read.
    pub sound_facts: Arc<dyn Fn() -> Option<crate::microphone::SoundFacts> + Send + Sync>,
    /// What Repair does to the sound stack (ADR 0064).
    pub repairer: Arc<dyn crate::microphone::Repairer>,
}

/// No sound stack to repair: a test's.
pub struct NoRepair;

impl crate::microphone::Repairer for NoRepair {
    fn restart_canceller(&self) -> Result<(), String> {
        Err("no echo canceller here".into())
    }
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
            fast_model: Arc::new(|voice, env| {
                let target = voice.llm_fast_target(Some(env))?;
                match ghost_llm::model::shared(&target) {
                    Ok(m) => Some(m as Arc<dyn ModelPort>),
                    Err(e) => {
                        warn!("llm: no client for the fast model ({e}); the main model answers");
                        None
                    }
                }
            }),
            desk_media: Arc::new(|req: DeskMediaRequest| {
                Box::pin(async move {
                    desk_media(&req.audio, req.thinking_sound, req.chime, req.mic)
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
            players: Arc::new(crate::players::MprisPlayers::default()),
            sound_facts: Arc::new(crate::microphone::live_facts),
            repairer: Arc::new(crate::microphone::LiveRepairer),
        }
    }

    /// No devices, no keyring of the operator's, no models: what a test starts from. The
    /// models, the model client and the sessions must be given before they are used.
    pub fn headless() -> Self {
        Self {
            keyring: Arc::new(ghost_core::MemoryKeyring::new()),
            load_models: Arc::new(|_| anyhow::bail!("no models in this test")),
            model: Arc::new(|_, _| anyhow::bail!("no model in this test")),
            fast_model: Arc::new(|_, _| None),
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
            players: Arc::new(crate::players::NoPlayers),
            sound_facts: Arc::new(|| None),
            repairer: Arc::new(NoRepair),
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
    /// The waiting-list key of the ring this call answers (ADR 0051), when it is one.
    pub item: Option<String>,
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
    /// The schedules and their runs (ADR 0047).
    pub schedules: OnceLock<Arc<ghost_core::schedules::Scheduler>>,
    /// The Hearing test's recorder (ADR 0042).
    pub hear: OnceLock<Arc<crate::hear::HearDesk>>,
    /// Kept wake takes (ADR 0065).
    pub wake_takes: OnceLock<Arc<ghost_core::wake_takes::Takes>>,
    /// The outside tools and their listener (ADR 0048).
    pub outside: OnceLock<Arc<crate::outside::OutsideHost>>,
}

/// What a session with any agent reaches (ADR 0034, ADR 0035).
pub type Reach = Arc<dyn Fn(&Profile) -> Vec<ToolSource> + Send + Sync>;

/// The waiting list's key for the realtime voice's note (ADR 0063).
pub const REALTIME_NOTE_KEY: &str = "voice:realtime";

pub struct Daemon {
    pub cfg: Mutex<GhostdSettings>,
    pub voice: Mutex<Arc<VoiceSettings>>,
    /// The daemon's own environment: the vault's values, and the settings saved since the
    /// start, over the process's (ADR 0036, ADR 0037).
    pub environ: SharedEnviron,
    /// The process environment the daemon was started in, read once (`env` resolves the
    /// two with `ghost_core::environment`, and so does the control API).
    pub started_env: Arc<HashMap<String, String>>,
    pub ports: Ports,
    pub rt: PathBuf,
    pub stop: CancellationToken,
    pub state: Arc<DaemonState>,
    pub passwords: Mutex<Option<Arc<Passwords>>>,
    followed_vault: AtomicBool,
    /// The last model load's failure while it is tried again; on the waiting list meanwhile.
    models_problem: Mutex<Option<String>>,
    /// Why the realtime voice was not used or stopped, for the waiting list (ADR 0063):
    /// a note under [`REALTIME_NOTE_KEY`], shown and never said.
    pub(crate) realtime_note: Mutex<Option<String>>,
    pub(crate) realtime_check: Mutex<Option<ghost_api::realtime::RealtimeCheckView>>,
    /// The connection is too slow for a good spoken reply (ADR 0056); on the waiting list.
    pub(crate) model_slow: Mutex<Option<ghost_llm::check::Slow>>,
    models_retry: std::time::Duration,
    pub late: Late,
    /// The agents, with the voices once Kokoro is loaded.
    pub conversations: Mutex<Option<Arc<Conversations>>>,
    reach: Mutex<Option<Reach>>,
    pub(crate) session: Mutex<Option<Arc<GhostSession>>>,
    pub(crate) last_desk_session: Mutex<Option<Instant>>,
    pub(crate) phone_route: Arc<PhoneRoute>,
    /// The phone's live stream (ADR 0051): what it is told and what waits on its word.
    pub(crate) phone_feed: Arc<crate::phone::stream::PhoneFeed>,
    /// The questions for the operator (ADR 0051); set once in `run`, and read by the
    /// connected apps' asker before then (it declines until they are there).
    pub(crate) questions: Arc<OnceLock<Arc<Questions>>>,
    pub(crate) first_via: Mutex<TurnVia>,
    /// When Ghost's voice last stopped in a session (`own_voice_now`).
    pub(crate) spoke_at: Mutex<Option<Instant>>,
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
    /// Not listening (ADR 0057): the operator's switch for the microphone, kept in the store.
    pub mic: Arc<Mic>,
    /// What the microphone path is and how it is doing (ADR 0064).
    pub(crate) mic_health: Arc<Mutex<crate::microphone::Health>>,
    /// The plan and when it was made: the facts are read again after `PLAN_FOR`.
    mic_plan: Mutex<Option<(Instant, crate::microphone::Plan)>>,
}

/// How long a read of the sound devices is trusted.
const PLAN_FOR: Duration = Duration::from_secs(10);

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
        let phone_feed = Arc::new(crate::phone::stream::PhoneFeed::new());
        Self {
            cfg: Mutex::new(cfg),
            voice: Mutex::new(Arc::new(voice)),
            environ: Arc::new(Mutex::new(HashMap::new())),
            started_env: Arc::new(std::env::vars().collect()),
            ports,
            rt: runtime_dir(),
            stop: CancellationToken::new(),
            state,
            passwords: Mutex::new(None),
            followed_vault: AtomicBool::new(false),
            models_problem: Mutex::new(None),
            realtime_note: Mutex::new(None),
            realtime_check: Mutex::new(None),
            model_slow: Mutex::new(None),
            models_retry: MODELS_RETRY,
            late: Late::default(),
            conversations: Mutex::new(None),
            reach: Mutex::new(None),
            session: Mutex::new(None),
            last_desk_session: Mutex::new(None),
            phone_route: Arc::new(PhoneRoute::new(phone_feed.clone())),
            phone_feed,
            questions: Arc::new(OnceLock::new()),
            first_via: Mutex::new(TurnVia::Voice),
            spoke_at: Mutex::new(None),
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
            mic: Arc::new(Mic::new()),
            mic_health: Arc::new(Mutex::new(crate::microphone::Health::default())),
            mic_plan: Mutex::new(None),
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

    /// The environment the daemon was started in, for a test (the process's by default).
    pub fn with_started_env(mut self, env: HashMap<String, String>) -> Self {
        self.started_env = Arc::new(env);
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

    /// The environment Ghost's settings read: the one it was started in, its own over it
    /// (the vault's values, the settings saved since). The control API resolves the same
    /// (`Deps::ghost_env`), so the desktop says what a session uses (ADR 0037).
    pub fn env(&self) -> HashMap<String, String> {
        ghost_core::environment::resolve(&self.started_env, &lock(&self.environ))
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

    /// Kept wake takes (ADR 0065): made once, under the data folder.
    pub fn wake_takes(&self) -> Arc<ghost_core::wake_takes::Takes> {
        self.late
            .wake_takes
            .get_or_init(|| Arc::new(ghost_core::wake_takes::Takes::new(&self.cfg().data_dir)))
            .clone()
    }

    /// The Hearing test's recorder (ADR 0042).
    pub fn hear(&self) -> Option<Arc<crate::hear::HearDesk>> {
        self.late.hear.get().cloned()
    }

    /// The recorder asking for the mic; never, without one.
    pub(crate) async fn next_hear(&self) -> crate::hear::Lease {
        match self.hear() {
            Some(h) => h.next_lease().await,
            None => std::future::pending().await,
        }
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

    /// The questions for the operator (ADR 0051).
    pub fn questions(&self) -> Option<Arc<Questions>> {
        self.questions.get().cloned()
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

    pub(crate) async fn next_phone(&self) -> PendingPhone {
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

    /// Ask the passive loop to speak first at the desk (ADR 0032): the desk route's
    /// queue, for one sentence an outside tool sent (ADR 0048).
    pub(crate) fn raise(&self, raised: Raised) -> bool {
        self.raise_tx.send(raised).is_ok()
    }

    pub(crate) async fn next_raised(&self) -> Option<Raised> {
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

    /// A question was asked or settled (ADR 0051): the event stream says so, and the
    /// phone's stream with it.
    pub(crate) fn question_changed(&self, q: &Question) {
        let view = ghost_api::questions::question_view(q);
        self.state.bus.publish(&ghost_api::models::Event::Question {
            question: view.clone(),
        });
        self.phone_feed.question(view);
    }

    /// The phone's live stream follows the waiting list (ADR 0051): its snapshot is the
    /// list and the questions as the phone sees them, and each change on the bus is said
    /// to it. A tap beside the bus, so it is not the desktop being open (ADR 0031).
    pub(crate) fn start_phone_feed(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let view = move || -> (Vec<ghost_api::models::WaitingItemView>, Vec<ghost_api::models::QuestionView>) {
            let Some(d) = weak.upgrade() else {
                return (Vec::new(), Vec::new());
            };
            let Some(deps) = d.deps() else {
                return (Vec::new(), Vec::new());
            };
            let items = ghost_api::waiting_view(&deps)
                .map(|v| v.items)
                .unwrap_or_default();
            let questions = ghost_api::questions::questions_view(&deps)
                .map(|v| v.questions)
                .unwrap_or_default();
            (items, questions)
        };
        let view: crate::phone::stream::Snapshot = Arc::new(view);
        self.phone_feed.set_snapshot(view.clone());
        let mut bus = self.state.bus.tap();
        let feed = self.phone_feed.clone();
        let stop = self.stop.clone();
        tokio::spawn(async move {
            loop {
                let text = tokio::select! {
                    () = stop.cancelled() => return,
                    t = bus.recv() => match t {
                        Ok(t) => t,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => return,
                    },
                };
                let kind = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from));
                if matches!(kind.as_deref(), Some("waiting" | "pending" | "outcome")) {
                    let (items, _) = view();
                    feed.waiting_now(&items);
                }
            }
        });
    }

    /// Re-read Ghost's own items now and then (a change's time running out, a server
    /// that went away; watches sync their own), and let the routes send what is due
    /// (ADR 0031).
    async fn refresh_waiting(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(WAITING_REFRESH_SECS)).await;
            // A question past its time is expired, and leaves the list (ADR 0051).
            if let Some(q) = self.questions()
                && let Err(e) = q.expire(None)
            {
                error!("questions: {e}");
            }
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
        !self.mic.is_off() // ADR 0057: no voice at the desk that would open the microphone
            && self.state.models_ready()
            && self.waiting_flag.load(Ordering::SeqCst)
            && self.session().is_none()
            && self.state.phase() == Phase::Passive
            && self.phone_q_empty()
            && self.raise_q_empty()
            && !is_dictating(&self.cfg().dictation_flag)
    }

    /// The phone's own session is running, or is about to start: the phone route does
    /// not alert over it.
    pub(crate) fn on_the_phone(&self) -> bool {
        *lock(&self.origin) == Some(SessionOrigin::Phone) || !self.phone_q_empty()
    }

    /// Any session is running, or a phone session is about to: the call route does not
    /// ring over it.
    pub(crate) fn in_session(&self) -> bool {
        self.session().is_some() || self.on_the_phone()
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
    /// The realtime voice's settings, live from the next session (ADR 0063). Not one of
    /// them is a password; a value that does not read is refused and the old kept.
    pub(crate) fn apply_realtime(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        let keys: Vec<String> = values
            .keys()
            .filter(|k| ghost_voice::realtime::settings::KEYS.contains(&k.as_str()))
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
                let value = values.get(k).and_then(|v| v.as_deref());
                ghost_core::environment::apply(&self.started_env, &mut env, k, value);
            }
        }
        let env = self.env();
        match ghost_voice::realtime::RealtimeSettings::from_vars(|k| env.get(k).cloned()) {
            Ok(s) => {
                info!("voice: {} from the next session", s.describe());
                if !s.on {
                    *lock(&self.realtime_note) = None;
                }
                keys
            }
            Err(e) => {
                warn!("voice: {e}; the realtime settings were not changed");
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
                Vec::new()
            }
        }
    }

    /// The realtime voice's settings as they are now.
    pub(crate) fn realtime_settings(&self) -> ghost_voice::realtime::RealtimeSettings {
        let env = self.env();
        ghost_voice::realtime::RealtimeSettings::from_vars(|k| env.get(k).cloned())
            .unwrap_or_default()
    }

    /// Today's realtime spend, kept beside Ghost's data (numbers only).
    pub(crate) fn realtime_ledger(&self) -> ghost_voice::realtime::spend::Ledger {
        ghost_voice::realtime::spend::Ledger::new(&self.cfg().data_dir)
    }

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
                let value = values.get(k).and_then(|v| v.as_deref());
                ghost_core::environment::apply(&self.started_env, &mut env, k, value);
            }
        }
        let env = self.env();
        match VoiceSettings::from_vars(|name| env.get(name).cloned()) {
            Ok(fresh) => {
                info!(
                    "llm: {} from the next session",
                    fresh.llm_target(Some(&env)).describe()
                );
                if let Some(fast) = fresh.llm_fast_target(Some(&env)) {
                    info!("llm: {} answers a spoken turn first", fast.model);
                }
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

    // --- the wake ------------------------------------------------------------------------

    fn stt_factory(models: &Arc<Models>) -> SttFactory {
        let stt = models.stt.clone();
        Arc::new(move |params| {
            Box::new(ParakeetStt::new(stt.clone(), None, params, 0.0))
                as Box<dyn ghost_voice::frames::Stage>
        })
    }

    /// The passive pipeline's input (ADR 0064): the wake's device from the plan, read
    /// again at each build; its frames counted for the microphone's health.
    fn passive_media(self: &Arc<Self>) -> MediaFactory {
        let inner = self.ports.passive_media.clone().unwrap_or_else(|| {
            let weak = Arc::downgrade(self);
            desk_capture(Arc::new(move || {
                weak.upgrade()
                    .map(|d| d.mic_plan().wake)
                    .unwrap_or_default()
            }))
        });
        let health = self.mic_health.clone();
        crate::wake::metered(
            inner,
            Arc::new(move |w| lock(&health).window(&w, Instant::now())),
        )
    }

    // --- the microphone path (ADR 0064) -----------------------------------------------------

    /// What Ghost listens on now: from the settings and the devices, read at most every
    /// ten seconds.
    pub fn mic_plan(&self) -> crate::microphone::Plan {
        if let Some((at, plan)) = lock(&self.mic_plan).as_ref()
            && at.elapsed() < PLAN_FOR
        {
            return plan.clone();
        }
        let voice = self.voice();
        let facts = (self.ports.sound_facts)();
        let plan = crate::microphone::plan(
            voice.input_device.as_deref(),
            voice.wake_input_device.as_deref(),
            voice.output_device.as_deref(),
            facts.as_ref(),
        );
        let changed = lock(&self.mic_plan)
            .as_ref()
            .is_none_or(|(_, p)| p.session != plan.session || p.wake != plan.wake);
        if changed {
            info!(
                "microphone: {} ({}); a session hears {}, the wake hears {}",
                plan.summary,
                plan.case.as_str(),
                plan.session,
                plan.wake
            );
            if let Some(f) = &plan.fell_back {
                warn!("microphone: {f}");
            }
        }
        *lock(&self.mic_plan) = Some((Instant::now(), plan.clone()));
        plan
    }

    /// The note for the waiting list, while the input has a problem.
    pub(crate) fn mic_note(&self) -> Option<String> {
        let problem = lock(&self.mic_health).problem(Instant::now())?;
        let plan = lock(&self.mic_plan)
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| self.mic_plan());
        Some(crate::microphone::note(&problem, &plan))
    }

    /// The microphone, for Settings › Sound and the first run.
    pub(crate) fn mic_view(&self) -> ghost_api::models::MicrophoneView {
        let plan = self.mic_plan();
        ghost_api::models::MicrophoneView {
            case: plan.case.as_str().to_string(),
            summary: plan.summary.clone(),
            advice: plan.advice.clone(),
            suggest: plan
                .suggest
                .iter()
                .map(|(key, value)| ghost_api::models::SettingSuggestion {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            fell_back: plan.fell_back.clone(),
            session_input: plan.session.to_string(),
            wake_input: plan.wake.to_string(),
            problem: self.mic_note(),
            can_repair: !self.in_session() && !self.mic.is_off(),
            repair: if self.cfg().input_repair_auto {
                "auto".into()
            } else {
                "ask".into()
            },
        }
    }

    /// Repair (ADR 0064): restart the echo canceller when it is in use, then reopen the
    /// passive microphone. Refused while a session is open (its stream would go with it)
    /// or while not listening (there is no microphone open to repair).
    pub(crate) async fn repair_mic(&self) -> Result<String, String> {
        if self.in_session() {
            return Err("End the session first: Repair reopens the microphone.".into());
        }
        if self.mic.is_off() {
            return Err("Ghost isn't listening; turn listening on first.".into());
        }
        *lock(&self.mic_plan) = None;
        let plan = self.mic_plan();
        let mut done = Vec::new();
        if plan.session.is_canceller() || plan.wake.is_canceller() {
            let repairer = self.ports.repairer.clone();
            tokio::task::spawn_blocking(move || repairer.restart_canceller())
                .await
                .map_err(|e| e.to_string())??;
            done.push("restarted the echo canceller");
            *lock(&self.mic_plan) = None;
        }
        if let Some(listener) = lock(&self.listener).clone() {
            listener.reopen();
            done.push("reopened the microphone");
        }
        lock(&self.mic_health).repaired(Instant::now());
        self.refresh_waiting_now();
        info!("microphone: repaired ({})", done.join(", "));
        Ok(if done.is_empty() {
            "Nothing to repair: the microphone opens with the next wake.".into()
        } else {
            let mut s = done.join(" and ");
            s.replace_range(0..1, &s[0..1].to_uppercase());
            format!("{s}.")
        })
    }

    /// Every minute: a note that appears or clears reaches the waiting list, and with
    /// `GHOST_INPUT_REPAIR=auto` a deaf input is repaired while no session is open, at
    /// most once an hour.
    async fn watch_mic(self: Arc<Self>) {
        let mut had = false;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let has = self.mic_note().is_some();
            if has != had {
                had = has;
                self.refresh_waiting_now();
            }
            let due = lock(&self.mic_health).auto_repair_due(Instant::now());
            if due && self.cfg().input_repair_auto && !self.in_session() && !self.mic.is_off() {
                match self.repair_mic().await {
                    Ok(what) => info!("microphone: repaired by itself: {what}"),
                    Err(e) => {
                        warn!("microphone: the automatic repair failed: {e}");
                        // Not again within the hour.
                        lock(&self.mic_health).repaired(Instant::now());
                    }
                }
            }
        }
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

    /// The trained detector could not be used: Ghost listens by transcript, and says so in
    /// its status (no wake takes are kept, ADR 0065).
    pub(crate) fn listen_by_transcript(&self) {
        lock(&self.cfg).wake_mode = "transcript".into();
        self.state.set_wake_mode("transcript");
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
        let (mut path, address) = wake_model_paths(&cfg, &voice);
        // ADR 0065: a retrained detector, with the threshold and floor from its own curve.
        let mut retrained: Option<(f64, f64)> = None;
        if let Some(name) = cfg.retrained_detector() {
            match crate::wake_detectors::load(&cfg.data_dir, name) {
                Ok(i) => {
                    info!(
                        "wake: retrained detector {name}, threshold {:.2}, floor {:.2}",
                        i.card.threshold, i.card.floor
                    );
                    path = i.path;
                    retrained = Some((i.card.threshold, i.card.floor));
                }
                Err(e) => error!("wake: {e}; hearing with the shipped detector"),
            }
        }
        let fall_back = |why: String| {
            error!("wake: {why}; listening by transcript instead");
            self.listen_by_transcript();
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
        let (threshold, floor) = match retrained {
            Some((t, f)) => (t, Some(f)),
            None => (cfg.detector_threshold(), cfg.wake_check_threshold),
        };
        let mut wake_models = vec![
            WakeModel::known(&path, threshold as f32, None).checked_from(floor.map(|t| t as f32)),
        ];
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
            let weak = match m.checked_from {
                Some(from) if from < m.threshold => {
                    format!(", Parakeet checks a score from {from}")
                }
                _ => String::new(),
            };
            info!(
                "wake: trained detector {}, threshold {}, {check}{weak}",
                m.name, m.threshold
            );
        }
        info!("wake: those phrases only");
        // ADR 0065: the array's own stream beside the one the detector hears, for the kept
        // takes, when there is one and the detector is not already hearing it.
        let tape = std::fs::read_to_string(cfg.config_dir.join("echo-cancel.conf"))
            .ok()
            .and_then(|c| crate::hear::capture_target(&c))
            .filter(|node| voice.input_device.as_deref() != Some(node.as_str()))
            .map(crate::wake_takes::ArrayTape::new);
        let spec = DetectorSpec {
            scorer: Arc::new(Mutex::new(scorer)),
            recognize: ghost_voice::stt::recognize_fn(&models.stt),
            models: wake_models,
            verify: cfg.wake_verify,
            cooldown_secs: cfg.wake_cooldown_secs,
            keeper: Some(Arc::new(crate::wake_takes::Keeper::new(
                self.wake_takes(),
                voice
                    .input_device
                    .clone()
                    .unwrap_or_else(|| "default".into()),
                tape.clone(),
            ))),
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
        listener.set_on_check(Some({
            let health = self.mic_health.clone();
            Arc::new(move |level, words, final_| {
                lock(&health).check(level, words, final_, Instant::now())
            })
        }));
        let on_heard: OnHeard = {
            let d = self.clone();
            Arc::new(move || {
                let d = d.clone();
                Box::pin(async move { d.heard_early().await })
            })
        };
        let spoken: Arc<dyn WakeDetector> =
            Arc::new(SpokenWakeDetector::new(listener, manual, Some(on_heard)));
        match tape {
            Some(tape) => Arc::new(crate::wake_takes::TapedWake::new(spoken, tape)),
            None => spoken,
        }
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
        if self.cfg().pause_players {
            self.ports.players.pause_playing().await;
        }
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
            let players = self.ports.players.clone();
            tokio::spawn(async move { players.resume().await });
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
        // ADR 0057: an operator who turned listening off and restarted is still not heard.
        match d.mic.attach(Items::new(store.clone())) {
            Ok(Listening::On) => {}
            Ok(_) => info!("listening: off, as the operator left it"),
            Err(e) => warn!("listening: the switch could not be read ({e}); listening"),
        }
        d.follow_mic();
        // ADR 0065: kept wake takes, on unless the operator turned them off.
        match d.wake_takes().attach(Items::new(store.clone())) {
            Ok(c) if c.keep && cfg.wake_mode == "transcript" => {
                // ADR 0065: only the trained detector's hits are takes; transcript wake has none.
                info!("wake takes: on, but none are kept with transcript wake (no detector)")
            }
            Ok(c) if c.keep => info!("wake takes: kept on this computer (Settings › Sound)"),
            Ok(_) => info!("wake takes: not kept, as the operator chose"),
            Err(e) => warn!("wake takes: the choice could not be read ({e})"),
        }
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
        let app_limits = {
            let env = d.env();
            ghost_core::apps::Limits::from_vars(|name| env.get(name).cloned())
        };
        let apps = Arc::new(
            Apps::new(
                apps_file.apps,
                Arc::new(EnvironView(d.environ.clone())) as Arc<dyn Environ>,
                Some(auth_for_apps(&signins)),
            )
            .with_limits(app_limits),
        );
        // ADR 0051: an app's question becomes a question from that app. Set before the
        // apps connect, so they are told Ghost takes questions; until the questions exist
        // (a moment later) one is declined.
        apps.set_asker(Arc::new(AppAsker::new(d.questions.clone())));
        // ADR 0046: the tools the model used or found are offered by name past the limits;
        // kept in SQLite so a daemon started again does not search for them first.
        let kept = store.recent_tools().unwrap_or_else(|e| {
            warn!("apps: the tools used lately could not be read: {e}");
            Vec::new()
        });
        apps.remember(kept, {
            let store = store.clone();
            Arc::new(move |names: &[String]| {
                if let Err(e) = store.set_recent_tools(names) {
                    warn!("apps: the tools used lately could not be kept: {e}");
                }
            })
        });
        apps.connect().await;
        apps.supervise(); // ADR 0046: a connection that dies is opened again
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
        // Each call's counts and sizes, as kept (ADR 0060): the desktop's event.
        gate.on_reach({
            let weak = Arc::downgrade(d);
            Arc::new(move |row| {
                if let Some(d) = weak.upgrade() {
                    d.state.publish_reach(row);
                }
            })
        });
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
                // Ghost's own app (ADR 0045) is not one the operator connected: it is
                // never in this set, so never unregistered and never listed with them.
                *reg = now.keys().filter(|n| *n != GHOST_APP).cloned().collect();
                if let Some(d) = weak.upgrade() {
                    // The changes only outside tools have, back beside Ghost's own.
                    if let Some(server) = d.late.outside.get().and_then(|o| o.server.as_ref()) {
                        server.register_operations();
                    }
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
                retention: (cfg.turn_retention_days > 0)
                    .then(|| chrono::Duration::days(cfg.turn_retention_days)),
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
        // ADR 0047: what the agents do on their own, and when.
        let jobs = crate::schedules::jobs(&store, &conversations);
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
                    let jobs = jobs.clone();
                    move || {
                        let folder_problems = lock(&folder).problems.clone();
                        let mut out = ghost_problems(
                            &*apps,
                            &folder_problems,
                            &password_problems(Some(&vault), &cfg),
                        );
                        out.extend(crate::schedules::problems(&jobs, &lock(&folder)));
                        if let Some(d) = weak.upgrade() {
                            if let Some(problem) = lock(&d.models_problem).clone() {
                                out.push((MODELS_PROBLEM_KEY.to_string(), problem));
                            }
                            if let Some(problem) = d.late.outside.get().and_then(|o| o.problem()) {
                                out.push((crate::outside::PROBLEM_KEY.to_string(), problem));
                            }
                        }
                        out
                    }
                }))
                // ADR 0056: Ghost's note about its own speed is for the screen; no route
                // and no wake says it.
                .with_notes(Arc::new({
                    let weak = Arc::downgrade(d);
                    move || {
                        let Some(d) = weak.upgrade() else {
                            return Vec::new();
                        };
                        let mut out: Vec<(String, String)> = d
                            .slow_note()
                            .map(|slow| (crate::model_check::SLOW_NOTE_KEY.to_string(), slow))
                            .into_iter()
                            .collect();
                        // ADR 0063: why the realtime voice was not used, shown, never said.
                        if let Some(note) = lock(&d.realtime_note).clone() {
                            out.push((REALTIME_NOTE_KEY.to_string(), note));
                        }
                        // ADR 0064: the microphone stopped hearing well, shown, never said.
                        if let Some(note) = d.mic_note() {
                            out.push((crate::microphone::NOTE_KEY.to_string(), note));
                        }
                        out
                    }
                }))
                .with_on_change(on_change.clone()),
        );
        d.late.waiting.set(waiting.clone()).ok();
        // ADR 0056: what each model route has shown, before the first turn.
        d.seed_capabilities();
        // ADR 0051: questions for the operator, on the waiting list. A question asked from
        // a run is followed up by a run of the same agent once it is answered.
        let questions =
            Arc::new(Questions::new(store.clone(), waiting.clone()).with_jobs(jobs.clone()));
        match questions.recover(None) {
            Ok(n) if n > 0 => info!("questions: {n} from an app, open at the last stop, declined"),
            Ok(_) => {}
            Err(e) => warn!("questions: {e}"),
        }
        questions.on_change({
            let weak = Arc::downgrade(d);
            Arc::new(move |q| {
                if let Some(d) = weak.upgrade() {
                    d.question_changed(q);
                }
            })
        });
        let _ = d.questions.set(questions.clone());
        // ADR 0047, ADR 0052: a written turn with an agent, with nobody there: what an
        // agent's `ask_agent` and an outside tool's `agent_ask` run.
        // One at a time per agent with its scheduled runs, asks first (ADR 0047).
        let ask_runner = Arc::new(
            crate::outside::AskRunner::new(
                Arc::new(crate::schedules::runner(d, &gate, &conversations, &skills)),
                d.stop.child_token(),
            )
            .with_turns(jobs.turns().clone(), ghost_core::schedules::ASK_WAIT),
        );
        // Made a moment later; Ghost's own app reads them from then on (ADR 0052).
        let late_watches: ghost_app::Late<WatchRunner> = Arc::default();
        let late_routes: ghost_app::Late<RouteEngine> = Arc::default();
        // ADR 0045: Ghost is an app its own agents use, served in process before any
        // session can open. It needs no vault, no network and no sign-in. ADR 0052: and it
        // reaches all of Ghost, through the same gate.
        let ghost_server =
            GhostServer::new(ghost_app::Parts {
                store: store.clone(),
                waiting: waiting.clone(),
                apps: Arc::downgrade(&apps),
                status: crate::ghost_app::status_fn(d),
            })
            .with_schedules(jobs.clone())
            .with_questions(questions.clone())
            .with_more(ghost_app::More {
                agents: Some(conversations.clone()),
                config_dir: Some(cfg.config_dir.clone()),
                catalog: Some(catalog.clone()),
                forget: Some(crate::ghost_app::forget_app(&apps, &signins, &passwords)),
                watches: Some(late_watches.clone()),
                refresh: Some({
                    let folder = folder.clone();
                    Arc::new(move || lock(&folder).refresh())
                }),
                routes: Some(late_routes.clone()),
                policy: Some({
                    let gate = gate.clone();
                    Arc::new(move || gate.autonomy())
                }),
                settings: Some({
                    let path = cfg.settings_path.clone();
                    Arc::new(move || ghost_core::envfile::read_env(&path).unwrap_or_default())
                }),
                skills: Some(skills.clone()),
                runner: Some(
                    ask_runner.clone() as Arc<dyn ghost_core::ghost_app::outside::AgentRunner>
                ),
            });
        let ghost_app = match ghost_app::attach(&apps, ghost_server.clone()).await {
            Ok(attached) => {
                gate.register(GHOST_APP, attached.app.operations());
                Some(attached)
            }
            Err(e) => {
                error!("app {GHOST_APP}: not started: {}", e.reason());
                None
            }
        };
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
        let _ = late_watches.set(watch_runner.clone());
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
                        // An agent's item says how loud it may get (ADR 0045); a watch's
                        // or a hook's item, what its declaration says.
                        ghost_app::urgency_of(item).unwrap_or_else(|| {
                            declared_urgency(&lock(&folder))
                                .get(&item.source)
                                .and_then(|u| Urgency::parse(u))
                                .unwrap_or(Urgency::Notify)
                        })
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
        let desktop_route = Arc::new(
            DesktopRoute::new({
                let waiting = waiting.clone();
                Arc::new(move |key: &str, now| {
                    waiting
                        .acknowledge(key, Some(now), Some(now))
                        .unwrap_or(false)
                })
            })
            // ADR 0051: a question's choices are the notification's buttons.
            .with_questions(
                {
                    let questions = questions.clone();
                    Arc::new(move |key: &str| questions.get(key).ok().flatten())
                },
                {
                    let questions = questions.clone();
                    Arc::new(move |key: &str, choice: &str| {
                        questions
                            .answer(key, choice, ghost_core::questions::via::DESKTOP, None)
                            .is_ok()
                    })
                },
            ),
        );
        routes.add_route(desktop_route.clone());
        d.phone_route.set_on_the_phone({
            let weak = Arc::downgrade(d);
            Arc::new(move || weak.upgrade().is_some_and(|d| d.on_the_phone()))
        });
        routes.add_route(d.phone_route.clone());
        // ADR 0051: the desk rings the phone, and ntfy for when the phone is off the tunnel.
        routes.add_route(Arc::new(
            CallRoute::new(d.phone_feed.clone(), Items::new(store.clone())).with_in_session({
                let weak = Arc::downgrade(d);
                Arc::new(move || weak.upgrade().is_some_and(|d| d.in_session()))
            }),
        ));
        routes.add_route(Arc::new(NtfyRoute::new({
            let weak = Arc::downgrade(d);
            Arc::new(move |name: &str| weak.upgrade().and_then(|d| d.env().get(name).cloned()))
        })));
        *lock(&d.route_names) = vec![
            RouteName::Desk,
            RouteName::Desktop,
            RouteName::Phone,
            RouteName::Call,
            RouteName::Ntfy,
        ];
        d.start_phone_feed();
        d.late.routes.set(routes.clone()).ok();
        let _ = late_routes.set(routes.clone());
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
        // ADR 0047: the schedules run from here on, beside any session, with no audio.
        let schedules = crate::schedules::start(
            d,
            jobs,
            &gate,
            &waiting,
            read_for.clone(),
            &conversations,
            &skills,
            &watch_runner,
        );
        // A run started or ended (ADR 0060): the desktop follows it without asking.
        schedules.scheduler.on_run({
            let weak = Arc::downgrade(d);
            let jobs = schedules.scheduler.jobs().clone();
            Arc::new(move |id, phase| {
                let (Some(d), Ok(Some(run))) = (weak.upgrade(), jobs.run(id)) else {
                    return;
                };
                d.state.publish_run(
                    ghost_api::schedules::run_info(&run),
                    match phase {
                        ghost_core::schedules::RunPhase::Started => {
                            ghost_api::models::RunPhase::Started
                        }
                        ghost_core::schedules::RunPhase::Ended => {
                            ghost_api::models::RunPhase::Ended
                        }
                    },
                );
            })
        });
        d.late.schedules.set(schedules.scheduler.clone()).ok();
        // ADR 0041: hidden turns past a week, and summarized ones past the window when the
        // operator set one, pruned now and then daily for as long as Ghost runs.
        tokio::spawn({
            let conversations = conversations.clone();
            let store_for_prune = store.clone();
            let stop = d.stop.child_token();
            async move {
                loop {
                    match conversations.prune() {
                        Ok(n) if n > 0 => info!("conversations: pruned {n} turns past retention"),
                        Ok(_) => {}
                        Err(e) => warn!("conversations: prune failed: {e}"),
                    }
                    // ADR 0060: the calls' counts and sizes, past their window.
                    let before = ghost_core::time::utcnow()
                        - chrono::Duration::days(ghost_core::reach::REACH_KEPT_DAYS);
                    if let Err(e) = store_for_prune.prune_reach(before) {
                        warn!("reach: prune failed: {e}");
                    }
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(PRUNE_EVERY) => {}
                    }
                }
            }
        });
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
        events.add({
            let state = d.state.clone();
            Arc::new(move |event| {
                let state = state.clone();
                Box::pin(async move {
                    // ADR 0059: "open the app" in a session; the desktop raises its window.
                    if event.kind == EventKind::ShowApp {
                        info!("desktop: the operator asked for the window");
                        state.publish_show_app();
                    }
                })
            })
        });
        events.add({
            let mic = d.mic.clone();
            Arc::new(move |event| {
                let mic = mic.clone();
                Box::pin(async move {
                    // ADR 0057: the operator's words in a session; the interceptor ends it
                    // once its line is said, and the passive loop opens no capture after.
                    if event.kind == EventKind::StopListening {
                        match mic.off(None) {
                            Ok(_) => info!("listening: off, as the operator said"),
                            Err(e) => warn!("listening: could not keep the switch: {e}"),
                        }
                    }
                })
            })
        });
        d.late.events.set(Arc::new(events)).ok();
        // ADR 0048: the same app for the outside tools the operator connected, on its own
        // listener. Made now (the API lists the clients), started after the phone's.
        let outside = Arc::new(crate::outside::OutsideHost::new(
            &cfg.mcp_listen,
            Arc::new(Clients::new(store.clone(), cfg.clients_dir())),
            ghost_app.as_ref().map(|_| &ghost_server),
            Outside::new(gate.clone())
                .with_desk(Arc::new(crate::outside::Desk(Arc::downgrade(d))))
                // ADR 0047's run with nobody there is what `agent_ask` runs.
                .with_runner(ask_runner.clone()),
        ));
        d.late.outside.set(outside.clone()).ok();
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
                schedules: schedules.scheduler.clone(),
                clients: outside.clients.clone(),
                clients_listening: outside.addresses(),
            })
            .await;
        d.start_phone_listener().await;
        outside.start(&waiting, d.state.bus.tap()).await;
        if let Err(e) = waiting.refresh(None) {
            warn!("waiting: refresh failed: {e}");
        }
        d.set_waiting_count();
        let refresher = tokio::spawn(d.clone().refresh_waiting());
        let mic_watch = tokio::spawn(d.clone().watch_mic());
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
        info!("permissions: {}", autonomy.source);
        info!("{}", describe_routes(&routes.policy()));
        info!(
            "llm: {}, thinking {}",
            voice.llm_target(Some(&env)).describe(),
            voice.llm_thinking
        );
        if let Some(fast) = voice.llm_fast_target(Some(&env)) {
            info!("llm: {} answers a spoken turn first", fast.model);
        }
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
            // Loaded, perhaps with a voice that fell back (ADR 0053): that stays on the
            // waiting list until the files are there; anything else resolves.
            let after = models.problem.clone();
            if *lock(&d.models_problem) != after {
                *lock(&d.models_problem) = after;
                d.refresh_waiting_now();
            }
            info!("models ready");
            let models = Arc::new(models);
            d.late.models.set(models.clone()).ok();
            d.state.set_models_ready(true);
            d.bind_voices(&models);
            // The acknowledgments in the default voice, before the first session (#43);
            // an agent's own voice is rendered when its session starts.
            {
                let v = d.voice();
                if v.ack && v.tts_engine != "elevenlabs" {
                    models
                        .acks
                        .warm(models.tts.clone(), &v.tts_voice, v.tts_speed);
                }
            }
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
        mic_watch.abort();
        if let Some(v) = vault_waiter {
            v.abort();
        }
        follower.abort();
        schedules.close().await;
        watch_runner.close().await;
        desktop_route.close().await;
        if let Some(h) = &hooks_listener {
            h.stop().await;
        }
        outside.stop().await;
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
        if let Some(ghost_app) = &ghost_app {
            ghost_app.stop(); // or it would be served again as the apps close
        }
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
            let mut mic = d.mic.watch();
            mic.borrow_and_update();
            // ADR 0057: with listening off no wake listener is built, so no capture is open.
            let (won, mut wake_task) = if let Listening::Off { until } = d.mic.now() {
                d.waiting_flag.store(true, Ordering::SeqCst);
                (d.wait_not_listening(until).await, None)
            } else {
                let mut wake_task: JoinHandle<Option<String>> = {
                    let w = wake.clone();
                    tokio::spawn(async move { w.wait().await })
                };
                d.waiting_flag.store(true, Ordering::SeqCst);
                let won = tokio::select! {
                    r = &mut wake_task => Won::Wake(r.unwrap_or(None)),
                    _ = d.stop.cancelled() => Won::Stop,
                    _ = d.rearm.wait() => Won::Rearm,
                    _ = mic.changed() => Won::Rearm, // turned off: the listener stops below
                    p = d.next_phone() => Won::Phone(p),
                    r = d.next_raised() => match r { Some(r) => Won::Raised(r), None => Won::Rearm },
                    l = d.next_hear() => Won::Hear(l),
                };
                (won, Some(wake_task))
            };
            d.waiting_flag.store(false, Ordering::SeqCst);
            let (mut woke, mut phone, mut raised, mut hear) = (None, None, None, None);
            match won {
                Won::Wake(r) => woke = Some(r),
                Won::Phone(p) => phone = Some(p),
                Won::Raised(r) => raised = Some(r),
                Won::Hear(l) => hear = Some(l),
                Won::Stop | Won::Rearm => {}
            }
            if woke.is_none() {
                // Decided before the cancel below, which makes a pending task done too.
                if let Some(task) = wake_task.take() {
                    if task.is_finished() {
                        woke = Some(task.await.unwrap_or(None));
                    } else {
                        task.abort();
                        let _ = task.await;
                    }
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
            if let Some(lease) = hear.take() {
                // The Hearing test (ADR 0042): a wake or a phone in the same tick came first,
                // and dropping the lease tells the recorder so.
                if woke.is_none()
                    && phone.is_none()
                    && raised.is_none()
                    && lease.granted.send(()).is_ok()
                {
                    info!("passive: the Hearing test has the mic; Ghost is not listening");
                    d.state.set_phase(Phase::Recording, None, None);
                    tokio::select! {
                        _ = lease.release => {}
                        _ = d.stop.cancelled() => {}
                    }
                    continue;
                }
            }
            if let Some(raised) = raised {
                // ADR 0032: Ghost greets at the desk and waits
                d.ports
                    .sessions
                    .run(
                        d.clone(),
                        SessionStart {
                            raised: Some(raised),
                            ..SessionStart::new(GHOST)
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
                            phone: Some(phone),
                            ..SessionStart::new(line)
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
            let start_in = lock(&d.start_in).clone();
            // Typed in the desktop app (`say`) rather than a wake or a Call: no microphone.
            let typed = std::mem::replace(&mut *lock(&d.first_via), TurnVia::Voice)
                == TurnVia::Text
                && first.is_some();
            if typed {
                d.typed_session(&wake, first, start_in).await;
                continue;
            }
            d.events().emit(EventKind::Wake, Map::new()).await;
            d.ports
                .sessions
                .run(
                    d.clone(),
                    SessionStart {
                        first_utterance: first,
                        ..SessionStart::new(start_in)
                    },
                )
                .await;
        }
    }

    /// A typed turn with no session open (2026-11-13): a typed session, which opens no
    /// microphone, with the passive listener running beside it unless listening is off
    /// (ADR 0057). The operator's name heard there ends the typed session and opens a voice
    /// session in the conversation the typed one is in; a wake while Ghost speaks, or just
    /// after, is its own voice coming back and is let go.
    pub(crate) async fn typed_session(
        self: &Arc<Self>,
        wake: &Arc<dyn WakeDetector>,
        first: Option<String>,
        start_in: String,
    ) {
        let d = self;
        let end = CancellationToken::new();
        let mut run = {
            let d = d.clone();
            let start = SessionStart {
                first_utterance: first,
                typed: true,
                end: end.clone(),
                ..SessionStart::new(start_in.clone())
            };
            tokio::spawn(async move { d.ports.sessions.run(d.clone(), start).await })
        };
        let mut ran = false;
        let mut called: Option<Option<String>> = None;
        if !d.mic.is_off() {
            info!("passive: a typed session; still listening for the wake word");
            let mut mic = d.mic.watch();
            mic.borrow_and_update();
            loop {
                wake.arm();
                let mut wait: JoinHandle<Option<String>> = {
                    let w = wake.clone();
                    tokio::spawn(async move { w.wait().await })
                };
                enum Beside {
                    Ended,
                    Wake(Option<String>),
                    Mic,
                    Stop,
                }
                let beside = tokio::select! {
                    _ = &mut run => { ran = true; Beside::Ended }
                    r = &mut wait => Beside::Wake(r.unwrap_or(None)),
                    _ = mic.changed() => Beside::Mic,
                    _ = d.stop.cancelled() => Beside::Stop,
                };
                if !matches!(beside, Beside::Wake(_)) {
                    wait.abort();
                    let _ = wait.await;
                    wake.stop().await; // releases the mic
                }
                match beside {
                    Beside::Ended | Beside::Stop => break,
                    Beside::Mic if d.mic.is_off() => break, // the session goes on, unheard
                    Beside::Mic => continue,
                    Beside::Wake(_) if d.own_voice_now() => {
                        info!("wake ignored: Ghost was speaking its typed reply");
                        continue;
                    }
                    Beside::Wake(words) => {
                        called = Some(words);
                        break;
                    }
                }
            }
        }
        // The conversation the typed session is in now (an agent may have taken it).
        let line = d.state.conversation().unwrap_or(start_in);
        if called.is_some() || d.stop.is_cancelled() {
            end.cancel();
        }
        if !ran {
            let _ = run.await;
        }
        let Some(words) = called else {
            return;
        };
        if d.stop.is_cancelled() {
            return;
        }
        info!("passive: called by name during a typed session; talking now");
        d.events().emit(EventKind::Wake, Map::new()).await;
        d.ports
            .sessions
            .run(
                d.clone(),
                SessionStart {
                    first_utterance: words,
                    ..SessionStart::new(line)
                },
            )
            .await;
    }

    /// Ghost is speaking, or stopped a moment ago (`OWN_VOICE_GRACE`).
    pub(crate) fn own_voice_now(&self) -> bool {
        self.state.phase() == Phase::Speaking
            || lock(&self.spoke_at).is_some_and(|t| t.elapsed() < OWN_VOICE_GRACE)
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
                retention: (cfg.turn_retention_days > 0)
                    .then(|| chrono::Duration::days(cfg.turn_retention_days)),
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

    /// The recorder, made once, before the API that serves it.
    fn hear_desk(self: &Arc<Self>) -> Arc<dyn ghost_api::HearPort> {
        let cfg = self.cfg();
        self.late
            .hear
            .get_or_init(|| {
                Arc::new(crate::hear::HearDesk::new(
                    Arc::downgrade(self),
                    &cfg.data_dir,
                    &cfg.config_dir,
                ))
            })
            .clone()
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
            started_env: self.started_env.clone(),
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
            hear: Some(self.hear_desk()),
            wake_takes: Some(crate::wake_takes::WakeTakesDesk::new(
                self.wake_takes(),
                Arc::downgrade(self),
            )),
            schedules: Some(p.schedules),
            clients: Some(p.clients),
            clients_listening: p.clients_listening,
            questions: self.questions(),
            in_appimage: ghost_core::ghost_app::clients::in_appimage(|k| std::env::var(k).ok()),
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

    /// The model a turn's first call goes to, when the connection names one (ADR 0050).
    pub(crate) fn fast_model(&self) -> Option<Arc<dyn ModelPort>> {
        (self.ports.fast_model)(&self.voice(), &self.env())
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

    pub fn audio_settings(&self, duck: bool) -> AudioSettings {
        let voice = self.voice();
        let cfg = self.cfg();
        AudioSettings {
            // ADR 0064: the plan's choice, a setting for a device that is gone falling back.
            input_device: self.mic_plan().session.to_setting(),
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

pub(crate) enum Won {
    Wake(Option<String>),
    Stop,
    Rearm,
    Phone(PendingPhone),
    Raised(Raised),
    /// The Hearing test's recorder wants the mic (ADR 0042).
    Hear(crate::hear::Lease),
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
    clients: Arc<Clients>,
    clients_listening: Vec<String>,
    passwords: Arc<Passwords>,
    conversations: Arc<Conversations>,
    schedules: Arc<ghost_core::schedules::Scheduler>,
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
            // Every path that defaults under the operator's home, in the temp folder.
            catalog_path: tmp.join("cache/catalog.json"),
            skills_dir: tmp.join("config/skills"),
            home_skills_dir: tmp.join("home-skills"),
            api_socket: Some(tmp.join("ghost.sock")),
            settings_path: tmp.join("config/settings.env"),
            env_path: tmp.join("config/env"),
            vault_env_path: tmp.join("no-heka"),
            infra_dir: tmp.join("infra"),
            dictation_flag: tmp.join("rt/dictation"),
            mcp_listen: String::new(), // never the operator's port from a test
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

    // --- a typed turn (2026-11-13): no microphone, the passive listener beside it -----------

    /// What a session was started with, as a typed test sees it.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct Started {
        pub start_in: String,
        pub typed: bool,
        pub first: Option<String>,
    }

    /// Records each start; a typed session stays open until the loop ends it or `release`
    /// is set (the session ending on its own), saying whether it was ended.
    pub(crate) struct TypedRecorder {
        pub starts: Mutex<Vec<Started>>,
        pub release: Arc<Event>,
        pub ended: AtomicUsize,
        pub speaking: bool,
        pub switch_to: Option<String>,
    }

    #[async_trait]
    impl SessionRunner for TypedRecorder {
        async fn run(&self, d: Arc<Daemon>, start: SessionStart) {
            self.starts.lock().unwrap().push(Started {
                start_in: start.start_in.clone(),
                typed: start.typed,
                first: start.first_utterance.clone(),
            });
            if !start.typed {
                return;
            }
            d.state
                .set_phase(Phase::Listening, Some("typed-1"), Some(SessionOrigin::Desk));
            d.state.set_conversation(Some(&start.start_in), None);
            if let Some(slug) = &self.switch_to {
                d.state.set_conversation(Some(slug), None);
            }
            if self.speaking {
                d.state
                    .set_phase(Phase::Speaking, Some("typed-1"), Some(SessionOrigin::Desk));
            }
            tokio::select! {
                _ = start.end.cancelled() => {
                    self.ended.fetch_add(1, Ordering::SeqCst);
                }
                _ = self.release.wait() => {}
            }
        }
    }

    fn typed_recorder(
        tmp: &Path,
        speaking: bool,
        switch_to: Option<&str>,
    ) -> (Arc<Daemon>, Arc<TypedRecorder>, Arc<FileWakeDetector>) {
        let recorder = Arc::new(TypedRecorder {
            starts: Mutex::new(Vec::new()),
            release: Arc::new(Event::new()),
            ended: AtomicUsize::new(0),
            speaking,
            switch_to: switch_to.map(String::from),
        });
        let mut d = Arc::try_unwrap(daemon(tmp)).ok().expect("one reference");
        d.ports.sessions = recorder.clone();
        let d = Arc::new(d);
        let manual = FileWakeDetector::new(tmp.join("rt/wake"));
        *lock(&d.manual) = Some(manual.clone());
        (d, recorder, manual)
    }

    #[tokio::test]
    async fn a_typed_turn_is_a_typed_session_and_the_wake_still_works() {
        // Typing opens no microphone: the session is typed, and the passive listener keeps
        // running beside it, so "hey Ghost" ends it and opens a voice session in the same
        // conversation.
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder, manual) = typed_recorder(tmp.path(), false, None);
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                let manual = manual.clone();
                move |n| {
                    let d = d.clone();
                    let manual = manual.clone();
                    Box::pin(async move {
                        match n {
                            1 => manual.wait().await,                       // the typed turn
                            2 => Some("read me the first one".to_string()), // hey Ghost, …
                            _ => {
                                d.stop.cancel();
                                std::future::pending().await
                            }
                        }
                    })
                }
            }),
        });
        let typist = {
            let d = d.clone();
            tokio::spawn(async move {
                use ghost_api::Controller;
                while !d.waiting_flag.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert!(d.say("what's on my list".into(), None).await);
            })
        };
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake.clone()))
            .await
            .unwrap();
        typist.await.unwrap();
        assert_eq!(
            *recorder.starts.lock().unwrap(),
            vec![
                Started {
                    start_in: GHOST.into(),
                    typed: true,
                    first: Some("what's on my list".into())
                },
                Started {
                    start_in: GHOST.into(),
                    typed: false,
                    first: Some("read me the first one".into())
                },
            ]
        );
        assert_eq!(
            recorder.ended.load(Ordering::SeqCst),
            1,
            "the wake ended it"
        );
    }

    #[tokio::test]
    async fn the_wake_during_a_typed_session_opens_the_conversation_it_is_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder, manual) = typed_recorder(tmp.path(), false, Some("legal"));
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                let manual = manual.clone();
                move |n| {
                    let d = d.clone();
                    let manual = manual.clone();
                    Box::pin(async move {
                        match n {
                            1 => manual.wait().await,
                            2 => {
                                // the typed session moved to another agent, then a bare "hey Ghost"
                                while d.state.conversation().as_deref() != Some("legal") {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                }
                                None
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
        *lock(&d.first_via) = TurnVia::Text;
        manual.trigger(Some("draft the reply".into()));
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake))
            .await
            .unwrap();
        let starts = recorder.starts.lock().unwrap().clone();
        assert_eq!(starts.len(), 2, "{starts:?}");
        assert_eq!(starts[1].start_in, "legal");
        assert!(!starts[1].typed);
    }

    #[tokio::test]
    async fn a_typed_session_ends_on_its_own_and_the_loop_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder, manual) = typed_recorder(tmp.path(), false, None);
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                let manual = manual.clone();
                let release = recorder.release.clone();
                move |n| {
                    let d = d.clone();
                    let manual = manual.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        match n {
                            1 => manual.wait().await,
                            2 => {
                                release.set(); // the reply was read; the session ends
                                std::future::pending().await
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
        *lock(&d.first_via) = TurnVia::Text;
        manual.trigger(Some("what time is it".into()));
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake.clone()))
            .await
            .unwrap();
        assert_eq!(recorder.starts.lock().unwrap().len(), 1);
        assert_eq!(recorder.ended.load(Ordering::SeqCst), 0);
        assert_eq!(wake.waits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn ghosts_own_voice_in_a_typed_session_does_not_wake_it() {
        // The reply is spoken; the passive listener hearing "Ghost" in it is not the
        // operator calling.
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder, manual) = typed_recorder(tmp.path(), true, None);
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new({
                let d = d.clone();
                let manual = manual.clone();
                let release = recorder.release.clone();
                move |n| {
                    let d = d.clone();
                    let manual = manual.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        match n {
                            1 => manual.wait().await,
                            2 => Some("can help with that".to_string()), // its own reply
                            3 => {
                                release.set();
                                std::future::pending().await
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
        *lock(&d.first_via) = TurnVia::Text;
        manual.trigger(Some("hello".into()));
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake.clone()))
            .await
            .unwrap();
        assert_eq!(recorder.starts.lock().unwrap().len(), 1);
        assert_eq!(recorder.ended.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_typed_session_while_not_listening_keeps_the_microphone_closed() {
        // ADR 0057: no wake listener beside it either.
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder, manual) = typed_recorder(tmp.path(), false, None);
        let _ = d.mic.off(None);
        assert!(d.mic.is_off());
        let wake = Arc::new(Scripted {
            waits: AtomicUsize::new(0),
            on_wait: Box::new(|_| Box::pin(std::future::pending())),
        });
        *lock(&d.first_via) = TurnVia::Text;
        let release = recorder.release.clone();
        let stopper = {
            let d = d.clone();
            let recorder = recorder.clone();
            tokio::spawn(async move {
                use ghost_api::Controller;
                while !d.waiting_flag.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert!(d.say("hello".into(), None).await);
                while recorder.starts.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                release.set();
                d.stop.cancel();
            })
        };
        let _ = manual;
        tokio::time::timeout(Duration::from_secs(5), d.passive(wake.clone()))
            .await
            .unwrap();
        stopper.await.unwrap();
        assert!(recorder.starts.lock().unwrap()[0].typed);
        assert_eq!(wake.waits.load(Ordering::SeqCst), 0, "no wake listener");
    }

    #[test]
    fn a_typed_session_opens_no_microphone() {
        assert!(!crate::session::opens_mic(true, false));
        assert!(!crate::session::opens_mic(false, true));
        assert!(!crate::session::opens_mic(true, true));
        assert!(crate::session::opens_mic(false, false));
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
        let d = Arc::try_unwrap(daemon(tmp.path()))
            .ok()
            .unwrap()
            .with_started_env(HashMap::new());
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
    fn a_detector_that_falls_back_reports_transcript_wake() {
        // ADR 0065: the desktop says wake takes are kept only under the detector, so the
        // status must not keep saying onnx once Ghost listens by transcript instead.
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        lock(&d.cfg).wake_mode = "onnx".into();
        d.state.set_wake_mode("onnx");
        d.listen_by_transcript();
        assert_eq!(d.cfg().wake_mode, "transcript");
        assert_eq!(d.state.wake_mode(), "transcript");
    }

    #[test]
    fn the_session_and_the_api_read_one_environment() {
        // ADR 0037: what the desktop is told is what a session uses. Started from a shell
        // with a model setting exported, the daemon reads it; cleared on the Model page,
        // it no longer counts for either.
        let tmp = tempfile::tempdir().unwrap();
        let started: HashMap<String, String> = [
            ("GHOST_LLM_PROVIDER".to_string(), "openai".to_string()),
            ("GHOST_LLM_MODEL".to_string(), "gpt-5-mini".to_string()),
        ]
        .into_iter()
        .collect();
        let d = Arc::try_unwrap(daemon(tmp.path()))
            .ok()
            .unwrap()
            .with_started_env(started);
        assert_eq!(d.env()["GHOST_LLM_MODEL"], "gpt-5-mini");
        let deps = d.started_env.clone();
        let live = lock(&d.environ).clone();
        assert_eq!(
            ghost_core::environment::resolve(&deps, &live),
            d.env(),
            "the API resolves what the daemon does"
        );
        let mut values = BTreeMap::new();
        values.insert("GHOST_LLM_MODEL".to_string(), None);
        assert_eq!(d.apply_model(&values), vec!["GHOST_LLM_MODEL"]);
        assert_eq!(d.voice().llm_connection().model, "");
        assert_eq!(d.voice().llm_connection().provider, "openai");
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
