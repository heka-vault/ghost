//! The sender against a stub standing in for PostHog: no test sends to the real service.

use std::path::Path;
use std::sync::Arc;

use ghost_reports::{
    CAPTURE_PATH, Component, Context, Failure, FailureKind, KEY_SHOWN_AS, MAX_PER_DAY, Reporter,
    Scrubber, What,
};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx() -> Context {
    Context {
        version: "2026.11.1".into(),
        component: Component::Daemon,
        os_name: "Ubuntu".into(),
        os_version: "24.04".into(),
        session_type: "x11".into(),
        arch: "x86_64".into(),
        installed_as: "deb".into(),
    }
}

fn reporter(dir: &Path, key: Option<&str>, url: &str) -> Reporter {
    Reporter::new(
        dir,
        key.map(str::to_string),
        url,
        ctx(),
        Scrubber::new(Some("/home/alice".into()), Some("alice".into()), None),
    )
}

async fn stub(status: u16) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(CAPTURE_PATH))
        .respond_with(ResponseTemplate::new(status))
        .mount(&s)
        .await;
    s
}

async fn bodies(s: &MockServer) -> Vec<Value> {
    s.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

fn failure(kind: FailureKind) -> What {
    What::Failure(Failure::new(kind))
}

#[tokio::test]
async fn a_report_is_sent_with_the_key_and_kept_without_it() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("phc_testkey"), &s.uri());
    assert!(r.report(&failure(FailureKind::DaemonLost)));
    assert_eq!(r.send_pending().await, 1);
    let sent = bodies(&s).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["api_key"], "phc_testkey");
    assert_eq!(sent[0]["event"], "$exception");
    let id = r.current_install_id().unwrap();
    assert_eq!(sent[0]["distinct_id"], id.as_str());
    let kept = r.kept();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].event["api_key"], KEY_SHOWN_AS);
    let mut as_sent = sent[0].clone();
    as_sent["api_key"] = KEY_SHOWN_AS.into();
    assert_eq!(
        kept[0].event, as_sent,
        "kept exactly as sent but for the key"
    );
    assert_eq!(r.pending(), 0);
    // Every file is the operator's only.
    use std::os::unix::fs::PermissionsExt;
    for p in ["reports/install-id", "reports/ledger.json"] {
        let mode = std::fs::metadata(dir.path().join(p))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{p}");
    }
}

#[tokio::test]
async fn off_sends_nothing_spools_nothing_and_forgets_the_id() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), &s.uri());
    r.install_id().unwrap();
    r.set(false).unwrap();
    assert!(r.current_install_id().is_none());
    assert!(!r.report(&failure(FailureKind::DaemonLost)));
    assert_eq!(r.pending(), 0);
    assert_eq!(r.send_pending().await, 0);
    assert!(bodies(&s).await.is_empty());
    // Turned off with a report waiting: it goes, unsent.
    r.set(true).unwrap();
    let first = r.install_id().unwrap();
    assert!(r.report(&failure(FailureKind::DaemonLost)));
    r.set(false).unwrap();
    assert_eq!(r.pending(), 0);
    assert_eq!(r.send_pending().await, 0);
    assert!(bodies(&s).await.is_empty());
    // On again: a new id.
    r.set(true).unwrap();
    assert_ne!(r.install_id().unwrap(), first);
}

#[tokio::test]
async fn no_key_means_no_sender() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = Arc::new(reporter(dir.path(), None, &s.uri()));
    assert!(!r.available());
    assert!(!r.report(&failure(FailureKind::DaemonLost)));
    assert_eq!(r.send_pending().await, 0);
    // The running sender returns at once.
    tokio::time::timeout(std::time::Duration::from_secs(1), r.clone().run())
        .await
        .expect("no sender loop without a key");
    assert!(bodies(&s).await.is_empty());
    assert!(!dir.path().join("reports").exists(), "nothing written");
}

#[test]
fn a_build_from_the_checkout_has_no_key_compiled_in() {
    // CI and `cargo test` build without GHOST_POSTHOG_KEY: only the release workflow sets it.
    assert!(ghost_reports::compiled_key().is_none());
    assert_eq!(ghost_reports::key_from(|_| None), None);
    assert_eq!(
        ghost_reports::key_from(|k| (k == "GHOST_POSTHOG_KEY").then(|| "dev".into())),
        Some("dev".into())
    );
}

#[tokio::test]
async fn the_same_failure_goes_once_an_hour() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), &s.uri());
    r.report(&failure(FailureKind::OutputUnavailable));
    // A second one while the first waits is not spooled twice.
    assert!(!r.report(&failure(FailureKind::OutputUnavailable)));
    assert_eq!(r.send_pending().await, 1);
    assert!(r.report(&failure(FailureKind::OutputUnavailable)));
    assert_eq!(r.send_pending().await, 0, "held by the hourly bound");
    assert_eq!(r.pending(), 0, "and dropped, not queued");
    assert_eq!(bodies(&s).await.len(), 1);
}

