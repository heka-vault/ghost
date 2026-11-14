//! The operator's own Claude Code, as a model Ghost can run (ADR 0069).
//!
//! Ghost asks the program, never its files: `claude --version`, `claude --help` (does it
//! have every switch Ghost needs to run it with its own tools off), and `claude auth status
//! --json` (is it signed in, and how). Each runs as argv, never through a shell, in an
//! environment built from scratch ([`clean_env`]): `HOME`, `PATH`, `USER`, `LANG` and the
//! one switch that keeps claude.ai's own apps out, and nothing named `ANTHROPIC_*` or
//! `CLAUDE_*` from the environment Ghost was started in. On the workstation an inherited
//! proxy token silently became the program's credential (2026-11-14), so the program here
//! is signed in only if it is signed in by itself.
//!
//! Of the status Ghost keeps whether it is signed in, the kind of sign-in (a Claude plan or
//! an API key) and the plan's name. Never the account's address or organisation, which the
//! text form prints: the JSON form is the one asked for, and it is read into a type with no
//! other field.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// The program's name on the PATH.
pub const BINARY: &str = "claude";

/// Every switch Ghost runs the program with (`ghost_llm::claude_code`). A version without
/// one of them is refused with a sentence: Ghost never runs it with its built-in tools on,
/// or with the operator's settings, hooks, plugins or apps loaded.
pub const REQUIRED_SWITCHES: [&str; 17] = [
    "--print",
    "--input-format",
    "--output-format",
    "--include-partial-messages",
    "--verbose",
    "--system-prompt",
    "--tools",
    "--setting-sources",
    "--strict-mcp-config",
    "--mcp-config",
    "--allowedTools",
    "--disable-slash-commands",
    "--no-session-persistence",
    "--permission-mode",
    "--settings",
    "--effort",
    "--model",
];

/// The versions Ghost was built and measured against: 2.1. Another major or minor still
/// runs when it has every switch ([`REQUIRED_SWITCHES`]), with a note that says so.
pub const TESTED_MAJOR_MINOR: (u32, u32) = (2, 1);
/// The version the fixtures were recorded from.
pub const RECORDED_WITH: &str = "2.1.325";

/// How long one of the three questions may take.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(15);
/// The most of an answer read.
const ANSWER_CAP: usize = 256 * 1024;

/// The environment the program runs in, built from scratch out of the one Ghost was started
/// in: `HOME`, `PATH`, `USER` and `LANG` as they are there (`LANG` `C.UTF-8` when unset),
/// and `ENABLE_CLAUDEAI_MCP_SERVERS=false` so the apps of the operator's claude.ai account
/// are not loaded. Nothing else: no `ANTHROPIC_*`, no `CLAUDE_*`, none of Ghost's passwords.
pub fn clean_env(started: &HashMap<String, String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in ["HOME", "PATH", "USER"] {
        if let Some(v) = started.get(key).filter(|v| !v.is_empty()) {
            env.insert(key.to_string(), v.clone());
        }
    }
    let lang = started
        .get("LANG")
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| "C.UTF-8".into());
    env.insert("LANG".into(), lang);
    env.insert("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into());
    env
}

/// `2.1.325` out of `2.1.325 (Claude Code)`.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().next()?;
    let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The switches of [`REQUIRED_SWITCHES`] the program's help does not list.
pub fn missing_switches(help: &str) -> Vec<&'static str> {
    REQUIRED_SWITCHES
        .into_iter()
        .filter(|s| {
            !help
                .split(|c: char| c.is_whitespace() || c == ',')
                .any(|w| {
                    // `--print` is listed as `-p, --print`; `--allowedTools` as
                    // `--allowedTools, --allowed-tools <tools...>`.
                    w == *s || w.strip_prefix(s).is_some_and(|rest| rest.starts_with('='))
                })
        })
        .collect()
}

