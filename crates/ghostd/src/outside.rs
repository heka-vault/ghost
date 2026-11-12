//! The listener for outside tools (ADR 0048): a fourth app in `ghostd`, serving Ghost's
//! own app (`ghost_core::ghost_app`) to the tools on this computer the operator connected.
//!
//! It binds its own addresses (`GHOST_MCP_LISTEN`, `127.0.0.1:47824` unless set; empty
//! turns it off) and shares no route with the control API, the phone listener or the
//! webhook listener: one path, `/mcp`, over rmcp's Streamable HTTP service, which serves
//! the session-based clients and the stateless ones alike. Before a request reaches it:
//!
//! - a request with an `Origin` header is refused. A browser sends one and a tool does
//!   not, so a page that reaches this port by DNS rebinding gets nothing; rmcp's own
//!   `Host` allow-list is kept beside it;
//! - it must carry a client's token (`ghost_core::ghost_app::clients`). No token, a wrong
//!   one, a removed client: 401. Five wrong ones for a name: 429 for five minutes.
//!
//! The client's name rides in the request's extensions to the server, which names the
//! caller from that and nothing else. This is not the control API: nothing here lists a
//! password, reads the policy file, or changes a permission, an app or a client.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use ghost_core::conversations::models::Profile;
use ghost_core::ghost_app::clients::{AuthError, CLIENT_SOURCE, ClientId, Clients, shown};
use ghost_core::ghost_app::outside::{AgentRunner, DeskVoice, Feed, Outside, RequestGone, Said};
use ghost_core::ghost_app::{AGENT_SOURCE, GhostServer};
use ghost_core::routes::{Delivery, RouteName, RoutePolicy};
use ghost_core::schedules::{ASK_WAIT, AgentTurns, DEFAULT_MAX_RUNTIME_SECS, DEFAULT_MAX_TURNS, Ended};
use ghost_core::store::Item;
use ghost_core::time::utcnow;
use ghost_core::waiting::WaitingList;
use ghost_voice::headless::{Asked, HeadlessRunner};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::daemon::Daemon;
use crate::phone::listener::parse_listen;
use crate::routes::{DELIVER_TIMEOUT_SECS, Raised};

