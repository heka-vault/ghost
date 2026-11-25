//! An install or a sign-in, on the operator's click (ADR 0078): one at a time per program,
//! bounded, cancellable, and shown as it goes.
//!
//! An install runs the vendor's own command with `sh -c`, as the operator, never as root,
//! its output (both streams) read as lines. A sign-in runs the program's own sign-in in a
//! pseudo-terminal of its own (a new session, the terminal its controlling one), so the
//! program behaves as it would in the operator's terminal: it opens the browser itself,
//! prints the address in case it didn't, and may ask for a code from the browser's page. A
//! code the operator pastes is written to the terminal with Enter, held in memory only
//! (zeroized), hidden wherever the program echoes it, and never logged, stored, audited or
//! shown. What the program prints is someone else's text: its escape sequences taken out,
//! each line bounded and redacted, and the sign-in address kept only on the vendor's own
//! domains. Nothing of it reaches a log line: the log says what started and how it ended.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use zeroize::Zeroizing;

use super::text::{self, Screen};
use super::{SignInHow, Sub};

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Install,
    SignIn,
}

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    /// The program waits for a code from the browser's page.
    NeedsCode,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn ended(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }
}

/// A job as the desktop sees it. Never the pasted code.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub kind: JobKind,
    pub state: JobState,
    /// The command as shown: the vendor's install command, or the sign-in's.
    pub command: String,
    /// What the program printed, cleaned and bounded, oldest first.
    pub lines: Vec<String>,
    /// The sign-in address on the vendor's own domains, whole, for a link.
    pub url: Option<String>,
    /// A code the program shows for the operator to type on another device.
    pub device_code: Option<String>,
    /// How it ended, or what it waits for, in a sentence.
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Asked of the program when a sign-in ended.
    pub signed_in: Option<bool>,
}

/// Why a start, a code or a cancel was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotInstalled,
    Installed,
    /// A job of this program is running.
    Busy,
    NoJob,
    /// The sign-in is not waiting for a code.
    NotWaiting,
    /// The code is not one Ghost will type (the rule, never the code).
    Code(&'static str),
    /// Ghost runs as root: nothing is installed or signed in that way.
    Root,
    /// This program signs in with choices that are the operator's, on a screen of its own,
    /// and no terminal could be opened for it: the command to run.
    Terminal(&'static str),
    /// It could not be started.
    Start(String),
}

impl Refused {
    pub fn sentence(&self, title: &str) -> String {
        match self {
            Refused::NotInstalled => format!("{title} isn't installed on this computer."),
            Refused::Installed => format!("{title} is already installed."),
            Refused::Busy => format!("{title} is already being installed or signed in."),
            Refused::NoJob => format!("Nothing of {title}'s is running."),
            Refused::NotWaiting => format!("{title} isn't waiting for a code."),
            Refused::Code(why) => (*why).to_string(),
            Refused::Root => {
                "Ghost runs as root here, so it installs and signs in nothing.".to_string()
            }
            Refused::Terminal(cmd) => format!(
                "Ghost couldn't open a terminal. Run `{cmd}` in a terminal and sign in there, \
                 then look again."
            ),
            Refused::Start(why) => format!("{title} could not be started: {why}."),
        }
    }
}

/// The terminals tried, in order, each with how it is told the program to run: the
/// system's chosen one first (`x-terminal-emulator`, Debian's and Ubuntu's), then GNOME's
/// Ptyxis and Terminal, GNOME Console, Konsole, xterm.
pub const TERMINALS: [(&str, &[&str]); 6] = [
    ("x-terminal-emulator", &["-e"]),
    ("ptyxis", &["--"]),
    ("gnome-terminal", &["--"]),
    ("kgx", &["--"]),
    ("konsole", &["-e"]),
    ("xterm", &["-e"]),
];

/// Open the first terminal there is, running `binary`, detached from Ghost (its own session,
/// so it outlives Ghost's restarts and Ghost's stop never reaches it), in `env`.
fn open_terminal(
    binary: &std::path::Path,
    env: &std::collections::BTreeMap<String, String>,
    home: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let path = env.get("PATH").cloned().unwrap_or_default();
    for (term, how) in TERMINALS {
        let found = path
            .split(':')
            .map(|d| std::path::Path::new(d).join(term))
            .find(|p| p.is_file());
        let Some(term) = found else { continue };
        let mut cmd = std::process::Command::new(term);
        cmd.args(how)
            .arg(binary)
            .env_clear()
            .envs(env)
            .current_dir(home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
}

/// How long each job may take.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub sign_in: Duration,
    pub install: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            sign_in: Duration::from_secs(15 * 60),
            install: Duration::from_secs(10 * 60),
        }
    }
}