/// How the program is signed in, as its status says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// Not signed in, or its sign-in has expired.
    SignedOut,
    /// With a Claude plan: its name when the program says it (`Pro`, `Max`).
    Plan(Option<String>),
    /// With an API key: replies are billed to that key.
    ApiKey,
}

/// `claude auth status --json`, read into these fields only.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    api_key_source: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
}

/// A plan's name as people say it: `claude_max_subscription` is `Max`.
fn plan_name(raw: &str) -> Option<String> {
    let n: String = raw
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let n = n
        .trim_start_matches("claude")
        .trim_end_matches("subscription")
        .trim_end_matches("plan");
    let name = match n {
        "pro" => "Pro",
        "max" | "max5" | "max5x" => "Max",
        "max20" | "max20x" | "ultra" => "Ultra",
        "team" => "Team",
        "enterprise" => "Enterprise",
        "free" => "Free",
        _ => return None,
    };
    Some(name.into())
}

/// What `claude auth status --json` said. Anything that doesn't read is signed out.
pub fn parse_status(json: &str) -> SignIn {
    let Ok(s) = serde_json::from_str::<Status>(json) else {
        return SignIn::SignedOut;
    };
    if !s.logged_in {
        return SignIn::SignedOut;
    }
    let method = s.auth_method.unwrap_or_default().to_lowercase();
    let key = s
        .api_key_source
        .as_deref()
        .is_some_and(|k| !k.is_empty() && k != "none");
    if key || method.replace(['_', '-'], "") == "apikey" {
        return SignIn::ApiKey;
    }
    SignIn::Plan(s.subscription_type.as_deref().and_then(plan_name))
}

/// The program as Ghost found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub binary: PathBuf,
    /// As it said it, `2.1.325`; None when it would not say.
    pub version: Option<String>,
    /// Switches Ghost needs that this version lacks: Ghost won't run it.
    pub missing: Vec<&'static str>,
    pub sign_in: SignIn,
}

impl Found {
    /// Ghost can run it as it is: every switch, and signed in.
    pub fn usable(&self) -> bool {
        self.missing.is_empty() && self.sign_in != SignIn::SignedOut
    }

    /// Why Ghost won't run it, in a sentence the operator can act on; None when it will.
    pub fn problem(&self) -> Option<String> {
        if !self.missing.is_empty() {
            return Some(format!(
                "This version of Claude Code ({}) lacks {} that Ghost needs to run it with its \
                 own tools off, so Ghost won't use it. Update it with `claude update` in a \
                 terminal, then Look again.",
                self.version.as_deref().unwrap_or("unknown"),
                self.missing.join(", ")
            ));
        }
        if self.sign_in == SignIn::SignedOut {
            return Some(SIGNED_OUT.into());
        }
        None
    }

    /// A version Ghost was not measured with, said once beside it; None for 2.1.
    pub fn version_note(&self) -> Option<String> {
        let v = self.version.as_deref()?;
        let (major, minor, _) = parse_version(v)?;
        ((major, minor) != TESTED_MAJOR_MINOR).then(|| {
            format!(
                "Ghost was tested with Claude Code {}.{}; this is {v}. It has every switch \
                 Ghost needs, so Ghost runs it.",
                TESTED_MAJOR_MINOR.0, TESTED_MAJOR_MINOR.1
            )
        })
    }

    /// How it is signed in, in a sentence; None when it isn't.
    pub fn sign_in_line(&self) -> Option<String> {
        match &self.sign_in {
            SignIn::SignedOut => None,
            SignIn::Plan(Some(plan)) => Some(format!(
                "Signed in with a Claude {plan} plan. Replies count against its limits."
            )),
            SignIn::Plan(None) => {
                Some("Signed in with a Claude plan. Replies count against its limits.".into())
            }
            SignIn::ApiKey => Some(
                "Signed in with an API key. Replies are billed to that key, not to a plan.".into(),
            ),
        }
    }
}

/// What the operator does when the program is installed and not signed in. Never a sign-in
/// inside Ghost: the program's own flow, in a terminal.
pub const SIGNED_OUT: &str = "Claude Code is installed but not signed in. Run `claude auth \
                              login` in a terminal, then Look again.";

