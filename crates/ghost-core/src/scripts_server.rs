//! `ghost-scripts`: the one way a skill's scripts run (ADR 0023), as an app Ghost ships,
//! so a run is a change through the gate like any other (ADR 0034).
//!
//! Connected in `apps.json` like any app that runs on this computer; `run` is a change, so
//! the operator's permissions decide whether it asks (`ask = ["scripts.run"]`):
//!
//! ```json
//! {"apps": {"scripts": {"title": "Skill scripts", "command": "ghost-scripts",
//!                       "args": ["--skills", "~/.config/ghost/skills",
//!                                "--root", "~/projects/x"]}}}
//! ```
//!
//! A script runs with a minimal environment (no secrets unless the app's `env` names
//! them), in its skill's folder, with a timeout and an output cap; the audit row carries the
//! arguments and the outcome. `list_scripts` is the read.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TIMEOUT_SECS: f64 = 60.0;
pub const OUTPUT_CAP: usize = 20_000;
/// The interpreter for a script's suffix; a file with another suffix runs by itself when
/// it is executable.
pub const INTERPRETERS: [(&str, &[&str]); 4] = [
    (".py", &["python3"]),
    (".sh", &["bash"]),
    (".js", &["node"]),
    (".ts", &["node"]),
];
/// What a script's environment keeps of the server's.
pub const PASS_ENV: [&str; 6] = ["PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR"];
/// A project's skills, under `--root`.
const ROOT_FOLDERS: [&str; 3] = ["skills", ".agents/skills", ".claude/skills"];

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ScriptsError {
    #[error("{0} is not a skill name")]
    NotASkillName(String),
    #[error("no skill named {0}")]
    NoSkill(String),
    #[error("{script} is not a script of the {skill} skill")]
    NotAScript { script: String, skill: String },
    #[error("{0} is not runnable (not an interpreter's file or executable)")]
    NotRunnable(String),
    #[error("{script} ran past {secs}s and was stopped")]
    TimedOut { script: String, secs: String },
    #[error("couldn’t start {script}: {reason}")]
    Start { script: String, reason: String },
}

