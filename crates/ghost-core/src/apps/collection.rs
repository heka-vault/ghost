//! Every connected app. Connected at daemon start; one the operator connects in the
//! desktop joins at once (`add`), one disconnected leaves (`remove`). A connection that
//! dies is opened again in the background (`supervise`), and when the apps have more
//! tools between them than a turn should carry, most are found by search first
//! (`search_tools`, ADR 0046).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde_json::{Map, Value, json};
use tracing::{error, info};

use super::ask::{Asker, AskerSlot};
use super::bridge::source_without;
pub use super::connection::ChangeHook;
use super::connection::{App, AuthFor};
use super::limits::Limits;
use super::spec::AppSpec;
use super::{AppError, Environ, TOOLS_SEARCH};
use crate::source::ToolSource;

/// How many tools one search gives.
pub const SEARCH_RESULTS: usize = 8;
/// How many of them, the best first, are offered by name from the next call (the rest
/// stay callable by the name the search gave). Fewer than it gives: every one offered
/// moves the tool list, and the cached prompt with it, and takes a place among the kept.
pub const SEARCH_KEPT: usize = 3;

/// The tools the model used or found lately, by the names it calls them: the ones still
/// offered by name when most are found by search. The newest `cap` are kept; using one
/// already there changes nothing the model sees, so the tool list, and the cached prompt
/// before it, moves only when a tool it had not used comes in.
///
/// What is kept outlives the daemon when a keeper is set (`Apps::remember`): the daemon
/// keeps it in SQLite (`Store::recent_tools`), so a new one offers by name what the model
/// used before, and the cached prompt of the first session holds as it did.
pub struct Recent {
    cap: usize,
    names: Mutex<Vec<String>>,
    save: Mutex<Option<SaveRecent>>,
}

/// Told what is kept, oldest first, whenever it changes.
pub type SaveRecent = Arc<dyn Fn(&[String]) + Send + Sync>;

