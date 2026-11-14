//! The relay (ADR 0069): the one app the Claude Code process Ghost runs is given, a
//! Model Context Protocol endpoint on loopback that runs nothing.
//!
//! One relay per process, on its own port: not the control API, the phone's listener, the
//! webhook listener or the listener for outside tools (ADR 0048), and none of their
//! routes. It offers exactly the session's tools as the stage last sent them (re-listed
//! when they change, `notifications/tools/list_changed`, ADR 0046), and a call to one of
//! them is parked: handed to the turn, which gives it to the stage as the same tool call an
//! HTTP model's reply carries, so the dispatcher and the gate run it exactly as they run
//! every call. What the stage would send back in its next model call is this request's
//! answer. A call to a tool not on the list is refused, and nothing here runs anything.
//!
//! Every request carries the process's bearer: 32 random bytes, kept in memory only, never
//! logged, compared in constant time, dead with the process. A request with an `Origin`
//! (a browser's) is refused, and so is one whose `Host` is not this loopback port.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures::Stream;
use rand::RngCore;
use serde_json::{Map, Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::Incoming;
use crate::tools::Tools;

/// The one path the relay answers on.
pub const PATH: &str = "/mcp";
/// The protocol versions the relay speaks; the client's is used when it is one of them.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// A call the program made, waiting for Ghost's dispatcher.
pub struct Parked {
    pub name: String,
    pub arguments: Map<String, Value>,
    pub answer: oneshot::Sender<Answer>,
}

impl std::fmt::Debug for Parked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parked")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// What a parked call is answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The result as the stage puts it in the context (JSON text).
    Result(String),
    /// Not run: why, in a sentence of Ghost's.
    Refused(String),
}

struct Shared {
    token: String,
    port: OnceLock<u16>,
    tools: Mutex<Vec<Value>>,
    calls: mpsc::UnboundedSender<Incoming>,
    changed: broadcast::Sender<()>,
}

/// A relay listening; dropped, it stops listening and every call waiting is refused.
pub struct Relay {
    shared: Arc<Shared>,
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn mcp_tools(tools: &Tools) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": {"type": "object", "properties": t.properties, "required": t.required},
            })
        })
        .collect()
}

/// 32 random bytes, as hex.
fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Relay {
    /// Listen on a loopback port of the system's choosing, offering `tools`; calls go to
    /// `calls`.
    pub async fn bind(
        tools: &Tools,
        calls: mpsc::UnboundedSender<Incoming>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let (changed, _) = broadcast::channel(8);
        let shared = Arc::new(Shared {
            token: new_token(),
            port: OnceLock::new(),
            tools: Mutex::new(mcp_tools(tools)),
            calls,
            changed,
        });
        let _ = shared.port.set(addr.port());
        let app = Router::new()
            .route(PATH, post(on_post).get(on_get).delete(on_delete))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { shared, addr, task })
    }

    pub fn url(&self) -> String {
        format!("http://{}{PATH}", self.addr)
    }

    /// The bearer, for the program's app config (a 0600 file); never logged.
    pub fn bearer(&self) -> &str {
        &self.shared.token
    }

    /// The session's tools now; true, and the program told, when they changed.
    pub fn set_tools(&self, tools: &Tools) -> bool {
        let now = mcp_tools(tools);
        let mut held = self.shared.tools.lock().unwrap_or_else(|e| e.into_inner());
        if *held == now {
            return false;
        }
        *held = now;
        let _ = self.shared.changed.send(());
        true
    }
}

fn refuse(status: StatusCode, why: &'static str) -> Response {
    (status, why).into_response()
}

/// The checks every request passes before its body is read.
fn admit(s: &Shared, headers: &HeaderMap) -> Result<(), (StatusCode, &'static str)> {
    if headers.contains_key(header::ORIGIN) {
        return Err((StatusCode::FORBIDDEN, "no browser requests"));
    }
    let port = s.port.get().copied().unwrap_or(0);
    let host_ok = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if !host_ok {
        return Err((StatusCode::FORBIDDEN, "not this host"));
    }
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if given.len() != s.token.len() || !bool::from(given.as_bytes().ct_eq(s.token.as_bytes())) {
        return Err((StatusCode::UNAUTHORIZED, "no"));
    }
    Ok(())
}

fn reply(id: &Value, result: Result<Value, (i64, String)>) -> Response {
    let body = match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn on_post(State(s): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err((status, why)) = admit(&s, &headers) {
        return refuse(status, why);
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "not JSON");
    };
    if !msg.is_object() {
        return refuse(StatusCode::BAD_REQUEST, "one message at a time");
    }
    let Some(id) = msg.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response(); // a notification
    };
    let params = &msg["params"];
    match msg["method"].as_str().unwrap_or("") {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS
                .into_iter()
                .find(|v| *v == asked)
                .unwrap_or(VERSIONS[0]);
            reply(
                &id,
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": {"name": "ghost", "version": env!("CARGO_PKG_VERSION")},
                })),
            )
        }
        "ping" => reply(&id, Ok(json!({}))),
        "tools/list" => {
            let tools = s.tools.lock().unwrap_or_else(|e| e.into_inner()).clone();
            reply(&id, Ok(json!({"tools": tools})))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_string();
            let offered = s
                .tools
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|t| t["name"] == name.as_str());
            if !offered {
                return reply(
                    &id,
                    Err((-32602, "that tool is not offered in this session".into())),
                );
            }
            let arguments = match &params["arguments"] {
                Value::Object(m) => m.clone(),
                Value::Null => Map::new(),
                _ => return reply(&id, Err((-32602, "arguments must be an object".into()))),
            };
            let (tx, rx) = oneshot::channel();
            let parked = Parked {
                name,
                arguments,
                answer: tx,
            };
            if s.calls.send(Incoming::Call(parked)).is_err() {
                return reply(&id, Err((-32603, "the session has ended".into())));
            }
            let (text, is_error) = match rx.await {
                Ok(Answer::Result(t)) => (t, false),
                Ok(Answer::Refused(why)) => (why, true),
                Err(_) => ("Not run: the turn ended.".to_string(), true),
            };
            reply(
                &id,
                Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error})),
            )
        }
        _ => reply(&id, Err((-32601, "not offered".into()))),
    }
}

/// The server's messages to the program: the tool list changed.
async fn on_get(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let Err((status, why)) = admit(&s, &headers) {
        return refuse(status, why);
    }
    let rx = s.changed.subscribe();
    Sse::new(list_changed(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `notifications/tools/list_changed`, once per change, until the relay goes.
fn list_changed(
    rx: broadcast::Receiver<()>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> + Send {
    futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(()) => {
                    let ev = SseEvent::default().data(
                        json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
                            .to_string(),
                    );
                    return Some((Ok(ev), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

async fn on_delete(State(s): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let Err((status, why)) = admit(&s, &headers) {
        return refuse(status, why);
    }
    StatusCode::OK.into_response()
}
