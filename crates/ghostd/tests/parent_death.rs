//! `ghostd run` exits when the parent that named itself in `GHOST_PARENT_PID` is gone
//! (ADR 0038), and `restore-volumes` exists.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Waits for `ok`, and on a timeout shows the daemon's own output, so a failure on a
/// machine without the operator's caches says what the daemon said.
fn wait_until(what: &str, timeout: Duration, log: &Path, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + timeout;
    while !ok() {
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}; the daemon said:\n{}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn run_stops_when_the_parent_that_named_itself_dies() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("api.sock");
    let log = dir.path().join("ghostd.log");
    // A stand-in shell: starts the daemon with its own pid as the parent, prints the
    // daemon's pid, then lives (as sleep, the same pid) until killed.
    let mut parent = Command::new("sh")
        .arg("-c")
        .arg(r#"GHOST_PARENT_PID=$$ "$0" run >"$1" 2>&1 & echo $!; exec sleep 300"#)
        .arg(GHOSTD)
        .arg(&log)
        .env("GHOST_API_SOCKET", &socket)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(parent.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let daemon: u32 = line.trim().parse().expect("the daemon's pid");
    // A loaded CI runner starts a debug build slowly; the limit is generous.
    wait_until("the socket", Duration::from_secs(30), &log, || {
        socket.exists()
    });
    assert!(alive(daemon));

    parent.kill().unwrap();
    parent.wait().unwrap();
    // PDEATHSIG ends it at once; the 3 s poll would otherwise.
    wait_until("the daemon to stop", Duration::from_secs(15), &log, || {
        !alive(daemon)
    });
    wait_until(
        "the socket file to go",
        Duration::from_secs(5),
        &log,
        || !socket.exists(),
    );
}

#[test]
fn run_without_a_named_parent_keeps_going_when_reparented() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("api.sock");
    let log = dir.path().join("ghostd.log");
    // No GHOST_PARENT_PID: the headless mode. The sh exits at once, so the daemon is
    // reparented, and must not care.
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#""$0" run >"$1" 2>&1 & echo $!"#)
        .arg(GHOSTD)
        .arg(&log)
        .env("GHOST_API_SOCKET", &socket)
        .env_remove("GHOST_PARENT_PID")
        .output()
        .unwrap();
    let daemon: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("the daemon's pid");
    wait_until("the socket", Duration::from_secs(30), &log, || {
        socket.exists()
    });
    std::thread::sleep(Duration::from_secs(4)); // longer than the poll
    assert!(
        alive(daemon),
        "a daemon with no named parent runs on; it said:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    let _ = Command::new("kill").arg(daemon.to_string()).status();
    wait_until("the daemon to stop", Duration::from_secs(10), &log, || {
        !alive(daemon)
    });
}

#[test]
fn restore_volumes_exists_and_succeeds() {
    let out = Command::new(GHOSTD)
        .arg("restore-volumes")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("nothing to restore"), "{text}");
}
