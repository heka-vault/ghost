//! Ghost's folder (ADR 0035): everything the operator edits, in one place:
//!
//! ```text
//! ~/.config/ghost/
//! ├── AGENTS.md          # rules for every agent, as the operator wrote them
//! ├── apps.json          # connected apps (ADR 0034)
//! ├── watches.json       # what Ghost watches (ADR 0028)
//! ├── hooks.json         # webhooks it takes (ADR 0027), fixtures in hooks/<name>/
//! ├── skills/<name>/SKILL.md
//! ├── out/               # what Ghost makes (ADR 0033)
//! ├── agents/<slug>/     # agent.json, persona.md, memory.md, notes.md
//! └── policy.toml, env, settings.env, apps.env
//! ```
//!
//! This module reads the folder. The model never writes `watches.json`, `hooks.json` or
//! `apps.json`; a rule appended to `AGENTS.md` goes through the gate (`crate::rules`).
//! What older layouts left here (a `department.json`, a shared `memory.md`, `docs/`,
//! `departments/`) is not read; `stale` names each, with no migration.
//!
//! `watches.json` and `hooks.json` are parsed by the watches and hooks modules, ported
//! beside this one: the folder reads the two files, names a broken one, and hands each
//! body to a [`Parsers`] implementation for the typed watches and hooks.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::models::py_str;

pub const RULES_FILE: &str = "AGENTS.md";
pub const AGENTS_DIR: &str = "agents";
pub const WATCHES_FILE: &str = "watches.json";
pub const HOOKS_FILE: &str = "hooks.json";
/// AGENTS.md as the prompt carries it.
pub const MAX_RULES_CHARS: usize = 8000;
// Left by older layouts, no longer read (ADR 0034, ADR 0035).
pub const OLD_APPS_FILE: &str = "mcp.json";
pub const OLD_DEPARTMENT_FILE: &str = "department.json";
pub const OLD_MEMORY_FILE: &str = "memory.md";
pub const OLD_DOCS_DIR: &str = "docs";
pub const OLD_DEPARTMENTS_DIR: &str = "departments";

/// Whether the path itself is a link (not followed).
pub(crate) fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Python's `Path.expanduser()`: a leading `~` is the operator's home.
pub(crate) fn expanduser(path: &Path) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|s| s.strip_prefix('~')) else {
        return path.to_path_buf();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return path.to_path_buf();
    }
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => Path::new(&home).join(rest.trim_start_matches('/')),
        _ => path.to_path_buf(),
    }
}

/// The model's string argument, as Python read it: `str(args.get(key) or "")`, so a
/// missing, null or empty value is "".
pub(crate) fn arg_str(args: &Map<String, Value>, key: &str) -> String {
    match args.get(key) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => String::new(),
        Some(Value::Array(a)) if a.is_empty() => String::new(),
        Some(Value::Object(o)) if o.is_empty() => String::new(),
        Some(v) => py_str(v),
    }
}

/// A regular file's text, capped; empty when it is missing, a link or unreadable.
pub fn read_text(path: &Path, cap: usize) -> String {
    if is_symlink(path) || !path.is_file() {
        return String::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    if text.chars().count() <= cap {
        text.into_owned()
    } else {
        let head: String = text.chars().take(cap).collect();
        format!("{}…", head.trim_end())
    }
}

/// Ghost's `AGENTS.md`, as every agent's prompt carries it.
pub fn read_rules(config_dir: &Path) -> String {
    read_text(&config_dir.join(RULES_FILE), MAX_RULES_CHARS)
}

/// The file's object, None when there is no file; a broken one is a problem.
fn read_json(file: &Path, problems: &mut Vec<String>) -> Option<Map<String, Value>> {
    if !file.is_file() {
        return None;
    }
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) => {
            problems.push(format!("{}: {e}", file.display()));
            return None;
        }
    };
    let data: Value = match serde_json::from_str(&text) {
        Ok(data) => data,
        Err(e) => {
            problems.push(format!("{}: {e}", file.display()));
            return None;
        }
    };
    match data {
        Value::Object(map) => Some(map),
        _ => {
            problems.push(format!(
                "{}: the file must be an object keyed by name",
                file.display()
            ));
            None
        }
    }
}

