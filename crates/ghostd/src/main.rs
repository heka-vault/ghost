//! `ghostd run`: the daemon. `ghostd status`: a client of its socket. `ghostd passwords`:
//! the vault from the command line. `ghostd version`: what this build is.
//!
//! W1 of heka-vault/ghost#39: the daemon starts, opens the control socket and answers
//! `/status` with `starting`. W8 adds the foundations in the library (`ghostd::*`); the
//! rest arrives package by package.

use std::sync::Arc;

use anyhow::Context;
use ghost_api::server::{default_socket_path, serve};
use ghost_api::{DaemonState, router};
use ghostd::GhostdSettings;
use ghostd::build_info;
use ghostd::passwords::{Cli, Redacting, operator_keyring, passwords_cli};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    // No log line carries a password: every line is redacted against the vault's values
    // before it is written (ADR 0036).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("GHOST_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(Redacting(std::io::stderr))
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => run(),
        Some("status") => status(),
        Some("passwords") => {
            let cfg = GhostdSettings::load()?;
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            let tty = rustix::termios::isatty(std::io::stdin());
            let mut cli = Cli {
                stdin: &mut input,
                stdout: &mut out,
                stderr: &mut err,
                tty,
            };
            std::process::exit(passwords_cli(
                &args[1..],
                &cfg,
                operator_keyring(),
                &mut cli,
            ))
        }
        Some("--version" | "version") => {
            let build = build_info::detect(None, None, None);
            println!("{}", build_info::describe(VERSION, &build));
            Ok(())
        }
        Some(other) => {
            anyhow::bail!("unknown command {other:?}; commands: run, status, passwords, version")
        }
    }
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let cfg = GhostdSettings::load()?;
    let build = build_info::detect(None, None, None);
    let state = Arc::new(RwLock::new(DaemonState::new(
        VERSION,
        cfg.wake_mode.clone(),
    )));
    info!("{} starting", build_info::describe(VERSION, &build));
    serve(&default_socket_path(), router(state), shutdown_signal()).await?;
    info!("ghostd stopped");
    Ok(())
}

/// SIGINT or SIGTERM, the two ways systemd, the shell and a terminal stop the daemon.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => info!("SIGTERM"),
        _ = int.recv() => info!("SIGINT"),
    }
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
