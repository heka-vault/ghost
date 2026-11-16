//! The owner against displays that misbehave (a socket that accepts and says nothing, one
//! that answers the setup with too little) and, where Xvfb is installed, a real display of
//! the test's own: never the operator's, and never shown the operator's cookie.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};

use super::*;

fn text(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_string())
}

/// A listener that accepts and then does `then` with each connection, on its own thread.
fn display(
    then: impl Fn(std::os::unix::net::UnixStream) + Send + Sync + 'static,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("X0");
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            then(s);
        }
    });
    (dir, path)
}

#[test]
fn a_text_over_the_limit_is_refused_before_anything_connects() {
    let c = XClipboard::at("/nonexistent/X0", 0, Auth::None);
    let long = "a".repeat(MAX_BYTES + 1);
    assert_eq!(c.set(text(&long)), Err(DictationReason::TooLong));
    assert!(!c.holding());
    // at the limit it is not refused for its length (here: no display)
    let at_limit = "a".repeat(MAX_BYTES);
    assert_eq!(c.set(text(&at_limit)), Err(DictationReason::NoDisplay));
}

#[test]
fn a_display_that_accepts_and_says_nothing_is_given_up_and_the_owner_joined() {
    // Finding 3: the setup is bounded, and the owner that waited is gone afterwards.
    let held = Arc::new(Mutex::new(Vec::new()));
    let (_dir, path) = display({
        let held = held.clone();
        move |s| lock(&held).push(s) // keep it open, answer nothing
    });
    let c = XClipboard::at(&path, 0, Auth::None);
    let began = Instant::now();
    let r = c.set(text("Pick up milk."));
    assert!(r.is_err(), "{r:?}");
    assert!(
        began.elapsed() < OWN_WAIT + Duration::from_secs(1),
        "{:?}",
        began.elapsed()
    );
    assert!(
        !c.holding(),
        "the owner thread is joined, not left with the text"
    );
}

#[test]
fn a_short_setup_reply_is_a_failure_not_a_panic() {
    // Finding 6: a success reply with too little after it.
    let (_dir, path) = display(|mut s| {
        let mut req = [0u8; 12];
        let _ = s.read_exact(&mut req);
        let _ = s.write_all(&[1, 0, 11, 0, 0, 0, 1, 0, 0, 0, 0, 0]);
        std::thread::sleep(Duration::from_millis(500));
    });
    let c = XClipboard::at(&path, 0, Auth::None);
    assert!(c.set(text("Pick up milk.")).is_err());
    assert!(!c.holding());
}

#[test]
fn closing_while_the_owner_sets_up_interrupts_it() {
    // Findings 3 and 4: Ghost stopping does not wait out a stalled display.
    let held = Arc::new(Mutex::new(Vec::new()));
    let (_dir, path) = display({
        let held = held.clone();
        move |s| lock(&held).push(s)
    });
    let c = Arc::new(XClipboard::at(&path, 0, Auth::None));
    let setting = {
        let c = c.clone();
        std::thread::spawn(move || c.set(text("Pick up milk.")))
    };
    std::thread::sleep(Duration::from_millis(200));
    let began = Instant::now();
    c.close();
    let r = setting.join().unwrap();
    assert!(r.is_err());
    assert!(
        began.elapsed() < Duration::from_millis(800),
        "{:?}",
        began.elapsed()
    );
    assert!(!c.holding()); // Closed for good: nothing more is set, nothing more connects.
    assert_eq!(c.set(text("Later.")), Err(DictationReason::ClipboardFailed));
    assert!(!c.holding());
}

// --- a real display ----------------------------------------------------------------------------

