//! Dictated text onto the clipboard (ADR 0077), from a daemon with no window and without
//! taking the keyboard from anyone.
//!
//! GNOME's mutter offers no clipboard protocol to an unfocused Wayland client (no
//! `ext_data_control`, no `zwlr_data_control`), so `wl-copy` there needs a surface that
//! takes focus. An X11 client owns a selection without focus, and mutter bridges XWayland's
//! CLIPBOARD to Wayland clients and copies it into its own clipboard manager at once
//! (checked on Ubuntu 26.04's GNOME 50.1, 2026-11-15: an unfocused X owner's text was read
//! by `wl-paste`, mutter read it twice before anyone pasted, and it was still there after
//! the owner went away). On X11 itself it is how every program copies.
//!
//! [`XClipboard`] speaks the few requests a selection owner needs over the display's Unix
//! socket itself: connect, an input-only window, `SetSelectionOwner`, and an answer to each
//! `SelectionRequest`. Xlib is not used because it ends the whole process on an X I/O error
//! (the server going away), which a daemon cannot take; no crate is added.
//!
//! The owner keeps the text in memory until something else is copied (`SelectionClear`), a
//! later dictation replaces it, or Ghost stops; then it is wiped. Ghost never restores what
//! was on the clipboard before in this slice, and never touches PRIMARY.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ghost_api::dictation::DictationReason;
use zeroize::{Zeroize, Zeroizing};

/// Puts dictated text on the clipboard; a test's fake records it instead.
pub trait Clipboard: Send + Sync {
    /// Own the clipboard with `text`. `Err` is the reason it could not.
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason>;
}

/// No clipboard: a daemon under test that was given none.
pub struct NoClipboard;

impl Clipboard for NoClipboard {
    fn set(&self, _text: Zeroizing<String>) -> Result<(), DictationReason> {
        Err(DictationReason::NoDisplay)
    }
}

/// The X11 clipboard, through XWayland on a Wayland desktop.
#[derive(Default)]
pub struct XClipboard {
    owner: Mutex<Option<Arc<AtomicBool>>>,
}

/// How long the owner waits for the server to say it owns the selection.
const OWN_WAIT: Duration = Duration::from_secs(3);
/// How often a waiting owner looks whether it was told to stop.
const POLL: Duration = Duration::from_millis(250);
/// The longest text Ghost puts on the clipboard: far more than 30 s of speech, and inside
/// one request without the protocol's incremental transfer.
pub const MAX_BYTES: usize = 64 * 1024;

impl Clipboard for XClipboard {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        if text.len() > MAX_BYTES {
            return Err(DictationReason::ClipboardFailed);
        }
        let display = std::env::var("DISPLAY").map_err(|_| DictationReason::NoDisplay)?;
        let target = Target::parse(&display).ok_or(DictationReason::NoDisplay)?;
        let auth = std::env::var_os("XAUTHORITY")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".Xauthority")))
            .and_then(|p| read_cookie(&p, &target.number));
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.owner.lock().unwrap_or_else(|e| e.into_inner()).take() {
            old.store(true, Ordering::SeqCst); // the new owner takes it from the old anyway
        }
        let (tx, rx) = mpsc::channel();
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("ghost-clipboard".into())
            .spawn(move || {
                let conn = match UnixStream::connect(&target.socket) {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = tx.send(Err(DictationReason::NoDisplay));
                        return;
                    }
                };
                own(conn, auth, text, &stop2, tx);
            })
            .map_err(|_| DictationReason::ClipboardFailed)?;
        let owned = rx
            .recv_timeout(OWN_WAIT)
            .unwrap_or(Err(DictationReason::ClipboardFailed));
        if owned.is_ok() {
            *self.owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        } else {
            stop.store(true, Ordering::SeqCst);
        }
        owned
    }
}

