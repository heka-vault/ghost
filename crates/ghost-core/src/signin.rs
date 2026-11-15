//! Signing in to an app (ADR 0034): OAuth 2.1 with PKCE, the way the MCP authorization spec
//! asks, through the operator's browser.
//!
//! Which client Ghost signs in as is chosen in [`crate::signin_clients`] (ADR 0076): the
//! person's own client ID for the app (only at the issuer it was given for), Ghost's own for
//! that authorization server (only for an app at one of the row's own hosts), dynamic
//! registration where it is offered, and Ghost's client metadata document (a static file on
//! heyghost.app, whose loopback redirects the specification allows) where only that is taken
//! and it names this sign-in's way back. A server none of them gets into is said in plain
//! words, never as an error's kind. The browser comes back to a loopback address, `http://127.0.0.1:<port>/callback`,
//! which listens only while a sign-in is open, on 127.0.0.1 only, for one path. The port is
//! fixed (`GHOST_SIGNIN_PORT`) because a registration names its redirect exactly. Ghost's
//! own client on a server that wants the device flow (GitHub) does without the browser's
//! way back: the person types a short code on the service's own page ([`device`]).
//!
//! Tokens and the registration are kept per app in `~/.local/share/ghost/apps/<name>/
//! oauth.json` (0600). They never reach the control API, a log line or the model; `rmcp`
//! refreshes them. When an app wants a new sign-in and nobody is at the browser (the
//! daemon starting), the connection fails as `AppError::NeedsSignIn` and the app waits on
//! the operator's list.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    CredentialStore, OAuthTokenResponse, StoredCredentials,
};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::apps::{App, AppError, AppSpec, AuthFor};
use crate::signin_clients::{
    CIMD_URL, Choice, ClientAuth, KnownServer, OwnClient, ServerTakes, choose, needs_a_client,
    registration_refused,
};

mod device;

pub const DEFAULT_PORT: u16 = 47823;
pub const CLIENT_NAME: &str = "Ghost";
const PAGE_HEAD: &str = "<!doctype html><meta charset=utf-8><title>Ghost</title><body \
                         style='font:16px system-ui;margin:4em;text-align:center'><p>";
const PAGE_TAIL: &str = "</p><p>You can close this tab.</p>";
/// One request line, then its headers: the browser sends them at once.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// What the browser brought back: the code, and the state and issuer when it sent them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationCodeResult {
    pub code: String,
    pub state: Option<String>,
    pub iss: Option<String>,
}

// --- the token file ---------------------------------------------------------------------

/// `rmcp`'s `CredentialStore`, as one 0600 JSON file: `tokens` and `client`, the shape
/// the Python daemon wrote, so a sign-in survives the port.
#[derive(Debug, Clone)]
pub struct FileTokenStorage {
    pub path: PathBuf,
    redirect_uri: Option<String>,
}

