//! The desktop's portals for the dictation key (ADR 0077 as amended 2026-11-25): GNOME's
//! GlobalShortcuts for the key's presses and releases, and RemoteDesktop for typing the
//! text. The daemon owns both sessions, on its own session-bus connection registered as
//! [`APP_ID`] (`org.freedesktop.host.portal.Registry`), so the key works with no window and
//! the text never leaves `ghostd` but for the window it is typed into.
//!
//! Measured on GNOME 50.1 (`.scratch/ptt-spike/`): the key is bound once after a dialog and
//! re-bound silently after; `Activated` repeats every 30 ms after 500 ms while the key is
//! held and one `Deactivated` comes at the release; a keyboard-only RemoteDesktop session
//! with `persist_mode` 2 gives a restore token, and `NotifyKeyboardKeysym` types about 1,600
//! characters a second. The typing session is opened for the moment of typing and closed
//! after, so GNOME's remote-control indicator is not left on.
//!
//! This module is transport only: no policy, no text kept, nothing logged but kinds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use super::key::KeyEvent;

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const GS: &str = "org.freedesktop.portal.GlobalShortcuts";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const REGISTRY: &str = "org.freedesktop.host.portal.Registry";

/// The app id the portals know Ghost by: the installed `.desktop` file's name.
pub const APP_ID: &str = "heka-ghost-desktop";
/// The one shortcut Ghost binds.
pub const SHORTCUT: &str = "dictate";
/// What GNOME's dialog and Settings › Keyboard call it.
pub const SHORTCUT_DESCRIPTION: &str = "Dictate (hold to talk, tap to lock)";
/// How long a request may wait for the operator at a dialog.
pub const DIALOG_WAIT: Duration = Duration::from_secs(180);
/// How long a request may take when no dialog is expected.
pub const QUIET_WAIT: Duration = Duration::from_secs(10);

