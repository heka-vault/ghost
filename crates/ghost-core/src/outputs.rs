//! What Ghost makes (ADR 0033): a file in Ghost's `out/` folder, written through the gate.
//!
//! Ghost does real work itself, code included: asked for a presentation, it writes the page.
//! `outputs.write` creates a file and `outputs.replace` replaces one, each a gate operation
//! with its level, snapshot, read-back and audit row. The name is a plain file name with an
//! allowed extension, in `out/` and nowhere else, never through a link; the content is
//! capped. Ghost never runs what it makes: a page is opened by the operator, and a script runs
//! only from a skill, through `ghost-scripts` (ADR 0023).
//!
//! There is one folder, `~/.config/ghost/out/`, whichever agent made the file (ADR 0035).

use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::folder::{arg_str, is_symlink};
use crate::levels::Risk;
use crate::models::{Change, ChangeSet, OutputSnapshot, OutputTarget, Snapshot, Target, WriteFile};
use crate::source::{Operation, PrepareFn, Prepared, ReadTool, ToolHandler, ToolSource, Writer};

pub const SOURCE: &str = "outputs";
pub const OUT_DIR: &str = "out";
pub const MAX_BYTES: usize = 256 * 1024;
pub const EXTENSIONS: [&str; 8] = [
    ".html", ".md", ".txt", ".csv", ".json", ".svg", ".py", ".sh",
];
static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9 ._-]{0,100}$").unwrap());

fn extensions_said() -> String {
    EXTENSIONS.join(", ")
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("an output is a plain file name, like week-in-review.html: no folders, no dots first")]
    BadName,
    #[error("an output ends in one of {}", extensions_said())]
    BadExtension,
    #[error("{} is a link; Ghost writes only into a real out/ folder", .0.display())]
    FolderLink(PathBuf),
    #[error("{} is a link; Ghost does not write through links", .0.display())]
    FileLink(PathBuf),
    #[error("an output stays in out/")]
    Escapes,
    #[error("there is nothing to write")]
    Nothing,
    #[error("an output is at most {} KB; make it smaller", MAX_BYTES / 1024)]
    TooBigToPrepare,
    #[error("an output is at most {} KB", MAX_BYTES / 1024)]
    TooBig,
    #[error("{0} is already there: use outputs.replace, or another name")]
    AlreadyThere(String),
    #[error("{0} is already there; replacing it is its own yes")]
    AlreadyThereToApply(String),
    #[error("there is no {0} to replace: use outputs.write")]
    NothingToReplace(String),
    #[error("OutputWriter cannot apply {0}")]
    CannotApply(String),
    #[error("not an output: {0}")]
    NotAnOutput(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A plain file name: no directories, no dot files, an allowed extension.
pub fn check_name(name: &str) -> Result<String, OutputError> {
    let n = name.trim();
    if !NAME.is_match(n) || n.contains('/') || n.contains('\\') || n.contains("..") {
        return Err(OutputError::BadName);
    }
    let lower = n.to_lowercase();
    if !EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return Err(OutputError::BadExtension);
    }
    Ok(n.to_string())
}

/// Ghost's outputs: `out/` in its folder.
pub fn out_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(OUT_DIR)
}

pub struct OutputWriter {
    folder: PathBuf,
}

impl OutputWriter {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
        }
    }

    fn path(&self, target: &Target) -> Result<PathBuf, OutputError> {
        let Target::Output(OutputTarget { name }) = target else {
            return Err(OutputError::NotAnOutput(target.to_string()));
        };
        let folder = &self.folder;
        if is_symlink(folder) {
            return Err(OutputError::FolderLink(folder.clone()));
        }
        let name = check_name(name)?;
        let path = folder.join(&name);
        if is_symlink(&path) {
            return Err(OutputError::FileLink(path));
        }
        if folder.exists() {
            let real_folder = folder.canonicalize()?;
            let real_path = if path.exists() {
                path.canonicalize()?
            } else {
                real_folder.join(&name)
            };
            if real_path.parent() != Some(real_folder.as_path()) {
                return Err(OutputError::Escapes);
            }
        }
        Ok(path)
    }

    pub fn allowed(&self, target: &Target) -> bool {
        matches!(target, Target::Output(_)) && self.path(target).is_ok()
    }
}

