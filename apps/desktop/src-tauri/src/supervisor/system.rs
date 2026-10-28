//! The [`Processes`](super::Processes) that touches the machine: the control socket, the
//! `ghostd` binary beside the shell, `systemd-cat`, and the log file.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ghost_api::models::Status;

use super::logfile::{self, LogFile};
use super::settings_env;
use super::{Child, Exit, Processes, Sig};

/// Where the daemon's output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Through `systemd-cat -t ghostd`, so `journalctl --user -t ghostd` keeps working.
    Journal,
    /// No journal on this machine: a file, turned over at 10 MB.
    File(PathBuf),
}

/// The daemon as the shell runs it.
pub struct System {
    pub socket: PathBuf,
    pub ghostd: PathBuf,
    pub output: Output,
    pub settings_env: PathBuf,
}

impl System {
    /// From the environment: `GHOST_API_SOCKET` or the runtime dir for the socket;
    /// `GHOST_DAEMON` or the `ghostd` beside this executable (the bundle's sidecar, or the
    /// workspace's `target/<profile>/ghostd`); the journal when `systemd-cat` can reach it.
    pub fn from_env() -> Self {
        Self {
            socket: ghost_api::server::default_socket_path(),
            ghostd: daemon_path(),
            output: if journal_available() {
                Output::Journal
            } else {
                Output::File(logfile::default_path())
            },
            settings_env: settings_env::settings_env_path(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = match &self.output {
            Output::Journal => {
                let mut c = Command::new("systemd-cat");
                c.args(["-t", "ghostd"]).arg(&self.ghostd);
                c
            }
            Output::File(_) => Command::new(&self.ghostd),
        };
        cmd.args(args);
        match settings_env::load(&self.settings_env) {
            Ok(s) => {
                for name in &s.left_out {
                    eprintln!(
                        "ghost: settings.env: {name} looks like a password and is not passed on; passwords live in Ghost's vault"
                    );
                }
                cmd.envs(s.values);
            }
            Err(e) => eprintln!("ghost: {e}"),
        }
        // The session's environment passes through (the display, the bus, the runtime dir),
        // but not a password someone exported in it: the daemon reads those from the vault.
        for (name, _) in std::env::vars_os() {
            if name
                .to_str()
                .is_some_and(settings_env::looks_like_a_password)
            {
                cmd.env_remove(&name);
            }
        }
        cmd.env("GHOST_LAUNCHER", "desktop")
            .env("GHOST_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::null());
        cmd
    }

    /// Stdout and stderr: the journal sets its own; a file gets one pipe for both, drained
    /// by a thread until the process ends.
    fn attach_output(&self, cmd: &mut Command) -> std::io::Result<Option<std::io::PipeReader>> {
        match &self.output {
            Output::Journal => {
                cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
                Ok(None)
            }
            Output::File(_) => {
                let (reader, writer) = std::io::pipe()?;
                cmd.stdout(writer.try_clone()?).stderr(writer);
                Ok(Some(reader))
            }
        }
    }

    fn drain_to_file(&self, reader: std::io::PipeReader) -> Option<std::thread::JoinHandle<()>> {
        let Output::File(path) = &self.output else {
            return None;
        };
        let path = path.clone();
        std::thread::Builder::new()
            .name("ghostd-log".into())
            .spawn(move || match LogFile::open(&path, logfile::ROTATE_AT) {
                Ok(mut log) => log.drain(reader),
                Err(e) => eprintln!("ghost: cannot open {}: {e}", path.display()),
            })
            .ok()
    }
}

/// `GHOST_DAEMON`, else `ghostd` beside this executable.
pub fn daemon_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GHOST_DAEMON").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("ghostd")))
        .unwrap_or_else(|| PathBuf::from("ghostd"))
}

/// `systemd-cat` is on the PATH and journald takes streams.
pub fn journal_available() -> bool {
    Path::new("/run/systemd/journal/stdout").exists() && on_path("systemd-cat")
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// `GET /status` on the socket, parsed as the daemon's own [`Status`]; a stale socket file
/// refuses the connection and a foreign listener does not parse.
pub fn probe(socket: &Path) -> Option<Status> {
    let mut s = UnixStream::connect(socket).ok()?;
    let t = Some(Duration::from_secs(2));
    s.set_read_timeout(t).ok()?;
    s.set_write_timeout(t).ok()?;
    s.write_all(b"GET /status HTTP/1.1\r\nHost: ghost\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return None;
    }
    serde_json::from_str(body).ok()
}

pub struct SystemChild {
    child: std::process::Child,
    log: Option<std::thread::JoinHandle<()>>,
}

impl SystemChild {
    fn exit_of(status: std::process::ExitStatus) -> Exit {
        use std::os::unix::process::ExitStatusExt;
        match status.code() {
            Some(c) => Exit::Code(c),
            None => Exit::Signal(status.signal().unwrap_or(0)),
        }
    }

    fn join_log(&mut self) {
        if let Some(h) = self.log.take() {
            let _ = h.join();
        }
    }
}

impl Child for SystemChild {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> Option<Exit> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.join_log();
                Some(Self::exit_of(status))
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("ghost: waiting for ghostd: {e}");
                Some(Exit::Code(-1))
            }
        }
    }