/// Xvfb on a display number of its own, killed (its own process only) when dropped.
struct Xvfb {
    child: Child,
    number: u16,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn xvfb() -> Option<Xvfb> {
    if Command::new("Xvfb")
        .arg("-help")
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("no Xvfb here: the real-display test is skipped");
        return None;
    }
    for number in (60u16..100).map(|n| n + (std::process::id() % 7) as u16) {
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
        let lockfile = PathBuf::from(format!("/tmp/.X{number}-lock"));
        if socket.exists() || lockfile.exists() {
            continue;
        }
        let child = Command::new("Xvfb")
            .args([
                format!(":{number}").as_str(),
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let x = Xvfb { child, number };
        // The socket's file is there before the server answers on it (a loaded CI runner
        // showed seconds between the two): wait for a plain client's setup to go through,
        // so what the test then measures is Ghost's client and not Xvfb's start.
        for _ in 0..200 {
            if socket.exists() && answers(number) {
                return Some(x);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("Xvfb :{number} never answered: the real-display test is skipped");
        return None;
    }
    None
}

/// Whether a plain blocking client gets through the display's setup.
fn answers(number: u16) -> bool {
    UnixStream::connect(format!("/tmp/.X11-unix/X{number}"))
        .ok()
        .and_then(|s| DefaultStream::from_unix_stream(s).ok())
        .and_then(|(stream, _)| RustConnection::connect_to_stream(stream, 0).ok())
        .is_some()
}

/// What another program pasting would get: CLIPBOARD as UTF8_STRING, or None.
fn paste(number: u16) -> Option<String> {
    let s = UnixStream::connect(format!("/tmp/.X11-unix/X{number}")).ok()?;
    let (stream, _) = DefaultStream::from_unix_stream(s).ok()?;
    let conn = RustConnection::connect_to_stream(stream, 0).ok()?;
    let root = conn.setup().roots[0].root;
    let win = conn.generate_id().ok()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .ok()?;
    let atom = |n: &[u8]| {
        conn.intern_atom(false, n)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    let (clipboard, utf8, prop) = (
        atom(b"CLIPBOARD")?,
        atom(b"UTF8_STRING")?,
        atom(b"GHOST_TEST")?,
    );
    conn.convert_selection(win, clipboard, utf8, prop, CURRENT_TIME)
        .ok()?;
    conn.flush().ok()?;
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        match conn.poll_for_event().ok()? {
            Some(Event::SelectionNotify(e)) => {
                if e.property == NONE {
                    return None;
                }
                let v = conn
                    .get_property(false, win, prop, AtomEnum::ANY, 0, 1 << 20)
                    .ok()?
                    .reply()
                    .ok()?;
                return String::from_utf8(v.value).ok();
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    None
}

#[test]
fn on_a_real_display_the_text_is_pasted_replaced_and_given_up() {
    let Some(x) = xvfb() else {
        return;
    };
    let c = XClipboard::at(
        format!("/tmp/.X11-unix/X{}", x.number),
        x.number,
        Auth::None,
    );
    c.set(text("Pick up milk.")).unwrap();
    assert_eq!(paste(x.number).as_deref(), Some("Pick up milk."));
    // the next dictation replaces it; the old owner is joined, not left behind
    c.set(text("And eggs, ünïcode too.")).unwrap();
    assert_eq!(paste(x.number).as_deref(), Some("And eggs, ünïcode too."));
    assert!(c.holding());
    // Ghost stopping: the owner gives it up and ends
    let began = Instant::now();
    c.close();
    assert!(
        began.elapsed() < Duration::from_millis(500),
        "{:?}",
        began.elapsed()
    );
    assert!(!c.holding());
    assert_eq!(paste(x.number), None, "nobody owns it after Ghost stopped");
}

#[test]
fn a_display_whose_queue_is_full_is_given_up_within_the_bound() {
    // A listener that never accepts, its queue filled: a blocking connect would wait for ever.
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("X0");
    let addr = SocketAddrUnix::new(&path).unwrap();
    let _listener = rustix::net::socket(AddressFamily::UNIX, SocketType::STREAM, None).unwrap();
    rustix::net::bind(&_listener, &addr).unwrap();
    rustix::net::listen(&_listener, 1).unwrap(); // a queue of one
    let mut queued = Vec::new();
    for _ in 0..4096 {
        let fd = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        if rustix::net::connect(&fd, &addr).is_err() {
            break; // the queue is full
        }
        queued.push(fd);
    }
    let c = Arc::new(XClipboard::at(&path, 0, Auth::None));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let c = c.clone();
        std::thread::spawn(move || {
            let _ = tx.send(c.set(text("Pick up milk.")));
        });
    }
    let r = rx
        .recv_timeout(OWN_WAIT + Duration::from_secs(3))
        .expect("set is still waiting on a full queue");
    assert!(r.is_err());
    assert!(!c.holding());
}