struct Inner {
    screen: Screen,
    state: JobState,
    message: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    signed_in: Option<bool>,
    /// Lines ended when the last code was written, and that one was.
    code_at: Option<usize>,
}

struct Job {
    kind: JobKind,
    command: String,
    started_at: DateTime<Utc>,
    inner: Mutex<Inner>,
    cancel: CancellationToken,
    code: Option<mpsc::Sender<Zeroizing<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Job {
    fn snapshot(&self) -> JobSnapshot {
        let i = lock(&self.inner);
        let mut lines = i.screen.lines();
        let pending = i.screen.pending();
        if !pending.is_empty() && lines.last() != Some(&pending) {
            lines.push(pending);
        }
        JobSnapshot {
            kind: self.kind,
            state: i.state,
            command: self.command.clone(),
            lines,
            url: i.screen.url.clone(),
            device_code: i.screen.device_code.clone(),
            message: i.message.clone(),
            started_at: self.started_at,
            ended_at: i.ended_at,
            signed_in: i.signed_in,
        }
    }

    fn running(&self) -> bool {
        !lock(&self.inner).state.ended()
    }

    fn end(&self, state: JobState, message: Option<String>, signed_in: Option<bool>) {
        let mut i = lock(&self.inner);
        i.state = state;
        i.message = message;
        i.signed_in = signed_in;
        i.ended_at = Some(Utc::now());
        i.screen.forget_code();
    }

    /// Take in what the program wrote, and whether it now waits for a code.
    fn feed(&self, bytes: &[u8]) {
        let mut i = lock(&self.inner);
        i.screen.feed(bytes);
        i.screen.notice_pending();
        if i.state.ended() || self.kind != JobKind::SignIn {
            return;
        }
        let ended = i.screen.lines().len() + i.screen.dropped;
        let pending = i.screen.pending();
        let (prompt, fresh) = if pending.is_empty() {
            let last = i.screen.lines().last().cloned().unwrap_or_default();
            (last, i.code_at.is_none_or(|at| ended > at + 1))
        } else {
            (pending, i.code_at.is_none_or(|at| ended > at))
        };
        // A prompt that holds the hidden echo is the one the code was given to.
        if fresh && !prompt.contains(text::HIDDEN) && text::asks_for_code(tail(&prompt)) {
            if i.state != JobState::NeedsCode && i.code_at.is_some() {
                i.message = Some("It asks for a code again. Check the code and paste it.".into());
            }
            i.state = JobState::NeedsCode;
        }
    }
}

/// The last 160 characters: a prompt drawn after other text on its line.
fn tail(s: &str) -> &str {
    let n = s.chars().count();
    if n <= 160 {
        return s;
    }
    let skip = s.char_indices().nth(n - 160).map_or(0, |(i, _)| i);
    &s[skip..]
}

/// The jobs, one per program at a time, and the environment Ghost was started in.
pub struct Jobs {
    started: Arc<HashMap<String, String>>,
    jobs: Mutex<HashMap<Sub, Arc<Job>>>,
    limits: Limits,
}

impl Jobs {
    pub fn new(started: Arc<HashMap<String, String>>) -> Self {
        Self::with_limits(started, Limits::default())
    }

    pub fn with_limits(started: Arc<HashMap<String, String>>, limits: Limits) -> Self {
        Self {
            started,
            jobs: Mutex::new(HashMap::new()),
            limits,
        }
    }

