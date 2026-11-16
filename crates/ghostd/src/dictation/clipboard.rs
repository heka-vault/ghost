//! Dictated text onto the clipboard (ADR 0077), from a daemon with no window and without
//! taking the keyboard from anyone.
//!
//! GNOME's mutter offers no clipboard protocol to an unfocused Wayland client (no
//! `ext_data_control`, no `zwlr_data_control`), so `wl-copy` there needs a surface that
//! takes focus. An X11 client owns a selection without focus, and mutter bridges XWayland's
//! CLIPBOARD to Wayland clients and copies it into its own clipboard manager at once
//! (checked on Ubuntu 26.04's GNOME 50.1, 2026-11-15). On X11 itself it is how every
//! program copies.
//!
//! [`XClipboard`] owns CLIPBOARD from an input-only window, never shown or focused, through
//! x11rb's pure-Rust connection (no Xlib, which ends the whole process on an X I/O error, and
//! no libxcb). x11rb reads the display's address and the Xauthority file itself. Under it
//! sits [`Bounded`]: every wait on the socket is bounded (the setup and each request's reply
//! by a deadline, every write by [`WRITE_WAIT`]) and a cancel interrupts it at once, by a
//! flag and by shutting the socket down from outside, so no owner is ever left blocked.
//!
//! Lifetime: one owner thread at a time. It answers requests for the text until something
//! else is copied (`SelectionClear`), the next dictation replaces it, or Ghost stops
//! ([`Clipboard::close`], and `Drop`); a replaced, failed or closed owner is joined, never
//! left behind. Its copy of the text is wiped when it ends. A text over [`MAX_BYTES`] is
//! refused, so one request always carries it and the incremental transfer is never needed.
//! Only CLIPBOARD, never PRIMARY; what was on the clipboard before is not restored.

use std::io::{self, IoSlice};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ghost_api::dictation::DictationReason;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb::utils::RawFdContainer;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};
use zeroize::{Zeroize, Zeroizing};

/// Puts dictated text on the clipboard; a test's fake records it instead.
pub trait Clipboard: Send + Sync {
    /// Own the clipboard with `text`. `Err` is the reason it could not.
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason>;
    /// Give the clipboard up and wait for its owner to end: Ghost is stopping.
    fn close(&self) {}
}

/// No clipboard: a daemon under test that was given none.
pub struct NoClipboard;

impl Clipboard for NoClipboard {
    fn set(&self, _text: Zeroizing<String>) -> Result<(), DictationReason> {
        Err(DictationReason::NoDisplay)
    }
}

/// The most Ghost puts on the clipboard, in bytes. Thirty seconds of speech is a few
/// hundred characters; one X request holds this many and more, so the incremental transfer
/// is never needed. A longer text is refused, never cut.
pub const MAX_BYTES: usize = 16 * 1024;
/// How long the owner has to connect to the display's socket.
pub const CONNECT_WAIT: Duration = Duration::from_secs(2);
/// How long x11rb's lookup of the cookie may take.
pub const AUTH_WAIT: Duration = Duration::from_secs(1);
/// How long the owner has to set up and say it owns the selection, once connected.
pub const OWN_WAIT: Duration = Duration::from_secs(3);
/// The longest wait for one reply while answering a paste.
pub const REPLY_WAIT: Duration = Duration::from_secs(2);
/// The longest a write may wait for the display to take it.
pub const WRITE_WAIT: Duration = Duration::from_secs(2);

/// Whose cookie to show the display: the one x11rb finds through `XAUTHORITY`, or none (a
/// test's own display, which must never be shown the operator's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    FromEnvironment,
    None,
}

#[derive(Debug, Clone)]
enum Place {
    /// `DISPLAY`, read at each set.
    Display,
    /// A socket and display number given (a test's).
    At {
        socket: PathBuf,
        display: u16,
        auth: Auth,
    },
}

