//! Error reports (ADR 0067): where Ghost did not work on someone's computer, sent to the
//! project's error tracking (PostHog, US cloud) from a released build, and nothing else.
//!
//! What is reported is a closed set: a panic in the daemon or the desktop app's processes,
//! a page's uncaught error (handed to the shell, which builds the report), and the typed
//! failures of [`FailureKind`]. Every string passes [`scrub::Scrubber`] first; the report
//! carries Ghost's version, the component, and coarse facts about the computer
//! ([`report::Context`]), and a random install id made when reporting is on.
//!
//! **Only a released build sends.** The project's key is compiled in from the build's
//! environment (`GHOST_POSTHOG_KEY`, set by the release workflow from a repository secret);
//! without it there is no sender: a checkout's build, a fork's and CI's report nothing. For
//! a developer, the same variable in the process's environment turns it on, with
//! `GHOST_REPORTS_URL` pointing it at a local stub.
//!
//! **The operator's switch** is a file in Ghost's data folder: on unless `reports/off`
//! exists. Off sends nothing, spools nothing, and the install id and the spool are deleted.
//! Both the daemon and the shell read it before each report and each send, so neither has to
//! ask the other. Only the operator reaches it: the control API, `ghostd reports`, and the
//! desktop's Settings; no tool, model, app, outside client or phone has a way to it.
//!
//! **Cheap and off every path that matters.** A report is written to the spool (the panic
//! hook does only that: no network in a panic); a background task sends the spool with a
//! short timeout, three tries a report at most, and gives up silently (a debug line). The
//! same fingerprint goes at most once an hour, at most 30 reports a day per install, and the
//! spool holds 50 reports or 512 KB. The last 20 sent are kept, exactly as sent but for the
//! project's key, for the operator to read.

pub mod frames;
pub mod panic;
pub mod report;
pub mod scrub;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

pub use report::{
    Component, Context, Failure, FailureKind, PageError, PageSource, What, status_class,
};
pub use scrub::{Redact, Scrubber};

/// PostHog's US cloud.
pub const DEFAULT_URL: &str = "https://us.i.posthog.com";
/// The capture endpoint for one event.
pub const CAPTURE_PATH: &str = "/i/v0/e/";
pub const KEY_VAR: &str = "GHOST_POSTHOG_KEY";
pub const URL_VAR: &str = "GHOST_REPORTS_URL";

pub const MAX_PER_DAY: u32 = 30;
pub const SAME_EVERY: Duration = Duration::from_secs(60 * 60);
pub const MAX_SPOOL: usize = 50;
pub const MAX_SPOOL_BYTES: u64 = 512 * 1024;
pub const KEEP_SENT: usize = 20;
pub const MAX_TRIES: u32 = 3;
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the running sender looks at the spool when nothing woke it.
pub const LOOK_EVERY: Duration = Duration::from_secs(60);

/// What the kept reports show in place of the project's key.
pub const KEY_SHOWN_AS: &str = "(Ghost's project key)";

/// The key this build was compiled with: the release workflow's only.
pub fn compiled_key() -> Option<&'static str> {
    option_env!("GHOST_POSTHOG_KEY").filter(|k| !k.trim().is_empty())
}

/// The compiled key, else the process's `GHOST_POSTHOG_KEY` (a developer trying it).
pub fn key_from(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    compiled_key()
        .map(str::to_string)
        .or_else(|| env(KEY_VAR).filter(|k| !k.trim().is_empty()))
}

