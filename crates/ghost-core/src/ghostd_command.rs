//! The `ghostd` command for a person who runs Ghost from the AppImage (ADR 0075).
//!
//! The .deb puts `ghostd` on the PATH; the AppImage cannot, since it is one file mounted at
//! a new path each time it starts. It takes `ghostd` as its first argument instead
//! (`Ghost.AppImage ghostd check`), and this module writes the small script that makes that
//! the `ghostd` command: `~/.local/bin/ghostd`, which runs the AppImage by its own path.
//!
//! Written only at the person's request (Settings › General, or `ghostd command install`
//! from the AppImage), never over a file that is not Ghost's own (its marker line), and
//! listed by `leftovers` so `ghostd uninstall` takes it away.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The line that makes the file Ghost's.
pub const MARKER: &str = "# Ghost's ghostd command (ADR 0075): runs the AppImage below.";

/// `~/.local/bin/ghostd`.
pub fn path(home: &Path) -> PathBuf {
    home.join(".local/bin/ghostd")
}

/// A path in single quotes for `sh`.
fn quoted(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// The script for the AppImage at `appimage`.
pub fn text(appimage: &Path) -> String {
    format!(
        "#!/bin/sh\n{MARKER}\n# Written by Ghost at your request; `ghostd uninstall` removes it.\n\
         exec {} ghostd \"$@\"\n",
        quoted(appimage)
    )
}

/// Whether the file at `p` is Ghost's own script.
pub fn is_ours(p: &Path) -> bool {
    std::fs::read_to_string(p).is_ok_and(|t| t.lines().nth(1) == Some(MARKER))
}

/// The AppImage the script at `p` runs, when it is Ghost's.
pub fn target(p: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(p).ok()?;
    if text.lines().nth(1) != Some(MARKER) {
        return None;
    }
    let line = text.lines().find(|l| l.starts_with("exec '"))?;
    let rest = line.strip_prefix("exec ")?;
    let end = rest.rfind("' ghostd")?;
    Some(PathBuf::from(rest[1..end].replace(r"'\''", "'")))
}

/// What the command is now.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum State {
    /// No file there.
    Absent,
    /// Ghost's, running this AppImage.
    Installed { appimage: PathBuf },
    /// Ghost's, running another path (an AppImage moved or replaced).
    Stale { appimage: PathBuf },
    /// A file that is not Ghost's: left alone.
    Other,
}

pub fn state(home: &Path, appimage: Option<&Path>) -> State {
    let p = path(home);
    if std::fs::symlink_metadata(&p).is_err() {
        return State::Absent;
    }
    match target(&p) {
        Some(t) if Some(t.as_path()) == appimage => State::Installed { appimage: t },
        Some(t) => State::Stale { appimage: t },
        None => State::Other,
    }
}

/// Write the script for `appimage`, 0755, by one rename. Refuses a file that is not Ghost's
/// and an AppImage path that is not an absolute file.
pub fn install(home: &Path, appimage: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if !appimage.is_absolute() || !appimage.is_file() {
        return Err(format!("{} is not the AppImage's file", appimage.display()));
    }
    let p = path(home);
    if std::fs::symlink_metadata(&p).is_ok() && !is_ours(&p) {
        return Err(format!(
            "{} is there already and is not Ghost's; Ghost leaves it alone",
            p.display()
        ));
    }
    let dir = p.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = p.with_extension("ghost-new");
    let _ = std::fs::remove_file(&tmp);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&tmp)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(text(appimage).as_bytes())
            .map_err(|e| e.to_string())?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// Remove the script when it is Ghost's. True when one was removed.
pub fn remove(home: &Path) -> Result<bool, String> {
    let p = path(home);
    if !is_ours(&p) {
        return Ok(false);
    }
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(true)
}

/// Whether `dir` is on the `PATH` this process has (a fresh login has `~/.local/bin` on it on
/// most distributions; a shell started before the folder existed may not).
pub fn on_path(dir: &Path, path_var: Option<&std::ffi::OsStr>) -> bool {
    path_var.is_some_and(|v| std::env::split_paths(v).any(|d| d == dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_runs_the_appimage_by_its_path_and_is_recognised_as_ghosts() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let app = t.path().join("Ghost's Apps/heka ghost.AppImage");
        std::fs::create_dir_all(app.parent().unwrap()).unwrap();
        std::fs::write(&app, b"ELF").unwrap();
        assert_eq!(state(&home, Some(&app)), State::Absent);
        let p = install(&home, &app).unwrap();
        assert_eq!(p, home.join(".local/bin/ghostd"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains(r"exec '/"), "{text}");
        assert!(
            text.contains(r"Ghost'\''s Apps/heka ghost.AppImage' ghostd"),
            "{text}"
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(target(&p).as_deref(), Some(app.as_path()));
        assert_eq!(
            state(&home, Some(&app)),
            State::Installed {
                appimage: app.clone()
            }
        );
        let moved = t.path().join("elsewhere.AppImage");
        assert_eq!(
            state(&home, Some(&moved)),
            State::Stale {
                appimage: app.clone()
            }
        );
        // Installing again (the AppImage moved) replaces Ghost's own script.
        std::fs::write(&moved, b"ELF").unwrap();
        install(&home, &moved).unwrap();
        assert_eq!(target(&p).as_deref(), Some(moved.as_path()));
        assert!(remove(&home).unwrap());
        assert_eq!(state(&home, None), State::Absent);
        assert!(!remove(&home).unwrap());
    }

    #[test]
    fn a_ghostd_that_is_not_ghosts_script_is_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let app = home.join("g.AppImage");
        std::fs::write(&app, b"ELF").unwrap();
        let p = path(home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            "#!/bin/sh\nexec ~/src/ghost/target/debug/ghostd \"$@\"\n",
        )
        .unwrap();
        assert_eq!(state(home, Some(&app)), State::Other);
        assert!(install(home, &app).unwrap_err().contains("not Ghost's"));
        assert!(!remove(home).unwrap());
        assert!(p.exists());
        assert!(install(home, Path::new("relative.AppImage")).is_err());
    }

    #[test]
    fn ghostd_uninstall_lists_ghosts_script_and_only_ghosts() {
        use crate::leftovers::{FilesOnly, Places, inventory};
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let listed = |home: &Path| {
            inventory(&Places::standard(home, &Default::default()), &FilesOnly)
                .into_iter()
                .find(|i| i.key == "ghostd-command")
                .unwrap()
        };
        assert!(!listed(home).exists);
        let app = home.join("g.AppImage");
        std::fs::write(&app, b"ELF").unwrap();
        install(home, &app).unwrap();
        let it = listed(home);
        assert!(it.exists && it.paths == vec![path(home)], "{it:?}");
        // Someone else's ghostd at the same path is not Ghost's to remove.
        std::fs::write(path(home), "#!/bin/sh\necho mine\n").unwrap();
        let it = listed(home);
        assert!(it.paths.is_empty() && !it.exists, "{it:?}");
    }
}
