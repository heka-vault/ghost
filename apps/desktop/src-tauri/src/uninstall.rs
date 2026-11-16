//! "Remove Ghost from this computer" (#147): the window's one command for it.
//!
//! The window can't remove its own storage while it is open, and an AppImage's mount goes
//! when the app quits, so the shell copies `ghostd` into a folder of its own under the
//! runtime folder, starts `ghostd uninstall --only <kinds> --yes --after <this pid>` in its
//! own process group (through `systemd-cat -t ghost-uninstall` when there is a journal), and
//! quits. That process waits for the app to be gone, stops anything of Ghost's still running,
//! removes what was chosen, says so in a desktop notification and deletes its own copy.
//! Only the window calls this; nothing a model, a tool, an app or the phone does reaches it.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::AppHandle;

use crate::supervisor::system::{daemon_path, journal_available};

/// The kinds `ghostd uninstall --only` takes (`ghost_core::leftovers::Category`).
pub const KINDS: [&str; 6] = [
    "running",
    "other-tools",
    "speech-models",
    "app-storage",
    "data",
    "vault",
];

/// The folder the copy runs from: `$XDG_RUNTIME_DIR/ghost-uninstall` (never under a folder
/// the removal deletes).
pub fn copy_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ghost-uninstall")
}

/// The command line, from the kinds the window sent: each must be one of `KINDS`. `appimage`
/// is the AppImage this app runs from, if it does: the copy runs from outside the mount, so
/// it cannot tell which file to name in the line that finishes, and is told.
pub fn argv(
    ghostd: &Path,
    kinds: &[String],
    pid: u32,
    appimage: Option<&Path>,
) -> Result<Vec<String>, String> {
    if kinds.is_empty() {
        return Err("Nothing was chosen.".into());
    }
    if let Some(k) = kinds.iter().find(|k| !KINDS.contains(&k.as_str())) {
        return Err(format!("Not a kind of thing Ghost leaves: {k}"));
    }
    let mut args = vec![
        ghostd.display().to_string(),
        "uninstall".into(),
        "--only".into(),
        kinds.join(","),
        "--yes".into(),
        "--after".into(),
        pid.to_string(),
    ];
    if let Some(a) = appimage {
        args.extend(["--appimage".into(), a.display().to_string()]);
    }
    Ok(args)
}

fn copy_ghostd() -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = copy_dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let to = dir.join("ghostd");
    let from = daemon_path();
    std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
    std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(to)
}

#[tauri::command]
pub fn remove_ghost(
    app: AppHandle,
    window: tauri::WebviewWindow,
    kinds: Vec<String>,
) -> Result<(), String> {
    // Settings › General in the main window only: never the floating room or the stage.
    if window.label() != "main" {
        return Err("Only Ghost's main window removes Ghost.".into());
    }
    let ghostd = copy_ghostd()?;
    // This app's own AppImage, never one a parent program left in the environment.
    let appimage = ghost_api::appimage::file();
    let args = argv(&ghostd, &kinds, std::process::id(), appimage.as_deref())?;
    let mut cmd = if journal_available() {
        let mut c = Command::new("systemd-cat");
        c.args(["-t", "ghost-uninstall"]).args(&args);
        c
    } else {
        let mut c = Command::new(&args[0]);
        c.args(&args[1..])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    };
    // Its own process group: the app's quit signals the daemon's group, not this one.
    cmd.stdin(Stdio::null()).process_group(0);
    cmd.spawn()
        .map_err(|e| format!("Ghost could not start removing itself: {e}"))?;
    // The ordinary quit: the daemon and the floating room stop (ADR 0038).
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_known_kinds_are_passed_on() {
        let g = Path::new("/run/user/1000/ghost-uninstall/ghostd");
        let a = argv(g, &["running".into(), "app-storage".into()], 42, None).unwrap();
        assert_eq!(
            a,
            [
                "/run/user/1000/ghost-uninstall/ghostd",
                "uninstall",
                "--only",
                "running,app-storage",
                "--yes",
                "--after",
                "42"
            ]
        );
        assert!(argv(g, &["running;rm -rf ~".into()], 42, None).is_err());
        assert!(argv(g, &[], 42, None).is_err());
        // From an AppImage the copy is told the file: it runs from outside the mount.
        let image = Path::new("/home/p/Apps/Ghost 1.AppImage");
        let a = argv(g, &["running".into()], 42, Some(image)).unwrap();
        assert_eq!(
            a[a.len() - 2..],
            ["--appimage", "/home/p/Apps/Ghost 1.AppImage"]
        );
        // The shell's list is the daemon's.
        assert_eq!(KINDS.len(), 6);
    }
}
