//! Error reports in the daemon (ADR 0067): the process's reporter, its panic hook, the
//! control API's side of the switch, and `ghostd reports on|off|status|test`.
//!
//! The daemon's reporter redacts every password value the vault holds before anything is
//! spooled (`ghost_core::redact::redact_text`, ADR 0036), on top of the report's own
//! scrubbing. The switch, the kept reports and the sender are the operator's: nothing here
//! is a tool of Ghost's own app, a route of the phone or of the listener for outside tools.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use ghost_api::reports::{KeptReportView, ReportsPort, ReportsView};
use ghost_reports::{Component, Context, Reporter, Scrubber, key_from, url_from};

/// The daemon's reporter over Ghost's data folder, with the vault's redaction.
pub fn reporter(data_dir: &Path, env: impl Fn(&str) -> Option<String>) -> Reporter {
    Reporter::new(
        data_dir,
        key_from(&env),
        &url_from(&env),
        Context::detect(crate::build_info::version(), Component::Daemon),
        Scrubber::from_env().with_redact(Arc::new(ghost_core::redact::redact_text)),
    )
}

/// Install it for the process: [`ghost_reports::failure`] from anywhere, the panic hook,
/// and the running sender (when there is a runtime). A build with no key installs a
/// reporter that does nothing, so the desktop can say so.
pub fn start(r: Arc<Reporter>) {
    ghost_reports::install(r.clone());
    ghost_reports::panic::install(r.clone());
    if r.available()
        && let Ok(h) = tokio::runtime::Handle::try_current()
    {
        h.spawn(r.run());
    }
}

pub fn view(r: &Reporter) -> ReportsView {
    ReportsView {
        available: r.available(),
        send: r.switched_on(),
        install_id: r.current_install_id().filter(|_| r.on()),
        pending: r.pending(),
        kept: r.kept().into_iter().map(kept_view).collect(),
    }
}

fn kept_view(k: ghost_reports::Kept) -> KeptReportView {
    let p = &k.event["properties"];
    let x = &p["$exception_list"][0];
    let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let (t, v) = (s(&x["type"]), s(&x["value"]));
    KeptReportView {
        sent_at: k.sent_at.to_rfc3339(),
        kind: s(&p["ghost_kind"]),
        component: s(&p["ghost_component"]),
        title: if v.is_empty() { t } else { format!("{t}: {v}") },
        body: k.event,
    }
}

/// The control API's side.
pub struct ReportsDesk(pub Arc<Reporter>);

impl ReportsPort for ReportsDesk {
    fn view(&self) -> ReportsView {
        view(&self.0)
    }

    fn set(&self, send: bool) -> Result<ReportsView, String> {
        self.0
            .set(send)
            .map_err(|e| format!("the switch was not saved: {e}"))?;
        tracing::info!("error reports: {}", if send { "on" } else { "off" });
        Ok(view(&self.0))
    }
}

