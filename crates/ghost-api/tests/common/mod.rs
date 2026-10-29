//! The world the API tests run in (the Python `conftest.world`): an in-memory store, a
//! fake app with a `flag` and a `nuke` operation, the agents, the rules, the outputs, a
//! vault over a fake keyring, and a `Controller` that records its calls with a real gate
//! behind confirm and cancel. Every optional part is set on the world before `client()`.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request};
use chrono::Utc;
use ghost_api::controller::{ModelEnv, PhoneConnection, Probe, Refusal};
use ghost_api::deps::{AppsPort, DevicesPort, ModelNet, SignInPort};
use ghost_api::models::{DeviceInfo, PhoneDevice, PhonePairing, PhoneView};
use ghost_api::{Controller, DaemonState, Deps, Phase};
use ghost_core::apps::{AppError, AppSpec, ToolInfo};
use ghost_core::catalog::Catalog;
use ghost_core::connection::{ConnectionError, ModelConnection, ModelOption, Pkce};
use ghost_core::conversations::{Conversations, ConversationsOptions, SOURCE as CONVERSATIONS};
use ghost_core::fakes::FakeWriter;
use ghost_core::folder::Folder;
use ghost_core::gate::{Request as GateRequest, Submission, WriteGate};
use ghost_core::levels::Risk;
use ghost_core::models::{Change, ChangeSet, FakeIssue, Outcome, SetLabels, Target};
use ghost_core::outputs::Outputs;
use ghost_core::passwords::{MemoryKeyring, Passwords, SharedEnviron};
use ghost_core::policy::{AutonomyPolicy, PolicyError};
use ghost_core::routes::RouteName;
use ghost_core::rules::{Rules, SOURCE as RULES};
use ghost_core::skill_catalog::SkillCatalog;
use ghost_core::skills::Skills;
use ghost_core::source::{Operation, Prepared, ReadTool, ToolSource};
use ghost_core::store::Store;
use ghost_core::waiting::{Changes, Items, WaitingList};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use zeroize::Zeroizing;

// --- the controller ---------------------------------------------------------------------------

pub type ProbeFn = Arc<dyn Fn(ModelEnv, String, u32) -> Probe + Send + Sync>;

/// Records calls; a real gate sits behind confirm/cancel.
pub struct FakeControl {
    pub state: Arc<DaemonState>,
    pub gate: Arc<WriteGate>,
    pub policy_path: PathBuf,
    pub calls: Mutex<Vec<Value>>,
    pub applied: Mutex<Vec<String>>,
    pub phone_address: Mutex<Option<String>>,
    pub phone_device: Mutex<Option<PhoneDevice>>,
    pub probe: Mutex<Option<ProbeFn>>,
}

impl FakeControl {
    pub fn calls(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }

    pub fn last_call(&self) -> Value {
        self.calls().last().cloned().unwrap_or(Value::Null)
    }

    pub fn has_call(&self, call: Value) -> bool {
        self.calls().contains(&call)
    }

    pub fn applied(&self) -> Vec<String> {
        self.applied.lock().unwrap().clone()
    }

    pub fn set_phone_address(&self, address: Option<&str>) {
        *self.phone_address.lock().unwrap() = address.map(String::from);
    }

    pub fn set_phone_device(&self, device: Option<PhoneDevice>) {
        *self.phone_device.lock().unwrap() = device;
    }

    pub fn set_probe(&self, probe: ProbeFn) {
        *self.probe.lock().unwrap() = Some(probe);
    }