/// Run the program once with `args`, in the clean environment: its standard output, or None
/// when it would not start, took too long or answered too much.
async fn ask(
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
    if out.stdout.len() > ANSWER_CAP {
        return None;
    }
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// Ask the program at `binary` its version, its switches and its sign-in. Opens no file of
/// its own: three commands, each as argv in [`clean_env`].
pub async fn look(binary: &Path, started: &HashMap<String, String>) -> Found {
    let env = clean_env(started);
    let version = ask(binary, &["--version"], &env)
        .await
        .and_then(|(_, out)| parse_version(&out).map(|(a, b, c)| format!("{a}.{b}.{c}")));
    let missing = match ask(binary, &["--help"], &env).await {
        Some((_, help)) => missing_switches(&help),
        None => REQUIRED_SWITCHES.to_vec(),
    };
    // Signed out exits 1 and still prints the JSON.
    let sign_in = ask(binary, &["auth", "status", "--json"], &env)
        .await
        .map_or(SignIn::SignedOut, |(_, out)| parse_status(&out));
    Found {
        binary: binary.to_path_buf(),
        version,
        missing,
        sign_in,
    }
}

/// The switches of [`REQUIRED_SWITCHES`] the program at `binary` lacks, from its own
/// `--help` in [`clean_env`]; every one when it would not answer.
pub async fn missing_at(binary: &Path, started: &HashMap<String, String>) -> Vec<&'static str> {
    match ask(binary, &["--help"], &clean_env(started)).await {
        Some((_, help)) => missing_switches(&help),
        None => REQUIRED_SWITCHES.to_vec(),
    }
}

