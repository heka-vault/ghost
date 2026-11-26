//! The operator's own AI subscriptions (ADR 0078): Claude Code, OpenAI Codex and Google
//! Antigravity, the vendors' own programs. Ghost asks each program whether it is here and
//! signed in, and on the operator's click runs the vendor's own install command or the
//! program's own sign-in, as the vendor documents them. Ghost is not another harness for
//! them: it never reads, copies or stores a sign-in, never offers a sign-in of its own in a
//! vendor's name, and never answers a program's questions (its terms, its menus) for the
//! operator.
//!
//! Detection asks the program only, never its files (ADR 0068, ADR 0069), in an environment
//! built from scratch ([`base_env`]): nothing named `ANTHROPIC_*`, `CLAUDE_*`, `OPENAI_*`,
//! `CODEX_*` (but `CODEX_HOME`), `GEMINI_*` or `GOOGLE_*` from the environment Ghost was
//! started in, and none of Ghost's passwords. On the workstation an inherited
//! `ANTHROPIC_AUTH_TOKEN` made `claude auth status` look signed in when it was not
//! (2026-11-25). A sign-in adds what a browser needs to open ([`BROWSER_VARS`]) and nothing
//! else.
//!
//! The routes that start any of it are the control API's, the operator's only: no tool of
//! Ghost's own app, no outside client, nothing on the phone, no run and no model
//! (`no_tool_installs_or_signs_in_another_tool`).

pub mod jobs;
#[cfg(test)]
mod tests;
pub mod text;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::other_tools::program;

pub use jobs::{JobKind, JobSnapshot, JobState, Jobs, Refused};

/// One of the subscriptions Ghost offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Sub {
    ClaudeCode,
    Codex,
    Antigravity,
}

/// How Ghost asks the program whether it is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusHow {
    /// `claude auth status --json` (with `--version` and the switches, `program::look`).
    ClaudeJson,
    /// `codex login status`: exit 0 and not "Not logged in".
    CodexLoginStatus,
    /// `agy models`: exit 0 and no "sign in" in what it says. Antigravity documents no status
    /// command (2026-11-25); this is the one its own message points at.
    AgyModels,
}

/// How the program signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInHow {
    /// Ghost runs it in a terminal of its own and shows what it prints: the browser it opens,
    /// the address in case it didn't, a code to paste back, a code to type elsewhere.
    Pty(&'static [&'static str]),
    /// The program's sign-in is a screen of its own with choices that are the operator's (a
    /// menu, its terms): Ghost shows the command to run in a terminal, and looks again after.
    Terminal(&'static str),
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub sub: Sub,
    pub id: &'static str,
    pub title: &'static str,
    /// The program's name on the PATH.
    pub binary: &'static str,
    /// The vendor's own install command, exactly as its documentation gives it. Run with
    /// `sh -c` on the operator's click, never as root.
    pub install: &'static str,
    /// Where that command is documented.
    pub install_source: &'static str,
    pub status: StatusHow,
    pub sign_in: SignInHow,
    /// The command the operator would type to sign in, as shown.
    pub sign_in_shown: &'static str,
    /// The domains a sign-in address may be on: anything else the program prints is not a link.
    pub domains: &'static [&'static str],
    /// The provider Ghost runs it as (`ghost_core::connection`), when it has one.
    pub provider: Option<&'static str>,
}

