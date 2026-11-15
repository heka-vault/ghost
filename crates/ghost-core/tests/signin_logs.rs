//! What a sign-in with the person's own client writes to the log (ADR 0076, ADR 0036): the
//! way it chose, never the secret.
//!
//! Its own test binary, with one test, so the subscriber is the process's before anything
//! logs. In the library's tests a scoped subscriber missed lines whose call sites another test
//! thread had met first (tracing caches "nobody listens" per call site), under `cargo test`
//! though not under nextest, which made a check that the secret is absent prove nothing. The
//! test also asserts the line it must see, so a log that stays empty fails it.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_core::apps::AppSpec;
use ghost_core::signin::SignIns;
use ghost_core::signin_clients::OwnClient;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "own-client-secret-in-the-log-test-0123";

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn a_sign_in_with_the_persons_client_never_logs_its_secret() {
    let lines = Lines::default();
    let writer = lines.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let server = MockServer::start().await;
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-token", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "refresh-1"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let signins = SignIns::new(dir.path())
        .with_port(port)
        .with_on_url(Arc::new(move |_, url| {
            seen.lock().unwrap().push(url.to_string())
        }));
    let spec = AppSpec {
        sign_in: true,
        ..AppSpec::at("acme", &format!("{base}/mcp"))
    };
    let own = OwnClient {
        client_id: "own-client".into(),
        client_secret: Some(SECRET.into()),
        issuer: Some(base.clone()),
    };
    let auth = Arc::new(signins.auth_for(&spec, true).unwrap());
    let signing_in = tokio::spawn({
        let auth = auth.clone();
        async move { auth.client(reqwest::Client::new(), false, Some(own)).await }
    });
    // The browser: open the address, come back with its state.
    let started = Instant::now();
    let url = loop {
        if let Some(u) = opened.lock().unwrap().first().cloned() {
            break u;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "no sign-in page"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let query: HashMap<String, String> = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!(
                "GET /callback?code=abc&state={} HTTP/1.1\r\nHost: x\r\n\r\n",
                query["state"]
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut page = String::new();
    stream.read_to_string(&mut page).await.unwrap();
    let client = tokio::time::timeout(Duration::from_secs(5), signing_in)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(client.get_access_token().await.unwrap(), "fresh-token");

    let logged = String::from_utf8_lossy(&lines.0.lock().unwrap()).to_string();
    // the line it must see: the log was listening
    assert!(logged.contains("sign-in: acme as own client"), "{logged}");
    assert!(!logged.contains(SECRET), "{logged}");
}