impl FileTokenStorage {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            redirect_uri: None,
        }
    }

    /// The redirect a new registration is written with.
    pub fn with_redirect_uri(mut self, uri: &str) -> Self {
        self.redirect_uri = Some(uri.to_string());
        self
    }

    fn read(&self) -> Map<String, Value> {
        match std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    fn write(&self, data: &Map<String, Value>) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if let Some(parent) = self.path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(Value::Object(data.clone()).to_string().as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &self.path)
    }

    pub fn get_tokens(&self) -> Option<OAuthTokenResponse> {
        let raw = self.read().remove("tokens")?;
        if !crate::apps::spec_truthy(&raw) {
            return None;
        }
        serde_json::from_value(raw).ok()
    }

    pub fn set_tokens(&self, tokens: &OAuthTokenResponse) -> std::io::Result<()> {
        let mut data = self.read();
        data.insert(
            "tokens".into(),
            serde_json::to_value(tokens).unwrap_or(Value::Null),
        );
        self.write(&data)
    }

    pub fn client_id(&self) -> Option<String> {
        self.read()
            .get("client")
            .and_then(|c| c.get("client_id"))
            .and_then(Value::as_str)
            .map(String::from)
    }

    pub fn set_client_id(&self, client_id: &str) -> std::io::Result<()> {
        let mut data = self.read();
        let mut client = match data.remove("client") {
            Some(Value::Object(m)) => m,
            _ => Map::from_iter([
                ("client_name".to_string(), json!(CLIENT_NAME)),
                (
                    "redirect_uris".to_string(),
                    json!(self.redirect_uri.iter().collect::<Vec<_>>()),
                ),
                (
                    "grant_types".to_string(),
                    json!(["authorization_code", "refresh_token"]),
                ),
                ("response_types".to_string(), json!(["code"])),
                ("token_endpoint_auth_method".to_string(), json!("none")),
            ]),
        };
        client.insert("client_id".into(), json!(client_id));
        data.insert("client".into(), Value::Object(client));
        self.write(&data)
    }

    /// What a device sign-in brought back, kept as `rmcp` keeps a sign-in: the client it
    /// was for, the tokens, the scopes and the issuer.
    async fn save_device_token(
        &self,
        client_id: &str,
        token: OAuthTokenResponse,
        scopes: Vec<String>,
        issuer: Option<String>,
    ) -> Result<(), AppError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        CredentialStore::save(
            self,
            StoredCredentials::new(client_id.to_string(), Some(token), scopes, Some(now))
                .with_issuer(issuer),
        )
        .await
        .map_err(|_| AppError::Connect("CredentialStoreError".into()))
    }

    pub fn has_tokens(&self) -> bool {
        self.read()
            .get("tokens")
            .is_some_and(crate::apps::spec_truthy)
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[async_trait]
impl CredentialStore for FileTokenStorage {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let data = self.read();
        let Some(client_id) = self.client_id() else {
            return Ok(None);
        };
        let tokens = data
            .get("tokens")
            .filter(|t| crate::apps::spec_truthy(t))
            .and_then(|t| serde_json::from_value::<OAuthTokenResponse>(t.clone()).ok());
        let granted_scopes = data
            .get("granted_scopes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let token_received_at = data.get("token_received_at").and_then(Value::as_u64);
        let issuer = data.get("issuer").and_then(Value::as_str).map(String::from);
        Ok(Some(
            StoredCredentials::new(client_id, tokens, granted_scopes, token_received_at)
                .with_issuer(issuer),
        ))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let store = |e: std::io::Error| AuthError::CredentialStoreError(e.kind().to_string());
        self.set_client_id(&credentials.client_id).map_err(store)?;
        let mut data = self.read();
        match &credentials.token_response {
            Some(t) => {
                data.insert(
                    "tokens".into(),
                    serde_json::to_value(t).unwrap_or(Value::Null),
                );
            }
            None => {
                data.remove("tokens");
            }
        }
        data.insert("granted_scopes".into(), json!(credentials.granted_scopes));
        data.insert(
            "token_received_at".into(),
            json!(credentials.token_received_at),
        );
        data.insert("issuer".into(), json!(credentials.issuer));
        self.write(&data).map_err(store)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        FileTokenStorage::clear(self);
        Ok(())
    }
}

// --- the sign-ins -----------------------------------------------------------------------

pub type OnUrl = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// One open sign-in: the app, the address the browser should open, and where the
/// browser's way back is sent.
struct Flow {
    /// Which flow this is: a device flow polls only while its own is open.
    id: u64,
    app: String,
    url: Option<String>,
    /// The code the person types on the service's page (the device flow).
    code: Option<String>,
    sender: Option<oneshot::Sender<Result<HashMap<String, String>, AppError>>>,
    receiver: Option<oneshot::Receiver<Result<HashMap<String, String>, AppError>>>,
}

#[derive(Default)]
struct State {
    /// One at a time: a new sign-in replaces the open one.
    flow: Option<Flow>,
    server: Option<tokio::task::JoinHandle<()>>,
    next_id: u64,
    /// The service an app's sign-in last found it needs a client for (ADR 0076).
    needs_client: HashMap<String, Option<&'static KnownServer>>,
}

/// Every app's sign-in. `auth_for` is what `crate::apps::Apps` asks for when it connects
/// an app that signs in; `on_url` hears the address the operator's browser should open
/// (the desktop opens it).
#[derive(Clone)]
pub struct SignIns {
    pub data_dir: PathBuf,
    pub port: u16,
    on_url: Option<OnUrl>,
    state: Arc<Mutex<State>>,
    /// The authorization servers Ghost knows ([`crate::signin_clients::KNOWN_SERVERS`]).
    servers: &'static [KnownServer],
    /// Ghost's client metadata document, as published.
    document: &'static str,
}

impl SignIns {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            on_url: None,
            state: Arc::new(Mutex::new(State::default())),
            servers: crate::signin_clients::KNOWN_SERVERS,
            document: crate::signin_clients::CIMD_DOCUMENT,
        }
    }

    /// Another document: a test's, naming its own port.
    #[cfg(test)]
    fn with_document(mut self, document: &'static str) -> Self {
        self.document = document;
        self
    }

    /// Another table of servers: a test's fake one, with a client of Ghost's own.
    #[cfg(test)]
    fn with_servers(mut self, servers: &'static [KnownServer]) -> Self {
        self.servers = servers;
        self
    }

    /// The row for this app's server, only when that server serves this app (ADR 0076).
    fn known(&self, issuer: Option<&str>, app_url: &str) -> Option<&'static KnownServer> {
        crate::signin_clients::server_for_app(self.servers, issuer, app_url)
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_on_url(mut self, on_url: OnUrl) -> Self {
        self.on_url = Some(on_url);
        self
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// The same loopback, by name: OpenRouter takes `localhost` callbacks only.
    pub fn localhost_redirect_uri(&self) -> String {
        format!("http://localhost:{}/callback", self.port)
    }

    pub fn storage(&self, app: &str) -> FileTokenStorage {
        FileTokenStorage::new(self.data_dir.join(app).join("oauth.json"))
            .with_redirect_uri(&self.redirect_uri())
    }

    pub fn signed_in(&self, app: &str) -> bool {
        self.storage(app).has_tokens()
    }

    /// The app was disconnected: its tokens and registration go.
    pub fn forget(&self, app: &str) {
        self.storage(app).clear();
        let mut state = self.lock();
        state.needs_client.remove(app);
        if state.flow.as_ref().is_some_and(|f| f.app == app) {
            Self::cancel_flow(&mut state);
        }
    }

    /// The authorization server the app at `url` names now, as its metadata says (ADR
    /// 0076): what the person's own client is bound to when they give it.
    pub async fn issuer_of(&self, url: &str) -> Result<Option<String>, AppError> {
        let manager = AuthorizationManager::new_allowing_private_addresses(url)
            .await
            .map_err(auth_error)?;
        let resolution = manager.resolve_metadata().await.map_err(auth_error)?;
        Ok(resolution
            .metadata
            .issuer
            .filter(|_| resolution.source.is_discovered()))
    }

    /// The code to type on the service's page, while a device sign-in waits for it.
    pub fn pending_code(&self, app: &str) -> Option<String> {
        let state = self.lock();
        state
            .flow
            .as_ref()
            .filter(|f| f.app == app)
            .and_then(|f| f.code.clone())
    }

    /// The service the app's last sign-in needed a client for, if it did: what the
    /// desktop asks the person for and where it sends them.
    pub fn needs_client(&self, app: &str) -> Option<Option<&'static KnownServer>> {
        self.lock().needs_client.get(app).copied()
    }

    fn set_needs_client(&self, app: &str, server: Option<Option<&'static KnownServer>>) {
        let mut state = self.lock();
        match server {
            Some(s) => {
                state.needs_client.insert(app.to_string(), s);
            }
            None => {
                state.needs_client.remove(app);
            }
        }
    }

    pub fn pending_url(&self, app: &str) -> Option<String> {
        let state = self.lock();
        state
            .flow
            .as_ref()
            .filter(|f| f.app == app)
            .and_then(|f| f.url.clone())
    }

    /// The sign-in for `spec`, or None when it doesn't sign in. Not interactive (the
    /// daemon starting, a watch reviving it), a sign-in that needs the browser fails as
    /// `AppError::NeedsSignIn`; stored tokens are used and refreshed either way.
    pub fn auth_for(&self, spec: &AppSpec, interactive: bool) -> Option<AppAuth> {
        if !spec.sign_in || spec.url.is_none() {
            return None;
        }
        Some(AppAuth {
            spec: spec.clone(),
            interactive,
            signins: self.clone(),
        })
    }

    /// A sign-in that isn't an app's (the model connection's OpenRouter key, ADR 0036):
    /// listen for the browser's way back from `url` and return its query. It replaces
    /// any sign-in still open, as an app's does; `NeedsSignIn` if it was refused or
    /// replaced, `NoAnswer` if nobody came back.
    pub async fn browser_return(
        &self,
        name: &str,
        url: &str,
        wait: Duration,
    ) -> Result<HashMap<String, String>, AppError> {
        let receiver = self.start_flow(name, url, false).await?;
        let out = match tokio::time::timeout(wait, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppError::NeedsSignIn("the sign-in was replaced".into())),
            Err(_) => Err(AppError::NoAnswer(wait.as_secs())),
        };
        self.end_flow(name).await;
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- the browser's way back ---------------------------------------------------------

    async fn start_flow(
        &self,
        app: &str,
        url: &str,
        announce: bool,
    ) -> Result<oneshot::Receiver<Result<HashMap<String, String>, AppError>>, AppError> {
        let (tx, rx) = oneshot::channel();
        let needs_server = {
            let mut state = self.lock();
            Self::cancel_flow(&mut state);
            state.next_id += 1;
            state.flow = Some(Flow {
                id: state.next_id,
                app: app.to_string(),
                url: Some(url.to_string()),
                code: None,
                sender: Some(tx),
                receiver: None,
            });
            state.server.is_none()
        };
        if needs_server {
            let listener = TcpListener::bind(("127.0.0.1", self.port))
                .await
                .map_err(AppError::Io)?;
            let state = self.state.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _ = handle(stream, state).await;
                    });
                }
            });
            self.lock().server = Some(task);
        }
        info!("sign-in: {app} is waiting for the browser");
        if announce && let Some(on_url) = &self.on_url {
            on_url(app, url);
        }
        Ok(rx)
    }

    /// A device sign-in: the page to open and the code to type there. No loopback
    /// listener; returns the flow's id, which `flow_open` checks while it polls.
    fn start_device_flow(&self, app: &str, url: &str, code: &str) -> u64 {
        let id = {
            let mut state = self.lock();
            Self::cancel_flow(&mut state);
            state.next_id += 1;
            state.flow = Some(Flow {
                id: state.next_id,
                app: app.to_string(),
                url: Some(url.to_string()),
                code: Some(code.to_string()),
                sender: None,
                receiver: None,
            });
            state.next_id
        };
        info!("sign-in: {app} is waiting for its code to be typed");
        if let Some(on_url) = &self.on_url {
            on_url(app, url);
        }
        id
    }

    fn flow_open(&self, id: u64) -> bool {
        self.lock().flow.as_ref().is_some_and(|f| f.id == id)
    }

    fn cancel_flow(state: &mut State) {
        if let Some(mut flow) = state.flow.take()
            && let Some(sender) = flow.sender.take()
        {
            let _ = sender.send(Err(AppError::NeedsSignIn(
                "the sign-in was replaced".into(),
            )));
        }
    }

    /// The flow for `app` is over: the listener goes with it when nothing else waits.
    async fn end_flow(&self, app: &str) {
        let server = {
            let mut state = self.lock();
            if state.flow.as_ref().is_some_and(|f| f.app == app) {
                state.flow = None;
            }
            if state.flow.is_none() {
                state.server.take()
            } else {
                None
            }
        };
        if let Some(server) = server {
            server.abort();
            let _ = server.await;
        }
    }

    pub async fn close(&self) {
        let server = {
            let mut state = self.lock();
            Self::cancel_flow(&mut state);
            state.server.take()
        };
        if let Some(server) = server {
            server.abort();
            let _ = server.await;
        }
    }
}

