//! Start at login (ADR 0038): on by default, off from the tray. The operator's choice is a
//! word in `~/.local/state/ghost/autostart` (`off`); without it the app registers itself at
//! every launch, so a moved AppImage registers its new path. A checkout's debug build never
//! registers: `make dev` is not a login item.

use std::path::PathBuf;

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

fn state_file() -> PathBuf {
    super::supervisor::logfile::default_path().with_file_name("autostart")
}

/// What the operator chose, `true` unless the state file says `off`.
pub fn wanted() -> bool {
    std::fs::read_to_string(state_file())
        .map(|s| s.trim() != "off")
        .unwrap_or(true)
}

/// Whether this build registers login items at all.
pub fn managed() -> bool {
    !cfg!(debug_assertions)
}

/// Make the login item match the choice. Release builds only.
pub fn apply(app: &AppHandle) {
    if !managed() {
        return;
    }
    let al = app.autolaunch();
    let result = if wanted() { al.enable() } else { al.disable() };
    if let Err(e) = result {
        eprintln!("ghost: start at login: {e}");
    }
}

/// The tray's toggle: record the choice, then apply it.
pub fn set(app: &AppHandle, on: bool) {
    let path = state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, if on { "on\n" } else { "off\n" }) {
        eprintln!("ghost: cannot write {}: {e}", path.display());
    }
    apply(app);
}
