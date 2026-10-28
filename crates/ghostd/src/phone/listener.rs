//! The phone listener (ADR 0016, ADR 0020): a small signed HTTP app on the configured addresses.
//!
//! It is not the control API and does not proxy it. Its routes: `POST /pair`
//! (unsigned, spends a pairing code), `GET /hello`, `GET /audit`, `GET /waiting` with
//! `POST /waiting/ack` and `/waiting/snooze` (ADR 0029), `POST`/`PATCH /offer`
//! (WebRTC signalling) and `POST /end`. Everything but `/pair` is signed both
//! ways by the signing middleware; the wire format is docs/phone-protocol.md.
//!
//! This slice holds the middleware, the pairing state and `/pair`, the pairing's view for
//! the control API, and how a verified request is noted (`seen`). The other routes need
//! the daemon's views and the media, and the servers on each address come with them;
//! they mount onto `router`.
//!
//! `GHOST_PHONE_LISTEN` may list several `host:port` addresses, comma separated: the LAN
//! address and the operator's tunnel address (ADR 0020). Each verified request notes which
//! listen address it came in on and whether that is the home network or the tunnel
//! (`networks.rs`); `GET /phone` on the control API shows it.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use ghost_api::models::{PhoneAddress, PhoneDevice, PhonePairing, PhoneView};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tracing::info;

use super::networks::Networks;
use super::pairing::{PairingCodes, PhoneStore};
use super::signing::{H_DEVICE, H_NONCE, H_SIGNATURE, H_TIME, Unverified, Verifier, sign_response};
use super::{Clock, wall};

/// Why the daemon won't start a phone session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    NotReady,
    NoSuchLine,
}

impl Refusal {
    pub fn status(self) -> StatusCode {
        match self {
            Refusal::Busy => StatusCode::CONFLICT,
            Refusal::NotReady => StatusCode::SERVICE_UNAVAILABLE,
            Refusal::NoSuchLine => StatusCode::NOT_FOUND,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Refusal::Busy => "busy",
            Refusal::NotReady => "not_ready",
            Refusal::NoSuchLine => "no_such_line",
        }
    }
}

/// How often a listen address whose interface isn't up yet is tried again.
pub const REBIND_SECS: f64 = 5.0;
pub const UNSIGNED_PATHS: [&str; 1] = ["/pair"];
/// The phone app's own scheme (apps/mobile/app.json).
pub const PAIR_LINK: &str = "ghost://pair";
/// A signed request's body may be this long (an SDP offer is well under it).
pub const MAX_BODY: usize = 256 * 1024;

/// What `{error: code}` says for a status with no code of its own.
pub fn status_code_name(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "bad_code",
        404 => "not_found",
        405 => "method_not_allowed",
        409 => "busy",
        422 => "bad_request",
        500 => "internal",
        503 => "not_ready",
        _ => "error",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ListenError {
    #[error("GHOST_PHONE_LISTEN must be host:port, got '{0}'")]
    NotHostPort(String),
    #[error("GHOST_PHONE_LISTEN is empty")]
    Empty,
}

/// `host:port` or `[v6]:port`.
pub fn parse_listen(value: &str) -> Result<(String, u16), ListenError> {
    let text = value.trim();
    let Some((host, port)) = text.rsplit_once(':') else {
        return Err(ListenError::NotHostPort(value.to_string()));
    };
    if host.is_empty() || port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return Err(ListenError::NotHostPort(value.to_string()));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| ListenError::NotHostPort(value.to_string()))?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Ok((host.to_string(), port))
}

/// `GHOST_PHONE_LISTEN`: one or more `host:port`, comma separated, each validated.
pub fn parse_listen_list(value: &str) -> Result<Vec<String>, ListenError> {
    let addresses: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect();
    if addresses.is_empty() {
        return Err(ListenError::Empty);
    }
    for a in &addresses {
        parse_listen(a)?;
    }
    Ok(addresses)
}

/// `urllib.parse.quote_plus`: everything but letters, digits and `-_.~` is encoded.
const FORM: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn form_encode(value: &str) -> String {
    utf8_percent_encode(value, FORM)
        .to_string()
        .replace("%20", "+")
}

/// The QR code's payload: `ghost://pair?code=482913&address=…&address=…`, every
/// listen address in order (docs/phone-protocol.md). It carries what typing would.
pub fn pair_link(code: &str, addresses: &[String]) -> String {
    let mut query = vec![format!("code={}", form_encode(code))];
    query.extend(
        addresses
            .iter()
            .map(|a| format!("address={}", form_encode(a))),
    );
    format!("{PAIR_LINK}?{}", query.join("&"))
}