/// The fixed table (ADR 0078). Each command is the vendor's, from the page named.
pub const TABLE: [Spec; 3] = [
    Spec {
        sub: Sub::ClaudeCode,
        id: "claude-code",
        title: "Claude Code",
        binary: program::BINARY,
        install: "curl -fsSL https://claude.ai/install.sh | bash",
        install_source: "https://code.claude.com/docs/en/setup",
        status: StatusHow::ClaudeJson,
        sign_in: SignInHow::Pty(&["auth", "login"]),
        sign_in_shown: "claude auth login",
        domains: &["claude.ai", "claude.com", "anthropic.com"],
        provider: Some(crate::connection::CLAUDE_CODE),
    },
    Spec {
        sub: Sub::Codex,
        id: "codex",
        title: "OpenAI Codex",
        binary: "codex",
        install: "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
        install_source: "https://learn.chatgpt.com/docs/codex/cli",
        status: StatusHow::CodexLoginStatus,
        sign_in: SignInHow::Pty(&["login"]),
        sign_in_shown: "codex login",
        domains: &["openai.com", "chatgpt.com"],
        provider: None,
    },
    Spec {
        sub: Sub::Antigravity,
        id: "antigravity",
        title: "Google Antigravity",
        binary: "agy",
        install: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        install_source: "https://codelabs.developers.google.com/antigravity-cli-hands-on",
        status: StatusHow::AgyModels,
        sign_in: SignInHow::Terminal("agy"),
        sign_in_shown: "agy",
        domains: &["google.com", "antigravity.google"],
        provider: None,
    },
];

impl Sub {
    pub fn spec(self) -> &'static Spec {
        TABLE
            .iter()
            .find(|s| s.sub == self)
            .expect("every sub has a row")
    }

    pub fn parse(id: &str) -> Option<Sub> {
        TABLE.iter().find(|s| s.id == id).map(|s| s.sub)
    }
}

/// How long one question to a program may take.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(15);
/// The most of an answer read.
const ANSWER_CAP: usize = 64 * 1024;

/// What a browser needs to open from a sign-in, and nothing else: the display, the session's
/// bus and runtime folder, the desktop's name (for `xdg-open`) and the operator's choice of
/// browser. Never a credential.
pub const BROWSER_VARS: [&str; 8] = [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "XAUTHORITY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "BROWSER",
];

/// The environment a program is asked in, built from scratch: `HOME`, `PATH` (with
/// `~/.local/bin`, where each vendor's installer puts its program), `USER` and `LANG`, for
/// Claude Code the switch that keeps claude.ai's own apps out (`program::clean_env`), and for
/// Codex `CODEX_HOME` when it is set, which says where Codex keeps its own files. Nothing
/// else: no key, no token, no password.
pub fn base_env(sub: Sub, started: &HashMap<String, String>) -> BTreeMap<String, String> {
    let mut env = program::clean_env(started);
    if sub != Sub::ClaudeCode {
        env.remove("ENABLE_CLAUDEAI_MCP_SERVERS");
    }
    if let Some(home) = env.get("HOME").cloned() {
        let local = format!("{home}/.local/bin");
        let path = env.get("PATH").cloned().unwrap_or_default();
        if !path.split(':').any(|d| d == local) {
            let joined = if path.is_empty() {
                local
            } else {
                format!("{path}:{local}")
            };
            env.insert("PATH".into(), joined);
        }
    }
    if sub == Sub::Codex
        && let Some(v) = started.get("CODEX_HOME").filter(|v| v.starts_with('/'))
    {
        env.insert("CODEX_HOME".into(), v.clone());
    }
    env
}

/// The sign-in's environment: [`base_env`] and [`BROWSER_VARS`], with a terminal's name.
pub fn sign_in_env(sub: Sub, started: &HashMap<String, String>) -> BTreeMap<String, String> {
    let mut env = base_env(sub, started);
    for k in BROWSER_VARS {
        if let Some(v) = started.get(k).filter(|v| !v.is_empty()) {
            env.insert(k.into(), v.clone());
        }
    }
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("NO_COLOR".into(), "1".into());
    env
}

/// Where the program is: the first executable of its name on `PATH` (absolute folders
/// only), else in `~/.local/bin`, where the vendors' installers put it.
pub fn which(binary: &str, started: &HashMap<String, String>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    let on_path = started.get("PATH").and_then(|path| {
        std::env::split_paths(path)
            .filter(|d| d.is_absolute())
            .map(|d| d.join(binary))
            .find(|p| executable(p))
    });
    on_path.or_else(|| {
        let home = started.get("HOME").filter(|h| h.starts_with('/'))?;
        let p = Path::new(home).join(".local/bin").join(binary);
        executable(&p).then_some(p)
    })
}

