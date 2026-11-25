//! The dictation key in the daemon (ADR 0077 as amended 2026-11-25): the operator's switch
//! ("Hold a key to dictate", Settings › Sound), the key bound through the desktop's shortcut
//! portal and driven through [`KeyMachine`], and what the coordinator types, tidies, shows
//! and mutes with ([`super::Parts`]).
//!
//! - The switch is off until the operator turns it on. Turning it on asks GNOME for the key
//!   (its dialog the first time) and then for typing (a second dialog, the first time); after
//!   that the daemon binds the key quietly at each start.
//! - The key asked for is the first of [`PREFERRED`] that the desktop's own bindings don't
//!   use. Ghost reads those bindings and never writes them; GNOME's dialog lets the operator
//!   pick any key, and Change key opens GNOME's own place for it.
//! - Typing's restore token is a password (ADR 0036): in the vault as [`TYPING_TOKEN`], never
//!   logged.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use ghost_api::dictation::{
    DictationKeyBody, DictationKeyPort, DictationKeyState, DictationKeyView, DictationLiveView,
    DictationReason, DictationState, DictationTyping, TidySource,
};
use ghost_core::connection::{CLAUDE_CODE, ModelConnection, Target};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::key::{Action, KeyEvent, KeyMachine};
use super::portal::{Portal, PortalError, Shortcuts};
use super::{Live, Muter, Reviewer, Typer, review, typing};
use crate::daemon::{Daemon, lock};

/// The setting's key in the store.
pub const SETTING: &str = "dictation_key";
/// Typing's restore token in the vault.
pub const TYPING_TOKEN: &str = "GHOST_DICTATION_TYPING";
/// The one-time note that the key exists (it leads to Settings › Sound).
pub const INTRO_KEY: &str = "dictation:key";
pub const INTRO: &str = "You can dictate by holding a key: Ghost types what you say where your \
cursor is. Turn it on in Settings › Sound.";

/// The keys Ghost asks for, in order: the first the desktop doesn't already use.
pub const PREFERRED: [&str; 5] = ["F9", "F10", "F8", "F7", "F6"];

/// The operator's choices, kept in the store. A choice kept before "Tidy with" (`on`, `tidy`
/// and OpenRouter's `tidy_model` only) still reads, and means what it did then
/// ([`Choice::source`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub on: bool,
    /// The tidy's switch from before "Tidy with": false is off. Kept equal to
    /// `tidy_source != off` once there is a source.
    pub tidy: bool,
    /// "Tidy with"; None until the operator chooses, read by [`Choice::source`].
    pub tidy_source: Option<TidySource>,
    /// The model the tidy asks on OpenRouter; None is [`OPENROUTER_TIDY_MODEL`].
    pub tidy_model: Option<String>,
    /// Claude Code's model for the subscription's tidy (one of [`CLAUDE_CODE_ALIASES`]); None
    /// is Ghost's own model, the fast one when set.
    pub tidy_alias: Option<String>,
}

impl Default for Choice {
    fn default() -> Self {
        Self {
            on: false,
            tidy: true,
            tidy_source: None,
            tidy_model: None,
            tidy_alias: None,
        }
    }
}

impl Choice {
    /// Where the tidy goes. A choice made before "Tidy with" reads as it behaved: off when its
    /// switch was off, OpenRouter when its key is set, else the subscription (Ghost's model).
    pub fn source(&self, openrouter_key: bool) -> TidySource {
        match self.tidy_source {
            Some(s) => s,
            None if !self.tidy => TidySource::Off,
            None if openrouter_key => TidySource::Openrouter,
            None => TidySource::Subscription,
        }
    }

    /// A change from Settings › Sound: the older switch first, then "Tidy with" and its model.
    pub fn apply(&mut self, body: &DictationKeyBody) {
        if let Some(t) = body.tidy {
            if !t {
                self.tidy_source = Some(TidySource::Off);
            } else if self.tidy_source == Some(TidySource::Off) {
                self.tidy_source = None; // back on as it was before there was a choice
            }
            self.tidy = t;
        }
        let model = body.tidy_model.as_deref().map(str::trim);
        match body.tidy_source {
            Some(source) => {
                self.tidy_source = Some(source);
                self.tidy = source != TidySource::Off;
                match (source, model) {
                    (TidySource::Openrouter, Some(m)) => self.set_openrouter_model(m),
                    (TidySource::Subscription, Some("")) => self.tidy_alias = None,
                    (TidySource::Subscription, Some(m)) if alias_ok(m) => {
                        self.tidy_alias = Some(m.to_string())
                    }
                    _ => {}
                }
            }
            // An older desktop names OpenRouter's model alone.
            None => {
                if let Some(m) = model {
                    self.set_openrouter_model(m);
                }
            }
        }
    }