/// A JSON response with `status`.
pub fn json_response(status: StatusCode, body: &Value) -> Response {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    plain(status, payload, None)
}

/// `{error: code}`, the code from the status when none is given.
pub fn error(status: StatusCode, code: Option<&str>) -> Response {
    let code = code.unwrap_or_else(|| status_code_name(status));
    json_response(status, &json!({ "error": code }))
}

/// The fallback: a route the listener doesn't have.
pub async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, None)
}

fn plain(status: StatusCode, payload: Vec<u8>, signed: Option<(String, String)>) -> Response {
    let mut resp = Response::new(Body::from(payload.clone()));
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(payload.len()));
    if let Some((ts, signature)) = signed {
        add_signature(headers, &ts, &signature);
    }
    resp
}

fn add_signature(headers: &mut axum::http::HeaderMap, ts: &str, signature: &str) {
    if let (Ok(t), Ok(s)) = (HeaderValue::from_str(ts), HeaderValue::from_str(signature)) {
        headers.insert(H_TIME, t);
        headers.insert(H_SIGNATURE, s);
    }
}

// --- request bodies ------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_name: String,
}

impl PairRequest {
    /// The bounds the Python model declared: a code of at most 16 characters, a name of 1
    /// to 64.
    pub fn check(&self) -> bool {
        let name = self.device_name.chars().count();
        self.code.chars().count() <= 16 && (1..=64).contains(&name)
    }
}

// --- signing middleware -------------------------------------------------------------------

/// The local IP a connection landed on, when the server that accepted it says (an
/// extension it adds to each request): what tells home from tunnel on a wildcard address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalAddr(pub IpAddr);

/// A verified request: the listen address it came in on and the local IP it landed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    pub listen: Option<String>,
    pub local: Option<IpAddr>,
}

/// The middleware's state: who verifies, what to tell on success, and which listen address
/// this server serves.
pub struct Signing {
    pub verifier: Arc<Verifier>,
    pub on_verified: Box<dyn Fn(&Reach) + Send + Sync>,
    pub listen: Option<String>,
    pub clock: Clock,
}

impl Signing {
    pub fn new(
        verifier: Arc<Verifier>,
        on_verified: impl Fn(&Reach) + Send + Sync + 'static,
    ) -> Self {
        Self {
            verifier,
            on_verified: Box::new(on_verified),
            listen: None,
            clock: wall(),
        }
    }

    pub fn with_listen(mut self, listen: impl Into<String>) -> Self {
        self.listen = Some(listen.into());
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }
}

/// Verifies every request outside `UNSIGNED_PATHS` and signs every response to one,
/// errors included. It reads the exact body bytes both ways, before any route sees them.
pub async fn sign_requests(State(s): State<Arc<Signing>>, req: Request, next: Next) -> Response {
    if UNSIGNED_PATHS.contains(&req.uri().path()) {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::to_vec(&json!({ "error": "too_large" })).unwrap_or_default(),
            None,
        );
    };
    let header = |name: &str| -> Option<String> {
        parts
            .headers
            .get(name)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    };
    let device = header(H_DEVICE);
    let nonce = header(H_NONCE);
    let ts = header(H_TIME);
    let signature = header(H_SIGNATURE);
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), |p| p.as_str().to_string());
    let ok = s.verifier.verify(&Unverified {
        method: parts.method.as_str(),
        path: &path,
        body: &bytes,
        device_id: device.as_deref(),
        ts: ts.as_deref(),
        nonce: nonce.as_deref(),
        signature: signature.as_deref(),
    });
    let secret = s.verifier.secret_for(device.as_deref());
    let sign = |status: StatusCode, payload: &[u8]| -> Option<(String, String)> {
        // Unknown device: nothing to sign with; the phone drops it.
        let (secret, nonce) = (secret.as_ref()?, nonce.as_deref()?);
        let ts = ((s.clock)().floor() as i64).to_string();
        let signature = sign_response(secret, status.as_u16(), nonce, &ts, payload);
        Some((ts, signature))
    };

    if !ok {
        // What was refused, never the headers: they carry the signature and nonce.
        let known = if secret.is_some() {
            "paired device"
        } else {
            "unknown device"
        };
        info!(
            "phone: refused {} {} ({known})",
            parts.method,
            parts.uri.path()
        );
        let payload = serde_json::to_vec(&json!({ "error": "unauthorized" })).unwrap_or_default();
        let signed = sign(StatusCode::UNAUTHORIZED, &payload);
        return plain(StatusCode::UNAUTHORIZED, payload, signed);
    }
    let local = parts.extensions.get::<LocalAddr>().map(|l| l.0);
    (s.on_verified)(&Reach {
        listen: s.listen.clone(),
        local,
    });

    let req = Request::from_parts(parts, Body::from(bytes));
    let resp = next.run(req).await;
    let (mut head, body) = resp.into_parts();
    let payload: Bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    if let Some((ts, signature)) = sign(head.status, &payload) {
        add_signature(&mut head.headers, &ts, &signature);
    }
    head.headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(payload.len()));
    Response::from_parts(head, Body::from(payload))
}

