//! The webhook listener (ADR 0027): a third app in `ghostd`, for webhooks only.
//!
//! It serves `POST /hooks/<name>` for the hooks `hooks.json` declares (ADR 0035) and
//! nothing else: no phone route, nothing of the control API, no static files. It binds
//! its own addresses (`GHOST_HOOKS_LISTEN`, comma separated; off when empty), apart from the
//! phone listener and the control API's socket. How the internet reaches those addresses is
//! the operator's network.
//!
//! One fallback handler, so it sees the exact bytes: the path and the method are checked
//! and the hook asked for before a byte of the body is read, the body is capped while it
//! is read, and the path, the headers and those bytes go to `ghost_core::hooks::HookIntake`,
//! which verifies the signature before anything else. The reply says only accepted or
//! refused; it never echoes the body. Nothing here is logged but the outcome.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use ghost_core::hooks::{HookIntake, HookResult};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::phone::listener::parse_listen;

/// A delivery may be this long; it is refused while it is read, before any check.
pub const MAX_BODY: usize = 256 * 1024;
pub const PREFIX: &str = "/hooks/";
/// How long `stop` waits for a server's connections before closing them.
pub const STOP_WAIT: Duration = Duration::from_secs(4);

/// What the listener needs of the intake: whether a path is a hook that is on (asked
/// before the body is read), and the delivery itself.
#[async_trait]
pub trait Intake: Send + Sync {
    fn knows(&self, path: &str) -> bool;
    async fn receive(
        &self,
        path: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> anyhow::Result<HookResult>;
}

#[async_trait]
impl Intake for HookIntake {
    fn knows(&self, path: &str) -> bool {
        HookIntake::knows(self, path)
    }

