//! The Rust side of the Ghost desktop app: owns the connection to ghostd's Unix socket.
//!
//! The WebView cannot open a Unix socket, so two commands bridge it: `api_request`
//! forwards one HTTP call, and `events_start` keeps a WebSocket to `/events` open and
//! re-emits every message as a Tauri event. The tray shows the daemon's phase.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("GHOST_API_SOCKET") {
        return PathBuf::from(p);
    }
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(rt).join("ghost").join("api.sock")
}

#[derive(Serialize)]
struct ApiResponse {
    status: u16,
    body: String,
}

/// One HTTP request to the daemon over the socket. Errors are strings for the frontend.
#[tauri::command]
async fn api_request(method: String, path: String, body: Option<String>) -> Result<ApiResponse, String> {
    let client: Client<UnixConnector, Full<Bytes>> = Client::unix();
    let uri: hyper::Uri = Uri::new(socket_path(), &path).into();
    let mut req = hyper::Request::builder()
        .method(method.as_str())
        .uri(uri)
        .header("content-type", "application/json");
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|e| e.to_string())?;
    let res = client.request(req).await.map_err(|e| format!("ghostd not reachable: {e}"))?;
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
    Ok(ApiResponse { status, body: String::from_utf8_lossy(&bytes).into_owned() })
}

/// First run: enable and start the per-user service. Returns systemctl's combined output.
#[tauri::command]
async fn start_daemon() -> Result<String, String> {
    let out = tokio::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "ghostd"])
        .output()
        .await
        .map_err(|e| format!("systemctl: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

struct EventsRunning(Arc<AtomicBool>);

/// Keep the /events WebSocket open; emit `ghost://event` per message, `ghost://closed` on
/// disconnect, and retry until the app exits. Idempotent: a second call is a no-op.
#[tauri::command]
async fn events_start(app: AppHandle, running: State<'_, EventsRunning>) -> Result<(), String> {
    if running.0.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let flag = running.0.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match tokio::net::UnixStream::connect(socket_path()).await {
                Ok(stream) => {
                    let req = hyper::Request::builder()
                        .uri("ws://ghost/events")
                        .header("Host", "ghost")
                        .header("Connection", "Upgrade")
                        .header("Upgrade", "websocket")
                        .header("Sec-WebSocket-Version", "13")
                        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
                        .body(())
                        .unwrap();
                    if let Ok((mut ws, _)) = tokio_tungstenite::client_async(req, stream).await {
                        while let Some(Ok(msg)) = ws.next().await {
                            if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
                                let _ = app.emit("ghost://event", t.to_string());
                                update_tray(&app, &t);
                            }
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = app.emit("ghost://closed", ());
            if !flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    });
    Ok(())
}

/// Tray tooltip follows the daemon's phase; cheap to parse here rather than round-trip.
fn update_tray(app: &AppHandle, payload: &str) {
    let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return,
    };
    let phase = v
        .get("phase")
        .or_else(|| v.get("status").and_then(|s| s.get("phase")))
        .and_then(|p| p.as_str());
    if let (Some(phase), Some(tray)) = (phase, app.tray_by_id("main")) {
        let label = match phase {
            "passive" => "Ghost: waiting for \"Hey Ghost\"",
            "listening" => "Ghost: listening",
            "thinking" => "Ghost: thinking",
            "speaking" => "Ghost: speaking",
            "starting" => "Ghost: starting",
            other => other,
        };
        let _ = tray.set_tooltip(Some(label));
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(EventsRunning(Arc::new(AtomicBool::new(false))))
        .invoke_handler(tauri::generate_handler![api_request, events_start, start_daemon])
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Open Ghost", true, None::<&str>)?;
            let wake = MenuItem::with_id(app, "wake", "Wake", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &wake, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Ghost")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "wake" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = api_request("POST".into(), "/wake".into(), None).await;
                            let _ = app.emit("ghost://woke", ());
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the tray alive; Quit in the tray exits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Ghost");
}
