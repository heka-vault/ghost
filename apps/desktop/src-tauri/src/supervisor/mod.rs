//! The shell owns the daemon's life (ADR 0038).
//!
//! At launch the supervisor looks for a daemon already answering on the control socket
//! (`make dev`'s, or one started by hand) and adopts it. Otherwise it runs
//! `ghostd restore-volumes`, spawns `ghostd run` in its own process group, and starts it
//! again with backoff (5 s doubling to 60 s) when it exits. On Quit it sends TERM to the
//! group, waits up to 20 s, then KILL, then `restore-volumes`.
//!
//! The state machine is here, over the [`Processes`] seam; [`system`] is the one
//! implementation that touches the operating system. `tick` takes the clock as an argument
//! so the tests drive it without waiting.

pub mod logfile;
pub mod settings_env;
pub mod system;

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde::Serialize;

/// How often a running child is checked, and the longest one tick pauses.
const CHILD_POLL: Duration = Duration::from_secs(1);
/// How often the socket is probed while adopted or waiting.
const PROBE_EVERY: Duration = Duration::from_secs(2);
/// An adopted daemon that has not answered for this long is gone; the shell takes over.
/// `make reload` restarts a checkout's daemon well inside it.
pub const ADOPT_GRACE: Duration = Duration::from_secs(30);
/// The first restart delay, doubled per consecutive exit up to [`BACKOFF_MAX`].
pub const BACKOFF_FIRST: Duration = Duration::from_secs(5);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A child that ran at least this long was healthy: the next delay starts over.
const HEALTHY_RUN: Duration = Duration::from_secs(120);
/// TERM, then this long, then KILL (ADR 0038; the unit's `TimeoutStopSec=20`).
pub const TERM_GRACE: Duration = Duration::from_secs(20);
const KILL_GRACE: Duration = Duration::from_secs(2);

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Code(c) => write!(f, "exit code {c}"),
            Exit::Signal(s) => write!(f, "signal {s}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sig {
    Term,
    Kill,
}

/// A spawned `ghostd run`: its process group is its own, so a signal reaches everything it
/// started.
pub trait Child {
    fn pid(&self) -> u32;
    /// `Some` once the child has exited; never blocks.
    fn try_wait(&mut self) -> Option<Exit>;
    /// Blocks up to `timeout` for the child to exit.
    fn wait_timeout(&mut self, timeout: Duration) -> Option<Exit>;
    /// Signal the child's whole process group.
    fn signal_group(&mut self, sig: Sig);
}

/// What the supervisor needs of the operating system: the socket, the daemon binary, and
/// `restore-volumes`.
pub trait Processes {
    type Child: Child;
    /// A daemon answers `GET /status` on the control socket.
    fn daemon_answers(&self) -> bool;
    /// `ghostd restore-volumes`, best effort: music a crash left ducked comes back.
    fn restore_volumes(&self);
    /// `ghostd run` in its own process group, with `settings.env` and the launcher's
    /// variables; its output in the journal or the log file.
    fn spawn(&self) -> Result<Self::Child, String>;
}

/// What the shell saw or did, for the window (`ghost://daemon`) and the log. The window
/// says "Ghost restarted" on `Started` with `restarts > 0`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum DaemonEvent {
    /// A daemon already answered on the socket; the shell runs none of its own.
    Adopted,
    /// Adopt-only (`make dev`, `make desktop`) and nothing answers yet.
    Waiting,
    /// The shell's own daemon is up.
    Started { pid: u32, restarts: u32 },
    /// The shell's daemon exited; it starts again after `in_secs`.
    Restarting {
        in_secs: u64,
        restarts: u32,
        exit: String,
    },
    /// The daemon could not be started; tried again after `in_secs`.
    Failed { error: String, in_secs: u64 },
    /// Quit: TERM to the group, then KILL after 20 s.
    Stopping,
    /// The daemon is down and the volumes restored.
    Stopped,
}

enum State<C> {
    /// Probe the socket: adopt, wait, or spawn.
    Probing,
    /// Adopt-only and nothing answers.
    Waiting,
    /// Another daemon has the socket; `last_seen` is its last answer.
    Adopted {
        last_seen: Instant,
    },
    /// The shell's own daemon.
    Running {
        child: C,
        since: Instant,
    },
    /// Between an exit and the next start.
    Backoff {
        until: Instant,
    },
    Stopped,
}

