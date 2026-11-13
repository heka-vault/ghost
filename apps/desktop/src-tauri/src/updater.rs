//! Updates (ADR 0066): the installed app checks the latest GitHub release's `latest.json`
//! when it starts and once a day, says quietly that a new version is there, and installs it
//! only when the operator presses "Update and restart". The window is the only caller: no
//! tool, model, app or outside client has a way to these commands, as none has a way to a
//! permission.
//!
//! The check is one GET to the release's address (GitHub answers it with a redirect to its
//! file host), with the updater's own user agent and nothing else that names this computer
//! or the operator. It runs in the background after the window is up, so it never delays a
//! start or a turn. The switch "Check for updates" (on unless the operator turned it off) is
//! a word in `~/.local/state/ghost/update-check`, beside "Start at login"'s.
//!
//! The install: the files are downloaded and their signature checked against the public key
//! the app carries (`tauri.conf.json`); then, if a session or a run is going, it waits for
//! it to end and says so; then the new version is installed (the AppImage replaced in place,
//! or the .deb through `pkexec dpkg -i`, which asks for the operator's password once), and
//! the app restarts through its ordinary exit, which stops the daemon it runs; the new app
//! starts the new daemon. `~/.config/ghost` and `~/.local/share/ghost` are not touched: the
//! new daemon migrates what it needs as on any start. A failure says what happened and
//! leaves the running version as it was.
//!
//! A build from the checkout (`make dev`, `cargo build`) is not a bundle and never offers an
//! update: it says it is a development build.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const EVENT: &str = "ghost://update";
/// After the window and the daemon are up.
const FIRST_CHECK: Duration = Duration::from_secs(45);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often a waiting install asks again whether the session or run has ended.
const WAIT_STEP: Duration = Duration::from_secs(5);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Checking,
    /// A session or a run is going: the install waits for it.
    Waiting {
        reason: String,
    },
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    /// The .deb asks for the operator's password here.
    Installing,
    Restarting,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offer {
    pub version: String,
    /// The release notes as the release wrote them, shown as plain text.
    pub notes: String,
    /// RFC 3339, when the release was published.
    pub date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateView {
    pub current: String,
    /// How this copy was installed: `deb` or `appimage`; None for a development build.
    pub installer: Option<&'static str>,
    /// Why this build never offers an update, in words.
    pub unavailable: Option<String>,
    /// The operator's "Check for updates".
    pub check: bool,
    /// Seconds since the epoch of the last check that got an answer.
    pub checked_at: Option<u64>,
    pub available: Option<Offer>,
    pub phase: Phase,
}

pub struct Updater {
    view: Mutex<UpdateView>,
    update: Mutex<Option<Update>>,
    installing: AtomicBool,
    cancel_wait: AtomicBool,
}

fn state_file() -> PathBuf {
    crate::supervisor::logfile::default_path().with_file_name("update-check")
}

/// The operator's switch, on unless the state file says `off`.
pub fn wanted() -> bool {
    std::fs::read_to_string(state_file())
        .map(|s| s.trim() != "off")
        .unwrap_or(true)
}

fn installer() -> Option<&'static str> {
    if cfg!(debug_assertions) {
        return None;
    }
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::Deb) => Some("deb"),
        Some(BundleType::AppImage) => Some("appimage"),
        _ => None,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What stops an install now: a session open, or a run going (ADR 0047). From the
/// daemon's `GET /status` and `GET /runs`; a daemon that does not answer stops nothing.
pub fn busy(
    status: Option<&serde_json::Value>,
    runs: Option<&serde_json::Value>,
) -> Option<String> {
    if let Some(s) = status
        && s.get("session_id").is_some_and(|v| !v.is_null())
    {
        return Some("Ghost will update when this conversation ends.".to_string());
    }
    let running = runs
        .and_then(|r| r.get("runs"))
        .and_then(|r| r.as_array())
        .is_some_and(|runs| {
            runs.iter()
                .any(|r| r.get("status").and_then(|s| s.as_str()) == Some("running"))
        });
    running.then(|| "Ghost will update when the agent's run ends.".to_string())
}