    fn set_openrouter_model(&mut self, m: &str) {
        if m.is_empty() || m == OPENROUTER_TIDY_MODEL {
            self.tidy_model = None;
        } else if tidy_model_ok(m) {
            self.tidy_model = Some(m.to_string());
        }
    }
}

/// Claude Code's own names for its models, which `claude --model` takes.
pub const CLAUDE_CODE_ALIASES: [&str; 3] = ["haiku", "sonnet", "opus"];

pub fn alias_ok(m: &str) -> bool {
    CLAUDE_CODE_ALIASES.contains(&m)
}

/// Claude Code asked with `alias`: Ghost's connection with its model replaced when Ghost runs
/// Claude Code, else a Claude Code connection of its own. Its `--model` takes the alias.
pub fn claude_code_target(env: &HashMap<String, String>, alias: &str) -> Target {
    let ghost = ModelConnection::from_env(env);
    if ghost.provider == CLAUDE_CODE {
        let mut t = ghost.target(env);
        t.model = alias.to_string();
        return t;
    }
    let own: HashMap<&str, &str> = [
        ("GHOST_LLM_PROVIDER", CLAUDE_CODE),
        ("GHOST_LLM_MODEL", alias),
    ]
    .into_iter()
    .collect();
    ModelConnection::from_env(&own).target(&own)
}

/// Where the tidy goes for `choice`: OpenRouter with its key, Claude Code with the chosen
/// alias, or Ghost's own model (`ghost`, asked only then). None when the tidy is off or has
/// nothing to go to (OpenRouter without its key, no model set up): the text is typed as read.
pub fn tidy_target(
    choice: &Choice,
    env: &HashMap<String, String>,
    ghost: impl FnOnce() -> Option<Target>,
) -> Option<Target> {
    let openrouter = openrouter_target(env, choice.tidy_model.as_deref());
    match choice.source(openrouter.is_some()) {
        TidySource::Off => None,
        TidySource::Openrouter => openrouter,
        TidySource::Subscription => match choice.tidy_alias.as_deref().filter(|a| alias_ok(a)) {
            Some(alias) => Some(claude_code_target(env, alias)),
            None => ghost(),
        },
    }
}

/// Ghost's own model for the tidy: the fast one when set, else the main one; None when the
/// connection isn't set up.
fn ghost_target(
    voice: &ghost_voice::config::VoiceSettings,
    env: &HashMap<String, String>,
) -> Option<Target> {
    if !voice.llm_problems(Some(env)).is_empty() {
        return None;
    }
    Some(
        voice
            .llm_fast_target(Some(env))
            .unwrap_or_else(|| voice.llm_target(Some(env))),
    )
}

/// The password that turns the tidy to OpenRouter (ADR 0078): the same one OpenRouter's
/// sign-in keeps in the vault.
pub const OPENROUTER_KEY: &str = "OPENROUTER_API_KEY";
/// OpenRouter's OpenAI-compatible address.
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
/// The tidy's model on OpenRouter unless the operator names another: a small, quick one
/// (listed by OpenRouter's `/api/v1/models` on 2026-11-25).
pub const OPENROUTER_TIDY_MODEL: &str = "anthropic/claude-haiku-5.5";

/// A model id as OpenRouter writes them (`anthropic/claude-haiku-5.5`, `meta-llama/llama-3.1-8b-instruct:nitro`).
pub fn tidy_model_ok(m: &str) -> bool {
    !m.is_empty()
        && m.len() <= 100
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/~-".contains(c))
}

/// Where the tidy goes on OpenRouter, when `env` holds its key: an OpenAI-protocol connection
/// to [`OPENROUTER_URL`] with `model`. None without the key.
pub fn openrouter_target(env: &HashMap<String, String>, model: Option<&str>) -> Option<Target> {
    let key = env.get(OPENROUTER_KEY).filter(|k| !k.trim().is_empty())?;
    let model = model
        .filter(|m| tidy_model_ok(m))
        .unwrap_or(OPENROUTER_TIDY_MODEL);
    let conn_env: HashMap<&str, &str> = [
        ("GHOST_LLM_PROVIDER", "custom"),
        ("GHOST_LLM_PROTOCOL", "openai"),
        ("GHOST_LLM_BASE_URL", OPENROUTER_URL),
        ("GHOST_LLM_PASSWORD", OPENROUTER_KEY),
        ("GHOST_LLM_MODEL", model),
        (OPENROUTER_KEY, key.as_str()),
    ]
    .into_iter()
    .collect();
    let mut target = ModelConnection::from_env(&conn_env).target(&conn_env);
    target.provider = "openrouter".into();
    Some(target)
}

struct Status {
    state: DictationKeyState,
    key: Option<String>,
    typing: DictationTyping,
}