/// Python's `repr` of a string, for the messages that quote what was asked.
fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `shlex.join`: each word as a shell would read it back.
fn shell_join<'a>(words: impl IntoIterator<Item = &'a str>) -> String {
    words
        .into_iter()
        .map(|w| {
            if w.is_empty() {
                "''".to_string()
            } else if w
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
            {
                w.to_string()
            } else {
                format!("'{}'", w.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The interpreter a suffix (with its dot) runs under, if any.
pub fn interpreter(suffix: &str) -> Option<&'static [&'static str]> {
    INTERPRETERS
        .iter()
        .find(|(s, _)| *s == suffix)
        .map(|(_, cmd)| *cmd)
}

fn suffix(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default()
}

/// The last `cap` characters, as Python's `text[-cap:]`.
fn tail(text: &str, cap: usize) -> String {
    let n = text.chars().count();
    if n <= cap {
        return text.to_string();
    }
    text.chars().skip(n - cap).collect()
}

/// What `run` answers: the audit row carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub skill: String,
    pub script: String,
    /// The script and its arguments, as a shell would read them.
    pub command: String,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug)]
pub struct ScriptRunner {
    pub skills_dir: PathBuf,
    pub roots: Vec<PathBuf>,
}

impl ScriptRunner {
    pub fn new(skills_dir: PathBuf, roots: Vec<PathBuf>) -> Self {
        ScriptRunner { skills_dir, roots }
    }

    /// The skill's folder: Ghost's skills, then each root's `skills`, `.agents/skills`
    /// and `.claude/skills`.
    pub fn folder(&self, skill: &str) -> Result<PathBuf, ScriptsError> {
        let skill = skill.trim().to_lowercase();
        if skill.is_empty() || skill.contains('/') || skill.starts_with('.') {
            return Err(ScriptsError::NotASkillName(repr(&skill)));
        }
        let name = skill.as_str();
        let places = std::iter::once(self.skills_dir.join(name)).chain(
            self.roots
                .iter()
                .flat_map(|r| ROOT_FOLDERS.iter().map(move |f| r.join(f).join(name))),
        );
        for p in places {
            if p.join("SKILL.md").is_file() {
                return Ok(p);
            }
        }
        Err(ScriptsError::NoSkill(repr(&skill)))
    }

    /// The scripts a skill carries: files an interpreter runs, or executables, at most
    /// three folders deep, hidden folders and `SKILL.md` left out, symlinks left out.
    pub fn scripts(&self, skill: &str) -> Result<Vec<String>, ScriptsError> {
        let folder = self.folder(skill)?;
        let mut found: Vec<(Vec<String>, PathBuf)> = Vec::new();
        walk(&folder, &folder, &mut found);
        found.sort();
        let mut out = Vec::new();
        for (parts, p) in found {
            if !p.is_file() || p.is_symlink() || p.file_name().is_some_and(|n| n == "SKILL.md") {
                continue;
            }
            if parts.iter().any(|part| part.starts_with('.')) || parts.len() > 3 {
                continue;
            }
            if interpreter(&suffix(&p)).is_some() || executable(&p) {
                out.push(parts.join("/"));
            }
        }
        Ok(out)
    }

    /// Run one of a skill's scripts with the server's own environment, less everything
    /// but `PASS_ENV` and `GHOST_SCRIPT_*`.
    pub async fn run(
        &self,
        skill: &str,
        script: &str,
        args: &[String],
    ) -> Result<RunResult, ScriptsError> {
        let environ: Vec<(String, String)> = std::env::vars().collect();
        self.run_with_env(skill, script, args, environ).await
    }

    /// `run`, with the environment given in place of the process's.
    pub async fn run_with_env(
        &self,
        skill: &str,
        script: &str,
        args: &[String],
        environ: impl IntoIterator<Item = (String, String)>,
    ) -> Result<RunResult, ScriptsError> {
        let not_a_script = || ScriptsError::NotAScript {
            script: repr(script),
            skill: skill.to_string(),
        };
        let folder = self.folder(skill)?;
        let real_folder = folder.canonicalize().map_err(|_| not_a_script())?;
        let target = folder
            .join(script)
            .canonicalize()
            .map_err(|_| not_a_script())?;
        if target == real_folder
            || !target.starts_with(&real_folder)
            || !target.is_file()
            || target.is_symlink()
        {
            return Err(not_a_script());
        }
        let rel = target
            .strip_prefix(&real_folder)
            .map_err(|_| not_a_script())?
            .to_string_lossy()
            .to_string();
        if !self.scripts(skill)?.contains(&rel) {
            return Err(ScriptsError::NotRunnable(repr(script)));
        }
        let interp = interpreter(&suffix(&target)).unwrap_or(&[]);
        let target_str = target.to_string_lossy().to_string();
        let mut cmd: Vec<String> = interp.iter().map(|s| s.to_string()).collect();
        cmd.push(target_str.clone());
        cmd.extend(args.iter().cloned());
        let env: Vec<(String, String)> = environ
            .into_iter()
            .filter(|(k, _)| PASS_ENV.contains(&k.as_str()) || k.starts_with("GHOST_SCRIPT_"))
            .collect();
        let mut command = tokio::process::Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .current_dir(&folder)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The operator declared this run through the gate.
        let child = command.spawn().map_err(|e| ScriptsError::Start {
            script: script.to_string(),
            reason: e.to_string(),
        })?;
        let done = match tokio::time::timeout(
            Duration::from_secs_f64(TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(done)) => done,
            Ok(Err(e)) => {
                return Err(ScriptsError::Start {
                    script: script.to_string(),
                    reason: e.to_string(),
                });
            }
            Err(_) => {
                return Err(ScriptsError::TimedOut {
                    script: script.to_string(),
                    secs: format!("{TIMEOUT_SECS:.0}"),
                });
            }
        };
        let exit = done
            .status
            .code()
            .or_else(|| done.status.signal().map(|s| -s))
            .unwrap_or(-1);
        Ok(RunResult {
            skill: skill.to_string(),
            script: script.to_string(),
            command: shell_join(cmd[interp.len()..].iter().map(String::as_str)),
            exit,
            stdout: tail(&String::from_utf8_lossy(&done.stdout), OUTPUT_CAP),
            stderr: tail(&String::from_utf8_lossy(&done.stderr), OUTPUT_CAP),
        })
    }
}

fn executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// Every path under `dir`, with its parts relative to `root`; a symlinked folder is not
/// entered.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(Vec<String>, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let parts: Vec<String> = p
            .strip_prefix(root)
            .map(|r| {
                r.components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        if p.is_dir() && !p.is_symlink() {
            walk(root, &p, out);
        }
        out.push((parts, p));
    }
}

// --- the server -----------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct ListScripts {
    pub skill: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct Run {
    pub skill: String,
    pub script: String,
    #[serde(default)]
    pub args: Option<Vec<String>>,
}

/// A tool's answer as the Python SDK wrote it: JSON text, two spaces of indent.
fn text<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

#[derive(Serialize)]
struct Listed {
    skill: String,
    scripts: Vec<String>,
}

fn reply(tool: &str, result: Result<String, ScriptsError>) -> CallToolResult {
    match result {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(v)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!(
            "Error executing tool {tool}: {e}"
        ))]),
    }
}

