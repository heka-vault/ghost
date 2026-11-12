//! `ghostd mcp` (ADR 0048): the outside tools the operator connects to Ghost, from the
//! command line, and the bridge for a tool that can only start a command.
//!
//! `add <name>` makes a client and prints its token and what to paste into the other tool,
//! once. `list` and `remove <name>` are what they say; a removed client is refused by the
//! running daemon at its next request, since both read the same store. `serve --client
//! <name>` speaks the protocol on stdin and stdout and forwards every message to the
//! running daemon's listener with that client's token, read from the private file
//! `add --command` wrote. The bridge goes through the listener, not a socket of its own:
//! one way in, so the token, the lockout, the budget and the audit are the same for every
//! client, and a peer on a socket would only be "this user", never a named client.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ghost_core::envfile::read_env;
use ghost_core::ghost_app::clients::{
    Clients, DEFAULT_LISTEN, http_snippets, read_token, stdio_snippets,
};
use ghost_core::store::Store;
use rmcp::RoleServer;
use rmcp::model::{
    ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorCode, ErrorData, RequestId,
    ServerJsonRpcMessage,
};
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, Transport};
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroizing;

use crate::GhostdSettings;
use crate::config::ConfigError;
use crate::outside::{PATH, is_loopback, parse_addresses};
use crate::phone::listener::parse_listen;

pub const USAGE: &str =
    "usage: ghostd mcp add <name> [--command] | list | remove <name> | serve --client <name>";
/// How long the bridge keeps trying a daemon that is not answering before it says so.
pub const RETRY_FOR: Duration = Duration::from_secs(15);
/// JSON-RPC's range for a server's own errors.
const NOT_REACHABLE: i32 = -32001;

/// The settings for `ghostd mcp`: the environment, and under it Ghost's `settings.env`.
/// The daemon is handed that file by whatever starts it; a tool that starts
/// `ghostd mcp serve` hands over nothing, and the listener's address may be set there.
pub fn settings_over(mut env: HashMap<String, String>) -> Result<GhostdSettings, ConfigError> {
    let base = GhostdSettings::from_env(&env)?;
    if let Ok(file) = read_env(&base.settings_path) {
        for (key, value) in file {
            env.entry(key).or_insert(value);
        }
    }
    GhostdSettings::from_env(&env)
}

fn clients(cfg: &GhostdSettings) -> Result<Clients, String> {
    if let Some(parent) = cfg.db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let store = Store::open(&cfg.db_path).map_err(|e| e.to_string())?;
    Ok(Clients::new(Arc::new(store), cfg.clients_dir()))
}

/// The address a client on this computer connects to: the first one listened on, with
/// "any address" read as this computer.
pub fn connect_address(cfg: &GhostdSettings) -> Option<String> {
    let first = parse_addresses(&cfg.mcp_listen).ok()?.into_iter().next()?;
    let (host, port) = parse_listen(&first).ok()?;
    Some(match host.as_str() {
        "0.0.0.0" => format!("127.0.0.1:{port}"),
        "::" => format!("[::1]:{port}"),
        h if h.contains(':') => format!("[{h}]:{port}"),
        h => format!("{h}:{port}"),
    })
}

fn ghostd_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "ghostd".to_string())
}

