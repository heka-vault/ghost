//! The floating room (ADR 0059): the room posture without a window. Ghost is a thin line of
//! light at a screen's bottom edge until its name is called, then comes forward over the
//! desktop with no application chrome, and goes back to the line when the session ends.
//!
//! Off until the operator turns it on (Settings › General, the tray). It runs as a second
//! process of this app, `heka-ghost-desktop --floating`, with `GDK_BACKEND=x11`: on X11 and
//! XWayland a window can keep itself above the others, stay off the taskbar and every
//! workspace's switcher, pass clicks through except where Ghost is drawn and never take the
//! keyboard. GNOME's native Wayland has none of that for an ordinary app, so the main window
//! stays native and only this one goes through XWayland. Where there is no X11 at all it
//! refuses with a plain sentence.
//!
//! This process (the shell) owns the switch, starts and stops the surface, tells it when the
//! main window is the one in use and when the screen is being captured, and raises the main
//! window when the surface asks. The surface (`surface.rs`) reads the daemon's events itself,
//! through the same bridge, and decides with the app's one posture rule (`posture.ts`).

pub mod capture;
pub mod place;

#[cfg(target_os = "linux")]
pub mod desktop;
#[cfg(target_os = "linux")]
pub mod surface;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use capture::Capture;
use place::{Monitor, MonitorPref};

/// The argument that makes this binary the surface.
pub const ARG: &str = "--floating";
/// The argument that prints what the desktop tells the floating room, and exits.
pub const CHECK_ARG: &str = "--floating-check";

/// The operator's choices, kept beside the window's own state
/// (`~/.config/dev.heka.ghost/floating.json`): how this desk shows Ghost, not Ghost's setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// The floating room is on. Off by default.
    pub on: bool,
    /// Which monitor it comes forward on.
    pub monitor: MonitorPref,
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

/// Whether this desktop can float a window properly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Support {
    /// An X11 session.
    X11,
    /// A Wayland session with XWayland (GNOME runs it on demand): the surface goes through it.
    Xwayland,
    /// Not here, and why, in the operator's words.
    No { why: String },
}

/// From the session's environment: `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `DISPLAY`.
pub fn support_from(
    session: Option<&str>,
    wayland: Option<&str>,
    display: Option<&str>,
) -> Support {
    if !cfg!(target_os = "linux") {
        return Support::No {
            why: "The floating room works on Linux desktops for now.".into(),
        };
    }
    let has = |v: Option<&str>| v.is_some_and(|s| !s.trim().is_empty());
    let wayland_session = session == Some("wayland") || has(wayland);
    match (wayland_session, has(display)) {
        (false, true) => Support::X11,
        (true, true) => Support::Xwayland,
        (true, false) => Support::No {
            why: "The floating room needs XWayland, and this desktop isn't running it.".into(),
        },
        (false, false) => Support::No {
            why: "The floating room needs a desktop to float over, and none is running.".into(),
        },
    }
}

pub fn support() -> Support {
    let var = |n: &str| std::env::var(n).ok();
    support_from(
        var("XDG_SESSION_TYPE").as_deref(),
        var("WAYLAND_DISPLAY").as_deref(),
        var("DISPLAY").as_deref(),
    )
}

/// The shell to the surface, one JSON object per line on its stdin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum ToSurface {
    /// The main window is the one the operator is using (focused and on screen).
    MainActive {
        active: bool,
    },
    Capture {
        capture: Capture,
    },
    Settings {
        settings: Settings,
    },
}

/// The surface to the shell, one JSON object per line on its stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum FromSurface {
    Ready {
        monitors: Vec<Monitor>,
    },
    /// "Open the app": bring the main window to this workspace and to the front.
    OpenApp,
}

/// What Settings and the posture rule in the main window read (`floating_status`, and the
/// `ghost://floating` event when it changes).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub support: Support,
    pub settings: Settings,
    /// The surface is up.
    pub running: bool,
    pub capture: Capture,
    /// The monitors the surface can come forward on, once it has started.
    pub monitors: Vec<Monitor>,
    /// The optional GNOME Shell helper is answering (it says where the operator is working).
    pub helper: bool,
}

impl Status {
    /// The posture rule's `floating` input: on, up, and nothing capturing the screen.
    pub fn can_float(&self) -> bool {
        self.settings.on && self.running && self.capture != Capture::Yes
    }
}

struct Inner {
    status: Status,
    main_active: bool,
    /// The surface's stdin, while it runs.
    tx: Option<mpsc::UnboundedSender<String>>,
    /// Bumped on every start and stop, so an old surface's exit doesn't restart a new one.
    generation: u64,
    restarts: u32,
}