/// One request: `GET /callback?code=…&state=…`. Anything else is a 404.
async fn handle(stream: TcpStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| std::io::Error::other("timed out"))??;
    loop {
        let mut header = String::new();
        let n = tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut header))
            .await
            .map_err(|_| std::io::Error::other("timed out"))??;
        if n == 0 || header == "\r\n" || header == "\n" {
            break; // headers: nothing in them is read
        }
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    let target = (parts.len() >= 2 && parts[0] == "GET")
        .then(|| url::Url::parse(&format!("http://127.0.0.1{}", parts[1])).ok())
        .flatten();
    let is_callback = target.as_ref().is_some_and(|t| t.path() == "/callback");
    let (status, message) = {
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        match (target, st.flow.as_mut()) {
            (Some(t), Some(flow)) if is_callback && flow.sender.is_some() => {
                let q: HashMap<String, String> = t
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                if let Some(err) = q.get("error") {
                    if let Some(sender) = flow.sender.take() {
                        let short: String = err.chars().take(40).collect();
                        let _ = sender.send(Err(AppError::NeedsSignIn(format!(
                            "the sign-in was refused ({short})"
                        ))));
                    }
                    (200, "The sign-in didn't go through.")
                } else {
                    if let Some(sender) = flow.sender.take() {
                        let _ = sender.send(Ok(q));
                    }
                    (200, "You're signed in.")
                }
            }
            _ => (404, "Nothing is waiting for this."),
        }
    };
    let body = format!("{PAGE_HEAD}{message}{PAGE_TAIL}");
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    writer.write_all(head.as_bytes()).await?;
    writer.write_all(body.as_bytes()).await?;
    writer.shutdown().await
}

// --- an app's sign-in -------------------------------------------------------------------

/// The sign-in of one app: where the browser goes, how it comes back, and the authorized
/// client once it has.
pub struct AppAuth {
    spec: AppSpec,
    interactive: bool,
    signins: SignIns,
}

impl AppAuth {
    /// The app's authorization server wants the operator at `url`.
    pub async fn redirect_handler(&self, url: &str) -> Result<(), AppError> {
        if !self.interactive {
            return Err(AppError::NeedsSignIn(format!(
                "{} needs you to sign in again",
                self.spec.shown()
            )));
        }
        let rx = self.signins.start_flow(&self.spec.name, url, true).await?;
        let mut state = self.signins.lock();
        if let Some(flow) = state.flow.as_mut().filter(|f| f.app == self.spec.name) {
            flow.receiver = Some(rx);
        }
        Ok(())
    }

