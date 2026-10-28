//! Append-only edits to local text files.
//!
//! The snapshot records the file's length and mtime, so an append is refused if
//! something else wrote in between (the version check catches that).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;

use crate::models::{AppendText, Change, FileSnapshot, FileTarget, Snapshot, Target};
use crate::source::Writer;

/// An append is between 1 and this many characters (the Python `AppendText`'s bounds).
pub const MAX_TEXT: usize = 4000;

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("{} is outside {}", .path.display(), .root.display())]
    Outside { path: PathBuf, root: PathBuf },
    #[error("an append is between 1 and {MAX_TEXT} characters")]
    Text,
    #[error("FileWriter cannot apply {0}")]
    CannotApply(String),
    #[error("not a file: {0}")]
    NotAFile(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Python's `Path.resolve()`: absolute, with links followed as far as the path exists;
/// what is not there yet is kept as written.
pub fn resolve(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(mut real) = cur.canonicalize() {
            for part in rest.iter().rev() {
                real.push(part);
            }
            return real;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return abs,
        }
    }
}

/// Writes under `root` only. `allowed` is the matching check for Operation.
pub struct FileWriter {
    pub root: PathBuf,
}

impl FileWriter {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: resolve(root.as_ref()),
        }
    }

    pub fn allowed(&self, target: &Target) -> bool {
        matches!(target, Target::File(FileTarget { path }) if resolve(path).starts_with(&self.root))
    }
}

#[async_trait]
impl Writer for FileWriter {
    async fn fetch(&self, target: &Target) -> anyhow::Result<Snapshot> {
        let Target::File(FileTarget { path: p }) = target else {
            return Err(FilesError::NotAFile(target.to_string()).into());
        };
        if !p.exists() {
            return Ok(Snapshot::File(FileSnapshot {
                version: "missing".into(),
                exists: false,
                length: 0,
                tail: String::new(),
            }));
        }
        let st = std::fs::metadata(p)?;
        let text = String::from_utf8_lossy(&std::fs::read(p)?).into_owned();
        let tail = if text.trim().is_empty() {
            String::new()
        } else {
            let last = text
                .trim_end_matches('\n')
                .rsplit('\n')
                .next()
                .unwrap_or_default();
            let kept: Vec<char> = last.chars().rev().take(80).collect();
            kept.into_iter().rev().collect()
        };
        let mtime_ns = st
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Ok(Snapshot::File(FileSnapshot {
            version: format!("{mtime_ns}:{}", st.len()),
            exists: true,
            length: text.chars().count() as u64,
            tail,
        }))
    }

    async fn apply(
        &self,
        target: &Target,
        change: &Change,
        _idempotency_key: &str,
    ) -> anyhow::Result<()> {
        let Target::File(FileTarget { path: p }) = target else {
            return Err(FilesError::NotAFile(target.to_string()).into());
        };
        if !self.allowed(target) {
            return Err(FilesError::Outside {
                path: p.clone(),
                root: self.root.clone(),
            }
            .into());
        }
        match change {
            Change::AppendText(AppendText { text }) => {
                if text.is_empty() || text.chars().count() > MAX_TEXT {
                    return Err(FilesError::Text.into());
                }
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut f = OpenOptions::new().append(true).create(true).open(p)?;
                f.write_all(text.as_bytes())?;
                if !text.ends_with('\n') {
                    f.write_all(b"\n")?;
                }
            }
            other => return Err(FilesError::CannotApply(other.describe()).into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{GateError, Request, WriteGate};
    use crate::levels::Risk;
    use crate::models::{ChangeSet, OutcomeStatus};
    use crate::policy::PolicyError;
    use crate::source::Operation;
    use crate::store::Store;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    fn make() -> (TempDir, WriteGate, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let w = Arc::new(FileWriter::new(&root));
        let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap()));
        let allowed = w.clone();
        let mut op = Operation::new(
            "append",
            "",
            Arc::new(|_args| Box::pin(async { anyhow::bail!("the test builds its requests") })),
            w,
        );
        op.risk = Risk::Low;
        op.allowed = Arc::new(move |t| allowed.allowed(t));
        gate.register("notes", vec![op]);
        (tmp, gate, root)
    }

    fn append(target: &Target, text: &str) -> Request {
        Request::new(
            "s",
            "notes",
            "append",
            target.clone(),
            ChangeSet::one(Change::AppendText(AppendText { text: text.into() })),
            "",
        )
    }

    #[tokio::test]
    async fn append_creates_file_and_announces() {
        let (_tmp, gate, root) = make();
        let t = Target::File(FileTarget {
            path: root.join("today.md"),
        });
        let sub = gate.submit(append(&t, "demo is Friday")).await.unwrap();
        assert!(sub.executed());
        assert_eq!(
            std::fs::read_to_string(root.join("today.md")).unwrap(),
            "demo is Friday\n"
        );
        let readback = sub.outcome.unwrap().readback.unwrap();
        assert!(
            readback.spoken().contains("ending 'demo is Friday'"),
            "{}",
            readback.spoken()
        );
        let sub2 = gate.submit(append(&t, "second")).await.unwrap();
        assert!(sub2.executed());
        assert_eq!(
            std::fs::read_to_string(root.join("today.md")).unwrap(),
            "demo is Friday\nsecond\n"
        );
    }

    #[tokio::test]
    async fn outside_root_is_refused() {
        let (tmp, gate, _root) = make();
        let elsewhere = Target::File(FileTarget {
            path: tmp.path().join("elsewhere.md"),
        });
        let err = gate.submit(append(&elsewhere, "x")).await.unwrap_err();
        assert!(
            matches!(err, GateError::Policy(PolicyError::Refused(_))),
            "{err}"
        );
        assert!(!tmp.path().join("elsewhere.md").exists());
    }

    #[tokio::test]
    async fn external_edit_makes_pending_append_stale() {
        let (_tmp, gate, root) = make();
        let file = root.join("a.md");
        let t = Target::File(FileTarget { path: file.clone() });
        std::fs::write(&file, "one\n").unwrap();
        gate.prepare(append(&t, "two")).await.unwrap();
        std::fs::write(&file, "one\nsomeone else wrote this\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(1))
            .unwrap();
        let out = gate
            .confirm_pending("s", "ui", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.status, OutcomeStatus::Failed);
        assert!(out.error.as_deref().unwrap().contains("changed since"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "one\nsomeone else wrote this\n"
        );
    }
}
