//! The tray: Open Ghost, Wake, Stop listening, Start at login, Quit. Closing the window
//! hides it; Quit in the tray is the one off switch (ADR 0038). Stop listening closes
//! Ghost's microphone (ADR 0057): the item turns into Listen again, and the tray's icon
//! is struck through in red, as a smart speaker's mic-off light, until it is turned back on.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager, Wry};

use crate::autostart;
use crate::bridge::api_request;

/// What the daemon last said of its microphone; the tray's item and icon follow it.
pub static NOT_LISTENING: AtomicBool = AtomicBool::new(false);

/// The tray's parts that change with the switch.
pub struct ListenTray {
    item: MenuItem<Wry>,
    on: Image<'static>,
    off: Image<'static>,
    shown: Mutex<Option<bool>>,
}

pub fn build(app: &App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Ghost", true, None::<&str>)?;
    let wake = MenuItem::with_id(app, "wake", "Wake", true, None::<&str>)?;
    let listen = MenuItem::with_id(app, "listen", "Stop listening", true, None::<&str>)?;
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
        Menu::with_items(app, &[&show, &wake, &listen, &sep, &login, &quit])?
    } else {
        Menu::with_items(app, &[&show, &wake, &listen, &sep, &quit])?
    };
    let icon = app.default_window_icon().unwrap().clone().to_owned();
    app.manage(ListenTray {
        item: listen,
        off: struck(&icon),
        on: icon.clone(),
        shown: Mutex::new(None),
    });
    TrayIconBuilder::with_id("main")
        .icon(icon)
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
            "listen" => {
                // Off until turned back on; the window offers 30 minutes and an hour.
                let body = format!("{{\"listening\":{}}}", NOT_LISTENING.load(Ordering::SeqCst));
                tauri::async_runtime::spawn(async move {
                    let _ = api_request("PUT".into(), "/listening".into(), Some(body)).await;
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

/// The daemon's status said whether it is listening: the item, the icon and the tooltip.
pub fn show_listening(app: &AppHandle, not_listening: bool) {
    NOT_LISTENING.store(not_listening, Ordering::SeqCst);
    let Some(parts) = app.try_state::<ListenTray>() else {
        return;
    };
    let mut shown = parts.shown.lock().unwrap_or_else(|e| e.into_inner());
    if *shown == Some(not_listening) {
        return;
    }
    *shown = Some(not_listening);
    let _ = parts.item.set_text(if not_listening {
        "Listen again"
    } else {
        "Stop listening"
    });
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(if not_listening {
            parts.off.clone()
        } else {
            parts.on.clone()
        }));
        if not_listening {
            let _ = tray.set_tooltip(Some("Ghost: not listening (mic off)"));
        }
    }
}

/// The icon faded to gray with a red stroke corner to corner: the mic-off light.
pub fn struck(icon: &Image<'_>) -> Image<'static> {
    let (w, h) = (icon.width() as usize, icon.height() as usize);
    let mut px = icon.rgba().to_vec();
    for p in px.as_chunks_mut::<4>().0 {
        let gray =
            ((u32::from(p[0]) * 30 + u32::from(p[1]) * 59 + u32::from(p[2]) * 11) / 100) as u8;
        p[0] = gray;
        p[1] = gray;
        p[2] = gray;
        p[3] = (u32::from(p[3]) * 3 / 5) as u8;
    }
    let half = (w.min(h) / 14).max(1) as i64;
    for y in 0..h {
        for x in 0..w {
            // distance from the diagonal, scaled to the shorter side
            let d = (x as i64 * h as i64 - y as i64 * w as i64).abs() / w.max(h) as i64;
            if d <= half {
                let i = (y * w + x) * 4;
                px[i..i + 4].copy_from_slice(&[0xd3, 0x2f, 0x2f, 0xff]);
            }
        }
    }
    Image::new_owned(px, w as u32, h as u32)
}

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_struck_icon_is_gray_with_a_red_diagonal() {
        let icon = Image::new_owned([0, 120, 255, 255].repeat(32 * 32), 32, 32);
        let off = struck(&icon);
        let at = |x: usize, y: usize| {
            let i = (y * 32 + x) * 4;
            off.rgba()[i..i + 4].to_vec()
        };
        assert_eq!(at(16, 16), vec![0xd3, 0x2f, 0x2f, 0xff]); // on the stroke
        let corner = at(30, 2);
        assert_eq!(corner[0], corner[1]); // gray
        assert!(corner[3] < 255); // faded
    }
}