    async fn receive(
        &self,
        path: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> anyhow::Result<HookResult> {
        HookIntake::receive(self, path, headers, body, None).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HooksListenError {
    #[error("GHOST_HOOKS_LISTEN is empty")]
    Empty,
    #[error("GHOST_HOOKS_LISTEN must be host:port, got '{0}'")]
    NotHostPort(String),
}

fn reply(status: StatusCode, body: &Value) -> Response {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let len = payload.len();
    let mut resp = Response::new(Body::from(payload));
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// A header's bytes as text, one char per byte (latin-1), so a value that isn't UTF-8 is
/// refused by the check instead of lost here.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The app. Anything but `POST /hooks/<name>` is a 404 or 405 with no body read.
pub fn hooks_app(intake: Arc<dyn Intake>) -> Router {
    Router::new().fallback(take).with_state(intake)
}

async fn take(State(intake): State<Arc<dyn Intake>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let name = path
        .strip_prefix(PREFIX)
        .filter(|n| !n.is_empty() && !n.contains('/'));
    if name.is_none() {
        return reply(StatusCode::NOT_FOUND, &json!({ "error": "not_found" }));
    }
    if req.method() != Method::POST {
        return reply(
            StatusCode::METHOD_NOT_ALLOWED,
            &json!({ "error": "method_not_allowed" }),
        );
    }
    if !intake.knows(&path) {
        // ADR 0027: an unknown source is a 404, no body read
        return reply(StatusCode::NOT_FOUND, &json!({ "error": "not_found" }));
    }
    let headers: HashMap<String, String> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_ascii_lowercase(), latin1(v.as_bytes())))
        .collect();
    let Ok(body) = axum::body::to_bytes(req.into_body(), MAX_BODY).await else {
        return reply(
            StatusCode::PAYLOAD_TOO_LARGE,
            &json!({ "error": "too_large" }),
        );
    };
    match intake.receive(&path, &headers, &body).await {
        Ok(result) => {
            let ok = result.status < 400;
            let status =
                StatusCode::from_u16(result.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = if ok {
                json!({ "ok": true })
            } else {
                json!({ "error": "refused" })
            };
            reply(status, &body)
        }
        Err(e) => {
            // The sender learns nothing of our failure.
            error!("hooks: intake failed: {e}");
            reply(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": "internal" }),
            )
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Server {
    address: String,
    port: u16,
    task: JoinHandle<io::Result<()>>,
}

/// The app on every address in `GHOST_HOOKS_LISTEN`; each must bind.
pub struct HooksListener {
    pub addresses: Vec<String>,
    app: Router,
    servers: Mutex<Vec<Server>>,
    stop: watch::Sender<bool>,
}

impl HooksListener {
    pub fn new(listen: &str, intake: Arc<dyn Intake>) -> Result<Self, HooksListenError> {
        let addresses: Vec<String> = listen
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(String::from)
            .collect();
        if addresses.is_empty() {
            return Err(HooksListenError::Empty);
        }
        for a in &addresses {
            parse_listen(a).map_err(|_| HooksListenError::NotHostPort(a.clone()))?;
        }
        let (stop, _) = watch::channel(false);
        Ok(Self {
            addresses,
            app: hooks_app(intake),
            servers: Mutex::new(Vec::new()),
            stop,
        })
    }

    /// Bind every address and serve; the first that won't bind is the error.
    pub async fn start(&self) -> io::Result<()> {
        for address in &self.addresses {
            let (host, port) = parse_listen(address)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            let socket = TcpListener::bind((host.as_str(), port)).await?;
            let port = socket.local_addr()?.port();
            let mut stop = self.stop.subscribe();
            let serve = axum::serve(socket, self.app.clone()).with_graceful_shutdown(async move {
                let _ = stop.wait_for(|s| *s).await;
            });
            let task = tokio::spawn(std::future::IntoFuture::into_future(serve));
            lock(&self.servers).push(Server {
                address: address.clone(),
                port,
                task,
            });
            info!("hooks: listening on {address}");
        }
        Ok(())
    }

    /// The addresses serving now.
    pub fn bound(&self) -> Vec<String> {
        lock(&self.servers)
            .iter()
            .map(|s| s.address.clone())
            .collect()
    }

    /// The bound ports, in the addresses' order (useful when a configured port is 0, in
    /// tests).
    pub fn ports(&self) -> Vec<u16> {
        lock(&self.servers).iter().map(|s| s.port).collect()
    }

    /// The first bound port.
    pub fn port(&self) -> u16 {
        self.ports().first().copied().unwrap_or(0)
    }

    pub async fn stop(&self) {
        self.stop.send_replace(true);
        let servers: Vec<Server> = std::mem::take(&mut *lock(&self.servers));
        for server in servers {
            let abort = server.task.abort_handle();
            if tokio::time::timeout(STOP_WAIT, server.task).await.is_err() {
                warn!("hooks: connections still open at shutdown; closing them");
                abort.abort();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! The webhook listener (ADR 0027): one route, the body capped before it is read, the
    //! reply never echoes it.

    use std::convert::Infallible;
    use std::task::Poll;

    use axum::body::Bytes;
    use ghost_core::hooks::{
        H_EVENT, H_ID, H_SIG, H_TIME, Hook, ItemsFactory, Secrets, parse_hook, sign_hmac,
    };
    use ghost_core::store::Store;
    use ghost_core::time::utcnow;
    use ghost_core::waiting::Items;
    use ghost_core::watches::WaitingItems;
    use tower::ServiceExt;

    use super::*;

    pub(crate) fn jobs() -> Value {
        json!({
            "verify": "hmac",
            "secret": "${JOB_SECRET}",
            "events": {
                "done": {
                    "fields": {
                        "job": {"at": "job", "pattern": r"^[\w-]+$"},
                        "ok": {"at": "ok", "type": "bool"},
                    },
                    "when": [{"field": "ok", "op": "==", "value": false}],
                    "key": "job:{job}",
                    "title": "{job} failed",
                }
            },
        })
    }

    struct Wired {
        intake: Arc<HookIntake>,
        hook: Hook,
        items: Arc<Items>,
    }

    fn wired() -> Wired {
        let store = Arc::new(Store::in_memory().unwrap());
        let items = Arc::new(Items::new(store.clone()));
        let secrets: Arc<dyn Secrets> =
            Arc::new(HashMap::from([("JOB_SECRET".to_string(), "k".to_string())]));
        let factory: ItemsFactory =
            Arc::new(|store| Arc::new(Items::new(store)) as Arc<dyn WaitingItems>);
        let intake = HookIntake::new(
            Arc::clone(&items) as Arc<dyn WaitingItems>,
            store,
            secrets,
            factory,
        );
        let hook = parse_hook("jobs", &jobs(), "test", None).unwrap();
        intake
            .hooks
            .lock()
            .unwrap()
            .insert(hook.path(), hook.clone());
        Wired {
            intake: Arc::new(intake),
            hook,
            items,
        }
    }

    fn signed(body: &[u8], delivery: &str) -> Vec<(&'static str, String)> {
        let ts = utcnow().timestamp().to_string();
        let sig = sign_hmac("k", &ts, delivery, body);
        vec![
            (H_TIME, ts),
            (H_ID, delivery.to_string()),
            (H_EVENT, "done".to_string()),
            (H_SIG, sig),
        ]
    }

    async fn send(app: &Router, req: Request) -> (StatusCode, Value) {
        let resp = app.clone().oneshot(req).await.unwrap();
        let (head, body) = resp.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (
            head.status,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }

    fn post(path: &str, body: Vec<u8>, headers: &[(&str, String)]) -> Request {
        let mut req = Request::builder().method("POST").uri(path);
        for (k, v) in headers {
            req = req.header(*k, v.as_str());
        }
        req.body(Body::from(body)).unwrap()
    }

    #[tokio::test]
    async fn only_post_to_a_declared_hook_reaches_the_intake() {
        let w = wired();
        let app = hooks_app(w.intake.clone());
        let body = serde_json::to_vec(&json!({"job": "backup", "ok": false})).unwrap();
        let r = send(
            &app,
            post(&w.hook.path(), body.clone(), &signed(&body, "d1")),
        )
        .await;
        assert_eq!(r, (StatusCode::ACCEPTED, json!({"ok": true})));
        let titles: Vec<String> = w
            .items
            .open(None)
            .unwrap()
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(titles, ["backup failed"]);
        let get = Request::builder()
            .uri(w.hook.path())
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, get).await.0, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(w.hook.path(), "/hooks/jobs"); // ADR 0035: by name alone
        for path in ["/hooks/nope", "/status", "/hooks/ghost/jobs", "/hooks/"] {
            let r = send(&app, post(path, b"{}".to_vec(), &[])).await;
            assert_eq!(r.0, StatusCode::NOT_FOUND, "{path}");
        }
        let mut bad = signed(&body, "d2");
        bad.retain(|(k, _)| *k != H_SIG);
        bad.push((H_SIG, "sha256=00".to_string()));
        let r = send(&app, post(&w.hook.path(), body.clone(), &bad)).await;
        assert_eq!(r, (StatusCode::UNAUTHORIZED, json!({"error": "refused"}))); // no reason, no echo
        let big = vec![b'x'; MAX_BODY + 1];
        let r = send(&app, post(&w.hook.path(), big, &signed(b"", "d3"))).await;
        assert_eq!(r.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_signature_header_with_a_high_byte_is_refused_not_a_crash() {
        let w = wired();
        let app = hooks_app(w.intake.clone());
        let body = serde_json::to_vec(&json!({"job": "backup", "ok": false})).unwrap();
        let mut req = Request::builder().method("POST").uri(w.hook.path());
        for (k, v) in signed(&body, "d1") {
            if k != H_SIG {
                req = req.header(k, v);
            }
        }
        req = req.header(H_SIG, HeaderValue::from_bytes(b"sha256=\xff").unwrap());
        let r = send(&app, req.body(Body::from(body)).unwrap()).await;
        assert_eq!(r, (StatusCode::UNAUTHORIZED, json!({"error": "refused"})));
    }

    #[tokio::test]
    async fn an_unknown_hook_is_not_found_before_its_body_is_read() {
        let w = wired();
        let app = hooks_app(w.intake.clone());
        let unread = Body::from_stream(futures::stream::poll_fn(
            |_| -> Poll<Option<Result<Bytes, Infallible>>> { panic!("the body was read") },
        ));
        let req = Request::builder()
            .method("POST")
            .uri("/hooks/nope")
            .body(unread)
            .unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_listener_binds_its_own_address() {
        let w = wired();
        let listener = HooksListener::new("127.0.0.1:0", w.intake.clone()).unwrap();
        listener.start().await.unwrap();
        let body = serde_json::to_vec(&json!({"job": "render", "ok": false})).unwrap();
        let mut req = reqwest::Client::new().post(format!(
            "http://127.0.0.1:{}{}",
            listener.port(),
            w.hook.path()
        ));
        for (k, v) in signed(&body, "d1") {
            req = req.header(k, v);
        }
        let r = req.body(body).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        assert_eq!(listener.bound(), vec!["127.0.0.1:0"]);
        listener.stop().await;
        assert!(listener.bound().is_empty());

        assert_eq!(
            HooksListener::new(" , ", w.intake.clone())
                .err()
                .map(|e| e.to_string()),
            Some("GHOST_HOOKS_LISTEN is empty".to_string())
        );
        assert!(HooksListener::new("nope", w.intake.clone()).is_err());
    }
}