pub struct ScriptsServer {
    runner: ScriptRunner,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ScriptsServer {
    pub fn new(skills_dir: PathBuf, roots: Vec<PathBuf>) -> Self {
        ScriptsServer {
            runner: ScriptRunner::new(skills_dir, roots),
            tool_router: Self::tool_router(),
        }
    }

    /// The tools this server offers, as the client lists them.
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    #[tool(
        description = "List the scripts a skill carries (files an interpreter runs, or executables).",
        annotations(read_only_hint = true)
    )]
    fn list_scripts(&self, Parameters(p): Parameters<ListScripts>) -> CallToolResult {
        reply(
            "list_scripts",
            self.runner.scripts(&p.skill).map(|scripts| {
                text(&Listed {
                    skill: p.skill.clone(),
                    scripts,
                })
            }),
        )
    }

    #[tool(
        description = "Run one of a skill's scripts with arguments; returns exit code and output."
    )]
    async fn run(&self, Parameters(p): Parameters<Run>) -> CallToolResult {
        let args = p.args.unwrap_or_default();
        reply(
            "run",
            self.runner
                .run(&p.skill, &p.script, &args)
                .await
                .map(|r| text(&r)),
        )
    }
}

#[tool_handler(router = self.tool_router, name = "ghost-scripts")]
impl ServerHandler for ScriptsServer {}

#[cfg(test)]
mod tests {
    //! The gated scripts server (ADR 0023, ADR 0034): a skill's scripts run only through it.

    use std::collections::BTreeSet;

    use super::*;

    fn make_skill_with_script(folder: &Path) -> PathBuf {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            "---\nname: counter\ndescription: Counts.\n---\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("count.py"),
            "import sys, os\nprint('args', sys.argv[1:])\n\
             print('secret', os.environ.get('GHOST_QA_SECRET', '<unset>'))\n",
        )
        .unwrap();
        std::fs::write(folder.join("notes.txt"), "not a script").unwrap();
        folder.to_path_buf()
    }

    #[tokio::test]
    async fn scripts_run_in_their_folder_with_a_minimal_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join("skills");
        make_skill_with_script(&skills.join("counter"));
        std::os::unix::fs::symlink(
            tmp.path().join("elsewhere.py"),
            skills.join("counter/link.py"),
        )
        .unwrap();
        let runner = ScriptRunner::new(skills.clone(), vec![]);
        assert_eq!(runner.scripts("counter").unwrap(), ["count.py"]);
        // a project root's skills (--root) may sit in any of the three usual folders
        let root = tmp.path().join("repo");
        make_skill_with_script(&root.join(".claude").join("skills").join("deploy"));
        assert_eq!(
            ScriptRunner::new(skills.clone(), vec![root])
                .scripts("deploy")
                .unwrap(),
            ["count.py"]
        );
        let environ =
            std::env::vars().chain([("GHOST_QA_SECRET".to_string(), "leak?".to_string())]);
        let out = runner
            .run_with_env("counter", "count.py", &["a".into(), "b".into()], environ)
            .await
            .unwrap();
        assert_eq!(out.exit, 0, "{}", out.stderr);
        assert!(out.stdout.contains("args ['a', 'b']"), "{}", out.stdout);
        assert!(out.stdout.contains("secret <unset>")); // no secret unless the server's env names it
        assert!(out.command.ends_with("/count.py a b"), "{}", out.command);
        let e = runner.run("counter", "notes.txt", &[]).await.unwrap_err();
        assert!(e.to_string().contains("not runnable"), "{e}");
        let e = runner
            .run("counter", "../../etc/passwd", &[])
            .await
            .unwrap_err();
        assert!(e.to_string().contains("not a script"), "{e}");
        let e = runner.run("counter", "link.py", &[]).await.unwrap_err();
        assert!(e.to_string().contains("not a script"), "{e}");
        let e = runner.run("nope", "count.py", &[]).await.unwrap_err();
        assert!(e.to_string().contains("no skill"), "{e}");
        assert_eq!(interpreter(".py"), Some(&["python3"][..])); // the interpreter the runner uses for .py
    }

    #[test]
    fn scripts_server_declares_the_shape_the_policy_expects() {
        let srv = ScriptsServer::new(PathBuf::from("/nonexistent"), vec![]);
        let tools: std::collections::BTreeMap<String, rmcp::model::Tool> = srv
            .tools()
            .into_iter()
            .map(|t| (t.name.to_string(), t))
            .collect();
        assert_eq!(
            tools.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["list_scripts", "run"])
        );
        assert_eq!(
            tools["list_scripts"]
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint),
            Some(true)
        );
        assert!(tools["run"].annotations.is_none());
        let properties: BTreeSet<&str> = tools["run"].input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(properties, BTreeSet::from(["skill", "script", "args"]));
    }

    #[test]
    fn a_command_is_joined_as_a_shell_reads_it() {
        assert_eq!(
            shell_join(["/tmp/a b/x.py", "a", "", "it's", ";x", "ok-_.:/@%+=,"]),
            "'/tmp/a b/x.py' a '' 'it'\"'\"'s' ';x' ok-_.:/@%+=,"
        );
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
    }
}