/// What an older layout left in Ghost's folder that is no longer read (ADR 0035), one
/// line each, for `ghostd check` and the waiting list. Nothing is moved for the operator.
pub fn stale(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let old = path.join(OLD_DEPARTMENT_FILE);
    if old.is_file() {
        out.push(format!(
            "{}: not read any more; put its watches in {} and its hooks in {} by hand (its \
             title, voice, permissions and servers are not read: permissions are in \
             policy.toml, apps in the Apps screen)",
            old.display(),
            path.join(WATCHES_FILE).display(),
            path.join(HOOKS_FILE).display()
        ));
    }
    if path.join(OLD_APPS_FILE).is_file() {
        out.push(format!(
            "{}: not read any more; rename it {} (it reads the same shape)",
            path.join(OLD_APPS_FILE).display(),
            path.join("apps.json").display()
        ));
    }
    if path.join(OLD_MEMORY_FILE).is_file() {
        out.push(format!(
            "{}: not read any more; each agent keeps its own memory in {}/<name>/memory.md",
            path.join(OLD_MEMORY_FILE).display(),
            path.join(AGENTS_DIR).display()
        ));
    }
    if path.join(OLD_DOCS_DIR).is_dir() {
        out.push(format!(
            "{}: not read any more; make what Ghost should read on demand into a skill",
            path.join(OLD_DOCS_DIR).display()
        ));
    }
    let folder = path.join(OLD_DEPARTMENTS_DIR);
    if folder.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for p in entries {
            out.push(format!(
                "{}: not read any more; hire its agents again, add its rules to {} and its \
                 watches and hooks to {WATCHES_FILE} and {HOOKS_FILE} by hand",
                p.display(),
                path.join(RULES_FILE).display()
            ));
        }
    }
    out
}

/// What the folder needs of `watches.json` and `hooks.json`: their parsers, in the watches
/// and hooks modules. Each takes the file's object (`None` when there is no file; a file
/// that is not an object is already a problem here) and where it came from, and gives
/// back the ones that read, keyed by name, and a problem for each that doesn't.
pub trait Parsers {
    type Watch;
    type Hook;
    fn parse_watches(
        body: Option<&Map<String, Value>>,
        where_: &str,
    ) -> (BTreeMap<String, Self::Watch>, Vec<String>);
    /// `folder` is Ghost's folder, which holds `hooks/<name>/`, the fixtures.
    fn parse_hooks(
        body: Option<&Map<String, Value>>,
        where_: &str,
        folder: &Path,
    ) -> (BTreeMap<String, Self::Hook>, Vec<String>);
}

/// Called after every re-read (the daemon restarts the watches and hooks).
pub type RefreshHook = Box<dyn Fn() + Send + Sync>;

/// Ghost's folder as it is now. `refresh` re-reads `watches.json` and `hooks.json` (the
/// desktop's reload, or the policy's) and runs `on_refresh`.
pub struct GhostFolder<P: Parsers> {
    pub path: PathBuf,
    pub watches: BTreeMap<String, P::Watch>,
    pub hooks: BTreeMap<String, P::Hook>,
    pub problems: Vec<String>,
    /// Called after every re-read (the daemon restarts the watches and hooks).
    pub on_refresh: Vec<RefreshHook>,
    parsers: PhantomData<fn() -> P>,
}

