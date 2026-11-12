//! SPIKE (not the product): Ghost as a floating, chrome-less surface over the desktop.
//!
//! `cargo run -p heka-ghost-desktop --example floating --features floating`, with the page
//! served by Vite on :1431 (`apps/desktop/src/design/floating/run.sh` does both). The normal
//! app is untouched: this is a second binary behind the `floating` feature, with its own
//! config (`floating/tauri.conf.json`; tauri-codegen reads the file named
//! tauri.conf.json in the folder given, whatever the path names) and no daemon, tray or socket bridge. Mock data only.
//!
//! What it tests (docs/design/floating-2026-11.md): a transparent, undecorated, keep-above
//! window with no taskbar entry, spanning one monitor, click-through except where Ghost is
//! drawn (a GDK input shape the page keeps current), shown without taking the keyboard.
//!
//! Environment switches (all optional):
//! - `FLOATING_PLACE` = `fullscreen` (default: fullscreen on the monitor, the only placement a
//!   Wayland client controls on GNOME), `maximize`, or `size` (monitor-sized, positioned at
//!   the monitor's origin, which only X11 honours).
//! - `FLOATING_MONITOR` = index of the monitor (default 0).
//! - `FLOATING_IDLE` = `mapped` (default: the window stays mapped, transparent and
//!   click-through when idle) or `hidden` (unmapped when idle, mapped on summon).
//! - `FLOATING_FOCUSABLE` = `1` lets the window take the keyboard when the operator clicks
//!   it (the prototype's keys); default `0`, the window never takes focus.
//! - `FLOATING_TYPE` = the window type hint: `normal` (default; mutter refuses fullscreen to
//!   a `utility` window), `utility`, `dock`,
//!   `notification`, `splash` (X11 window managers stack and list these differently).
//! - `FLOATING_URL` = a page to load instead of the prototype (the motion benches).
//!
//! Control: `floating send <message>` (or any datagram to `$XDG_RUNTIME_DIR/ghost-floating.sock`):
//! `summon`, `demo`, `dismiss`, `state <name>`, `quit`. In the product the summon would be
//! the daemon's wake event.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use gtk::prelude::*;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "floating";

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(dir).join("ghost-floating.sock")
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

struct Started(Instant);
struct Idle(Mutex<bool>);

fn window(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// The page tells the shell where Ghost is drawn, in CSS pixels (= GDK logical pixels at zoom
/// 1). Everything outside these rectangles passes clicks to whatever is underneath. An empty
/// list makes the whole surface click-through.
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

/// The page's measurements and observations go to stderr, for the report.
#[tauri::command]
fn floating_report(app: tauri::AppHandle, line: String) {
    let t = app.state::<Started>().0.elapsed().as_secs_f64();
    eprintln!("floating[{t:7.2}s] {line}");
}

/// The page asks for the window to be unmapped (idle = hidden) or mapped.
#[tauri::command]
fn floating_visible(app: tauri::AppHandle, visible: bool) {
    if env("FLOATING_IDLE", "mapped") != "hidden" {
        return;
    }
    *app.state::<Idle>().0.lock().unwrap() = !visible;
    if let Some(w) = window(&app) {
        let _ = if visible { show(&w) } else { w.hide() };
    }
}

/// Map the window without taking the keyboard: GTK's focus-on-map off, accept-focus as
/// configured, keep-above re-asserted (a no-op on native Wayland; see the report).
fn show(w: &WebviewWindow) -> tauri::Result<()> {
    let w2 = w.clone();
    w.run_on_main_thread(move || {
        let Ok(gw) = w2.gtk_window() else { return };
        gw.set_focus_on_map(false);
        gw.set_accept_focus(env("FLOATING_FOCUSABLE", "0") == "1");
        gw.show_all();
        gw.set_keep_above(true);
        place(&gw);
        // What the compositor actually gave us, for the report.
        let probe = gw.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            let (w, h) = probe.size();
            let (x, y) = probe.position();
            let display = probe.display();
            let monitor = probe
                .window()
                .and_then(|g| display.monitor_at_window(&g))
                .map(|m| {
                    let g = m.geometry();
                    format!("{}x{}+{}+{}", g.width(), g.height(), g.x(), g.y())
                })
                .unwrap_or_default();
            let state = probe.window().map(|g| format!("{:?}", g.state())).unwrap_or_default();
            eprintln!(
                "floating: mapped size {w}x{h} at {x},{y} monitor {monitor:?} scale {} active {} state {state}",
                probe.scale_factor(),
                probe.is_active(),
            );
        });
    })
}