/// Connect, own CLIPBOARD, say so on `owned`, then answer requests until another owner takes
/// it or `stop` is set. The text is wiped when this returns.
fn own<S: Read + Write + Pollable>(
    stream: S,
    auth: Option<Vec<u8>>,
    mut text: Zeroizing<String>,
    stop: &AtomicBool,
    owned: mpsc::Sender<Result<(), DictationReason>>,
) {
    let mut conn = match Conn::open(stream, auth.as_deref()) {
        Ok(c) => c,
        Err(_) => {
            let _ = owned.send(Err(DictationReason::NoDisplay));
            return;
        }
    };
    let ready = (|| -> io::Result<(Atoms, u32)> {
        let atoms = Atoms::intern(&mut conn)?;
        let window = conn.create_window()?;
        conn.set_selection_owner(window, atoms.clipboard)?;
        if conn.selection_owner(atoms.clipboard)? != window {
            return Err(io::Error::other("the server did not give us the selection"));
        }
        Ok((atoms, window))
    })();
    let (atoms, window) = match ready {
        Ok(r) => r,
        Err(_) => {
            let _ = owned.send(Err(DictationReason::ClipboardFailed));
            return;
        }
    };
    let _ = owned.send(Ok(()));
    while !stop.load(Ordering::SeqCst) {
        let event = match conn.next_event(POLL) {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(_) => break, // the server went away
        };
        match event[0] & 0x7f {
            SELECTION_CLEAR if u32_at(&event, 8) == window => break,
            // Answered in the guard; an answer that cannot be sent means the server is gone.
            SELECTION_REQUEST if conn.answer(&event, &atoms, &text).is_err() => break,
            _ => {}
        }
    }
    text.zeroize();
    let _ = conn.destroy_window(window);
}

// --- where the display is --------------------------------------------------------------------

/// `:0`, `:0.0`, `unix:0` → the socket and the display's number. A display over TCP is not
/// used.
#[derive(Debug, PartialEq)]
struct Target {
    socket: PathBuf,
    number: String,
}

impl Target {
    fn parse(display: &str) -> Option<Self> {
        let rest = display
            .strip_prefix("unix:")
            .or_else(|| display.strip_prefix(':'))?;
        let number = rest.split('.').next()?;
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            socket: PathBuf::from(format!("/tmp/.X11-unix/X{number}")),
            number: number.to_string(),
        })
    }
}

const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";

/// The MIT cookie for display `number` from an Xauthority file: an entry for that number,
/// else one for any (mutter's XWayland file names none).
fn read_cookie(path: &Path, number: &str) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    parse_cookie(&data, number)
}

fn parse_cookie(data: &[u8], number: &str) -> Option<Vec<u8>> {
    fn field<'a>(data: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
        let len = usize::from(u16::from_be_bytes([*data.get(*at)?, *data.get(*at + 1)?]));
        let out = data.get(*at + 2..*at + 2 + len)?;
        *at += 2 + len;
        Some(out)
    }
    let mut at = 0;
    let mut any = None;
    while at + 2 <= data.len() {
        at += 2; // the family
        let _address = field(data, &mut at)?;
        let num = field(data, &mut at)?;
        let name = field(data, &mut at)?;
        let cookie = field(data, &mut at)?;
        if name != COOKIE_NAME {
            continue;
        }
        if num == number.as_bytes() {
            return Some(cookie.to_vec());
        }
        if num.is_empty() && any.is_none() {
            any = Some(cookie.to_vec());
        }
    }
    any
}

// --- the protocol --------------------------------------------------------------------------

const REPLY: u8 = 1;
const SELECTION_CLEAR: u8 = 29;
const SELECTION_REQUEST: u8 = 30;
const SELECTION_NOTIFY: u8 = 31;
const ATOM: u32 = 4;
const STRING: u32 = 31;

/// A stream that can wait for something to read.
pub trait Pollable {
    /// True when a byte is waiting, within `wait`.
    fn readable(&mut self, wait: Duration) -> io::Result<bool>;
}

