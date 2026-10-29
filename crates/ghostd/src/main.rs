//! `ghostd run`: the daemon. `ghostd status|wake|say|sleep`: clients of its socket.
//! `ghostd passwords`: the vault from the command line. `ghostd check`: the settings,
//! the vault, Ghost's folder and the apps, with a problem as the exit code. `ghostd push`:
//! a signed delivery to the webhook listener (ADR 0027). `ghostd latency`: where the
//! turns' time went (ADR 0018). `ghostd api-schema`: the control API's document.
//! `ghostd version`: what this build is. `ghostd restore-volumes`: what the shell runs
//! before a start and after a stop (ADR 0038).
//!
//! `ghostd perf`: the latency harness (`ghost_voice::perf`, ADR 0018), with the vault's
//! keys. W1 of heka-vault/ghost#39: the daemon starts, opens the control socket and
//! answers `/status` with `starting`. W3 gave it the whole control API over a `Deps` with
//! nothing behind it yet; W8 the foundations in the library (`ghostd::*`); W7 the
//! daemon itself (`ghostd::daemon`).

mod parent;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use ghost_api::GhostClient;
use ghost_api::server::default_socket_path;
use ghost_core::passwords::SharedEnviron;
use ghostd::build_info;
use ghostd::check::check_command;
use ghostd::config::runtime_dir;
use ghostd::daemon::{Daemon, LogMirror, Ports, settings_from};
use ghostd::latency::latency_command;
use ghostd::passwords::{Cli, Redacting, open_passwords, operator_keyring, passwords_cli};
use ghostd::push::push_cli;
use ghostd::{GhostdSettings, wake};
use tokio::sync::Notify;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Set when the parent the shell named is gone: one more way to stop.
static PARENT_GONE: Notify = Notify::const_new();

fn main() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    // No log line carries a password: every line is redacted against the vault's values
    // before it is written (ADR 0036), and so is what the mirror hands the desktop.
    let level = std::env::var("GHOST_LOG")
        .or_else(|_| std::env::var("GHOST_LOG_LEVEL").map(|l| l.to_lowercase()))
        .unwrap_or_else(|_| "info".into());
    let mirror = LogMirror::new();
    tracing_subscriber::registry()
        .with(EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(Redacting(std::io::stderr))
                .with_target(false)
                // Colour for a terminal only: the journal (through the shell's systemd-cat,
                // or the unit) and a log file get plain lines.
                .with_ansi(std::io::stderr().is_terminal()),
        )
        .with(mirror.clone())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => run(mirror),
        Some("status") => client("status", &args[1..]),
        Some("wake") => client("wake", &args[1..]),
        Some("say") => client("say", &args[1..]),
        Some("sleep") => client("sleep", &args[1..]),
        Some("restore-volumes") => restore_volumes(),
        Some("check") => {
            let cfg = GhostdSettings::load()?;
            std::process::exit(check(&cfg))
        }
        Some("push") => {
            let cfg = GhostdSettings::load()?;
            // The hook's password (ADR 0036).
            let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
            let _vault = open_passwords(&cfg, operator_keyring(), Arc::clone(&environ));
            let mut secrets: HashMap<String, String> = std::env::vars().collect();
            secrets.extend(environ.lock().unwrap_or_else(|e| e.into_inner()).clone());
            std::process::exit(push_cli(
                &args[1..],
                &cfg,
                &secrets,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            ))
        }
        Some("latency") => {
            let cfg = GhostdSettings::load()?;
            std::process::exit(latency_command(
                &args[1..],
                &cfg.latency_path(),
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            ))
        }
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
        Some("perf") => {
            // The model and Jev keys (ADR 0036): the vault's values into this process's
            // environment, which the harness reads, before any thread or runtime exists;
            // `child_env` keeps them from the apps it starts.
            let cfg = GhostdSettings::load()?;
            let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
            let _vault = open_passwords(&cfg, operator_keyring(), Arc::clone(&environ));
            let values = environ.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for (k, v) in values {
                // SAFETY: before the runtime and its threads; nothing reads the
                // environment concurrently.
                unsafe { std::env::set_var(k, v) };
            }
            std::process::exit(ghost_voice::perf::cli::main(&args[1..]))
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
            "unknown command {other:?}; commands: run, status, wake, say, sleep, check, push, latency, perf, restore-volumes, passwords, api-schema, version"
        ),
    }
}

/// `ghostd run`: the vault first (ADR 0036), since Ghost's settings read its passwords
/// from the daemon's environment; then the daemon, until SIGTERM, SIGINT or the parent
/// going away.
#[tokio::main]
async fn run(mirror: LogMirror) -> anyhow::Result<()> {
    let base = GhostdSettings::load()?;
    let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
    let vault = Arc::new(open_passwords(
        &base,
        operator_keyring(),
        Arc::clone(&environ),
    ));
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.extend(environ.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let (cfg, voice) = settings_from(&env)?;
    ghost_voice::debug::enable(voice.debug_audio);
    let daemon = Arc::new(
        Daemon::new(cfg, voice, Ports::live())
            .with_passwords(vault)
            .with_environ(environ)
            .with_log_mirror(mirror),
    );
    parent::watch(&PARENT_GONE);
    tokio::spawn({
        let stop = daemon.stop.clone();
        async move {
            shutdown_signal().await;
            stop.cancel();
        }
    });
    daemon.run().await?;
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

/// Music a crash left ducked comes back: every stream `duck.json` saved goes back to its
/// exact previous volume and the file is removed (crates/ghost-audio). The shell runs
/// this before a start and after a stop (ADR 0038), so a sound stack that can't be
/// reached is said, not failed on.
fn restore_volumes() -> anyhow::Result<()> {
    let state = ghost_audio::duck::default_state_file();
    match ghost_audio::duck::restore_from_state(&state) {
        Ok(restored) if restored.is_empty() => info!("restore-volumes: nothing to restore"),
        Ok(restored) => info!("restore-volumes: {} stream(s) restored", restored.len()),
        Err(e) => warn!("restore-volumes: {e}"),
    }
    Ok(())
}

/// `ghostd check`, on its own runtime: the apps connect to be described.
#[tokio::main]
async fn check(cfg: &GhostdSettings) -> i32 {
    check_command(cfg, operator_keyring(), &mut std::io::stdout()).await
}

/// One call over the socket, the body printed as the daemon sent it; `wake` falls back
/// to the wake file when no daemon answers.
#[tokio::main]
async fn client(cmd: &str, args: &[String]) -> anyhow::Result<()> {
    let path = default_socket_path();
    if !path.exists() {
        if cmd == "wake" {
            std::fs::write(wake::wake_file_in(&runtime_dir()), "")?;
            println!("wake requested (file; daemon API not running)");
            return Ok(());
        }
        anyhow::bail!("ghostd is not running (no socket at {})", path.display());
    }
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