    /// The browser's way back, once it comes.
    pub async fn callback_handler(&self) -> Result<AuthorizationCodeResult, AppError> {
        let rx = {
            let mut state = self.signins.lock();
            state
                .flow
                .as_mut()
                .filter(|f| f.app == self.spec.name)
                .and_then(|f| f.receiver.take())
        };
        let Some(rx) = rx else {
            return Err(AppError::NeedsSignIn(format!(
                "the sign-in to {} was replaced",
                self.spec.shown()
            )));
        };
        let out = match rx.await {
            Ok(Ok(params)) => Ok(AuthorizationCodeResult {
                code: params.get("code").cloned().unwrap_or_default(),
                state: params.get("state").cloned(),
                iss: params.get("iss").cloned(),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(AppError::NeedsSignIn("the sign-in was replaced".into())),
        };
        self.signins.end_flow(&self.spec.name).await;
        out
    }

    /// The authorized HTTP client: stored tokens when there are any (refreshed by `rmcp`
    /// as they expire), else a sign-in through the browser when interactive. `fresh`:
    /// the app refused the stored tokens, so forget them and sign in again. `own`: the
    /// person's own client for this app (ADR 0076), first in the order of choice.
    pub async fn client(
        &self,
        http: reqwest::Client,
        fresh: bool,
        own: Option<OwnClient>,
    ) -> Result<AuthClient<reqwest::Client>, AppError> {
        let storage = self.signins.storage(&self.spec.name);
        let url = self.spec.url.as_deref().unwrap_or("");
        let mut manager = AuthorizationManager::new_allowing_private_addresses(url)
            .await
            .map_err(auth_error)?;
        manager.set_credential_store(storage.clone());
        // A person's client with a secret is a confidential one: its refreshes need the
        // secret, and some servers take it only in the body. The metadata is resolved
        // here, once, as `initialize_from_store` would have.
        let with_secret = own.as_ref().and_then(|o| {
            o.client_secret
                .as_ref()
                .map(|s| (o.client_id.clone(), s.clone()))
        });
        let mut server: Option<&'static KnownServer> = None;
        if with_secret.is_some() && !fresh {
            let resolution = manager.resolve_metadata().await.map_err(auth_error)?;
            let issuer = resolution.metadata.issuer.clone();
            // The secret refreshes only at the server it was given for.
            self.own_bound(own.as_ref(), issuer.as_deref())?;
            server = self.signins.known(issuer.as_deref(), url);
            manager.set_metadata(shaped(resolution.metadata, server, true));
        }
        if fresh {
            // The tokens were refused: the registration may stand, the tokens may not.
            let data = storage.read();
            if data.contains_key("tokens") {
                let mut data = data;
                data.remove("tokens");
                storage.write(&data).map_err(AppError::Io)?;
            }
        } else if manager.initialize_from_store().await.map_err(auth_error)? {
            if let Some((id, secret)) = &with_secret
                && storage.client_id().as_deref() == Some(id.as_str())
            {
                let config = rmcp::transport::auth::OAuthClientConfig::new(
                    id.clone(),
                    self.signins.redirect_uri(),
                )
                .with_client_secret(secret.clone());
                manager.configure_client(config).map_err(auth_error)?;
            }
            return Ok(AuthClient::new(http, manager));
        }
        if !self.interactive {
            return Err(AppError::NeedsSignIn(format!(
                "{} needs you to sign in again",
                self.spec.shown()
            )));
        }
        let resolution = manager.resolve_metadata().await.map_err(auth_error)?;
        let metadata = resolution.metadata;
        if server.is_none() {
            server = self.signins.known(metadata.issuer.as_deref(), url);
        }
        // The person's client goes only to the server they gave it for.
        self.own_bound(own.as_ref(), metadata.issuer.as_deref())?;
        // The document only for a way back it names: a server checks the redirect against it.
        let document_names_this_way_back =
            crate::signin_clients::document_redirects(self.signins.document)
                .contains(&self.signins.redirect_uri());
        let takes = ServerTakes {
            metadata_document: metadata
                .additional_fields
                .get("client_id_metadata_document_supported")
                .and_then(Value::as_bool)
                == Some(true)
                && document_names_this_way_back,
            registration: metadata.registration_endpoint.is_some(),
        };
        let choice = choose(own.as_ref(), server, takes);
        let shown = self.spec.shown();
        info!("sign-in: {} as {}", self.spec.name, choice.kind());
        if choice == Choice::NeedsClient {
            self.signins.set_needs_client(&self.spec.name, Some(server));
            return Err(AppError::NeedsClient(needs_a_client(&shown, server)));
        }
        self.signins.set_needs_client(&self.spec.name, None);
        let has_secret = matches!(
            &choice,
            Choice::Own(OwnClient {
                client_secret: Some(_),
                ..
            })
        );
        let device_endpoint = metadata
            .additional_fields
            .get("device_authorization_endpoint")
            .and_then(Value::as_str)
            .map(String::from);
        let issuer = metadata.issuer.clone();
        let token_endpoint = metadata.token_endpoint.clone();
        manager.set_metadata(shaped(metadata, server, has_secret));
        let scopes: Vec<String> = crate::catalog::in_tree_at(url)
            .map(|t| t.scopes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let mut request =
            AuthorizationRequest::new(self.signins.redirect_uri()).with_client_name(CLIENT_NAME);
        if !scopes.is_empty() {
            request = request.with_scopes(scopes.clone());
        }
        match &choice {
            Choice::Own(c) => {
                request = request.with_preregistered_client(c.client_id.clone());
                if let Some(secret) = &c.client_secret {
                    request = request.with_client_secret(secret.clone());
                }
            }
            Choice::Ghost(g) if g.auth == ClientAuth::DeviceFlow => {
                let Some(endpoint) = device_endpoint else {
                    warn!("sign-in: no device endpoint for Ghost's client");
                    return Err(AppError::NeedsClient(needs_a_client(&shown, server)));
                };
                let scopes = if scopes.is_empty() {
                    manager.select_scopes(None, &[])
                } else {
                    scopes
                };
                let token = device::sign_in(
                    &self.signins,
                    &self.spec.name,
                    &http,
                    &endpoint,
                    issuer.as_deref(),
                    g.client_id,
                    &scopes,
                    &token_endpoint,
                )
                .await?;
                storage
                    .save_device_token(g.client_id, token, scopes, issuer)
                    .await?;
                if !manager.initialize_from_store().await.map_err(auth_error)? {
                    return Err(AppError::NeedsSignIn(format!(
                        "the sign-in to {shown} didn't keep"
                    )));
                }
                return Ok(AuthClient::new(http, manager));
            }
            Choice::Ghost(g) => {
                request = request.with_preregistered_client(g.client_id);
            }
            Choice::MetadataDocument => {
                request = request.with_client_metadata_url(CIMD_URL);
            }
            Choice::Register | Choice::NeedsClient => {}
        }
        let session =
            AuthorizationSession::new(manager, request)
                .await
                .map_err(|(_, e)| match e {
                    AuthError::RegistrationFailed(_) if choice == Choice::Register => {
                        warn!("sign-in: {}: registration refused", self.spec.name);
                        self.signins.set_needs_client(&self.spec.name, Some(server));
                        AppError::NeedsClient(registration_refused(&shown, server))
                    }
                    e => auth_error(e),
                })?;
        let address = with_params(
            &session.auth_url,
            server.map_or(&[], |s| s.authorize_params),
        );
        self.redirect_handler(&address).await?;
        let result = self.callback_handler().await?;
        session
            .handle_callback_with_issuer(
                &result.code,
                result.state.as_deref().unwrap_or(""),
                result.iss.as_deref(),
            )
            .await
            .map_err(auth_error)?;
        Ok(AuthClient::new(http, session.auth_manager))
    }
}

impl AppAuth {
    /// The person's client may go to `issuer` only when it is the server it was given for;
    /// otherwise the app asks for it again, in words (ADR 0076).
    fn own_bound(&self, own: Option<&OwnClient>, issuer: Option<&str>) -> Result<(), AppError> {
        let Some(own) = own else {
            return Ok(());
        };
        if crate::signin_clients::own_client_bound(own, issuer) {
            return Ok(());
        }
        warn!(
            "sign-in: {}: own client not sent to another server",
            self.spec.name
        );
        self.signins.set_needs_client(&self.spec.name, Some(None));
        Err(AppError::NeedsClient(
            crate::signin_clients::own_client_moved(&self.spec.shown(), own.issuer.is_some()),
        ))
    }
}

/// The server's metadata as Ghost uses it: a person's secret goes in the body where the
/// table says that server wants it there.
fn shaped(
    mut metadata: rmcp::transport::auth::AuthorizationMetadata,
    server: Option<&KnownServer>,
    has_secret: bool,
) -> rmcp::transport::auth::AuthorizationMetadata {
    if has_secret && server.is_some_and(|s| s.secret_in_body) {
        metadata.additional_fields.insert(
            "token_endpoint_auth_methods_supported".into(),
            json!(["client_secret_post"]),
        );
    }
    metadata
}

/// The browser's address with a server's own parameters added (Google's offline access).
fn with_params(address: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return address.to_string();
    }
    match url::Url::parse(address) {
        Ok(mut u) => {
            let have: Vec<String> = u.query_pairs().map(|(k, _)| k.into_owned()).collect();
            {
                let mut q = u.query_pairs_mut();
                for (k, v) in params {
                    if !have.iter().any(|h| h == k) {
                        q.append_pair(k, v);
                    }
                }
            }
            u.to_string()
        }
        Err(_) => address.to_string(),
    }
}

/// An authorization error by kind only: never a body, a token or an address.
fn auth_error(e: AuthError) -> AppError {
    let kind = match &e {
        AuthError::AuthorizationRequired | AuthError::TokenRefreshRejected(_) => {
            return AppError::NeedsSignIn("needs sign-in".into());
        }
        AuthError::AuthorizationFailed(_) => "AuthorizationFailed",
        AuthError::TokenExchangeFailed(_) => "TokenExchangeFailed",
        AuthError::TokenRefreshFailed(_) => "TokenRefreshFailed",
        AuthError::CredentialStoreError(_) => "CredentialStoreError",
        AuthError::HttpError(_) => "HttpError",
        AuthError::OAuthError(_) => "OAuthError",
        AuthError::MetadataError(_) => "MetadataError",
        AuthError::PkceUnsupported => "PkceUnsupported",
        AuthError::UrlError(_) => "UrlError",
        AuthError::NoAuthorizationSupport => "NoAuthorizationSupport",
        AuthError::RegistrationFailed(_) => "RegistrationFailed",
        AuthError::InsufficientScope { .. } => "InsufficientScope",
        AuthError::AuthorizationServerMismatch { .. } => "AuthorizationServerMismatch",
        AuthError::AuthorizationServerMissingIssuer { .. } => "AuthorizationServerMissingIssuer",
        _ => "AuthError",
    };
    warn!("sign-in: {kind}");
    AppError::Connect(kind.to_string())
}

#[async_trait]
impl AuthFor for SignIns {
    async fn auth_for(
        &self,
        spec: &AppSpec,
        own: Option<OwnClient>,
        http: reqwest::Client,
        interactive: bool,
        fresh: bool,
    ) -> Result<Option<AuthClient<reqwest::Client>>, AppError> {
        match SignIns::auth_for(self, spec, interactive) {
            None => Ok(None),
            Some(auth) => auth.client(http, fresh, own).await.map(Some),
        }
    }
}

/// An app's sign-in, as `Apps` takes it.
pub fn auth_for_apps(signins: &SignIns) -> Arc<dyn AuthFor> {
    Arc::new(signins.clone())
}

#[allow(dead_code)]
fn _uses(_: &App, _: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;
    use tokio::io::AsyncReadExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn get(port: u16, path: &str) -> std::io::Result<String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await?;
        let mut data = String::new();
        stream.read_to_string(&mut data).await?;
        Ok(data)
    }

    fn signs_in(name: &str, url: &str) -> AppSpec {
        AppSpec {
            sign_in: true,
            ..AppSpec::at(name, url)
        }
    }

    #[tokio::test]
    async fn tokens_live_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStorage::new(dir.path().join("linear").join("oauth.json"));
        assert!(store.get_tokens().is_none() && !store.has_tokens());
        let tokens: OAuthTokenResponse =
            serde_json::from_value(json!({"access_token": "secret", "token_type": "Bearer"}))
                .unwrap();
        store.set_tokens(&tokens).unwrap();
        let back = serde_json::to_value(store.get_tokens().unwrap()).unwrap();
        assert_eq!(back["access_token"], "secret");
        assert_eq!(
            std::fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.clear();
        assert!(!store.path.exists());
    }

    #[tokio::test]
    async fn only_an_app_that_signs_in_gets_an_auth() {
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path());
        assert!(s.auth_for(&AppSpec::runs("x", "y"), true).is_none());
        assert!(
            s.auth_for(&AppSpec::at("x", "https://x/mcp"), true)
                .is_none()
        );
        assert!(s.auth_for(&signs_in("x", "https://x/mcp"), true).is_some());
    }

