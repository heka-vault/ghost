//! The tray: Open Ghost, Wake, Start at login, Quit. Closing the window hides it; Quit in
//! the tray is the one off switch (ADR 0038).

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager};

use crate::autostart;
use crate::bridge::api_request;

pub fn build(app: &App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Ghost", true, None::<&str>)?;
    let wake = MenuItem::with_id(app, "wake", "Wake", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(
        app,
        "login",
        "Start at login",
        true,
        autostart::wanted(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Ghost", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = if autostart::managed() {
        Menu::with_items(app, &[&show, &wake, &sep, &login, &quit])?
    } else {
        Menu::with_items(app, &[&show, &wake, &sep, &quit])?
    };
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .tooltip("Ghost")
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "wake" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = api_request("POST".into(), "/wake".into(), None).await;
                    let _ = app.emit("ghost://woke", ());
                });
            }
            "login" => {
                let on = login.is_checked().unwrap_or(true);
                autostart::set(app, on);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
