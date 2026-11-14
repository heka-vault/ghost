//! The panic hook writes the spool and touches no network: its own process, since a hook
//! is the whole process's.

use std::sync::Arc;

use ghost_reports::{CAPTURE_PATH, Component, Context, Reporter, Scrubber};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_panic_is_spooled_without_the_network_and_sent_by_the_next_start() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = Context::detect("2026.11.1", Component::Daemon);
    let r = Arc::new(Reporter::new(
        dir.path(),
        Some("k".into()),
        &s.uri(),
        ctx.clone(),
        Scrubber::from_env(),
    ));
    ghost_reports::panic::install(r.clone());
    let joined = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| {
            let v: Vec<u8> = Vec::new();
            std::hint::black_box(v[3]); // index out of bounds
        })
        .unwrap()
        .join();
    assert!(joined.is_err());
    assert_eq!(r.pending(), 1, "the hook spooled it");
    assert!(
        s.received_requests().await.unwrap().is_empty(),
        "and sent nothing itself"
    );

    // The next start sends it.
    let next = Reporter::new(
        dir.path(),
        Some("k".into()),
        &s.uri(),
        ctx,
        Scrubber::from_env(),
    );
    assert_eq!(next.send_pending().await, 1);
    let got = s.received_requests().await.unwrap();
    assert_eq!(got[0].url.path(), CAPTURE_PATH);
    let body: serde_json::Value = serde_json::from_slice(&got[0].body).unwrap();
    let x = &body["properties"]["$exception_list"][0];
    assert_eq!(x["type"], "panic");
    assert_eq!(x["value"], "index out of bounds");
    assert_eq!(body["properties"]["ghost_facts"]["thread"], "worker");
    assert!(
        body["properties"]["ghost_facts"]["location"]
            .as_str()
            .unwrap()
            .starts_with("crates/ghost-reports/tests/panic_hook.rs:")
    );
    let frames = x["stacktrace"]["frames"].as_array().unwrap();
    // The nearest of Ghost's own frames is where it broke (std's indexing is below it).
    let own = frames.iter().rev().find(|f| f["in_app"] == true).unwrap();
    assert!(
        own["filename"]
            .as_str()
            .unwrap()
            .ends_with("tests/panic_hook.rs"),
        "{own}"
    );
    assert_eq!(own["lineno"], 31);
    let home = std::env::var("HOME").unwrap_or_default();
    if home.len() > 1 {
        assert!(!body.to_string().contains(&home));
    }
}
