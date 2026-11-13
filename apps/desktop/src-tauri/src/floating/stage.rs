//! Ghost on stage (ADR 0059, amended 2026-11-13): the floating room drawn in an ordinary window
//! on its own backdrop, for showing Ghost to people. A meeting's "share a window" can pick it,
//! and a recording of it alone looks finished, where the floating surface shared alone is Ghost
//! on black (its transparent pixels arrive as (0, 0, 0, 0), and a meeting drops the alpha).
//!
//! It is opened on purpose (the tray's Ghost on stage, or Settings) and is meant to be seen,
//! so it never hides from a share. It is a window of this process, native wherever the main
//! window is, and reads the daemon's events as the main window does: what it shows is the
//! floating room's page (`stage.html`, `src/floating/`) with the stage's choices, nothing new
//! from the daemon. It changes nothing the main window cannot.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "stage";
/// What a meeting's window picker lists it as: not the main window's "Ghost".
pub const TITLE: &str = "Ghost · Stage";

/// The stage's own capability: the daemon's bridge (the app's commands) and its events,
/// nothing else of Tauri's. Once, at setup.
pub fn capability(app: &AppHandle) -> tauri::Result<()> {
    app.add_capability(
        tauri::ipc::CapabilityBuilder::new("stage")
            .window(LABEL)
            .permission("core:default")
            .permission("core:event:default"),
    )
}

/// Open the stage, or bring it forward when it is already open.
pub fn open(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("stage.html".into()))
        .title(TITLE)
        .inner_size(1280.0, 720.0)
        .min_inner_size(640.0, 360.0)
        .resizable(true)
        .background_color(tauri::window::Color(5, 6, 10, 255))
        .build();
    if let Err(e) = built {
        eprintln!("ghost: stage: couldn't open: {e}");
    }
}

/// Settings' "Open the stage".
#[tauri::command]
pub fn stage_open(app: AppHandle) {
    open(&app);
}