/// One owner thread and what interrupts it: a flag it checks before every wait and before
/// it takes the clipboard, and its socket, which it publishes as soon as it has connected so
/// a cancel can shut it down under any wait.
struct Owner {
    id: u64,
    cancel: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<UnixStream>>>,
    thread: JoinHandle<()>,
}

impl Owner {
    /// Interrupt it and wait for it: its copy of the text is wiped and its socket closed.
    /// Every step of the owner is bounded or interruptible, so the join is too.
    fn end(self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.socket).as_ref() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let _ = self.thread.join();
    }
}

/// The clipboard's whole lifecycle, under one lock: nothing can be installed that `close`
/// does not see, and whatever `close` takes it joins.
enum State {
    Empty,
    /// Setting up: connecting, looking up the cookie, taking the selection.
    Pending(Owner),
    /// Holding CLIPBOARD, answering pastes.
    Established(Owner),
    /// Ghost is stopping: nothing more is set.
    Closed,
}

/// The X11 clipboard, through XWayland on a Wayland desktop.
pub struct XClipboard {
    place: Place,
    state: Mutex<State>,
    next: std::sync::atomic::AtomicU64,
}

impl Default for XClipboard {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl XClipboard {
    /// The display in `DISPLAY`, with the cookie x11rb finds for it.
    pub fn new() -> Self {
        Self::with_place(Place::Display)
    }

    /// A given socket and display number: for tests, with their own display.
    pub fn at(socket: impl Into<PathBuf>, display: u16, auth: Auth) -> Self {
        Self::with_place(Place::At {
            socket: socket.into(),
            display,
            auth,
        })
    }

