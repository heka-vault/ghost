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

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use ghost_api::dictation::{
    DictationKeyBody, DictationKeyPort, DictationKeyState, DictationKeyView, DictationLiveView,
    DictationReason, DictationState, DictationTyping,
};
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

/// The operator's choices, kept in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub on: bool,
    pub tidy: bool,
}

impl Default for Choice {
    fn default() -> Self {
        Self {
            on: false,
            tidy: true,
        }
    }
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
        *lock(&self.choice) = choice;
        if choice.on {
            let me = self.clone();
            tokio::spawn(async move { me.enable(false).await });
        }
    }

    pub fn choice(&self) -> Choice {
        *lock(&self.choice)
    }

    /// The one-time note: while the switch is off, on a desktop not known to lack the portal.
    /// Dismissed on the waiting list like any note.
    pub fn intro_note(&self) -> Option<String> {
        let off = !self.choice().on;
        let possible = lock(&self.status).state != DictationKeyState::Unavailable;
        (off && possible).then(|| INTRO.to_string())
    }

    fn save(&self, choice: Choice) {
        *lock(&self.choice) = choice;
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
            match portal.grant_typing(None).await {
                Ok(next) => {
                    self.keep_token(next).await;
                    lock(&self.status).typing = DictationTyping::Allowed;
                }
                Err(_) => lock(&self.status).typing = DictationTyping::Refused,
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

    /// The provider the tidy's words go to, when a model is set.
    fn tidy_with(&self) -> Option<String> {
        let d = self.d.upgrade()?;
        let env = d.env();
        let voice = d.voice();
        if !voice.llm_problems(Some(&env)).is_empty() {
            return None;
        }
        let target = voice
            .llm_fast_target(Some(&env))
            .unwrap_or_else(|| voice.llm_target(Some(&env)));
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
        let st = lock(&self.status);
        DictationKeyView {
            on: choice.on,
            state: st.state,
            key: st.key.clone(),
            typing: st.typing,
            tidy: choice.tidy,
            tidy_with: self.tidy_with(),
        }
    }

    pub async fn set(self: &Arc<Self>, body: DictationKeyBody) -> DictationKeyView {
        let mut choice = self.choice();
        if let Some(t) = body.tidy {
            choice.tidy = t;
        }
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
        if !self.desk.upgrade()?.choice().tidy {
            return None;
        }
        let d = self.d.upgrade()?;
        let env = d.env();
        let voice = d.voice();
        if !voice.llm_problems(Some(&env)).is_empty() {
            return None;
        }
        let target = voice
            .llm_fast_target(Some(&env))
            .unwrap_or_else(|| voice.llm_target(Some(&env)));
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
                tidy: true
            }
        );
        let old: Choice = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Choice::default());
    }
}