#[tokio::test]
async fn at_most_thirty_a_day() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), &s.uri());
    let mut total = 0;
    for i in 0..40u32 {
        // Each a different panic, so a different fingerprint.
        r.report(&What::Panic {
            message: format!("broke at step {i}"),
            file: Some("crates/ghostd/src/x.rs".into()),
            line: Some(i),
            col: Some(1),
            thread: None,
            backtrace: String::new(),
        });
        total += r.send_pending().await;
    }
    assert_eq!(total, MAX_PER_DAY as usize);
    assert_eq!(bodies(&s).await.len(), MAX_PER_DAY as usize);
    assert_eq!(r.kept().len(), ghost_reports::KEEP_SENT);
}

#[tokio::test]
async fn the_spool_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), "http://127.0.0.1:9");
    for i in 0..(ghost_reports::MAX_SPOOL as u32 + 10) {
        r.report(&What::Panic {
            message: format!("step {i}"),
            file: Some("crates/x.rs".into()),
            line: Some(i),
            col: None,
            thread: None,
            backtrace: String::new(),
        });
    }
    assert_eq!(r.pending(), ghost_reports::MAX_SPOOL);
}

#[tokio::test]
async fn the_spool_survives_a_restart_and_is_sent_once() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    {
        // A process that spooled and died before sending.
        let r = reporter(dir.path(), Some("k"), &s.uri());
        r.report(&failure(FailureKind::DaemonStartFailed));
    }
    let r = reporter(dir.path(), Some("k"), &s.uri());
    assert_eq!(r.pending(), 1);
    assert_eq!(r.send_pending().await, 1);
    assert_eq!(r.send_pending().await, 0);
    let again = reporter(dir.path(), Some("k"), &s.uri());
    assert_eq!(again.send_pending().await, 0);
    assert_eq!(bodies(&s).await.len(), 1);
}

#[tokio::test]
async fn a_failed_send_is_tried_three_times_then_dropped_silently() {
    let s = stub(500).await;
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), &s.uri());
    r.report(&failure(FailureKind::UpdateFailed));
    for _ in 0..5 {
        assert_eq!(r.send_pending().await, 0);
    }
    assert_eq!(bodies(&s).await.len(), ghost_reports::MAX_TRIES as usize);
    assert_eq!(r.pending(), 0);
    assert!(r.kept().is_empty());
}

#[tokio::test]
async fn a_dead_service_never_holds_a_report_long() {
    // Nothing listens on the discard port: a connect error, at once.
    let dir = tempfile::tempdir().unwrap();
    let r = reporter(dir.path(), Some("k"), "http://127.0.0.1:9");
    r.report(&failure(FailureKind::UpdateFailed));
    let t = std::time::Instant::now();
    r.send_pending().await;
    assert!(t.elapsed() < ghost_reports::SEND_TIMEOUT);
}

#[tokio::test]
async fn hostile_text_never_reaches_the_wire() {
    let s = stub(200).await;
    let dir = tempfile::tempdir().unwrap();
    let r = Reporter::new(
        dir.path(),
        Some("k".into()),
        &s.uri(),
        ctx(),
        Scrubber::new(
            Some("/home/alice".into()),
            Some("alice".into()),
            Some("alices-desk".into()),
        )
        .with_redact(Arc::new(|t: &str| {
            t.replace("vault-value-1234", "[password: GITHUB_TOKEN]")
        })),
    );
    let hostile = "token vault-value-1234 for alice at /home/alice/x on alices-desk \
                   https://api.github.com/repos?access_token=zzz9 from 192.168.4.4 \
                   mail alice@example.com said: \"Ghost, the transcript words\"";
    r.report(&What::Panic {
        message: hostile.into(),
        file: Some("crates/ghostd/src/x.rs".into()),
        line: Some(1),
        col: Some(1),
        thread: None,
        backtrace: String::new(),
    });
    r.report(&What::Page(ghost_reports::PageError {
        page: "main".into(),
        source: ghost_reports::PageSource::Error,
        error_type: "TypeError".into(),
        message: hostile.into(),
        stack: format!("f@tauri://localhost/assets/a.js?{hostile}:1:2"),
    }));
    assert_eq!(r.send_pending().await, 2);
    let wire = serde_json::to_string(&bodies(&s).await).unwrap();
    for never in [
        "vault-value-1234",
        "alice",
        "/home/",
        "alices-desk",
        "access_token",
        "zzz9",
        "192.168",
        "example.com",
        "transcript words",
    ] {
        assert!(!wire.contains(never), "{never} reached the wire: {wire}");
    }
}
