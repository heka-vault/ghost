//! Is the screen being shared or recorded? The floating room hides while it is (ADR 0059):
//! a reply or a question in large type must not reach a meeting or a recording.
//!
//! On GNOME every capture of the screen goes through mutter's screen-cast service: a share
//! through the desktop portal (a browser's meeting, OBS), GNOME's own recorder, and remote
//! desktop. Mutter makes one D-Bus object per session under
//! `/org/gnome/Mutter/ScreenCast/Session` (and `/org/gnome/Mutter/RemoteDesktop/Session`), and
//! those paths can be introspected by any client on the session bus: a child node is a
//! session. No portal prompt, nothing recorded, one small call every two seconds while the
//! floating room is on.
//!
//! What it cannot see: a client that grabs XWayland's own pixels (`ffmpeg -f x11grab`), and
//! any other desktop's capture. There the operator turns the floating room off from the tray.

use serde::{Deserialize, Serialize};

pub const SCREEN_CAST: (&str, &str) = (
    "org.gnome.Mutter.ScreenCast",
    "/org/gnome/Mutter/ScreenCast/Session",
);
pub const REMOTE_DESKTOP: (&str, &str) = (
    "org.gnome.Mutter.RemoteDesktop",
    "/org/gnome/Mutter/RemoteDesktop/Session",
);

/// How often to look while the floating room is on.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capture {
    /// Something is capturing the screen now.
    Yes,
    No,
    /// This desktop doesn't say (not GNOME, or no session bus).
    Unknown,
}

/// The child nodes of an introspection document: one per live session.
pub fn sessions(xml: &str) -> usize {
    // `<node>` is the object itself; its children are `<node name="u3"/>`. Interfaces and
    // their members never use the element name `node`.
    xml.match_indices("<node")
        .filter(|(i, _)| {
            let rest = &xml[i + 5..];
            rest.trim_start().starts_with("name=")
        })
        .count()
}

/// Both services' answers together: any session is a capture; nothing answering is unknown.
pub fn combine(answers: &[Option<usize>]) -> Capture {
    if answers.iter().any(|a| matches!(a, Some(n) if *n > 0)) {
        Capture::Yes
    } else if answers.iter().any(Option::is_some) {
        Capture::No
    } else {
        Capture::Unknown
    }
}

async fn introspect(conn: &zbus::Connection, (dest, path): (&str, &str)) -> Option<String> {
    let reply = conn
        .call_method(
            Some(dest),
            path,
            Some("org.freedesktop.DBus.Introspectable"),
            "Introspect",
            &(),
        )
        .await
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// Look once.
pub async fn probe(conn: &zbus::Connection) -> Capture {
    let mut answers = Vec::with_capacity(2);
    for svc in [SCREEN_CAST, REMOTE_DESKTOP] {
        answers.push(introspect(conn, svc).await.map(|x| sessions(&x)));
    }
    combine(&answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    // What mutter answers on GNOME Shell 50 with no session (read on the workstation,
    // 2026-11-12) and with two (the same document with the children mutter adds).
    const NONE: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
                      "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<!-- GDBus 2.88.0 -->
<node>
</node>
"#;
    const TWO: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
                      "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<!-- GDBus 2.88.0 -->
<node>
  <node name="u1"/>
  <node name="u2"/>
</node>
"#;
    const SESSION_ITSELF: &str = r#"<node>
  <interface name="org.gnome.Mutter.ScreenCast.Session">
    <method name="Start"/>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg type="s" name="xml_data" direction="out"/></method>
  </interface>
</node>"#;

    #[test]
    fn a_child_node_is_a_session() {
        assert_eq!(sessions(NONE), 0);
        assert_eq!(sessions(TWO), 2);
        assert_eq!(sessions(SESSION_ITSELF), 0);
    }

    #[test]
    fn any_session_is_a_capture_and_silence_is_unknown() {
        assert_eq!(combine(&[Some(0), Some(0)]), Capture::No);
        assert_eq!(combine(&[Some(0), Some(1)]), Capture::Yes);
        assert_eq!(combine(&[None, Some(2)]), Capture::Yes);
        assert_eq!(combine(&[Some(0), None]), Capture::No);
        assert_eq!(combine(&[None, None]), Capture::Unknown);
    }
}