/// Where `claude` is on `PATH`: the first executable file of that name, absolute folders
/// only.
pub fn which(path: Option<&str>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(BINARY))
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The help text of the version measured (2.1.325), cut to the switches' lines.
    const HELP: &str = include_str!("fixtures/claude-help.txt");

    #[test]
    fn the_clean_environment_carries_nothing_inherited_but_four_names() {
        let started: HashMap<String, String> = [
            ("HOME", "/home/op"),
            ("PATH", "/usr/bin"),
            ("USER", "op"),
            ("ANTHROPIC_BASE_URL", "http://localhost:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "proxy-token-0123456789"),
            ("ANTHROPIC_API_KEY", "sk-ant-fixture"),
            ("CLAUDE_CODE_SESSION_ID", "x"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "y"),
            ("GHOST_LLM_PASSWORD", "OPENAI_API_KEY"),
            ("OPENAI_API_KEY", "sk-fixture"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let env = clean_env(&started);
        let keys: Vec<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "ENABLE_CLAUDEAI_MCP_SERVERS",
                "HOME",
                "LANG",
                "PATH",
                "USER"
            ]
        );
        assert_eq!(env["ENABLE_CLAUDEAI_MCP_SERVERS"], "false");
        assert_eq!(env["LANG"], "C.UTF-8");
        assert!(
            env.keys()
                .all(|k| !k.starts_with("ANTHROPIC") && !k.starts_with("CLAUDE"))
        );
    }

    #[test]
    fn the_measured_version_has_every_switch_and_one_without_is_refused() {
        assert_eq!(missing_switches(HELP), Vec::<&str>::new());
        let old = HELP.replace("--strict-mcp-config", "--other-thing");
        assert_eq!(missing_switches(&old), vec!["--strict-mcp-config"]);
        let found = Found {
            binary: "/usr/bin/claude".into(),
            version: Some("1.0.0".into()),
            missing: missing_switches(&old),
            sign_in: SignIn::Plan(None),
        };
        assert!(!found.usable());
        let p = found.problem().unwrap();
        assert!(
            p.contains("--strict-mcp-config") && p.contains("won't use it"),
            "{p}"
        );
    }

    #[test]
    fn versions_read_and_another_minor_is_noted_not_refused() {
        assert_eq!(parse_version("2.1.325 (Claude Code)"), Some((2, 1, 325)));
        assert_eq!(parse_version("garbage"), None);
        let mut f = Found {
            binary: "/usr/bin/claude".into(),
            version: Some("2.1.325".into()),
            missing: vec![],
            sign_in: SignIn::ApiKey,
        };
        assert_eq!(f.version_note(), None);
        f.version = Some("2.4.0".into());
        assert!(
            f.version_note()
                .unwrap()
                .contains("tested with Claude Code 2.1")
        );
        assert!(f.usable());
    }

    #[test]
    fn the_status_is_read_for_four_fields_and_never_the_account() {
        // Recorded with the clean environment on 2026-11-14 (the paths are the fixture's).
        let out = r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty",
            "configDirectory": "/home/op/.claude", "loginExpiresAt": null}"#;
        assert_eq!(parse_status(out), SignIn::SignedOut);
        let key =
            r#"{"loggedIn": true, "authMethod": "api_key", "apiKeySource": "ANTHROPIC_API_KEY"}"#;
        assert_eq!(parse_status(key), SignIn::ApiKey);
        let plan = r#"{"loggedIn": true, "authMethod": "claude.ai", "apiKeySource": "none",
            "subscriptionType": "claude_max_subscription", "email": "op@example.com"}"#;
        assert_eq!(parse_status(plan), SignIn::Plan(Some("Max".into())));
        let unnamed = r#"{"loggedIn": true, "authMethod": "claude.ai"}"#;
        assert_eq!(parse_status(unnamed), SignIn::Plan(None));
        assert_eq!(parse_status("not json"), SignIn::SignedOut);
        let f = Found {
            binary: "/usr/bin/claude".into(),
            version: None,
            missing: vec![],
            sign_in: parse_status(plan),
        };
        let line = f.sign_in_line().unwrap();
        assert!(!line.contains("example.com"));
        assert!(line.contains("Max plan"), "{line}");
    }

    #[test]
    fn signed_out_points_at_the_programs_own_sign_in_never_ghosts() {
        let f = Found {
            binary: "/usr/bin/claude".into(),
            version: Some("2.1.325".into()),
            missing: vec![],
            sign_in: SignIn::SignedOut,
        };
        assert_eq!(f.problem().as_deref(), Some(SIGNED_OUT));
        assert!(SIGNED_OUT.contains("claude auth login"));
        assert!(!SIGNED_OUT.to_lowercase().contains("sign in with"));
    }

    /// A fake `claude` that writes each argv it is given to a file and answers like the
    /// signed-out program: the three questions are asked as argv, and nothing else is run.
    #[tokio::test]
    async fn look_asks_three_questions_as_argv_in_the_clean_environment() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let help = dir.path().join("help.txt");
        std::fs::write(&help, HELP).unwrap();
        let bin = dir.path().join("claude");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$* | ${{ANTHROPIC_AUTH_TOKEN:-none}} ${{CLAUDECODE:-none}}\" >> {log}\n\
                 case \"$1\" in\n--version) echo '2.1.325 (Claude Code)';;\n--help) cat {help};;\n\
                 auth) echo '{{\"loggedIn\": false, \"authMethod\": \"none\"}}'; exit 1;;\nesac\n",
                log = log.display(),
                help = help.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started: HashMap<String, String> = [
            ("HOME", dir.path().to_str().unwrap()),
            ("PATH", "/usr/bin:/bin"),
            ("ANTHROPIC_AUTH_TOKEN", "proxy-token-0123456789"),
            ("CLAUDECODE", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let found = look(&bin, &started).await;
        assert_eq!(found.version.as_deref(), Some("2.1.325"));
        assert!(found.missing.is_empty(), "{:?}", found.missing);
        assert_eq!(found.sign_in, SignIn::SignedOut);
        let asked = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            asked.lines().collect::<Vec<_>>(),
            [
                "--version | none none",
                "--help | none none",
                "auth status --json | none none"
            ]
        );
    }
}
