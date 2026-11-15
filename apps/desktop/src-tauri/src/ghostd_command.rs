//! The `ghostd` command for a person with the AppImage (ADR 0075).
//!
//! `Ghost.AppImage ghostd <args>` hands its arguments to the bundled `ghostd` (the AppImage's
//! launcher runs this binary), and Settings › General's "Install the ghostd command" runs
//! `ghostd command install`, which writes `~/.local/bin/ghostd` (`ghost_core::ghostd_command`,
//! the one writer). Only the main window calls the commands; nothing a model, a tool, an app,
//! an outside client or the phone does reaches them.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::supervisor::system::daemon_path;

/// The first argument that makes the app the daemon's command line.
pub const ARG: &str = "ghostd";

/// Replace this process with the bundled `ghostd` and `args`. Returns only on failure, which
/// ends the process with the shell's usual code for a command that could not run.
pub fn exec_ghostd(args: impl Iterator<Item = OsString>) -> ! {
    let ghostd = daemon_path();
    let err = Command::new(&ghostd).args(args).exec();
    eprintln!("ghost: cannot run {}: {err}", ghostd.display());
    std::process::exit(127)
}

fn run_command(args: &[&str]) -> Result<(bool, String, String), String> {
    let out = Command::new(daemon_path())
        .arg("command")
        .args(args)
        .output()
        .map_err(|e| format!("Ghost could not run ghostd: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

fn main_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only Ghost's main window does this.".into());
    }
    Ok(())
}

/// `ghostd command status --json`: whether this is an AppImage, the script's path and state,
/// and whether its folder is on the PATH.
#[tauri::command]
pub fn ghostd_command_status(window: tauri::WebviewWindow) -> Result<serde_json::Value, String> {
    main_window(&window)?;
    let (_, out, err) = run_command(&["status", "--json"])?;
    serde_json::from_str(&out).map_err(|_| err)
}

/// `ghostd command install`: what it said, or why not.
#[tauri::command]
pub fn ghostd_command_install(window: tauri::WebviewWindow) -> Result<String, String> {
    main_window(&window)?;
    match run_command(&["install"])? {
        (true, out, _) => Ok(out),
        (false, _, err) => Err(err),
    }
}
