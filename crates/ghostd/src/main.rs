//! `ghostd run`: the daemon. `ghostd status|wake|say|sleep`: clients of its socket.
//!
//! W1 of heka-vault/ghost#39: the daemon starts, opens the control socket and answers
//! `/status` with `starting`. W3 gave it the whole control API over a `Deps` with nothing
//! behind it yet; the pipeline, the apps and the folder arrive package by package.

use std::sync::Arc;

use anyhow::Context;
use ghost_api::server::{default_socket_path, serve};
use ghost_api::{DaemonState, Deps, GhostClient, Unavailable, router};
use ghost_core::gate::WriteGate;
use ghost_core::store::Store;
use tracing::info;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("GHOST_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => run(),
        Some("status") => client("status", &args[1..]),
        Some("wake") => client("wake", &args[1..]),
        Some("say") => client("say", &args[1..]),
        Some("sleep") => client("sleep", &args[1..]),
        Some("api-schema") => {
            let doc = ghost_api::openapi::document();
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(())
        }
        Some("--version" | "version") => {
            println!("ghostd {VERSION}");
            Ok(())
        }
        Some(other) => anyhow::bail!(
            "unknown command {other:?}; commands: run, status, wake, say, sleep, api-schema, version"
        ),
    }
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let wake_mode = std::env::var("GHOST_WAKE_MODE").unwrap_or_else(|_| "transcript".into());
    let state = Arc::new(DaemonState::new(VERSION, wake_mode));
    let store = Arc::new(Store::in_memory().context("opening the store")?);
    let gate = Arc::new(WriteGate::new(store.clone()));
    let deps = Arc::new(Deps::minimal(state, Arc::new(Unavailable), store, gate));
    info!("ghostd {VERSION} starting");
    serve(&default_socket_path(), router(deps), shutdown_signal()).await?;
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