    #[tokio::test]
    async fn with_nobody_at_the_browser_a_sign_in_waits_on_the_operator() {
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path()).with_port(free_port());
        let auth = s
            .auth_for(&signs_in("linear", "https://x/mcp"), false)
            .unwrap();
        let err = auth
            .redirect_handler("https://auth.example/authorize?x=1")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NeedsSignIn(_)), "{err}");
        assert!(s.pending_url("linear").is_none());
    }

    #[tokio::test]
    async fn the_browser_comes_back_to_the_loopback_address_once() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let heard: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = heard.clone();
        let s = SignIns::new(dir.path())
            .with_port(port)
            .with_on_url(Arc::new(move |app, url| {
                seen.lock()
                    .unwrap()
                    .push((app.to_string(), url.to_string()));
            }));
        let auth = s
            .auth_for(&signs_in("linear", "https://x/mcp"), true)
            .unwrap();
        auth.redirect_handler("https://auth.example/authorize?x=1")
            .await
            .unwrap();
        assert_eq!(
            *heard.lock().unwrap(),
            vec![(
                "linear".to_string(),
                "https://auth.example/authorize?x=1".to_string()
            )]
        );
        assert_eq!(
            s.pending_url("linear").as_deref(),
            Some("https://auth.example/authorize?x=1")
        );
        let waiting = tokio::spawn({
            let auth = Arc::new(auth);
            async move { auth.callback_handler().await }
        });
        assert!(
            get(port, "/elsewhere")
                .await
                .unwrap()
                .starts_with("HTTP/1.1 404")
        );
        let page = get(port, "/callback?code=abc&state=st").await.unwrap();
        assert!(page.contains("signed in"));
        let result = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            (result.code.as_str(), result.state.as_deref()),
            ("abc", Some("st"))
        );
        assert!(s.pending_url("linear").is_none());
        // the listener is gone with the sign-in
        assert!(get(port, "/callback?code=again").await.is_err());
        s.close().await;
    }

    #[tokio::test]
    async fn a_refused_sign_in_and_a_replaced_one_are_said_so() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let s = SignIns::new(dir.path()).with_port(port);
        let auth = s
            .auth_for(&signs_in("linear", "https://x/mcp"), true)
            .unwrap();
        auth.redirect_handler("https://auth.example/a")
            .await
            .unwrap();
        let page = get(port, "/callback?error=access_denied").await.unwrap();
        assert!(page.contains("didn't go through"));
        let err = auth.callback_handler().await.unwrap_err();
        assert!(err.to_string().contains("refused (access_denied)"), "{err}");
        // a new sign-in replaces the open one
        let first = s
            .auth_for(&signs_in("linear", "https://x/mcp"), true)
            .unwrap();
        first
            .redirect_handler("https://auth.example/a")
            .await
            .unwrap();
        let second = s
            .auth_for(&signs_in("notion", "https://y/mcp"), true)
            .unwrap();
        second
            .redirect_handler("https://auth.example/b")
            .await
            .unwrap();
        let err = first.callback_handler().await.unwrap_err();
        assert!(err.to_string().contains("replaced"), "{err}");
        assert_eq!(
            s.pending_url("notion").as_deref(),
            Some("https://auth.example/b")
        );
        s.forget("notion");
        assert!(s.pending_url("notion").is_none());
        s.close().await;
    }

    #[tokio::test]
    async fn stored_tokens_are_used_without_the_browser() {
        // Discovery 404s everywhere, so rmcp falls back to the legacy endpoints and the
        // stored tokens (the Python daemon's file shape) are the client's.
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path()).with_port(free_port());
        let store = s.storage("linear");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            json!({"tokens": {"access_token": "stored-token", "token_type": "Bearer"},
                   "client": {"client_id": "ghost-1", "redirect_uris": [s.redirect_uri()]}})
            .to_string(),
        )
        .unwrap();
        assert!(s.signed_in("linear"));
        let spec = signs_in("linear", &format!("{}/mcp", server.uri()));
        let auth = s.auth_for(&spec, false).unwrap();
        let client = auth
            .client(reqwest::Client::new(), false, None)
            .await
            .unwrap();
        assert_eq!(client.get_access_token().await.unwrap(), "stored-token");
        assert!(s.pending_url("linear").is_none());
    }

    #[tokio::test]
    async fn a_sign_in_registers_opens_the_browser_and_keeps_the_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "client_id": "ghost-registered",
                "redirect_uris": ["http://127.0.0.1:1/callback"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "fresh-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-1"
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = opened.clone();
        let s = SignIns::new(dir.path())
            .with_port(port)
            .with_on_url(Arc::new(move |_, url| {
                seen.lock().unwrap().push(url.to_string())
            }));
        let spec = signs_in("linear", &format!("{}/mcp", server.uri()));
        let auth = Arc::new(s.auth_for(&spec, true).unwrap());
        let signing_in = tokio::spawn({
            let auth = auth.clone();
            async move { auth.client(reqwest::Client::new(), false, None).await }
        });
        // The "browser": wait for the address, then come back with its state.
        let url = loop {
            if let Some(u) = opened.lock().unwrap().first().cloned() {
                break u;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let parsed = url::Url::parse(&url).unwrap();
        assert!(
            url.starts_with(&format!("{}/authorize?", server.uri())),
            "{url}"
        );
        let state = parsed
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert!(parsed.query_pairs().any(|(k, _)| k == "code_challenge"));
        let page = get(port, &format!("/callback?code=abc&state={state}"))
            .await
            .unwrap();
        assert!(page.contains("signed in"));
        let client = tokio::time::timeout(Duration::from_secs(5), signing_in)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");
        let store = s.storage("linear");
        assert!(store.has_tokens() && s.signed_in("linear"));
        assert_eq!(store.client_id().as_deref(), Some("ghost-registered"));
        assert_eq!(
            std::fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let text = std::fs::read_to_string(&store.path).unwrap();
        assert!(text.contains("\"tokens\"") && text.contains("\"client\""));
        s.forget("linear");
        assert!(!s.signed_in("linear"));
    }

    // --- the order of choice, against fake authorization servers (ADR 0076) -------------

    const CLIENT_SECRET: &str = "own-client-secret-0123456789";

    /// A fake authorization server: the app at `/mcp`, its metadata, the token endpoint.
    /// `extra` joins the server's metadata (a registration endpoint, the CIMD flag).
    async fn fake_server(extra: Value) -> MockServer {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [base],
            })))
            .mount(&server)
            .await;
        let mut meta = json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
        });
        for (k, v) in extra.as_object().unwrap() {
            // "BASE" in a value is this server's own address.
            meta[k] = match v {
                Value::String(t) => Value::String(t.replace("BASE", &base)),
                other => other.clone(),
            };
        }
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(meta))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "fresh-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-1"
            })))
            .mount(&server)
            .await;
        server
    }

    /// Sign in to `server`'s app as `own`, playing the browser: returns the address the
    /// browser was sent to, and the client or the error.
    async fn sign_in_as(
        s: &SignIns,
        opened: &Arc<Mutex<Vec<String>>>,
        port: u16,
        server: &MockServer,
        own: Option<OwnClient>,
    ) -> (
        Option<String>,
        Result<AuthClient<reqwest::Client>, AppError>,
    ) {
        let spec = signs_in("acme", &format!("{}/mcp", server.uri()));
        let auth = Arc::new(s.auth_for(&spec, true).unwrap());
        let signing_in = tokio::spawn({
            let auth = auth.clone();
            async move { auth.client(reqwest::Client::new(), false, own).await }
        });
        let started = Instant::now();
        let url = loop {
            if let Some(u) = opened.lock().unwrap().first().cloned() {
                break Some(u);
            }
            if signing_in.is_finished() || started.elapsed() > Duration::from_secs(5) {
                break None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        if let Some(url) = &url {
            let parsed = url::Url::parse(url).unwrap();
            let state = parsed
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
                .unwrap();
            get(port, &format!("/callback?code=abc&state={state}"))
                .await
                .unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), signing_in)
            .await
            .unwrap()
            .unwrap();
        (url, result)
    }

    fn browser(dir: &Path) -> (SignIns, Arc<Mutex<Vec<String>>>, u16) {
        let port = free_port();
        let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = opened.clone();
        let s = SignIns::new(dir)
            .with_port(port)
            .with_on_url(Arc::new(move |_, url| {
                seen.lock().unwrap().push(url.to_string())
            }));
        (s, opened, port)
    }

    fn query(url: &str, key: &str) -> Option<String> {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }

    #[tokio::test]
    async fn a_server_that_takes_no_registration_says_what_it_needs() {
        let server = fake_server(json!({})).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let (url, result) = sign_in_as(&s, &opened, port, &server, None).await;
        assert!(url.is_none(), "no browser for a sign-in that can't happen");
        let err = result.err().unwrap();
        assert!(matches!(err, AppError::NeedsClient(_)), "{err}");
        let said = err.to_string();
        assert!(
            said.starts_with("Acme doesn't let Ghost register itself"),
            "{said}"
        );
        assert!(!said.contains("RegistrationFailed"));
        assert_eq!(err.reason(), said);
        assert_eq!(s.needs_client("acme"), Some(None));
        s.forget("acme");
        assert_eq!(s.needs_client("acme"), None);
    }

    #[tokio::test]
    async fn a_refused_registration_is_said_in_plain_words() {
        let server = fake_server(json!({})).await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(ResponseTemplate::new(403).set_body_string("no clients today"))
            .mount(&server)
            .await;
        let server_meta_with_registration = fake_server(json!({
            "registration_endpoint": format!("{}/register", server.uri())
        }))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let (_, result) = sign_in_as(&s, &opened, port, &server_meta_with_registration, None).await;
        let err = result.err().unwrap();
        assert!(matches!(err, AppError::NeedsClient(_)), "{err}");
        assert!(
            err.to_string()
                .starts_with("Acme refused to let Ghost register itself"),
            "{err}"
        );
        assert!(!err.to_string().contains("no clients today"));
    }

    #[tokio::test]
    async fn the_persons_own_client_is_used_and_its_secret_stays_in_memory() {
        let server = fake_server(json!({
            "registration_endpoint": "https://unused.example/register",
            "client_id_metadata_document_supported": true
        }))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let own = OwnClient {
            client_id: "own-client".into(),
            client_secret: Some(CLIENT_SECRET.into()),
            issuer: Some(server.uri()),
        };
        let (url, result) = sign_in_as(&s, &opened, port, &server, Some(own.clone())).await;
        let client = result.unwrap();
        let url = url.unwrap();
        // first in the order of choice, over registration and the document alike
        assert_eq!(query(&url, "client_id").as_deref(), Some("own-client"));
        assert!(!url.contains(CLIENT_SECRET));
        assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");
        // the token request carried the secret; nothing kept it
        let requests = server.received_requests().await.unwrap();
        let token = requests
            .iter()
            .find(|r| r.url.path() == "/token")
            .expect("a token request");
        let sent = format!(
            "{} {:?}",
            String::from_utf8_lossy(&token.body),
            token.headers.get("authorization")
        );
        assert!(
            sent.contains(CLIENT_SECRET) || sent.contains("Basic "),
            "{sent}"
        );
        let store = s.storage("acme");
        assert_eq!(store.client_id().as_deref(), Some("own-client"));
        let kept = std::fs::read_to_string(&store.path).unwrap();
        assert!(!kept.contains(CLIENT_SECRET), "{kept}");
        // What the log says is held in its own test binary (`tests/signin_logs.rs`), where a
        // subscriber is in place before anything logs, under either runner.
    }

    #[tokio::test]
    async fn a_server_in_the_table_wants_the_secret_in_the_body() {
        let server = fake_server(json!({})).await;
        let row: &'static [KnownServer] = Box::leak(Box::new([KnownServer {
            issuer: Box::leak(server.uri().into_boxed_str()),
            service: "Acme",
            ghost_client: None,
            own_secret: crate::signin_clients::SecretNeed::Required,
            secret_in_body: true,
            make_a_client: "https://acme.example/clients",
            authorize_params: &[("access_type", "offline")],
            resources: Box::leak(Box::new([authority(&server)])),
        }]));
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_servers(row);
        let own = OwnClient {
            client_id: "own-client".into(),
            client_secret: Some(CLIENT_SECRET.into()),
            issuer: Some(server.uri()),
        };
        let (url, result) = sign_in_as(&s, &opened, port, &server, Some(own)).await;
        result.unwrap();
        assert_eq!(
            query(&url.unwrap(), "access_type").as_deref(),
            Some("offline")
        );
        let requests = server.received_requests().await.unwrap();
        let token = requests.iter().find(|r| r.url.path() == "/token").unwrap();
        assert!(
            String::from_utf8_lossy(&token.body)
                .contains(&format!("client_secret={CLIENT_SECRET}"))
        );
        assert!(token.headers.get("authorization").is_none());
    }

    #[tokio::test]
    async fn a_known_server_with_no_client_says_where_to_make_one() {
        let server = fake_server(json!({})).await;
        let row: &'static [KnownServer] = Box::leak(Box::new([KnownServer {
            issuer: Box::leak(format!("{}/", server.uri()).into_boxed_str()),
            service: "Acme Cloud",
            ghost_client: None,
            own_secret: crate::signin_clients::SecretNeed::Required,
            secret_in_body: false,
            make_a_client: "https://acme.example/clients",
            authorize_params: &[],
            resources: Box::leak(Box::new([authority(&server)])),
        }]));
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_servers(row);
        let (_, result) = sign_in_as(&s, &opened, port, &server, None).await;
        let said = result.err().unwrap().to_string();
        assert!(
            said.starts_with("Acme Cloud only lets in apps registered with it"),
            "{said}"
        );
        assert!(
            said.contains("Register one of your own with Acme Cloud"),
            "{said}"
        );
        assert_eq!(
            s.needs_client("acme").flatten().map(|k| k.service),
            Some("Acme Cloud")
        );
    }

    #[tokio::test]
    async fn a_server_that_takes_only_the_document_gets_ghosts_url() {
        // No registration offered, and the document names this sign-in's way back.
        let server = fake_server(json!({"client_id_metadata_document_supported": true})).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_document(document_for(port));
        let (url, result) = sign_in_as(&s, &opened, port, &server, None).await;
        result.unwrap();
        assert_eq!(
            query(&url.unwrap(), "client_id").as_deref(),
            Some(crate::signin_clients::CIMD_URL)
        );
        assert_eq!(
            s.storage("acme").client_id().as_deref(),
            Some(crate::signin_clients::CIMD_URL)
        );
    }

    /// Ghost's document, as if it named `port` (the published one names 47823 only).
    fn document_for(port: u16) -> &'static str {
        let mut doc: Value = serde_json::from_str(crate::signin_clients::CIMD_DOCUMENT).unwrap();
        doc["redirect_uris"] = json!([format!("http://127.0.0.1:{port}/callback")]);
        Box::leak(doc.to_string().into_boxed_str())
    }

    #[tokio::test]
    async fn a_server_that_offers_both_is_registered_with_as_today() {
        // Notion, Linear, Atlassian and Sentry advertise both; registration is what works for
        // them today, so it comes first.
        let server = fake_server(json!({
            "client_id_metadata_document_supported": true,
            "registration_endpoint": "BASE/register",
        }))
        .await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "client_id": "ghost-registered",
                "redirect_uris": ["http://127.0.0.1:1/callback"]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_document(document_for(port));
        let (url, result) = sign_in_as(&s, &opened, port, &server, None).await;
        result.unwrap();
        assert_eq!(
            query(&url.unwrap(), "client_id").as_deref(),
            Some("ghost-registered")
        );
    }

    #[tokio::test]
    async fn the_document_is_not_used_for_a_way_back_it_doesnt_name() {
        // GHOST_SIGNIN_PORT set to another port: the published document names 47823 only, so
        // the server would refuse the redirect; Ghost doesn't send it there.
        let server = fake_server(json!({"client_id_metadata_document_supported": true})).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        assert_ne!(port, DEFAULT_PORT);
        let (url, result) = sign_in_as(&s, &opened, port, &server, None).await;
        assert!(url.is_none(), "{url:?}");
        assert!(matches!(result.err(), Some(AppError::NeedsClient(_))));
    }

    #[tokio::test]
    async fn ghosts_own_device_client_signs_in_with_a_code_and_no_secret() {
        let server2 = fake_server(json!({
            "device_authorization_endpoint": "BASE/device/code",
        }))
        .await;
        let base2 = server2.uri();
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dev-1",
                "user_code": "WDJB-MJHT",
                "verification_uri": format!("{base2}/login/device"),
                "interval": 1,
                "expires_in": 60
            })))
            .mount(&server2)
            .await;
        let row: &'static [KnownServer] = Box::leak(Box::new([KnownServer {
            issuer: Box::leak(base2.clone().into_boxed_str()),
            service: "Acme",
            ghost_client: Some(crate::signin_clients::GhostClient {
                client_id: "ghost-device-client",
                auth: ClientAuth::DeviceFlow,
            }),
            own_secret: crate::signin_clients::SecretNeed::Required,
            secret_in_body: true,
            make_a_client: "https://acme.example/clients",
            authorize_params: &[],
            resources: Box::leak(Box::new([authority(&server2)])),
        }]));
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, _) = browser(dir.path());
        let s = s.with_servers(row);
        let spec = signs_in("acme", &format!("{base2}/mcp"));
        let auth = Arc::new(s.auth_for(&spec, true).unwrap());
        let signing_in = tokio::spawn({
            let auth = auth.clone();
            async move { auth.client(reqwest::Client::new(), false, None).await }
        });
        let started = Instant::now();
        while s.pending_code("acme").is_none() && started.elapsed() < Duration::from_secs(5) {
            if signing_in.is_finished() {
                panic!("ended early: {:?}", signing_in.await.unwrap().err());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(s.pending_code("acme").as_deref(), Some("WDJB-MJHT"));
        assert_eq!(
            opened.lock().unwrap().first().cloned(),
            Some(format!("{base2}/login/device"))
        );
        let client = tokio::time::timeout(Duration::from_secs(10), signing_in)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");
        assert!(s.pending_code("acme").is_none());
        assert_eq!(
            s.storage("acme").client_id().as_deref(),
            Some("ghost-device-client")
        );
        let requests = server2.received_requests().await.unwrap();
        let token = requests.iter().find(|r| r.url.path() == "/token").unwrap();
        let body = String::from_utf8_lossy(&token.body);
        assert!(body.contains("device_code=dev-1"), "{body}");
        assert!(!body.contains("client_secret"), "{body}");
    }

    #[tokio::test]
    async fn a_stored_sign_in_of_the_persons_client_refreshes_with_its_secret() {
        let server = fake_server(json!({})).await;
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path()).with_port(free_port());
        let store = s.storage("acme");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            json!({"tokens": {"access_token": "old", "token_type": "Bearer",
                              "expires_in": 1, "refresh_token": "refresh-0"},
                   "token_received_at": 1,
                   "issuer": server.uri(),
                   "client": {"client_id": "own-client"}})
            .to_string(),
        )
        .unwrap();
        let spec = signs_in("acme", &format!("{}/mcp", server.uri()));
        let auth = s.auth_for(&spec, false).unwrap();
        let own = OwnClient {
            client_id: "own-client".into(),
            client_secret: Some(CLIENT_SECRET.into()),
            issuer: Some(server.uri()),
        };
        let client = auth
            .client(reqwest::Client::new(), false, Some(own))
            .await
            .unwrap();
        assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");
        let requests = server.received_requests().await.unwrap();
        let token = requests.iter().find(|r| r.url.path() == "/token").unwrap();
        let sent = format!(
            "{} {:?}",
            String::from_utf8_lossy(&token.body),
            token.headers.get("authorization")
        );
        assert!(sent.contains("refresh_token=refresh-0"), "{sent}");
        assert!(
            sent.contains(CLIENT_SECRET) || sent.contains("Basic "),
            "{sent}"
        );
        assert!(
            !std::fs::read_to_string(&store.path)
                .unwrap()
                .contains(CLIENT_SECRET)
        );
    }

    #[tokio::test]
    async fn a_refresh_the_server_refuses_waits_on_the_person_to_sign_in_again() {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path()).with_port(free_port());
        let store = s.storage("acme");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            json!({"tokens": {"access_token": "old", "token_type": "Bearer",
                              "expires_in": 1, "refresh_token": "refresh-0"},
                   "token_received_at": 1,
                   "client": {"client_id": "ghost-1"}})
            .to_string(),
        )
        .unwrap();
        let spec = signs_in("acme", &format!("{base}/mcp"));
        let auth = s.auth_for(&spec, false).unwrap();
        let client = auth
            .client(reqwest::Client::new(), false, None)
            .await
            .unwrap();
        let err = client.get_access_token().await.unwrap_err();
        assert!(
            matches!(auth_error(err), AppError::NeedsSignIn(_)),
            "a refused refresh is a sign-in again, not an error's kind"
        );
    }

    /// A server's host and port as a row's `resources` names it, for a test's fake.
    fn authority(server: &MockServer) -> &'static str {
        Box::leak(server.address().to_string().into_boxed_str())
    }

    /// An app somewhere else that names `issuer`'s server as its authorization server (the
    /// review of 2026-11-15): it must never get a token minted for Ghost's own client.
    async fn app_naming(issuer: &MockServer) -> MockServer {
        let app = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resource": format!("{}/mcp", app.uri()),
                "authorization_servers": [issuer.uri()],
            })))
            .mount(&app)
            .await;
        app
    }

    fn row_with_ghosts_client(issuer: &MockServer, auth: ClientAuth) -> &'static [KnownServer] {
        Box::leak(Box::new([KnownServer {
            issuer: Box::leak(issuer.uri().into_boxed_str()),
            service: "Acme",
            ghost_client: Some(crate::signin_clients::GhostClient {
                client_id: "ghost-own-client",
                auth,
            }),
            own_secret: crate::signin_clients::SecretNeed::Required,
            secret_in_body: true,
            make_a_client: "https://acme.example/clients",
            authorize_params: &[("access_type", "offline")],
            resources: Box::leak(Box::new([authority(issuer)])),
        }]))
    }

    fn paths_called(requests: &[wiremock::Request]) -> Vec<String> {
        requests.iter().map(|r| r.url.path().to_string()).collect()
    }

    #[tokio::test]
    async fn an_app_elsewhere_cannot_borrow_ghosts_device_client() {
        let issuer = fake_server(json!({
            "device_authorization_endpoint": "BASE/device/code",
        }))
        .await;
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dev-1", "user_code": "WDJB-MJHT",
                "verification_uri": format!("{}/login/device", issuer.uri()),
                "interval": 1, "expires_in": 60
            })))
            .mount(&issuer)
            .await;
        let evil = app_naming(&issuer).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_servers(row_with_ghosts_client(&issuer, ClientAuth::DeviceFlow));
        let (url, result) = sign_in_as(&s, &opened, port, &evil, None).await;
        assert!(url.is_none() && s.pending_code("acme").is_none());
        assert!(matches!(result.err(), Some(AppError::NeedsClient(_))));
        let called = paths_called(&issuer.received_requests().await.unwrap());
        assert!(
            !called.iter().any(|p| p == "/device/code" || p == "/token"),
            "{called:?}"
        );
        assert!(s.storage("acme").get_tokens().is_none());
    }

    #[tokio::test]
    async fn an_app_elsewhere_cannot_borrow_ghosts_pkce_client_or_its_parameters() {
        let issuer = fake_server(json!({})).await;
        let row = row_with_ghosts_client(&issuer, ClientAuth::Pkce);
        // At the server's own app: Ghost's client and the row's parameters.
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_servers(row);
        let (url, result) = sign_in_as(&s, &opened, port, &issuer, None).await;
        result.unwrap();
        let url = url.unwrap();
        assert_eq!(
            query(&url, "client_id").as_deref(),
            Some("ghost-own-client")
        );
        assert_eq!(query(&url, "access_type").as_deref(), Some("offline"));
        // At an app elsewhere naming the same server: neither.
        let evil = app_naming(&issuer).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let s = s.with_servers(row);
        let (url, result) = sign_in_as(&s, &opened, port, &evil, None).await;
        assert!(url.is_none(), "{url:?}");
        let said = result.err().unwrap().to_string();
        assert!(
            said.starts_with("Acme doesn't let Ghost register itself"),
            "{said}"
        );
    }

    #[tokio::test]
    async fn the_persons_client_goes_only_to_the_server_it_was_given_for() {
        let given_for = fake_server(json!({})).await;
        let other = fake_server(json!({})).await;
        let moved = app_naming(&other).await;
        let dir = tempfile::tempdir().unwrap();
        let (s, opened, port) = browser(dir.path());
        let own = OwnClient {
            client_id: "own-client".into(),
            client_secret: Some(CLIENT_SECRET.into()),
            issuer: Some(given_for.uri()),
        };
        let (url, result) = sign_in_as(&s, &opened, port, &moved, Some(own.clone())).await;
        assert!(url.is_none());
        let err = result.err().unwrap();
        assert!(matches!(err, AppError::NeedsClient(_)), "{err}");
        assert!(
            err.to_string()
                .starts_with("Acme now signs in somewhere other than where you gave Ghost"),
            "{err}"
        );
        assert!(!err.to_string().contains(CLIENT_SECRET));
        for r in other.received_requests().await.unwrap() {
            assert!(!String::from_utf8_lossy(&r.body).contains(CLIENT_SECRET));
            assert_ne!(r.url.path(), "/token");
        }
        // with no server recorded at all, the same
        let unbound = OwnClient {
            issuer: None,
            ..own
        };
        let (_, result) = sign_in_as(&s, &opened, port, &moved, Some(unbound)).await;
        assert!(matches!(result.err(), Some(AppError::NeedsClient(_))));
    }

    /// Ghost's device client at `server`'s own app, with the device endpoint answering.
    async fn device_server() -> (MockServer, &'static [KnownServer]) {
        let server =
            fake_server(json!({"device_authorization_endpoint": "BASE/device/code"})).await;
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "dev-1", "user_code": "WDJB-MJHT",
                "verification_uri": format!("{}/login/device", server.uri()),
                "interval": 1, "expires_in": 60
            })))
            .mount(&server)
            .await;
        let row = row_with_ghosts_client(&server, ClientAuth::DeviceFlow);
        (server, row)
    }

    async fn device_sign_in(
        s: &SignIns,
        server: &MockServer,
    ) -> Result<AuthClient<reqwest::Client>, AppError> {
        let spec = signs_in("acme", &format!("{}/mcp", server.uri()));
        let auth = s.auth_for(&spec, true).unwrap();
        tokio::time::timeout(
            Duration::from_secs(20),
            auth.client(reqwest::Client::new(), false, None),
        )
        .await
        .expect("the device sign-in ended")
    }

    #[tokio::test]
    async fn a_device_sign_in_rides_out_a_server_that_stumbles() {
        let (server, row) = device_server().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("<html>busy</html>"))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path())
            .with_port(free_port())
            .with_servers(row);
        let client = device_sign_in(&s, &server).await.unwrap();
        assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");
        let tries = paths_called(&server.received_requests().await.unwrap())
            .iter()
            .filter(|p| *p == "/token")
            .count();
        assert_eq!(tries, 3);
        assert!(s.pending_code("acme").is_none());
    }

    #[tokio::test]
    async fn a_device_sign_in_that_fails_takes_its_code_off_the_screen() {
        let (server, row) = device_server().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string("not json"))
            .with_priority(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let s = SignIns::new(dir.path())
            .with_port(free_port())
            .with_servers(row);
        assert!(device_sign_in(&s, &server).await.is_err());
        assert!(
            s.pending_code("acme").is_none(),
            "the code stayed on screen"
        );
        assert!(s.pending_url("acme").is_none());
    }
}
