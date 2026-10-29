//! The Rust side of the Ghost desktop app: the process that runs Ghost (ADR 0038).
//!
//! It supervises `ghostd` (adopting one already on the socket), bridges the window to the
//! daemon's Unix socket, keeps a tray, starts at login, and stops everything on Quit.

mod autostart;
mod bridge;
mod supervisor;
mod tray;

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

pub fn run() {
    let app = tauri::Builder::default()
        // First, so a second launch hands off and exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
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
            daemon_start
        ])
        .setup(|app| {
            tray::build(app)?;
            if std::env::args().any(|a| a == "--hidden")
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }
            autostart::apply(app.handle());
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
            // Closing the window keeps the tray alive; Quit in the tray exits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Ghost");
    app.run(|app, event| match event {
        // Quit: the daemon goes down with the shell, before the process ends.
        RunEvent::ExitRequested { .. } | RunEvent::Exit => stop_daemon(app),
        _ => {}
    });
}
