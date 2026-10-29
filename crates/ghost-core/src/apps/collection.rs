//! Every connected app. Connected at daemon start; one the operator connects in the
//! desktop joins at once (`add`), one disconnected leaves (`remove`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde_json::{Map, Value};
use tracing::{error, info};

use super::bridge::source_of;
use super::connection::{App, AuthFor};
use super::spec::AppSpec;
use super::{AppError, Environ};
use crate::source::ToolSource;

pub type ChangeHook = Arc<dyn Fn() + Send + Sync>;

pub struct Apps {
    /// In the file's order: the first to name a tool keeps the name.
    specs: RwLock<Vec<AppSpec>>,
    environ: Arc<dyn Environ>,
    auth_for: Option<Arc<dyn AuthFor>>,
    apps: RwLock<HashMap<String, Arc<App>>>,
    failed: RwLock<HashMap<String, String>>,
    connecting: RwLock<HashSet<String>>,
    /// Called after an app joins or leaves (the gate and the sessions follow).
    on_change: RwLock<Vec<ChangeHook>>,
}

impl Apps {
    pub fn new(
        specs: impl IntoIterator<Item = AppSpec>,
        environ: Arc<dyn Environ>,
        auth_for: Option<Arc<dyn AuthFor>>,
    ) -> Self {
        Self {
            specs: RwLock::new(specs.into_iter().collect()),
            environ,
            auth_for,
            apps: RwLock::new(HashMap::new()),
            failed: RwLock::new(HashMap::new()),
            connecting: RwLock::new(HashSet::new()),
            on_change: RwLock::new(Vec::new()),
        }
    }

    pub fn specs(&self) -> Vec<AppSpec> {
        self.specs.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn spec(&self, name: &str) -> Option<AppSpec> {
        self.specs().into_iter().find(|s| s.name == name)
    }

    pub fn app(&self, name: &str) -> Option<Arc<App>> {
        self.apps
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// Every app, connected or not, by name.
    pub fn apps(&self) -> HashMap<String, Arc<App>> {
        self.apps.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn failed(&self) -> HashMap<String, String> {
        self.failed
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn on_change(&self, hook: ChangeHook) {
        self.on_change
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(hook);
    }

    /// An app connected elsewhere (a test, an in-process app) joins as it is.
    pub fn attach(&self, app: Arc<App>) {
        self.apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app.name().to_string(), app);
    }

    fn make(&self, spec: AppSpec) -> Arc<App> {
        Arc::new(App::new(spec, self.environ.clone(), self.auth_for.clone()))
    }

    pub async fn connect(&self) {
        for spec in self.specs() {
            self.start(spec, false).await;
        }
    }

    async fn start(&self, spec: AppSpec, interactive: bool) -> Option<Arc<App>> {
        let name = spec.name.clone();
        let app = self.make(spec);
        if let Err(e) = app.connect(interactive).await {
            let why = e.reason();
            error!("app {name}: not connected: {why}");
            self.failed
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name, why);
            app.close().await;
            return None;
        }
        self.apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.clone(), app.clone());
        self.failed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&name);
        app.log_connected();
        Some(app)
    }

    pub fn needs_sign_in(&self, name: &str) -> bool {
        self.failed().get(name).map(String::as_str) == Some("needs sign-in")
    }

