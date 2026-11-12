//! What the surface can learn about the desktop at the moment of a wake: the monitors, where
//! the operator is working, whether a full-screen app has the focus, and the workspace.
//!
//! Measured on GNOME Shell 50 under XWayland (2026-11-12, docs/floating-room.md):
//! with a native Wayland window focused, `_NET_ACTIVE_WINDOW` names a window that is not in
//! `_NET_CLIENT_LIST` (mutter's own), and `XQueryPointer` keeps answering the last place the
//! pointer crossed an X window, so neither says where the operator is. A focused X client
//! (in the client list) is still the truth, and `_NET_CURRENT_DESKTOP` follows the
//! workspace. On an X11 session all of it is live. The optional GNOME Shell helper
//! (`apps/desktop/gnome-shell/`) answers the rest from inside the compositor.

use std::ffi::CString;
use std::os::raw::{c_int, c_uchar, c_uint, c_ulong};
use std::time::Duration;

use serde::Deserialize;
use x11::xlib;

use super::place::{Known, Monitor, Rect};
use super::{HELPER_NAME, HELPER_PATH};

/// What each source said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The surface runs through XWayland (the pointer query is not to be believed).
    pub xwayland: bool,
    pub x_pointer: Option<(i32, i32)>,
    /// The focused window when it is a real X client: its frame and whether it is full screen.
    pub x_focus: Option<(Rect, bool)>,
    pub x_workspace: Option<u32>,
    pub helper: Option<HelperWhere>,
}

/// The GNOME Shell helper's answer to `Where()`, in the stage's logical pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct HelperWhere {
    pub pointer: Option<[i32; 2]>,
    pub focus: Option<[i32; 4]>,
    #[serde(default)]
    pub focus_fullscreen: bool,
    pub workspace: Option<u32>,
}

/// Only what is reliable goes into `Known`.
pub fn known(monitors: Vec<Monitor>, seen: &Seen) -> Known {
    if let Some(h) = &seen.helper {
        return Known {
            monitors,
            pointer: h.pointer.map(|[x, y]| (x, y)),
            focus: h.focus.map(|[x, y, width, height]| Rect {
                x,
                y,
                width,
                height,
            }),
            focus_fullscreen: h.focus.is_some() && h.focus_fullscreen,
        };
    }
    Known {
        monitors,
        pointer: if seen.xwayland { None } else { seen.x_pointer },
        focus: seen.x_focus.map(|(r, _)| r),
        focus_fullscreen: seen.x_focus.is_some_and(|(_, fs)| fs),
    }
}

/// The workspace the operator is on, from the helper or the root window's property.
pub fn workspace(seen: &Seen) -> Option<u32> {
    seen.helper
        .as_ref()
        .and_then(|h| h.workspace)
        .or(seen.x_workspace)
}

/// Ask the helper, if it is there, within 150 ms.
pub async fn helper_where(conn: &zbus::Connection) -> Option<HelperWhere> {
    let call = conn.call_method(
        Some(HELPER_NAME),
        HELPER_PATH,
        Some(HELPER_NAME),
        "Where",
        &(),
    );
    let reply = tokio::time::timeout(Duration::from_millis(150), call)
        .await
        .ok()?
        .ok()?;
    let json: String = reply.body().deserialize().ok()?;
    serde_json::from_str(&json).ok()
}

/// One short-lived Xlib connection, for a handful of reads.
pub struct Xlib {
    dpy: *mut xlib::Display,
    root: xlib::Window,
}

impl Drop for Xlib {
    fn drop(&mut self) {
        unsafe { xlib::XCloseDisplay(self.dpy) };
    }
}

impl Xlib {
    pub fn open() -> Option<Self> {
        let dpy = unsafe { xlib::XOpenDisplay(std::ptr::null()) };
        if dpy.is_null() {
            return None;
        }
        let root = unsafe { xlib::XDefaultRootWindow(dpy) };
        Some(Self { dpy, root })
    }

    fn atom(&self, name: &str) -> xlib::Atom {
        let c = CString::new(name).unwrap_or_default();
        unsafe { xlib::XInternAtom(self.dpy, c.as_ptr(), xlib::False) }
    }