    fn wait_timeout(&mut self, timeout: Duration) -> Option<Exit> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(e) = self.try_wait() {
                return Some(e);
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn signal_group(&mut self, sig: Sig) {
        use rustix::process::{Pid, Signal, kill_process_group};
        let Some(pid) = Pid::from_raw(self.child.id() as i32) else {
            return;
        };
        let signal = match sig {
            Sig::Term => Signal::TERM,
            Sig::Kill => Signal::KILL,
        };
        if let Err(e) = kill_process_group(pid, signal) {
            eprintln!("ghost: {sig:?} to ghostd's group {}: {e}", self.child.id());
        }
    }
}

impl Processes for System {
    type Child = SystemChild;

    fn daemon_answers(&self) -> bool {
        probe(&self.socket).is_some()
    }

    fn restore_volumes(&self) {
        let mut cmd = self.command(&["restore-volumes"]);
        let reader = match self.attach_output(&mut cmd) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ghost: restore-volumes: {e}");
                return;
            }
        };
        let spawned = cmd.spawn();
        drop(cmd); // its copies of the pipe's write end, or the drain never sees the end
        match spawned {
            Ok(mut child) => {
                let log = reader.and_then(|r| self.drain_to_file(r));
                let until = Instant::now() + Duration::from_secs(10);
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(s)) => break Some(s),
                        Ok(None) if Instant::now() < until => {
                            std::thread::sleep(Duration::from_millis(100))
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break None;
                        }
                    }
                };
                if let Some(h) = log {
                    let _ = h.join();
                }
                match status {
                    Some(s) if s.success() => {}
                    Some(s) => eprintln!(
                        "ghost: ghostd restore-volumes is not available in this daemon ({}); nothing restored",
                        SystemChild::exit_of(s)
                    ),
                    None => eprintln!("ghost: ghostd restore-volumes did not finish in 10 s"),
                }
            }
            Err(e) => eprintln!(
                "ghost: cannot run {} restore-volumes: {e}",
                self.ghostd.display()
            ),
        }
    }

    fn spawn(&self) -> Result<SystemChild, String> {
        if !self.ghostd.is_file() {
            return Err(format!("no daemon at {}", self.ghostd.display()));
        }
        let mut cmd = self.command(&["run"]);
        // Its own process group: a signal to the group reaches the apps it started too.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let reader = self.attach_output(&mut cmd).map_err(|e| e.to_string())?;
        let spawned = cmd.spawn();
        drop(cmd); // see restore_volumes
        let child = spawned.map_err(|e| format!("starting {}: {e}", self.ghostd.display()))?;
        let log = reader.and_then(|r| self.drain_to_file(r));
        match &self.output {
            Output::Journal => eprintln!(
                "ghost: started {} run (pid {}); journalctl --user -t ghostd",
                self.ghostd.display(),
                child.id()
            ),
            Output::File(p) => eprintln!(
                "ghost: started {} run (pid {}); log in {}",
                self.ghostd.display(),
                child.id(),
                p.display()
            ),
        }
        Ok(SystemChild { child, log })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A fake daemon on a socket: one request, one canned answer.
    fn serve_once(socket: &Path, body: &'static str, ok: bool) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let status = if ok { "200 OK" } else { "404 Not Found" };
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        })
    }

    const STATUS: &str = r#"{"phase":"starting","session_id":null,"models_ready":false,"uptime_secs":1.5,"version":"2026.10.0","build":{"kind":"dev"},"api_version":"1","wake_mode":"transcript","apps":[],"pending":null,"conversation":null,"session_origin":null,"ending":false,"waiting":0}"#;

    #[test]
    fn probe_adopts_a_daemon_that_answers_status() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("api.sock");
        let t = serve_once(&sock, STATUS, true);
        let st = probe(&sock).expect("a status");
        assert_eq!(st.version, "2026.10.0");
        t.join().unwrap();
    }

    #[test]
    fn probe_refuses_a_stale_socket_and_a_foreign_listener() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("api.sock");
        assert!(probe(&sock).is_none(), "no file");
        drop(UnixListener::bind(&sock).unwrap());
        assert!(probe(&sock).is_none(), "a stale file nobody answers on");
        let _ = std::fs::remove_file(&sock);
        let t = serve_once(&sock, "<html>no</html>", true);
        assert!(probe(&sock).is_none(), "not a status");
        t.join().unwrap();
        let _ = std::fs::remove_file(&sock);
        let t = serve_once(&sock, STATUS, false);
        assert!(probe(&sock).is_none(), "not a 200");
        t.join().unwrap();
    }

    /// A stand-in daemon: a shell script that traps TERM the way `ghostd run` does
    /// (exits) or ignores it, with `GHOST_PARENT_PID` and the settings in its environment.
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("ghostd");
        std::fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// The stand-in has set its traps once its first line is in the log.
    fn wait_for_log(dir: &Path, needle: &str) -> String {
        let p = dir.join("ghostd.log");
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            if text.contains(needle) {
                return text;
            }
            assert!(Instant::now() < until, "no {needle:?} in the log: {text}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn system(dir: &Path, ghostd: PathBuf) -> System {
        System {
            socket: dir.join("api.sock"),
            ghostd,
            output: Output::File(dir.join("ghostd.log")),
            settings_env: dir.join("settings.env"),
        }
    }

    #[test]
    fn spawns_in_its_own_group_with_the_settings_and_the_launcher_and_logs_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.env"),
            "GHOST_INPUT_DEVICE=ghost.mic\nANTHROPIC_AUTH_TOKEN=sk-secret-value\n",
        )
        .unwrap();
        let ghostd = script(
            dir.path(),
            r#"echo "args=$* launcher=$GHOST_LAUNCHER parent=$GHOST_PARENT_PID device=$GHOST_INPUT_DEVICE token=${ANTHROPIC_AUTH_TOKEN:-unset} pgid=$(ps -o pgid= -p $$ | tr -d ' ')"
echo "to stderr" >&2
trap 'exit 0' TERM
while :; do sleep 0.1; done"#,
        );
        let sys = system(dir.path(), ghostd);
        // Exported in the shell's own environment: not passed on either.
        // SAFETY: this test's process, before any thread reads the environment.
        unsafe { std::env::set_var("ANTHROPIC_AUTH_TOKEN", "sk-exported-value") };
        let mut child = sys.spawn().unwrap();
        let pid = child.pid();
        wait_for_log(dir.path(), "to stderr");
        assert!(child.try_wait().is_none(), "still running");
        child.signal_group(Sig::Term);
        let exit = child
            .wait_timeout(Duration::from_secs(5))
            .expect("exits on TERM");
        assert_eq!(exit, Exit::Code(0));
        let log = std::fs::read_to_string(dir.path().join("ghostd.log")).unwrap();
        assert!(log.contains("args=run "), "{log}");
        assert!(log.contains("launcher=desktop"), "{log}");
        assert!(
            log.contains(&format!("parent={}", std::process::id())),
            "{log}"
        );
        assert!(log.contains("device=ghost.mic"), "{log}");
        assert!(
            log.contains("token=unset"),
            "a password is not passed on: {log}"
        );
        assert!(
            log.contains(&format!("pgid={pid}")),
            "its own process group: {log}"
        );
        assert!(log.contains("to stderr"), "{log}");
    }

    #[test]
    fn a_child_that_ignores_term_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let ghostd = script(
            dir.path(),
            "trap '' TERM\necho ready\nwhile :; do sleep 0.1; done",
        );
        let sys = system(dir.path(), ghostd);
        let mut child = sys.spawn().unwrap();
        wait_for_log(dir.path(), "ready");
        child.signal_group(Sig::Term);
        assert!(child.wait_timeout(Duration::from_millis(500)).is_none());
        child.signal_group(Sig::Kill);
        assert_eq!(
            child.wait_timeout(Duration::from_secs(5)),
            Some(Exit::Signal(9))
        );
    }

    #[test]
    fn restore_volumes_tolerates_a_daemon_without_the_subcommand() {
        let dir = tempfile::tempdir().unwrap();
        let ghostd = script(dir.path(), r#"echo "unknown command $1" >&2; exit 2"#);
        let sys = system(dir.path(), ghostd);
        sys.restore_volumes(); // logs, returns
        let log = std::fs::read_to_string(dir.path().join("ghostd.log")).unwrap();
        assert!(log.contains("unknown command restore-volumes"), "{log}");
    }

    #[test]
    fn a_missing_daemon_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let sys = system(dir.path(), dir.path().join("nope"));
        let err = sys.spawn().err().expect("an error");
        assert!(err.contains("no daemon at"), "{err}");
    }
}
