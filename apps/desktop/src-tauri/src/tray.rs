//! The tray: Open Ghost, Wake, Stop listening, Start at login, Quit. Closing the window
//! hides it; Quit in the tray is the one off switch (ADR 0038). Stop listening closes
//! Ghost's microphone (ADR 0057): the item turns into Listen again, and the tray's icon
//! is struck through in red, as a smart speaker's mic-off light, until it is turned back on.
//! The icon is Ghost's mark as a one-colour glyph (docs/design-system.md, The mark; drawn by
//! `packages/ui/src/mark.ts`, written by `make marks`): the cool ink at rest, the state's
//! colour in a session (green listening, violet reaching, cyan speaking, amber for a yes), grey
//! and struck through when not listening.
//! Float over the desktop turns the floating room on and off (ADR 0059): the quick way to hide
//! it, where a screen share can't be seen. During a share, Show Ghost in this share puts it in
//! the share or takes it out, for that share only; Ghost on stage opens the stage window, the
//! floating room drawn on its own backdrop for showing Ghost to people (ADR 0059, 2026-11-13).

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

/// The tray's icon, by what it shows (`look`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Rest,
    Listening,
    Reaching,
    Speaking,
    Waiting,
    Off,
}

/// What the tray shows: not listening first, then a change waiting for a yes, then the phase.
pub fn look(not_listening: bool, phase: Option<&str>, pending: bool) -> Look {
    if not_listening {
        return Look::Off;
    }
    if pending {
        return Look::Waiting;
    }
    match phase {
        Some("listening") => Look::Listening,
        Some("thinking") => Look::Reaching,
        Some("speaking") => Look::Speaking,
        _ => Look::Rest,
    }
}

fn image(l: Look) -> Image<'static> {
    match l {
        Look::Rest => tauri::include_image!("icons/tray/rest.png"),
        Look::Listening => tauri::include_image!("icons/tray/listening.png"),
        Look::Reaching => tauri::include_image!("icons/tray/reaching.png"),
        Look::Speaking => tauri::include_image!("icons/tray/speaking.png"),
        Look::Waiting => tauri::include_image!("icons/tray/waiting.png"),
        Look::Off => tauri::include_image!("icons/tray/off.png"),
    }
}

/// The tray's parts that change with the switch and the session.
pub struct ListenTray {
    item: MenuItem<Wry>,
    shown: Mutex<Option<bool>>,
    /// The last phase and pending the daemon said, and the icon on show.
    phase: Mutex<(Option<String>, bool)>,
    icon: Mutex<Option<Look>>,
}

/// The floating room's items, where the desktop has it: the switch, and the share's toggle.
pub struct FloatTray(CheckMenuItem<Wry>, CheckMenuItem<Wry>);

pub fn build(app: &App, floating: &crate::floating::Status) -> tauri::Result<()> {
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
    let float = CheckMenuItem::with_id(
        app,
        "float",
        "Float over the desktop",
        true,
        floating.settings.on,
        None::<&str>,
    )?;
    let (share_on, share_checked) = share_item(floating);
    let share = CheckMenuItem::with_id(
        app,
        "share",
        "Show Ghost in this share",
        share_on,
        share_checked,
        None::<&str>,
    )?;
    let stage = MenuItem::with_id(app, "stage", "Ghost on stage", true, None::<&str>)?;
    let floats = !matches!(floating.support, crate::floating::Support::No { .. });
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = vec![&show, &wake, &listen];
    if floats {
        items.push(&float);
        items.push(&share);
    }
    items.push(&stage);
    items.push(&sep);
    if autostart::managed() {
        items.push(&login);
    }
    items.push(&quit);
    let menu = Menu::with_items(app, &items)?;
    app.manage(FloatTray(float.clone(), share.clone()));
    app.manage(ListenTray {
        item: listen,
        shown: Mutex::new(None),
        phase: Mutex::new((None, false)),
        icon: Mutex::new(Some(Look::Rest)),
    });
    TrayIconBuilder::with_id("main")
        .icon(image(Look::Rest))
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
            "float" => {
                let on = float.is_checked().unwrap_or(false);
                if let Some(h) = app.try_state::<std::sync::Arc<crate::floating::Host>>() {
                    h.set_on(on);
                }
            }
            "share" => {
                if let Some(h) = app.try_state::<std::sync::Arc<crate::floating::Host>>() {
                    h.share_toggle();
                }
            }
            "stage" => crate::floating::stage::open(app),
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
    {
        let mut shown = parts.shown.lock().unwrap_or_else(|e| e.into_inner());
        if *shown != Some(not_listening) {
            *shown = Some(not_listening);
            let _ = parts.item.set_text(if not_listening {
                "Listen again"
            } else {
                "Stop listening"
            });
            if not_listening && let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_tooltip(Some("Ghost: not listening (mic off)"));
            }
        }
    }
    show_look(app, &parts);
}