    fn record(&self, call: Value) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl Controller for FakeControl {
    async fn wake(&self, first: Option<String>, conversation: Option<String>) -> bool {
        self.record(match &conversation {
            None => json!(["wake", first]),
            Some(c) => json!(["wake", first, c]),
        });
        if self.state.phase() != Phase::Passive {
            return false;
        }
        self.state.set_phase(Phase::Listening, Some("sess1"), None);
        self.state
            .set_conversation(Some(conversation.as_deref().unwrap_or("ghost")), None);
        true
    }

    async fn sleep(&self) -> bool {
        self.record(json!(["sleep"]));
        if self.state.session_id().is_none() {
            return false;
        }
        self.state.set_phase(Phase::Passive, None, None);
        true
    }

    async fn say(&self, text: String, conversation: Option<String>) -> bool {
        self.record(json!(["say", text]));
        if self.state.phase() == Phase::Starting {
            return false;
        }
        if self.state.session_id().is_none() {
            self.wake(Some(text.clone()), conversation).await;
        }
        self.state.add_turn(
            ghost_api::models::TurnRole::User,
            &text,
            ghost_api::models::TurnVia::Text,
        );
        true
    }

    async fn confirm_pending(&self) -> Option<Outcome> {
        let sid = self.state.session_id()?;
        let out = self
            .gate
            .confirm_pending(&sid, "ui", None)
            .await
            .ok()
            .flatten();
        if out.is_some() {
            self.state.set_pending(None);
        }
        out
    }

    async fn cancel_pending(&self) -> bool {
        let Some(sid) = self.state.session_id() else {
            return false;
        };
        let p = self.gate.cancel(&sid, "ui").ok().flatten();
        if p.is_some() {
            self.state.set_pending(None);
        }
        p.is_some()
    }

    async fn confirm_proposal(&self, proposal_id: &str) -> Option<Outcome> {
        self.gate
            .confirm_id(proposal_id, "ui", None)
            .await
            .ok()
            .flatten()
    }

    async fn cancel_proposal(&self, proposal_id: &str) -> bool {
        self.gate
            .cancel_id(proposal_id, "ui")
            .ok()
            .flatten()
            .is_some()
    }

    fn reload_policy(&self) -> Result<AutonomyPolicy, PolicyError> {
        let policy = AutonomyPolicy::load(&self.policy_path)?;
        self.gate.set_autonomy(policy.clone());
        self.record(json!(["reload_policy"]));
        Ok(policy)
    }

    fn apply_settings(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        let applied: Vec<String> = values
            .keys()
            .filter(|k| k.starts_with("GHOST_WAKE_"))
            .cloned()
            .collect();
        *self.applied.lock().unwrap() = applied.clone();
        applied
    }

    fn apply_model(&self, _values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        Vec::new()
    }

    async fn probe_model(&self, env: ModelEnv, prompt: &str, max_tokens: u32) -> Option<Probe> {
        let probe = self.probe.lock().unwrap().clone()?;
        Some(probe(env, prompt.to_string(), max_tokens))
    }

    fn routes_available(&self) -> Vec<RouteName> {
        vec![RouteName::Digest]
    }

    async fn switch_conversation(&self, slug: &str) -> bool {
        self.record(json!(["switch", slug]));
        if self.state.session_id().is_none() {
            return false;
        }
        self.state.set_conversation(Some(slug), None);
        true
    }

    async fn compact_conversation(&self, slug: &str) -> bool {
        self.record(json!(["compact", slug]));
        true
    }

    fn voices(&self) -> Vec<String> {
        vec!["af_heart".into(), "am_adam".into(), "bf_emma".into()]
    }

    async fn sample_voice(&self, voice: &str, speed: f64, text: Option<String>) -> bool {
        self.record(json!(["sample", voice, speed, text]));
        true
    }

    fn phone_pairing(&self) -> Option<PhonePairing> {
        self.record(json!(["phone_pairing"]));
        let address = self.phone_address.lock().unwrap().clone()?;
        Some(PhonePairing {
            code: "123456".into(),
            link: format!("ghost://pair?code=123456&address={address}"),
            address,
            addresses: Vec::new(),
            expires_at: Utc::now() + chrono::Duration::seconds(120),
        })
    }

    fn phone_view(&self) -> PhoneView {
        PhoneView {
            address: self.phone_address.lock().unwrap().clone(),
            addresses: Vec::new(),
            listen: Vec::new(),
            device: self.phone_device.lock().unwrap().clone(),
        }
    }

    async fn forget_phone(&self) -> bool {
        self.record(json!(["forget_phone"]));
        self.phone_device.lock().unwrap().take().is_some()
    }

    fn admit_phone(&self) -> Option<Refusal> {
        None
    }

    async fn start_phone(
        &self,
        _c: PhoneConnection,
        _conversation: Option<String>,
    ) -> Option<Refusal> {
        None
    }

    async fn end_phone(&self) -> bool {
        false
    }

    async fn passwords_opened(&self) {
        self.record(json!(["passwords_opened"]));
    }
}

// --- the fake app ------------------------------------------------------------------------------

pub fn the_issue() -> FakeIssue {
    FakeIssue {
        owner: "acme".into(),
        repo: "app".into(),
        number: 1,
    }
}

/// A fake app with `flag` (low risk) and `nuke` (high risk), both adding the `flagged`
/// label to the one issue, and an `echo` read.
pub fn make_source(writer: Arc<FakeWriter>) -> ToolSource {
    let prepare = |_args: Map<String, Value>| -> ghost_core::source::BoxFuture<'static, anyhow::Result<Prepared>> {
        Box::pin(async {
            Ok(Prepared {
                target: Target::FakeIssue(the_issue()),
                changes: ChangeSet::one(Change::SetLabels(SetLabels {
                    add: vec!["flagged".into()],
                    remove: Vec::new(),
                })),
                rationale: String::new(),
                asked_by: Vec::new(),
            })
        })
    };
    let mut flag = Operation::new("flag", "Flag it.", Arc::new(prepare), writer.clone());
    flag.properties = json!({"name": {"type": "string"}});
    flag.required = vec!["name".into()];
    flag.risk = Risk::Low;
    let mut nuke = Operation::new("nuke", "Dangerous.", Arc::new(prepare), writer);
    nuke.properties = json!({"name": {"type": "string"}});
    nuke.required = vec!["name".into()];
    nuke.risk = Risk::High;
    let echo = ReadTool {
        name: "echo".into(),
        description: "Echo.".into(),
        properties: json!({}),
        required: Vec::new(),
        handler: Arc::new(|args| Box::pin(async move { Ok(args) })),
        operation: String::new(),
    };
    ToolSource {
        name: "fake".into(),
        prompt: "A fake app. It echoes.".into(),
        read_tools: vec![echo],
        operations: vec![flag, nuke],
        ..Default::default()
    }
}

