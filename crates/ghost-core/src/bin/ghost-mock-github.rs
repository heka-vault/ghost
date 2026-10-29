//! `ghost-mock-github`: a pretend GitHub, as an MCP server over stdio. With `--state PATH`
//! every write is saved there and a restart picks it up again. The name is what
//! `apps.json` names (ADR 0034).

use std::path::PathBuf;

use ghost_core::mock_github::MockGithubServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: ghost-mock-github [-h] [--state STATE]";
const HELP: &str = "\
usage: ghost-mock-github [-h] [--state STATE]

A pretend GitHub, as an MCP server over stdio.

options:
  -h, --help     show this help message and exit
  --state STATE  JSON file that keeps every write";

/// `~` and `~/...` as the operator's home.
fn expand_user(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with('/'))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest.trim_start_matches('/'));
    }
    PathBuf::from(path)
}

/// The state path, if given. `Err` carries what to say before exiting with 2.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<PathBuf>, String> {
    let mut args = args.into_iter();
    let mut state = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            "--state" => {
                let Some(value) = args.next() else {
                    return Err(format!(
                        "{USAGE}\nghost-mock-github: error: argument --state: expected one argument"
                    ));
                };
                state = Some(expand_user(&value));
            }
            other => {
                if let Some(value) = other.strip_prefix("--state=") {
                    state = Some(expand_user(value));
                } else {
                    return Err(format!(
                        "{USAGE}\nghost-mock-github: error: unrecognized arguments: {other}"
                    ));
                }
            }
        }
    }
    Ok(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout is the protocol; anything said goes to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let state = match parse_args(std::env::args().skip(1)) {
        Ok(state) => state,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if let Some(path) = &state
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let server = MockGithubServer::new(state.as_deref())?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