/// The bound key: the portal it was bound on, and the loop reading it.
struct Bound {
    portal: Arc<Portal>,
    shortcuts: Shortcuts,
    task: tokio::task::JoinHandle<()>,
}

pub struct KeyDesk {
    d: Weak<Daemon>,
    status: Mutex<Status>,
    choice: Mutex<Choice>,
    bound: tokio::sync::Mutex<Option<Bound>>,
}

impl KeyDesk {
    pub fn new(d: Weak<Daemon>) -> Arc<Self> {
        Arc::new(Self {
            d,
            status: Mutex::new(Status {
                state: DictationKeyState::Off,
                key: None,
                typing: DictationTyping::Unknown,
            }),
            choice: Mutex::new(Choice::default()),
            bound: tokio::sync::Mutex::new(None),
        })
    }

    fn items(&self) -> Option<ghost_core::waiting::Items> {
        let d = self.d.upgrade()?;
        d.late
            .store
            .get()
            .map(|s| ghost_core::waiting::Items::new(s.clone()))
    }

    /// Read the operator's choice and, when the key is on, bind it quietly. At the daemon's
    /// start; a sound left muted by a crash is put back first.
    pub async fn start(self: &Arc<Self>) {
        SinkMute::recover();
        let choice = self
            .items()
            .and_then(|i| i.setting(SETTING).ok().flatten())
            .and_then(|s| serde_json::from_str::<Choice>(&s).ok())
            .unwrap_or_default();
        let on = choice.on;
        *lock(&self.choice) = choice;
        if on {
            let me = self.clone();
            tokio::spawn(async move { me.enable(false).await });
        }
    }

    pub fn choice(&self) -> Choice {
        lock(&self.choice).clone()
    }

    /// The one-time note: while the switch is off, on a desktop not known to lack the portal.
    /// Dismissed on the waiting list like any note.
    pub fn intro_note(&self) -> Option<String> {
        let off = !self.choice().on;
        let possible = lock(&self.status).state != DictationKeyState::Unavailable;
        (off && possible).then(|| INTRO.to_string())
    }

    fn save(&self, choice: Choice) {
        *lock(&self.choice) = choice.clone();
        if let Some(i) = self.items()
            && let Ok(s) = serde_json::to_string(&choice)
            && let Err(e) = i.set_setting(SETTING, &s)
        {
            warn!("dictation key: the choice could not be kept ({e})");
        }
    }

    fn set_state(&self, state: DictationKeyState) {
        lock(&self.status).state = state;
    }

    /// Bind the key and, the first time, ask for typing. `asking`: the operator turned the
    /// switch on now, so GNOME's dialogs are expected; at a start it binds quietly.
    async fn enable(self: &Arc<Self>, asking: bool) {
        let mut bound = self.bound.lock().await;
        if bound.is_some() {
            return;
        }
        self.set_state(DictationKeyState::Binding);
        let portal = match Portal::connect().await {
            Ok(p) => Arc::new(p),
            Err(e) => return self.failed(e),
        };
        let preferred = free_key(&taken_keys().await);
        let shortcuts = match portal.bind(preferred).await {
            Ok(s) => s,
            Err(e) => return self.failed(e),
        };
        info!("dictation key: bound");
        {
            let mut st = lock(&self.status);
            st.state = DictationKeyState::Ready;
            st.key = shortcuts.trigger.clone();
        }
        // Typing: asked once, at the switch; after that the token restores it quietly.
        let has_token = self
            .d
            .upgrade()
            .and_then(|d| d.passwords())
            .is_some_and(|p| p.contains(TYPING_TOKEN));
        if has_token {
            lock(&self.status).typing = DictationTyping::Allowed;
        } else if asking {
            // Only a no at the dialog is a refusal; a dialog left unanswered, or a failure,
            // leaves it unasked, and GNOME asks again at the first typing.
            match portal.grant_typing(None).await {
                Ok(next) => {
                    self.keep_token(next).await;
                    lock(&self.status).typing = DictationTyping::Allowed;
                }
                Err(PortalError::Refused) => lock(&self.status).typing = DictationTyping::Refused,
                Err(_) => {
                    info!("dictation key: typing was not answered; GNOME asks at the first typing");
                    lock(&self.status).typing = DictationTyping::Unknown;
                }
            }
        }
        let task = tokio::spawn(key_loop(
            self.d.clone(),
            portal.clone(),
            shortcuts.session.clone(),
        ));
        *bound = Some(Bound {
            portal,
            shortcuts,
            task,
        });
    }

    fn failed(&self, e: PortalError) {
        let state = match e {
            PortalError::Unavailable => DictationKeyState::Unavailable,
            PortalError::Refused => DictationKeyState::Refused,
            PortalError::Failed(kind) => {
                warn!("dictation key: the desktop's portal failed ({kind})");
                DictationKeyState::Failed
            }
        };
        info!("dictation key: not bound ({state:?})");
        self.set_state(state);
    }

