//! The window's way to the daemon: the WebView cannot open a Unix socket, so `api_request`
//! forwards one HTTP call over it, and `events_start` keeps a WebSocket to `/events` open
//! and re-emits every message as a Tauri event. The tray's tooltip follows the phase.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

pub fn socket_path() -> std::path::PathBuf {
    ghost_api::server::default_socket_path()
}

#[derive(Serialize)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

/// One HTTP request to the daemon over the socket. Errors are strings for the frontend.
#[tauri::command]
pub async fn api_request(
    method: String,
    path: String,
    body: Option<String>,
) -> Result<ApiResponse, String> {
    let client: Client<UnixConnector, Full<Bytes>> = Client::unix();
    let uri: hyper::Uri = Uri::new(socket_path(), &path).into();
    let req = hyper::Request::builder()
        .method(method.as_str())
        .uri(uri)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|e| e.to_string())?;
    let res = client.request(req).await.map_err(|e| {
        eprintln!("ghost: {method} {path}: {e}");
        format!("ghostd not reachable: {e}")
    })?;
    let status = res.status().as_u16();
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok(ApiResponse {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

pub struct EventsRunning(pub Arc<AtomicBool>);

/// Keep the /events WebSocket open; emit `ghost://event` per message, `ghost://closed` on
/// disconnect, and retry until the app exits. Idempotent: a second call is a no-op.
#[tauri::command]
pub async fn events_start(app: AppHandle, running: State<'_, EventsRunning>) -> Result<(), String> {
    if running.0.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let flag = running.0.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match tokio::net::UnixStream::connect(socket_path()).await {
                Ok(stream) => {
                    eprintln!("ghost: connected to {}", socket_path().display());
                    let req = hyper::Request::builder()
                        .uri("ws://ghost/events")
                        .header("Host", "ghost")
                        .header("Connection", "Upgrade")
                        .header("Upgrade", "websocket")
                        .header("Sec-WebSocket-Version", "13")
                        .header(
                            "Sec-WebSocket-Key",
                            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                        )
                        .body(())
                        .unwrap();
                    match tokio_tungstenite::client_async(req, stream).await {
                        Ok((mut ws, _)) => {
                            while let Some(Ok(msg)) = ws.next().await {
                                if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
                                    let _ = app.emit("ghost://event", t.to_string());
                                    update_tray(&app, &t);
                                }
                            }
                            eprintln!("ghost: event stream closed");
                        }
                        Err(e) => eprintln!("ghost: websocket handshake failed: {e}"),
                    }
                }
                Err(e) => eprintln!("ghost: cannot reach {}: {e}", socket_path().display()),
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