fn place(gw: &gtk::ApplicationWindow) {
    let monitor: i32 = env("FLOATING_MONITOR", "0").parse().unwrap_or(0);
    match env("FLOATING_PLACE", "fullscreen").as_str() {
        "fullscreen" => {
            if let Some(screen) = WidgetExt::screen(gw) {
                gw.fullscreen_on_monitor(&screen, monitor);
            }
        }
        "maximize" => gw.maximize(),
        _ => {
            let display = gw.display();
            if let Some(m) = display.monitor(monitor) {
                let g = m.geometry();
                gw.move_(g.x(), g.y());
                gw.resize(g.width(), g.height());
            }
        }
    }
}

fn deliver(app: &tauri::AppHandle, msg: &str) {
    let msg = msg.trim();
    if msg == "quit" {
        app.exit(0);
        return;
    }
    let Some(w) = window(app) else { return };
    let summoning = matches!(
        msg.split_whitespace().next(),
        Some("summon" | "demo" | "state")
    );
    if summoning && *app.state::<Idle>().0.lock().unwrap() {
        *app.state::<Idle>().0.lock().unwrap() = false;
        let _ = show(&w);
    }
    let json = serde_json::to_string(msg).unwrap_or_default();
    let _ = w.eval(format!(
        "window.__floating && window.__floating.command({json})"
    ));
}

fn listen(app: tauri::AppHandle) {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let sock = match UnixDatagram::bind(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("floating: cannot bind {}: {e}", path.display());
            return;
        }
    };
    eprintln!("floating: listening on {}", path.display());
    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.recv(&mut buf) {
            let msg = String::from_utf8_lossy(&buf[..n]).to_string();
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || deliver(&app2, &msg));
        }
    });
}

fn send(args: &[String]) -> ! {
    let msg = args.join(" ");
    let sock = UnixDatagram::unbound().expect("socket");
    match sock.send_to(msg.as_bytes(), socket_path()) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("floating: is it running? {e}");
            std::process::exit(1)
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("send") {
        send(&args[1..]);
    }
    let started = Started(Instant::now());
    let idle_hidden = env("FLOATING_IDLE", "mapped") == "hidden";
    let app = tauri::Builder::default()
        .manage(started)
        .manage(Idle(Mutex::new(idle_hidden)))
        .invoke_handler(tauri::generate_handler![
            floating_region,
            floating_report,
            floating_visible
        ])
        .setup(move |app| {
            // The window's own capability: the three commands above and nothing else of Tauri's.
            app.add_capability(
                tauri::ipc::CapabilityBuilder::new("floating")
                    .window(LABEL)
                    .permission("core:default"),
            )?;
            let url = match std::env::var("FLOATING_URL") {
                Ok(u) => WebviewUrl::External(u.parse().expect("FLOATING_URL")),
                Err(_) => WebviewUrl::App("src/design/floating/index.html".into()),
            };
            let w = WebviewWindowBuilder::new(app, LABEL, url)
                .title("Ghost")
                .transparent(true)
                .decorations(false)
                .shadow(false)
                // Mutter refuses fullscreen to a window that can't be resized.
                .resizable(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible_on_all_workspaces(true)
                .focused(false)
                .focusable(env("FLOATING_FOCUSABLE", "0") == "1")
                .inner_size(1280.0, 800.0)
                .visible(false)
                .on_page_load(|w, p| {
                    eprintln!("floating: page {:?} {}", p.event(), p.url());
                    let _ = w;
                })
                .build()?;
            let gw = w.gtk_window()?;
            // The type hint: X11 window managers stack and list each type differently.
            gw.set_type_hint(match env("FLOATING_TYPE", "normal").as_str() {
                "dock" => gtk::gdk::WindowTypeHint::Dock,
                "notification" => gtk::gdk::WindowTypeHint::Notification,
                "splash" => gtk::gdk::WindowTypeHint::Splashscreen,
                "utility" => gtk::gdk::WindowTypeHint::Utility,
                _ => gtk::gdk::WindowTypeHint::Normal,
            });
            gw.set_skip_pager_hint(true);
            gw.set_skip_taskbar_hint(true);
            // Start fully click-through: nothing is drawn until a summon.
            gw.connect_realize(|gw| {
                if let Some(gdk) = gw.window() {
                    gdk.input_shape_combine_region(&gtk::cairo::Region::create(), 0, 0);
                }
            });
            if !idle_hidden {
                show(&w)?;
            }
            eprintln!(
                "floating: backend {}, place {}, idle {}, focusable {}",
                std::env::var("GDK_BACKEND").unwrap_or_else(|_| "default (wayland)".into()),
                env("FLOATING_PLACE", "fullscreen"),
                env("FLOATING_IDLE", "mapped"),
                env("FLOATING_FOCUSABLE", "0"),
            );
            listen(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!("floating/tauri.conf.json"))
        .expect("error while building the floating spike");
    app.run(|_, event| {
        if let tauri::RunEvent::Exit = event {
            let _ = std::fs::remove_file(socket_path());
        }
    });
}