    async fn disable(&self) {
        if let Some(b) = self.bound.lock().await.take() {
            b.task.abort();
            b.portal.unbind(b.shortcuts).await;
        }
        let mut st = lock(&self.status);
        st.state = DictationKeyState::Off;
        st.key = None;
        info!("dictation key: off");
    }

    async fn keep_token(&self, next: Option<String>) {
        let (Some(next), Some(vault)) = (next, self.d.upgrade().and_then(|d| d.passwords())) else {
            return;
        };
        let next = Zeroizing::new(next);
        let set = tokio::task::spawn_blocking(move || vault.set(TYPING_TOKEN, &next)).await;
        if !matches!(set, Ok(Ok(_))) {
            warn!("dictation key: typing's permission could not be kept; GNOME will ask again");
        }
    }

    fn token(&self) -> Option<Zeroizing<String>> {
        self.d.upgrade()?.passwords()?.value(TYPING_TOKEN)
    }

    /// Type through the bound portal, the token rotated.
    async fn type_keysyms(&self, keysyms: &[i32]) -> Result<(), DictationReason> {
        let portal = {
            let b = self.bound.lock().await;
            b.as_ref()
                .map(|b| b.portal.clone())
                .ok_or(DictationReason::TypingFailed)?
        };
        let token = self.token();
        match portal
            .type_keysyms(keysyms, token.as_deref().map(|t| t.as_str()))
            .await
        {
            Ok(next) => {
                lock(&self.status).typing = DictationTyping::Allowed;
                self.keep_token(next).await;
                Ok(())
            }
            Err(PortalError::Refused) => {
                lock(&self.status).typing = DictationTyping::Refused;
                Err(DictationReason::TypingFailed)
            }
            Err(_) => Err(DictationReason::TypingFailed),
        }
    }

    /// The provider the tidy's words go to, by the operator's choice; None when it is off or
    /// has nothing to go to.
    fn tidy_with(&self, choice: &Choice, env: &HashMap<String, String>) -> Option<String> {
        let d = self.d.upgrade()?;
        let voice = d.voice();
        let target = tidy_target(choice, env, || ghost_target(&voice, env))?;
        Some(
            ghost_core::connection::provider(&target.provider)
                .map(|p| p.title.to_string())
                .unwrap_or(target.provider),
        )
    }
}

impl KeyDesk {
    pub fn view(&self) -> DictationKeyView {
        let choice = self.choice();
        let env = self.d.upgrade().map(|d| d.env()).unwrap_or_default();
        let openrouter = openrouter_target(&env, None).is_some();
        let source = choice.source(openrouter);
        let ghost = ModelConnection::from_env(&env);
        let on_claude_code = ghost.provider == CLAUDE_CODE;
        let tidy_model = match source {
            TidySource::Off => String::new(),
            TidySource::Openrouter => choice
                .tidy_model
                .clone()
                .unwrap_or_else(|| OPENROUTER_TIDY_MODEL.into()),
            // No alias chosen: Ghost's own model, named when it is one of Claude Code's.
            TidySource::Subscription => choice.tidy_alias.clone().unwrap_or_else(|| {
                let m = ghost.wire_model();
                if on_claude_code && alias_ok(&m) {
                    m
                } else {
                    String::new()
                }
            }),
        };
        let subscription = on_claude_code
            || ghost_core::subscriptions::which(ghost_core::other_tools::program::BINARY, &env)
                .is_some();
        let tidy_with = self.tidy_with(&choice, &env);
        let st = lock(&self.status);
        DictationKeyView {
            on: choice.on,
            state: st.state,
            key: st.key.clone(),
            typing: st.typing,
            tidy: source != TidySource::Off,
            tidy_with,
            tidy_source: source,
            tidy_model,
            tidy_model_default: OPENROUTER_TIDY_MODEL.into(),
            openrouter,
            subscription,
        }
    }

    pub async fn set(self: &Arc<Self>, body: DictationKeyBody) -> DictationKeyView {
        let mut choice = self.choice();
        choice.apply(&body);
        let turned = body.on.filter(|&on| on != choice.on);
        if let Some(on) = body.on {
            choice.on = on;
        }
        self.save(choice);
        match turned {
            Some(true) => self.enable(true).await,
            Some(false) => self.disable().await,
            None => {}
        }
        self.view()
    }

    pub async fn change(&self) -> DictationKeyView {
        let configured = {
            let b = self.bound.lock().await;
            match b.as_ref() {
                Some(b) => b.portal.configure(&b.shortcuts).await,
                None => Err(PortalError::Unavailable),
            }
        };
        if configured.is_err() {
            info!("dictation key: the desktop has no place to change it from here");
        }
        self.view()
    }
}