/// Run the program once with `args` as argv in `env`: whether it exited 0, and what it
/// printed (both streams), or None when it would not start, took too long or said too much.
pub async fn ask(
    binary: &Path,
    args: &[&str],
    env: &BTreeMap<String, String>,
) -> Option<(bool, String)> {
    let child = tokio::process::Command::new(binary)
        .args(args)
        .env_clear()
        .envs(env)
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(ASK_TIMEOUT, child).await.ok()?.ok()?;
    if out.stdout.len() + out.stderr.len() > ANSWER_CAP {
        return None;
    }
    Some((
        out.status.success(),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

/// `codex login status`'s answer: signed in when it exited 0 and does not say it isn't.
pub fn codex_signed_in(ok: bool, said: &str) -> bool {
    let s = said.to_lowercase();
    ok && !s.contains("not logged in") && !s.contains("not signed in")
}

/// `agy models`' answer: signed in when it exited 0 and does not ask to sign in.
pub fn agy_signed_in(ok: bool, said: &str) -> bool {
    let s = said.to_lowercase();
    ok && !s.contains("sign in") && !s.contains("signed in") && !s.contains("log in")
}

/// A version out of `2.1.330 (Claude Code)`, `codex-cli 0.166.0`, `agy 1.2.3`: the first
/// word that is numbers and dots.
pub fn version_of(said: &str) -> Option<String> {
    said.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            w.contains('.')
                && w.split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|w| w.chars().take(32).collect())
}

/// A program as Ghost found it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub sub: Sub,
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub signed_in: bool,
    /// Claude Code as `program::look` found it (its switches, how it is signed in).
    pub claude: Option<program::Found>,
}

/// Whether `sub`'s program is signed in, asked of the program at `binary`.
pub async fn signed_in(sub: Sub, binary: &Path, started: &HashMap<String, String>) -> bool {
    let env = base_env(sub, started);
    match sub.spec().status {
        StatusHow::ClaudeJson => ask(binary, &["auth", "status", "--json"], &env)
            .await
            .is_some_and(|(_, out)| program::parse_status(&out) != program::SignIn::SignedOut),
        StatusHow::CodexLoginStatus => ask(binary, &["login", "status"], &env)
            .await
            .is_some_and(|(ok, out)| codex_signed_in(ok, &out)),
        StatusHow::AgyModels => ask(binary, &["models"], &env)
            .await
            .is_some_and(|(ok, out)| agy_signed_in(ok, &out)),
    }
}

/// Ask `sub`'s program, if it is here, its version and whether it is signed in. Opens no
/// file of its own.
pub async fn look(sub: Sub, started: &HashMap<String, String>) -> Seen {
    let spec = sub.spec();
    let Some(binary) = which(spec.binary, started) else {
        return Seen {
            sub,
            binary: None,
            version: None,
            signed_in: false,
            claude: None,
        };
    };
    if sub == Sub::ClaudeCode {
        // The clean environment `program::look` builds, with `~/.local/bin` on the PATH.
        let mut env: HashMap<String, String> = base_env(sub, started).into_iter().collect();
        env.remove("ENABLE_CLAUDEAI_MCP_SERVERS");
        let found = program::look(&binary, &env).await;
        return Seen {
            sub,
            version: found.version.clone(),
            signed_in: found.sign_in != program::SignIn::SignedOut,
            binary: Some(binary),
            claude: Some(found),
        };
    }
    let env = base_env(sub, started);
    let version = ask(&binary, &["--version"], &env)
        .await
        .and_then(|(_, out)| version_of(&out));
    let signed_in = signed_in(sub, &binary, started).await;
    Seen {
        sub,
        binary: Some(binary),
        version,
        signed_in,
        claude: None,
    }
}