    /// A 32-bit list property (windows, atoms, cardinals).
    fn longs(&self, w: xlib::Window, prop: &str) -> Vec<c_ulong> {
        let mut actual: xlib::Atom = 0;
        let mut format: c_int = 0;
        let mut n: c_ulong = 0;
        let mut after: c_ulong = 0;
        let mut data: *mut c_uchar = std::ptr::null_mut();
        let ok = unsafe {
            xlib::XGetWindowProperty(
                self.dpy,
                w,
                self.atom(prop),
                0,
                4096,
                xlib::False,
                xlib::AnyPropertyType as c_ulong,
                &mut actual,
                &mut format,
                &mut n,
                &mut after,
                &mut data,
            )
        };
        if ok != 0 || data.is_null() {
            return Vec::new();
        }
        let out = if format == 32 {
            unsafe { std::slice::from_raw_parts(data as *const c_ulong, n as usize) }.to_vec()
        } else {
            Vec::new()
        };
        unsafe { xlib::XFree(data.cast()) };
        out
    }

    pub fn workspace(&self) -> Option<u32> {
        self.longs(self.root, "_NET_CURRENT_DESKTOP")
            .first()
            .map(|&d| d as u32)
    }

    /// The focused window, when it is one of the window manager's clients.
    pub fn focus(&self) -> Option<(Rect, bool)> {
        let active = *self.longs(self.root, "_NET_ACTIVE_WINDOW").first()?;
        if active == 0 || !self.longs(self.root, "_NET_CLIENT_LIST").contains(&active) {
            return None;
        }
        let mut attrs: xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
        if unsafe { xlib::XGetWindowAttributes(self.dpy, active, &mut attrs) } == 0 {
            return None;
        }
        let (mut x, mut y, mut child) = (0, 0, 0);
        unsafe {
            xlib::XTranslateCoordinates(
                self.dpy, active, self.root, 0, 0, &mut x, &mut y, &mut child,
            )
        };
        let fs = self.atom("_NET_WM_STATE_FULLSCREEN");
        let fullscreen = self.longs(active, "_NET_WM_STATE").contains(&fs);
        Some((
            Rect {
                x,
                y,
                width: attrs.width,
                height: attrs.height,
            },
            fullscreen,
        ))
    }

    pub fn pointer(&self) -> Option<(i32, i32)> {
        let (mut root, mut child) = (0, 0);
        let (mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0);
        let mut mask: c_uint = 0;
        let ok = unsafe {
            xlib::XQueryPointer(
                self.dpy, self.root, &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy,
                &mut mask,
            )
        };
        (ok != 0).then_some((rx, ry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitors() -> Vec<Monitor> {
        vec![Monitor {
            name: "HDMI-1".into(),
            geometry: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary: true,
        }]
    }

    #[test]
    fn under_xwayland_the_pointer_query_is_not_believed() {
        let seen = Seen {
            xwayland: true,
            x_pointer: Some((942, 1079)),
            ..Seen::default()
        };
        assert_eq!(known(monitors(), &seen).pointer, None);
        let seen = Seen {
            xwayland: false,
            ..seen
        };
        assert_eq!(known(monitors(), &seen).pointer, Some((942, 1079)));
    }

    #[test]
    fn a_focused_x_client_is_believed_on_either() {
        let r = Rect {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        };
        let seen = Seen {
            xwayland: true,
            x_focus: Some((r, true)),
            ..Seen::default()
        };
        let k = known(monitors(), &seen);
        assert_eq!((k.focus, k.focus_fullscreen), (Some(r), true));
    }

    #[test]
    fn the_helper_wins_when_it_answers() {
        let seen = Seen {
            xwayland: true,
            x_pointer: Some((1, 1)),
            x_workspace: Some(0),
            helper: Some(
                serde_json::from_str(
                    r#"{"pointer":[2500,300],"focus":[1920,0,1920,1080],"focus_fullscreen":true,"workspace":2}"#,
                )
                .unwrap(),
            ),
            ..Seen::default()
        };
        let k = known(monitors(), &seen);
        assert_eq!(k.pointer, Some((2500, 300)));
        assert!(k.focus_fullscreen);
        assert_eq!(workspace(&seen), Some(2));
        // No focused window: nothing is full screen, whatever the flag says.
        let mut h = seen.helper.clone().unwrap();
        h.focus = None;
        let k = known(
            monitors(),
            &Seen {
                helper: Some(h),
                ..seen.clone()
            },
        );
        assert!(!k.focus_fullscreen);
    }
}