/// The control API's side of the key.
pub struct KeyPort(pub Arc<KeyDesk>);

#[async_trait]
impl DictationKeyPort for KeyPort {
    fn view(&self) -> DictationKeyView {
        self.0.view()
    }
    async fn set(&self, body: DictationKeyBody) -> DictationKeyView {
        self.0.set(body).await
    }
    async fn change(&self) -> DictationKeyView {
        self.0.change().await
    }
}

/// The key's presses and releases through the hold-or-tap machine into the coordinator.
async fn key_loop(d: Weak<Daemon>, portal: Arc<Portal>, session: zbus::zvariant::OwnedObjectPath) {
    let origin = Instant::now();
    let events = match portal.key_events(session, origin).await {
        Ok(e) => e,
        Err(_) => {
            warn!("dictation key: the key's events could not be followed");
            return;
        }
    };
    tokio::pin!(events);
    let Some(dictation) = d.upgrade().map(|d| d.dictation()) else {
        return;
    };
    let mut decided = dictation.decided();
    // A start that did not record: the machine waits the key out.
    let (unstarted_tx, mut unstarted) = mpsc::unbounded_channel::<()>();
    let mut m = KeyMachine::new();
    loop {
        let tick = m
            .deadline()
            .map(|t| tokio::time::Instant::from_std(origin + t));
        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(ev) => m.on(ev),
                None => {
                    warn!("dictation key: the desktop stopped sending the key");
                    return;
                }
            },
            _ = async { tokio::time::sleep_until(tick.unwrap()).await }, if tick.is_some() => {
                m.on(KeyEvent::Tick(origin.elapsed()))
            }
            changed = decided.changed() => {
                if changed.is_err() {
                    return;
                }
                if m.active() && dictation.state() == DictationState::Idle {
                    m.ended(origin.elapsed()); // the limit, a cancel, a failure
                }
                None
            }
            Some(()) = unstarted.recv() => {
                m.ended(origin.elapsed());
                None
            }
        };
        match action {
            Some(Action::Start) => {
                let dictation = dictation.clone();
                let tx = unstarted_tx.clone();
                tokio::spawn(async move {
                    let v = dictation.start_key().await;
                    if v.state != DictationState::Recording {
                        let _ = tx.send(());
                    }
                });
            }
            Some(Action::Stop) => {
                let dictation = dictation.clone();
                tokio::spawn(async move {
                    dictation.stop().await;
                });
            }
            None => {}
        }
    }
}

// --- which keys the desktop already uses --------------------------------------------------

/// The bare keys (no modifier) in `gsettings` output: every quoted binding without `<`.
pub fn bare_keys(output: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for part in output.split('\'').skip(1).step_by(2) {
        let b = part.trim();
        if !b.is_empty() && !b.contains('<') {
            out.insert(b.to_string());
        }
    }
    out
}

/// The first of [`PREFERRED`] not in `taken`, or the first of them when every one is.
pub fn free_key(taken: &HashSet<String>) -> &'static str {
    PREFERRED
        .iter()
        .find(|k| !taken.contains(**k))
        .copied()
        .unwrap_or(PREFERRED[0])
}

