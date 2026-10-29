//! The phone listener's routes and servers (ADR 0016, ADR 0020): the signed routes the
//! listener mounts, one server per configured address, and the two pieces of a phone
//! session the daemon runs beside the media: the events it forwards to the phone and the
//! keepalive it watches.
//!
//! The routes read the daemon through `PhoneHost` (`listener.rs`), and so does an offer:
//! the SDP goes to the host and the answer comes back from it; the media crate builds the
//! peer, and the listener keeps none. Events for the phone end at `PhoneEvents`: the data
//! channel is the media's, and what goes over it is the same event JSON the control API's
//! WebSocket sends, never a log line (docs/phone-protocol.md).
//!
//! The first address in `GHOST_PHONE_LISTEN` must bind at start; a later one whose
//! interface is not up yet (a tunnel that comes up after the daemon) is retried every
//! `REBIND_SECS` until it binds or the listener stops. Each server tags its requests with
//! the listen address and the local IP the connection landed on, so the desk can say how
//! the phone reached it.

use std::future::{Future, IntoFuture};
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, RawQuery, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::serve::IncomingStream;
use ghost_api::models::{Event, Status};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::listener::{
    Candidate, CandidateError, LocalAddr, OfferError, PhoneListener, REBIND_SECS, error,
    parse_listen, reply,
};

/// An offer's SDP may be this long.
pub const MAX_SDP: usize = 65_536;
/// A waiting-list key, as the control API bounds it.
pub const MAX_KEY: usize = 300;
/// A snooze is 1 minute to a week.
pub const MAX_SNOOZE_MINUTES: i64 = 7 * 24 * 60;
/// A conversation slug.
pub const MAX_CONVERSATION: usize = 80;
/// Candidates per `PATCH /offer`.
pub const MAX_CANDIDATES: usize = 64;
/// How long `stop` waits for a server's connections before closing them.
pub const STOP_WAIT: Duration = Duration::from_secs(4);

// --- a phone session, beside the media -----------------------------------------------------

/// No keepalive from the phone for this long ends its session; long enough for a cellular
/// blip or a tunnel re-handshake away from home (ADR 0020).
pub const PHONE_PING_GRACE_SECS: u32 = 15;
/// An accepted offer whose peer hasn't connected by then is dropped.
pub const PHONE_CONNECT_SECS: f64 = 30.0;
/// After the connection: how long to wait for the phone's client-ready. The phone holds
/// it until its audio route has settled (a headset takes up to ~2 s), so this must
/// outlast that.
pub const PHONE_READY_SECS: f64 = 4.0;
/// Let the phone's playback settle before the first word.
pub const PHONE_GREETING_LEAD_SECS: f64 = 0.3;
/// A write to a gone phone fails after this; the desk is free sooner.
pub const PHONE_WRITE_TIMEOUT_SECS: f64 = 0.5;
/// How long the loop waits for a cancelled phone session to drain.
pub const PHONE_WIND_DOWN_SECS: f64 = 3.0;

/// Where the desk's events go for the phone: the media crate's data channel, which sends
/// each as an RTVI `server-message` whose `data` is the event JSON.
#[async_trait]
pub trait PhoneEvents: Send + Sync {
    async fn send_server_message(&self, data: &Value);
}