pub struct Host {
    app: AppHandle,
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl Host {
    pub fn new(app: AppHandle) -> Arc<Self> {
        let path = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("floating.json");
        let settings = load(&path);
        Arc::new(Self {
            app,
            path,
            inner: Mutex::new(Inner {
                status: Status {
                    support: support(),
                    settings,
                    running: false,
                    capture: Capture::Unknown,
                    monitors: Vec::new(),
                    helper: false,
                },
                main_active: false,
                tx: None,
                generation: 0,
                restarts: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    fn changed(&self) {
        let s = self.status();
        crate::tray::show_floating(&self.app, s.settings.on);
        let _ = self.app.emit("ghost://floating", s);
    }

    fn send(&self, msg: &ToSurface) {
        if let Some(tx) = &self.lock().tx {
            let _ = tx.send(serde_json::to_string(msg).unwrap_or_default());
        }
    }

    /// The operator's choice, from Settings or the tray.
    pub fn set(self: &Arc<Self>, settings: Settings) -> Result<Status, String> {
        save(&self.path, &settings).map_err(|e| format!("Couldn't keep the setting: {e}"))?;
        let was_on = {
            let mut i = self.lock();
            let was = i.status.settings.on;
            i.status.settings = settings.clone();
            i.restarts = 0;
            was
        };
        self.send(&ToSurface::Settings { settings });
        self.apply(was_on);
        self.changed();
        Ok(self.status())
    }

    pub fn set_on(self: &Arc<Self>, on: bool) {
        let mut s = self.status().settings;
        s.on = on;
        if let Err(e) = self.set(s) {
            eprintln!("ghost: floating: {e}");
        }
    }

    /// Start or stop the surface to match the switch.
    fn apply(self: &Arc<Self>, was_on: bool) {
        let (on, ok, running) = {
            let i = self.lock();
            (
                i.status.settings.on,
                !matches!(i.status.support, Support::No { .. }),
                i.tx.is_some(),
            )
        };
        if on && ok && !running {
            self.start();
        } else if !on && (running || was_on) {
            self.stop();
        }
    }

    pub fn main_active(&self, active: bool) {
        {
            let mut i = self.lock();
            if i.main_active == active {
                return;
            }
            i.main_active = active;
        }
        self.send(&ToSurface::MainActive { active });
    }

    fn stop(&self) {
        let mut i = self.lock();
        i.generation += 1;
        // Dropping stdin ends the surface: it exits when its input closes.
        i.tx = None;
        i.status.running = false;
    }

    fn start(self: &Arc<Self>) {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("ghost: floating: no executable to start: {e}");
                return;
            }
        };
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg(ARG)
            // The one window that goes through XWayland; the main window stays native.
            .env("GDK_BACKEND", "x11")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let _guard = tauri::async_runtime::handle().inner().enter();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ghost: floating: couldn't start the surface: {e}");
                return;
            }
        };
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let generation = {
            let mut i = self.lock();
            i.generation += 1;
            i.tx = Some(tx.clone());
            i.status.running = true;
            let first = [
                ToSurface::Settings {
                    settings: i.status.settings.clone(),
                },
                ToSurface::MainActive {
                    active: i.main_active,
                },
                ToSurface::Capture {
                    capture: i.status.capture,
                },
            ];
            for m in first {
                let _ = tx.send(serde_json::to_string(&m).unwrap_or_default());
            }
            i.generation
        };
        eprintln!("ghost: floating: surface started (pid {:?})", child.id());
        tauri::async_runtime::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            // The sender is gone (stopped): closing stdin ends the surface.
        });
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<FromSurface>(&line) {
                    Ok(FromSurface::Ready { monitors }) => {
                        host.lock().status.monitors = monitors;
                        host.changed();
                    }
                    Ok(FromSurface::OpenApp) => {
                        let app = host.app.clone();
                        let _ = host
                            .app
                            .run_on_main_thread(move || crate::tray::show_window(&app));
                    }
                    Err(_) => eprintln!("ghost: floating: {line}"),
                }
            }
        });
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            let exit = child.wait().await;
            eprintln!("ghost: floating: surface exited ({exit:?})");
            let again = {
                let mut i = host.lock();
                if i.generation != generation {
                    false
                } else {
                    i.tx = None;
                    i.status.running = false;
                    i.restarts += 1;
                    i.status.settings.on && i.restarts <= 5
                }
            };
            host.changed();
            if again {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let still = host.lock().generation == generation;
                if still {
                    host.start();
                    host.changed();
                }
            }
        });
        self.changed();
    }

    /// While the floating room is on: is the screen being captured, and is the GNOME Shell
    /// helper there? One look every two seconds; nothing at all while it is off.
    pub fn watch(self: &Arc<Self>) {
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            let conn = zbus::Connection::session().await.ok();
            loop {
                tokio::time::sleep(capture::EVERY).await;
                if !host.status().settings.on {
                    continue;
                }
                let (capture, helper) = match &conn {
                    Some(c) => (capture::probe(c).await, desktop_helper_present(c).await),
                    None => (Capture::Unknown, false),
                };
                let moved = {
                    let mut i = host.lock();
                    let moved = i.status.capture != capture || i.status.helper != helper;
                    i.status.capture = capture;
                    i.status.helper = helper;
                    moved
                };
                if moved {
                    eprintln!("ghost: floating: screen capture {capture:?}, helper {helper}");
                    host.send(&ToSurface::Capture { capture });
                    host.changed();
                }
            }
        });
    }
}

