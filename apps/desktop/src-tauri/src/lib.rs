//! The Rust side of the Ghost desktop app: the process that runs Ghost (ADR 0038).
//!
//! It supervises `ghostd` (adopting one already on the socket), bridges the window to the
//! daemon's Unix socket, keeps a tray, starts at login, and stops everything on Quit. With
//! `--floating` the same binary is the floating room's surface instead (ADR 0059), which the
//! shell starts when the operator turns it on.

mod autostart;
mod bridge;
pub mod floating;
mod supervisor;
mod tray;
mod updater;

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use tauri::{Emitter, Manager, RunEvent};

use supervisor::system::System;
use supervisor::{DaemonEvent, Handle, Supervisor};

/// The supervisor's handle and its last word, for a window that opens after the fact.
struct Daemon {
    handle: Mutex<Option<Handle>>,
    last: Mutex<Option<DaemonEvent>>,
}

/// The supervisor's last event, so a freshly loaded window knows where things stand.
#[tauri::command]
fn daemon_state(daemon: tauri::State<'_, Daemon>) -> Option<DaemonEvent> {
    daemon.last.lock().unwrap().clone()
}

/// The window's Start button: skip the rest of a restart delay.
#[tauri::command]
fn daemon_start(daemon: tauri::State<'_, Daemon>) -> Result<(), String> {
    match daemon.handle.lock().unwrap().as_ref() {
        Some(h) => h.start_now(),
        None => Err("Ghost is quitting.".into()),
    }
}

/// `GHOST_SHELL_ADOPT_ONLY=1`: never spawn a daemon, only adopt the one on the socket.
/// `scripts/dev.sh` sets it, since it runs the daemon itself.
fn adopt_only() -> bool {
    std::env::var("GHOST_SHELL_ADOPT_ONLY")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn start_supervisor(app: &tauri::AppHandle) -> Handle {
    let system = System::from_env();
    eprintln!(
        "ghost: daemon {}; socket {}; output {}",
        system.ghostd.display(),
        system.socket.display(),
        match &system.output {
            supervisor::system::Output::Journal => "journal (systemd-cat -t ghostd)".to_string(),
            supervisor::system::Output::File(p) => p.display().to_string(),
        }
    );
    let emitter = app.clone();
    let sup = Supervisor::new(system, adopt_only(), move |e| {
        eprintln!("ghost: daemon {e:?}");
        if let Some(d) = emitter.try_state::<Daemon>() {
            *d.last.lock().unwrap() = Some(e.clone());
        }
        let _ = emitter.emit("ghost://daemon", e);
    });
    supervisor::spawn_thread(sup)
}

/// Stop the daemon the shell runs; idempotent, so both exit events may call it.
fn stop_daemon(app: &tauri::AppHandle) {
    if let Some(d) = app.try_state::<Daemon>()
        && let Some(h) = d.handle.lock().unwrap().take()
    {
        h.stop();
    }
}

/// The app's one context (its config and the window's assets, embedded once).
fn context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

pub fn run() {
    if std::env::args().any(|a| a == floating::CHECK_ARG) {
        #[cfg(target_os = "linux")]
        floating::surface::check();
        return;
    }
    if std::env::args().any(|a| a == floating::ARG) {
        #[cfg(target_os = "linux")]
        {
            let mut ctx = context();
            // The surface has its own window and no main one.
            ctx.config_mut().app.windows.clear();
            floating::surface::run(ctx);
        }
        return;
    }
    let app = tauri::Builder::default()
        // First, so a second launch hands off and exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        // ADR 0066: the window's "Update and restart" (updater.rs); the endpoint and the
        // public key are tauri.conf.json's.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // At login the app starts in the tray; Open Ghost shows the window.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--hidden")
                .build(),
        )
        .manage(bridge::EventsRunning(std::sync::Arc::new(AtomicBool::new(
            false,
        ))))
        .manage(Daemon {
            handle: Mutex::new(None),
            last: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            bridge::api_request,
            bridge::events_start,
            daemon_state,
            daemon_start,
            floating::floating_status,
            floating::floating_set,
            floating::stage::stage_open,
            updater::update_view,
            updater::update_check,
            updater::update_set_check,
            updater::update_install,
            updater::update_cancel
        ])
        .setup(|app| {
            let floating = floating::setup(app.handle());
            floating::stage::capability(app.handle())?;
            tray::build(app, &floating.status())?;
            if let Some(w) = app.get_webview_window("main") {
                floating.main_active(w.is_focused().unwrap_or(false));
            }
            if std::env::args().any(|a| a == "--hidden")
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }
            autostart::apply(app.handle());
            app.manage(updater::Updater::new(app.handle()));
            updater::start(app.handle());
            let handle = start_supervisor(app.handle());
            *app.state::<Daemon>().handle.lock().unwrap() = Some(handle);
            // Logout or a terminal's Ctrl+C: the same clean stop as Quit.
            let signalled = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut term), Ok(mut int)) = (
                    signal(SignalKind::terminate()),
                    signal(SignalKind::interrupt()),
                ) else {
                    return;
                };
                tokio::select! {
                    _ = term.recv() => eprintln!("ghost: SIGTERM"),
                    _ = int.recv() => eprintln!("ghost: SIGINT"),
                }
                signalled.exit(0);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            let host = window
                .app_handle()
                .try_state::<std::sync::Arc<floating::Host>>();
            match event {
                // Closing the window keeps the tray alive; Quit in the tray exits. The stage
                // closes for good.
                tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                    let _ = window.hide();
                    api.prevent_close();
                    if let Some(h) = host {
                        h.main_active(false);
                    }
                }
                // The floating room leaves a session to the window the operator is using.
                tauri::WindowEvent::Focused(focused) if window.label() == "main" => {
                    if let Some(h) = host {
                        h.main_active(*focused);
                    }
                }
                _ => {}
            }
        })
        .build(context())
        .expect("error while building Ghost");
    app.run(|app, event| match event {
        // Quit: the daemon goes down with the shell, before the process ends.
        RunEvent::ExitRequested { .. } | RunEvent::Exit => stop_daemon(app),
        _ => {}
    });
}