/// `ghostd mcp add|list|remove`. The token is printed by `add` and nowhere else.
pub fn mcp_cli(
    args: &[String],
    cfg: &GhostdSettings,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let fail = |err: &mut dyn Write, e: String| {
        let _ = writeln!(err, "{e}");
        1
    };
    match args.first().map(String::as_str) {
        Some("add") => {
            let command = args.iter().any(|a| a == "--command");
            let names: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
            let [name] = names[..] else {
                let _ = writeln!(err, "{USAGE}");
                return 2;
            };
            let made = match clients(cfg)
                .and_then(|c| c.create(name, command).map_err(|e| e.to_string()))
            {
                Ok(m) => m,
                Err(e) => return fail(err, e),
            };
            let name = &made.client.name;
            let _ = writeln!(out, "Connected {name}. Its token, shown this once:\n");
            let _ = writeln!(out, "  {}\n", *made.token);
            let snippets = if command {
                let _ = writeln!(
                    out,
                    "Ghost keeps it in {} (only you can read it), and the tool runs ghostd to reach Ghost.\n",
                    made.token_file
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
                stdio_snippets(name, &ghostd_path())
            } else {
                let address = connect_address(cfg).unwrap_or_else(|| DEFAULT_LISTEN.to_string());
                if cfg.mcp_listen.trim().is_empty() {
                    let _ = writeln!(
                        out,
                        "Ghost is not listening for outside tools (GHOST_MCP_LISTEN is empty); set it, then restart Ghost.\n"
                    );
                }
                http_snippets(&made.token, &address)
            };
            let _ = writeln!(out, "Claude Code:\n\n  {}\n", snippets.claude_code);
            let _ = writeln!(
                out,
                "Cursor (~/.cursor/mcp.json) and other tools that read an mcp.json:\n\n{}",
                snippets.config_json
            );
            0
        }
        Some("list") => {
            let list = match clients(cfg).and_then(|c| c.list().map_err(|e| e.to_string())) {
                Ok(l) => l,
                Err(e) => return fail(err, e),
            };
            if list.is_empty() {
                let _ = writeln!(
                    out,
                    "No outside tools are connected. ghostd mcp add <name> connects one."
                );
                return 0;
            }
            for c in list {
                let used = c.last_used_at.map_or_else(
                    || "never used".to_string(),
                    |t| format!("last used {}", t.format("%Y-%m-%d %H:%M UTC")),
                );
                let _ = writeln!(
                    out,
                    "{}  added {}  {used}",
                    c.name,
                    c.created_at.format("%Y-%m-%d")
                );
            }
            0
        }
        Some("remove") => {
            let Some(name) = args.get(1) else {
                let _ = writeln!(err, "{USAGE}");
                return 2;
            };
            match clients(cfg).and_then(|c| c.remove(name).map_err(|e| e.to_string())) {
                Ok(()) => {
                    let _ = writeln!(out, "Removed {name}. Its token no longer works.");
                    0
                }
                Err(e) => fail(err, e),
            }
        }
        _ => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
    }
}

/// `ghostd mcp serve --client <name>`: stdin and stdout to the running daemon.
pub async fn serve_cli(args: &[String], cfg: &GhostdSettings) -> i32 {
    let name = match args {
        [flag, name] if flag == "--client" => name.clone(),
        [arg] if arg.starts_with("--client=") => arg["--client=".len()..].to_string(),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let token = match read_token(&cfg.clients_dir(), &name) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "ghostd mcp serve: no token for {name} ({e}). Run: ghostd mcp add {name} --command"
            );
            return 1;
        }
    };
    let Some(address) = connect_address(cfg) else {
        eprintln!(
            "ghostd mcp serve: Ghost is not set to listen for outside tools (GHOST_MCP_LISTEN is empty)"
        );
        return 1;
    };
    if let Ok((host, _)) = parse_listen(&address)
        && !is_loopback(&host)
    {
        eprintln!("ghostd mcp serve: reaching Ghost at {address}, which is not this computer");
    }
    let target = Target {
        url: format!("http://{address}{PATH}"),
        token,
        retry_for: RETRY_FOR,
    };
    match bridge(tokio::io::stdin(), tokio::io::stdout(), target).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ghostd mcp serve: {e}");
            1
        }
    }
}

/// Where the bridge forwards to, and as whom.
pub struct Target {
    pub url: String,
    pub token: Zeroizing<String>,
    pub retry_for: Duration,
}

type Upstream = StreamableHttpClientTransport<reqwest::Client>;

struct Bridge {
    target: Target,
    up: Option<Upstream>,
    /// The client's `initialize` and its `initialized`, said again to a daemon that
    /// restarted and so knows nothing of this client. A client on the stateless protocol
    /// has neither: each of its requests stands alone.
    init: Option<ClientJsonRpcMessage>,
    initialized: Option<ClientJsonRpcMessage>,
    /// The answers to an `initialize` said again: the client already has its own.
    swallow: HashSet<RequestId>,
}

fn is_init(msg: &ClientJsonRpcMessage) -> bool {
    matches!(msg, ClientJsonRpcMessage::Request(r) if matches!(r.request, ClientRequest::InitializeRequest(_)))
}

fn is_initialized(msg: &ClientJsonRpcMessage) -> bool {
    matches!(msg, ClientJsonRpcMessage::Notification(n)
        if matches!(n.notification, ClientNotification::InitializedNotification(_)))
}

/// A refusal no amount of waiting changes: the token is not a client's.
fn refused(error: &str) -> bool {
    error.contains("Auth required") || error.contains("HTTP 401") || error.contains("HTTP 429")
}