/// Put a confirm-level proposal in the gate the way the model's `change` tool would.
pub async fn pend(gate: &WriteGate, session: &str) -> Submission {
    let op = gate.operation("fake", "nuke").unwrap();
    let prepared = (op.prepare)(Map::new()).await.unwrap();
    gate.submit(GateRequest::new(
        session,
        "fake",
        "nuke",
        prepared.target,
        prepared.changes,
        "test",
    ))
    .await
    .unwrap()
}

// --- the apps port, faked ------------------------------------------------------------------------

/// Apps as a test sees them: what is connected by hand, and what connecting would have
/// started (recorded, never run).
#[derive(Default)]
pub struct FakeApps {
    pub specs: Mutex<Vec<AppSpec>>,
    pub up: Mutex<HashSet<String>>,
    pub tools: Mutex<HashMap<String, Vec<ToolInfo>>>,
    pub failed: Mutex<HashMap<String, String>>,
    pub started: Mutex<Vec<AppSpec>>,
    pub revived: Mutex<Vec<String>>,
    pub revive_result: Mutex<bool>,
}

impl FakeApps {
    pub fn connected(spec: AppSpec, tools: Vec<ToolInfo>) -> Self {
        let apps = Self::default();
        apps.up.lock().unwrap().insert(spec.name.clone());
        apps.tools.lock().unwrap().insert(spec.name.clone(), tools);
        apps.specs.lock().unwrap().push(spec);
        apps
    }

    pub fn push_spec(&self, spec: AppSpec) {
        self.specs.lock().unwrap().push(spec);
    }

    pub fn started(&self) -> Vec<AppSpec> {
        self.started.lock().unwrap().clone()
    }

    pub fn has_spec(&self, name: &str) -> bool {
        self.specs.lock().unwrap().iter().any(|s| s.name == name)
    }

    pub fn revived(&self) -> Vec<String> {
        self.revived.lock().unwrap().clone()
    }
}

#[async_trait]
impl AppsPort for FakeApps {
    fn specs(&self) -> Vec<AppSpec> {
        self.specs.lock().unwrap().clone()
    }

    fn spec(&self, name: &str) -> Option<AppSpec> {
        self.specs().into_iter().find(|s| s.name == name)
    }