impl Pollable for UnixStream {
    fn readable(&mut self, wait: Duration) -> io::Result<bool> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let timeout = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&*self, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => Ok(false),
            Ok(_) => {
                // A whole message arrives at once on a local socket; this bounds a stalled
                // server mid-message.
                self.set_read_timeout(Some(Duration::from_secs(2)))?;
                Ok(true)
            }
            Err(rustix::io::Errno::INTR) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn pad4(n: usize) -> usize {
    (4 - n % 4) % 4
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

struct Conn<S> {
    s: S,
    seq: u16,
    id_base: u32,
    id_mask: u32,
    next_id: u32,
    root: u32,
    events: VecDeque<[u8; 32]>,
}

struct Atoms {
    clipboard: u32,
    targets: u32,
    utf8: u32,
    text: u32,
    plain_utf8: u32,
    plain: u32,
}

impl Atoms {
    fn intern<S: Read + Write + Pollable>(c: &mut Conn<S>) -> io::Result<Self> {
        Ok(Self {
            clipboard: c.intern("CLIPBOARD")?,
            targets: c.intern("TARGETS")?,
            utf8: c.intern("UTF8_STRING")?,
            text: c.intern("TEXT")?,
            plain_utf8: c.intern("text/plain;charset=utf-8")?,
            plain: c.intern("text/plain")?,
        })
    }
}

impl<S: Read + Write + Pollable> Conn<S> {
    /// The connection setup, little-endian, with the cookie if there is one.
    fn open(mut s: S, cookie: Option<&[u8]>) -> io::Result<Self> {
        let (name, data): (&[u8], &[u8]) = match cookie {
            Some(c) => (COOKIE_NAME, c),
            None => (b"", b""),
        };
        let mut req = vec![b'l', 0];
        req.extend(11u16.to_le_bytes());
        req.extend(0u16.to_le_bytes());
        req.extend((name.len() as u16).to_le_bytes());
        req.extend((data.len() as u16).to_le_bytes());
        req.extend([0, 0]);
        req.extend(name);
        req.extend(vec![0; pad4(name.len())]);
        req.extend(data);
        req.extend(vec![0; pad4(data.len())]);
        s.write_all(&req)?;
        let mut head = [0u8; 8];
        s.read_exact(&mut head)?;
        let mut more = vec![0u8; usize::from(u16_at(&head, 6)) * 4];
        s.read_exact(&mut more)?;
        if head[0] != 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the display refused the connection",
            ));
        }
        let vendor = usize::from(u16_at(&more, 16));
        let formats = usize::from(more[21]);
        let screen = 32 + vendor + pad4(vendor) + 8 * formats;
        if more.len() < screen + 4 {
            return Err(io::Error::other("a short setup reply"));
        }
        Ok(Self {
            s,
            seq: 0,
            id_base: u32_at(&more, 4),
            id_mask: u32_at(&more, 8),
            next_id: 1,
            root: u32_at(&more, screen),
            events: VecDeque::new(),
        })
    }

    fn send(&mut self, req: &[u8]) -> io::Result<u16> {
        self.s.write_all(req)?;
        self.seq = self.seq.wrapping_add(1);
        Ok(self.seq)
    }

    /// One message from the server: an event or error (32 bytes), or a reply with its tail.
    fn read_message(&mut self) -> io::Result<Vec<u8>> {
        let mut m = vec![0u8; 32];
        self.s.read_exact(&mut m)?;
        if m[0] == REPLY {
            let extra = u32_at(&m, 4) as usize * 4;
            if extra > 1 << 20 {
                return Err(io::Error::other("an oversized reply"));
            }
            let mut tail = vec![0u8; extra];
            self.s.read_exact(&mut tail)?;
            m.extend(tail);
        }
        Ok(m)
    }

    /// The reply to request `seq`; events that come first wait their turn, an error for it
    /// fails it.
    fn reply(&mut self, seq: u16) -> io::Result<Vec<u8>> {
        loop {
            let m = self.read_message()?;
            match m[0] {
                REPLY if u16_at(&m, 2) == seq => return Ok(m),
                0 if u16_at(&m, 2) == seq => {
                    return Err(io::Error::other(format!("X error {}", m[1])));
                }
                0 | REPLY => {}
                _ => {
                    let mut e = [0u8; 32];
                    e.copy_from_slice(&m[..32]);
                    self.events.push_back(e);
                }
            }
        }
    }

    fn next_event(&mut self, wait: Duration) -> io::Result<Option<[u8; 32]>> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        if !self.s.readable(wait)? {
            return Ok(None);
        }
        let m = self.read_message()?;
        if m[0] == 0 || m[0] == REPLY {
            return Ok(None); // an error for a request we did not wait on: not ours to act on
        }
        let mut e = [0u8; 32];
        e.copy_from_slice(&m[..32]);
        Ok(Some(e))
    }

    fn intern(&mut self, name: &str) -> io::Result<u32> {
        let n = name.len();
        let mut req = vec![16, 0];
        req.extend(((8 + n + pad4(n)) as u16 / 4).to_le_bytes());
        req.extend((n as u16).to_le_bytes());
        req.extend([0, 0]);
        req.extend(name.as_bytes());
        req.extend(vec![0; pad4(n)]);
        let seq = self.send(&req)?;
        Ok(u32_at(&self.reply(seq)?, 8))
    }

    fn create_window(&mut self) -> io::Result<u32> {
        let wid = self.id_base | (self.next_id & self.id_mask);
        self.next_id += 1;
        let mut req = vec![1, 0];
        req.extend(8u16.to_le_bytes());
        req.extend(wid.to_le_bytes());
        req.extend(self.root.to_le_bytes());
        req.extend(0i16.to_le_bytes()); // x
        req.extend(0i16.to_le_bytes()); // y
        req.extend(1u16.to_le_bytes()); // width
        req.extend(1u16.to_le_bytes()); // height
        req.extend(0u16.to_le_bytes()); // border
        req.extend(2u16.to_le_bytes()); // InputOnly: never shown, never focused
        req.extend(0u32.to_le_bytes()); // the parent's visual
        req.extend(0u32.to_le_bytes()); // no values
        self.send(&req)?;
        Ok(wid)
    }

    fn destroy_window(&mut self, window: u32) -> io::Result<()> {
        let mut req = vec![4, 0];
        req.extend(2u16.to_le_bytes());
        req.extend(window.to_le_bytes());
        self.send(&req).map(|_| ())
    }

    fn set_selection_owner(&mut self, owner: u32, selection: u32) -> io::Result<()> {
        let mut req = vec![22, 0];
        req.extend(4u16.to_le_bytes());
        req.extend(owner.to_le_bytes());
        req.extend(selection.to_le_bytes());
        req.extend(0u32.to_le_bytes()); // CurrentTime
        self.send(&req).map(|_| ())
    }

    fn selection_owner(&mut self, selection: u32) -> io::Result<u32> {
        let mut req = vec![23, 0];
        req.extend(2u16.to_le_bytes());
        req.extend(selection.to_le_bytes());
        let seq = self.send(&req)?;
        Ok(u32_at(&self.reply(seq)?, 8))
    }

    fn change_property(
        &mut self,
        window: u32,
        property: u32,
        kind: u32,
        format: u8,
        data: &[u8],
    ) -> io::Result<()> {
        let units = data.len() / usize::from(format / 8);
        let mut req = vec![18, 0];
        req.extend(((24 + data.len() + pad4(data.len())) as u16 / 4).to_le_bytes());
        req.extend(window.to_le_bytes());
        req.extend(property.to_le_bytes());
        req.extend(kind.to_le_bytes());
        req.extend([format, 0, 0, 0]);
        req.extend((units as u32).to_le_bytes());
        req.extend(data);
        req.extend(vec![0; pad4(data.len())]);
        self.send(&req).map(|_| ())
    }

    /// Answer one `SelectionRequest`: the text in the form asked for, or a refusal.
    fn answer(&mut self, ev: &[u8; 32], a: &Atoms, text: &str) -> io::Result<()> {
        let time = u32_at(ev, 4);
        let requestor = u32_at(ev, 12);
        let selection = u32_at(ev, 16);
        let target = u32_at(ev, 20);
        let mut property = u32_at(ev, 24);
        if property == 0 {
            property = target; // an old client's request
        }
        let given = if selection != a.clipboard {
            false
        } else if target == a.targets {
            let list: Vec<u8> = [a.targets, a.utf8, a.plain_utf8, a.plain, a.text, STRING]
                .iter()
                .flat_map(|t| t.to_le_bytes())
                .collect();
            self.change_property(requestor, property, ATOM, 32, &list)?;
            true
        } else if target == a.utf8
            || target == a.text
            || target == a.plain_utf8
            || target == a.plain
        {
            let kind = if target == a.text { a.utf8 } else { target };
            self.change_property(requestor, property, kind, 8, text.as_bytes())?;
            true
        } else if target == STRING {
            let mut latin: Zeroizing<Vec<u8>> = Zeroizing::new(
                text.chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                    .collect(),
            );
            self.change_property(requestor, property, STRING, 8, &latin)?;
            latin.zeroize();
            true
        } else {
            false
        };
        let mut notify = vec![SELECTION_NOTIFY, 0, 0, 0];
        notify.extend(time.to_le_bytes());
        notify.extend(requestor.to_le_bytes());
        notify.extend(selection.to_le_bytes());
        notify.extend(target.to_le_bytes());
        notify.extend((if given { property } else { 0 }).to_le_bytes());
        notify.extend([0u8; 8]);
        let mut req = vec![25, 0];
        req.extend(11u16.to_le_bytes());
        req.extend(requestor.to_le_bytes());
        req.extend(0u32.to_le_bytes());
        req.extend(notify);
        self.send(&req).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    //! A scripted display server on the other end of a socket pair: it answers the setup and
    //! the requests the owner makes, then sends a request for the text and a clear.

    use super::*;
    use std::os::unix::net::UnixStream;

    fn setup_reply(root: u32) -> Vec<u8> {
        let vendor = b"fake";
        let mut more = Vec::new();
        more.extend(0u32.to_le_bytes()); // release
        more.extend(0x0040_0000u32.to_le_bytes()); // id base
        more.extend(0x001f_ffffu32.to_le_bytes()); // id mask
        more.extend(0u32.to_le_bytes()); // motion buffer
        more.extend((vendor.len() as u16).to_le_bytes());
        more.extend(0xffffu16.to_le_bytes()); // max request
        more.extend([1, 1, 0, 0, 32, 32, 8, 255]); // screens, formats, ...
        more.extend([0u8; 4]);
        more.extend(vendor);
        more.extend([0u8; 8]); // one format
        more.extend(root.to_le_bytes()); // the screen's root
        more.extend([0u8; 36]);
        let mut out = vec![1, 0];
        out.extend(11u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(((more.len() / 4) as u16).to_le_bytes());
        out.extend(more);
        out
    }

    fn reply(seq: u16, value: u32) -> Vec<u8> {
        let mut r = vec![REPLY, 0];
        r.extend(seq.to_le_bytes());
        r.extend(0u32.to_le_bytes());
        r.extend(value.to_le_bytes());
        r.extend([0u8; 20]);
        r
    }

    fn read_request(s: &mut UnixStream) -> Vec<u8> {
        let mut head = [0u8; 4];
        s.read_exact(&mut head).unwrap();
        let mut rest = vec![0u8; usize::from(u16_at(&head, 2)) * 4 - 4];
        s.read_exact(&mut rest).unwrap();
        let mut out = head.to_vec();
        out.extend(rest);
        out
    }

    #[test]
    fn the_owner_answers_a_paste_with_the_text_and_wipes_it_when_cleared() {
        let (ours, mut server) = UnixStream::pair().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let owner = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                own(
                    ours,
                    Some(b"cookie".to_vec()),
                    Zeroizing::new("Pick up milk.".to_string()),
                    &stop,
                    tx,
                )
            })
        };
        // setup: the cookie is sent under its name
        let mut setup = [0u8; 12];
        server.read_exact(&mut setup).unwrap();
        assert_eq!(setup[0], b'l');
        assert_eq!(u16_at(&setup, 6), 18);
        let mut auth = vec![0u8; 20 + 8];
        server.read_exact(&mut auth).unwrap();
        assert!(auth.starts_with(COOKIE_NAME));
        server.write_all(&setup_reply(0x2a)).unwrap();
        // six atoms, then the window, the owner and the check
        let mut seq = 0u16;
        let mut atoms = Vec::new();
        for n in 0..6u32 {
            let req = read_request(&mut server);
            assert_eq!(req[0], 16);
            seq += 1;
            atoms.push(100 + n);
            server.write_all(&reply(seq, 100 + n)).unwrap();
        }
        let create = read_request(&mut server);
        assert_eq!(create[0], 1);
        assert_eq!(u32_at(&create, 8), 0x2a, "a child of the root");
        assert_eq!(u16_at(&create, 22), 2, "input only: never shown or focused");
        let window = u32_at(&create, 4);
        let set = read_request(&mut server);
        assert_eq!(
            (set[0], u32_at(&set, 4), u32_at(&set, 8)),
            (22, window, 100)
        );
        let get = read_request(&mut server);
        assert_eq!(get[0], 23);
        seq += 3;
        server.write_all(&reply(seq, window)).unwrap();
        assert_eq!(rx.recv().unwrap(), Ok(()));
        // a paste asks for UTF8_STRING into property 77 of window 9
        let mut ev = vec![SELECTION_REQUEST, 0, 0, 0];
        ev.extend(5u32.to_le_bytes());
        ev.extend(window.to_le_bytes());
        ev.extend(9u32.to_le_bytes());
        ev.extend(100u32.to_le_bytes());
        ev.extend(102u32.to_le_bytes());
        ev.extend(77u32.to_le_bytes());
        ev.extend([0u8; 4]);
        server.write_all(&ev).unwrap();
        let change = read_request(&mut server);
        assert_eq!(change[0], 18);
        assert_eq!((u32_at(&change, 4), u32_at(&change, 8)), (9, 77));
        assert_eq!(u32_at(&change, 20) as usize, "Pick up milk.".len());
        assert_eq!(&change[24..24 + 13], b"Pick up milk.");
        let notify = read_request(&mut server);
        assert_eq!((notify[0], notify[12]), (25, SELECTION_NOTIFY));
        assert_eq!(u32_at(&notify, 12 + 20), 77, "the property it was given");
        // an unknown target is refused
        let mut odd = ev.clone();
        odd[20..24].copy_from_slice(&999u32.to_le_bytes());
        server.write_all(&odd).unwrap();
        let notify = read_request(&mut server);
        assert_eq!(u32_at(&notify, 12 + 20), 0);
        // someone copies something else: the owner lets go
        let mut clear = vec![SELECTION_CLEAR, 0, 0, 0];
        clear.extend(6u32.to_le_bytes());
        clear.extend(window.to_le_bytes());
        clear.extend(100u32.to_le_bytes());
        clear.extend([0u8; 16]);
        server.write_all(&clear).unwrap();
        let destroy = read_request(&mut server);
        assert_eq!(destroy[0], 4);
        owner.join().unwrap();
    }

    #[test]
    fn a_display_is_a_local_socket_and_a_cookie_comes_from_its_entry() {
        assert_eq!(
            Target::parse(":0.0"),
            Some(Target {
                socket: "/tmp/.X11-unix/X0".into(),
                number: "0".into()
            })
        );
        assert_eq!(Target::parse("unix:12").unwrap().number, "12");
        assert_eq!(Target::parse("host:0"), None);
        assert_eq!(Target::parse(":"), None);
        let entry = |num: &str, cookie: &[u8]| {
            let mut e = vec![1, 0];
            for f in [b"host".as_slice(), num.as_bytes(), COOKIE_NAME, cookie] {
                e.extend((f.len() as u16).to_be_bytes());
                e.extend(f);
            }
            e
        };
        let mut file = entry("1", b"one");
        file.extend(entry("", b"any"));
        file.extend(entry("0", b"zero"));
        assert_eq!(parse_cookie(&file, "0").unwrap(), b"zero");
        assert_eq!(parse_cookie(&file, "5").unwrap(), b"any");
        assert_eq!(parse_cookie(&entry("1", b"one"), "5"), None);
        assert_eq!(
            parse_cookie(&file[..7], "0"),
            None,
            "a cut file reads as none"
        );
    }
}
