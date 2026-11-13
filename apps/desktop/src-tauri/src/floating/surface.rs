//! The floating surface: `heka-ghost-desktop --floating`, started by the shell with
//! `GDK_BACKEND=x11` (ADR 0059). One transparent, undecorated window the size of a monitor,
//! kept above the others, on every workspace, off the taskbar and the switcher, with an empty
//! input shape when idle (every click passes through) and never the keyboard.
//!
//! The page (`floating.html`, `src/floating/`) reads the daemon's events through the same
//! bridge as the main window, decides with the posture rule, and tells this process where it
//! draws (`floating_region`) and when it comes forward (`floating_summon`, which picks the
//! monitor). The shell's lines on stdin arrive as `floating://shell` events; stdin closing
//! ends this process.

use std::io::{BufRead, Write};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use gtk::prelude::*;
use serde::Serialize;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::capture::Capture;
use super::desktop::{self, Seen, Xlib};
use super::place::{self, Choice, Monitor, Rect};
use super::share::Sharing;
use super::{FromSurface, Settings, ToSurface};
use crate::bridge;

pub const LABEL: &str = "floating";

/// What the shell last said.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ShellState {
    pub settings: Settings,
    pub main_active: bool,
    pub capture: Option<Capture>,
    /// What to do about a share now (`share.rs`).
    pub sharing: Sharing,
}

struct State {
    shell: Mutex<ShellState>,
    /// Where the window is now.
    at: Mutex<Option<Rect>>,
}

fn say(msg: &FromSurface) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(msg).unwrap_or_default());
    let _ = out.flush();
}