/// `ghostd reports on|off|status|test`.
pub async fn cli(args: &[String], r: &Reporter, out: &mut dyn Write) -> i32 {
    let usage = "ghostd reports on|off|status|test";
    let say = |out: &mut dyn Write, s: &str| {
        let _ = writeln!(out, "{s}");
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let v = view(r);
            say(
                out,
                &format!(
                    "error reports: {}",
                    match (v.available, v.send) {
                        (false, _) => "this build has no key; nothing is ever sent",
                        (true, true) => "on",
                        (true, false) => "off",
                    }
                ),
            );
            if let Some(id) = &v.install_id {
                say(out, &format!("install id: {id}"));
            }
            say(out, &format!("waiting: {}", v.pending));
            say(
                out,
                &format!(
                    "kept: {} in {}",
                    v.kept.len(),
                    r.dir().join("sent").display()
                ),
            );
            for k in v.kept.iter().take(5) {
                say(
                    out,
                    &format!("  {}  {}  {}", k.sent_at, k.component, k.title),
                );
            }
            0
        }
        Some(word @ ("on" | "off")) => match r.set(word == "on") {
            Ok(()) => {
                say(out, &format!("error reports: {word}"));
                if word == "off" {
                    say(
                        out,
                        "nothing is sent or kept waiting; the install id is deleted",
                    );
                }
                0
            }
            Err(e) => {
                say(out, &format!("not saved: {e}"));
                1
            }
        },
        Some("test") => match r.send_test().await {
            Ok(body) => {
                say(out, "sent:");
                say(
                    out,
                    &serde_json::to_string_pretty(&body).unwrap_or_default(),
                );
                0
            }
            Err(e) => {
                say(out, &e);
                1
            }
        },
        Some(other) => {
            say(out, &format!("unknown: ghostd reports {other}; {usage}"));
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_core::passwords::{MemoryKeyring, Passwords};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn stub() -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn a_password_in_the_vault_never_reaches_a_report() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Passwords::new(
            tmp.path().join("passwords.vault"),
            Arc::new(MemoryKeyring::new()),
        );
        assert!(vault.open());
        let value = "vault-held-Zq8xT2v";
        vault.set("GITHUB_TOKEN", value).unwrap();
        let s = stub().await;
        let url = s.uri();
        let r = reporter(tmp.path(), |k| match k {
            "GHOST_POSTHOG_KEY" => Some("k".into()),
            "GHOST_REPORTS_URL" => Some(url.clone()),
            _ => None,
        });
        r.report(&ghost_reports::What::Panic {
            message: format!("the app refused {value}"),
            file: Some("crates/ghostd/src/x.rs".into()),
            line: Some(1),
            col: Some(1),
            thread: None,
            backtrace: String::new(),
        });
        r.report(&ghost_reports::What::Page(ghost_reports::PageError {
            page: "main".into(),
            source: ghost_reports::PageSource::Error,
            error_type: "TypeError".into(),
            message: format!("{value} is not a function"),
            stack: format!("f@tauri://localhost/assets/{value}.js:1:1"),
        }));
        assert_eq!(r.send_pending().await, 2);
        let wire: String = s
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|q| String::from_utf8_lossy(&q.body).into_owned())
            .collect();
        assert!(!wire.contains(value), "{wire}");
        assert!(wire.contains("[password: GITHUB_TOKEN]"), "{wire}");
        // Nor on disk.
        for k in r.kept() {
            assert!(!k.event.to_string().contains(value));
        }
    }

    #[tokio::test]
    async fn the_cli_turns_it_off_and_on_and_says_what_a_test_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let s = stub().await;
        let url = s.uri();
        let r = reporter(tmp.path(), |k| match k {
            "GHOST_POSTHOG_KEY" => Some("phc_secret_key".into()),
            "GHOST_REPORTS_URL" => Some(url.clone()),
            _ => None,
        });
        let mut out = Vec::new();
        assert_eq!(cli(&["off".into()], &r, &mut out).await, 0);
        assert_eq!(cli(&["test".into()], &r, &mut out).await, 1);
        assert!(s.received_requests().await.unwrap().is_empty());
        assert_eq!(cli(&["on".into()], &r, &mut out).await, 0);
        assert_eq!(cli(&["test".into()], &r, &mut out).await, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("\"ghost_kind\": \"test\""), "{text}");
        assert!(!text.contains("phc_secret_key"), "the key is never printed");
        assert_eq!(s.received_requests().await.unwrap().len(), 1);
        let mut out = Vec::new();
        assert_eq!(cli(&[], &r, &mut out).await, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("error reports: on")
        );
    }

    #[tokio::test]
    async fn a_build_with_no_key_says_so_and_sends_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let r = reporter(tmp.path(), |_| None);
        let mut out = Vec::new();
        assert_eq!(cli(&["test".into()], &r, &mut out).await, 1);
        let mut out2 = Vec::new();
        cli(&[], &r, &mut out2).await;
        assert!(String::from_utf8(out2).unwrap().contains("no key"));
        assert!(!view(&r).available);
    }
}