/// Forward the desk's events to the phone once its data channel says client-ready
/// (`ready`): first what happened before the channel opened, minus log lines and stale
/// statuses, then a fresh `status`, then live events, never a log line. The same JSON the
/// control API's WebSocket sends. Returns when the bus closes.
pub async fn forward_events(
    mut events: broadcast::Receiver<String>,
    ready: impl Future<Output = ()>,
    status: impl Fn() -> Status,
    sink: &dyn PhoneEvents,
) {
    ready.await;
    let mut backlog = Vec::new();
    loop {
        match events.try_recv() {
            Ok(text) => {
                if let Some(data) = event_of(&text)
                    && !matches!(kind(&data), Some("log" | "status"))
                {
                    backlog.push(data);
                }
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    for data in backlog {
        sink.send_server_message(&data).await;
    }
    if let Ok(snapshot) = serde_json::to_value(Event::Status { status: status() }) {
        sink.send_server_message(&snapshot).await;
    }
    loop {
        match events.recv().await {
            Ok(text) => {
                if let Some(data) = event_of(&text)
                    && kind(&data) != Some("log")
                {
                    sink.send_server_message(&data).await;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn event_of(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
}

fn kind(data: &Value) -> Option<&str> {
    data.get("type").and_then(Value::as_str)
}

/// Why the watchdog gave up on the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hangup {
    /// The peer never connected after its offer was accepted.
    NeverConnected,
    /// The phone's keepalive stopped for the grace period.
    KeepaliveLost,
}

/// Notice when the phone is gone: `connected` not within `connect_wait`, or
/// `is_connected` false for `grace` ticks in a row. The daemon's values are
/// `PHONE_CONNECT_SECS`, one second and `PHONE_PING_GRACE_SECS`.
pub async fn watchdog(
    connected: impl Future<Output = ()>,
    is_connected: impl Fn() -> bool,
    connect_wait: Duration,
    tick: Duration,
    grace: u32,
) -> Hangup {
    if tokio::time::timeout(connect_wait, connected).await.is_err() {
        info!("phone: the peer never connected");
        return Hangup::NeverConnected;
    }
    let mut missed = 0;
    while missed < grace {
        tokio::time::sleep(tick).await;
        missed = if is_connected() { 0 } else { missed + 1 };
    }
    Hangup::KeepaliveLost
}

// --- request bodies -------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Key {
    key: String,
}

impl Key {
    fn check(&self) -> bool {
        (1..=MAX_KEY).contains(&self.key.chars().count())
    }
}

#[derive(Debug, Deserialize)]
struct Snooze {
    key: String,
    minutes: i64,
}

impl Snooze {
    fn check(&self) -> bool {
        (1..=MAX_KEY).contains(&self.key.chars().count())
            && (1..=MAX_SNOOZE_MINUTES).contains(&self.minutes)
    }
}

/// `POST /offer`: `{sdp, type: "offer", pc_id?, restart_pc?, conversation?}`.
#[derive(Debug, Clone, Deserialize)]
pub struct OfferRequest {
    pub sdp: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub pc_id: Option<String>,
    pub restart_pc: Option<bool>,
    /// The agent to start with (a slug from `/conversations`); Ghost when absent.
    pub conversation: Option<String>,
}

impl OfferRequest {
    pub fn check(&self) -> bool {
        self.sdp.chars().count() <= MAX_SDP
            && self.kind == "offer"
            && self
                .conversation
                .as_ref()
                .is_none_or(|c| c.chars().count() <= MAX_CONVERSATION)
    }
}

/// `PATCH /offer`: `{pc_id, candidates}`.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchRequest {
    pub pc_id: String,
    pub candidates: Vec<Candidate>,
}

impl PatchRequest {
    pub fn check(&self) -> bool {
        self.candidates.len() <= MAX_CANDIDATES
    }
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// `?limit=N&offset=M`: limit 1 to 500 (50 unsaid), offset 0 or more (0 unsaid); anything
/// else doesn't fit.
fn audit_bounds(query: Option<&str>) -> Option<(i64, i64)> {
    let (mut limit, mut offset) = (50i64, 0i64);
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_encoding::percent_decode_str(v).decode_utf8().ok()?;
        match k {
            "limit" => limit = v.parse().ok()?,
            "offset" => offset = v.parse().ok()?,
            _ => {}
        }
    }
    ((1..=500).contains(&limit) && offset >= 0).then_some((limit, offset))
}

// --- the routes ----------------------------------------------------------------------------

/// The signed routes, over the listener and its host. `PhoneListener::router` adds `/pair`,
/// the fallback and the signing middleware around them.
pub fn routes(listener: Arc<PhoneListener>) -> Router {
    Router::new()
        .route("/hello", get(hello))
        .route("/conversations", get(conversations))
        .route("/audit", get(audit))
        .route("/waiting", get(waiting))
        .route("/waiting/ack", post(acknowledge))
        .route("/waiting/snooze", post(snooze))
        .route("/offer", post(offer).patch(candidates))
        .route("/end", post(end))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(listener)
}

type Listener = State<Arc<PhoneListener>>;

async fn hello(State(l): Listener) -> Response {
    reply(
        StatusCode::OK,
        json!({
            "workstation": l.workstation,
            "addresses": l.addresses,
            "status": l.host.status(),
        }),
    )
}

/// Every thread a call may start in: Ghost first, settled ones left out.
async fn conversations(State(l): Listener) -> Response {
    reply(StatusCode::OK, l.host.agents())
}

async fn audit(State(l): Listener, RawQuery(query): RawQuery) -> Response {
    let Some((limit, offset)) = audit_bounds(query.as_deref()) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    reply(StatusCode::OK, l.host.audit(limit, offset))
}

/// ADR 0029: the same list the desktop shows; reading it is the phone route's delivery
/// (no push, 2026-10-27).
async fn waiting(State(l): Listener) -> Response {
    reply(StatusCode::OK, l.host.waiting_view())
}

async fn acknowledge(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<Key>(&body).filter(Key::check) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    reply(
        StatusCode::OK,
        json!({ "ok": l.host.acknowledge_item(&req.key) }),
    )
}

async fn snooze(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<Snooze>(&body).filter(Snooze::check) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    reply(
        StatusCode::OK,
        json!({ "ok": l.host.snooze_item(&req.key, req.minutes) }),
    )
}

fn offer_error(e: OfferError) -> Response {
    match e {
        OfferError::Refused(r) => error(r.status(), Some(r.code())),
        OfferError::BadOffer => error(StatusCode::BAD_REQUEST, Some("bad_offer")),
        OfferError::NoSuchPeer => error(StatusCode::NOT_FOUND, None),
        OfferError::Internal => error(StatusCode::INTERNAL_SERVER_ERROR, None),
    }
}

/// Without `pc_id` an offer starts a session; with the live peer's it renegotiates.
async fn offer(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<OfferRequest>(&body).filter(OfferRequest::check) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    if let Some(pc_id) = req.pc_id.as_deref().filter(|p| !p.is_empty()) {
        return match l
            .host
            .renegotiate(pc_id, &req.sdp, req.restart_pc.unwrap_or(false))
            .await
        {
            Ok(answer) => reply(StatusCode::OK, answer),
            Err(e) => offer_error(e),
        };
    }
    if let Some(refusal) = l.host.admit_phone() {
        return error(refusal.status(), Some(refusal.code()));
    }
    match l
        .host
        .start_phone(&req.sdp, req.conversation.as_deref())
        .await
    {
        Ok(answer) => {
            info!("phone: offer accepted");
            reply(StatusCode::OK, answer)
        }
        Err(e) => {
            if e == OfferError::BadOffer {
                info!("phone: offer rejected");
            }
            offer_error(e)
        }
    }
}

async fn candidates(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<PatchRequest>(&body).filter(PatchRequest::check) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    match l.host.add_candidates(&req.pc_id, &req.candidates).await {
        Ok(()) => reply(StatusCode::OK, json!({ "ok": true })),
        Err(CandidateError::NoSuchPeer) => error(StatusCode::NOT_FOUND, None),
        Err(CandidateError::BadCandidate) => error(StatusCode::BAD_REQUEST, Some("bad_candidate")),
    }
}

async fn end(State(l): Listener) -> Response {
    l.host.end_phone().await;
    reply(StatusCode::OK, json!({ "ok": true }))
}

async fn method_not_allowed() -> Response {
    error(StatusCode::METHOD_NOT_ALLOWED, None)
}

// --- the servers ---------------------------------------------------------------------------

/// The local IP each accepted connection landed on, for the signing middleware: a wildcard
/// listen address (`0.0.0.0:8765`) still tells home from tunnel.
impl Connected<IncomingStream<'_, TcpListener>> for LocalAddr {
    fn connect_info(target: IncomingStream<'_, TcpListener>) -> Self {
        LocalAddr(
            target
                .io()
                .local_addr()
                .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |a| a.ip()),
        )
    }
}

/// The connection's local IP, as the middleware reads it (`LocalAddr` in the extensions).
async fn tag_local(mut req: Request, next: Next) -> Response {
    if let Some(ConnectInfo(local)) = req.extensions().get::<ConnectInfo<LocalAddr>>().copied() {
        req.extensions_mut().insert(local);
    }
    next.run(req).await
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Server {
    address: String,
    port: u16,
    task: JoinHandle<io::Result<()>>,
}

/// One server per configured address, serving the one signed app.
pub struct PhoneServers {
    listener: Arc<PhoneListener>,
    servers: Arc<Mutex<Vec<Server>>>,
    rebind: Mutex<Option<JoinHandle<()>>>,
    stop: watch::Sender<bool>,
}

impl PhoneServers {
    /// Bind every configured address and serve. The first address must bind, or this
    /// fails. A later one whose interface isn't up yet (a tunnel that comes up after the
    /// daemon) is retried every `REBIND_SECS` until it binds or the listener stops.
    pub async fn start(listener: Arc<PhoneListener>) -> io::Result<Self> {
        let (stop, _) = watch::channel(false);
        let servers = Arc::new(Mutex::new(Vec::new()));
        let mut addresses = listener.addresses.iter();
        let Some(first) = addresses.next() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "GHOST_PHONE_LISTEN is empty",
            ));
        };
        let server = serve_one(&listener, first, stop.subscribe()).await?;
        lock(&servers).push(server);
        let mut late = Vec::new();
        for address in addresses {
            match serve_one(&listener, address, stop.subscribe()).await {
                Ok(server) => lock(&servers).push(server),
                Err(e) => {
                    warn!("phone: {address} not bound yet ({e}); retrying");
                    late.push(address.clone());
                }
            }
        }
        let rebind = (!late.is_empty()).then(|| {
            tokio::spawn(rebind_loop(
                Arc::clone(&listener),
                Arc::clone(&servers),
                late,
                stop.subscribe(),
            ))
        });
        Ok(Self {
            listener,
            servers,
            rebind: Mutex::new(rebind),
            stop,
        })
    }

    /// The addresses serving right now, in the order they bound (a late one is missing
    /// until it binds).
    pub fn bound(&self) -> Vec<String> {
        lock(&self.servers)
            .iter()
            .map(|s| s.address.clone())
            .collect()
    }

    /// The bound ports, in the same order (useful when a configured port is 0, in tests).
    pub fn ports(&self) -> Vec<u16> {
        lock(&self.servers).iter().map(|s| s.port).collect()
    }

    /// The first bound port.
    pub fn port(&self) -> u16 {
        self.ports().first().copied().unwrap_or(0)
    }

    /// A late address is still being retried.
    pub fn rebinding(&self) -> bool {
        lock(&self.rebind)
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    /// Stop retrying, stop every server (connections still open after `STOP_WAIT` are
    /// closed), and flush the paired phone's file.
    pub async fn stop(&self) {
        let rebind = lock(&self.rebind).take();
        if let Some(task) = rebind {
            task.abort();
            let _ = task.await;
        }
        self.stop.send_replace(true);
        let servers: Vec<Server> = std::mem::take(&mut *lock(&self.servers));
        for server in servers {
            let abort = server.task.abort_handle();
            if tokio::time::timeout(STOP_WAIT, server.task).await.is_err() {
                warn!("phone: connections still open at shutdown; closing them");
                abort.abort();
            }
            self.listener.set_bound(&server.address, false);
        }
        self.listener.store.flush();
    }
}

/// Bind `address` and serve the app on it, tagging each request with the address and the
/// local IP its connection landed on.
async fn serve_one(
    listener: &Arc<PhoneListener>,
    address: &str,
    mut stop: watch::Receiver<bool>,
) -> io::Result<Server> {
    let (host, port) = parse_listen(address)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let socket = TcpListener::bind((host.as_str(), port)).await?;
    let port = socket.local_addr()?.port();
    let app = listener
        .router(Some(address), routes(Arc::clone(listener)))
        .layer(axum::middleware::from_fn(tag_local));
    let serve = axum::serve(
        socket,
        app.into_make_service_with_connect_info::<LocalAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stop.wait_for(|s| *s).await;
    });
    let task = tokio::spawn(serve.into_future());
    listener.set_bound(address, true);
    info!("phone: listening on {address}");
    Ok(Server {
        address: address.to_string(),
        port,
        task,
    })
}

async fn rebind_loop(
    listener: Arc<PhoneListener>,
    servers: Arc<Mutex<Vec<Server>>>,
    mut late: Vec<String>,
    mut stop: watch::Receiver<bool>,
) {
    while !late.is_empty() {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs_f64(REBIND_SECS)) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
        let mut still = Vec::new();
        for address in late {
            match serve_one(&listener, &address, stop.clone()).await {
                Ok(server) => lock(&servers).push(server),
                Err(_) => still.push(address),
            }
        }
        late = still;
    }
}