/// The daemon said its phase (and whether a change waits for a yes): the icon follows it.
/// `None` leaves what was said before.
pub fn show_phase(app: &AppHandle, phase: Option<&str>, pending: Option<bool>) {
    let Some(parts) = app.try_state::<ListenTray>() else {
        return;
    };
    {
        let mut p = parts.phase.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(phase) = phase {
            p.0 = Some(phase.to_owned());
        }
        if let Some(pending) = pending {
            p.1 = pending;
        }
    }
    show_look(app, &parts);
}

fn show_look(app: &AppHandle, parts: &ListenTray) {
    let next = {
        let p = parts.phase.lock().unwrap_or_else(|e| e.into_inner());
        look(NOT_LISTENING.load(Ordering::SeqCst), p.0.as_deref(), p.1)
    };
    let mut icon = parts.icon.lock().unwrap_or_else(|e| e.into_inner());
    if *icon == Some(next) {
        return;
    }
    *icon = Some(next);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(image(next)));
    }
}

/// The share's item: usable only while the floating room is on and a share is seen, checked
/// while Ghost is in it.
fn share_item(s: &crate::floating::Status) -> (bool, bool) {
    let during = s.settings.on && s.capture == crate::floating::capture::Capture::Yes;
    (
        during,
        during && s.sharing == crate::floating::share::Sharing::Shown,
    )
}

/// The floating room's switch moved (from Settings or here), or a share began or ended: the
/// items follow it.
pub fn show_floating(app: &AppHandle, s: &crate::floating::Status) {
    if let Some(item) = app.try_state::<FloatTray>() {
        let _ = item.0.set_checked(s.settings.on);
        let (on, checked) = share_item(s);
        let _ = item.1.set_enabled(on);
        let _ = item.1.set_checked(checked);
    }
}

/// Bring the main window to the operator: onto the workspace they are on and to the front.
///
/// A window that is showing but not focused may be on another workspace or behind others.
/// Raising it in place would, on GNOME, either switch the operator's workspace to it or, with
/// no activation token, only flash "Ghost is ready". Unmapping it and mapping it again makes
/// it a new window to the compositor, which puts it on the current workspace and gives it the
/// focus (docs/floating-room.md, Workspaces). Its page, scroll and drafts are kept: a
/// hidden window keeps its webview.
pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let visible = w.is_visible().unwrap_or(false);
        let focused = w.is_focused().unwrap_or(false);
        if visible && focused {
            return;
        }
        if visible {
            let _ = w.hide();
        }
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_shows_not_listening_then_a_yes_then_the_phase() {
        assert_eq!(look(true, Some("speaking"), true), Look::Off);
        assert_eq!(look(false, Some("listening"), true), Look::Waiting);
        assert_eq!(look(false, Some("listening"), false), Look::Listening);
        assert_eq!(look(false, Some("thinking"), false), Look::Reaching);
        assert_eq!(look(false, Some("speaking"), false), Look::Speaking);
        assert_eq!(look(false, Some("passive"), false), Look::Rest);
        assert_eq!(look(false, None, false), Look::Rest);
    }

    /// Pixel (x, y) of a tray icon, RGBA.
    fn px(i: &Image<'_>, x: u32, y: u32) -> [u8; 4] {
        let k = ((y * i.width() + x) * 4) as usize;
        i.rgba()[k..k + 4].try_into().unwrap()
    }

    #[test]
    fn every_tray_icon_is_the_glyph_in_its_own_colour() {
        let looks = [
            Look::Rest,
            Look::Listening,
            Look::Reaching,
            Look::Speaking,
            Look::Waiting,
            Look::Off,
        ];
        let mut middles = Vec::new();
        for l in looks {
            let i = image(l);
            assert_eq!((i.width(), i.height()), (32, 32), "{l:?}");
            // The corners are clear: a glyph, not a tile.
            assert_eq!(px(&i, 0, 0)[3], 0, "{l:?}");
            // The body at the forehead, above the eyes.
            middles.push(px(&i, 16, 6));
        }
        for (a, ma) in middles.iter().enumerate() {
            assert!(ma[3] > 200, "{:?} is drawn", looks[a]);
            for (b, mb) in middles.iter().enumerate().skip(a + 1) {
                assert_ne!(ma[..3], mb[..3], "{:?} and {:?} differ", looks[a], looks[b]);
            }
        }
        // Not listening carries the red stroke through the middle.
        let off = px(&image(Look::Off), 16, 16);
        assert!(off[0] > 150 && off[1] < 100, "red at the middle: {off:?}");
    }
}
