//! Reading another tool's file (ADR 0068): only a file on the fixed list, never a sign-in
//! file, never through a link that leaves the home folder, never more than a bound.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// A file on the list, and what came of reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// Read; `apps` entries found in it.
    Read,
    /// Not there: nothing to say.
    Missing,
    /// Larger than Ghost reads.
    TooLarge(u64),
    /// A link to somewhere outside the home folder: not followed.
    Outside,
    /// There, but it could not be read or understood: the sentence says why.
    Unreadable(String),
}

/// File names that hold another tool's sign-in. The detector never opens one, whatever
/// path leads to it (ADR 0068): those are licensed to that tool.
pub const CREDENTIAL_NAMES: [&str; 6] = [
    ".credentials.json",
    "credentials.json",
    "auth.json",
    "credentials",
    ".netrc",
    "keychain",
];

/// Folders whose files are another tool's sign-in, session or private state: nothing
/// under them is read (relative to the home folder).
pub const PRIVATE_FOLDERS: [&str; 4] = [
    ".claude",
    ".config/Cursor",
    ".config/claude",
    ".codex/sessions",
];

/// Whether `path` is a sign-in file or lies in a folder of private state.
pub fn is_credential(home: &Path, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if CREDENTIAL_NAMES.iter().any(|c| name == *c)
        || name.contains("credential")
        || name.contains("token")
        || name.ends_with(".key")
        || name.ends_with(".pem")
    {
        return true;
    }
    if path
        .components()
        .any(|c| matches!(c, Component::Normal(s) if s == "Cookies" || s == "Local Storage"))
    {
        return true;
    }
    PRIVATE_FOLDERS
        .iter()
        .any(|f| path.starts_with(home.join(f)))
}

/// `~/.cursor/mcp.json` for a path under the home folder: how a file is named on screen.
pub fn shown_path(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The path to read or write for `path`: itself, or what its link points at when that
/// is inside the home folder. A link out of it is `Outside`.
pub fn resolve(home: &Path, path: &Path) -> Result<PathBuf, FileState> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(FileState::Missing),
        Err(e) => {
            return Err(FileState::Unreadable(format!(
                "could not be read ({})",
                e.kind()
            )));
        }
    }
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    // The file, or a folder on the way, may be a link: where it really is decides.
    let Ok(real) = path.canonicalize() else {
        return Err(FileState::Missing);
    };
    if !real.starts_with(&home) {
        return Err(FileState::Outside);
    }
    Ok(real)
}

/// The text of `path`, when it is on the list, under `cap` bytes, inside the home folder
/// and not a sign-in file. This is the one function in the module that opens a file to
/// read it, and each path it opens is recorded in `opened`.
pub fn read_bounded(
    home: &Path,
    path: &Path,
    cap: u64,
    opened: &mut Vec<PathBuf>,
) -> Result<String, FileState> {
    if is_credential(home, path) {
        return Err(FileState::Unreadable(
            "is a sign-in file, which Ghost never reads".into(),
        ));
    }
    let real = resolve(home, path)?;
    let home_real = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if is_credential(&home_real, &real) {
        return Err(FileState::Unreadable(
            "is a sign-in file, which Ghost never reads".into(),
        ));
    }
    let meta = std::fs::metadata(&real)
        .map_err(|e| FileState::Unreadable(format!("could not be read ({})", e.kind())))?;
    if !meta.is_file() {
        return Err(FileState::Unreadable("is not a file".into()));
    }
    if meta.len() > cap {
        return Err(FileState::TooLarge(meta.len()));
    }
    opened.push(real.clone());
    let mut f = std::fs::File::open(&real)
        .map_err(|e| FileState::Unreadable(format!("could not be read ({})", e.kind())))?;
    let mut bytes = Vec::new();
    f.by_ref()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| FileState::Unreadable(format!("could not be read ({})", e.kind())))?;
    if bytes.len() as u64 > cap {
        return Err(FileState::TooLarge(bytes.len() as u64));
    }
    String::from_utf8(bytes).map_err(|_| FileState::Unreadable("is not text".into()))
}

/// JSON with `//` and `/* */` comments and trailing commas taken out, strings left alone,
/// for a file whose tool reads it that way. Returns None when the text had none.
pub fn strip_jsonc(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                changed = true;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                changed = true;
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // A comma whose next non-space is a closing bracket is a trailing one.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    // A comment between the comma and the bracket.
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                            j += 1;
                        }
                        j += 2;
                        continue;
                    }
                    break;
                }
                if matches!(chars.get(j), Some('}') | Some(']')) {
                    changed = true;
                } else {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    changed.then_some(out)
}