#[cfg(test)]
mod tests {
    //! The phone listener's routes (ADR 0016, docs/phone-protocol.md): admission, the
    //! agents, the waiting list, the offer's answer, and the servers on every address.

    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::http::HeaderMap;
    use ghost_api::models::{API_VERSION, AuditPage, Build, ConversationView, Phase, WaitingView};
    use tower::ServiceExt;

    use super::super::listener::{Answer, PhoneHost, Refusal};
    use super::super::pairing::{PairingCodes, PhoneStore};
    use super::super::signing::{
        H_SIGNATURE, H_TIME, Secret, new_nonce, signed_headers, verify_response,
    };
    use super::*;

    const PC_ID: &str = "pc-1";

    #[derive(Default)]
    struct FakeHost {
        refusal: Mutex<Option<Refusal>>,
        started: Mutex<Vec<(String, Option<String>)>>,
        ended: AtomicUsize,
        fetched: AtomicUsize,
        acked: Mutex<Vec<String>>,
        snoozed: Mutex<Option<(String, i64)>>,
    }

    impl FakeHost {
        fn refuse(&self, r: Option<Refusal>) {
            *lock(&self.refusal) = r;
        }
    }

    #[async_trait]
    impl PhoneHost for FakeHost {
        fn status(&self) -> Status {
            Status {
                phase: Phase::Passive,
                session_id: None,
                models_ready: true,
                uptime_secs: 1.0,
                version: "test".into(),
                build: Build::default(),
                api_version: API_VERSION.into(),
                wake_mode: "file".into(),
                apps: vec!["fake".into()],
                pending: None,
                conversation: None,
                session_origin: None,
                ending: false,
                waiting: 0,
            }
        }