    pub fn started(&self) -> &HashMap<String, String> {
        &self.started
    }

    /// The program's last job, running or ended.
    pub fn job(&self, sub: Sub) -> Option<JobSnapshot> {
        lock(&self.jobs).get(&sub).map(|j| j.snapshot())
    }

    fn free(&self, sub: Sub) -> Result<(), Refused> {
        if rustix::process::geteuid().is_root() {
            return Err(Refused::Root);
        }
        if lock(&self.jobs).get(&sub).is_some_and(|j| j.running()) {
            return Err(Refused::Busy);
        }
        Ok(())
    }

    fn home(&self) -> PathBuf {
        self.started
            .get("HOME")
            .filter(|h| h.starts_with('/'))
            .map_or_else(std::env::temp_dir, PathBuf::from)
    }

    /// Run the vendor's own install command, on the operator's click.
    pub fn start_install(&self, sub: Sub) -> Result<JobSnapshot, Refused> {
        let spec = sub.spec();
        self.free(sub)?;
        if super::which(spec.binary, &self.started).is_some() {
            return Err(Refused::Installed);
        }
        let env = super::base_env(sub, &self.started);
        let (reader, writer) = std::io::pipe().map_err(|e| Refused::Start(e.kind().to_string()))?;
        let err_writer = writer
            .try_clone()
            .map_err(|e| Refused::Start(e.kind().to_string()))?;
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(spec.install)
            .env_clear()
            .envs(&env)
            .current_dir(self.home())
            .stdin(std::process::Stdio::null())
            .stdout(writer)
            .stderr(err_writer)
            .process_group(0)
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| Refused::Start(e.kind().to_string()))?;
        drop(cmd); // the parent's copies of the pipe's write end
        let job = self.put(sub, JobKind::Install, spec.install.to_string(), None);
        info!(
            "subscriptions: installing {} (the vendor's command)",
            spec.title
        );
        let started = self.started.clone();
        let limit = self.limits.install;
        let j = job.clone();
        tokio::spawn(async move {
            run_install(j, child, OwnedFd::from(reader), sub, started, limit).await;
        });
        Ok(job.snapshot())
    }

    /// Run the program's own sign-in in a terminal of Ghost's, on the operator's click.
    pub fn start_sign_in(&self, sub: Sub) -> Result<JobSnapshot, Refused> {
        let spec = sub.spec();
        let args = match spec.sign_in {
            SignInHow::Pty(args) => args,
            // Its sign-in is a screen of its own with choices that are the operator's (the
            // account, the terms, which folder to trust): Ghost opens a terminal window running
            // it, on the operator's click, and the operator signs in there.
            SignInHow::Terminal(cmd) => {
                self.free(sub)?;
                let binary =
                    super::which(spec.binary, &self.started).ok_or(Refused::NotInstalled)?;
                let env = super::sign_in_env(sub, &self.started);
                open_terminal(&binary, &env, &self.home()).map_err(|_| Refused::Terminal(cmd))?;
                let job = self.put(sub, JobKind::SignIn, cmd.to_string(), None);
                job.end(
                    JobState::Done,
                    Some(format!(
                        "Ghost opened a terminal running {cmd}. Sign in there, then look again."
                    )),
                    None,
                );
                info!("subscriptions: {} sign-in opened in a terminal", spec.title);
                return Ok(job.snapshot());
            }
        };
        self.free(sub)?;
        let binary = super::which(spec.binary, &self.started).ok_or(Refused::NotInstalled)?;
        let env = super::sign_in_env(sub, &self.started);
        let (child, master) = spawn_in_pty(&binary, args, &env, &self.home())
            .map_err(|e| Refused::Start(e.kind().to_string()))?;
        let (tx, rx) = mpsc::channel(2);
        let job = self.put(
            sub,
            JobKind::SignIn,
            spec.sign_in_shown.to_string(),
            Some(tx),
        );
        info!(
            "subscriptions: {} sign-in started (its own command)",
            spec.title
        );
        let started = self.started.clone();
        let limit = self.limits.sign_in;
        let j = job.clone();
        tokio::spawn(async move {
            run_sign_in(j, child, master, rx, sub, binary, started, limit).await;
        });
        Ok(job.snapshot())
    }