    fn with_place(place: Place) -> Self {
        Self {
            place,
            state: Mutex::new(State::Empty),
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// An owner thread is running (holding the clipboard, or setting up to).
    pub fn holding(&self) -> bool {
        matches!(
            *lock(&self.state),
            State::Pending(_) | State::Established(_)
        )
    }

    /// The local socket, the display's number, and whose cookie to show.
    fn endpoint(&self) -> Result<(PathBuf, u16, Auth), DictationReason> {
        match &self.place {
            Place::At {
                socket,
                display,
                auth,
            } => Ok((socket.clone(), *display, *auth)),
            Place::Display => {
                let parsed = x11rb_protocol::parse_display::parse_display(None)
                    .map_err(|_| DictationReason::NoDisplay)?;
                let socket = parsed
                    .connect_instruction()
                    .find_map(|a| match a {
                        x11rb_protocol::parse_display::ConnectAddress::Socket(p) => {
                            Some(PathBuf::from(p))
                        }
                        _ => None,
                    })
                    .ok_or(DictationReason::NoDisplay)?;
                Ok((socket, parsed.display, Auth::FromEnvironment))
            }
        }
    }
}

impl Clipboard for XClipboard {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        if text.len() > MAX_BYTES {
            return Err(DictationReason::TooLong);
        }
        let (socket_path, display, auth) = self.endpoint()?;
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        let socket: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        // Installed as pending under the state's lock, the previous owner taken out with it:
        // `close` either sees this one or this one sees `Closed`.
        let previous = {
            let mut state = lock(&self.state);
            if matches!(*state, State::Closed) {
                return Err(DictationReason::ClipboardFailed);
            }
            let thread = {
                let (cancel, socket) = (cancel.clone(), socket.clone());
                std::thread::Builder::new()
                    .name("ghost-clipboard".into())
                    .spawn(move || own(socket_path, display, auth, text, &cancel, &socket, &tx))
            };
            let Ok(thread) = thread else {
                return Err(DictationReason::ClipboardFailed);
            };
            std::mem::replace(
                &mut *state,
                State::Pending(Owner {
                    id,
                    cancel,
                    socket,
                    thread,
                }),
            )
        };
        // The previous owner, joined: one owner at a time.
        if let State::Pending(o) | State::Established(o) = previous {
            o.end();
        }
        // The owner bounds each of its steps; this only bounds a thread that died.
        let owned = rx
            .recv_timeout(CONNECT_WAIT + AUTH_WAIT + OWN_WAIT + Duration::from_secs(1))
            .unwrap_or(Err(DictationReason::ClipboardFailed));
        let failed = {
            let mut state = lock(&self.state);
            match std::mem::replace(&mut *state, State::Empty) {
                State::Pending(o) if o.id == id => {
                    if owned.is_ok() && !o.cancel.load(Ordering::SeqCst) {
                        *state = State::Established(o);
                        return Ok(());
                    }
                    Some(o)
                }
                // Closed or replaced meanwhile: whoever did it joins this owner.
                other => {
                    *state = other;
                    None
                }
            }
        };
        if let Some(o) = failed {
            o.end(); // joined: nothing is left holding the text
        }
        Err(owned.err().unwrap_or(DictationReason::ClipboardFailed))
    }

    fn close(&self) {
        let previous = std::mem::replace(&mut *lock(&self.state), State::Closed);
        if let State::Pending(o) | State::Established(o) = previous {
            o.end();
        }
    }
}

impl Drop for XClipboard {
    fn drop(&mut self) {
        self.close();
    }
}

// --- the bounded, interruptible stream under x11rb -------------------------------------------

/// The display's socket with every wait bounded: reads by the deadline set for the request
/// being made (none while the owner waits for the next paste), writes by [`WRITE_WAIT`],
/// and all of them ended by `cancel` (which also shuts the socket down, waking the poll).
struct Bounded {
    inner: DefaultStream,
    cancel: Arc<AtomicBool>,
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Stream for Bounded {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let reads_until = *lock(&self.deadline);
        let until = if mode.writable() {
            let w = Instant::now() + WRITE_WAIT;
            Some(reads_until.map_or(w, |r| r.min(w)))
        } else {
            reads_until
        };
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "the clipboard owner was told to stop",
                ));
            }
            let timeout = match until {
                Some(u) => {
                    let left = u.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "the display did not answer in time",
                        ));
                    }
                    Some(Timespec {
                        tv_sec: left.as_secs() as i64,
                        tv_nsec: i64::from(left.subsec_nanos()),
                    })
                }
                None => None,
            };
            let mut fds = [PollFd::new(&self.inner, flags)];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(0) => continue,
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn read(&self, buf: &mut [u8], fd_storage: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(buf, fd_storage)
    }

    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(buf, fds)
    }

    fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(bufs, fds)
    }
}

// --- the owner ------------------------------------------------------------------------------

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8: Atom,
    text: Atom,
    plain_utf8: Atom,
    plain: Atom,
}

type Conn = RustConnection<Bounded>;