        fn audit(&self, limit: i64, offset: i64) -> AuditPage {
            AuditPage {
                items: Vec::new(),
                total: 0,
                limit,
                offset,
            }
        }

        fn waiting_view(&self) -> WaitingView {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            WaitingView {
                items: Vec::new(),
                count: 0,
            }
        }

        fn acknowledge_item(&self, key: &str) -> bool {
            lock(&self.acked).push(key.to_string());
            key == "eng:ci:1"
        }

        fn snooze_item(&self, key: &str, minutes: i64) -> bool {
            *lock(&self.snoozed) = Some((key.to_string(), minutes));
            true
        }

        fn agents(&self) -> Vec<ConversationView> {
            Vec::new()
        }

        fn admit_phone(&self) -> Option<Refusal> {
            *lock(&self.refusal)
        }

        async fn start_phone(
            &self,
            sdp: &str,
            conversation: Option<&str>,
        ) -> Result<Answer, OfferError> {
            if let Some(r) = *lock(&self.refusal) {
                return Err(OfferError::Refused(r));
            }
            lock(&self.started).push((sdp.to_string(), conversation.map(String::from)));
            Ok(Answer::new(
                "v=0\r\na=fingerprint:sha-256 AA:BB:CC\r\n",
                PC_ID,
            ))
        }