/// `GHOST_REPORTS_URL`, else PostHog's US cloud.
pub fn url_from(env: impl Fn(&str) -> Option<String>) -> String {
    env(URL_VAR)
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// `$GHOST_DATA_DIR`, else `~/.local/share/ghost`, the daemon's default (ghostd config).
pub fn default_data_dir() -> PathBuf {
    match std::env::var_os("GHOST_DATA_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from("/"), PathBuf::from)
            .join(".local/share/ghost"),
    }
}

/// One report as it was sent, for the operator to read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub sent_at: DateTime<Utc>,
    /// The request's body as sent, the project's key shown as [`KEY_SHOWN_AS`].
    pub event: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Spooled {
    tries: u32,
    built: report::Built,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Ledger {
    day: String,
    count: u32,
    /// Fingerprint → when it was last sent (seconds since the epoch).
    seen: BTreeMap<String, i64>,
}

/// One process's reporter over Ghost's data folder.
pub struct Reporter {
    dir: PathBuf,
    key: Option<String>,
    endpoint: String,
    ctx: Context,
    scrub: Scrubber,
    wake: tokio::sync::Notify,
    http: OnceLock<reqwest::Client>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reporter")
            .field("dir", &self.dir)
            .field("available", &self.key.is_some())
            .finish()
    }
}

impl Reporter {
    /// `data_dir` is Ghost's data folder; reports live in `reports/` under it. With no
    /// `key` there is no sender: nothing is spooled, kept or sent.
    pub fn new(
        data_dir: &Path,
        key: Option<String>,
        url: &str,
        ctx: Context,
        scrub: Scrubber,
    ) -> Self {
        Self {
            dir: data_dir.join("reports"),
            key,
            endpoint: format!("{}{CAPTURE_PATH}", url.trim_end_matches('/')),
            ctx,
            scrub,
            wake: tokio::sync::Notify::new(),
            http: OnceLock::new(),
        }
    }

    /// This process's, from its environment: the key, the URL and the data folder.
    pub fn from_env(version: &str, component: Component, scrub: Scrubber) -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self::new(
            &default_data_dir(),
            key_from(env),
            &url_from(env),
            Context::detect(version, component),
            scrub,
        )
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// This build can send at all.
    pub fn available(&self) -> bool {
        self.key.is_some()
    }

    /// The operator's switch: on unless they turned it off.
    pub fn switched_on(&self) -> bool {
        !self.dir.join("off").exists()
    }

    /// Reports are made and sent.
    pub fn on(&self) -> bool {
        self.available() && self.switched_on()
    }

    /// The operator's switch. Off deletes the install id and the spool; on makes a new id
    /// with the next report.
    pub fn set(&self, on: bool) -> std::io::Result<()> {
        mkdir(&self.dir)?;
        if on {
            match std::fs::remove_file(self.dir.join("off")) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        } else {
            write_private(&self.dir.join("off"), b"off\n")?;
            let _ = std::fs::remove_file(self.dir.join("install-id"));
            let _ = std::fs::remove_dir_all(self.dir.join("spool"));
            let _ = std::fs::remove_file(self.dir.join("ledger.json"));
        }
        Ok(())
    }

    /// The install's random id, made the first time it is needed while on. Not derived
    /// from the computer, the user or anything else.
    pub fn install_id(&self) -> std::io::Result<String> {
        let path = self.dir.join("install-id");
        if let Ok(s) = std::fs::read_to_string(&path)
            && uuid::Uuid::parse_str(s.trim()).is_ok()
        {
            return Ok(s.trim().to_string());
        }
        mkdir(&self.dir)?;
        let id = uuid::Uuid::new_v4().to_string();
        write_private(&path, format!("{id}\n").as_bytes())?;
        Ok(id)
    }

    /// The install id if one exists now (never makes one).
    pub fn current_install_id(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("install-id"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Build the report and put it in the spool; the sender takes it from there. File
    /// work only, safe in a panic hook. Returns whether it was spooled.
    pub fn report(&self, what: &What) -> bool {
        if !self.on() {
            return false;
        }
        let Ok(id) = self.install_id() else {
            return false;
        };
        let built = report::build(what, &self.ctx, &id, Utc::now(), &self.scrub);
        let spooled = self.spool(built).unwrap_or_else(|e| {
            debug!("error reports: not spooled: {e}");
            false
        });
        if spooled {
            self.wake.notify_one();
        }
        spooled
    }

    /// The report, built as it would be sent now, for `ghostd reports test` to print.
    pub fn preview(&self, what: &What) -> Option<Value> {
        let id = self.install_id().ok()?;
        let built = report::build(what, &self.ctx, &id, Utc::now(), &self.scrub);
        Some(self.body(&built.event, true))
    }

    fn spool_dir(&self) -> PathBuf {
        self.dir.join("spool")
    }

    fn spool(&self, built: report::Built) -> std::io::Result<bool> {
        let dir = self.spool_dir();
        mkdir(&dir)?;
        let fp = built.fingerprint.clone();
        let (mut count, mut bytes) = (0usize, 0u64);
        for e in std::fs::read_dir(&dir)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // One of a kind waiting is enough.
            if name.contains(&format!("-{fp}.json")) {
                return Ok(false);
            }
            count += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
        let body = serde_json::to_vec(&Spooled { tries: 0, built })?;
        if count >= MAX_SPOOL || bytes + body.len() as u64 > MAX_SPOOL_BYTES {
            return Ok(false);
        }
        let name = format!("{:020}-{fp}.json", Utc::now().timestamp_micros());
        write_private(&dir.join(name), &body)?;
        Ok(true)
    }

    /// The reports waiting to be sent.
    pub fn pending(&self) -> usize {
        std::fs::read_dir(self.spool_dir())
            .map(|d| {
                d.flatten()
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The last reports sent, newest first.
    pub fn kept(&self) -> Vec<Kept> {
        let mut names = sorted(&self.dir.join("sent"));
        names.reverse();
        names
            .iter()
            .filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
            .collect()
    }

    fn body(&self, event: &Value, shown: bool) -> Value {
        let mut body = event.clone();
        body["api_key"] = Value::String(if shown {
            KEY_SHOWN_AS.to_string()
        } else {
            self.key.clone().unwrap_or_default()
        });
        body
    }

    fn http(&self) -> &reqwest::Client {
        self.http.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(SEND_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .user_agent(concat!("ghost-reports/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default()
        })
    }

    /// Send one event now; true on a 2xx.
    async fn post(&self, event: &Value) -> bool {
        match self
            .http()
            .post(&self.endpoint)
            .json(&self.body(event, false))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => true,
            Ok(r) => {
                debug!("error reports: the service answered {}", r.status());
                false
            }
            Err(e) => {
                debug!("error reports: not sent ({})", reqwest_kind(&e));
                false
            }
        }
    }

    /// `ghostd reports test`: one marked test report, sent now (the bounds apply). Returns
    /// the body as kept, or why nothing was sent.
    pub async fn send_test(&self) -> Result<Value, String> {
        if !self.available() {
            return Err("this build has no error report key: nothing is sent".into());
        }
        if !self.switched_on() {
            return Err("error reports are off (ghostd reports on)".into());
        }
        if !self.report(&What::Test) {
            return Err(
                "a test report is already waiting, or the spool is full: try again later".into(),
            );
        }
        self.send_pending().await;
        self.kept()
            .into_iter()
            .find(|k| k.event["properties"]["ghost_kind"] == "test")
            .map(|k| k.event)
            .ok_or_else(|| {
                "not sent: the service did not answer, or the hourly or daily bound held it".into()
            })
    }

    /// Send what the spool holds, oldest first. Returns how many went.
    pub async fn send_pending(&self) -> usize {
        if !self.available() {
            return 0;
        }
        self.reclaim_stale();
        let mut sent = 0;
        for path in sorted(&self.spool_dir()) {
            if !self.on() {
                let _ = std::fs::remove_dir_all(self.spool_dir());
                break;
            }
            // Claim it: the other process sending from the same folder skips it.
            let claim = path.with_extension(format!("json.{}.sending", std::process::id()));
            if std::fs::rename(&path, &claim).is_err() {
                continue;
            }
            let Some(mut spooled) = std::fs::read(&claim)
                .ok()
                .and_then(|b| serde_json::from_slice::<Spooled>(&b).ok())
            else {
                let _ = std::fs::remove_file(&claim);
                continue;
            };
            let now = Utc::now();
            let mut ledger = self.ledger();
            let today = now.format("%Y-%m-%d").to_string();
            if ledger.day != today {
                ledger = Ledger {
                    day: today,
                    ..Ledger::default()
                };
            }
            ledger
                .seen
                .retain(|_, t| now.timestamp() - *t < SAME_EVERY.as_secs() as i64);
            let fp = spooled.built.fingerprint.clone();
            if ledger.count >= MAX_PER_DAY || ledger.seen.contains_key(&fp) {
                debug!("error reports: {fp} held by the bounds, dropped");
                let _ = std::fs::remove_file(&claim);
                continue;
            }
            if self.post(&spooled.built.event).await {
                sent += 1;
                ledger.count += 1;
                ledger.seen.insert(fp, now.timestamp());
                let _ = self.save_ledger(&ledger);
                let _ = self.keep(&spooled.built.event, now);
                let _ = std::fs::remove_file(&claim);
            } else {
                spooled.tries += 1;
                if spooled.tries >= MAX_TRIES {
                    let _ = std::fs::remove_file(&claim);
                } else if let Ok(b) = serde_json::to_vec(&spooled) {
                    let _ = write_private(&path, &b);
                    let _ = std::fs::remove_file(&claim);
                }
                // The service or the network is down: try the rest later.
                break;
            }
        }
        sent
    }

    /// A claim whose process is gone goes back to the spool.
    fn reclaim_stale(&self) {
        for e in std::fs::read_dir(self.spool_dir())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".sending") else {
                continue;
            };
            let Some((json, pid)) = stem.rsplit_once('.') else {
                continue;
            };
            let alive = pid.parse::<u32>().is_ok_and(|p| {
                p == std::process::id() || Path::new(&format!("/proc/{p}")).exists()
            });
            if !alive {
                let _ = std::fs::rename(e.path(), self.spool_dir().join(json));
            }
        }
    }

    fn ledger(&self) -> Ledger {
        std::fs::read(self.dir.join("ledger.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_ledger(&self, l: &Ledger) -> std::io::Result<()> {
        write_private(&self.dir.join("ledger.json"), &serde_json::to_vec(l)?)
    }

    fn keep(&self, event: &Value, at: DateTime<Utc>) -> std::io::Result<()> {
        let dir = self.dir.join("sent");
        mkdir(&dir)?;
        let kept = Kept {
            sent_at: at,
            event: self.body(event, true),
        };
        let name = format!("{:020}.json", at.timestamp_micros());
        write_private(&dir.join(name), &serde_json::to_vec_pretty(&kept)?)?;
        let names = sorted(&dir);
        if names.len() > KEEP_SENT {
            for old in &names[..names.len() - KEEP_SENT] {
                let _ = std::fs::remove_file(old);
            }
        }
        Ok(())
    }

    /// The running sender: what a past run left, then whenever a report is spooled, and
    /// every minute for what another process spooled. Never awaited on quitting.
    pub async fn run(self: Arc<Self>) {
        if !self.available() {
            return;
        }
        loop {
            self.send_pending().await;
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(LOOK_EVERY) => {}
            }
        }
    }
}

fn reqwest_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else {
        "request"
    }
}

/// The `.json` files of a folder, in name order (oldest first: names start with the time).
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

fn mkdir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// 0600, written beside and renamed into place.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, path)
}

static GLOBAL: OnceLock<Arc<Reporter>> = OnceLock::new();
static RECENT: std::sync::Mutex<BTreeMap<String, std::time::Instant>> =
    std::sync::Mutex::new(BTreeMap::new());

/// This process's reporter, for [`failure`] from anywhere in it.
pub fn install(reporter: Arc<Reporter>) {
    let _ = GLOBAL.set(reporter);
}

pub fn global() -> Option<&'static Arc<Reporter>> {
    GLOBAL.get()
}

/// Report one of the closed list of failures, off the caller's path: a no-op in a process
/// with no reporter, or with reporting off.
pub fn failure(f: Failure) {
    let Some(r) = GLOBAL.get().cloned() else {
        return;
    };
    if !r.on() {
        return;
    }
    // A failure seen again within the hour is not even built: the hourly bound would drop
    // it, and a problem that persists is reported from code that runs often.
    {
        let key = format!("{:?}", (f.kind, &f.error, &f.facts));
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        recent.retain(|_, t| now.duration_since(*t) < SAME_EVERY);
        if recent.contains_key(&key) {
            return;
        }
        recent.insert(key, now);
    }
    let what = What::Failure(f);
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn_blocking(move || r.report(&what));
        }
        Err(_) => {
            r.report(&what);
        }
    }
}
