//! Read and write KEY=value files (`settings.env`, and the plaintext files a password
//! import reads before it removes them) without ever echoing a value.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::password_import::EnvFiles;

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$").unwrap()
});
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());

/// The file's pairs; empty when there is no file. Quotes around a value and a leading
/// `export` are dropped; comments and blank lines skipped.
pub fn read_env(path: &Path) -> io::Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut out = BTreeMap::new();
    for line in std::fs::read_to_string(path)?.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(m) = LINE.captures(line) {
            let mut value = m[2].to_string();
            let bytes = value.as_bytes();
            if bytes.len() >= 2
                && bytes[0] == bytes[bytes.len() - 1]
                && matches!(bytes[0], b'"' | b'\'')
            {
                value = value[1..value.len() - 1].to_string();
            }
            out.insert(m[1].to_string(), value);
        }
    }
    Ok(out)
}

/// Atomic, mode-restricted, one KEY=value per line, sorted by key. Comments in the old file
/// are kept at the top so operator notes survive a UI edit.
pub fn write_env(path: &Path, values: &BTreeMap<String, String>, mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let mut lines: Vec<String> = Vec::new();
    if path.exists() {
        lines.extend(
            std::fs::read_to_string(path)?
                .lines()
                .filter(|l| l.trim_start().starts_with('#'))
                .map(String::from),
        );
    }
    lines.extend(values.iter().map(|(k, v)| format!("{k}={}", quote(v))));
    let body = format!("{}\n", lines.join("\n"));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Set (`Some`) or remove (`None`) each key, then write the file.
pub fn update_env(
    path: &Path,
    changes: &BTreeMap<String, Option<String>>,
    mode: u32,
) -> io::Result<BTreeMap<String, String>> {
    let mut values = read_env(path)?;
    for (k, v) in changes {
        if !NAME.is_match(k) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid setting name '{k}'"),
            ));
        }
        match v {
            None => {
                values.remove(k);
            }
            Some(v) => {
                values.insert(k.clone(), v.clone());
            }
        }
    }
    write_env(path, &values, mode)?;
    Ok(values)
}

fn quote(v: &str) -> String {
    if v.is_empty() || v.chars().any(|c| " #\"'\\$\n\t".contains(c)) {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// The KEY=value files, for the password import.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvFile;

impl EnvFiles for EnvFile {
    fn read_env(&self, path: &Path) -> io::Result<BTreeMap<String, String>> {
        read_env(path)
    }

    fn update_env(
        &self,
        path: &Path,
        changes: &BTreeMap<String, Option<String>>,
        mode: u32,
    ) -> io::Result<()> {
        update_env(path, changes, mode).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn reads_quotes_exports_and_comments_as_python_did() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.env");
        std::fs::write(
            &p,
            "# note\n\nexport A=1\nB = \"two words\"\nC='x'\nD=\"\"\nnot a line\n",
        )
        .unwrap();
        let v = read_env(&p).unwrap();
        assert_eq!(v["A"], "1");
        assert_eq!(v["B"], "two words");
        assert_eq!(v["C"], "x");
        assert_eq!(v["D"], "");
        assert_eq!(v.len(), 4);
        assert!(
            read_env(&dir.path().join("missing.env"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn update_writes_atomically_keeps_comments_and_refuses_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("settings.env");
        let mut changes = BTreeMap::new();
        changes.insert("GHOST_LLM_MODEL".to_string(), Some("claude".to_string()));
        changes.insert("SPACED".to_string(), Some("a b#c".to_string()));
        update_env(&p, &changes, 0o600).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "GHOST_LLM_MODEL=claude\nSPACED=\"a b#c\"\n");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::write(&p, format!("# keep me\n{text}")).unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("SPACED".to_string(), None);
        changes.insert("Z".to_string(), Some("".to_string()));
        let values = update_env(&p, &changes, 0o600).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "# keep me\nGHOST_LLM_MODEL=claude\nZ=\"\"\n"
        );
        assert_eq!(read_env(&p).unwrap()["Z"], "");
        let mut bad = BTreeMap::new();
        bad.insert("1BAD".to_string(), Some("x".to_string()));
        let err = update_env(&p, &bad, 0o600).unwrap_err();
        assert!(err.to_string().contains("invalid setting name '1BAD'"));
        assert!(
            !dir.path().join("sub").read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')),
            "no temp file left"
        );
    }
}