    /// connected, connecting, needs_sign_in, failed or stopped, for the operator.
    pub fn state(&self, name: &str) -> &'static str {
        let app = self.app(name);
        if app.as_ref().is_some_and(|a| a.connected()) {
            return "connected";
        }
        if self
            .connecting
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(name)
        {
            return "connecting";
        }
        if self.needs_sign_in(name) {
            return "needs_sign_in";
        }
        if self.failed().contains_key(name) {
            return "failed";
        }
        if app.is_some() { "stopped" } else { "failed" }
    }

    pub fn problem(&self, name: &str) -> Option<String> {
        let app = self.app(name);
        if let Some(app) = &app
            && app.connected()
        {
            self.claim_names();
            let withheld = app.withheld();
            if withheld.is_empty() {
                return None;
            }
            let names: Vec<String> = withheld
                .iter()
                .map(|(t, n)| format!("{t} (as {n})"))
                .collect();
            return Some(format!(
                "not offered: {}; another tool already has that name. Give the app another \
                 name to offer them",
                names.join(", ")
            ));
        }
        if let Some(why) = self.failed().get(name) {
            return Some(why.clone());
        }
        app.and_then(|a| a.failed())
    }

    /// Connect an app now (the operator connected it): its tools are offered from the
    /// next session, or the next transfer in a running one. One already connected under
    /// the name is replaced.
    pub async fn add(&self, spec: AppSpec, interactive: bool) -> Option<Arc<App>> {
        let name = spec.name.clone();
        self.stop(&name).await;
        {
            let mut specs = self.specs.write().unwrap_or_else(|e| e.into_inner());
            match specs.iter_mut().find(|s| s.name == name) {
                Some(slot) => *slot = spec.clone(),
                None => specs.push(spec.clone()),
            }
        }
        self.failed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&name);
        self.connecting
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.clone());
        let app = self.start(spec, interactive).await;
        self.connecting
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&name);
        self.changed();
        app
    }

    pub async fn remove(&self, name: &str) {
        self.stop(name).await;
        self.specs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| s.name != name);
        self.failed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        self.changed();
    }

    async fn stop(&self, name: &str) {
        let old = self
            .apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        if let Some(old) = old {
            old.close().await;
        }
    }

    fn changed(&self) {
        let hooks = self
            .on_change
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for hook in hooks {
            hook();
        }
    }

    /// Connected apps in the file's order: the first to name a tool keeps the name.
    fn in_order(&self) -> Vec<Arc<App>> {
        let apps = self.apps();
        let mut order: Vec<String> = self.specs().into_iter().map(|s| s.name).collect();
        let mut extra: Vec<&String> = apps.keys().filter(|n| !order.contains(n)).collect();
        extra.sort();
        order.extend(extra.into_iter().cloned());
        order
            .iter()
            .filter_map(|n| apps.get(n).cloned())
            .filter(|a| a.connected())
            .collect()
    }

    /// Which app offers a tool name the model sees two apps give (`github` with
    /// `issue_read`, `github_issue` with `read`): the first in apps.json.
    fn claim_names(&self) {
        let mut taken: HashSet<String> = HashSet::new();
        for app in self.in_order() {
            app.set_taken_elsewhere(taken.clone());
            taken.extend(app.offered().into_iter().map(|(_, n)| n));
        }
    }

    pub fn sources(&self) -> Vec<ToolSource> {
        self.claim_names();
        self.in_order()
            .iter()
            .map(source_of)
            .filter(|s| !s.read_tools.is_empty() || !s.operations.is_empty())
            .collect()
    }

    pub fn connected(&self) -> bool {
        self.apps().values().any(|a| a.connected())
    }

    pub fn is_up(&self, name: &str) -> bool {
        self.app(name).is_some_and(|a| a.connected())
    }

    /// Start an app again whose connection died or that failed at start (a watch reads
    /// it, ADR 0028). The caller backs off; this tries once, without a browser.
    pub async fn revive(&self, name: &str) -> bool {
        let Some(spec) = self.spec(name) else {
            return false;
        };
        self.stop(name).await;
        let app = self.start(spec, false).await;
        if app.is_some() {
            info!("app {name}: connected again");
            self.changed();
        }
        app.is_some()
    }

    /// A read the daemon makes (a watch, a webhook's check): the app brought back first
    /// if its connection died.
    pub async fn read_for(
        &self,
        name: &str,
        tool: &str,
        args: &Map<String, Value>,
    ) -> Result<Map<String, Value>, AppError> {
        if !self.is_up(name) {
            self.revive(name).await;
        }
        let Some(app) = self.app(name).filter(|a| a.connected()) else {
            return Err(AppError::Value(format!("{name} is not connected")));
        };
        app.watch_read(tool, args).await
    }

    pub async fn close(&self) {
        let apps: Vec<Arc<App>> = self
            .apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, a)| a)
            .collect();
        for app in apps {
            app.close().await;
        }
    }
}