/// The restart delay: 5 s doubling to 60 s per consecutive exit, over after a healthy run.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    consecutive: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self { consecutive: 0 }
    }

    /// The delay before the next start, after a run of `ran` length.
    pub fn next(&mut self, ran: Duration) -> Duration {
        if ran >= HEALTHY_RUN {
            self.consecutive = 0;
        }
        let d = BACKOFF_FIRST.saturating_mul(1u32 << self.consecutive.min(4));
        self.consecutive = self.consecutive.saturating_add(1);
        d.min(BACKOFF_MAX)
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Supervisor<P: Processes> {
    procs: P,
    adopt_only: bool,
    state: State<P::Child>,
    backoff: Backoff,
    restarts: u32,
    on_event: Box<dyn FnMut(&DaemonEvent) + Send>,
}

impl<P: Processes> Supervisor<P> {
    /// `adopt_only`: never spawn; `make dev` sets it, since dev.sh runs the daemon.
    pub fn new(
        procs: P,
        adopt_only: bool,
        on_event: impl FnMut(&DaemonEvent) + Send + 'static,
    ) -> Self {
        Self {
            procs,
            adopt_only,
            state: State::Probing,
            backoff: Backoff::new(),
            restarts: 0,
            on_event: Box::new(on_event),
        }
    }

    fn emit(&mut self, e: DaemonEvent) {
        (self.on_event)(&e);
    }

    /// The shell's own daemon is running.
    #[cfg(test)]
    pub fn owns_daemon(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// One step at `now`; returns how long to pause before the next.
    pub fn tick(&mut self, now: Instant) -> Duration {
        match std::mem::replace(&mut self.state, State::Stopped) {
            State::Probing => self.probe(now),
            State::Waiting => {
                if self.procs.daemon_answers() {
                    self.state = State::Adopted { last_seen: now };
                    self.emit(DaemonEvent::Adopted);
                } else {
                    self.state = State::Waiting;
                }
                PROBE_EVERY
            }
            State::Adopted { last_seen } => {
                if self.procs.daemon_answers() {
                    self.state = State::Adopted { last_seen: now };
                } else if self.adopt_only {
                    if now.duration_since(last_seen) >= ADOPT_GRACE {
                        self.state = State::Waiting;
                        self.emit(DaemonEvent::Waiting);
                    } else {
                        self.state = State::Adopted { last_seen };
                    }
                } else if now.duration_since(last_seen) >= ADOPT_GRACE {
                    // Gone for good: the shell takes over, with one more probe first.
                    self.state = State::Probing;
                    return Duration::ZERO;
                } else {
                    self.state = State::Adopted { last_seen };
                }
                PROBE_EVERY
            }
            State::Running { mut child, since } => match child.try_wait() {
                None => {
                    self.state = State::Running { child, since };
                    CHILD_POLL
                }
                Some(exit) => {
                    let delay = self.backoff.next(now.duration_since(since));
                    self.restarts += 1;
                    self.emit(DaemonEvent::Restarting {
                        in_secs: delay.as_secs(),
                        restarts: self.restarts,
                        exit: exit.to_string(),
                    });
                    self.state = State::Backoff { until: now + delay };
                    delay
                }
            },
            State::Backoff { until } => {
                if now >= until {
                    self.state = State::Probing;
                    Duration::ZERO
                } else {
                    self.state = State::Backoff { until };
                    until - now
                }
            }
            State::Stopped => {
                self.state = State::Stopped;
                Duration::from_secs(3600)
            }
        }
    }

    fn probe(&mut self, now: Instant) -> Duration {
        if self.procs.daemon_answers() {
            self.state = State::Adopted { last_seen: now };
            self.emit(DaemonEvent::Adopted);
            return PROBE_EVERY;
        }
        if self.adopt_only {
            self.state = State::Waiting;
            self.emit(DaemonEvent::Waiting);
            return PROBE_EVERY;
        }
        self.procs.restore_volumes();
        match self.procs.spawn() {
            Ok(child) => {
                let pid = child.pid();
                self.state = State::Running { child, since: now };
                self.emit(DaemonEvent::Started {
                    pid,
                    restarts: self.restarts,
                });
                CHILD_POLL
            }
            Err(error) => {
                let delay = self.backoff.next(Duration::ZERO);
                self.emit(DaemonEvent::Failed {
                    error,
                    in_secs: delay.as_secs(),
                });
                self.state = State::Backoff { until: now + delay };
                delay
            }
        }
    }

    /// The window's Start button: skip the rest of a restart delay. Nothing to do while
    /// a daemon runs; an error in adopt-only mode, where the daemon is dev.sh's to start.
    pub fn start_now(&mut self) -> Result<(), String> {
        match &self.state {
            State::Backoff { .. } => {
                self.state = State::Probing;
                Ok(())
            }
            State::Waiting => {
                Err("Ghost is run from a checkout here (make dev); make reload starts it.".into())
            }
            State::Stopped => Err("Ghost is quitting.".into()),
            State::Probing | State::Adopted { .. } | State::Running { .. } => Ok(()),
        }
    }

    /// Quit: stop the shell's own daemon, if it has one, and restore the volumes. An
    /// adopted daemon is not the shell's to stop.
    pub fn stop(&mut self) {
        match std::mem::replace(&mut self.state, State::Stopped) {
            State::Running { mut child, .. } => {
                self.emit(DaemonEvent::Stopping);
                child.signal_group(Sig::Term);
                if child.wait_timeout(TERM_GRACE).is_none() {
                    child.signal_group(Sig::Kill);
                    child.wait_timeout(KILL_GRACE);
                }
                self.procs.restore_volumes();
                self.emit(DaemonEvent::Stopped);
            }
            _ => self.emit(DaemonEvent::Stopped),
        }
    }
}

/// What the shell sends its supervisor thread.
pub enum Command {
    /// Quit: the stop sequence, then the thread ends.
    Stop,
    /// The window's Start button; the answer says whether there was anything to do.
    StartNow(Sender<Result<(), String>>),
}

/// A handle to the supervisor thread: `stop` runs the stop sequence and returns when the
/// daemon is down (or the thread is gone).
pub struct Handle {
    tx: Sender<Command>,
    done: Receiver<()>,
}

impl Handle {
    /// Skip the rest of a restart delay.
    pub fn start_now(&self) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx
            .send(Command::StartNow(tx))
            .map_err(|_| "Ghost is quitting.".to_string())?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the supervisor did not answer".to_string())?
    }

    /// Stop the shell's daemon and wait for the sequence to finish.
    pub fn stop(&self) {
        if self.tx.send(Command::Stop).is_ok() {
            // TERM grace, KILL grace, restore-volumes: a little more than their sum.
            let _ = self
                .done
                .recv_timeout(TERM_GRACE + KILL_GRACE + Duration::from_secs(15));
        }
    }
}

/// Run `sup` on its own thread until `Command::Stop`. A thread of its own, not the async
/// runtime's: the daemon's parent-death signal is tied to the thread that spawned it.
pub fn spawn_thread<P: Processes + Send + 'static>(mut sup: Supervisor<P>) -> Handle
where
    P::Child: Send,
{
    let (tx, rx) = std::sync::mpsc::channel::<Command>();
    let (done_tx, done) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("ghost-supervisor".into())
        .spawn(move || {
            loop {
                let pause = sup.tick(Instant::now());
                match rx.recv_timeout(pause) {
                    Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        sup.stop();
                        let _ = done_tx.send(());
                        return;
                    }
                    Ok(Command::StartNow(reply)) => {
                        let _ = reply.send(sup.start_now());
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        })
        .expect("supervisor thread");
    Handle { tx, done }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    /// The exits the spawned children will report, set by the test after the spawn.
    type ExitCells = Rc<RefCell<Vec<Rc<RefCell<Option<Exit>>>>>>;

    /// What the fake child does when signalled.
    #[derive(Clone, Copy)]
    enum OnTerm {
        Exits,
        Ignores,
    }

    struct FakeChild {
        pid: u32,
        exit: Option<Exit>,
        on_term: OnTerm,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl Child for FakeChild {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn try_wait(&mut self) -> Option<Exit> {
            self.exit
        }
        fn wait_timeout(&mut self, timeout: Duration) -> Option<Exit> {
            self.log
                .borrow_mut()
                .push(format!("wait {}s", timeout.as_secs()));
            self.exit
        }
        fn signal_group(&mut self, sig: Sig) {
            self.log
                .borrow_mut()
                .push(format!("{sig:?} pid {}", self.pid));
            match (sig, self.on_term) {
                (Sig::Term, OnTerm::Exits) => self.exit = Some(Exit::Signal(15)),
                (Sig::Kill, _) => self.exit = Some(Exit::Signal(9)),
                (Sig::Term, OnTerm::Ignores) => {}
            }
        }
    }

    /// The seam's fake: answers or not, spawns or fails, and records every call. The exits
    /// the children will report come from `exits`, in order.
    struct Fake {
        answers: Rc<RefCell<bool>>,
        spawn_ok: bool,
        on_term: OnTerm,
        log: Rc<RefCell<Vec<String>>>,
        next_pid: RefCell<u32>,
        children: ExitCells,
    }

    /// A child whose exit the test can set later, through the shared cell.
    struct LiveChild {
        inner: FakeChild,
        cell: Rc<RefCell<Option<Exit>>>,
    }

    impl Child for LiveChild {
        fn pid(&self) -> u32 {
            self.inner.pid
        }
        fn try_wait(&mut self) -> Option<Exit> {
            if let Some(e) = *self.cell.borrow() {
                self.inner.exit = Some(e);
            }
            self.inner.try_wait()
        }
        fn wait_timeout(&mut self, timeout: Duration) -> Option<Exit> {
            self.inner.wait_timeout(timeout)
        }
        fn signal_group(&mut self, sig: Sig) {
            self.inner.signal_group(sig)
        }
    }

    impl Processes for Fake {
        type Child = LiveChild;
        fn daemon_answers(&self) -> bool {
            self.log.borrow_mut().push("probe".into());
            *self.answers.borrow()
        }
        fn restore_volumes(&self) {
            self.log.borrow_mut().push("restore-volumes".into());
        }
        fn spawn(&self) -> Result<LiveChild, String> {
            if !self.spawn_ok {
                self.log.borrow_mut().push("spawn failed".into());
                return Err("no such file".into());
            }
            let mut pid = self.next_pid.borrow_mut();
            *pid += 1;
            self.log.borrow_mut().push(format!("spawn pid {}", *pid));
            let cell = Rc::new(RefCell::new(None));
            self.children.borrow_mut().push(cell.clone());
            Ok(LiveChild {
                inner: FakeChild {
                    pid: *pid,
                    exit: None,
                    on_term: self.on_term,
                    log: self.log.clone(),
                },
                cell,
            })
        }
    }

    struct World {
        answers: Rc<RefCell<bool>>,
        log: Rc<RefCell<Vec<String>>>,
        children: ExitCells,
        events: Arc<Mutex<Vec<DaemonEvent>>>,
        sup: Supervisor<Fake>,
        now: Instant,
    }

    fn world(answers: bool, adopt_only: bool, spawn_ok: bool, on_term: OnTerm) -> World {
        let answers = Rc::new(RefCell::new(answers));
        let log = Rc::new(RefCell::new(Vec::new()));
        let children = Rc::new(RefCell::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let fake = Fake {
            answers: answers.clone(),
            spawn_ok,
            on_term,
            log: log.clone(),
            next_pid: RefCell::new(100),
            children: children.clone(),
        };
        let sup = Supervisor::new(fake, adopt_only, move |e| {
            sink.lock().unwrap().push(e.clone())
        });
        World {
            answers,
            log,
            children,
            events,
            sup,
            now: Instant::now(),
        }
    }

    impl World {
        fn tick(&mut self) -> Duration {
            self.sup.tick(self.now)
        }
        fn advance(&mut self, d: Duration) {
            self.now += d;
        }
        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
        fn events(&self) -> Vec<DaemonEvent> {
            self.events.lock().unwrap().clone()
        }
        fn child_exits(&self, i: usize, exit: Exit) {
            *self.children.borrow()[i].borrow_mut() = Some(exit);
        }
    }

    #[test]
    fn a_daemon_on_the_socket_is_adopted_and_nothing_is_spawned() {
        let mut w = world(true, false, true, OnTerm::Exits);
        assert_eq!(w.tick(), PROBE_EVERY);
        assert_eq!(w.events(), vec![DaemonEvent::Adopted]);
        assert_eq!(w.log(), vec!["probe"]);
        assert!(!w.sup.owns_daemon());
        // Quit leaves it alone: not the shell's.
        w.sup.stop();
        assert_eq!(w.log(), vec!["probe"]);
        assert_eq!(w.events().last(), Some(&DaemonEvent::Stopped));
    }

    #[test]
    fn with_nothing_on_the_socket_volumes_are_restored_then_the_daemon_is_spawned() {
        let mut w = world(false, false, true, OnTerm::Exits);
        assert_eq!(w.tick(), CHILD_POLL);
        assert_eq!(w.log(), vec!["probe", "restore-volumes", "spawn pid 101"]);
        assert_eq!(
            w.events(),
            vec![DaemonEvent::Started {
                pid: 101,
                restarts: 0
            }]
        );
        assert!(w.sup.owns_daemon());
    }

    #[test]
    fn adopt_only_waits_and_never_spawns() {
        let mut w = world(false, true, true, OnTerm::Exits);
        assert_eq!(w.tick(), PROBE_EVERY);
        assert_eq!(w.events(), vec![DaemonEvent::Waiting]);
        for _ in 0..50 {
            w.advance(PROBE_EVERY);
            w.tick();
        }
        assert!(w.log().iter().all(|l| l == "probe"), "{:?}", w.log());
        // The daemon dev.sh runs comes up: adopted.
        *w.answers.borrow_mut() = true;
        w.tick();
        assert_eq!(w.events().last(), Some(&DaemonEvent::Adopted));
    }

    #[test]
    fn an_exit_restarts_with_backoff_doubling_to_sixty_seconds() {
        let mut w = world(false, false, true, OnTerm::Exits);
        w.tick();
        let mut delays = Vec::new();
        for i in 0..6 {
            w.advance(Duration::from_secs(1));
            w.child_exits(i, Exit::Code(1));
            let d = w.tick(); // sees the exit, schedules the restart
            delays.push(d.as_secs());
            match w.events().last() {
                Some(DaemonEvent::Restarting {
                    in_secs,
                    restarts,
                    exit,
                }) => {
                    assert_eq!(*in_secs, d.as_secs());
                    assert_eq!(*restarts, i as u32 + 1);
                    assert_eq!(exit, "exit code 1");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(w.tick(), d, "still waiting"); // nothing happens inside the delay
            w.advance(d);
            assert_eq!(w.tick(), Duration::ZERO); // the delay is over: probe next
            assert_eq!(w.tick(), CHILD_POLL); // probed, restored, spawned
            assert_eq!(
                w.events().last(),
                Some(&DaemonEvent::Started {
                    pid: 102 + i as u32,
                    restarts: i as u32 + 1
                })
            );
        }
        assert_eq!(delays, vec![5, 10, 20, 40, 60, 60]);
        let spawns = w.log().iter().filter(|l| l.starts_with("spawn")).count();
        assert_eq!(spawns, 7);
        let restores = w.log().iter().filter(|l| *l == "restore-volumes").count();
        assert_eq!(restores, 7, "restore-volumes before every spawn");
    }

    #[test]
    fn a_long_healthy_run_resets_the_backoff() {
        let mut w = world(false, false, true, OnTerm::Exits);
        w.tick();
        w.child_exits(0, Exit::Code(1));
        assert_eq!(w.tick().as_secs(), 5);
        w.advance(Duration::from_secs(5));
        w.tick();
        w.tick(); // spawned the second
        w.child_exits(1, Exit::Code(1));
        assert_eq!(w.tick().as_secs(), 10);
        w.advance(Duration::from_secs(10));
        w.tick();
        w.tick(); // the third, which runs for a while
        w.advance(HEALTHY_RUN);
        w.child_exits(2, Exit::Signal(11));
        assert_eq!(w.tick().as_secs(), 5, "over after a healthy run");
    }

    #[test]
    fn a_daemon_that_took_the_socket_during_the_delay_is_adopted() {
        let mut w = world(false, false, true, OnTerm::Exits);
        w.tick();
        w.child_exits(0, Exit::Code(2));
        let d = w.tick();
        *w.answers.borrow_mut() = true; // make dev started one meanwhile
        w.advance(d);
        w.tick();
        w.tick();
        assert_eq!(w.events().last(), Some(&DaemonEvent::Adopted));
        assert_eq!(w.log().iter().filter(|l| l.starts_with("spawn")).count(), 1);
    }

    #[test]
    fn a_failed_spawn_is_tried_again_with_backoff() {
        let mut w = world(false, false, false, OnTerm::Exits);
        assert_eq!(w.tick().as_secs(), 5);
        assert_eq!(
            w.events(),
            vec![DaemonEvent::Failed {
                error: "no such file".into(),
                in_secs: 5
            }]
        );
        w.advance(Duration::from_secs(5));
        w.tick();
        assert_eq!(w.tick().as_secs(), 10);
    }

    #[test]
    fn an_adopted_daemon_gone_past_the_grace_is_replaced() {
        let mut w = world(true, false, true, OnTerm::Exits);
        w.tick();
        *w.answers.borrow_mut() = false;
        w.advance(Duration::from_secs(10));
        assert_eq!(w.tick(), PROBE_EVERY, "inside the grace: still theirs");
        assert!(!w.sup.owns_daemon());
        w.advance(ADOPT_GRACE);
        assert_eq!(w.tick(), Duration::ZERO);
        w.tick();
        assert!(w.sup.owns_daemon());
        assert_eq!(
            w.events().last(),
            Some(&DaemonEvent::Started {
                pid: 101,
                restarts: 0
            })
        );
    }

    #[test]
    fn an_adopted_daemon_gone_in_adopt_only_mode_is_waited_for() {
        let mut w = world(true, true, true, OnTerm::Exits);
        w.tick();
        *w.answers.borrow_mut() = false;
        w.advance(ADOPT_GRACE);
        w.tick();
        assert_eq!(w.events(), vec![DaemonEvent::Adopted, DaemonEvent::Waiting]);
        assert!(!w.log().iter().any(|l| l.starts_with("spawn")));
    }

    #[test]
    fn quit_sends_term_waits_and_restores_the_volumes() {
        let mut w = world(false, false, true, OnTerm::Exits);
        w.tick();
        w.sup.stop();
        assert_eq!(
            w.log(),
            vec![
                "probe",
                "restore-volumes",
                "spawn pid 101",
                "Term pid 101",
                "wait 20s",
                "restore-volumes"
            ]
        );
        assert_eq!(
            &w.events()[1..],
            &[DaemonEvent::Stopping, DaemonEvent::Stopped]
        );
        // After the stop nothing starts again.
        assert_eq!(w.tick(), Duration::from_secs(3600));
        assert!(!w.sup.owns_daemon());
    }

    #[test]
    fn a_daemon_that_ignores_term_is_killed_after_the_grace() {
        let mut w = world(false, false, true, OnTerm::Ignores);
        w.tick();
        w.sup.stop();
        assert_eq!(
            &w.log()[3..],
            &[
                "Term pid 101",
                "wait 20s",
                "Kill pid 101",
                "wait 2s",
                "restore-volumes"
            ]
        );
    }

    #[test]
    fn start_now_skips_the_rest_of_a_delay_and_is_refused_in_adopt_only_mode() {
        let mut w = world(false, false, true, OnTerm::Exits);
        w.tick();
        assert_eq!(w.sup.start_now(), Ok(()), "nothing to do while it runs");
        w.child_exits(0, Exit::Code(1));
        assert_eq!(w.tick().as_secs(), 5);
        w.advance(Duration::from_secs(1));
        assert_eq!(w.sup.start_now(), Ok(()));
        assert_eq!(w.tick(), CHILD_POLL, "spawned at once");
        assert_eq!(w.log().iter().filter(|l| l.starts_with("spawn")).count(), 2);

        let mut w = world(false, true, true, OnTerm::Exits);
        w.tick();
        let err = w.sup.start_now().unwrap_err();
        assert!(err.contains("make reload"), "{err}");
        assert!(!w.log().iter().any(|l| l.starts_with("spawn")));
    }

    #[test]
    fn the_backoff_sequence() {
        let mut b = Backoff::new();
        let s: Vec<u64> = (0..7).map(|_| b.next(Duration::ZERO).as_secs()).collect();
        assert_eq!(s, vec![5, 10, 20, 40, 60, 60, 60]);
        assert_eq!(b.next(HEALTHY_RUN).as_secs(), 5);
    }

    #[test]
    fn events_serialize_for_the_window() {
        let v = serde_json::to_value(DaemonEvent::Restarting {
            in_secs: 10,
            restarts: 2,
            exit: "exit code 1".into(),
        })
        .unwrap();
        assert_eq!(v["state"], "restarting");
        assert_eq!(v["in_secs"], 10);
        let v = serde_json::to_value(DaemonEvent::Started {
            pid: 7,
            restarts: 0,
        })
        .unwrap();
        assert_eq!(v["state"], "started");
        assert_eq!(
            serde_json::to_value(DaemonEvent::Adopted).unwrap()["state"],
            "adopted"
        );
    }
}
