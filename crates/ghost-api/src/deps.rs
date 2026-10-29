//! What the routes read: the daemon's parts, each behind an `Arc`, the optional ones
//! `None` when the daemon runs without them (the schema export, a test, the first start).
//!
//! Three of them are reached through a small port of their own, so a test can stand in
//! for the part that reaches the network: the apps (`AppsPort`: connecting runs an app
//! or opens a browser), the sign-ins (`SignInPort`: a loopback server), and the audio
//! devices (`DevicesPort`: the sound stack, which arrives with W4). Everything else is
//! the core's own type.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ghost_core::apps::{App, AppError, AppSpec, Apps, ToolInfo};
use ghost_core::catalog::Catalog;
use ghost_core::connection::{ConnectionError, ModelConnection, ModelOption, Pkce};
use ghost_core::conversations::Conversations;
use ghost_core::folder::Folder;
use ghost_core::gate::WriteGate;
use ghost_core::hooks::HookIntake;
use ghost_core::outputs::Outputs;
use ghost_core::passwords::{Passwords, SharedEnviron};
use ghost_core::rules::Rules;
use ghost_core::signin::SignIns;
use ghost_core::skill_catalog::SkillCatalog;
use ghost_core::skills::Skills;
use ghost_core::source::ToolSource;
use ghost_core::store::Store;
use ghost_core::waiting::WaitingList;
use ghost_core::watches::WatchRunner;
use zeroize::Zeroizing;

use crate::controller::Controller;
use crate::models::DeviceInfo;
use crate::state::DaemonState;

/// The desktop's own session id for audited edits (the conversations, the rules, the
/// skills): the operator's own click, audited like the spoken change.
pub const UI_SESSION: &str = "ui";

/// The apps, as the routes need them (ADR 0034). `Apps` is the daemon's; a test's fake
/// records what would have connected.
#[async_trait]
pub trait AppsPort: Send + Sync {
    /// Every app in apps.json, in the file's order.
    fn specs(&self) -> Vec<AppSpec>;
    fn spec(&self, name: &str) -> Option<AppSpec>;
    /// The names of every app, connected or not.
    fn names(&self) -> Vec<String>;
    fn is_up(&self, name: &str) -> bool;
    /// connected, connecting, needs_sign_in, failed or stopped.
    fn state(&self, name: &str) -> &'static str;
    fn problem(&self, name: &str) -> Option<String>;
    /// The tools a connected app offers; None while it isn't connected.
    fn live_tools(&self, name: &str) -> Option<Vec<ToolInfo>>;
    /// Connect now; its tools are offered from the next session.
    async fn add(&self, spec: AppSpec, interactive: bool);
    async fn remove(&self, name: &str);
    /// Start an app again whose connection died or that failed at start.
    async fn revive(&self, name: &str) -> bool;
    fn sources(&self) -> Vec<ToolSource>;
}

#[async_trait]
impl AppsPort for Apps {
    fn specs(&self) -> Vec<AppSpec> {
        Apps::specs(self)
    }

    fn spec(&self, name: &str) -> Option<AppSpec> {
        Apps::spec(self, name)
    }

    fn names(&self) -> Vec<String> {
        Apps::apps(self).keys().cloned().collect()
    }

    fn is_up(&self, name: &str) -> bool {
        Apps::is_up(self, name)
    }

    fn state(&self, name: &str) -> &'static str {
        Apps::state(self, name)
    }

    fn problem(&self, name: &str) -> Option<String> {
        Apps::problem(self, name)
    }

    fn live_tools(&self, name: &str) -> Option<Vec<ToolInfo>> {
        let app: Arc<App> = Apps::app(self, name)?;
        app.connected().then(|| app.tools())
    }

    async fn add(&self, spec: AppSpec, interactive: bool) {
        Apps::add(self, spec, interactive).await;
    }

    async fn remove(&self, name: &str) {
        Apps::remove(self, name).await;
    }

    async fn revive(&self, name: &str) -> bool {
        Apps::revive(self, name).await
    }

    fn sources(&self) -> Vec<ToolSource> {
        Apps::sources(self)
    }
}

/// The sign-ins, as the routes need them: an app's pending browser address, forgetting
/// one, and the model connection's own sign-in (ADR 0037).
#[async_trait]
pub trait SignInPort: Send + Sync {
    fn pending_url(&self, app: &str) -> Option<String>;
    fn forget(&self, app: &str);
    fn localhost_redirect_uri(&self) -> String;
    /// Listen for the browser's way back from `url` and return its query.
    async fn browser_return(
        &self,
        name: &str,
        url: &str,
        wait: Duration,
    ) -> Result<HashMap<String, String>, AppError>;
}

#[async_trait]
impl SignInPort for SignIns {
    fn pending_url(&self, app: &str) -> Option<String> {
        SignIns::pending_url(self, app)
    }

    fn forget(&self, app: &str) {
        SignIns::forget(self, app);
    }

    fn localhost_redirect_uri(&self) -> String {
        SignIns::localhost_redirect_uri(self)
    }

    async fn browser_return(
        &self,
        name: &str,
        url: &str,
        wait: Duration,
    ) -> Result<HashMap<String, String>, AppError> {
        SignIns::browser_return(self, name, url, wait).await
    }
}

/// The audio devices, for `GET /devices`: the sound stack's list (W4), and whether the
/// echo-cancel unit runs. The default knows no devices and asks systemd.
pub trait DevicesPort: Send + Sync {
    fn devices(&self) -> Vec<DeviceInfo>;
    fn echo_cancel_active(&self) -> bool {
        unit_active("ghost-echo-cancel.service")
    }
}