/// The one path.
pub const PATH: &str = "/mcp";
/// How long `stop` waits for open requests (an `events_wait` may be one) before closing.
pub const STOP_WAIT: Duration = Duration::from_secs(2);
/// The waiting list's key for a listener that did not start.
pub const PROBLEM_KEY: &str = "ghost:outside-listener";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn refuse(status: StatusCode, error: &str) -> Response {
    let payload = serde_json::to_vec(&json!({ "error": error })).unwrap_or_default();
    let mut resp = Response::new(Body::from(payload));
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// The bus's events the feed reads; a turn, a log line or the voice's level is not even
/// parsed.
const FED: [&str; 5] = [
    r#"{"type":"waiting""#,
    r#"{"type":"pending""#,
    r#"{"type":"outcome""#,
    r#"{"type":"status""#,
    // A question asked or settled (ADR 0051): its asker reads the answer.
    r#"{"type":"question""#,
];

/// Every request, before the server sees it: no browser, and a client's token.
async fn guard(
    State(clients): State<Arc<Clients>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return refuse(StatusCode::FORBIDDEN, "origin_not_allowed");
    }
    let authorization = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // The check reads the store: off the listener's thread.
    let checked = {
        let clients = clients.clone();
        let peer = peer.ip().to_string();
        tokio::task::spawn_blocking(move || clients.authenticate(authorization.as_deref(), &peer))
            .await
    };
    match checked {
        Ok(Ok(name)) => {
            req.extensions_mut().insert(ClientId(name));
            // The response's body going away before its end is the tool closing the
            // connection: a change it asked for and still running is stopped
            // (`RequestGone`). A body that ended normally cancels it too, by then harmlessly.
            let gone = CancellationToken::new();
            req.extensions_mut().insert(RequestGone(gone.clone()));
            let resp = next.run(req).await;
            let guard = gone.drop_guard();
            resp.map(|body| {
                Body::from_stream(body.into_data_stream().map(move |chunk| {
                    let _held = &guard;
                    chunk
                }))
            })
        }
        Ok(Err(AuthError::Locked { retry_after })) => {
            let mut resp = refuse(StatusCode::TOO_MANY_REQUESTS, "locked_out");
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
            resp
        }
        Ok(Err(AuthError::Missing | AuthError::Bad)) => {
            let mut resp = refuse(StatusCode::UNAUTHORIZED, "unauthorized");
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            resp
        }
        Err(_) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

/// The app: `/mcp` behind the guard, and a 404 for everything else.
pub fn outside_app(
    server: GhostServer,
    clients: Arc<Clients>,
    hosts: Vec<String>,
    stop: CancellationToken,
) -> Router {
    let service: StreamableHttpService<GhostServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts(hosts)
                .enforce_origin_validation()
                .with_cancellation_token(stop),
        );
    Router::new()
        .nest_service(PATH, service)
        .layer(middleware::from_fn_with_state(clients, guard))
        .fallback(|| async { refuse(StatusCode::NOT_FOUND, "not_found") })
}

/// Whether an address is this computer only.
pub fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutsideListenError {
    #[error("GHOST_MCP_LISTEN must be host:port, got '{0}'")]
    NotHostPort(String),
}

/// `GHOST_MCP_LISTEN`: `host:port`, comma separated; empty is off.
pub fn parse_addresses(listen: &str) -> Result<Vec<String>, OutsideListenError> {
    let addresses: Vec<String> = listen
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect();
    for a in &addresses {
        parse_listen(a).map_err(|_| OutsideListenError::NotHostPort(a.clone()))?;
    }
    Ok(addresses)
}

struct Server {
    address: String,
    port: u16,
    task: JoinHandle<io::Result<()>>,
}

/// The app on every address in `GHOST_MCP_LISTEN`.
pub struct OutsideListener {
    pub addresses: Vec<String>,
    app: Router,
    servers: Mutex<Vec<Server>>,
    stop: CancellationToken,
}

impl OutsideListener {
    pub fn new(
        listen: &str,
        server: GhostServer,
        clients: Arc<Clients>,
    ) -> Result<Self, OutsideListenError> {
        let addresses = parse_addresses(listen)?;
        // rmcp's allow-list: this computer's names, and any other host the operator set.
        let mut hosts: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
        for a in &addresses {
            if let Ok((host, _)) = parse_listen(a)
                && !hosts.contains(&host)
            {
                hosts.push(host);
            }
        }
        let stop = CancellationToken::new();
        Ok(Self {
            addresses,
            app: outside_app(server, clients, hosts, stop.child_token()),
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
            let stop = self.stop.clone();
            let serve = axum::serve(
                socket,
                self.app
                    .clone()
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move { stop.cancelled().await });
            let task = tokio::spawn(std::future::IntoFuture::into_future(serve));
            lock(&self.servers).push(Server {
                address: address.clone(),
                port,
                task,
            });
            if is_loopback(&host) {
                info!("outside tools: listening on {address}");
            } else {
                warn!(
                    "outside tools: listening on {address}, which is not this computer only \
                     (GHOST_MCP_LISTEN); a client's token crosses that network unencrypted"
                );
            }
        }
        Ok(())
    }

    pub fn bound(&self) -> Vec<String> {
        lock(&self.servers)
            .iter()
            .map(|s| s.address.clone())
            .collect()
    }

    /// The first bound port (useful when a configured port is 0, in tests).
    pub fn port(&self) -> u16 {
        lock(&self.servers).first().map_or(0, |s| s.port)
    }

    pub async fn stop(&self) {
        self.stop.cancel();
        let servers: Vec<Server> = std::mem::take(&mut *lock(&self.servers));
        for server in servers {
            let abort = server.task.abort_handle();
            if tokio::time::timeout(STOP_WAIT, server.task).await.is_err() {
                abort.abort(); // a client still waiting on events_wait
            }
        }
    }
}

// --- a written turn with an agent ---------------------------------------------------------------

/// `agent_ask`, through the run with nobody there (ADR 0047): the agent's own prompt,
/// memory and tools, the same gate, no audio. A change that would ask is set aside on the
/// waiting list, since nothing an outside tool writes is the operator's yes. The turn is
/// kept in the agent's transcript as asked by that tool, bounded as a scheduled run is
/// (30 model calls, ten minutes), and stopped when the request it was made for is
/// cancelled or dropped, or when Ghost stops.
pub struct AskRunner {
    runner: Arc<HeadlessRunner>,
    /// Cancelled when the daemon stops.
    stop: CancellationToken,
    max_turns: u32,
    max_runtime: Duration,
    /// One piece of work at a time per agent, shared with the scheduler: an ask waits for
    /// a run of the same agent to finish, and goes before the next one.
    turns: Option<Arc<AgentTurns>>,
    ask_wait: Duration,
}

impl AskRunner {
    pub fn new(runner: Arc<HeadlessRunner>, stop: CancellationToken) -> Self {
        Self {
            runner,
            stop,
            max_turns: DEFAULT_MAX_TURNS,
            max_runtime: Duration::from_secs(DEFAULT_MAX_RUNTIME_SECS),
            turns: None,
            ask_wait: ASK_WAIT,
        }
    }

    /// Take turns with the scheduler's runs (`Jobs::turns`), waiting `wait` at the most.
    pub fn with_turns(mut self, turns: Arc<AgentTurns>, wait: Duration) -> Self {
        self.turns = Some(turns);
        self.ask_wait = wait;
        self
    }

    pub fn with_limits(mut self, max_turns: u32, max_runtime: Duration) -> Self {
        self.max_turns = max_turns;
        self.max_runtime = max_runtime;
        self
    }
}

#[async_trait]
impl AgentRunner for AskRunner {
    async fn ask(&self, agent: &Profile, text: &str, from: &str) -> Result<String, String> {
        // Who asks: an outside tool (`client:<name>`, ADR 0048) or another of the
        // operator's agents (`agent:<slug>`, ADR 0052's `ask_agent`). The run's id says
        // which, so recall can tell an outside tool's own turns from everyone else's:
        // `ask-<client>-<32 hex>`, or `agentask-<32 hex>`.
        let run = uuid::Uuid::new_v4().simple().to_string();
        let (id, who, by_agent) = match from.strip_prefix(AGENT_SOURCE) {
            Some(slug) => {
                let name = self.runner.agent_name(slug).unwrap_or_else(|| shown(slug));
                (format!("agentask-{run}"), name, true)
            }
            None => {
                let client = from.strip_prefix(CLIENT_SOURCE).unwrap_or(from);
                (format!("ask-{client}-{run}"), shown(client), false)
            }
        };
        let cancel = self.stop.child_token();
        // The request this turn is for (`ghost_app::outside` runs a change under it).
        let request = ghost_core::apps::current_cancel();
        // The agent's turn: after a scheduled run of it that is going, before the next.
        let turn = match &self.turns {
            None => None,
            Some(turns) => {
                let waiting = turns.ask(&agent.slug, self.ask_wait);
                let got = match &request {
                    Some(request) => tokio::select! {
                        got = waiting => got,
                        () = request.cancelled() => {
                            return Err(format!("stopped before {} answered", agent.name));
                        }
                    },
                    None => waiting.await,
                };
                match got {
                    Ok(turn) => Some(turn),
                    Err(busy) => {
                        return Err(format!(
                            "{} is busy with other work and was still busy after {} seconds; \
                             ask again in a few minutes",
                            agent.name,
                            busy.waited.as_secs()
                        ));
                    }
                }
            }
        };
        let asked = Asked {
            id,
            agent: agent.slug.clone(),
            from: who,
            by_agent,
            text: text.to_string(),
            max_turns: self.max_turns,
            max_runtime: self.max_runtime,
            cancel: cancel.clone(),
        };
        // Its own task, stopped through its token and never dropped where it stands: a
        // change the agent has in the gate still gets its outcome.
        let runner = self.runner.clone();
        // The turn goes with the run, so the agent is free only once the run has ended,
        // even when the request that asked for it is gone first.
        let run = tokio::spawn(async move {
            let out = runner.ask(asked).await;
            drop(turn);
            out
        });
        tokio::pin!(run);
        let _gone = cancel.clone().drop_guard();
        let out = match request {
            Some(request) => tokio::select! {
                out = &mut run => out,
                () = request.cancelled() => {
                    cancel.cancel();
                    run.await
                }
            },
            None => run.await,
        }
        .map_err(|e| format!("the turn did not finish: {e}"))?;
        match out.ended {
            Ended::Done if out.answer.is_empty() => {
                Ok(format!("{} had nothing to say to that.", agent.name))
            }
            Ended::Done => Ok(out.answer),
            Ended::TimedOut => Err(format!(
                "{} did not finish within {} minutes; what it changed before that is in the \
                 audit",
                agent.name,
                self.max_runtime.as_secs().div_ceil(60)
            )),
            Ended::Cancelled => Err(format!("stopped before {} answered", agent.name)),
            Ended::Transient(e) => Err(format!("the model could not be reached: {e}")),
            Ended::Failed(e) => Err(e),
        }
    }
}

// --- the desk's voice for one sentence --------------------------------------------------------

/// `say`, through desk voice (ADR 0032): only where the operator let Ghost speak first
/// (`desk` in `[routes].enabled`), outside quiet hours and do-not-disturb, and only when
/// the desk is idle. The sentence is the greeting; a reply is the acknowledgment.
pub struct Desk(pub Weak<Daemon>);

#[async_trait]
impl DeskVoice for Desk {
    async fn say(&self, line: &str, item: &Item) -> Said {
        let busy = |why: &str| Said::Busy(why.to_string());
        let Some(d) = self.0.upgrade() else {
            return busy("Ghost is stopping");
        };
        let (Some(routes), Some(waiting)) = (d.routes(), d.waiting()) else {
            return busy("Ghost is starting");
        };
        let policy = routes.policy();
        if !policy.enabled.contains(&RouteName::Desk) {
            return busy("the operator has not let Ghost speak first at the desk");
        }
        if routes.quiet(utcnow()) {
            return busy("it is a quiet time");
        }
        if !d.can_raise() {
            return busy("the desk is busy");
        }
        let (reply, answer) = oneshot::channel();
        let raised = Raised {
            items: vec![item.clone()],
            policy: RoutePolicy {
                greeting: line.to_string(),
                ..policy
            },
            reply,
        };
        if !d.raise(raised) {
            return busy("the desk is busy");
        }
        // Said at the desk: the routes need not carry it again unless nobody answers.
        let _ = waiting
            .items
            .delivered(&item.key, RouteName::Desk.as_str(), None);
        let key = item.key.clone();
        tokio::spawn(async move {
            let wait = Duration::from_secs_f64(DELIVER_TIMEOUT_SECS);
            if let Ok(Ok(Delivery::Heard)) = tokio::time::timeout(wait, answer).await {
                let _ = waiting.acknowledge(&key, None, None);
            }
        });
        Said::Speaking
    }
}

// --- the daemon's side ------------------------------------------------------------------------

/// What the daemon keeps of all this: the clients (there whether or not anything
/// listens, so Settings can list them), the server, the listener and why it is not up.
pub struct OutsideHost {
    pub clients: Arc<Clients>,
    /// The server as outside clients are served it; None when Ghost's own app is not up.
    pub server: Option<GhostServer>,
    listener: Option<OutsideListener>,
    problem: Mutex<Option<String>>,
    feed: Mutex<Option<JoinHandle<()>>>,
}

impl OutsideHost {
    /// `server` is Ghost's own app as its agents have it; what outside clients get is
    /// made from it here, with `outside` (the gate, the desk's voice). Nothing is bound
    /// yet.
    pub fn new(
        listen: &str,
        clients: Arc<Clients>,
        server: Option<&GhostServer>,
        outside: Outside,
    ) -> Self {
        let server = server.map(|s| s.for_outside(outside));
        let mut problem = None;
        let listener = match (&server, listen.trim().is_empty()) {
            (_, true) => None,
            (None, false) => {
                problem = Some("Ghost's own app is not running".to_string());
                None
            }
            (Some(server), false) => {
                match OutsideListener::new(listen, server.clone(), clients.clone()) {
                    Ok(l) => Some(l),
                    Err(e) => {
                        problem = Some(e.to_string());
                        None
                    }
                }
            }
        };
        Self {
            clients,
            server,
            listener,
            problem: Mutex::new(problem.map(|p| Self::sentence(listen, &p))),
            feed: Mutex::new(None),
        }
    }

    fn sentence(listen: &str, why: &str) -> String {
        format!("Ghost is not listening for outside tools on {listen}: {why}")
    }

    /// The addresses it is set to listen on; empty when off.
    pub fn addresses(&self) -> Vec<String> {
        self.listener
            .as_ref()
            .map(|l| l.addresses.clone())
            .unwrap_or_default()
    }

    /// Why nothing is listening though something should be: a problem on the waiting
    /// list, never a crash.
    pub fn problem(&self) -> Option<String> {
        lock(&self.problem).clone()
    }

    /// The first bound port.
    pub fn port(&self) -> u16 {
        self.listener.as_ref().map_or(0, OutsideListener::port)
    }

    /// Bind and serve, and start turning the daemon's events into what `events_wait`
    /// returns. `events` is the bus's tap: the control API's own events, as JSON.
    pub async fn start(&self, waiting: &Arc<WaitingList>, events: broadcast::Receiver<String>) {
        if let Some(p) = self.problem() {
            error!("outside tools: {p}");
        }
        let Some(listener) = &self.listener else {
            return;
        };
        if let Err(e) = listener.start().await {
            let p = Self::sentence(&listener.addresses.join(","), &format!("{:?}", e.kind()));
            error!("outside tools: {p}");
            *lock(&self.problem) = Some(p);
            return;
        }
        if let Some(log) = self.server.as_ref().and_then(GhostServer::events) {
            let feed = Feed::new(log, waiting.clone());
            *lock(&self.feed) = Some(tokio::spawn(follow(feed, events)));
        }
    }

    pub async fn stop(&self) {
        if let Some(feed) = lock(&self.feed).take() {
            feed.abort();
        }
        if let Some(listener) = &self.listener {
            listener.stop().await;
        }
    }
}

/// The bus, an event at a time. A reader that fell behind looks at the waiting list
/// again, so nothing it missed about it is lost.
async fn follow(feed: Feed, mut events: broadcast::Receiver<String>) {
    loop {
        match events.recv().await {
            Ok(text) => {
                if FED.iter().any(|kind| text.starts_with(kind))
                    && let Ok(event) = serde_json::from_str::<Value>(&text)
                {
                    feed.on_bus(&event);
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => feed.waiting_changed(),
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
#[path = "outside_tests.rs"]
mod tests;