impl Recent {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            names: Mutex::new(Vec::new()),
            save: Mutex::new(None),
        }
    }

    pub fn touch(&self, name: &str) {
        let kept = {
            let mut names = self.names.lock().unwrap_or_else(|e| e.into_inner());
            let before = names.clone();
            names.retain(|n| n != name);
            names.push(name.to_string());
            let over = names.len().saturating_sub(self.cap);
            names.drain(..over);
            if *names == before {
                return; // used again, and already the newest
            }
            names.clone()
        };
        let save = self.save.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(save) = save {
            save(&kept);
        }
    }

    /// Start from `names` (oldest first; the newest `cap` are kept) and tell `save` of
    /// every change from here on.
    pub fn restore(&self, names: Vec<String>, save: SaveRecent) {
        {
            let mut kept = self.names.lock().unwrap_or_else(|e| e.into_inner());
            let over = names.len().saturating_sub(self.cap);
            *kept = names.into_iter().skip(over).collect();
        }
        *self.save.lock().unwrap_or_else(|e| e.into_inner()) = Some(save);
    }

    pub fn names(&self) -> HashSet<String> {
        self.names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

pub struct Apps {
    /// In the file's order: the first to name a tool keeps the name.
    specs: RwLock<Vec<AppSpec>>,
    environ: Arc<dyn Environ>,
    auth_for: Option<Arc<dyn AuthFor>>,
    apps: RwLock<HashMap<String, Arc<App>>>,
    failed: RwLock<HashMap<String, String>>,
    connecting: RwLock<HashSet<String>>,
    /// Called after an app joins or leaves, or its tools change (the gate and the
    /// sessions follow).
    on_change: Arc<RwLock<Vec<ChangeHook>>>,
    limits: Limits,
    asker: Arc<AskerSlot>,
    recent: Arc<Recent>,
    /// When each app that is down is tried next, and how often it has been.
    waits: Mutex<HashMap<String, (u32, Instant)>>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
            on_change: Arc::new(RwLock::new(Vec::new())),
            limits: Limits::default(),
            asker: Arc::new(AskerSlot::default()),
            recent: Arc::new(Recent::new(Limits::default().tools_kept)),
            waits: Mutex::new(HashMap::new()),
            supervisor: Mutex::new(None),
        }
    }

    /// The limits every app is held to, where its own entry does not say (ADR 0046).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.recent = Arc::new(Recent::new(limits.tools_kept));
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The tools the model used or found before (oldest first), kept by the caller and
    /// told to `save` whenever what is kept changes. Offered by name past the limits like
    /// any used lately, so a daemon started again does not search for them first.
    pub fn remember(&self, names: Vec<String>, save: SaveRecent) {
        self.recent.restore(names, save);
        self.changed();
    }

    /// Who an app's question for the operator goes to. Until this is called every
    /// question is declined, and apps are not told Ghost can ask; an app connected
    /// before the call learns of it when it next connects.
    pub fn set_asker(&self, asker: Arc<dyn Asker>) {
        self.asker.set(asker);
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
        app.join(self.hook(), self.recent.clone());
        self.apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app.name().to_string(), app);
    }

    /// What an app calls when its tools changed or its connection did.
    fn hook(&self) -> ChangeHook {
        let hooks = self.on_change.clone();
        Arc::new(move || run(&hooks))
    }

    fn make(&self, spec: AppSpec) -> Arc<App> {
        let app = App::new(spec, self.environ.clone(), self.auth_for.clone())
            .with_limits(self.limits)
            .with_asker(self.asker.clone());
        app.join(self.hook(), self.recent.clone());
        Arc::new(app)
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
            || self.app(name).and_then(|a| a.failed()).as_deref() == Some("needs sign-in")
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
    /// next model call, in a running session too. One already connected under the name
    /// is replaced.
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
            old.retire().await;
        }
    }

    fn changed(&self) {
        run(&self.on_change);
    }

    /// The apps whose tools are offered, in the file's order: the first to name a tool
    /// keeps the name. One whose connection died stays, with the tools it had: the next
    /// call opens it again.
    fn in_order(&self) -> Vec<Arc<App>> {
        let apps = self.apps();
        let mut order: Vec<String> = self.specs().into_iter().map(|s| s.name).collect();
        let mut extra: Vec<&String> = apps.keys().filter(|n| !order.contains(n)).collect();
        extra.sort();
        order.extend(extra.into_iter().cloned());
        // Ghost's own app first (ADR 0045): an app called `ghost_docs` can't take
        // `ghost_docs_search` from it.
        order.sort_by_key(|n| !apps.get(n).is_some_and(|a| a.is_own())); // stable
        order
            .iter()
            .filter_map(|n| apps.get(n).cloned())
            .filter(|a| a.known())
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
        let apps = self.in_order();
        let hidden = self.hidden(&apps);
        apps.iter()
            .map(|a| source_without(a, &hidden))
            .filter(|s| !s.read_tools.is_empty() || !s.operations.is_empty())
            .collect()
    }

    // --- many tools (ADR 0046) ------------------------------------------------------------

    /// The tools not offered by name right now, by the names the model would call them.
    /// While the connected apps' tools are within the limits (`tools_max` of them,
    /// `tools_bytes` of schema) that is only Ghost's own search tool, which nothing
    /// needs. Past either limit it is every app tool the operator has not pinned
    /// (`pinned` in `apps.json`) and the model has not used or found lately; those are
    /// found with the search tool, and stay callable by name.
    fn hidden(&self, apps: &[Arc<App>]) -> HashSet<String> {
        let own = apps.iter().find(|a| a.is_own());
        let search = own.map(|a| a.public_name(TOOLS_SEARCH));
        let recent = self.recent.names();
        // Ghost's own tools past its core (ADR 0052) are found by search like an app's,
        // whatever the limits: the core is offered by name, and the rest once used or
        // found. The search itself is in the core.
        let mut hidden: HashSet<String> = HashSet::new();
        if let Some(own) = own {
            for (tool, name) in own.offered() {
                if !crate::ghost_app::CORE.contains(&tool.as_str()) && !recent.contains(&name) {
                    hidden.insert(name);
                }
            }
        }
        let outside: Vec<&Arc<App>> = apps.iter().filter(|a| !a.is_own()).collect();
        let count: usize = outside.iter().map(|a| a.offered().len()).sum();
        let bytes: usize = outside.iter().map(|a| a.bytes()).sum();
        let over = count > self.limits.tools_max || bytes > self.limits.tools_bytes;
        // Within the limits, or with no search tool to find them by (Ghost's own app is not
        // running), every app's tool stays offered; the search is offered only when
        // something is hidden.
        if !over || search.is_none() {
            if hidden.is_empty() {
                hidden.extend(search);
            }
            return hidden;
        }
        for app in outside {
            for (tool, name) in app.offered() {
                if !app.spec.pinned.contains(&tool) && !recent.contains(&name) {
                    hidden.insert(name);
                }
            }
        }
        hidden
    }

    /// Whether most of the connected apps' tools are found by search right now (Ghost's
    /// own past its core always are, ADR 0052).
    pub fn deferring(&self) -> bool {
        self.claim_names();
        let apps = self.in_order();
        let own: HashSet<String> = apps
            .iter()
            .filter(|a| a.is_own())
            .flat_map(|a| a.offered().into_iter().map(|(_, n)| n))
            .collect();
        self.hidden(&apps).iter().any(|n| !own.contains(n))
    }

    /// The connected apps' tools that match `query`, best first: each with the name to
    /// call it by, what it does and the arguments it takes. Every word of the query is
    /// looked for in the tool's name (which counts three times), its app and its
    /// description. What is found is offered by name from the next model call. Ghost's
    /// own tools past the core are searched too (ADR 0052); the core is always offered.
    pub fn search_tools(&self, query: &str) -> Vec<Value> {
        self.claim_names();
        let words: Vec<String> = query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
            .map(String::from)
            .collect();
        let mut hits: Vec<(usize, usize, Value, String)> = Vec::new();
        let mut order = 0;
        for app in self.in_order().iter() {
            let offered = app.offered();
            for t in app.tools() {
                let Some((_, name)) = offered.iter().find(|(n, _)| *n == t.name) else {
                    continue;
                };
                if app.is_own() && crate::ghost_app::CORE.contains(&t.name.as_str()) {
                    continue; // always offered
                }
                order += 1;
                // Ghost's own description is Ghost's text, whole (ADR 0045).
                let said = if app.is_own() {
                    t.description.clone()
                } else {
                    t.offered_description()
                };
                let (in_name, in_app, in_text) = (
                    name.to_lowercase(),
                    app.spec.shown().to_lowercase(),
                    said.to_lowercase(),
                );
                let score: usize = words
                    .iter()
                    .map(|w| {
                        3 * usize::from(in_name.contains(w))
                            + usize::from(in_app.contains(w))
                            + usize::from(in_text.contains(w))
                    })
                    .sum();
                if score == 0 {
                    continue;
                }
                hits.push((
                    score,
                    order,
                    json!({
                        "name": name,
                        "app": app.spec.shown(),
                        "kind": if t.read_only { "read" } else { "change" },
                        "description": said,
                        "arguments": t.properties(),
                        "required": t.required(),
                    }),
                    name.clone(),
                ));
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.truncate(SEARCH_RESULTS);
        // Best last, so the best is the newest and the last to be forgotten.
        for (_, _, _, name) in hits.iter().take(SEARCH_KEPT).rev() {
            self.recent.touch(name);
        }
        hits.into_iter().map(|(_, _, tool, _)| tool).collect()
    }

    pub fn connected(&self) -> bool {
        self.apps().values().any(|a| a.connected())
    }

    pub fn is_up(&self, name: &str) -> bool {
        self.app(name).is_some_and(|a| a.connected())
    }

    /// Start an app again whose connection died or that failed at start (a watch reads
    /// it, ADR 0028). The caller backs off; this tries once, without a browser. An app
    /// that was connected is opened again in place, so the tools a session holds keep
    /// working.
    pub async fn revive(&self, name: &str) -> bool {
        if let Some(app) = self.app(name) {
            if app.connected() {
                return true;
            }
            return app.reconnect().await.is_ok(); // tells the listeners itself
        }
        let Some(spec) = self.spec(name) else {
            return false;
        };
        let app = self.start(spec, false).await;
        if app.is_some() {
            info!("app {name}: connected again");
            self.changed();
        }
        app.is_some()
    }

    // --- keeping connections up (ADR 0046) ------------------------------------------------

    /// Keep the apps connected, in the background: every few seconds each connection is
    /// looked at (an app over HTTP that has been quiet is pinged), and one that died or
    /// never started is tried again, the wait doubling up to a minute for five tries and
    /// then every five minutes (`Backoff`; the numbers are Hermes Agent's). An app that
    /// needs a new sign-in is left for the operator, on the waiting list. Stops when the
    /// apps close.
    pub fn supervise(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let every = self.limits.backoff.watch;
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(apps) = weak.upgrade() else { return };
                apps.look().await;
            }
        });
        if let Some(old) = self
            .supervisor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(task)
        {
            old.abort();
        }
    }

    /// One look at every app (`supervise`).
    pub async fn look(&self) {
        let backoff = self.limits.backoff;
        let names: Vec<String> = self.specs().into_iter().map(|s| s.name).collect();
        for name in &names {
            let app = self.app(name);
            if let Some(app) = &app
                && app.connected()
                && app.check(self.limits.keepalive).await
            {
                self.forget_wait(name);
                continue;
            }
            let connecting = self
                .connecting
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(name);
            if connecting || self.needs_sign_in(name) {
                self.forget_wait(name); // the operator's to do; nothing to retry
                continue;
            }
            let now = Instant::now();
            let due = {
                let mut waits = self.waits.lock().unwrap_or_else(|e| e.into_inner());
                let (_, at) = waits
                    .entry(name.clone())
                    .or_insert_with(|| (0, now + backoff.jittered(0, rand::random::<f64>())));
                now >= *at
            };
            if !due {
                continue;
            }
            if self.revive(name).await {
                self.forget_wait(name);
                continue;
            }
            let mut waits = self.waits.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((tried, at)) = waits.get_mut(name) {
                *tried += 1;
                *at = Instant::now() + backoff.jittered(*tried, rand::random::<f64>());
            }
        }
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|n, _| names.contains(n));
    }

    fn forget_wait(&self, name: &str) {
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }

    /// How often an app that is down has been tried, for a test and the log.
    pub fn tries(&self, name: &str) -> Option<u32> {
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .map(|(n, _)| *n)
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
        if let Some(task) = self
            .supervisor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
        let apps: Vec<Arc<App>> = self
            .apps
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, a)| a)
            .collect();
        for app in apps {
            app.retire().await;
        }
    }
}

fn run(hooks: &RwLock<Vec<ChangeHook>>) {
    let hooks = hooks.read().unwrap_or_else(|e| e.into_inner()).clone();
    for hook in hooks {
        hook();
    }
}
