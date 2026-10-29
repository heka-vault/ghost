//! A tiny client over the Unix socket for the CLI (`ghostd wake|say|sleep|status`), the
//! way the Tauri shell reaches it: hyper over `hyperlocal`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("ghostd not reachable on {0}")]
    Unreachable(PathBuf),
    #[error("{0}")]
    Http(String),
    /// A status the daemon answered that isn't success, with its body.
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("the answer wasn't JSON")]
    NotJson,
    #[error("no answer in {0:?}")]
    Timeout(Duration),
}

/// What one request gave back.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub struct GhostClient {
    socket: PathBuf,
    timeout: Duration,
    http: Client<UnixConnector, Full<Bytes>>,
}

impl GhostClient {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self::with_timeout(socket_path, Duration::from_secs(10))
    }

    pub fn with_timeout(socket_path: impl AsRef<Path>, timeout: Duration) -> Self {
        Self {
            socket: socket_path.as_ref().to_path_buf(),
            timeout,
            http: Client::unix(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// One request; the body is JSON when there is one. Any status comes back as a
    /// `Reply`; only a transport failure is an error.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Reply, ClientError> {
        self.request_with(method, path, body, self.timeout).await
    }

    pub async fn request_with(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Reply, ClientError> {
        let uri: hyper::Uri = Uri::new(&self.socket, path).into();
        let mut req = Request::builder().method(method).uri(uri);
        let bytes = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Bytes::from(serde_json::to_vec(v).map_err(|_| ClientError::NotJson)?)
            }
            None => Bytes::new(),
        };
        let req = req
            .body(Full::new(bytes))
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let res = tokio::time::timeout(timeout, self.http.request(req))
            .await
            .map_err(|_| ClientError::Timeout(timeout))?
            .map_err(|e| {
                if e.is_connect() {
                    ClientError::Unreachable(self.socket.clone())
                } else {
                    ClientError::Http(e.to_string())
                }
            })?;
        let status = res.status();
        let raw = res
            .into_body()
            .collect()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?
            .to_bytes();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).map_err(|_| ClientError::NotJson)?
        };
        Ok(Reply {
            status: status.as_u16(),
            body,
        })
    }

    /// Like `request`, but a status that isn't success is an error (`raise_for_status`).
    async fn expect_ok(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ClientError> {
        let reply = self.request(method, path, body).await?;
        if !reply.ok() {
            return Err(ClientError::Status {
                status: reply.status,
                body: reply.body.to_string(),
            });
        }
        Ok(reply.body)
    }

    pub async fn status(&self) -> Result<Value, ClientError> {
        self.expect_ok(Method::GET, "/status", None).await
    }

    pub async fn wake(&self, text: Option<&str>) -> Result<Value, ClientError> {
        let body = text.map(|t| serde_json::json!({"text": t}));
        self.expect_ok(Method::POST, "/wake", body.as_ref()).await
    }

    pub async fn sleep(&self) -> Result<Value, ClientError> {
        self.expect_ok(Method::POST, "/sleep", None).await
    }

    pub async fn say(&self, text: &str) -> Result<Value, ClientError> {
        let body = serde_json::json!({"text": text});
        self.expect_ok(Method::POST, "/session/say", Some(&body))
            .await
    }

    pub async fn confirm(&self) -> Result<Value, ClientError> {
        self.expect_ok(Method::POST, "/proposals/pending/confirm", None)
            .await
    }

    // passwords (ADR 0036): a value goes in; nothing here reads one back

    pub async fn passwords(&self) -> Result<Value, ClientError> {
        self.expect_ok(Method::GET, "/passwords", None).await
    }

    pub async fn set_password(&self, name: &str, value: &str) -> Result<Reply, ClientError> {
        let body = serde_json::json!({"value": value});
        self.request(Method::PUT, &format!("/passwords/{name}"), Some(&body))
            .await
    }

    pub async fn delete_password(&self, name: &str) -> Result<Reply, ClientError> {
        self.request(Method::DELETE, &format!("/passwords/{name}"), None)
            .await
    }

    pub async fn import_passwords(&self) -> Result<Reply, ClientError> {
        self.request_with(
            Method::POST,
            "/passwords/import",
            None,
            Duration::from_secs(120),
        )
        .await
    }

    pub async fn unlock_passwords(&self) -> Result<Reply, ClientError> {
        self.request_with(
            Method::POST,
            "/passwords/unlock",
            None,
            Duration::from_secs(120),
        )
        .await
    }
}

impl std::fmt::Debug for GhostClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhostClient")
            .field("socket", &self.socket)
            .finish_non_exhaustive()
    }
}

/// The HTTP status the daemon answered, when a `ClientError` carries one.
pub fn status_of(e: &ClientError) -> Option<StatusCode> {
    match e {
        ClientError::Status { status, .. } => StatusCode::from_u16(*status).ok(),
        _ => None,
    }
}