/// Connect, own CLIPBOARD, say so on `owned`, then answer requests until another owner
/// takes it or `cancel` is set. The text is wiped when this returns.
fn own(
    socket_path: PathBuf,
    display: u16,
    auth: Auth,
    mut text: Zeroizing<String>,
    cancel: &Arc<AtomicBool>,
    socket: &Arc<Mutex<Option<UnixStream>>>,
    owned: &mpsc::Sender<Result<(), DictationReason>>,
) {
    let stream = match connect_within(&socket_path, cancel, Instant::now() + CONNECT_WAIT) {
        Ok(s) => s,
        Err(why) => {
            text.zeroize();
            let _ = owned.send(Err(why));
            return;
        }
    };
    // Published for a cancel to shut down, then the flag looked at: a cancel that came
    // before this either sees the socket or is seen here.
    match stream.try_clone() {
        Ok(c) => *lock(socket) = Some(c),
        Err(_) => {
            text.zeroize();
            let _ = owned.send(Err(DictationReason::ClipboardFailed));
            return;
        }
    }
    if cancel.load(Ordering::SeqCst) {
        text.zeroize();
        let _ = owned.send(Err(DictationReason::ClipboardFailed));
        return;
    }
    let deadline = Arc::new(Mutex::new(Some(Instant::now() + OWN_WAIT)));
    let (conn, window, atoms) = match set_up(stream, display, auth, &text, cancel, &deadline) {
        Ok(r) => r,
        Err(why) => {
            text.zeroize();
            let _ = owned.send(Err(why));
            return;
        }
    };
    *lock(&deadline) = None; // waiting for the next paste is not bounded; a cancel ends it
    let _ = owned.send(Ok(()));
    while !cancel.load(Ordering::SeqCst) {
        match conn.wait_for_event() {
            Ok(Event::SelectionClear(e)) if e.owner == window => break,
            Ok(Event::SelectionRequest(e)) => {
                *lock(&deadline) = Some(Instant::now() + REPLY_WAIT);
                let answered = answer(&conn, &e, &atoms, &text);
                *lock(&deadline) = None;
                if answered.is_err() {
                    break; // the display is gone or stalled
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    text.zeroize();
    *lock(&deadline) = Some(Instant::now() + WRITE_WAIT);
    let _ = conn.destroy_window(window);
    let _ = conn.flush();
}

/// Connect to the display's socket without waiting longer than `until` (a display whose
/// queue is full would block a plain connect for ever), and stop at a cancel.
fn connect_within(
    path: &std::path::Path,
    cancel: &AtomicBool,
    until: Instant,
) -> Result<UnixStream, DictationReason> {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let addr = SocketAddrUnix::new(path).map_err(|_| DictationReason::NoDisplay)?;
    let fd = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|_| DictationReason::ClipboardFailed)?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DictationReason::ClipboardFailed);
        }
        match rustix::net::connect(&fd, &addr) {
            Ok(()) => return Ok(UnixStream::from(fd)),
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {}
            Err(_) => return Err(DictationReason::NoDisplay),
        }
        if Instant::now() >= until {
            return Err(DictationReason::NoDisplay);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// x11rb's lookup of the cookie, on a thread of its own: a home folder that stalls (a network
/// mount) costs at most `wait`, after which the display is tried with no cookie and the
/// lookup is left to end by itself, holding no text.
fn cookie_within(
    peer: (x11rb_protocol::xauth::Family, Vec<u8>),
    display: u16,
    wait: Duration,
) -> (Vec<u8>, Vec<u8>) {
    let (tx, rx) = mpsc::channel();
    let looked = std::thread::Builder::new()
        .name("ghost-clipboard-auth".into())
        .spawn(move || {
            let _ = tx.send(
                x11rb_protocol::xauth::get_auth(peer.0, &peer.1, display)
                    .ok()
                    .flatten(),
            );
        });
    if looked.is_err() {
        return (Vec::new(), Vec::new());
    }
    rx.recv_timeout(wait).ok().flatten().unwrap_or_default()
}

fn set_up(
    stream: UnixStream,
    display: u16,
    auth: Auth,
    text: &str,
    cancel: &Arc<AtomicBool>,
    deadline: &Arc<Mutex<Option<Instant>>>,
) -> Result<(Conn, Window, Atoms), DictationReason> {
    let failed = |_| DictationReason::ClipboardFailed;
    let (inner, peer) = DefaultStream::from_unix_stream(stream).map_err(failed)?;
    let (name, data) = match auth {
        // x11rb reads the Xauthority file; Ghost never does.
        Auth::FromEnvironment => cookie_within(peer, display, AUTH_WAIT),
        Auth::None => (Vec::new(), Vec::new()),
    };
    let bounded = Bounded {
        inner,
        cancel: cancel.clone(),
        deadline: deadline.clone(),
    };
    let conn = RustConnection::connect_to_stream_with_auth_info(bounded, 0, name, data)
        .map_err(|_| DictationReason::NoDisplay)?;
    if text.len() + 64 > conn.maximum_request_bytes() {
        return Err(DictationReason::TooLong);
    }
    let root = conn
        .setup()
        .roots
        .first()
        .map(|s| s.root)
        .ok_or(DictationReason::ClipboardFailed)?;
    let window = conn
        .generate_id()
        .map_err(|_| DictationReason::ClipboardFailed)?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY, // never shown, never focused
        0,
        &CreateWindowAux::new(),
    )
    .map_err(|_| DictationReason::ClipboardFailed)?
    .check()
    .map_err(|_| DictationReason::ClipboardFailed)?;
    let intern = |name: &[u8]| -> Result<Atom, DictationReason> {
        Ok(conn
            .intern_atom(false, name)
            .map_err(|_| DictationReason::ClipboardFailed)?
            .reply()
            .map_err(|_| DictationReason::ClipboardFailed)?
            .atom)
    };
    let atoms = Atoms {
        clipboard: intern(b"CLIPBOARD")?,
        targets: intern(b"TARGETS")?,
        utf8: intern(b"UTF8_STRING")?,
        text: intern(b"TEXT")?,
        plain_utf8: intern(b"text/plain;charset=utf-8")?,
        plain: intern(b"text/plain")?,
    };
    // The last look before taking the clipboard: a cancel since means nothing is taken.
    if cancel.load(Ordering::SeqCst) {
        return Err(DictationReason::ClipboardFailed);
    }
    conn.set_selection_owner(window, atoms.clipboard, CURRENT_TIME)
        .map_err(|_| DictationReason::ClipboardFailed)?;
    let owner = conn
        .get_selection_owner(atoms.clipboard)
        .map_err(|_| DictationReason::ClipboardFailed)?
        .reply()
        .map_err(|_| DictationReason::ClipboardFailed)?
        .owner;
    if owner != window {
        return Err(DictationReason::ClipboardFailed);
    }
    Ok((conn, window, atoms))
}

/// Answer one `SelectionRequest`: the text in the form asked for, success only when the
/// display took the property, otherwise a refusal. `Err` when the display is gone.
fn answer(conn: &Conn, e: &SelectionRequestEvent, a: &Atoms, text: &str) -> Result<(), ()> {
    let property = if e.property == NONE {
        e.target // an old client's request
    } else {
        e.property
    };
    let checked =
        |r: Result<x11rb::cookie::VoidCookie<'_, Conn>, x11rb::errors::ConnectionError>| {
            r.map_err(|_| ())
                .and_then(|c| c.check().map_err(|_| ()))
                .is_ok()
        };
    let string: Atom = AtomEnum::STRING.into();
    let given = if e.selection != a.clipboard {
        false
    } else if e.target == a.targets {
        let list = [a.targets, a.utf8, a.plain_utf8, a.plain, a.text, string];
        checked(conn.change_property32(
            PropMode::REPLACE,
            e.requestor,
            property,
            AtomEnum::ATOM,
            &list,
        ))
    } else if [a.utf8, a.text, a.plain_utf8, a.plain].contains(&e.target) {
        let kind = if e.target == a.text { a.utf8 } else { e.target };
        checked(conn.change_property8(
            PropMode::REPLACE,
            e.requestor,
            property,
            kind,
            text.as_bytes(),
        ))
    } else if e.target == string {
        let mut latin = Zeroizing::new(Vec::with_capacity(text.len()));
        latin.extend(
            text.chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')),
        );
        checked(conn.change_property8(PropMode::REPLACE, e.requestor, property, string, &latin))
    } else {
        false
    };
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: e.time,
        requestor: e.requestor,
        selection: e.selection,
        target: e.target,
        property: if given { property } else { NONE },
    };
    // A requestor that went away is an error of its own, not the display's.
    let _ = conn.send_event(false, e.requestor, EventMask::NO_EVENT, notify);
    conn.flush().map_err(|_| ())
}

#[cfg(test)]
mod tests;
