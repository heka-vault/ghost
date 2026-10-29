//! `ghostd push <hook> <event> [json]`: a signed delivery to the local webhook listener
//! (ADR 0027), for a local sender: `ghostd push jobs done '{"job": "backup", "ok": false}'`
//! at the end of a long job. The hook must use `hmac`; its password comes from the vault.

use std::io::Write;
use std::time::Duration;

use ghost_core::folder::Folder;
use ghost_core::hooks::{H_EVENT, H_ID, H_SIG, H_TIME, Scheme, Secrets, sign_hmac};
use ghost_core::time::utcnow;

use crate::config::GhostdSettings;

pub const USAGE: &str = "usage: ghostd push <hook> <event> [json]";
const TIMEOUT: Duration = Duration::from_secs(10);

/// What a failed request is called in one word, never its text (which names the URL).
fn kind(e: &reqwest::Error) -> &'static str {
    if e.is_connect() {
        "ConnectError"
    } else if e.is_timeout() {
        "TimeoutError"
    } else {
        "HTTPError"
    }
}

/// The exit code: 0 accepted, 1 refused or unreachable, 2 a usage or setup problem.
pub fn push_cli(
    args: &[String],
    cfg: &GhostdSettings,
    secrets: &dyn Secrets,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    if args.len() < 2 || args[0].contains('/') {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    let folder = Folder::new(&cfg.config_dir);
    let Some(hook) = folder
        .hooks
        .get(&args[0])
        .filter(|h| h.verify == Scheme::Hmac)
    else {
        let _ = writeln!(err, "no hmac hook {} is declared", args[0]);
        return 2;
    };
    let name = hook.secret_name();
    let Some(secret) = secrets.secret(&name).filter(|s| !s.is_empty()) else {
        let _ = writeln!(err, "{name} is not set");
        return 2;
    };
    if cfg.hooks_listen.trim().is_empty() {
        let _ = writeln!(err, "GHOST_HOOKS_LISTEN is not set");
        return 2;
    }
    let body = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "{}".to_string())
        .into_bytes();
    let ts = utcnow().timestamp().to_string();
    let delivery = uuid::Uuid::new_v4().simple().to_string();
    let address = cfg
        .hooks_listen
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let signature = sign_hmac(&secret, &ts, &delivery, &body);
    let sent = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .and_then(|client| {
            client
                .post(format!("http://{address}{}", hook.path()))
                .header("content-type", "application/json")
                .header(H_TIME, ts)
                .header(H_ID, delivery)
                .header(H_EVENT, args[1].as_str())
                .header(H_SIG, signature)
                .body(body)
                .send()
        });
    match sent {
        Err(e) => {
            let _ = writeln!(
                err,
                "could not reach the hooks listener on {address} ({})",
                kind(&e)
            );
            1
        }
        Ok(r) if r.status().as_u16() < 400 => {
            let _ = writeln!(out, "accepted");
            0
        }
        Ok(r) => {
            let _ = writeln!(out, "refused ({})", r.status().as_u16());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn push_with_the_listener_down_says_so_in_one_line() {
        // a port nothing listens on
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("hooks.json"),
            json!({"jobs": crate::hooks_listener::tests::jobs()}).to_string(),
        )
        .unwrap();
        let env: HashMap<String, String> = HashMap::from([
            (
                "GHOST_CONFIG_DIR".to_string(),
                dir.path().to_string_lossy().into_owned(),
            ),
            (
                "GHOST_HOOKS_LISTEN".to_string(),
                format!("127.0.0.1:{port}"),
            ),
        ]);
        let cfg = GhostdSettings::from_env(&env).unwrap();
        let secrets: HashMap<String, String> =
            HashMap::from([("JOB_SECRET".to_string(), "k".to_string())]);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = strings(&["jobs", "done", r#"{"job": "backup", "ok": false}"#]);
        assert_eq!(push_cli(&args, &cfg, &secrets, &mut out, &mut err), 1);
        let err = String::from_utf8(err).unwrap();
        let line = err.trim();
        assert!(
            !line.contains('\n') && line.contains("could not reach"),
            "{err}"
        );
        // the older <department>/<hook> form is not a hook name
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = strings(&["ghost/jobs", "done"]);
        assert_eq!(push_cli(&args, &cfg, &secrets, &mut out, &mut err), 2);
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("usage: ghostd push <hook>")
        );
        // the password has to be there
        let none: HashMap<String, String> = HashMap::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = strings(&["jobs", "done"]);
        assert_eq!(push_cli(&args, &cfg, &none, &mut out, &mut err), 2);
        assert_eq!(
            String::from_utf8(err).unwrap().trim(),
            "JOB_SECRET is not set"
        );
    }
}