/// The bare keys GNOME's own bindings use: the window manager's, the shell's, the media
/// keys' and the operator's custom shortcuts. Read with `gsettings`, never written; nothing
/// when it isn't there.
async fn taken_keys() -> HashSet<String> {
    tokio::task::spawn_blocking(|| {
        let run = |args: &[&str]| -> String {
            std::process::Command::new("gsettings")
                .args(args)
                .env_clear()
                .envs(crate::passwords::child_env())
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        let mut all = String::new();
        for schema in [
            "org.gnome.desktop.wm.keybindings",
            "org.gnome.shell.keybindings",
            "org.gnome.mutter.keybindings",
            "org.gnome.settings-daemon.plugins.media-keys",
        ] {
            all.push_str(&run(&["list-recursively", schema]));
        }
        let customs = run(&[
            "get",
            "org.gnome.settings-daemon.plugins.media-keys",
            "custom-keybindings",
        ]);
        for path in customs.split('\'').skip(1).step_by(2) {
            let schema =
                format!("org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{path}");
            all.push_str(&run(&["get", &schema, "binding"]));
        }
        bare_keys(&all)
    })
    .await
    .unwrap_or_default()
}

// --- what the coordinator uses ------------------------------------------------------------

/// Types through the key's portal.
pub struct PortalTyper(pub Weak<KeyDesk>);

#[async_trait]
impl Typer for PortalTyper {
    async fn type_text(&self, text: &str) -> Result<(), DictationReason> {
        let desk = self.0.upgrade().ok_or(DictationReason::TypingFailed)?;
        let keysyms: Vec<i32> = text.chars().filter_map(typing::keysym).collect();
        desk.type_keysyms(&keysyms).await
    }
}

/// The operator's model for the tidy, by their switch.
pub struct ModelReviewer {
    pub d: Weak<Daemon>,
    pub desk: Weak<KeyDesk>,
}

#[async_trait]
impl Reviewer for ModelReviewer {
    async fn review(&self, text: &str) -> Option<Zeroizing<String>> {
        let choice = self.desk.upgrade()?.choice();
        let d = self.d.upgrade()?;
        let env = d.env();
        // By the operator's "Tidy with" (ADR 0078): OpenRouter, Claude Code with the model they
        // chose, or Ghost's own model. The same budget and guard-rails apply to each.
        let target = tidy_target(&choice, &env, || ghost_target(&d.voice(), &env))?;
        let answer = if let Some(cc) = crate::daemon::claude_code_for(&target, &env) {
            cc.complete(review::SYSTEM, text).await.ok()
        } else {
            let model = ghost_llm::model::shared(&target).ok()?;
            model
                .complete(review::SYSTEM, text, review::MAX_TOKENS)
                .await
                .ok()
        };
        answer.map(Zeroizing::new)
    }
}

/// The live view to the operator's desktop only (its own channel, not the bus).
pub struct DeskLive(pub Weak<Daemon>);

impl Live for DeskLive {
    fn show(&self, view: DictationLiveView) {
        if let Some(d) = self.0.upgrade() {
            d.state.publish_dictation(&view);
        }
    }
}

/// The default output muted with `pactl` while recording and its earlier state put back, on
/// one worker thread so a mute and its restore never pass each other. What it changed is
/// kept in a file in the runtime folder, so a crash's mute is put back at the next start.
pub struct SinkMute {
    tx: std::sync::mpsc::Sender<bool>,
}

#[derive(Serialize, Deserialize)]
struct Muted {
    sink: String,
}

impl SinkMute {
    fn state_file() -> std::path::PathBuf {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        runtime.join("ghost").join("dictation-mute.json")
    }

    fn pactl(args: &[&str]) -> Option<String> {
        std::process::Command::new("pactl")
            .args(args)
            .env_clear()
            .envs(crate::passwords::child_env())
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::Builder::new()
            .name("ghost-dictation-mute".into())
            .spawn(move || {
                while let Ok(mute) = rx.recv() {
                    if mute {
                        Self::mute_now();
                    } else {
                        Self::restore_now();
                    }
                }
            })
            .ok();
        Self { tx }
    }

    fn mute_now() {
        let Some(sink) = Self::pactl(&["get-default-sink"]).filter(|s| !s.is_empty()) else {
            return;
        };
        let was = Self::pactl(&["get-sink-mute", &sink]).unwrap_or_default();
        if was.contains("yes") {
            return; // already muted: nothing to put back
        }
        let file = Self::state_file();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(&Muted { sink: sink.clone() }) {
            let _ = std::fs::write(&file, s);
        }
        let _ = Self::pactl(&["set-sink-mute", &sink, "1"]);
    }

    fn restore_now() {
        let file = Self::state_file();
        let Some(m) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Muted>(&s).ok())
        else {
            return;
        };
        let _ = Self::pactl(&["set-sink-mute", &m.sink, "0"]);
        let _ = std::fs::remove_file(&file);
    }

    /// A mute a crash left: put back.
    pub fn recover() {
        if Self::state_file().exists() {
            info!("dictation: putting back the sound a stopped Ghost left muted");
            Self::restore_now();
        }
    }
}

impl Default for SinkMute {
    fn default() -> Self {
        Self::new()
    }
}

