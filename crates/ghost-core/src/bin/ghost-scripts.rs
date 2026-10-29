//! `ghost-scripts`: run a skill's scripts, as an MCP server over stdio (ADR 0023). The name
//! is what `apps.json` names (ADR 0034): `--skills` is Ghost's skills folder, `--root` a
//! project root with `skills`, `.agents/skills` or `.claude/skills` (repeatable).

use std::path::PathBuf;

use ghost_core::scripts_server::ScriptsServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: ghost-scripts [-h] [--skills SKILLS] [--root ROOT]";
const HELP: &str = "\
usage: ghost-scripts [-h] [--skills SKILLS] [--root ROOT]

Run a skill's scripts, as an MCP server over stdio.

options:
  -h, --help       show this help message and exit
  --skills SKILLS
  --root ROOT      a project root with .agents/skills";
const DEFAULT_SKILLS: &str = "~/.config/ghost/skills";

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

struct Args {
    skills: PathBuf,
    roots: Vec<PathBuf>,
}

/// The folders. `Err` carries what to say before exiting with 2.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut skills = expand_user(DEFAULT_SKILLS);
    let mut roots = Vec::new();
    let missing = |name: &str| {
        format!("{USAGE}\nghost-scripts: error: argument {name}: expected one argument")
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            "--skills" => {
                skills = expand_user(&args.next().ok_or_else(|| missing("--skills"))?);
            }
            "--root" => {
                roots.push(expand_user(&args.next().ok_or_else(|| missing("--root"))?));
            }
            other => {
                if let Some(value) = other.strip_prefix("--skills=") {
                    skills = expand_user(value);
                } else if let Some(value) = other.strip_prefix("--root=") {
                    roots.push(expand_user(value));
                } else {
                    return Err(format!(
                        "{USAGE}\nghost-scripts: error: unrecognized arguments: {other}"
                    ));
                }
            }
        }
    }
    Ok(Args { skills, roots })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout is the protocol; anything said goes to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let service = ScriptsServer::new(args.skills, args.roots)
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
