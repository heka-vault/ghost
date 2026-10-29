//! `~/.config/ghost/settings.env`, the one file the shell loads into the daemon's
//! environment: non-secret settings only (ADR 0036). Passwords live in Ghost's vault, which
//! the daemon opens itself; the shell never reads `~/.config/ghost/env`, `apps.env`, a
//! `.env`, or anything but `settings.env`, and a name in it that looks like a password is
//! left out with a line naming it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's name; nothing else is ever loaded.
pub const FILE_NAME: &str = "settings.env";

/// `$XDG_CONFIG_HOME/ghost/settings.env`, `~/.config/ghost/settings.env` without it.
pub fn settings_env_path() -> PathBuf {
    config_dir().join(FILE_NAME)
}

fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("ghost");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    home.join(".config").join("ghost")
}

/// A name that is a password's: not a setting, never passed on.
pub fn looks_like_a_password(name: &str) -> bool {
    const ENDINGS: [&str; 6] = [
        "_API_KEY",
        "_AUTH_TOKEN",
        "_TOKEN",
        "_SECRET",
        "_PASSWORD",
        "_PASSPHRASE",
    ];
    ENDINGS.iter().any(|e| name.ends_with(e))
}

/// What was read: the settings to pass, and the names left out (never their values).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub values: BTreeMap<String, String>,
    pub left_out: Vec<String>,
}

/// Read `path`, which must be named `settings.env`; a missing file is no settings. The
/// shape is the daemon's own `envfile`: `KEY=value`, an optional `export`, quotes dropped,
/// comments and blank lines skipped.
pub fn load(path: &Path) -> Result<Settings, String> {
    if path.file_name().and_then(|n| n.to_str()) != Some(FILE_NAME) {
        return Err(format!(
            "only {FILE_NAME} is loaded into the daemon's environment, not {}",
            path.display()
        ));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    Ok(parse(&text))
}

fn parse(text: &str) -> Settings {
    let mut out = Settings::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let body = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        let Some((name, value)) = body.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !is_name(name) {
            continue;
        }
        let value = unquote(value.trim());
        if looks_like_a_password(name) {
            out.left_out.push(name.to_string());
            continue;
        }
        out.values.insert(name.to_string(), value);
    }
    out
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn unquote(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2 && b[0] == b[b.len() - 1] && matches!(b[0], b'"' | b'\'') {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_settings_the_way_the_daemon_does() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(
            &p,
            "# the mic\nGHOST_INPUT_DEVICE=ghost.mic\nexport GHOST_WAKE_MODE=\"onnx\"\n\nGHOST_CHIME='no'\nnot a line\n9BAD=x\n",
        )
        .unwrap();
        let s = load(&p).unwrap();
        assert_eq!(
            s.values,
            BTreeMap::from([
                ("GHOST_INPUT_DEVICE".to_string(), "ghost.mic".to_string()),
                ("GHOST_WAKE_MODE".to_string(), "onnx".to_string()),
                ("GHOST_CHIME".to_string(), "no".to_string()),
            ])
        );
        assert!(s.left_out.is_empty());
    }

    #[test]
    fn a_missing_file_is_no_settings() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join(FILE_NAME)).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn never_loads_a_file_that_is_not_settings_env() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["env", "apps.env", ".env", "ghost.env", "settings.env.bak"] {
            let p = dir.path().join(name);
            std::fs::write(&p, "ANTHROPIC_AUTH_TOKEN=sk-live-0123456789\n").unwrap();
            let err = load(&p).unwrap_err();
            assert!(err.contains("only settings.env is loaded"), "{name}: {err}");
            assert!(!err.contains("sk-live"), "no value in the error");
        }
    }

    #[test]
    fn a_password_in_settings_env_is_left_out_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(
            &p,
            "GHOST_INPUT_DEVICE=ghost.mic\nANTHROPIC_AUTH_TOKEN=sk-live-0123456789\nGITHUB_TOKEN=ghp_x\nGHOST_OPENROUTER_API_KEY=or-1\nJEV_SECRET=j\n",
        )
        .unwrap();
        let s = load(&p).unwrap();
        assert_eq!(
            s.values.keys().collect::<Vec<_>>(),
            vec!["GHOST_INPUT_DEVICE"]
        );
        assert_eq!(
            s.left_out,
            vec![
                "ANTHROPIC_AUTH_TOKEN",
                "GITHUB_TOKEN",
                "GHOST_OPENROUTER_API_KEY",
                "JEV_SECRET"
            ]
        );
        assert!(
            !format!("{s:?}").contains("sk-live"),
            "a left-out value is nowhere"
        );
    }

    #[test]
    fn the_path_is_ghosts_folder() {
        let p = settings_env_path();
        assert!(p.ends_with("ghost/settings.env"), "{}", p.display());
    }
}