    fn put(
        &self,
        sub: Sub,
        kind: JobKind,
        command: String,
        code: Option<mpsc::Sender<Zeroizing<String>>>,
    ) -> Arc<Job> {
        let job = Arc::new(Job {
            kind,
            command,
            started_at: Utc::now(),
            inner: Mutex::new(Inner {
                screen: Screen::new(sub.spec().domains),
                state: JobState::Running,
                message: None,
                ended_at: None,
                signed_in: None,
                code_at: None,
            }),
            cancel: CancellationToken::new(),
            code,
        });
        lock(&self.jobs).insert(sub, job.clone());
        job
    }

    /// Give the waiting sign-in the code from the browser's page. The code is typed with
    /// Enter and kept only to hide its echo until the program ends.
    pub async fn send_code(
        &self,
        sub: Sub,
        code: Zeroizing<String>,
    ) -> Result<JobSnapshot, Refused> {
        text::check_code(&code).map_err(Refused::Code)?;
        let job = lock(&self.jobs).get(&sub).cloned().ok_or(Refused::NoJob)?;
        if job.kind != JobKind::SignIn || !job.running() {
            return Err(Refused::NoJob);
        }
        if lock(&job.inner).state != JobState::NeedsCode {
            return Err(Refused::NotWaiting);
        }
        let tx = job.code.clone().ok_or(Refused::NoJob)?;
        let code = Zeroizing::new(code.trim().to_string());
        tx.send(code).await.map_err(|_| Refused::NoJob)?;
        Ok(job.snapshot())
    }

    /// Stop the program's job: its whole process group.
    pub fn cancel(&self, sub: Sub) -> Result<JobSnapshot, Refused> {
        let job = lock(&self.jobs).get(&sub).cloned().ok_or(Refused::NoJob)?;
        if !job.running() {
            return Err(Refused::NoJob);
        }
        job.cancel.cancel();
        Ok(job.snapshot())
    }

    /// Stop every job (Ghost stopping).
    pub fn cancel_all(&self) {
        for j in lock(&self.jobs).values() {
            j.cancel.cancel();
        }
    }
}