async fn daemon_json(path: &str) -> Option<serde_json::Value> {
    let r = crate::bridge::api_request("GET".into(), path.into(), None)
        .await
        .ok()?;
    (r.status == 200)
        .then(|| serde_json::from_str(&r.body).ok())
        .flatten()
}

async fn busy_now() -> Option<String> {
    let status = daemon_json("/status").await;
    let runs = daemon_json("/runs?limit=20").await;
    busy(status.as_ref(), runs.as_ref())
}

impl Updater {
    pub fn new(app: &AppHandle) -> Self {
        let installer = installer();
        Self {
            view: Mutex::new(UpdateView {
                current: app.package_info().version.to_string(),
                installer,
                unavailable: installer.is_none().then(|| {
                    "This is a development build; it does not update itself. Pull and rebuild \
                     the checkout instead."
                        .to_string()
                }),
                check: wanted(),
                checked_at: None,
                available: None,
                phase: Phase::Idle,
            }),
            update: Mutex::new(None),
            installing: AtomicBool::new(false),
            cancel_wait: AtomicBool::new(false),
        }
    }

    pub fn view(&self) -> UpdateView {
        self.view.lock().unwrap().clone()
    }

    fn set(&self, app: &AppHandle, f: impl FnOnce(&mut UpdateView)) {
        let v = {
            let mut v = self.view.lock().unwrap();
            f(&mut v);
            v.clone()
        };
        let _ = app.emit(EVENT, v);
    }
}

fn updater(app: &AppHandle) -> tauri::State<'_, Updater> {
    app.state::<Updater>()
}

/// One check. Quiet: a failure is kept on the view, never a notification.
async fn check(app: &AppHandle) {
    let u = updater(app);
    if u.view().installer.is_none() || u.installing.load(Ordering::SeqCst) {
        return;
    }
    u.set(app, |v| v.phase = Phase::Checking);
    let found = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
        Ok(up) => up.check().await,
        Err(e) => Err(e),
    };
    match found {
        Ok(Some(update)) => {
            let offer = Offer {
                version: update.version.clone(),
                notes: update.body.clone().unwrap_or_default(),
                date: update
                    .raw_json
                    .get("pub_date")
                    .and_then(|d| d.as_str())
                    .map(str::to_string),
            };
            eprintln!("ghost: update {} is available", offer.version);
            *u.update.lock().unwrap() = Some(update);
            u.set(app, |v| {
                v.available = Some(offer);
                v.checked_at = Some(now());
                v.phase = Phase::Idle;
            });
        }
        Ok(None) => {
            *u.update.lock().unwrap() = None;
            u.set(app, |v| {
                v.available = None;
                v.checked_at = Some(now());
                v.phase = Phase::Idle;
            });
        }
        Err(e) => {
            eprintln!("ghost: update check: {e}");
            u.set(app, |v| {
                v.phase = Phase::Failed {
                    error: format!("Ghost could not check for updates: {e}"),
                }
            });
        }
    }
}

/// When the app starts and once a day after, while the operator's switch is on.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if updater(&app).view().check {
                check(&app).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command]
pub fn update_view(app: AppHandle) -> UpdateView {
    updater(&app).view()
}

/// Settings' "Check now".
#[tauri::command]
pub async fn update_check(app: AppHandle) -> UpdateView {
    check(&app).await;
    updater(&app).view()
}

/// Settings' "Check for updates".
#[tauri::command]
pub fn update_set_check(app: AppHandle, on: bool) -> Result<UpdateView, String> {
    let path = state_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, if on { "on\n" } else { "off\n" })
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let u = updater(&app);
    u.set(&app, |v| v.check = on);
    Ok(u.view())
}

/// Stop waiting for a session or run to end; nothing is installed.
#[tauri::command]
pub fn update_cancel(app: AppHandle) {
    updater(&app).cancel_wait.store(true, Ordering::SeqCst);
}