fn window(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// The monitors as GDK sees them through X11: RandR's output names (`HDMI-1`), the same
/// names GNOME's display settings use.
fn monitors(gw: &gtk::ApplicationWindow) -> Vec<Monitor> {
    let display = gw.display();
    (0..display.n_monitors())
        .filter_map(|i| display.monitor(i))
        .enumerate()
        .map(|(i, m)| {
            let g = m.geometry();
            Monitor {
                name: m
                    .model()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Monitor {}", i + 1)),
                geometry: Rect {
                    x: g.x(),
                    y: g.y(),
                    width: g.width(),
                    height: g.height(),
                },
                primary: m.is_primary(),
            }
        })
        .collect()
}

/// Where Ghost is drawn, in CSS pixels (GDK's logical pixels at zoom 1). Everything outside
/// these rectangles passes clicks to whatever is underneath; an empty list, all of it.
#[tauri::command]
fn floating_region(app: tauri::AppHandle, rects: Vec<[f64; 4]>) {
    let Some(w) = window(&app) else { return };
    let _ = w.clone().run_on_main_thread(move || {
        let Ok(gw) = w.gtk_window() else { return };
        let Some(gdk) = gw.window() else { return };
        let region = gtk::cairo::Region::create();
        for [x, y, width, height] in rects {
            let r = gtk::cairo::RectangleInt::new(
                x.floor() as i32,
                y.floor() as i32,
                width.ceil() as i32,
                height.ceil() as i32,
            );
            let _ = region.union_rectangle(&r);
        }
        gdk.input_shape_combine_region(&region, 0, 0);
    });
}

/// What the page learns on a summon.
#[derive(Debug, Clone, Serialize)]
pub struct Summoned {
    pub choice: Option<Choice>,
    pub monitor: Option<Monitor>,
    pub workspace: Option<u32>,
    /// Where the operator's work was known from: `helper`, `x11` or `nothing`.
    pub source: &'static str,
}

/// The wake: choose the monitor from what is known now, and move there.
#[tauri::command]
async fn floating_summon(app: tauri::AppHandle) -> Result<Summoned, String> {
    let helper = match zbus::Connection::session().await {
        Ok(c) => desktop::helper_where(&c).await,
        Err(_) => None,
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(place_now(&app2, helper));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

fn place_now(
    app: &tauri::AppHandle,
    helper: Option<desktop::HelperWhere>,
) -> Result<Summoned, String> {
    let w = window(app).ok_or("no window")?;
    let gw = w.gtk_window().map_err(|e| e.to_string())?;
    let mons = monitors(&gw);
    let x = Xlib::open();
    let seen = Seen {
        xwayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        x_pointer: x.as_ref().and_then(Xlib::pointer),
        x_focus: x.as_ref().and_then(Xlib::focus),
        x_workspace: x.as_ref().and_then(Xlib::workspace),
        helper,
    };
    let source = if seen.helper.is_some() {
        "helper"
    } else if seen.x_focus.is_some() || !seen.xwayland {
        "x11"
    } else {
        "nothing"
    };
    let known = desktop::known(mons.clone(), &seen);
    let pref = {
        let state = app.state::<State>();
        state.shell.lock().unwrap().settings.monitor.clone()
    };
    let choice = place::choose(&pref, &known);
    if let Some(c) = &choice {
        let g = workarea(&gw, c.monitor, mons[c.monitor].geometry);
        let state = app.state::<State>();
        let mut at = state.at.lock().unwrap();
        if *at != Some(g) {
            move_to(&gw, g);
            *at = Some(g);
        }
    }
    // Every workspace: re-asserted, since a window manager may drop it over a remap.
    gw.stick();
    gw.set_keep_above(true);
    eprintln!("floating: summon pref {pref:?} source {source} seen {seen:?} -> {choice:?}",);
    Ok(Summoned {
        monitor: choice.map(|c| mons[c.monitor].clone()),
        choice,
        workspace: desktop::workspace(&seen),
        source,
    })
}

/// The part of a monitor windows may use (without GNOME's top bar and any dock), as GDK
/// reads it through X11 (`_NET_WORKAREA`, per monitor under GNOME); the whole monitor when
/// it can't tell.
fn workarea(gw: &gtk::ApplicationWindow, index: usize, whole: Rect) -> Rect {
    gw.display()
        .monitor(index as i32)
        .map(|m| {
            let w = m.workarea();
            Rect {
                x: w.x(),
                y: w.y(),
                width: w.width(),
                height: w.height(),
            }
        })
        .filter(|w| w.width > 0 && w.height > 0)
        .unwrap_or(whole)
}

/// Move the surface onto a monitor: its work area, never more.
///
/// Measured 2026-11-13 on GNOME Shell 50.1 under XWayland (docs/floating-room.md, Which
/// monitor): a window the monitor's full size doesn't fit a monitor with the top bar, so
/// mutter keeps it on, or sends it back to, the monitor where it fits, and the surface never
/// moved to the primary monitor; and an undecorated window that asks to cover a whole monitor
/// is made a legacy full-screen one (`_NET_WM_STATE_FULLSCREEN`), which then ignores sizes,
/// moves only as a whole monitor, and on the primary monitor makes GNOME Shell hide its top
/// bar. So: out of full screen first, then one request for the work area.
fn move_to(gw: &gtk::ApplicationWindow, g: Rect) {
    gw.unfullscreen();
    if let Some(gdk) = gw.window() {
        let display = gdk.display();
        display.flush();
        gdk.move_resize(g.x, g.y, g.width, g.height);
        display.flush();
    }
    // GTK's own idea of the window, so it doesn't put it back.
    gw.move_(g.x, g.y);
    gw.resize(g.width, g.height);
}

/// "Open the app": the shell brings the main window here.
#[tauri::command]
fn floating_open_app() {
    say(&FromSurface::OpenApp);
}

/// The share card's answer: show Ghost in this share, or keep it out (for this share only).
#[tauri::command]
fn floating_share(show: bool) {
    say(&FromSurface::Share { show });
}

/// What the shell last said, for a page that just loaded.
#[tauri::command]
fn floating_shell(state: tauri::State<'_, State>) -> ShellState {
    state.shell.lock().unwrap().clone()
}

/// The default output's volume and mute, for the captions' "when muted" choice: one
/// `wpctl get-volume` (PipeWire), read only, at most a second; null when it can't be read.
#[tauri::command]
async fn floating_sound() -> Option<super::Sound> {
    let run = tokio::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .ok()?
        .ok()?;
    super::parse_wpctl(&String::from_utf8_lossy(&out.stdout))
}

/// A line for the journal (the page's frame timings and decisions).
#[tauri::command]
fn floating_log(line: String) {
    eprintln!("floating: {line}");
}

fn listen_to_shell(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(msg) = serde_json::from_str::<ToSurface>(&line) else {
                continue;
            };
            {
                let state = app.state::<State>();
                let mut s = state.shell.lock().unwrap();
                match &msg {
                    ToSurface::MainActive { active } => s.main_active = *active,
                    ToSurface::Capture { capture, sharing } => {
                        s.capture = Some(*capture);
                        s.sharing = *sharing;
                    }
                    ToSurface::Settings { settings } => s.settings = settings.clone(),
                }
            }
            let _ = app.emit_to(LABEL, "floating://shell", &msg);
        }
        // The shell is gone, or turned the floating room off.
        app.exit(0);
    });
}