impl Muter for SinkMute {
    fn mute(&self) {
        let _ = self.tx.send(true);
    }
    fn restore(&self) {
        let _ = self.tx.send(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_keys_are_read_from_gsettings_output_and_combinations_are_not() {
        let out = "org.gnome.desktop.wm.keybindings close ['<Alt>F4']\n\
                   org.gnome.settings-daemon.plugins.media-keys screenshot ['Print']\n\
                   org.gnome.desktop.wm.keybindings toggle-fullscreen ['F11', '<Super>Up']\n\
                   'F9'\n\
                   org.gnome.desktop.wm.keybindings minimize @as []\n";
        let keys = bare_keys(out);
        assert!(keys.contains("Print"));
        assert!(keys.contains("F11"));
        assert!(keys.contains("F9"));
        assert!(!keys.iter().any(|k| k.contains('<')), "{keys:?}");
    }

    #[test]
    fn the_first_free_key_is_asked_for() {
        let none = HashSet::new();
        assert_eq!(free_key(&none), "F9");
        let f9: HashSet<String> = ["F9".to_string()].into();
        assert_eq!(free_key(&f9), "F10");
        let all: HashSet<String> = PREFERRED.iter().map(|s| s.to_string()).collect();
        assert_eq!(free_key(&all), "F9", "GNOME's dialog settles it then");
    }

    #[test]
    fn the_switch_is_off_and_the_tidy_on_until_the_operator_says() {
        assert_eq!(
            Choice::default(),
            Choice {
                on: false,
                tidy: true,
                tidy_source: None,
                tidy_model: None,
                tidy_alias: None,
            }
        );
        let old: Choice = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Choice::default());
        let before: Choice = serde_json::from_str(r#"{"on":true,"tidy":false}"#).unwrap();
        assert_eq!(before.tidy_model, None);
    }

    #[test]
    fn a_choice_kept_before_tidy_with_reads_as_it_behaved() {
        let read = |json: &str| serde_json::from_str::<Choice>(json).unwrap();
        // Off stays off, with the key or without.
        let off = read(r#"{"on":true,"tidy":false}"#);
        assert_eq!(off.source(true), TidySource::Off);
        assert_eq!(off.source(false), TidySource::Off);
        // On: OpenRouter when its key is set, else the subscription.
        let on = read(r#"{"on":true,"tidy":true,"tidy_model":"google/gemini-3.5-flash-lite"}"#);
        assert_eq!(on.source(true), TidySource::Openrouter);
        assert_eq!(on.source(false), TidySource::Subscription);
        assert_eq!(
            on.tidy_model.as_deref(),
            Some("google/gemini-3.5-flash-lite")
        );
        // A choice made is kept as made, whatever the key.
        let chosen = read(r#"{"tidy":true,"tidy_source":"subscription","tidy_alias":"sonnet"}"#);
        assert_eq!(chosen.source(true), TidySource::Subscription);
        assert_eq!(chosen.tidy_alias.as_deref(), Some("sonnet"));
        // And it goes back into the store the way it came out.
        let json = serde_json::to_string(&chosen).unwrap();
        assert!(json.contains(r#""tidy_source":"subscription""#), "{json}");
        assert_eq!(read(&json), chosen);
    }

    #[test]
    fn tidy_with_is_set_from_settings_and_the_older_switch_still_works() {
        let body = |source: Option<TidySource>, model: Option<&str>| DictationKeyBody {
            tidy_source: source,
            tidy_model: model.map(String::from),
            ..Default::default()
        };
        let mut c = Choice::default();
        c.apply(&body(Some(TidySource::Subscription), Some("haiku")));
        assert_eq!(
            (c.tidy_source, c.tidy, c.tidy_alias.as_deref()),
            (Some(TidySource::Subscription), true, Some("haiku"))
        );
        // Not one of Claude Code's names: kept as it was.
        c.apply(&body(Some(TidySource::Subscription), Some("gpt-6")));
        assert_eq!(c.tidy_alias.as_deref(), Some("haiku"));
        c.apply(&body(
            Some(TidySource::Openrouter),
            Some("meta-llama/llama-3.1-8b-instruct:nitro"),
        ));
        assert_eq!(c.source(false), TidySource::Openrouter);
        assert_eq!(
            c.tidy_model.as_deref(),
            Some("meta-llama/llama-3.1-8b-instruct:nitro")
        );
        assert_eq!(
            c.tidy_alias.as_deref(),
            Some("haiku"),
            "each source keeps its own model"
        );
        c.apply(&body(Some(TidySource::Openrouter), Some("")));
        assert_eq!(c.tidy_model, None, "empty is the default");
        c.apply(&body(Some(TidySource::Off), None));
        assert_eq!((c.source(true), c.tidy), (TidySource::Off, false));
        // The older switch: on again is as before there was a choice.
        c.apply(&DictationKeyBody {
            tidy: Some(true),
            ..Default::default()
        });
        assert_eq!((c.tidy_source, c.tidy), (None, true));
        c.apply(&DictationKeyBody {
            tidy: Some(false),
            ..Default::default()
        });
        assert_eq!(c.source(false), TidySource::Off);
        // An older desktop naming OpenRouter's model alone changes only that.
        let mut c = Choice::default();
        c.apply(&body(None, Some("google/gemini-3.8-flash")));
        assert_eq!(
            (c.tidy_source, c.tidy_model.as_deref()),
            (None, Some("google/gemini-3.8-flash"))
        );
    }

    #[test]
    fn the_tidy_goes_where_the_choice_says() {
        let key = (
            OPENROUTER_KEY.to_string(),
            "sk-or-fixture-0123456789".to_string(),
        );
        let with_key: HashMap<String, String> = [key.clone()].into();
        let no_key: HashMap<String, String> = HashMap::new();
        let on_cc: HashMap<String, String> = [
            key,
            ("GHOST_LLM_PROVIDER".to_string(), CLAUDE_CODE.to_string()),
            ("GHOST_LLM_MODEL".to_string(), "opus".to_string()),
            ("GHOST_LLM_TIMEOUT_SECS".to_string(), "40".to_string()),
        ]
        .into();
        let ghosts = || {
            let env: HashMap<&str, &str> = [
                ("GHOST_LLM_PROVIDER", "anthropic"),
                ("ANTHROPIC_API_KEY", "x"),
            ]
            .into();
            Some(ModelConnection::from_env(&env).target(&env))
        };
        let never = || -> Option<Target> { panic!("Ghost's own model is not asked") };
        let choice = |source: Option<TidySource>, alias: Option<&str>| Choice {
            tidy_source: source,
            tidy_alias: alias.map(String::from),
            ..Choice::default()
        };

        // Off: nowhere.
        assert!(tidy_target(&choice(Some(TidySource::Off), None), &with_key, never).is_none());
        // OpenRouter: with its key only; without it, the text as read, never another model.
        let t = tidy_target(
            &choice(Some(TidySource::Openrouter), None),
            &with_key,
            never,
        )
        .unwrap();
        assert_eq!(
            (t.provider.as_str(), t.model.as_str()),
            ("openrouter", OPENROUTER_TIDY_MODEL)
        );
        assert!(tidy_target(&choice(Some(TidySource::Openrouter), None), &no_key, never).is_none());
        // The subscription with a chosen model: Claude Code with that alias, even beside
        // OpenRouter's key and when Ghost's model is another provider.
        let t = tidy_target(
            &choice(Some(TidySource::Subscription), Some("haiku")),
            &with_key,
            never,
        )
        .unwrap();
        assert_eq!(
            (t.provider.as_str(), t.model.as_str()),
            (CLAUDE_CODE, "haiku")
        );
        // When Ghost runs Claude Code: its connection, the model replaced.
        let t = tidy_target(
            &choice(Some(TidySource::Subscription), Some("sonnet")),
            &on_cc,
            never,
        )
        .unwrap();
        assert_eq!(
            (t.provider.as_str(), t.model.as_str(), t.timeout_secs),
            (CLAUDE_CODE, "sonnet", Some(40.0))
        );
        // No model chosen: Ghost's own.
        let t = tidy_target(
            &choice(Some(TidySource::Subscription), None),
            &with_key,
            ghosts,
        )
        .unwrap();
        assert_eq!(t.provider, "anthropic");
        assert!(
            tidy_target(
                &choice(Some(TidySource::Subscription), None),
                &no_key,
                || None
            )
            .is_none()
        );
        // A choice kept from before: OpenRouter with the key, Ghost's model without.
        assert_eq!(
            tidy_target(&choice(None, None), &with_key, never)
                .unwrap()
                .provider,
            "openrouter"
        );
        assert_eq!(
            tidy_target(&choice(None, None), &no_key, ghosts)
                .unwrap()
                .provider,
            "anthropic"
        );
        // An alias that isn't one is never put on the program's command line.
        assert_eq!(
            tidy_target(
                &choice(Some(TidySource::Subscription), Some("--dangerous")),
                &no_key,
                ghosts
            )
            .unwrap()
            .provider,
            "anthropic"
        );
    }

    #[test]
    fn the_tidy_goes_to_openrouter_only_with_its_key() {
        let none: HashMap<String, String> = HashMap::new();
        assert!(openrouter_target(&none, None).is_none());
        let blank: HashMap<String, String> = [(OPENROUTER_KEY.to_string(), " ".to_string())].into();
        assert!(openrouter_target(&blank, None).is_none());
        let env: HashMap<String, String> = [
            (
                OPENROUTER_KEY.to_string(),
                "sk-or-fixture-0123456789".to_string(),
            ),
            ("GHOST_LLM_PROVIDER".to_string(), "anthropic".to_string()),
        ]
        .into();
        let t = openrouter_target(&env, None).unwrap();
        assert_eq!(t.provider, "openrouter");
        assert_eq!(t.base_url, OPENROUTER_URL);
        assert_eq!(t.protocol, ghost_core::connection::Protocol::Openai);
        assert_eq!(t.model, OPENROUTER_TIDY_MODEL);
        assert_eq!(t.api_key.as_str(), "sk-or-fixture-0123456789");
        let t = openrouter_target(&env, Some("meta-llama/llama-3.1-8b-instruct:nitro")).unwrap();
        assert_eq!(t.model, "meta-llama/llama-3.1-8b-instruct:nitro");
        // A model id that isn't one falls back to the default, never onto the wire.
        let t = openrouter_target(&env, Some("x\nAuthorization: y")).unwrap();
        assert_eq!(t.model, OPENROUTER_TIDY_MODEL);
        assert!(!tidy_model_ok("has space"));
        assert!(tidy_model_ok("~anthropic/claude-haiku-latest"));
    }
}