        async fn renegotiate(
            &self,
            pc_id: &str,
            sdp: &str,
            _restart_pc: bool,
        ) -> Result<Answer, OfferError> {
            if pc_id != PC_ID {
                return Err(OfferError::NoSuchPeer);
            }
            Ok(Answer::new(format!("{sdp}\r\na=renegotiated"), PC_ID))
        }

        async fn add_candidates(
            &self,
            pc_id: &str,
            candidates: &[Candidate],
        ) -> Result<(), CandidateError> {
            if pc_id != PC_ID {
                return Err(CandidateError::NoSuchPeer);
            }
            if candidates.iter().any(|c| c.candidate == "candidate:junk") {
                return Err(CandidateError::BadCandidate);
            }
            Ok(())
        }

        async fn end_phone(&self) -> bool {
            self.ended.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    struct Phone {
        listener: Arc<PhoneListener>,
        host: Arc<FakeHost>,
        app: Router,
        _dir: tempfile::TempDir,
    }

    fn phone_at(address: &str) -> Phone {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::default());
        let store = Arc::new(PhoneStore::new(dir.path().join("phone.json")));
        let listener = Arc::new(
            PhoneListener::new(address, store, host.clone())
                .unwrap()
                .with_codes(PairingCodes::new())
                .with_workstation("desk"),
        );
        let app = listener.router(None, routes(Arc::clone(&listener)));
        Phone {
            listener,
            host,
            app,
            _dir: dir,
        }
    }

    fn phone() -> Phone {
        phone_at("127.0.0.1:0")
    }

    #[derive(Clone)]
    struct Creds {
        device_id: String,
        secret: Secret,
    }

    struct Reply {
        status: StatusCode,
        headers: HeaderMap,
        body: Bytes,
    }

    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn response_ok(r: &Reply, creds: &Creds, nonce: &str) -> bool {
        let ts = r.headers.get(H_TIME).and_then(|v| v.to_str().ok());
        let sig = r.headers.get(H_SIGNATURE).and_then(|v| v.to_str().ok());
        let (Some(ts), Some(sig)) = (ts, sig) else {
            return false;
        };
        verify_response(&creds.secret, r.status.as_u16(), nonce, ts, &r.body, sig)
    }

    // --- in process, through the router ------------------------------------------------

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