// --- the listener ------------------------------------------------------------------------

/// What the daemon gives the listener. This slice needs one thing of it; the routes that
/// read the status, the audit, the waiting list and the agents, and that admit and start
/// a phone session, add theirs.
#[async_trait]
pub trait PhoneHost: Send + Sync {
    /// End the phone's session if one is running.
    async fn end_phone(&self) -> bool;
}

/// Pairing state and the signed app's foundation; one server per configured address
/// comes with the routes.
pub struct PhoneListener {
    pub addresses: Vec<String>,
    /// The one the desktop shows for pairing.
    pub address: String,
    pub store: Arc<PhoneStore>,
    pub codes: Mutex<PairingCodes>,
    pub verifier: Arc<Verifier>,
    pub workstation: String,
    pub networks: Networks,
    pub host: Arc<dyn PhoneHost>,
    bound: Mutex<Vec<String>>,
}

impl PhoneListener {
    pub fn new(
        address: &str,
        store: Arc<PhoneStore>,
        host: Arc<dyn PhoneHost>,
    ) -> Result<Self, ListenError> {
        let addresses = parse_listen_list(address)?;
        let lookup = {
            let store = Arc::clone(&store);
            move |id: &str| store.secret_for(id)
        };
        let workstation = nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();
        Ok(Self {
            address: addresses[0].clone(),
            addresses,
            store,
            codes: Mutex::new(PairingCodes::new()),
            verifier: Arc::new(Verifier::new(lookup)),
            workstation,
            networks: Networks::new(),
            host,
            bound: Mutex::new(Vec::new()),
        })
    }

    pub fn with_codes(mut self, codes: PairingCodes) -> Self {
        self.codes = Mutex::new(codes);
        self
    }

    pub fn with_verifier(mut self, verifier: Verifier) -> Self {
        self.verifier = Arc::new(verifier);
        self
    }

    pub fn with_workstation(mut self, workstation: impl Into<String>) -> Self {
        self.workstation = workstation.into();
        self
    }

    pub fn with_networks(mut self, networks: Networks) -> Self {
        self.networks = networks;
        self
    }

    // --- control API side (the controller delegates here) ------------------------------

    pub fn new_pairing(&self) -> PhonePairing {
        let (code, expires_at) = self.codes.lock().unwrap_or_else(|e| e.into_inner()).mint();
        info!("phone: pairing code issued (valid 2 minutes)"); // never the code
        PhonePairing {
            link: pair_link(&code, &self.addresses),
            code,
            address: self.address.clone(),
            addresses: self.addresses.clone(),
            expires_at,
        }
    }

    pub fn view(&self) -> PhoneView {
        let device = self.store.device().map(|d| PhoneDevice {
            id: d.id,
            name: d.name,
            paired_at: d.paired_at,
            last_seen: d.last_seen,
            last_address: d.last_address,
            last_network: d.last_network,
        });
        let bound = self.bound();
        let listen = self
            .addresses
            .iter()
            .map(|a| PhoneAddress {
                address: a.clone(),
                network: parse_listen(a)
                    .ok()
                    .and_then(|(host, _)| self.networks.of(Some(&host))),
                listening: bound.contains(a),
            })
            .collect();
        PhoneView {
            address: Some(self.address.clone()),
            addresses: self.addresses.clone(),
            listen,
            device,
        }
    }