#[async_trait]
impl Writer for OutputWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        let p = self.path(target)?;
        if !p.is_file() {
            return Ok(Snapshot::Output(OutputSnapshot {
                version: "missing".into(),
                exists: false,
                size: 0,
            }));
        }
        let data = std::fs::read(&p)?;
        let digest = format!("{:x}", Sha256::digest(&data));
        Ok(Snapshot::Output(OutputSnapshot {
            version: digest[..16].to_string(),
            exists: true,
            size: data.len() as u64,
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        _idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let p = self.path(target)?;
        match change {
            Change::WriteFile(WriteFile { content, replace }) => {
                let data = content.as_bytes();
                if data.len() > MAX_BYTES {
                    return Err(OutputError::TooBig.into());
                }
                if p.exists() && !replace {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return Err(OutputError::AlreadyThereToApply(name).into());
                }
                write_atomic(&p, data)?;
            }
            other => return Err(OutputError::CannotApply(other.describe()).into()),
        }
        Ok(())
    }
}

/// Write beside the file, then rename over it: a reader never sees half a page.
fn write_atomic(p: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let (tmp, file) = loop {
        let candidate = parent.join(format!(".ghost-{}.tmp", uuid::Uuid::new_v4().simple()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = write_then_rename(&tmp, file, p, data);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_then_rename(tmp: &Path, mut file: File, p: &Path, data: &[u8]) -> std::io::Result<()> {
    file.write_all(data)?;
    drop(file);
    // readable; never executable: Ghost doesn't run it
    std::fs::set_permissions(tmp, Permissions::from_mode(0o644))?;
    std::fs::rename(tmp, p)
}

/// One file in Ghost's outputs, as `GET /outputs` and the desktop list it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputFile {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Seconds since the epoch, as `st_mtime`.
    pub modified: f64,
}

pub fn list_outputs(config_dir: &Path) -> Vec<OutputFile> {
    let folder = out_dir(config_dir);
    if !folder.is_dir() || is_symlink(&folder) {
        return Vec::new();
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&folder)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    let mut out = Vec::new();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_symlink(&p) || !p.is_file() || name.starts_with('.') {
            continue;
        }
        let Ok(st) = p.metadata() else { continue };
        let modified = st
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        out.push(OutputFile {
            name,
            bytes: st.len(),
            path: p,
            modified,
        });
    }
    out
}

/// The `outputs` source: one read (what is in out/) and the two writes.
#[derive(Clone)]
pub struct Outputs {
    pub config_dir: PathBuf,
    pub folder: PathBuf,
    pub writer: Arc<OutputWriter>,
}

impl Outputs {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        let config_dir = config_dir.into();
        let folder = out_dir(&config_dir);
        Self {
            writer: Arc::new(OutputWriter::new(folder.clone())),
            config_dir,
            folder,
        }
    }

    fn prepare(&self, args: &Map<String, Value>, replace: bool) -> Result<Prepared, OutputError> {
        let name = check_name(&arg_str(args, "name"))?;
        let content = arg_str(args, "content");
        if content.trim().is_empty() {
            return Err(OutputError::Nothing);
        }
        if content.len() > MAX_BYTES {
            return Err(OutputError::TooBigToPrepare);
        }
        let exists = self.folder.join(&name).is_file();
        if exists && !replace {
            return Err(OutputError::AlreadyThere(name));
        }
        if replace && !exists {
            return Err(OutputError::NothingToReplace(name));
        }
        let rationale = match arg_str(args, "rationale") {
            r if r.is_empty() => format!("{name}, made for the operator"),
            r => r,
        };
        Ok(Prepared {
            target: Target::Output(OutputTarget { name }),
            changes: ChangeSet::one(Change::WriteFile(WriteFile { content, replace })),
            rationale,
            asked_by: Vec::new(),
        })
    }

    fn prepare_fn(&self, replace: bool) -> PrepareFn {
        let outputs = self.clone();
        Arc::new(move |args: Map<String, Value>| {
            let outputs = outputs.clone();
            Box::pin(async move { Ok(outputs.prepare(&args, replace)?) })
        })
    }

    pub fn operations(&self) -> Vec<Operation> {
        let w = self.writer.clone();
        let props = json!({
            "name": {
                "type": "string",
                "description": format!("A plain file name ending in one of {}.", extensions_said()),
            },
            "content": {"type": "string", "description": "The whole file."},
        });
        let allowed = |w: &Arc<OutputWriter>| {
            let w = w.clone();
            Arc::new(move |t: &Target| w.allowed(t)) as Arc<dyn Fn(&Target) -> bool + Send + Sync>
        };
        vec![
            Operation {
                name: "write".into(),
                description: "Make a file for the operator (a page, a summary, a table, a small \
                              script) in Ghost's outputs folder. Ghost never runs it. Say where \
                              it is in a few words afterwards."
                    .into(),
                properties: props.clone(),
                required: vec!["name".into(), "content".into()],
                prepare: self.prepare_fn(false),
                writer: w.clone(),
                risk: Risk::Low,
                allowed: allowed(&w),
                asked_by: ["make", "write", "create", "build", "draft", "put together"]
                    .map(String::from)
                    .to_vec(),
                tool_name: None,
            },
            Operation {
                name: "replace".into(),
                description: "Replace a file already in Ghost's outputs with a new version.".into(),
                properties: props,
                required: vec!["name".into(), "content".into()],
                prepare: self.prepare_fn(true),
                writer: w.clone(),
                risk: Risk::Medium,
                allowed: allowed(&w),
                asked_by: ["replace", "update", "redo", "rewrite"]
                    .map(String::from)
                    .to_vec(),
                tool_name: None,
            },
        ]
    }

    pub fn read_tools(&self) -> Vec<ReadTool> {
        let config_dir = self.config_dir.clone();
        let handler: ToolHandler = Arc::new(move |_args: Map<String, Value>| {
            let config_dir = config_dir.clone();
            Box::pin(async move {
                let files: Vec<Value> = list_outputs(&config_dir)
                    .into_iter()
                    .map(|f| json!({"name": f.name, "bytes": f.bytes}))
                    .collect();
                let mut out = Map::new();
                out.insert("folder".into(), json!("Ghost's outputs"));
                out.insert("files".into(), Value::Array(files));
                Ok(out)
            })
        });
        vec![ReadTool {
            name: "list_outputs".into(),
            description: "The files Ghost has made in its outputs folder.".into(),
            properties: json!({}),
            required: Vec::new(),
            handler,
            operation: String::new(),
        }]
    }

    pub fn source(&self) -> ToolSource {
        ToolSource {
            name: SOURCE.into(),
            prompt: "You can make self-contained things yourself (a page, a summary, a table, a \
                     small script) with outputs.write; they land in Ghost's outputs folder and \
                     you never run them. Sustained work inside a codebase belongs to the \
                     operator's coding tools, not you."
                .into(),
            read_tools: self.read_tools(),
            operations: self.operations(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{Request, Submission, WriteGate};
    use crate::levels::Autonomy;
    use crate::models::OutcomeStatus;
    use crate::policy::AutonomyPolicy;
    use crate::store::Store;
    use tempfile::TempDir;

    const PAGE: &str = "<!doctype html><title>Week</title><h1>Week in review</h1>";

    struct World {
        tmp: TempDir,
        cfg: PathBuf,
        outputs: Outputs,
        gate: WriteGate,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("ghost");
        std::fs::create_dir(&cfg).unwrap();
        let outputs = Outputs::new(&cfg);
        let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap()));
        gate.register(SOURCE, outputs.operations());
        World {
            tmp,
            cfg,
            outputs,
            gate,
        }
    }

    async fn make(
        gate: &WriteGate,
        outputs: &Outputs,
        op: &str,
        args: Value,
    ) -> anyhow::Result<Submission> {
        let operation = outputs
            .operations()
            .into_iter()
            .find(|o| o.name == op)
            .unwrap();
        let prepared = (operation.prepare)(args.as_object().cloned().unwrap()).await?;
        Ok(gate
            .submit(Request::new(
                "s1",
                SOURCE,
                op,
                prepared.target,
                prepared.changes,
                "",
            ))
            .await?)
    }

    fn output_error(err: &anyhow::Error) -> &OutputError {
        err.downcast_ref::<OutputError>()
            .unwrap_or_else(|| panic!("not an OutputError: {err}"))
    }

    #[tokio::test]
    async fn a_new_file_is_written_at_announce_and_never_executable() {
        let w = world();
        let sub = make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "week.html", "content": PAGE}),
        )
        .await
        .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Announce);
        let outcome = sub.outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Success);
        let path = w.cfg.join("out").join("week.html");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PAGE);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0
        );
        assert!(matches!(
            outcome.readback,
            Some(Snapshot::Output(OutputSnapshot { exists: true, .. }))
        ));
        assert!(
            sub.proposal
                .target
                .spoken()
                .contains("week.html in Ghost's outputs")
        );
        let names: Vec<String> = list_outputs(&w.cfg).into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["week.html"]);
        assert_eq!(sub.proposal.target.scope(), "week.html"); // what a permission rule matches
    }

    #[tokio::test]
    async fn replacing_is_its_own_operation_and_an_ask_rule_makes_it_wait() {
        let w = world();
        make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "week.html", "content": PAGE}),
        )
        .await
        .unwrap();
        let err = make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "week.html", "content": PAGE}),
        )
        .await
        .unwrap_err();
        assert!(output_error(&err).to_string().contains("replace"), "{err}");
        let sub = make(
            &w.gate,
            &w.outputs,
            "replace",
            json!({"name": "week.html", "content": format!("{PAGE}<p>v2")}),
        )
        .await
        .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Announce); // ADR 0034
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        w.gate.set_autonomy(
            AutonomyPolicy::parse("[permissions]\nask = [\"outputs.replace\"]\n", "<test>")
                .unwrap(),
        );
        let sub = make(
            &w.gate,
            &w.outputs,
            "replace",
            json!({"name": "week.html", "content": format!("{PAGE}<p>v3")}),
        )
        .await
        .unwrap();
        assert!(sub.autonomy == Autonomy::Confirm && sub.outcome.is_none()); // waits for a yes
    }

    #[tokio::test]
    async fn a_bad_name_is_refused_before_any_write() {
        let long = format!("{}.md", "a".repeat(120));
        for name in [
            "../escape.html",
            "notes/week.md",
            ".hidden.md",
            "run.exe",
            "week",
            long.as_str(),
            "/etc/passwd.txt",
        ] {
            let w = world();
            let err = make(
                &w.gate,
                &w.outputs,
                "write",
                json!({"name": name, "content": "x"}),
            )
            .await
            .unwrap_err();
            output_error(&err);
            let out = w.cfg.join("out");
            assert!(
                !out.exists() || std::fs::read_dir(&out).unwrap().next().is_none(),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn too_big_or_empty_is_refused() {
        let w = world();
        let err = make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "big.txt", "content": "x".repeat(MAX_BYTES + 1)}),
        )
        .await
        .unwrap_err();
        assert!(output_error(&err).to_string().contains("KB"), "{err}");
        let err = make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "empty.txt", "content": "  "}),
        )
        .await
        .unwrap_err();
        assert!(output_error(&err).to_string().contains("nothing"), "{err}");
    }

    #[tokio::test]
    async fn never_through_a_link() {
        let w = world();
        let elsewhere = w.tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, w.cfg.join("out")).unwrap();
        let target = Target::Output(OutputTarget {
            name: "week.html".into(),
        });
        assert!(!w.outputs.writer.allowed(&target));
        let err = make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "week.html", "content": PAGE}),
        )
        .await
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("not allowed") || text.contains("link"),
            "{text}"
        );
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn the_model_lists_what_is_there() {
        let w = world();
        make(
            &w.gate,
            &w.outputs,
            "write",
            json!({"name": "week.html", "content": PAGE}),
        )
        .await
        .unwrap();
        let out = (w.outputs.read_tools()[0].handler)(Map::new())
            .await
            .unwrap();
        assert_eq!(
            Value::Object(out),
            json!({
                "folder": "Ghost's outputs",
                "files": [{"name": "week.html", "bytes": PAGE.len()}],
            })
        );
    }
}
