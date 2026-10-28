//! `ghostd run`: the daemon. `ghostd status`: a client of its socket. `ghostd
//! restore-volumes`: what the shell runs before a start and after a stop (ADR 0038).
//!
//! W1 of heka-vault/ghost#39: the daemon starts, opens the control socket and answers
//! `/status` with `starting`. Everything else arrives package by package.

mod parent;

use std::sync::Arc;

use anyhow::Context;
use ghost_api::server::{default_socket_path, serve};
use ghost_api::{DaemonState, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{Notify, RwLock};
use tracing::info;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Set when the parent the shell named is gone: one more way to stop.
static PARENT_GONE: Notify = Notify::const_new();

fn main() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("GHOST_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        // Colour for a terminal only: the journal (through the shell's systemd-cat, or the
        // unit) and a log file get plain lines.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => run(),
        Some("status") => status(),
        Some("restore-volumes") => restore_volumes(),
        Some("--version" | "version") => {
            println!("ghostd {VERSION}");
            Ok(())
        }
        Some(other) => anyhow::bail!(
            "unknown command {other:?}; commands: run, status, restore-volumes, version"
        ),
    }
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let wake_mode = std::env::var("GHOST_WAKE_MODE").unwrap_or_else(|_| "transcript".into());
    let state = Arc::new(RwLock::new(DaemonState::new(VERSION, wake_mode)));
    info!("ghostd {VERSION} starting");
    parent::watch(&PARENT_GONE);
    serve(&default_socket_path(), router(state), shutdown_signal()).await?;
    info!("ghostd stopped");
    Ok(())
}

/// SIGINT or SIGTERM, the two ways systemd, the shell and a terminal stop the daemon; or
/// the parent the shell named going away.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => info!("SIGTERM"),
        _ = int.recv() => info!("SIGINT"),
        _ = PARENT_GONE.notified() => info!("parent gone"),
    }
}

/// Music a crash left ducked comes back. W4 brings the ducking and what it saves; until
/// then there is nothing to restore, and the shell's call before a start and after a stop
/// succeeds all the same.
fn restore_volumes() -> anyhow::Result<()> {
    info!("restore-volumes: nothing to restore");
    Ok(())
}

/// One GET over the socket, the body printed as the daemon sent it.
#[tokio::main]
async fn status() -> anyhow::Result<()> {
    let path = default_socket_path();
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("ghostd not reachable on {}", path.display()))?;
    stream
        .write_all(b"GET /status HTTP/1.1\r\nHost: ghost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map_or(text.as_ref(), |(_, b)| b);
    println!("{body}");
    Ok(())
}