/// The GNOME Shell helper's bus name (`apps/desktop/gnome-shell/`).
pub const HELPER_NAME: &str = "dev.heka.Ghost.Desktop";
pub const HELPER_PATH: &str = "/dev/heka/Ghost/Desktop";

async fn desktop_helper_present(conn: &zbus::Connection) -> bool {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(HELPER_NAME) else {
        return false;
    };
    dbus.name_has_owner(name).await.unwrap_or(false)
}

/// Settings reads where things stand.
#[tauri::command]
pub fn floating_status(host: tauri::State<'_, Arc<Host>>) -> Status {
    host.status()
}

/// Settings changes the switch or the monitor.
#[tauri::command]
pub fn floating_set(
    host: tauri::State<'_, Arc<Host>>,
    settings: Settings,
) -> Result<Status, String> {
    host.inner().set(settings)
}

pub fn setup(app: &AppHandle) -> Arc<Host> {
    let host = Host::new(app.clone());
    app.manage(host.clone());
    let s = host.status();
    match &s.support {
        Support::No { why } if s.settings.on => eprintln!("ghost: floating: {why}"),
        _ => {}
    }
    host.apply(false);
    host.watch();
    host
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_floats_on_x11_and_through_xwayland_and_says_why_not_elsewhere() {
        assert_eq!(support_from(Some("x11"), None, Some(":0")), Support::X11);
        assert_eq!(
            support_from(Some("wayland"), Some("wayland-0"), Some(":0")),
            Support::Xwayland
        );
        assert!(matches!(
            support_from(Some("wayland"), Some("wayland-0"), None),
            Support::No { why } if why.contains("XWayland")
        ));
        assert!(matches!(
            support_from(Some("wayland"), Some("wayland-0"), Some("  ")),
            Support::No { .. }
        ));
        assert!(matches!(support_from(None, None, None), Support::No { .. }));
    }

    #[test]
    fn it_is_off_until_turned_on_and_keeps_the_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/floating.json");
        assert_eq!(load(&path), Settings::default());
        assert!(!load(&path).on);
        let s = Settings {
            on: true,
            monitor: MonitorPref::Named("HDMI-3".into()),
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        // A file this build can't read is the default: off.
        std::fs::write(&path, "{nonsense").unwrap();
        assert_eq!(load(&path), Settings::default());
        // A file from a later build with more fields keeps what it knows.
        std::fs::write(&path, r#"{"on":true,"monitor":"here","later":1}"#).unwrap();
        assert_eq!(load(&path).monitor, MonitorPref::Here);
    }

    #[test]
    fn it_floats_only_when_on_up_and_not_captured() {
        let mut s = Status {
            support: Support::Xwayland,
            settings: Settings {
                on: true,
                monitor: MonitorPref::Other,
            },
            running: true,
            capture: Capture::No,
            monitors: vec![],
            helper: false,
        };
        assert!(s.can_float());
        s.capture = Capture::Unknown;
        assert!(s.can_float());
        s.capture = Capture::Yes;
        assert!(!s.can_float());
        s.capture = Capture::No;
        s.running = false;
        assert!(!s.can_float());
        s.running = true;
        s.settings.on = false;
        assert!(!s.can_float());
    }

    #[test]
    fn the_lines_between_the_two_processes_read_back() {
        for m in [
            ToSurface::MainActive { active: true },
            ToSurface::Capture {
                capture: Capture::Yes,
            },
            ToSurface::Settings {
                settings: Settings::default(),
            },
        ] {
            let line = serde_json::to_string(&m).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<ToSurface>(&line).unwrap(), m);
        }
        let line = serde_json::to_string(&FromSurface::OpenApp).unwrap();
        assert_eq!(line, r#"{"t":"open_app"}"#);
    }
}