impl Bridge {
    fn connect(&self) -> Upstream {
        StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(self.target.url.clone())
                .auth_header(self.target.token.to_string()),
        )
    }

    async fn drop_up(&mut self) {
        if let Some(mut up) = self.up.take() {
            let _ = up.close().await;
        }
    }

    fn swallowed(&mut self, msg: &ServerJsonRpcMessage) -> bool {
        match msg {
            ServerJsonRpcMessage::Response(r) => self.swallow.remove(&r.id),
            ServerJsonRpcMessage::Error(e) => {
                e.id.as_ref().is_some_and(|id| self.swallow.remove(id))
            }
            _ => false,
        }
    }

    /// The next message from the daemon; never, while nothing is connected.
    async fn next(&mut self) -> Option<ServerJsonRpcMessage> {
        match &mut self.up {
            Some(up) => up.receive().await,
            None => std::future::pending().await,
        }
    }

    /// One message up, passing down whatever arrives meanwhile.
    async fn send<T>(&mut self, msg: ClientJsonRpcMessage, down: &mut T) -> Result<(), String>
    where
        T: Transport<RoleServer>,
    {
        let Some(up) = self.up.as_mut() else {
            return Err("not connected".into());
        };
        let sent = up.send(msg);
        tokio::pin!(sent);
        loop {
            let got = tokio::select! {
                r = &mut sent => return r.map_err(|e| e.to_string()),
                m = up.receive() => m,
            };
            match got {
                Some(m) => {
                    let mine = match &m {
                        ServerJsonRpcMessage::Response(r) => self.swallow.remove(&r.id),
                        ServerJsonRpcMessage::Error(e) => {
                            e.id.as_ref().is_some_and(|id| self.swallow.remove(id))
                        }
                        _ => false,
                    };
                    if !mine {
                        down.send(m).await.map_err(|e| e.to_string())?;
                    }
                }
                // The connection's task ended: the send says why.
                None => return sent.await.map_err(|e| e.to_string()),
            }
        }
    }

    async fn attempt<T>(&mut self, msg: &ClientJsonRpcMessage, down: &mut T) -> Result<(), String>
    where
        T: Transport<RoleServer>,
    {
        if self.up.is_none() {
            self.up = Some(self.connect());
            if let Some(init) = self.init.clone()
                && !is_init(msg)
            {
                if let ClientJsonRpcMessage::Request(r) = &init {
                    self.swallow.insert(r.id.clone());
                }
                self.send(init, down).await?;
                if let Some(done) = self.initialized.clone()
                    && !is_initialized(msg)
                {
                    self.send(done, down).await?;
                }
            }
        }
        self.send(msg.clone(), down).await
    }

    /// Forward one message, trying again while the daemon is not there. What comes back
    /// is an error for the client to read, when it could not be forwarded.
    async fn forward<T>(
        &mut self,
        msg: ClientJsonRpcMessage,
        down: &mut T,
    ) -> Option<ServerJsonRpcMessage>
    where
        T: Transport<RoleServer>,
    {
        let deadline = Instant::now() + self.target.retry_for;
        let mut pause = Duration::from_millis(200);
        loop {
            let why = match self.attempt(&msg, down).await {
                Ok(()) => return None,
                Err(why) => why,
            };
            self.drop_up().await;
            let said = if refused(&why) {
                Some("Ghost refused this client's token. It was removed, or too many wrong tokens were sent; connect it again with: ghostd mcp add <name> --command".to_string())
            } else if Instant::now() >= deadline {
                Some(format!(
                    "Ghost is not answering at {} (tried for {} s). Start Ghost, then try again.",
                    self.target.url,
                    self.target.retry_for.as_secs()
                ))
            } else {
                None
            };
            if let Some(said) = said {
                eprintln!("ghostd mcp serve: {said}");
                return match &msg {
                    ClientJsonRpcMessage::Request(r) => Some(ServerJsonRpcMessage::error(
                        ErrorData::new(ErrorCode(NOT_REACHABLE), said, None),
                        Some(r.id.clone()),
                    )),
                    _ => None,
                };
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(2));
        }
    }
}

/// The bridge: every message read from `reader` goes to the daemon at `target`, and
/// every message from the daemon is written to `writer`. It ends when the reader does.
pub async fn bridge<R, W>(reader: R, writer: W, target: Target) -> std::io::Result<()>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut down = AsyncRwTransport::<RoleServer, R, W>::new_server(reader, writer);
    let mut b = Bridge {
        target,
        up: None,
        init: None,
        initialized: None,
        swallow: HashSet::new(),
    };
    loop {
        tokio::select! {
            msg = down.receive() => {
                let Some(msg) = msg else { break };
                if is_init(&msg) {
                    b.init = Some(msg.clone());
                    b.initialized = None;
                } else if is_initialized(&msg) {
                    b.initialized = Some(msg.clone());
                }
                if let Some(error) = b.forward(msg, &mut down).await {
                    down.send(error).await?;
                }
            }
            msg = b.next() => match msg {
                Some(m) => {
                    if !b.swallowed(&m) {
                        down.send(m).await?;
                    }
                }
                None => b.drop_up().await,
            },
        }
    }
    b.drop_up().await;
    Ok(())
}