    fn names(&self) -> Vec<String> {
        self.specs().into_iter().map(|s| s.name).collect()
    }

    fn is_up(&self, name: &str) -> bool {
        self.up.lock().unwrap().contains(name)
    }

    fn state(&self, name: &str) -> &'static str {
        if self.is_up(name) {
            "connected"
        } else {
            "failed"
        }
    }

    fn problem(&self, name: &str) -> Option<String> {
        self.failed.lock().unwrap().get(name).cloned()
    }

    fn live_tools(&self, name: &str) -> Option<Vec<ToolInfo>> {
        if !self.is_up(name) {
            return None;
        }
        Some(
            self.tools
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or_default(),
        )
    }

    async fn add(&self, spec: AppSpec, _interactive: bool) {
        self.started.lock().unwrap().push(spec.clone());
        let mut specs = self.specs.lock().unwrap();
        match specs.iter_mut().find(|s| s.name == spec.name) {
            Some(slot) => *slot = spec.clone(),
            None => specs.push(spec.clone()),
        }
        self.failed
            .lock()
            .unwrap()
            .insert(spec.name, "not reachable".into());
    }

    async fn remove(&self, name: &str) {
        self.specs.lock().unwrap().retain(|s| s.name != name);
        self.up.lock().unwrap().remove(name);
        self.failed.lock().unwrap().remove(name);
    }

    async fn revive(&self, name: &str) -> bool {
        self.revived.lock().unwrap().push(name.to_string());
        *self.revive_result.lock().unwrap()
    }

    fn sources(&self) -> Vec<ToolSource> {
        Vec::new()
    }
}

// --- the sign-ins, faked -------------------------------------------------------------------------

pub type ReturnFn = Arc<dyn Fn() -> Result<HashMap<String, String>, AppError> + Send + Sync>;

pub struct FakeSignIns {
    pub urls: Mutex<Vec<String>>,
    pub answer: ReturnFn,
    pub pending: Mutex<HashMap<String, String>>,
}

impl FakeSignIns {
    pub fn new() -> Self {
        Self::answering(Arc::new(|| {
            let mut q = HashMap::new();
            q.insert("code".to_string(), "the-code".to_string());
            Ok(q)
        }))
    }