/// Open a pseudo-terminal and start `binary args` on it: a session of its own, the terminal
/// its controlling one, echo off, wide enough that an address is never wrapped.
fn spawn_in_pty(
    binary: &Path,
    args: &[&str],
    env: &std::collections::BTreeMap<String, String>,
    cwd: &Path,
) -> std::io::Result<(tokio::process::Child, OwnedFd)> {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;
    use rustix::termios::{LocalModes, OptionalActions, Winsize};

    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    let name = rustix::pty::ptsname(&master, Vec::new())?;
    rustix::termios::tcsetwinsize(
        &master,
        Winsize {
            ws_row: 50,
            ws_col: 1000,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    // The terminal doesn't echo what Ghost types: a program in its own raw mode may still,
    // and the screen hides that.
    let mut t = rustix::termios::tcgetattr(&slave)?;
    t.local_modes.remove(LocalModes::ECHO);
    rustix::termios::tcsetattr(&slave, OptionalActions::Now, &t)?;
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(args)
        .env_clear()
        .envs(env)
        .current_dir(cwd)
        .stdin(std::process::Stdio::from(slave.try_clone()?))
        .stdout(std::process::Stdio::from(slave.try_clone()?))
        .stderr(std::process::Stdio::from(slave))
        .kill_on_drop(true);
    // SAFETY: only async-signal-safe system calls between fork and exec: a new session, and
    // standard input (the terminal) made its controlling terminal.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()?;
            rustix::process::ioctl_tiocsctty(BorrowedFd::borrow_raw(0))?;
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    drop(cmd); // the parent's copies of the terminal's other end
    rustix::io::ioctl_fionbio(&master, true)?;
    Ok((child, master))
}

/// Signal the process group `pid` leads.
fn signal_group(pid: Option<u32>, sig: rustix::process::Signal) {
    if let Some(p) = pid.and_then(|p| rustix::process::Pid::from_raw(p as i32)) {
        let _ = rustix::process::kill_process_group(p, sig);
    }
}

/// Stop the group: TERM, then KILL if it is still there after two seconds.
async fn stop(child: &mut tokio::process::Child) {
    let pid = child.id();
    signal_group(pid, rustix::process::Signal::TERM);
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        signal_group(pid, rustix::process::Signal::KILL);
        let _ = child.wait().await;
    }
}

/// Read what is there now; `false` when the other end is gone.
fn read_now(fd: BorrowedFd<'_>, buf: &mut [u8]) -> std::io::Result<usize> {
    rustix::io::read(fd, buf).map_err(std::io::Error::from)
}

#[allow(clippy::too_many_arguments)]
async fn run_sign_in(
    job: Arc<Job>,
    mut child: tokio::process::Child,
    master: OwnedFd,
    mut codes: mpsc::Receiver<Zeroizing<String>>,
    sub: Sub,
    binary: PathBuf,
    started: Arc<HashMap<String, String>>,
    limit: Duration,
) {
    let title = sub.spec().title;
    // SAFETY: the `OwnedFd` moves into the `AsyncFd`, which owns it until it is dropped and
    // deregistered; nothing else can close it in between.
    let fd = match unsafe { AsyncFd::register(master) }.map_err(std::io::Error::from) {
        Ok(fd) => fd,
        Err(e) => {
            stop(&mut child).await;
            job.end(
                JobState::Failed,
                Some(Refused::Start(e.kind().to_string()).sentence(title)),
                None,
            );
            return;
        }
    };
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut buf = vec![0u8; 8192];
    let mut open = true;
    enum End {
        Exited(Option<i32>),
        Cancelled,
        TimedOut,
    }
    let end = loop {
        tokio::select! {
            () = job.cancel.cancelled() => {
                stop(&mut child).await;
                break End::Cancelled;
            }
            () = &mut deadline => {
                stop(&mut child).await;
                break End::TimedOut;
            }
            Some(code) = codes.recv() => {
                {
                    let mut i = lock(&job.inner);
                    i.screen.hide(Zeroizing::new(code.to_string()));
                    i.code_at = Some(i.screen.lines().len() + i.screen.dropped);
                    i.state = JobState::Running;
                    i.message = None;
                }
                let mut typed = Zeroizing::new(code.as_bytes().to_vec());
                typed.push(b'\r');
                write_all(&fd, &typed).await;
            }
            status = child.wait() => {
                // What it wrote last, before the terminal is let go.
                for _ in 0..64 {
                    match read_now(fd.get_ref().as_fd(), &mut buf) {
                        Ok(n) if n > 0 => job.feed(&buf[..n]),
                        _ => break,
                    }
                }
                break End::Exited(status.ok().and_then(|s| s.code()));
            }
            guard = fd.readable(), if open => {
                let Ok(mut guard) = guard else { open = false; continue };
                match guard.try_io(|f| read_now(f.get_ref().as_fd(), &mut buf)) {
                    Ok(Ok(0)) => open = false,
                    Ok(Ok(n)) => job.feed(&buf[..n]),
                    // EIO: the program's end of the terminal is closed.
                    Ok(Err(_)) => open = false,
                    Err(_would_block) => {}
                }
            }
        }
    };
    buf.fill(0);
    match end {
        End::Cancelled => {
            info!("subscriptions: {title} sign-in cancelled");
            job.end(JobState::Cancelled, Some("Cancelled.".into()), None);
        }
        End::TimedOut => {
            info!("subscriptions: {title} sign-in stopped after its limit");
            job.end(
                JobState::Failed,
                Some(format!(
                    "Stopped after {} minutes with no sign-in.",
                    limit.as_secs().div_ceil(60)
                )),
                None,
            );
        }
        End::Exited(code) => {
            let signed = super::signed_in(sub, &binary, &started).await;
            info!("subscriptions: {title} sign-in ended (exit {code:?}, signed in {signed})");
            let (state, message) = match (signed, code) {
                (true, _) => (JobState::Done, format!("{title} is signed in.")),
                (false, Some(0)) => (
                    JobState::Failed,
                    format!("{title}'s sign-in ended, and it still isn't signed in."),
                ),
                (false, Some(n)) => (
                    JobState::Failed,
                    format!("{title}'s sign-in ended without signing in (it exited with {n})."),
                ),
                (false, None) => (
                    JobState::Failed,
                    format!("{title}'s sign-in was stopped before it signed in."),
                ),
            };
            job.end(state, Some(message), Some(signed));
        }
    }
}

/// Write all of `bytes` to the terminal.
async fn write_all(fd: &AsyncFd<OwnedFd>, bytes: &[u8]) {
    let mut at = 0;
    let mut tries = 0;
    while at < bytes.len() && tries < 200 {
        tries += 1;
        let Ok(mut guard) = fd.writable().await else {
            return;
        };
        match guard.try_io(|f| {
            rustix::io::write(f.get_ref().as_fd(), &bytes[at..]).map_err(std::io::Error::from)
        }) {
            Ok(Ok(n)) => at += n,
            Ok(Err(_)) => return,
            Err(_would_block) => {}
        }
    }
}

async fn run_install(
    job: Arc<Job>,
    mut child: tokio::process::Child,
    reader: OwnedFd,
    sub: Sub,
    started: Arc<HashMap<String, String>>,
    limit: Duration,
) {
    let title = sub.spec().title;
    let rx = match tokio::net::unix::pipe::Receiver::from_owned_fd(reader) {
        Ok(r) => r,
        Err(e) => {
            stop(&mut child).await;
            job.end(
                JobState::Failed,
                Some(Refused::Start(e.kind().to_string()).sentence(title)),
                None,
            );
            return;
        }
    };
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut buf = vec![0u8; 8192];
    let mut open = true;
    enum End {
        Exited(Option<i32>),
        Cancelled,
        TimedOut,
    }
    let end = loop {
        tokio::select! {
            () = job.cancel.cancelled() => {
                stop(&mut child).await;
                break End::Cancelled;
            }
            () = &mut deadline => {
                stop(&mut child).await;
                break End::TimedOut;
            }
            status = child.wait() => {
                for _ in 0..64 {
                    match rx.try_read(&mut buf) {
                        Ok(n) if n > 0 => job.feed(&buf[..n]),
                        _ => break,
                    }
                }
                break End::Exited(status.ok().and_then(|s| s.code()));
            }
            r = rx.readable(), if open => {
                if r.is_err() { open = false; continue; }
                match rx.try_read(&mut buf) {
                    Ok(0) => open = false,
                    Ok(n) => job.feed(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => open = false,
                }
            }
        }
    };
    // The last line, if it had no end.
    job.feed(b"\n");
    match end {
        End::Cancelled => {
            info!("subscriptions: installing {title} cancelled");
            job.end(JobState::Cancelled, Some("Cancelled.".into()), None);
        }
        End::TimedOut => {
            info!("subscriptions: installing {title} stopped after its limit");
            job.end(
                JobState::Failed,
                Some(format!(
                    "Stopped after {} minutes.",
                    limit.as_secs().div_ceil(60)
                )),
                None,
            );
        }
        End::Exited(code) => {
            let here = super::which(sub.spec().binary, &started).is_some();
            info!("subscriptions: installing {title} ended (exit {code:?}, installed {here})");
            let (state, message) = match (here, code) {
                (true, _) => (JobState::Done, format!("{title} is installed.")),
                (false, Some(0)) => (
                    JobState::Failed,
                    format!(
                        "The installer finished, but Ghost can't find `{}`.",
                        sub.spec().binary
                    ),
                ),
                (false, Some(n)) => (
                    JobState::Failed,
                    format!("The installer stopped with an error (it exited with {n})."),
                ),
                (false, None) => (JobState::Failed, "The installer was stopped.".to_string()),
            };
            job.end(state, Some(message), None);
        }
    }
}