/// "Update and restart": the operator's button, and nothing else's.
#[tauri::command]
pub fn update_install(app: AppHandle) -> Result<UpdateView, String> {
    let u = updater(&app);
    let Some(update) = u.update.lock().unwrap().clone() else {
        return Err("There is no update to install; check again.".into());
    };
    if u.installing.swap(true, Ordering::SeqCst) {
        return Ok(u.view());
    }
    u.cancel_wait.store(false, Ordering::SeqCst);
    let task = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = install(&task, update).await {
            eprintln!("ghost: update: {error}");
            let u = updater(&task);
            u.set(&task, |v| v.phase = Phase::Failed { error });
            u.installing.store(false, Ordering::SeqCst);
        }
    });
    Ok(u.view())
}

async fn install(app: &AppHandle, update: Update) -> Result<(), String> {
    let u = updater(app);
    let version = update.version.clone();
    let current = u.view().current;
    // 1. The files, their signature checked against the app's key before anything else.
    u.set(app, |v| {
        v.phase = Phase::Downloading {
            done: 0,
            total: None,
        }
    });
    let mut done = 0u64;
    let mut shown = 0u64;
    let bytes = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                // A redraw per percent at most.
                let step = total.map_or(1 << 20, |t| (t / 100).max(1));
                if done >= shown + step || total == Some(done) {
                    shown = done;
                    updater(app).set(app, |v| v.phase = Phase::Downloading { done, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| match e {
            tauri_plugin_updater::Error::Minisign(_)
            | tauri_plugin_updater::Error::SignedVersionMismatch { .. }
            | tauri_plugin_updater::Error::MissingSignedVersion => format!(
                "The download of {version} is not signed with Ghost's key, so nothing was \
                 installed. Ghost {current} keeps running. ({e})"
            ),
            e => format!(
                "The download of {version} failed: {e}. Ghost {current} keeps running; try again."
            ),
        })?;
    // 2. Never in the middle of a session or a run.
    loop {
        if u.cancel_wait.load(Ordering::SeqCst) {
            u.installing.store(false, Ordering::SeqCst);
            u.set(app, |v| v.phase = Phase::Idle);
            return Ok(());
        }
        match busy_now().await {
            Some(reason) => {
                u.set(app, |v| v.phase = Phase::Waiting { reason });
                tokio::time::sleep(WAIT_STEP).await;
            }
            None => break,
        }
    }
    // 3. Install. The running app and daemon keep running until the restart.
    u.set(app, |v| v.phase = Phase::Installing);
    tauri::async_runtime::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            format!(
                "Installing {version} did not finish: {e}. Ghost {current} keeps running; \
                 nothing was changed."
            )
        })?;
    // 4. Restart through the ordinary exit: the daemon stops with the old app, and the new
    // app starts the new daemon.
    eprintln!("ghost: update {version} installed; restarting");
    u.set(app, |v| v.phase = Phase::Restarting);
    app.request_restart();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_session_or_a_running_run_holds_the_install() {
        assert_eq!(
            busy(None, None),
            None,
            "a daemon that does not answer holds nothing"
        );
        let idle = json!({"session_id": null, "phase": "passive"});
        assert_eq!(busy(Some(&idle), Some(&json!({"runs": []}))), None);
        let talking = json!({"session_id": "s1"});
        assert!(busy(Some(&talking), None).unwrap().contains("conversation"));
        let runs = json!({"runs": [{"status": "done"}, {"status": "running"}]});
        assert!(busy(Some(&idle), Some(&runs)).unwrap().contains("run"));
        let over = json!({"runs": [{"status": "done"}, {"status": "failed"}]});
        assert_eq!(busy(Some(&idle), Some(&over)), None);
    }

    #[test]
    fn a_test_build_is_a_development_build() {
        // `cargo test` builds with debug assertions and no bundle marker.
        assert_eq!(installer(), None);
    }
}