/// No sound stack: no devices.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDevices;

impl DevicesPort for NoDevices {
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}

/// `systemctl --user is-active`, with no password in the child's environment.
pub fn unit_active(unit: &str) -> bool {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars()))
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "active",
        Err(_) => false,
    }
}

/// The model providers over the network, as `/model` reaches them: the list of models
/// and OpenRouter's key exchange. A test answers without the network.
#[async_trait]
pub trait ModelNet: Send + Sync {
    async fn list_models(
        &self,
        conn: &ModelConnection,
        env: &HashMap<String, String>,
    ) -> Result<Vec<ModelOption>, ConnectionError>;
    async fn openrouter_exchange(
        &self,
        code: &str,
        pkce: &Pkce,
    ) -> Result<Zeroizing<String>, ConnectionError>;
}

/// The providers at their real addresses.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveModelNet;

#[async_trait]
impl ModelNet for LiveModelNet {
    async fn list_models(
        &self,
        conn: &ModelConnection,
        env: &HashMap<String, String>,
    ) -> Result<Vec<ModelOption>, ConnectionError> {
        ghost_core::connection::list_models(conn, env, None).await
    }

    async fn openrouter_exchange(
        &self,
        code: &str,
        pkce: &Pkce,
    ) -> Result<Zeroizing<String>, ConnectionError> {
        ghost_core::connection::openrouter_exchange(code, pkce, None).await
    }
}

/// The model connection's sign-in under way (ADR 0037): the task waiting for the browser,
/// and how the last one ended.
#[derive(Default)]
pub struct ModelSignIn {
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub error: Mutex<Option<String>>,
}

impl ModelSignIn {
    pub fn pending(&self) -> bool {
        self.task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_error(&self, error: Option<String>) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = error;
    }
}

pub struct Deps {
    pub state: Arc<DaemonState>,
    pub control: Arc<dyn Controller>,
    pub gate: Arc<WriteGate>,
    pub store: Arc<Store>,
    /// The sources the gate was given, when there are no apps to ask (`/sources`).
    pub sources: Vec<ToolSource>,
    pub policy_path: PathBuf,
    pub settings_path: PathBuf,
    /// `~/.config/ghost/env`, the plaintext file an import removes.
    pub env_path: PathBuf,
    /// heka-vault's config on this computer, for the one-time import.
    pub vault_env_path: PathBuf,
    pub infra_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub stt_model: String,
    /// The daemon's own environment, in memory, where the vault fills the keys (ADR
    /// 0036) and the model connection reads them (ADR 0037).
    pub environ: SharedEnviron,
    pub conversations: Option<Arc<Conversations>>,
    pub apps: Option<Arc<dyn AppsPort>>,
    pub catalog: Option<Arc<Catalog>>,
    pub signins: Option<Arc<dyn SignInPort>>,
    pub skills: Option<Arc<Skills>>,
    pub skill_catalog: Option<Arc<SkillCatalog>>,
    pub folder: Option<Arc<Mutex<Folder>>>,
    pub rules: Option<Arc<Rules>>,
    pub outputs: Option<Arc<Outputs>>,
    pub waiting: Option<Arc<WaitingList>>,
    pub watches: Option<Arc<WatchRunner>>,
    pub hooks: Option<Arc<HookIntake>>,
    /// The webhook listener's addresses, when on.
    pub hooks_listening: Vec<String>,
    pub passwords: Option<Arc<Passwords>>,
    pub devices: Arc<dyn DevicesPort>,
    pub model_net: Arc<dyn ModelNet>,
    pub model_sign_in: ModelSignIn,
}

impl Deps {
    /// A daemon with nothing but its state, a store and a gate: the schema export, the
    /// first start. Every optional part is off and every path is `/dev/null`.
    pub fn minimal(
        state: Arc<DaemonState>,
        control: Arc<dyn Controller>,
        store: Arc<Store>,
        gate: Arc<WriteGate>,
    ) -> Self {
        Self {
            state,
            control,
            gate,
            store,
            sources: Vec::new(),
            policy_path: PathBuf::from("/dev/null"),
            settings_path: PathBuf::from("/dev/null"),
            env_path: PathBuf::from("/dev/null"),
            vault_env_path: PathBuf::from("/dev/null"),
            infra_dir: PathBuf::from("/dev/null"),
            cache_dir: PathBuf::from("/dev/null"),
            input_device: None,
            output_device: None,
            stt_model: String::new(),
            environ: Arc::new(Mutex::new(HashMap::new())),
            conversations: None,
            apps: None,
            catalog: None,
            signins: None,
            skills: None,
            skill_catalog: None,
            folder: None,
            rules: None,
            outputs: None,
            waiting: None,
            watches: None,
            hooks: None,
            hooks_listening: Vec::new(),
            passwords: None,
            devices: Arc::new(NoDevices),
            model_net: Arc::new(LiveModelNet),
            model_sign_in: ModelSignIn::default(),
        }
    }

    /// The daemon's environment now, copied.
    pub fn environ(&self) -> HashMap<String, String> {
        self.environ
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// `settings.env`, read now; empty when there is no file.
    pub fn settings(&self) -> BTreeMap<String, String> {
        ghost_core::envfile::read_env(&self.settings_path).unwrap_or_default()
    }
}

pub type SharedDeps = Arc<Deps>;