/// `heka-ghost-desktop --floating-check`: what this desktop tells the floating room right now,
/// as JSON, and the monitor each choice would pick. For the operator's checks (move the
/// pointer, switch workspace, share the screen, run it again) and for bug reports.
pub fn check() {
    // SAFETY: first thing in a single-threaded process, before GTK or anything else reads it.
    unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (capture, helper) = rt.block_on(async {
        match zbus::Connection::session().await {
            Ok(c) => (
                Some(super::capture::probe(&c).await),
                desktop::helper_where(&c).await,
            ),
            Err(_) => (None, None),
        }
    });
    let mons = if gtk::init().is_ok() {
        let display = gtk::gdk::Display::default();
        display
            .map(|d| {
                (0..d.n_monitors())
                    .filter_map(|i| d.monitor(i))
                    .enumerate()
                    .map(|(i, m)| {
                        let g = m.geometry();
                        Monitor {
                            name: m
                                .model()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| format!("Monitor {}", i + 1)),
                            geometry: Rect {
                                x: g.x(),
                                y: g.y(),
                                width: g.width(),
                                height: g.height(),
                            },
                            primary: m.is_primary(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let x = Xlib::open();
    let seen = Seen {
        xwayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        x_pointer: x.as_ref().and_then(Xlib::pointer),
        x_focus: x.as_ref().and_then(Xlib::focus),
        x_workspace: x.as_ref().and_then(Xlib::workspace),
        helper: helper.clone(),
    };
    let known = desktop::known(mons.clone(), &seen);
    let mut choices = serde_json::Map::new();
    let mut prefs = vec![place::MonitorPref::Other, place::MonitorPref::Here];
    prefs.extend(
        mons.iter()
            .map(|m| place::MonitorPref::Named(m.name.clone())),
    );
    for p in prefs {
        let c = place::choose(&p, &known);
        choices.insert(
            serde_json::to_string(&p).unwrap_or_default(),
            serde_json::json!(c.map(|c| serde_json::json!({
                "monitor": mons[c.monitor].name, "side": c.side, "why": c.why, "covered": c.covered,
            }))),
        );
    }
    let out = serde_json::json!({
        "support": super::support(),
        "capture": capture,
        "monitors": mons,
        "x11": {
            "xwayland": seen.xwayland,
            "pointer": seen.x_pointer,
            "focus": seen.x_focus.map(|(r, fs)| serde_json::json!({"frame": r, "fullscreen": fs})),
            "workspace": seen.x_workspace,
        },
        "helper": helper.map(|h| serde_json::json!({
            "pointer": h.pointer, "focus": h.focus, "focus_fullscreen": h.focus_fullscreen, "workspace": h.workspace,
        })),
        "where_i_am": place::here(&known).map(|i| mons[i].name.clone()),
        "choices": choices,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

pub fn run(context: tauri::Context<tauri::Wry>) {
    let app = tauri::Builder::default()
        .manage(State {
            shell: Mutex::new(ShellState::default()),
            at: Mutex::new(None),
        })
        .manage(bridge::EventsRunning(std::sync::Arc::new(AtomicBool::new(
            false,
        ))))
        .invoke_handler(tauri::generate_handler![
            bridge::api_request,
            bridge::events_start,
            floating_region,
            floating_summon,
            floating_open_app,
            floating_share,
            floating_shell,
            floating_log,
            floating_sound
        ])
        .setup(|app| {
            // The window's own capability: the daemon bridge, its events and the commands
            // above, nothing else of Tauri's.
            app.add_capability(
                tauri::ipc::CapabilityBuilder::new("floating")
                    .window(LABEL)
                    .permission("core:default")
                    .permission("core:event:default"),
            )?;
            // Development only: the page with a query (`?mock=floating`), served from the
            // app's own dev address so it is the app's page; never in a release.
            let url = match std::env::var("GHOST_FLOATING_URL") {
                Ok(u) if cfg!(debug_assertions) => WebviewUrl::External(u.parse()?),
                _ => WebviewUrl::App("floating.html".into()),
            };
            let w = WebviewWindowBuilder::new(app, LABEL, url)
                .title("Ghost")
                .transparent(true)
                .decorations(false)
                .shadow(false)
                .resizable(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible_on_all_workspaces(true)
                .focused(false)
                .focusable(false)
                .inner_size(800.0, 600.0)
                .visible(false)
                .on_page_load(|_, p| {
                    if matches!(p.event(), tauri::webview::PageLoadEvent::Finished) {
                        eprintln!("floating: page {}", p.url());
                    }
                })
                .build()?;
            let gw = w.gtk_window()?;
            gw.set_type_hint(gtk::gdk::WindowTypeHint::Normal);
            gw.set_skip_pager_hint(true);
            gw.set_skip_taskbar_hint(true);
            gw.set_focus_on_map(false);
            gw.set_accept_focus(false);
            // Fully click-through from the first frame: nothing is drawn but the line.
            gw.connect_realize(|gw| {
                if let Some(gdk) = gw.window() {
                    gdk.input_shape_combine_region(&gtk::cairo::Region::create(), 0, 0);
                }
            });
            let mons = monitors(&gw);
            // Idle on the monitor that isn't the primary one (the operator's choice moves
            // it at each wake).
            let pref = super::load(
                &app.path()
                    .app_config_dir()
                    .unwrap_or_default()
                    .join("floating.json"),
            );
            app.state::<State>().shell.lock().unwrap().settings = pref.clone();
            let idle = place::choose(
                &pref.monitor,
                &place::Known {
                    monitors: mons.clone(),
                    ..Default::default()
                },
            );
            if let Some(c) = idle {
                let g = workarea(&gw, c.monitor, mons[c.monitor].geometry);
                gw.move_(g.x, g.y);
                gw.resize(g.width, g.height);
                *app.state::<State>().at.lock().unwrap() = Some(g);
            }
            gw.show_all();
            // Click-through from the first frame, whether or not the page ever loads: the
            // page widens it only to its buttons while Ghost is forward.
            if let Some(gdk) = gw.window() {
                gdk.input_shape_combine_region(&gtk::cairo::Region::create(), 0, 0);
            }
            gw.stick();
            gw.set_keep_above(true);
            eprintln!(
                "floating: up on {} monitor(s), backend {}",
                mons.len(),
                std::env::var("GDK_BACKEND").unwrap_or_default()
            );
            say(&FromSurface::Ready { monitors: mons });
            listen_to_shell(app.handle().clone());
            Ok(())
        })
        .build(context)
        .expect("error while building Ghost's floating room");
    // Its own web-content process ended on the way out, as the shell's (#115).
    app.run(|_, event| {
        if let tauri::RunEvent::Exit = event {
            crate::webkit_exit::end_web_processes();
        }
    });
}