impl<P: Parsers> GhostFolder<P> {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let mut folder = Self {
            path: expanduser(path.as_ref()),
            watches: BTreeMap::new(),
            hooks: BTreeMap::new(),
            problems: Vec::new(),
            on_refresh: Vec::new(),
            parsers: PhantomData,
        };
        folder.refresh();
        folder
    }

    pub fn rules_path(&self) -> PathBuf {
        self.path.join(RULES_FILE)
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.path.join(AGENTS_DIR)
    }

    pub fn watches_path(&self) -> PathBuf {
        self.path.join(WATCHES_FILE)
    }

    pub fn hooks_path(&self) -> PathBuf {
        self.path.join(HOOKS_FILE)
    }

    /// `AGENTS.md`, as every agent's prompt carries it.
    pub fn rules(&self) -> String {
        read_rules(&self.path)
    }

    /// The folder a fresh install needs: `agents/` for agent zero.
    pub fn ensure_layout(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.agents_dir())
    }

    pub fn refresh(&mut self) {
        let mut problems = Vec::new();
        let rules_path = self.rules_path();
        if is_symlink(&rules_path) {
            problems.push(format!(
                "{}: a link; Ghost reads and writes only regular files",
                rules_path.display()
            ));
        }
        let watches_path = self.watches_path();
        let body = read_json(&watches_path, &mut problems);
        let (watches, found) = P::parse_watches(body.as_ref(), &watches_path.display().to_string());
        self.watches = watches;
        problems.extend(found);
        let hooks_path = self.hooks_path();
        let body = read_json(&hooks_path, &mut problems);
        let (hooks, found) =
            P::parse_hooks(body.as_ref(), &hooks_path.display().to_string(), &self.path);
        self.hooks = hooks;
        problems.extend(found);
        problems.extend(stale(&self.path));
        self.problems = problems;
        for hook in &self.on_refresh {
            hook();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// A watch as the folder's tests need it: its name, and `source` as the watches
    /// module gives it (`watch:<name>`). The parsing itself is the watches module's.
    #[derive(Debug, Clone, PartialEq)]
    struct TestWatch {
        name: String,
        app: String,
    }

    impl TestWatch {
        fn source(&self) -> String {
            format!("watch:{}", self.name)
        }
    }

    struct TestParsers;

    impl Parsers for TestParsers {
        type Watch = TestWatch;
        type Hook = Value;

        fn parse_watches(
            body: Option<&Map<String, Value>>,
            _where: &str,
        ) -> (BTreeMap<String, TestWatch>, Vec<String>) {
            let watches = body
                .map(|m| {
                    m.iter()
                        .map(|(name, spec)| {
                            let app = spec["app"].as_str().unwrap_or_default().to_string();
                            (
                                name.clone(),
                                TestWatch {
                                    name: name.clone(),
                                    app,
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            (watches, Vec::new())
        }

        fn parse_hooks(
            body: Option<&Map<String, Value>>,
            _where: &str,
            _folder: &Path,
        ) -> (BTreeMap<String, Value>, Vec<String>) {
            let hooks = body
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            (hooks, Vec::new())
        }
    }

    fn watch() -> Value {
        json!({
            "app": "github",
            "read": "list_workflow_runs",
            "every": 60,
            "items": "workflow_runs",
            "fields": {"id": {"at": "id", "type": "int"}},
            "key": "{id}",
            "title": "Run {id} failed",
        })
    }

    #[test]
    fn an_empty_folder_reads_as_nothing_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = GhostFolder::<TestParsers>::new(tmp.path());
        assert!(folder.watches.is_empty() && folder.hooks.is_empty() && folder.problems.is_empty());
        assert_eq!(folder.rules(), "");
        folder.ensure_layout().unwrap();
        assert!(tmp.path().join("agents").is_dir());
    }

    #[test]
    fn the_rules_are_agents_md_capped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            format!("# Ghost\n\n{}", "x".repeat(9000)),
        )
        .unwrap();
        let text = read_rules(tmp.path());
        assert!(text.starts_with("# Ghost") && text.ends_with('…') && text.chars().count() < 8100);
        assert_eq!(GhostFolder::<TestParsers>::new(tmp.path()).rules(), text);
    }

    #[test]
    fn watches_and_hooks_load_from_their_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("watches.json"),
            json!({"ci": watch()}).to_string(),
        )
        .unwrap();
        let mut folder = GhostFolder::<TestParsers>::new(tmp.path());
        let w = &folder.watches["ci"];
        assert_eq!(w.source(), "watch:ci");
        assert_eq!(w.app, "github");
        // a broken file is a problem, and the folder still reads
        std::fs::write(tmp.path().join("hooks.json"), "{not json").unwrap();
        std::fs::write(tmp.path().join("watches.json"), json!(["ci"]).to_string()).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        folder
            .on_refresh
            .push(Box::new(move || seen2.lock().unwrap().push(1)));
        folder.refresh();
        assert_eq!(*seen.lock().unwrap(), vec![1]);
        assert!(folder.watches.is_empty() && folder.hooks.is_empty());
        assert!(folder.problems.iter().any(|p| p.contains("hooks.json")));
        assert!(
            folder
                .problems
                .iter()
                .any(|p| p.contains("watches.json") && p.contains("keyed by name"))
        );
    }

    #[test]
    fn what_an_older_layout_left_is_named_and_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("department.json"),
            json!({"title": "Ghost", "voice": "bm_george", "watches": {"ci": watch()}}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("memory.md"), "Shared facts.\n").unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("mcp.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("departments").join("marketing")).unwrap();
        std::fs::create_dir_all(root.join("departments").join("sales")).unwrap();
        let lines = stale(root);
        assert_eq!(lines.len(), 6);
        let old = lines
            .iter()
            .find(|line| line.contains("department.json"))
            .unwrap();
        assert!(
            old.contains("watches.json") && old.contains("hooks.json") && old.contains("by hand")
        );
        assert!(lines.iter().any(|line| line.contains("memory.md")));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(&root.join("docs").display().to_string()))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("mcp.json") && line.contains("apps.json"))
        );
        let departments: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("/departments/"))
            .collect();
        let named: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("marketing") || line.contains("sales"))
            .collect();
        assert_eq!(departments, named);
        let folder = GhostFolder::<TestParsers>::new(root);
        assert!(folder.watches.is_empty()); // department.json's watches are not read
        assert_eq!(folder.problems, lines);
    }

    #[test]
    fn an_empty_departments_folder_is_not_named() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("departments")).unwrap();
        assert!(stale(tmp.path()).is_empty());
    }

    #[test]
    fn a_tilde_is_the_operators_home_and_an_argument_reads_as_python_did() {
        let home = std::env::var_os("HOME").unwrap();
        assert_eq!(
            expanduser(Path::new("~/.config/ghost")),
            Path::new(&home).join(".config/ghost")
        );
        assert_eq!(expanduser(Path::new("/tmp/x")), Path::new("/tmp/x"));
        assert_eq!(expanduser(Path::new("~other/x")), Path::new("~other/x"));
        let args: Map<String, Value> = json!({"a": "x", "b": null, "c": 0, "d": 7, "e": false})
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(arg_str(&args, "a"), "x");
        assert_eq!(arg_str(&args, "b"), "");
        assert_eq!(arg_str(&args, "c"), "");
        assert_eq!(arg_str(&args, "d"), "7");
        assert_eq!(arg_str(&args, "e"), "");
        assert_eq!(arg_str(&args, "missing"), "");
    }
}