    async fn pair(p: &Phone) -> Creds {
        let pairing = p.listener.new_pairing();
        let body = json!({"code": pairing.code, "device_name": "Pixel 9"});
        let req = Request::builder()
            .method("POST")
            .uri("/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let r = send(&p.app, req).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
        let v = r.json();
        Creds {
            device_id: v["device_id"].as_str().unwrap().to_string(),
            secret: Secret::parse(v["secret"].as_str().unwrap()).unwrap(),
        }
    }

    async fn signed(
        app: &Router,
        creds: &Creds,
        method: &str,
        path: &str,
        body: Option<Value>,
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
            None,
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

    // --- over a real socket ----------------------------------------------------------

    async fn http_pair(base: &str, listener: &PhoneListener) -> Creds {
        let pairing = listener.new_pairing();
        let v: Value = reqwest::Client::new()
            .post(format!("{base}/pair"))
            .json(&json!({"code": pairing.code, "device_name": "p"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        Creds {
            device_id: v["device_id"].as_str().unwrap().to_string(),
            secret: Secret::parse(v["secret"].as_str().unwrap()).unwrap(),
        }
    }

    async fn http_signed(
        base: &str,
        creds: &Creds,
        method: &str,
        path: &str,
        body: Option<Value>,
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
            None,
            None,
        );
        let mut req =
            reqwest::Client::new().request(method.parse().unwrap(), format!("{base}{path}"));
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        if !raw.is_empty() {
            req = req.header("content-type", "application/json");
        }
        let resp = req.body(raw).send().await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.unwrap();
        (
            Reply {
                status,
                headers,
                body,
            },
            h.nonce,
        )
    }

    // --- offers -----------------------------------------------------------------------

    #[tokio::test]
    async fn offer_refused_while_busy_or_not_ready() {
        let p = phone();
        let creds = pair(&p).await;
        for (refusal, status) in [
            (Refusal::Busy, StatusCode::CONFLICT),
            (Refusal::NotReady, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            p.host.refuse(Some(refusal));
            let (r, nonce) = signed(
                &p.app,
                &creds,
                "POST",
                "/offer",
                Some(json!({"sdp": "v=0", "type": "offer"})),
            )
            .await;
            assert_eq!(r.status, status);
            assert_eq!(r.json(), json!({"error": refusal.code()}));
            assert!(response_ok(&r, &creds, &nonce));
        }
        assert!(lock(&p.host.started).is_empty());
    }

    #[tokio::test]
    async fn agents_are_served_and_a_call_can_start_with_one() {
        // ADR 0014 on the phone: the listener lists the agents and an offer may name one.
        let p = phone();
        let creds = pair(&p).await;
        let (r, nonce) = signed(&p.app, &creds, "GET", "/conversations", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(response_ok(&r, &creds, &nonce));
        assert_eq!(r.json(), json!([]));
        let body = json!({"sdp": "v=0", "type": "offer", "conversation": "heka-workshop"});
        let (r, _) = signed(&p.app, &creds, "POST", "/offer", Some(body.clone())).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(
            lock(&p.host.started).last().unwrap().1.as_deref(),
            Some("heka-workshop")
        );
        p.host.refuse(Some(Refusal::NoSuchLine));
        let (r, _) = signed(&p.app, &creds, "POST", "/offer", Some(body)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert_eq!(r.json(), json!({"error": "no_such_line"}));
    }

    #[tokio::test]
    async fn end_asks_the_host() {
        let p = phone();
        let creds = pair(&p).await;
        let (r, nonce) = signed(&p.app, &creds, "POST", "/end", None).await;
        assert_eq!(r.json(), json!({"ok": true}));
        assert_eq!(p.host.ended.load(Ordering::SeqCst), 1);
        assert!(response_ok(&r, &creds, &nonce));
    }

    fn cand(text: &str) -> Value {
        json!({"candidate": text, "sdp_mid": "0", "sdp_mline_index": 0})
    }

    #[tokio::test]
    async fn offer_returns_a_signed_answer_the_client_can_apply() {
        // The peer itself is the media crate's; here the host stands in for it and the
        // signalling around it is what is checked.
        let p = phone();
        let creds = pair(&p).await;
        let (r, nonce) = signed(
            &p.app,
            &creds,
            "POST",
            "/offer",
            Some(json!({"sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1", "type": "offer"})),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
        assert!(response_ok(&r, &creds, &nonce)); // the phone checks this before applying it
        let answer = r.json();
        assert_eq!(answer["type"], "answer");
        assert_eq!(answer["pc_id"], PC_ID);
        assert!(answer["sdp"].as_str().unwrap().contains("a=fingerprint"));
        assert_eq!(lock(&p.host.started).len(), 1);

        // Trickle ICE for the live peer; an unknown pc_id is not found.
        let (r, _) = signed(
            &p.app,
            &creds,
            "PATCH",
            "/offer",
            Some(json!({"pc_id": PC_ID, "candidates": []})),
        )
        .await;
        assert_eq!(r.json(), json!({"ok": true}));
        let host_cand = "candidate:1 1 udp 2130706431 127.0.0.1 9 typ host";
        let patch = json!({"pc_id": PC_ID, "candidates": [cand(host_cand), cand("")]});
        let (r, nonce) = signed(&p.app, &creds, "PATCH", "/offer", Some(patch)).await;
        assert_eq!(r.json(), json!({"ok": true}));
        assert!(response_ok(&r, &creds, &nonce));
        let patch = json!({"pc_id": PC_ID, "candidates": [cand("candidate:junk")]});
        let (r, _) = signed(&p.app, &creds, "PATCH", "/offer", Some(patch)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.json(), json!({"error": "bad_candidate"}));
        let patch = json!({"pc_id": "nope", "candidates": []});
        let (r, _) = signed(&p.app, &creds, "PATCH", "/offer", Some(patch)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);

        // An offer with the live pc_id renegotiates; with another it is not found.
        let again = json!({"sdp": "v=0", "type": "offer", "pc_id": PC_ID, "restart_pc": true});
        let (r, _) = signed(&p.app, &creds, "POST", "/offer", Some(again)).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.json()["sdp"].as_str().unwrap().contains("renegotiated"));
        assert_eq!(lock(&p.host.started).len(), 1); // not a second start
        let lost = json!({"sdp": "v=0", "type": "offer", "pc_id": "nope"});
        let (r, _) = signed(&p.app, &creds, "POST", "/offer", Some(lost)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);

        // A body outside its bounds doesn't fit.
        let big = json!({"sdp": "x".repeat(MAX_SDP + 1), "type": "offer"});
        let (r, _) = signed(&p.app, &creds, "POST", "/offer", Some(big)).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        let (r, _) = signed(&p.app, &creds, "GET", "/audit?limit=0", None).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(r.json(), json!({"error": "bad_request"}));
        let (r, _) = signed(&p.app, &creds, "GET", "/end", None).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(r.json(), json!({"error": "method_not_allowed"}));
    }

    #[tokio::test]
    async fn the_waiting_list_is_signed_like_everything_else() {
        // ADR 0029: the phone reads the same list and acknowledges or snoozes from it.
        let p = phone();
        let creds = pair(&p).await;
        let (r, nonce) = signed(&p.app, &creds, "GET", "/waiting", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json(), json!({"items": [], "count": 0}));
        assert!(response_ok(&r, &creds, &nonce));
        assert_eq!(p.host.fetched.load(Ordering::SeqCst), 1);
        let (r, _) = signed(
            &p.app,
            &creds,
            "POST",
            "/waiting/ack",
            Some(json!({"key": "eng:ci:1"})),
        )
        .await;
        assert_eq!(r.json(), json!({"ok": true}));
        assert_eq!(*lock(&p.host.acked), vec!["eng:ci:1"]);
        let (r, _) = signed(
            &p.app,
            &creds,
            "POST",
            "/waiting/snooze",
            Some(json!({"key": "eng:ci:1", "minutes": 30})),
        )
        .await;
        assert_eq!(r.json(), json!({"ok": true}));
        assert_eq!(*lock(&p.host.snoozed), Some(("eng:ci:1".to_string(), 30)));
        let (r, _) = signed(
            &p.app,
            &creds,
            "POST",
            "/waiting/snooze",
            Some(json!({"key": "eng:ci:1", "minutes": 0})),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
        // unsigned
        let r = send(
            &p.app,
            Request::builder()
                .uri("/waiting")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }

    // --- the servers ---------------------------------------------------------------------

    #[tokio::test]
    async fn listener_binds_its_address_and_signs_over_real_http() {
        let p = phone();
        let servers = PhoneServers::start(Arc::clone(&p.listener)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", servers.port());
        let creds = http_pair(&base, &p.listener).await;
        let (r, nonce) = http_signed(&base, &creds, "GET", "/audit?limit=2", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json()["limit"], 2);
        assert!(response_ok(&r, &creds, &nonce));
        // no control API here
        let r = reqwest::Client::new()
            .get(format!("{base}/status"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        servers.stop().await;
        assert!(servers.bound().is_empty());
        assert!(p.listener.bound().is_empty());
    }

    #[tokio::test]
    async fn listener_serves_every_address_and_tells_the_phone_all_of_them() {
        // Two addresses (ADR 0020): both serve the same signed app, and /pair and /hello
        // carry the whole list so the phone can pick the one that answers wherever it is.
        let p = phone_at("127.0.0.1:0,127.0.0.1:0");
        let servers = PhoneServers::start(Arc::clone(&p.listener)).await.unwrap();
        assert_eq!(servers.bound().len(), 2);
        let ports = servers.ports();
        assert_ne!(ports[0], ports[1]);
        let pairing = p.listener.new_pairing();
        assert_eq!(pairing.addresses, vec!["127.0.0.1:0", "127.0.0.1:0"]);
        let v: Value = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/pair", ports[0]))
            .json(&json!({"code": pairing.code, "device_name": "p"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(v["addresses"], json!(["127.0.0.1:0", "127.0.0.1:0"]));
        let creds = Creds {
            device_id: v["device_id"].as_str().unwrap().to_string(),
            secret: Secret::parse(v["secret"].as_str().unwrap()).unwrap(),
        };
        let base = format!("http://127.0.0.1:{}", ports[1]);
        let (r, nonce) = http_signed(&base, &creds, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(response_ok(&r, &creds, &nonce));
        assert_eq!(r.json()["addresses"], json!(["127.0.0.1:0", "127.0.0.1:0"]));
        assert_eq!(r.json()["workstation"], "desk");
        assert_eq!(r.json()["status"]["apps"], json!(["fake"]));
        assert_eq!(
            p.listener.view().addresses,
            vec!["127.0.0.1:0", "127.0.0.1:0"]
        );
        servers.stop().await;
    }

    #[tokio::test]
    async fn listener_keeps_trying_an_address_whose_interface_is_not_up() {
        // The tunnel can come up after the daemon: its address is retried, not given up
        // on, while the LAN address serves from the start.
        let p = phone_at("127.0.0.1:0,192.0.2.1:0"); // 192.0.2.1 (TEST-NET) is not here
        let servers = PhoneServers::start(Arc::clone(&p.listener)).await.unwrap();
        assert_eq!(servers.bound(), vec!["127.0.0.1:0"]);
        assert_eq!(p.listener.bound(), vec!["127.0.0.1:0"]);
        assert_eq!(p.listener.addresses, vec!["127.0.0.1:0", "192.0.2.1:0"]);
        assert!(servers.rebinding());
        let base = format!("http://127.0.0.1:{}", servers.port());
        let creds = http_pair(&base, &p.listener).await;
        assert!(!creds.device_id.is_empty());
        servers.stop().await;
        assert!(!servers.rebinding());
        const { assert!(REBIND_SECS > 0.0) };

        // The first address has to bind.
        let p = phone_at("192.0.2.1:0");
        assert!(PhoneServers::start(Arc::clone(&p.listener)).await.is_err());
    }

    #[tokio::test]
    async fn a_request_is_signed_for_the_path_and_query_it_took() {
        // Over a real socket the signed path is exactly what the server sees.
        let p = phone();
        let servers = PhoneServers::start(Arc::clone(&p.listener)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", servers.port());
        let creds = http_pair(&base, &p.listener).await;
        let (r, _) = http_signed(&base, &creds, "GET", "/hello", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(!response_ok(&r, &creds, &new_nonce()));
        let d = p.listener.view().device.unwrap();
        assert_eq!(d.last_address.as_deref(), Some("127.0.0.1:0"));
        servers.stop().await;
    }

    // --- beside the media ------------------------------------------------------------

    #[derive(Default)]
    struct Sink {
        sent: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl PhoneEvents for Sink {
        async fn send_server_message(&self, data: &Value) {
            lock(&self.sent).push(data.clone());
        }
    }

    #[tokio::test]
    async fn events_reach_the_phone_after_client_ready_without_logs_or_stale_statuses() {
        let (tx, rx) = broadcast::channel::<String>(16);
        for e in [
            json!({"type": "status", "status": {"old": true}}),
            json!({"type": "turn", "turn": {"n": 1}}),
            json!({"type": "log", "level": "info", "message": "x", "at": "2026-10-28T00:00:00Z"}),
            json!({"type": "pending", "pending": null}),
        ] {
            tx.send(e.to_string()).unwrap();
        }
        let sink = Sink::default();
        let host = FakeHost::default();
        let forward = forward_events(rx, std::future::ready(()), || host.status(), &sink);
        let live = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(json!({"type": "log", "level": "info", "message": "y", "at": "2026-10-28T00:00:00Z"}).to_string()).unwrap();
            tx.send(json!({"type": "waiting", "count": 2}).to_string())
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(tx);
        };
        tokio::join!(forward, live);
        let kinds: Vec<String> = lock(&sink.sent)
            .iter()
            .map(|v| v["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["turn", "pending", "status", "waiting"]);
        let sent = lock(&sink.sent);
        assert_eq!(sent[2]["status"]["apps"], json!(["fake"])); // a fresh one, not the old
    }

    #[tokio::test]
    async fn the_watchdog_notices_a_peer_that_never_connects_or_stops_pinging() {
        let never = watchdog(
            std::future::pending(),
            || true,
            Duration::from_millis(20),
            Duration::from_millis(1),
            3,
        )
        .await;
        assert_eq!(never, Hangup::NeverConnected);
        let pings = AtomicUsize::new(0);
        let lost = watchdog(
            std::future::ready(()),
            || pings.fetch_add(1, Ordering::SeqCst) < 2, // two pings, then silence
            Duration::from_millis(20),
            Duration::from_millis(1),
            3,
        )
        .await;
        assert_eq!(lost, Hangup::KeepaliveLost);
        assert_eq!(pings.load(Ordering::SeqCst), 5); // 2 heard, then 3 missed in a row
    }
}
