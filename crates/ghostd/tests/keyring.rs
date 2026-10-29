//! A vault file and no keyring that can open it: `ghostd run` starts and waits for the
//! keyring (ADR 0036), and `ghostd check` says it waits, each from inside its own
//! runtime. The first `make dev` on a machine with a vault file panicked here instead
//! ("Cannot start a runtime from within a runtime"): the blocking Secret Service client
//! drives the bus on a tokio runtime of its own, and the daemon called it from a thread of
//! its own runtime.
//!
//! Isolated from the operator: Ghost's folders in the temp dir and a session bus address
//! that points nowhere, so their files, their vault and their keyring are never reached.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ghost_core::passwords::VAULT_FILE;
use ghost_core::{MemoryKeyring, Passwords};

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");
const NO_BUS: &str = "unix:path=/nonexistent/ghost-test-bus";

/// A sealed vault with one password in it, its key in a keyring this process forgets: to
/// the daemon under test, a vault whose keyring can't be reached.
fn seal_a_vault(dir: &Path) {
    let vault = Passwords::new(dir.join(VAULT_FILE), Arc::new(MemoryKeyring::new()));
    assert!(vault.open());
    vault
        .set("GITHUB_TOKEN", "ghp_0123456789abcdefABCDEF")
        .unwrap();
    assert!(dir.join(VAULT_FILE).exists());
}

/// Ghost's folders in `dir`, the socket there, and no bus to a keyring.
fn isolated(cmd: &mut Command, dir: &Path, socket: &Path) {
    std::fs::create_dir_all(dir.join("config")).unwrap();
    cmd.env("GHOST_API_SOCKET", socket)
        .env("GHOST_DATA_DIR", dir)
        .env("GHOST_CONFIG_DIR", dir.join("config"))
        .env("DBUS_SESSION_BUS_ADDRESS", NO_BUS)
        .env_remove("GHOST_PARENT_PID");
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn said(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// Waits for `ok`, and on a timeout shows the daemon's own output.
fn wait_until(what: &str, timeout: Duration, log: &Path, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + timeout;
    while !ok() {
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}; the daemon said:\n{}",
            said(log)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn run_with_a_vault_and_no_keyring_waits_instead_of_crashing() {
    let dir = tempfile::tempdir().unwrap();
    seal_a_vault(dir.path());
    let socket = dir.path().join("api.sock");
    let log = dir.path().join("ghostd.log");
    let mut sh = Command::new("sh");
    sh.arg("-c")
        .arg(r#""$0" run >"$1" 2>&1 & echo $!"#)
        .arg(GHOSTD)
        .arg(&log);
    isolated(&mut sh, dir.path(), &socket);
    let out = sh.output().unwrap();
    let daemon: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("the daemon's pid");
    wait_until("the socket", Duration::from_secs(30), &log, || {
        socket.exists()
    });
    assert!(
        alive(daemon),
        "the daemon stopped; it said:\n{}",
        said(&log)
    );
    let text = said(&log);
    assert!(!text.contains("panicked"), "{text}");
    assert!(
        text.contains("vault not open") && text.contains("keyring"),
        "the keyring is said to be out of reach:\n{text}"
    );

    // The waiting list has it, through the daemon's own client.
    let mut last = String::new();
    wait_until("the waiting list", Duration::from_secs(10), &log, || {
        let mut status = Command::new(GHOSTD);
        isolated(&mut status, dir.path(), &socket);
        let out = status.arg("status").output().unwrap();
        last = String::from_utf8_lossy(&out.stdout).into_owned();
        serde_json::from_str::<serde_json::Value>(&last)
            .ok()
            .and_then(|v| v["waiting"].as_u64())
            .is_some_and(|n| n >= 1)
    });

    let _ = Command::new("kill").arg(daemon.to_string()).status();
    wait_until("the daemon to stop", Duration::from_secs(10), &log, || {
        !alive(daemon)
    });
}

#[test]
fn check_with_a_vault_and_no_keyring_says_it_waits() {
    let dir = tempfile::tempdir().unwrap();
    seal_a_vault(dir.path());
    let mut check = Command::new(GHOSTD);
    isolated(&mut check, dir.path(), &dir.path().join("api.sock"));
    let out = check.arg("check").output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!text.contains("panicked"), "{text}");
    assert_ne!(out.status.code(), Some(101), "{text}");
    assert!(
        text.contains("(unavailable)") && text.contains("Ghost starts anyway and waits for it"),
        "{text}"
    );
}