    pub fn answering(answer: ReturnFn) -> Self {
        Self {
            urls: Mutex::new(Vec::new()),
            answer,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn urls(&self) -> Vec<String> {
        self.urls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SignInPort for FakeSignIns {
    fn pending_url(&self, app: &str) -> Option<String> {
        self.pending.lock().unwrap().get(app).cloned()
    }

    fn forget(&self, app: &str) {
        self.pending.lock().unwrap().remove(app);
    }

    fn localhost_redirect_uri(&self) -> String {
        "http://localhost:47823/callback".into()
    }

    async fn browser_return(
        &self,
        _name: &str,
        url: &str,
        _wait: Duration,
    ) -> Result<HashMap<String, String>, AppError> {
        self.urls.lock().unwrap().push(url.to_string());
        (self.answer)()
    }
}

// --- the model providers, faked ------------------------------------------------------------------

pub type ListFn = Arc<
    dyn Fn(&ModelConnection, &HashMap<String, String>) -> Result<Vec<ModelOption>, ConnectionError>
        + Send
        + Sync,
>;

pub struct FakeModelNet {
    pub list: Mutex<Option<ListFn>>,
    pub key: String,
    pub traded: Mutex<Vec<String>>,
}

impl FakeModelNet {
    pub fn new() -> Self {
        Self {
            list: Mutex::new(None),
            key: "sk-or-v1-fedcba9876543210".into(),
            traded: Mutex::new(Vec::new()),
        }
    }

    pub fn listing(list: ListFn) -> Self {
        let net = Self::new();
        *net.list.lock().unwrap() = Some(list);
        net
    }

    pub fn traded(&self) -> Vec<String> {
        self.traded.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelNet for FakeModelNet {
    async fn list_models(
        &self,
        conn: &ModelConnection,
        env: &HashMap<String, String>,
    ) -> Result<Vec<ModelOption>, ConnectionError> {
        let list = self.list.lock().unwrap().clone();
        match list {
            Some(f) => f(conn, env),
            None => panic!("the model list was asked for without a fake"),
        }
    }

    async fn openrouter_exchange(
        &self,
        code: &str,
        _pkce: &Pkce,
    ) -> Result<Zeroizing<String>, ConnectionError> {
        self.traded.lock().unwrap().push(code.to_string());
        Ok(Zeroizing::new(self.key.clone()))
    }
}

// --- the devices, faked --------------------------------------------------------------------------

pub struct FakeDevices(pub Vec<DeviceInfo>);

impl DevicesPort for FakeDevices {
    fn devices(&self) -> Vec<DeviceInfo> {
        self.0.clone()
    }

    fn echo_cancel_active(&self) -> bool {
        false
    }
}

// --- the world -------------------------------------------------------------------------------------

pub struct World {
    pub tmp: TempDir,
    pub store: Arc<Store>,
    pub writer: Arc<FakeWriter>,
    pub gate: Arc<WriteGate>,
    pub policy_path: PathBuf,
    pub config: PathBuf,
    pub conversations: Arc<Conversations>,
    pub folder: Arc<Mutex<Folder>>,
    pub rules: Arc<Rules>,
    pub outputs: Arc<Outputs>,
    pub state: Arc<DaemonState>,
    pub control: Arc<FakeControl>,
    pub keyring: Arc<MemoryKeyring>,
    pub vault: Arc<Passwords>,
    pub environ: SharedEnviron,
    pub source: ToolSource,
    pub settings_path: PathBuf,
    pub env_path: PathBuf,
    pub vault_env_path: PathBuf,
    pub infra_dir: PathBuf,
    pub cache_dir: PathBuf,
    // the optional parts a test sets before `client()`
    pub apps: Option<Arc<dyn AppsPort>>,
    pub catalog: Option<Arc<Catalog>>,
    pub signins: Option<Arc<dyn SignInPort>>,
    pub skills: Option<Arc<Skills>>,
    pub skill_catalog: Option<Arc<SkillCatalog>>,
    pub hooks_listening: Vec<String>,
    pub passwords: Option<Arc<Passwords>>,
    pub devices: Option<Arc<dyn DevicesPort>>,
    pub model_net: Option<Arc<dyn ModelNet>>,
}

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let store = Arc::new(Store::in_memory().unwrap());
        let writer = Arc::new(FakeWriter::default());
        writer.seed(&the_issue());
        let source = make_source(writer.clone());
        let policy_path = root.join("policy.toml");
        // ADR 0034: everything runs unless a rule asks; this world asks before a nuke.
        std::fs::write(&policy_path, "[permissions]\nask = [\"fake.nuke\"]\n").unwrap();
        let gate = Arc::new(
            WriteGate::new(store.clone())
                .with_autonomy(AutonomyPolicy::load(&policy_path).unwrap()),
        );
        gate.register("fake", source.operations.clone());
        let config = root.join("config");
        let folder = Folder::new(&config);
        folder.ensure_layout().unwrap();
        let conversations = Arc::new(
            Conversations::new(
                store.clone(),
                &config,
                ConversationsOptions {
                    default_voice: "af_heart".into(),
                    voices: vec!["af_heart".into(), "am_adam".into(), "bf_emma".into()],
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        gate.register(CONVERSATIONS, conversations.operations());
        let rules = Arc::new(Rules::new(&config));
        gate.register(RULES, rules.operations());
        let outputs = Arc::new(Outputs::new(&config));
        gate.register("outputs", outputs.operations());
        let state = Arc::new(DaemonState::new("test", "file"));
        let control = Arc::new(FakeControl {
            state: state.clone(),
            gate: gate.clone(),
            policy_path: policy_path.clone(),
            calls: Mutex::new(Vec::new()),
            applied: Mutex::new(Vec::new()),
            phone_address: Mutex::new(None),
            phone_device: Mutex::new(None),
            probe: Mutex::new(None),
        });
        let infra = root.join("infra").join("pipewire");
        std::fs::create_dir_all(&infra).unwrap();
        let script = infra.join("install.sh");
        std::fs::write(&script, "#!/bin/sh\necho installed $1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // A fake keyring, and an environment of its own: never the operator's (ADR 0036).
        let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
        let keyring = Arc::new(MemoryKeyring::new());
        let vault = Arc::new(
            Passwords::new(root.join("data").join("passwords.vault"), keyring.clone())
                .with_environ(environ.clone()),
        );
        assert!(vault.open());
        World {
            store,
            writer,
            gate,
            policy_path,
            config,
            conversations,
            folder: Arc::new(Mutex::new(folder)),
            rules,
            outputs,
            state,
            control,
            keyring,
            vault,
            environ,
            source,
            settings_path: root.join("settings.env"),
            env_path: root.join("env"),
            vault_env_path: root.join("no-vault"),
            infra_dir: root.join("infra"),
            cache_dir: root.join("cache"),
            apps: None,
            catalog: None,
            signins: None,
            skills: None,
            skill_catalog: None,
            hooks_listening: Vec::new(),
            passwords: None,
            devices: None,
            model_net: None,
            tmp,
        }
    }

    pub fn path(&self) -> &Path {
        self.tmp.path()
    }

    pub fn set_env(&self, key: &str, value: &str) {
        self.environ
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub fn env(&self) -> HashMap<String, String> {
        self.environ.lock().unwrap().clone()
    }

    pub fn deps(&self) -> Arc<Deps> {
        let changes: Arc<dyn Changes> = self.gate.clone();
        let waiting =
            Arc::new(WaitingList::new(Items::new(self.store.clone())).with_changes(changes));
        let mut deps = Deps::minimal(
            self.state.clone(),
            self.control.clone(),
            self.store.clone(),
            self.gate.clone(),
        );
        deps.sources = vec![self.source.clone()];
        deps.policy_path = self.policy_path.clone();
        deps.settings_path = self.settings_path.clone();
        deps.env_path = self.env_path.clone();
        deps.vault_env_path = self.vault_env_path.clone();
        deps.infra_dir = self.infra_dir.clone();
        deps.cache_dir = self.cache_dir.clone();
        deps.input_device = Some("ghost.mic".into());
        deps.output_device = None;
        deps.stt_model = "parakeet".into();
        deps.environ = self.environ.clone();
        deps.conversations = Some(self.conversations.clone());
        deps.folder = Some(self.folder.clone());
        deps.rules = Some(self.rules.clone());
        deps.outputs = Some(self.outputs.clone());
        deps.waiting = Some(waiting);
        deps.passwords = Some(self.passwords.clone().unwrap_or_else(|| self.vault.clone()));
        deps.apps = self.apps.clone();
        deps.catalog = self.catalog.clone();
        deps.signins = self.signins.clone();
        deps.skills = self.skills.clone();
        deps.skill_catalog = self.skill_catalog.clone();
        deps.hooks_listening = self.hooks_listening.clone();
        if let Some(d) = &self.devices {
            deps.devices = d.clone();
        }
        if let Some(n) = &self.model_net {
            deps.model_net = n.clone();
        }
        Arc::new(deps)
    }

    pub fn client(&self) -> TestClient {
        TestClient {
            router: ghost_api::router(self.deps()),
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

// --- the client ---------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub json: Value,
    pub text: String,
}

impl Resp {
    pub fn ok(&self) -> bool {
        self.status == 200
    }

    pub fn detail(&self) -> String {
        match &self.json["detail"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

impl std::ops::Index<&str> for Resp {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        &self.json[key]
    }
}

impl std::ops::Index<usize> for Resp {
    type Output = Value;

    fn index(&self, i: usize) -> &Value {
        &self.json[i]
    }
}

pub struct TestClient {
    pub router: Router,
}

impl TestClient {
    pub async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Resp {
        let mut req = Request::builder().method(method).uri(path);
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp { status, json, text }
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Option<Value>) -> Resp {
        self.send(Method::POST, path, body).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Resp {
        self.send(Method::PUT, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Resp {
        self.send(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Resp {
        self.send(Method::DELETE, path, None).await
    }
}

/// `json` as a map of `name -> item` by one of its string fields.
pub fn by<'a>(items: &'a Value, key: &str) -> BTreeMap<String, &'a Value> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i[key].as_str().unwrap().to_string(), i))
        .collect()
}

pub fn strings(items: &Value, key: &str) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i[key].as_str().unwrap().to_string())
        .collect()
}