/// Keyboard, for RemoteDesktop's `types`.
const KEYBOARD: u32 = 1;
/// Persist the permission until the operator revokes it.
const PERSIST_UNTIL_REVOKED: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalError {
    /// No session bus, or no portal with this interface (not GNOME or KDE, a desktop with
    /// no portal backend).
    Unavailable,
    /// The operator said no at the dialog, or closed it.
    Refused,
    /// Anything else, as a kind: no words from the desktop reach a log line.
    Failed(&'static str),
}

impl From<zbus::Error> for PortalError {
    fn from(e: zbus::Error) -> Self {
        match e {
            zbus::Error::MethodError(name, _, _)
                if name.as_str().ends_with("UnknownMethod")
                    || name.as_str().ends_with("UnknownInterface")
                    || name.as_str().ends_with("ServiceUnknown") =>
            {
                PortalError::Unavailable
            }
            zbus::Error::InputOutput(_) | zbus::Error::Address(_) => PortalError::Unavailable,
            _ => PortalError::Failed("dbus"),
        }
    }
}

impl From<zbus::zvariant::Error> for PortalError {
    fn from(_: zbus::zvariant::Error) -> Self {
        PortalError::Failed("reply")
    }
}

type Results = HashMap<String, OwnedValue>;

/// A session-bus connection registered as Ghost, for both portals.
pub struct Portal {
    conn: Connection,
    tokens: AtomicU64,
}

/// A bound GlobalShortcuts session.
pub struct Shortcuts {
    pub session: OwnedObjectPath,
    /// GNOME's words for the key ("Press F9"), or None when it gave none.
    pub trigger: Option<String>,
}

impl Portal {
    /// Connect and register Ghost's app id. A portal without the registry (older than 1.19)
    /// is used as it is: it names the app by the process.
    pub async fn connect() -> Result<Self, PortalError> {
        let conn = Connection::session()
            .await
            .map_err(|_| PortalError::Unavailable)?;
        let none: HashMap<&str, Value> = HashMap::new();
        if conn
            .call_method(
                Some(DEST),
                PATH,
                Some(REGISTRY),
                "Register",
                &(APP_ID, none),
            )
            .await
            .is_err()
        {
            tracing::info!("dictation: the portal has no app registry; it names Ghost itself");
        }
        Ok(Self {
            conn,
            tokens: AtomicU64::new(0),
        })
    }

    fn token(&self, what: &str) -> String {
        format!(
            "ghost_{what}_{}_{}",
            std::process::id(),
            self.tokens.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn request_path(&self, token: &str) -> String {
        let sender = self
            .conn
            .unique_name()
            .map(|n| n.as_str().trim_start_matches(':').replace('.', "_"))
            .unwrap_or_default();
        format!("{PATH}/request/{sender}/{token}")
    }

    /// A portal method that answers through a Request object: subscribe to its Response
    /// first, call, then wait for the answer.
    async fn request<B>(
        &self,
        iface: &str,
        method: &str,
        body: &B,
        token: &str,
        wait: Duration,
    ) -> Result<Results, PortalError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let path = self.request_path(token);
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DEST)
            .map_err(|_| PortalError::Failed("rule"))?
            .interface(REQUEST)
            .map_err(|_| PortalError::Failed("rule"))?
            .member("Response")
            .map_err(|_| PortalError::Failed("rule"))?
            .path(path.as_str())
            .map_err(|_| PortalError::Failed("rule"))?
            .build();
        let mut stream = MessageStream::for_match_rule(rule, &self.conn, Some(4)).await?;
        self.conn
            .call_method(Some(DEST), PATH, Some(iface), method, body)
            .await?;
        let msg = tokio::time::timeout(wait, stream.next())
            .await
            .map_err(|_| PortalError::Failed("no answer"))?
            .ok_or(PortalError::Failed("no answer"))??;
        let (code, results): (u32, Results) = msg.body().deserialize()?;
        match code {
            0 => Ok(results),
            1 => Err(PortalError::Refused),
            _ => Err(PortalError::Failed("ended")),
        }
    }

    async fn create_session(
        &self,
        iface: &str,
        what: &str,
    ) -> Result<OwnedObjectPath, PortalError> {
        let token = self.token(what);
        let session_token = self.token(&format!("{what}_session"));
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        opts.insert("session_handle_token", Value::from(session_token.as_str()));
        let r = self
            .request(iface, "CreateSession", &(opts,), &token, QUIET_WAIT)
            .await?;
        let handle = r
            .get("session_handle")
            .ok_or(PortalError::Failed("no session"))?;
        if let Ok(s) = String::try_from(
            handle
                .try_clone()
                .map_err(|_| PortalError::Failed("session"))?,
        ) {
            return OwnedObjectPath::try_from(s).map_err(|_| PortalError::Failed("session"));
        }
        OwnedObjectPath::try_from(
            handle
                .try_clone()
                .map_err(|_| PortalError::Failed("session"))?,
        )
        .map_err(|_| PortalError::Failed("session"))
    }

    async fn close(&self, session: &OwnedObjectPath) {
        let _ = self
            .conn
            .call_method(Some(DEST), session.as_str(), Some(SESSION), "Close", &())
            .await;
    }

    // --- the key --------------------------------------------------------------------------

    /// Open a GlobalShortcuts session and bind the dictation key, `preferred` ("F9") as the
    /// suggestion. The first time GNOME asks in a dialog; after that it binds quietly.
    pub async fn bind(&self, preferred: &str) -> Result<Shortcuts, PortalError> {
        let session = self.create_session(GS, "keys").await?;
        let token = self.token("bind");
        let mut props: HashMap<&str, Value> = HashMap::new();
        props.insert("description", Value::from(SHORTCUT_DESCRIPTION));
        props.insert("preferred_trigger", Value::from(preferred));
        let shortcuts = vec![(SHORTCUT, props)];
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        let r = self
            .request(
                GS,
                "BindShortcuts",
                &(&session, shortcuts, "", opts),
                &token,
                DIALOG_WAIT,
            )
            .await;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                self.close(&session).await;
                return Err(e);
            }
        };
        let trigger = r
            .get("shortcuts")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| <Vec<(String, HashMap<String, OwnedValue>)>>::try_from(v).ok())
            .and_then(|list| {
                list.into_iter()
                    .find(|(id, _)| id == SHORTCUT)
                    .and_then(|(_, p)| p.get("trigger_description").cloned())
            })
            .and_then(|v| String::try_from(v).ok())
            .filter(|s| !s.is_empty());
        Ok(Shortcuts { session, trigger })
    }

    /// GNOME's own place to change the key (`ConfigureShortcuts`, portal version 2).
    pub async fn configure(&self, s: &Shortcuts) -> Result<(), PortalError> {
        let opts: HashMap<&str, Value> = HashMap::new();
        self.conn
            .call_method(
                Some(DEST),
                PATH,
                Some(GS),
                "ConfigureShortcuts",
                &(&s.session, "", opts),
            )
            .await?;
        Ok(())
    }

    /// The key's presses and releases on `session`, timed on `origin`'s clock as they
    /// arrive (the portal's own timestamps are another clock).
    pub async fn key_events(
        &self,
        session: OwnedObjectPath,
        origin: Instant,
    ) -> Result<impl futures::Stream<Item = KeyEvent> + use<>, PortalError> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DEST)
            .map_err(|_| PortalError::Failed("rule"))?
            .interface(GS)
            .map_err(|_| PortalError::Failed("rule"))?
            .path(PATH)
            .map_err(|_| PortalError::Failed("rule"))?
            .build();
        let stream = MessageStream::for_match_rule(rule, &self.conn, Some(256)).await?;
        Ok(stream.filter_map(move |m| {
            let session = session.clone();
            async move {
                let m = m.ok()?;
                let member = m.header().member()?.to_string();
                let (s, id, _ts, _opts): (OwnedObjectPath, String, u64, Results) =
                    m.body().deserialize().ok()?;
                if s != session || id != SHORTCUT {
                    return None;
                }
                let at = origin.elapsed();
                match member.as_str() {
                    "Activated" => Some(KeyEvent::Down(at)),
                    "Deactivated" => Some(KeyEvent::Up(at)),
                    _ => None,
                }
            }
        }))
    }

    pub async fn unbind(&self, s: Shortcuts) {
        self.close(&s.session).await;
    }

    // --- typing ---------------------------------------------------------------------------

    /// Open a keyboard-only RemoteDesktop session: restored with `restore` quietly, or after
    /// GNOME's dialog. Returns the session and the next restore token (the portal rotates it).
    async fn typing_session(
        &self,
        restore: Option<&str>,
    ) -> Result<(OwnedObjectPath, Option<String>), PortalError> {
        let session = self.create_session(RD, "type").await?;
        let token = self.token("select");
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        opts.insert("types", Value::from(KEYBOARD));
        opts.insert("persist_mode", Value::from(PERSIST_UNTIL_REVOKED));
        if let Some(r) = restore {
            opts.insert("restore_token", Value::from(r));
        }
        if let Err(e) = self
            .request(RD, "SelectDevices", &(&session, opts), &token, QUIET_WAIT)
            .await
        {
            self.close(&session).await;
            return Err(e);
        }
        let token = self.token("start");
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("handle_token", Value::from(token.as_str()));
        let started = self
            .request(RD, "Start", &(&session, "", opts), &token, DIALOG_WAIT)
            .await;
        match started {
            Ok(r) => {
                let next = r
                    .get("restore_token")
                    .and_then(|v| v.try_clone().ok())
                    .and_then(|v| String::try_from(v).ok());
                Ok((session, next))
            }
            Err(e) => {
                self.close(&session).await;
                Err(e)
            }
        }
    }

    /// Ask for the typing permission now (the switch's second dialog), and give the restore
    /// token back. Nothing is typed.
    pub async fn grant_typing(&self, restore: Option<&str>) -> Result<Option<String>, PortalError> {
        let (session, next) = self.typing_session(restore).await?;
        self.close(&session).await;
        Ok(next)
    }

    /// Type `keysyms` into the window that has the keyboard, each pressed and released, in a
    /// session opened for this and closed after. Returns the next restore token.
    pub async fn type_keysyms(
        &self,
        keysyms: &[i32],
        restore: Option<&str>,
    ) -> Result<Option<String>, PortalError> {
        let (session, next) = self.typing_session(restore).await?;
        let mut result = Ok(());
        'keys: for &k in keysyms {
            for state in [1u32, 0u32] {
                let opts: HashMap<&str, Value> = HashMap::new();
                if let Err(e) = self
                    .conn
                    .call_method(
                        Some(DEST),
                        PATH,
                        Some(RD),
                        "NotifyKeyboardKeysym",
                        &(&session, opts, k, state),
                    )
                    .await
                {
                    // A key left down is released before giving up.
                    if state == 0 {
                        result = Err(PortalError::from(e));
                        break 'keys;
                    }
                    let opts: HashMap<&str, Value> = HashMap::new();
                    let _ = self
                        .conn
                        .call_method(
                            Some(DEST),
                            PATH,
                            Some(RD),
                            "NotifyKeyboardKeysym",
                            &(&session, opts, k, 0u32),
                        )
                        .await;
                    result = Err(PortalError::from(e));
                    break 'keys;
                }
            }
        }
        self.close(&session).await;
        result.map(|()| next)
    }
}
