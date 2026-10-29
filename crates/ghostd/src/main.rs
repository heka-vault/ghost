//! `ghostd run`: the daemon. `ghostd status|wake|say|sleep`: clients of its socket.
//! `ghostd passwords`: the vault from the command line. `ghostd api-schema`: the control
//! API's document. `ghostd version`: what this build is. `ghostd restore-volumes`: what
//! the shell runs before a start and after a stop (ADR 0038).
//!
//! W1 of heka-vault/ghost#39: the daemon starts, opens the control socket and answers
//! `/status` with `starting`. W3 gave it the whole control API over a `Deps` with nothing
//! behind it yet; W8 the foundations in the library (`ghostd::*`); the pipeline, the apps
//! and the folder arrive package by package.

mod parent;

use std::sync::Arc;

use anyhow::Context;
use ghost_api::server::{default_socket_path, serve};
use ghost_api::{DaemonState, Deps, GhostClient, Unavailable, router};
use ghost_core::gate::WriteGate;
use ghost_core::store::Store;
use ghostd::GhostdSettings;
use ghostd::build_info;
use ghostd::passwords::{Cli, Redacting, operator_keyring, passwords_cli};
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Set when the parent the shell named is gone: one more way to stop.
static PARENT_GONE: Notify = Notify::const_new();

fn main() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    // No log line carries a password: every line is redacted against the vault's values
    // before it is written (ADR 0036).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("GHOST_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(Redacting(std::io::stderr))
        .with_target(false)
        // Colour for a terminal only: the journal (through the shell's systemd-cat, or the
        // unit) and a log file get plain lines.
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => run(),
        Some("status") => client("status", &args[1..]),
        Some("wake") => client("wake", &args[1..]),
        Some("say") => client("say", &args[1..]),
        Some("sleep") => client("sleep", &args[1..]),
        Some("restore-volumes") => restore_volumes(),
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
        Some("api-schema") => {
            let doc = ghost_api::openapi::document();
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(())
        }
        Some("--version" | "version") => {
            let build = build_info::detect(None, None, None);
            println!("{}", build_info::describe(VERSION, &build));
            Ok(())
        }
        Some(other) => anyhow::bail!(
            "unknown command {other:?}; commands: run, status, wake, say, sleep, restore-volumes, passwords, api-schema, version"
        ),
    }
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let cfg = GhostdSettings::load()?;
    let build = build_info::detect(None, None, None);
    let state = Arc::new(DaemonState::new(VERSION, cfg.wake_mode.clone()));
    let store = Arc::new(Store::in_memory().context("opening the store")?);
    let gate = Arc::new(WriteGate::new(store.clone()));
    let deps = Arc::new(Deps::minimal(state, Arc::new(Unavailable), store, gate));
    info!("{} starting", build_info::describe(VERSION, &build));
    parent::watch(&PARENT_GONE);
    serve(&default_socket_path(), router(deps), shutdown_signal()).await?;
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

/// One call over the socket, the body printed as the daemon sent it.
#[tokio::main]
async fn client(cmd: &str, args: &[String]) -> anyhow::Result<()> {
    let path = default_socket_path();
    let client = GhostClient::new(&path);
    let text = args.join(" ");
    let out = match cmd {
        "status" => client.status().await,
        "wake" => {
            client
                .wake((!text.is_empty()).then_some(text.as_str()))
                .await
        }
        "say" => {
            if text.is_empty() {
                anyhow::bail!("say what? ghostd say <words>");
            }
            client.say(&text).await
        }
        _ => client.sleep().await,
    }
    .with_context(|| format!("ghostd {cmd} on {}", path.display()))?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