    /// The addresses serving right now (a late one is missing until it binds).
    pub fn bound(&self) -> Vec<String> {
        self.bound.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A server bound `address` (or stopped serving it).
    pub fn set_bound(&self, address: &str, listening: bool) {
        let mut bound = self.bound.lock().unwrap_or_else(|e| e.into_inner());
        bound.retain(|a| a != address);
        if listening {
            bound.push(address.to_string());
        }
    }

    /// A verified request: note the listen address it came in on and its network. The
    /// network is the local IP the connection landed on, so a wildcard address
    /// (`0.0.0.0:8765`) still tells home from tunnel.
    pub fn seen(&self, reach: &Reach) {
        let mut network = reach
            .local
            .and_then(|ip| self.networks.of(Some(&ip.to_string())));
        if network.is_none()
            && let Some(address) = &reach.listen
            && let Ok((host, _)) = parse_listen(address)
        {
            network = self.networks.of(Some(&host));
        }
        self.store.seen(reach.listen.as_deref(), network);
    }

    pub async fn forget(&self) -> bool {
        let had = self.store.forget();
        if had {
            self.host.end_phone().await;
        }
        had
    }

    // --- the routes ------------------------------------------------------------------------

    /// `POST /pair`: spends the code, pairs the phone, answers with its credentials. The
    /// one route that is unsigned.
    pub fn pair(&self, body: &[u8]) -> Response {
        let Ok(req) = serde_json::from_slice::<PairRequest>(body) else {
            return error(StatusCode::UNPROCESSABLE_ENTITY, None);
        };
        if !req.check() {
            return error(StatusCode::UNPROCESSABLE_ENTITY, None);
        }
        let redeemed = self
            .codes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .redeem(req.code.trim());
        if !redeemed {
            info!("phone: pairing refused (wrong or spent code)");
            return error(StatusCode::FORBIDDEN, Some("bad_code"));
        }
        let device = self.store.pair(req.device_name.trim());
        json_response(
            StatusCode::OK,
            &json!({
                "device_id": device.id,
                "secret": device.secret.as_str(),
                "workstation": self.workstation,
                "addresses": self.addresses,
            }),
        )
    }

    /// The app for one listen address: `routes` (the signed ones, from the routes slice)
    /// with `/pair`, the `not_found` fallback, and the signing middleware around all of it.
    pub fn router(self: &Arc<Self>, listen: Option<&str>, routes: Router) -> Router {
        let me = Arc::clone(self);
        let signing = {
            let me = Arc::clone(self);
            let mut s = Signing::new(Arc::clone(&self.verifier), move |reach| me.seen(reach));
            if let Some(listen) = listen {
                s = s.with_listen(listen);
            }
            Arc::new(s)
        };
        routes
            .route(
                "/pair",
                post(move |body: Bytes| async move { me.pair(&body) }),
            )
            .fallback(not_found)
            .layer(axum::middleware::from_fn_with_state(signing, sign_requests))
    }
}

/// A JSON reply for a route (the status, then the body).
pub fn reply<T: serde::Serialize>(status: StatusCode, body: T) -> Response {
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    //! The phone listener (ADR 0016, docs/phone-protocol.md): pairing, signatures both ways,
    //! and how the desk records a reach.

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::Query;
    use axum::routing::{get, patch};
    use tower::ServiceExt;

    use super::super::networks::Network;
    use super::super::signing::{Secret, new_nonce, signed_headers, verify_response};
    use super::*;

    struct FakeHost {
        ended: AtomicUsize,
    }

    #[async_trait]
    impl PhoneHost for FakeHost {
        async fn end_phone(&self) -> bool {
            self.ended.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    fn clock(start: f64) -> (Arc<Mutex<f64>>, Clock) {
        let t = Arc::new(Mutex::new(start));
        let c: Clock = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap())
        };
        (t, c)
    }

    /// The routes a later slice brings, as stubs: enough to see the middleware work.
    fn stub_routes() -> Router {
        async fn hello() -> Response {
            reply(
                StatusCode::OK,
                json!({"workstation": "desk", "status": {"session_origin": null}}),
            )
        }
        async fn audit(Query(q): Query<HashMap<String, String>>) -> Response {
            let limit: u32 = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
            reply(
                StatusCode::OK,
                json!({"items": [], "total": 0, "limit": limit}),
            )
        }
        async fn offer(body: Bytes) -> Response {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if v.get("type").and_then(Value::as_str) != Some("offer") {
                return error(StatusCode::UNPROCESSABLE_ENTITY, None);
            }
            reply(StatusCode::OK, json!({"ok": true}))
        }
        async fn candidates() -> Response {
            error(StatusCode::NOT_FOUND, None)
        }
        Router::new()
            .route("/hello", get(hello))
            .route("/audit", get(audit))
            .route("/offer", post(offer))
            .route("/offer", patch(candidates))
    }

    struct Phone {
        listener: Arc<PhoneListener>,
        host: Arc<FakeHost>,
        app: Router,
        clock: Arc<Mutex<f64>>,
        dir: tempfile::TempDir,
    }

    fn phone() -> Phone {
        let dir = tempfile::tempdir().unwrap();
        let (clock, c) = clock(1000.0);
        let host = Arc::new(FakeHost {
            ended: AtomicUsize::new(0),
        });
        let store = Arc::new(PhoneStore::new(dir.path().join("phone.json")));
        let listener = Arc::new(
            PhoneListener::new("127.0.0.1:0", store, host.clone())
                .unwrap()
                .with_codes(PairingCodes::new().with_clock(c))
                .with_workstation("desk"),
        );
        let app = listener.router(None, stub_routes());
        Phone {
            listener,
            host,
            app,
            clock,
            dir,
        }
    }

    struct Creds {
        device_id: String,
        secret: Secret,
    }

    struct Reply {
        status: StatusCode,
        headers: axum::http::HeaderMap,
        body: Bytes,
    }

    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    async fn send(app: &Router, req: Request) -> Reply {
        let resp = app.clone().oneshot(req).await.unwrap();
        let (head, body) = resp.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        Reply {
            status: head.status,
            headers: head.headers,
            body,
        }
    }

    async fn post_json(app: &Router, path: &str, body: Value) -> Reply {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        send(app, req).await
    }

    async fn pair(p: &Phone, name: &str) -> (Creds, Value) {
        let pairing = p.listener.new_pairing();
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": pairing.code, "device_name": name}),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
        let v = r.json();
        let creds = Creds {
            device_id: v["device_id"].as_str().unwrap().to_string(),
            secret: Secret::parse(v["secret"].as_str().unwrap()).unwrap(),
        };
        (creds, v)
    }

    async fn signed_at(
        app: &Router,
        creds: &Creds,
        method: &str,
        path: &str,
        body: Option<Value>,
        now: Option<f64>,
    ) -> (Reply, String) {
        let raw = body
            .map(|b| serde_json::to_vec(&b).unwrap())
            .unwrap_or_default();
        let h = signed_headers(
            &creds.secret,
            &creds.device_id,
            method,
            path,
            &raw,
            now,
            None,
        );
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        if !raw.is_empty() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(Body::from(raw)).unwrap();
        (send(app, req).await, h.nonce)
    }

    async fn signed(
        app: &Router,
        creds: &Creds,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (Reply, String) {
        signed_at(app, creds, method, path, body, None).await
    }

    fn response_ok(r: &Reply, creds: &Creds, nonce: &str) -> bool {
        let ts = r.headers.get(H_TIME).and_then(|v| v.to_str().ok());
        let sig = r.headers.get(H_SIGNATURE).and_then(|v| v.to_str().ok());
        let (Some(ts), Some(sig)) = (ts, sig) else {
            return false;
        };
        verify_response(&creds.secret, r.status.as_u16(), nonce, ts, &r.body, sig)
    }

    // --- signing ------------------------------------------------------------------------------

    #[tokio::test]
    async fn pair_then_hello_and_audit_are_signed_both_ways() {
        let p = phone();
        let (creds, reply) = pair(&p, "Pixel 9").await;
        let mut keys: Vec<&str> = reply
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["addresses", "device_id", "secret", "workstation"]);
        assert_eq!(reply["workstation"], "desk");
        assert_eq!(reply["secret"].as_str().unwrap().len(), 43); // 32 bytes b64url

        let (r, nonce) = signed(&p.app, &creds, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json()["workstation"], "desk");
        assert_eq!(r.json()["status"]["session_origin"], Value::Null);
        assert!(response_ok(&r, &creds, &nonce));
        assert!(!response_ok(&r, &creds, &new_nonce())); // bound to the request's nonce

        let (r, nonce) = signed(&p.app, &creds, "GET", "/audit?limit=5&offset=0", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json()["limit"], 5);
        assert!(response_ok(&r, &creds, &nonce));
        assert!(p.listener.store.device().unwrap().last_seen.is_some());
    }

    #[tokio::test]
    async fn bad_signature_stale_time_and_replay_are_refused() {
        let p = phone();
        let (creds, _) = pair(&p, "Pixel 9").await;

        // wrong secret
        let reversed: String = creds.secret.as_str().chars().rev().collect();
        let bad = Creds {
            device_id: creds.device_id.clone(),
            secret: Secret::parse(&reversed).unwrap(),
        };
        let (r, nonce) = signed(&p.app, &bad, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert_eq!(r.json(), json!({"error": "unauthorized"}));
        assert!(response_ok(&r, &creds, &nonce)); // errors to a known device are signed too

        // signed for one path, sent to another
        let h = signed_headers(
            &creds.secret,
            &creds.device_id,
            "GET",
            "/hello",
            b"",
            None,
            None,
        );
        let mut req = Request::builder().method("GET").uri("/audit");
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        let r = send(&p.app, req.body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);

        // tampered body
        let body = serde_json::to_vec(&json!({"sdp": "x", "type": "offer"})).unwrap();
        let h = signed_headers(
            &creds.secret,
            &creds.device_id,
            "POST",
            "/offer",
            &body,
            None,
            None,
        );
        let mut req = Request::builder().method("POST").uri("/offer");
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        let mut tampered = body.clone();
        tampered.push(b' ');
        let r = send(&p.app, req.body(Body::from(tampered)).unwrap()).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);

        // more than 60 s off, either way
        let now = super::super::wall_clock();
        for skew in [-61.0, 61.0] {
            let (r, _) = signed_at(&p.app, &creds, "GET", "/hello", None, Some(now + skew)).await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED, "skew {skew}");
        }
        let (r, _) = signed_at(&p.app, &creds, "GET", "/hello", None, Some(now - 50.0)).await;
        assert_eq!(r.status, StatusCode::OK);

        // replay: the same signed request twice
        let h = signed_headers(
            &creds.secret,
            &creds.device_id,
            "GET",
            "/hello",
            b"",
            None,
            None,
        );
        for want in [StatusCode::OK, StatusCode::UNAUTHORIZED] {
            let mut req = Request::builder().method("GET").uri("/hello");
            for (k, v) in h.pairs() {
                req = req.header(k, v);
            }
            let r = send(&p.app, req.body(Body::empty()).unwrap()).await;
            assert_eq!(r.status, want);
        }

        // no headers at all, and an unknown device: 401, unsigned (nothing to sign with)
        let r = send(
            &p.app,
            Request::builder()
                .uri("/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(!r.headers.contains_key(H_SIGNATURE));
        let nope = Creds {
            device_id: "nope".into(),
            secret: creds.secret.clone(),
        };
        let (r, _) = signed(&p.app, &nope, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(!r.headers.contains_key(H_SIGNATURE));
    }

    #[tokio::test]
    async fn error_responses_are_signed() {
        let p = phone();
        let (creds, _) = pair(&p, "Pixel 9").await;
        let (r, nonce) = signed(
            &p.app,
            &creds,
            "PATCH",
            "/offer",
            Some(json!({"pc_id": "nope", "candidates": []})),
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert_eq!(r.json(), json!({"error": "not_found"}));
        assert!(response_ok(&r, &creds, &nonce));

        let (r, nonce) = signed(
            &p.app,
            &creds,
            "POST",
            "/offer",
            Some(json!({"type": "answer"})),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(r.json(), json!({"error": "bad_request"}));
        assert!(response_ok(&r, &creds, &nonce));

        let (r, nonce) = signed(&p.app, &creds, "GET", "/nowhere", None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert_eq!(r.json(), json!({"error": "not_found"}));
        assert!(response_ok(&r, &creds, &nonce));
    }

    #[tokio::test]
    async fn a_body_past_the_limit_is_refused_before_it_is_read() {
        let p = phone();
        let (creds, _) = pair(&p, "p").await;
        let big = vec![b'x'; MAX_BODY + 1];
        let h = signed_headers(
            &creds.secret,
            &creds.device_id,
            "POST",
            "/offer",
            &big,
            None,
            None,
        );
        let mut req = Request::builder().method("POST").uri("/offer");
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        let r = send(&p.app, req.body(Body::from(big)).unwrap()).await;
        assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(r.json(), json!({"error": "too_large"}));
        assert!(p.listener.store.device().unwrap().last_seen.is_none());
    }

    // --- pairing ------------------------------------------------------------------------------

    #[tokio::test]
    async fn code_expires_after_two_minutes() {
        let p = phone();
        let pairing = p.listener.new_pairing();
        *p.clock.lock().unwrap() += 121.0;
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": pairing.code, "device_name": "p"}),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_eq!(r.json(), json!({"error": "bad_code"}));
        assert!(p.listener.store.device().is_none());
    }

    #[tokio::test]
    async fn five_wrong_tries_spend_the_code() {
        let p = phone();
        let pairing = p.listener.new_pairing();
        let wrong = format!(
            "{:06}",
            (pairing.code.parse::<u32>().unwrap() + 1) % 1_000_000
        );
        for _ in 0..5 {
            let r = post_json(&p.app, "/pair", json!({"code": wrong, "device_name": "p"})).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN);
        }
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": pairing.code, "device_name": "p"}),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN); // spent, even though it is right
    }

    #[tokio::test]
    async fn code_is_single_use_and_a_new_one_replaces_the_old() {
        let p = phone();
        let first = p.listener.new_pairing();
        let second = p.listener.new_pairing();
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": first.code, "device_name": "p"}),
        )
        .await;
        assert!(r.status == StatusCode::FORBIDDEN || first.code == second.code);
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": second.code, "device_name": "p"}),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": second.code, "device_name": "p"}),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_pair_body_outside_its_bounds_is_a_bad_request() {
        let p = phone();
        let pairing = p.listener.new_pairing();
        let r = post_json(
            &p.app,
            "/pair",
            json!({"code": pairing.code, "device_name": ""}),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(r.json(), json!({"error": "bad_request"}));
        let r = post_json(&p.app, "/pair", json!({"code": pairing.code})).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(p.listener.codes.lock().unwrap().active()); // nothing was spent
    }

    #[tokio::test]
    async fn pairing_again_replaces_the_device() {
        let p = phone();
        let (old, _) = pair(&p, "old phone").await;
        let (new, _) = pair(&p, "new phone").await;
        assert_ne!(old.device_id, new.device_id);
        assert_ne!(old.secret, new.secret);
        assert_eq!(
            signed(&p.app, &old, "GET", "/hello", None).await.0.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            signed(&p.app, &new, "GET", "/hello", None).await.0.status,
            StatusCode::OK
        );
        assert_eq!(p.listener.view().device.unwrap().name, "new phone");

        use std::os::unix::fs::PermissionsExt;
        let path = p.dir.path().join("phone.json");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["id"], new.device_id);
        assert_eq!(saved["secret"], new.secret.as_str());
        // survives a restart
        assert_eq!(
            PhoneStore::new(&path).secret_for(&new.device_id).unwrap(),
            new.secret
        );
    }

    #[tokio::test]
    async fn forget_revokes_at_once_and_ends_a_phone_session() {
        let p = phone();
        let (creds, _) = pair(&p, "Pixel 9").await;
        assert!(p.listener.forget().await);
        assert_eq!(p.host.ended.load(Ordering::SeqCst), 1);
        assert_eq!(
            signed(&p.app, &creds, "GET", "/hello", None).await.0.status,
            StatusCode::UNAUTHORIZED
        );
        assert!(!p.dir.path().join("phone.json").exists());
        assert!(p.listener.view().device.is_none());
        assert!(!p.listener.forget().await);
    }

    #[test]
    fn parse_listen_() {
        assert_eq!(
            parse_listen("192.168.1.20:8765").unwrap(),
            ("192.168.1.20".to_string(), 8765)
        );
        assert_eq!(
            parse_listen("[fe80::1]:8765").unwrap(),
            ("fe80::1".to_string(), 8765)
        );
        for bad in ["", "8765", "host:", "host:port"] {
            assert!(parse_listen(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse_listen("host:port").unwrap_err().to_string(),
            "GHOST_PHONE_LISTEN must be host:port, got 'host:port'"
        );
        // ADR 0020: the LAN address and the tunnel address, comma separated
        assert_eq!(
            parse_listen_list("192.168.1.20:8765, 10.8.0.4:8765").unwrap(),
            vec!["192.168.1.20:8765", "10.8.0.4:8765"]
        );
        for bad in ["", " , ", "192.168.1.20:8765,nope"] {
            assert!(parse_listen_list(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse_listen_list("").unwrap_err().to_string(),
            "GHOST_PHONE_LISTEN is empty"
        );
    }

    // --- how the phone reached Ghost (ADR 0020, #36) -------------------------------------------

    #[test]
    fn the_pairing_link_carries_the_code_and_every_address() {
        let link = pair_link(
            "482913",
            &[
                "192.168.1.20:8765".to_string(),
                "10.8.0.4:8765".to_string(),
                "[fd00::4]:8765".to_string(),
            ],
        );
        assert_eq!(
            link,
            "ghost://pair?code=482913&address=192.168.1.20%3A8765\
             &address=10.8.0.4%3A8765&address=%5Bfd00%3A%3A4%5D%3A8765"
        );
        let (scheme, rest) = link.split_once("://").unwrap();
        let (host, query) = rest.split_once('?').unwrap();
        assert_eq!((scheme, host), ("ghost", "pair"));
        let pairs: Vec<(String, String)> = query
            .split('&')
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap();
                (
                    k.to_string(),
                    percent_encoding::percent_decode_str(v)
                        .decode_utf8()
                        .unwrap()
                        .into_owned(),
                )
            })
            .collect();
        assert_eq!(pairs[0], ("code".to_string(), "482913".to_string()));
        let addresses: Vec<&str> = pairs
            .iter()
            .filter(|(k, _)| k == "address")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(
            addresses,
            ["192.168.1.20:8765", "10.8.0.4:8765", "[fd00::4]:8765"]
        );
    }

    #[tokio::test]
    async fn a_pairing_has_the_link_for_its_qr_code() {
        let p = phone();
        let pairing = p.listener.new_pairing();
        assert_eq!(
            pairing.link,
            format!("ghost://pair?code={}&address=127.0.0.1%3A0", pairing.code)
        );
        assert_eq!(pairing.address, "127.0.0.1:0");
        assert_eq!(pairing.addresses, vec!["127.0.0.1:0"]);
    }

    #[tokio::test]
    async fn the_desk_records_how_the_phone_last_reached_it() {
        // Each listen address is its own server, so a verified request says which one it
        // came in on; the interface holding it says home or tunnel. GET /phone shows both.
        let networks = Networks::with(
            || {
                Ok([("127.0.0.1", "enp3s0"), ("127.0.0.2", "wg0")]
                    .into_iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect())
            },
            |name| name == "wg0",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phone.json");
        let host = Arc::new(FakeHost {
            ended: AtomicUsize::new(0),
        });
        let listener = Arc::new(
            PhoneListener::new(
                "127.0.0.1:0,127.0.0.2:0,192.0.2.1:0",
                Arc::new(PhoneStore::new(&path)),
                host,
            )
            .unwrap()
            .with_networks(networks),
        );
        // The LAN and the tunnel bind; 192.0.2.1 is not on this host: retried, not listening.
        listener.set_bound("127.0.0.1:0", true);
        listener.set_bound("127.0.0.2:0", true);
        let view = listener.view();
        let listen: Vec<(String, Option<Network>, bool)> = view
            .listen
            .iter()
            .map(|a| (a.address.clone(), a.network, a.listening))
            .collect();
        assert_eq!(
            listen,
            vec![
                ("127.0.0.1:0".to_string(), Some(Network::Home), true),
                ("127.0.0.2:0".to_string(), Some(Network::Tunnel), true),
                ("192.0.2.1:0".to_string(), None, false),
            ]
        );
        let home = listener
            .router(Some("127.0.0.1:0"), stub_routes())
            .layer(axum::Extension(LocalAddr("127.0.0.1".parse().unwrap())));
        let tunnel = listener
            .router(Some("127.0.0.2:0"), stub_routes())
            .layer(axum::Extension(LocalAddr("127.0.0.2".parse().unwrap())));

        let pairing = listener.new_pairing();
        let r = post_json(
            &home,
            "/pair",
            json!({"code": pairing.code, "device_name": "p"}),
        )
        .await;
        let v = r.json();
        let creds = Creds {
            device_id: v["device_id"].as_str().unwrap().to_string(),
            secret: Secret::parse(v["secret"].as_str().unwrap()).unwrap(),
        };
        assert!(listener.view().device.unwrap().last_seen.is_none()); // /pair is not a reach
        let (r, _) = signed(&home, &creds, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::OK);
        let d = listener.view().device.unwrap();
        assert_eq!(
            (d.last_address.as_deref(), d.last_network),
            (Some("127.0.0.1:0"), Some(Network::Home))
        );
        assert!(d.last_seen.is_some());

        let (r, _) = signed(&tunnel, &creds, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::OK);
        // A refused request reaches nothing.
        let wrong = Creds {
            device_id: creds.device_id.clone(),
            secret: Secret::parse(&"A".repeat(43)).unwrap(),
        };
        let (r, _) = signed(&tunnel, &wrong, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        let d = listener.view().device.unwrap();
        assert_eq!(
            (d.last_address.as_deref(), d.last_network),
            (Some("127.0.0.2:0"), Some(Network::Tunnel))
        );
        // Moving to the tunnel is written at once, not on the minute.
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["last_address"], "127.0.0.2:0");
        assert_eq!(saved["last_network"], "tunnel");
        listener.store.flush();
        let again = PhoneStore::new(&path).device().unwrap();
        assert_eq!(again.last_network, Some(Network::Tunnel));
    }

    #[tokio::test]
    async fn a_wildcard_listen_address_is_told_apart_by_the_local_ip() {
        let networks = Networks::with(
            || {
                Ok([("192.168.1.20", "enp3s0"), ("10.8.0.4", "wg0")]
                    .into_iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect())
            },
            |name| name == "wg0",
        );
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost {
            ended: AtomicUsize::new(0),
        });
        let listener = Arc::new(
            PhoneListener::new(
                "0.0.0.0:8765",
                Arc::new(PhoneStore::new(dir.path().join("phone.json"))),
                host,
            )
            .unwrap()
            .with_networks(networks),
        );
        assert_eq!(listener.view().listen[0].network, None);
        listener.store.pair("p");
        listener.seen(&Reach {
            listen: Some("0.0.0.0:8765".into()),
            local: Some("10.8.0.4".parse().unwrap()),
        });
        let d = listener.store.device().unwrap();
        assert_eq!(d.last_network, Some(Network::Tunnel));
        assert_eq!(d.last_address.as_deref(), Some("0.0.0.0:8765"));
    }
}
