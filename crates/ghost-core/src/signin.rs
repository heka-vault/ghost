//! Signing in to an app (ADR 0034): OAuth 2.1 with PKCE, the way the MCP authorization spec
//! asks, through the operator's browser.
//!
//! Ghost has no public address, so it cannot publish a client ID metadata document; it
//! registers itself with the app's authorization server (dynamic client registration),
//! which `rmcp` does when the server offers it. The browser comes back to a loopback
//! address, `http://127.0.0.1:<port>/callback`, which listens only while a sign-in is
//! open, on 127.0.0.1 only, for one path. The port is fixed (`GHOST_SIGNIN_PORT`) because
//! a registration names its redirect exactly.
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
    app: String,
    url: Option<String>,
    sender: Option<oneshot::Sender<Result<HashMap<String, String>, AppError>>>,
    receiver: Option<oneshot::Receiver<Result<HashMap<String, String>, AppError>>>,
}

#[derive(Default)]
struct State {
    /// One at a time: a new sign-in replaces the open one.
    flow: Option<Flow>,
    server: Option<tokio::task::JoinHandle<()>>,
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
}

impl SignIns {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            on_url: None,
            state: Arc::new(Mutex::new(State::default())),
        }
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
        if state.flow.as_ref().is_some_and(|f| f.app == app) {
            Self::cancel_flow(&mut state);
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
            state.flow = Some(Flow {
                app: app.to_string(),
                url: Some(url.to_string()),
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
    /// the app refused the stored tokens, so forget them and sign in again.
    pub async fn client(
        &self,
        http: reqwest::Client,
        fresh: bool,
    ) -> Result<AuthClient<reqwest::Client>, AppError> {
        let storage = self.signins.storage(&self.spec.name);
        let url = self.spec.url.as_deref().unwrap_or("");
        let mut manager = AuthorizationManager::new_allowing_private_addresses(url)
            .await
            .map_err(auth_error)?;
        manager.set_credential_store(storage.clone());
        if fresh {
            // The tokens were refused: the registration may stand, the tokens may not.
            let data = storage.read();
            if data.contains_key("tokens") {
                let mut data = data;
                data.remove("tokens");
                storage.write(&data).map_err(AppError::Io)?;
            }
        } else if manager.initialize_from_store().await.map_err(auth_error)? {
            return Ok(AuthClient::new(http, manager));
        }
        if !self.interactive {
            return Err(AppError::NeedsSignIn(format!(
                "{} needs you to sign in again",
                self.spec.shown()
            )));
        }
        let resolution = manager.resolve_metadata().await.map_err(auth_error)?;
        manager.set_metadata(resolution.metadata);
        let request =
            AuthorizationRequest::new(self.signins.redirect_uri()).with_client_name(CLIENT_NAME);
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| auth_error(e))?;
        self.redirect_handler(&session.auth_url).await?;
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

/// An authorization error by kind only: never a body, a token or an address.
fn auth_error(e: AuthError) -> AppError {
    let kind = match &e {
        AuthError::AuthorizationRequired => return AppError::NeedsSignIn("needs sign-in".into()),
        AuthError::AuthorizationFailed(_) => "AuthorizationFailed",
        AuthError::TokenExchangeFailed(_) => "TokenExchangeFailed",
        AuthError::TokenRefreshFailed(_) => "TokenRefreshFailed",
        AuthError::TokenRefreshRejected(_) => "TokenRefreshRejected",
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
        http: reqwest::Client,
        interactive: bool,
        fresh: bool,
    ) -> Result<Option<AuthClient<reqwest::Client>>, AppError> {
        match SignIns::auth_for(self, spec, interactive) {
            None => Ok(None),
            Some(auth) => auth.client(http, fresh).await.map(Some),
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
        let client = auth.client(reqwest::Client::new(), false).await.unwrap();
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
            async move { auth.client(reqwest::Client::new(), false).await }
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
}
