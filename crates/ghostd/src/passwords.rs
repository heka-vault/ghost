//! The daemon's side of Ghost's vault (ADR 0036): opening it at start, the plaintext files
//! a machine still has before its import, what the waiting list says, and
//! `ghostd passwords`.
//!
//! A process that needs passwords (`ghostd run`, `check`, `perf`, `push`) opens the
//! vault itself; nothing hands it a secret through its environment. The values go into
//! the environment Ghost's settings read (`SharedEnviron`, in memory, never the process
//! environment) and into the redactor; `child_env` keeps them from anything it starts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ghost_core::apps::{APPS_FILE, load_apps};
use ghost_core::connection::ModelConnection;
use ghost_core::envfile::EnvFile;
use ghost_core::folder::{Declared, GhostFolder};
use ghost_core::password_import::{
    ImportOptions, ImportReport, SourceKind, fetch_heka, import_passwords, legacy_values, sources,
};
use ghost_core::passwords::{
    GHOST_USES, Keyring, Passwords, SecretServiceKeyring, SharedEnviron, State, VAULT_FILE,
    is_password_name,
};
use ghost_core::redact::{also_redact, redact_text};
use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::watch;
use tracing::{debug, info, warn};
use tracing_subscriber::fmt::MakeWriter;

use crate::config::GhostdSettings;

/// A locked keyring is tried again this often.
pub const VAULT_RETRY_SECS: f64 = 10.0;
pub const VAULT_RETRY: Duration = Duration::from_secs(10);

pub fn vault_path(cfg: &GhostdSettings) -> PathBuf {
    cfg.data_dir.join(VAULT_FILE)
}

/// The vault, opened if it can be, its values in `environ`, the environment Ghost's
/// settings read. A machine that hasn't imported yet still has `~/.config/ghost/env` and
/// `apps.env`: their values are read too, under the vault's (which win), and kept out of
/// logs and child processes like the vault's, until the import removes the files.
pub fn open_passwords(
    cfg: &GhostdSettings,
    keyring: Arc<dyn Keyring>,
    environ: SharedEnviron,
) -> Passwords {
    let legacy = match legacy_values(&cfg.config_dir, &EnvFile) {
        Ok(values) => values,
        Err(e) => {
            warn!(
                "passwords: the plaintext files could not be read ({})",
                e.kind()
            );
            BTreeMap::new()
        }
    };
    if !legacy.is_empty() {
        {
            let mut env = environ.lock().unwrap_or_else(|e| e.into_inner());
            for (k, v) in &legacy {
                env.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        also_redact(
            "legacy",
            legacy
                .iter()
                .filter(|(k, _)| is_password_name(k, &[]))
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }
    let vault = Passwords::new(vault_path(cfg), keyring).with_environ(environ);
    vault.open();
    vault
}

/// Ghost's own items about its passwords, for the waiting list (ADR 0029).
pub fn problems(vault: Option<&Passwords>, cfg: &GhostdSettings) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(vault) = vault {
        if vault.state() == State::Locked {
            out.push((
                "passwords:locked".to_string(),
                "Unlock your keyring so Ghost can read its passwords".to_string(),
            ));
        } else if !vault.is_open() {
            out.push((
                "passwords:vault".to_string(),
                format!(
                    "Ghost can't open its passwords: {}",
                    vault.problem().unwrap_or_default()
                ),
            ));
        }
    }
    let plain = sources(&cfg.config_dir, None, &EnvFile)
        .map(|s| s.iter().any(|s| s.kind == SourceKind::File))
        .unwrap_or(false);
    if plain {
        out.push((
            "passwords:import".to_string(),
            "Some passwords are still in plain files. Import them on Settings › Passwords"
                .to_string(),
        ));
    }
    out
}

/// Ghost's own passwords a session fills in, for "last used": the model connection's
/// (ADR 0037), whichever provider it names, and the ElevenLabs voice's.
pub fn in_use(connection: Option<&ModelConnection>, tts_engine: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(conn) = connection
        && !conn.password.is_empty()
    {
        names.push(conn.password.clone());
    }
    if tts_engine == "elevenlabs" {
        names.push("ELEVENLABS_API_KEY".to_string());
    }
    names
}

/// An environment for a process Ghost starts: this process's, less every password.
pub fn child_env() -> Vec<(String, String)> {
    ghost_core::redact::child_env(std::env::vars())
}

// --- the log -------------------------------------------------------------------------------

/// A log writer that lets no line carry a password (ADR 0036): what the subscriber
/// formats, an error chain included, is redacted before it is written. Wraps any
/// `MakeWriter` (`std::io::stderr`, a test buffer).
#[derive(Debug, Clone)]
pub struct Redacting<M>(pub M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Redacting<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter(self.0.make_writer())
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        RedactingWriter(self.0.make_writer_for(meta))
    }
}

pub struct RedactingWriter<W>(W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let clean = redact_text(&text);
        self.0.write_all(clean.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

// --- a vault that opens late -----------------------------------------------------------------

/// What follows the vault opening after a locked start (`Daemon.passwords_opened`): Ghost's
/// own settings read again, Jev joining, the apps that waited connecting. Once: the unlock
/// route and the retry loop may both see the vault open.
pub struct Opened {
    done: AtomicBool,
    follow: Box<dyn Fn() + Send + Sync>,
}

impl Opened {
    pub fn new(follow: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            done: AtomicBool::new(false),
            follow: Box::new(follow),
        }
    }

    /// The vault is open: run what follows, the first time only. True when it ran.
    pub fn opened(&self) -> bool {
        if self.done.swap(true, Ordering::SeqCst) {
            return false;
        }
        info!("passwords: the vault is open");
        (self.follow)();
        true
    }

    pub fn followed(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

/// The keyring was locked at start: try again every `retry` until the vault opens or
/// `stop` says the daemon is stopping. "Unlock your keyring" is on the waiting list
/// meanwhile.
pub async fn wait_for_vault(
    vault: Arc<Passwords>,
    retry: Duration,
    mut stop: watch::Receiver<bool>,
    opened: Arc<Opened>,
) {
    while !vault.is_open() {
        tokio::select! {
            _ = tokio::time::sleep(retry) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
                continue;
            }
        }
        let v = Arc::clone(&vault);
        let now_open = tokio::task::spawn_blocking(move || !v.is_open() && v.open())
            .await
            .unwrap_or(false);
        if now_open {
            opened.opened();
        }
    }
}

// --- ghostd check ----------------------------------------------------------------------------

pub const KEYRING_WAITS: [State; 2] = [State::Locked, State::Unavailable];

/// `ghostd check` for the vault (ADR 0036). Passwords still in plain files are said,
/// not a problem: a build is deployed before its import. A locked or unreachable keyring
/// is said too, not a problem: the daemon waits for it (`keyring_waits`), and a deploy
/// over SSH has no keyring to unlock. A vault the keyring's key doesn't open is one.
pub fn check_passwords(
    vault: &Passwords,
    cfg: &GhostdSettings,
    out: &mut dyn Write,
) -> Vec<String> {
    let _ = writeln!(
        out,
        "passwords: {} in {} ({})",
        vault.len(),
        vault.path().display(),
        vault.state()
    );
    let mut problems_out = Vec::new();
    if KEYRING_WAITS.contains(&vault.state()) {
        let _ = writeln!(
            out,
            "passwords: warning: {}. Ghost starts anyway and waits for it; what needs a password is checked when it opens",
            vault.problem().unwrap_or_default()
        );
    } else if !vault.is_open() {
        problems_out.push(format!(
            "passwords: {}",
            vault.problem().unwrap_or_default()
        ));
    }
    for (key, title) in problems(Some(vault), cfg) {
        if key == "passwords:import" {
            let _ = writeln!(
                out,
                "passwords: still in plain files; run `ghostd passwords import` ({title})"
            );
        }
    }
    problems_out
}

/// `ghostd check`'s problems, less those that only say a password is missing while
/// the keyring is locked or unreachable: a model or voice key not set, an app that needs
/// one not connected. Those are warnings; the daemon fills them in when the vault opens.
/// `passwords`: names that are passwords whatever they look like (the model's own).
pub fn keyring_waits(
    vault: &Passwords,
    problems: &[String],
    apps_with_passwords: &BTreeSet<String>,
    passwords: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    if !KEYRING_WAITS.contains(&vault.state()) {
        return (problems.to_vec(), Vec::new());
    }
    let app_re = Regex::new(r"^\s*app (\S+) \(").unwrap();
    let not_set = Regex::new(r"\b([A-Z][A-Z0-9_]*) is not set\b").unwrap();
    let referenced: Vec<&str> = passwords.iter().map(String::as_str).collect();
    let mut real = Vec::new();
    let mut waits = Vec::new();
    for p in problems {
        let app = app_re
            .captures(p)
            .map(|c| c[1].to_string())
            .is_some_and(|a| apps_with_passwords.contains(&a));
        let missing = not_set
            .captures(p)
            .is_some_and(|c| is_password_name(&c[1], &referenced));
        if app || missing {
            waits.push(p.clone());
        } else {
            real.push(p.clone());
        }
    }
    (real, waits)
}

// --- ghostd passwords ------------------------------------------------------------------------

pub const USAGE: &str = "usage: ghostd passwords [list | set NAME | delete NAME | import | unlock]

  list           what is in Ghost's vault (names only; a value is never shown)
  set NAME       save one; the value is read from the terminal without echo, or stdin
  delete NAME    forget one
  import         once: heka-vault and ~/.config/ghost/env and apps.env into the vault,
                 then the plaintext files are removed
  unlock         ask your keyring to unlock, then open the vault

With Ghost running, these go through it; otherwise they edit the vault directly.";

/// The terminal the command runs in.
pub struct Cli<'a> {
    pub stdin: &'a mut dyn BufRead,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    /// stdin is a terminal: the value is asked for without echo.
    pub tty: bool,
}

/// `ghostd passwords …`: the exit code. `keyring` is the operator's own
/// (`SecretServiceKeyring`) outside tests.
pub fn passwords_cli(
    args: &[String],
    cfg: &GhostdSettings,
    keyring: Arc<dyn Keyring>,
    io: &mut Cli<'_>,
) -> i32 {
    let cmd = args.first().map_or("list", String::as_str);
    let known = ["list", "set", "delete", "import", "unlock"].contains(&cmd);
    if !known || (matches!(cmd, "set" | "delete") && args.len() != 2) {
        let _ = writeln!(io.stderr, "{USAGE}");
        return 2;
    }
    let value = (cmd == "set").then(|| read_value(&args[1], io));
    if cmd == "set" && value.as_deref().is_none_or(str::is_empty) {
        let _ = writeln!(io.stderr, "no value given; nothing saved");
        return 2;
    }
    let sock = cfg
        .api_socket
        .clone()
        .unwrap_or_else(ghost_api::server::default_socket_path);
    if sock.exists() {
        return through_daemon(&sock, cmd, args, value.as_deref(), io);
    }
    direct(cfg, keyring, cmd, args, value.as_deref(), io)
}

/// The operator's own keyring, for the command line.
pub fn operator_keyring() -> Arc<dyn Keyring> {
    Arc::new(SecretServiceKeyring)
}

fn read_value(name: &str, io: &mut Cli<'_>) -> String {
    if io.tty {
        return read_without_echo(name).unwrap_or_default();
    }
    let mut line = String::new();
    let _ = io.stdin.read_line(&mut line);
    line.trim().to_string()
}

/// `getpass`: the prompt on the terminal, the value typed without echo.
fn read_without_echo(name: &str) -> io::Result<String> {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};

    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    write!(tty, "{name}: ")?;
    tty.flush()?;
    let saved = tcgetattr(&tty)?;
    let mut quiet = saved.clone();
    quiet.local_modes.remove(LocalModes::ECHO);
    tcsetattr(&tty, OptionalActions::Flush, &quiet)?;
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    let read = loop {
        match tty.read(&mut byte) {
            Ok(0) => break Ok(()),
            Ok(_) if byte[0] == b'\n' => break Ok(()),
            Ok(_) => raw.push(byte[0]),
            Err(e) => break Err(e),
        }
    };
    let _ = tcsetattr(&tty, OptionalActions::Flush, &saved);
    let _ = writeln!(tty);
    read?;
    Ok(String::from_utf8_lossy(&raw).trim().to_string())
}

fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

fn print_list(view: &Value, out: &mut dyn Write) {
    let vault = &view["vault"];
    let why = match vault["problem"].as_str() {
        Some(p) if !p.is_empty() => format!(": {p}"),
        _ => String::new(),
    };
    let _ = writeln!(
        out,
        "vault: {} ({}{why})",
        vault["path"].as_str().unwrap_or(""),
        vault["state"].as_str().unwrap_or("")
    );
    for p in view["passwords"].as_array().into_iter().flatten() {
        let mark = match p["set"] {
            Value::Null => "unknown",
            Value::Bool(true) => "set",
            _ => "not set",
        };
        let uses: Vec<&str> = p["used_by"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let uses = if uses.is_empty() {
            "nothing names it".to_string()
        } else {
            uses.join(", ")
        };
        let last = match p["last_used"].as_str() {
            Some(l) if !l.is_empty() => {
                format!(", last used {}", l.chars().take(16).collect::<String>())
            }
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "  {}: {mark} ({uses}{last})",
            p["name"].as_str().unwrap_or("")
        );
    }
    for s in view["importable"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "  to import: {}", s["path"].as_str().unwrap_or(""));
    }
}

fn print_report(out: &Value, w: &mut dyn Write) {
    fn names(v: &Value) -> Vec<&str> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }
    let imported = names(&out["imported"]);
    let _ = writeln!(
        w,
        "imported: {}",
        if imported.is_empty() {
            "none".to_string()
        } else {
            imported.join(", ")
        }
    );
    let kept = names(&out["kept"]);
    if !kept.is_empty() {
        let _ = writeln!(w, "already in the vault, kept: {}", kept.join(", "));
    }
    let settings = names(&out["settings"]);
    if !settings.is_empty() {
        let _ = writeln!(w, "moved to settings.env: {}", settings.join(", "));
    }
    for path in names(&out["removed"]) {
        let _ = writeln!(w, "removed {path}");
    }
    for note in names(&out["notes"]) {
        let _ = writeln!(w, "{note}");
    }
    if out["restart_required"].as_bool() == Some(true) {
        let _ = writeln!(
            w,
            "Restart Ghost so it uses them: make service-on, or make reload under make dev."
        );
    }
}

fn report_json(report: &ImportReport, restart_required: bool) -> Value {
    json!({
        "imported": report.imported,
        "kept": report.kept,
        "settings": report.settings,
        "removed": report.removed.iter().map(|p| tilde(p)).collect::<Vec<_>>(),
        "notes": report.notes,
        "restart_required": restart_required,
    })
}

/// One request to the running daemon over its socket: the status and the body.
fn socket_request(
    sock: &Path,
    method: &str,
    route: &str,
    body: Option<&Value>,
) -> io::Result<(u16, String)> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(sock)?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let payload = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let mut head = format!("{method} {route} HTTP/1.1\r\nHost: ghost\r\nConnection: close\r\n");
    if let Some(p) = &payload {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            p.len()
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(p) = &payload {
        stream.write_all(p)?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("not an HTTP response"))?;
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().replace(' ', "") == "transfer-encoding:chunked");
    Ok((
        status,
        if chunked {
            dechunk(body)
        } else {
            body.to_string()
        },
    ))
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16) else {
            break;
        };
        if n == 0 || after.len() < n {
            break;
        }
        out.push_str(&after[..n]);
        rest = after[n..].strip_prefix("\r\n").unwrap_or("");
    }
    out
}

fn through_daemon(
    sock: &Path,
    cmd: &str,
    args: &[String],
    value: Option<&str>,
    io: &mut Cli<'_>,
) -> i32 {
    let (status, text) = match cmd {
        "list" => socket_request(sock, "GET", "/passwords", None),
        "set" => socket_request(
            sock,
            "PUT",
            &format!("/passwords/{}", args[1]),
            Some(&json!({ "value": value.unwrap_or("") })),
        ),
        "delete" => socket_request(sock, "DELETE", &format!("/passwords/{}", args[1]), None),
        "unlock" => socket_request(sock, "POST", "/passwords/unlock", None),
        _ => socket_request(sock, "POST", "/passwords/import", None),
    }
    .unwrap_or_else(|e| (0, e.to_string()));
    if status == 0 {
        let _ = writeln!(
            io.stderr,
            "Ghost didn't answer on {}: {text}",
            sock.display()
        );
        return 1;
    }
    let out: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status >= 400 {
        let detail = out["detail"].as_str().map_or(text.as_str(), |d| d);
        let _ = writeln!(io.stderr, "{detail}");
        return 1;
    }
    match cmd {
        "list" => print_list(&out, io.stdout),
        "import" => print_report(&out, io.stdout),
        "unlock" => {
            let state = out["vault"]["state"].as_str().unwrap_or("");
            let _ = writeln!(io.stdout, "vault: {state}");
            return i32::from(state != "open");
        }
        _ => {
            let _ = writeln!(
                io.stdout,
                "{}: {}",
                args[1],
                if cmd == "set" { "saved" } else { "forgotten" }
            );
            if out["restart_required"].as_bool() == Some(true) {
                let _ = writeln!(io.stdout, "Ghost uses it from its next start.");
            }
        }
    }
    0
}

/// Ghost isn't running: the vault file itself. The daemon reads it at its next start.
fn direct(
    cfg: &GhostdSettings,
    keyring: Arc<dyn Keyring>,
    cmd: &str,
    args: &[String],
    value: Option<&str>,
    io: &mut Cli<'_>,
) -> i32 {
    let vault = Passwords::new(vault_path(cfg), keyring);
    let opened = if cmd == "unlock" {
        vault.unlock()
    } else {
        vault.open()
    };
    if cmd == "unlock" {
        let _ = writeln!(io.stdout, "vault: {}", vault.state());
        return i32::from(!opened);
    }
    if cmd == "list" {
        let rows: Vec<Value> = vault
            .info()
            .iter()
            .map(|m| {
                let used_by: Vec<&str> = GHOST_USES
                    .iter()
                    .filter(|(n, _)| *n == m.name)
                    .map(|(_, u)| *u)
                    .collect();
                json!({
                    "name": m.name,
                    "set": true,
                    "used_by": used_by,
                    "last_used": m.last_used.map(|t| ghost_core::time::iso(&t)),
                })
            })
            .collect();
        let importable: Vec<Value> = sources(&cfg.config_dir, Some(&cfg.vault_env_path), &EnvFile)
            .unwrap_or_default()
            .iter()
            .map(|s| json!({ "path": tilde(&s.path) }))
            .collect();
        let view = json!({
            "vault": {
                "path": tilde(vault.path()),
                "state": vault.state().as_str(),
                "problem": vault.problem(),
            },
            "passwords": rows,
            "importable": importable,
        });
        print_list(&view, io.stdout);
        return 0;
    }
    if !opened {
        let _ = writeln!(
            io.stderr,
            "Ghost's vault isn't open: {}",
            vault.problem().unwrap_or_default()
        );
        return 1;
    }
    let result: Result<(), ghost_core::passwords::PasswordsError> = match cmd {
        "set" => vault.set(&args[1], value.unwrap_or("")).map(|()| {
            let _ = writeln!(io.stdout, "{}: saved", args[1]);
        }),
        "delete" => vault.delete(&[args[1].as_str()]).map(|gone| {
            let _ = writeln!(
                io.stdout,
                "{}: {}",
                args[1],
                if gone.is_empty() {
                    "not in the vault"
                } else {
                    "forgotten"
                }
            );
        }),
        _ => {
            let mut names: BTreeSet<String> =
                GHOST_USES.iter().map(|(n, _)| n.to_string()).collect();
            for spec in load_apps(&cfg.config_dir.join(APPS_FILE)).apps {
                names.extend(spec.secrets());
            }
            let folder = GhostFolder::<Declared>::new(&cfg.config_dir);
            names.extend(folder.hooks.values().map(|h| h.secret_name()));
            import_passwords(
                &vault,
                &cfg.config_dir,
                &EnvFile,
                ImportOptions {
                    heka_env: Some(&cfg.vault_env_path),
                    referenced: names,
                    fetch: &fetch_heka,
                },
            )
            .map(|report| print_report(&report_json(&report, false), io.stdout))
        }
    };
    if let Err(e) = result {
        let _ = writeln!(io.stderr, "{e}");
        return 1;
    }
    debug!("passwords: {cmd} done");
    0
}

#[cfg(test)]
mod tests {
    //! The daemon's side of the vault (ADR 0036): opened at start with plaintext files still
    //! read until the import, the waiting list's items, the log redacted, and
    //! `ghostd passwords`. A fake keyring, temp folders and a map for the environment: never
    //! the operator's own.

    use std::collections::HashMap;
    use std::sync::Mutex;

    use ghost_core::passwords::MemoryKeyring;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    const TOKEN: &str = "sk-ant-api03-the-vaults-own-value";
    const OLD: &str = "sk-ant-api03-the-plaintext-copy";

    fn cfg_for(tmp: &Path) -> GhostdSettings {
        let config = tmp.join("config");
        std::fs::create_dir_all(&config).unwrap();
        GhostdSettings {
            data_dir: tmp.join("data"),
            config_dir: config,
            vault_env_path: tmp.join("no-heka"),
            api_socket: Some(tmp.join("no-daemon.sock")),
            ..Default::default()
        }
    }

    fn environ(pairs: &[(&str, &str)]) -> SharedEnviron {
        Arc::new(Mutex::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ))
    }

    #[test]
    fn open_reads_the_vault_over_plaintext_files_until_the_import() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        std::fs::write(
            cfg.config_dir.join("env"),
            format!("ANTHROPIC_AUTH_TOKEN={OLD}\nANTHROPIC_BASE_URL=http://proxy\nMCP_PROXY_KEY=proxy-0123456789\n"),
        )
        .unwrap();
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(cfg.data_dir.join("passwords.vault"), ring.clone());
        assert!(seeded.open());
        seeded.set("ANTHROPIC_AUTH_TOKEN", TOKEN).unwrap();

        let env = environ(&[("PATH", "/bin")]);
        let vault = open_passwords(&cfg, ring, env.clone());
        assert!(vault.is_open());
        let snapshot: HashMap<String, String> = env.lock().unwrap().clone();
        assert_eq!(snapshot["ANTHROPIC_AUTH_TOKEN"], TOKEN); // the vault's wins
        assert_eq!(snapshot["MCP_PROXY_KEY"], "proxy-0123456789"); // not imported yet: still read
        assert_eq!(snapshot["ANTHROPIC_BASE_URL"], "http://proxy");
        // Both are passwords to every check: redacted, and never handed to a child process.
        assert_eq!(
            redact_text(&format!("a {OLD} b proxy-0123456789")),
            "a [password: ANTHROPIC_AUTH_TOKEN] b [password: MCP_PROXY_KEY]"
        );
        let kept: BTreeSet<String> = ghost_core::redact::child_env(snapshot.clone())
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            kept,
            ["PATH", "ANTHROPIC_BASE_URL"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        let keys: Vec<String> = problems(Some(&vault), &cfg)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["passwords:import"]);
    }

    #[test]
    fn a_locked_keyring_is_on_the_waiting_list() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(cfg.data_dir.join("passwords.vault"), ring.clone());
        assert!(seeded.open());
        seeded.set("HF_TOKEN", "hf_0123456789abcdef").unwrap();
        ring.set_locked(true);
        let vault = open_passwords(&cfg, ring, environ(&[]));
        assert_eq!(vault.state(), State::Locked);
        assert_eq!(
            problems(Some(&vault), &cfg),
            vec![(
                "passwords:locked".to_string(),
                "Unlock your keyring so Ghost can read its passwords".to_string()
            )]
        );
        assert_eq!(problems(None, &cfg), vec![]);
    }

    #[tokio::test]
    async fn the_daemon_follows_a_vault_that_opens_late() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("passwords.vault");
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(&path, ring.clone());
        assert!(seeded.open());
        seeded
            .set("OPENROUTER_API_KEY", "sk-or-0123456789abcdef")
            .unwrap();
        ring.set_locked(true);
        let env = environ(&[]);
        let vault = Arc::new(Passwords::new(&path, ring.clone()).with_environ(env.clone()));
        assert!(!vault.open());

        let followed = Arc::new(Mutex::new(0));
        let opened = {
            let followed = followed.clone();
            Arc::new(Opened::new(move || *followed.lock().unwrap() += 1))
        };
        let (_stop, rx) = watch::channel(false);
        ring.set_locked(false);
        tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_vault(vault.clone(), Duration::from_millis(10), rx, opened.clone()),
        )
        .await
        .expect("the vault opens once the keyring does");
        assert!(vault.is_open());
        assert!(env.lock().unwrap()["OPENROUTER_API_KEY"].starts_with("sk-or-"));
        assert_eq!(*followed.lock().unwrap(), 1);
        assert!(opened.followed());
    }

    #[tokio::test]
    async fn the_wait_ends_when_the_daemon_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("passwords.vault");
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(&path, ring.clone());
        assert!(seeded.open());
        seeded.set("HF_TOKEN", "hf_0123456789abcdef").unwrap();
        ring.set_locked(true);
        let vault = Arc::new(Passwords::new(&path, ring.clone()));
        assert!(!vault.open());
        let opened = Arc::new(Opened::new(|| {}));
        let (stop, rx) = watch::channel(false);
        let waiting = tokio::spawn(wait_for_vault(
            vault.clone(),
            Duration::from_secs(60),
            rx,
            opened.clone(),
        ));
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("stops at once")
            .unwrap();
        assert!(!vault.is_open());
        assert!(!opened.followed());
    }

    /// A log sink for one test: what the subscriber wrote.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Sink;

        fn make_writer(&'a self) -> Sink {
            self.clone()
        }
    }

    impl Sink {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    fn logged(sink: &Sink, f: impl FnOnce()) {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Redacting(sink.clone()))
            .with_ansi(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, f);
    }

    #[test]
    fn no_log_line_carries_a_password() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Passwords::new(
            tmp.path().join("passwords.vault"),
            Arc::new(MemoryKeyring::new()),
        );
        assert!(vault.open());
        vault.set("ANTHROPIC_AUTH_TOKEN", TOKEN).unwrap();
        let sink = Sink::default();
        logged(&sink, || warn!("the endpoint said: bad token {TOKEN}"));
        let out = sink.text();
        assert!(!out.contains(TOKEN));
        assert!(
            out.contains("bad token [password: ANTHROPIC_AUTH_TOKEN]"),
            "{out}"
        );
    }

    #[test]
    fn no_exception_in_the_log_carries_a_password() {
        // An error chain logged whole (`{:#}`), the cause included, is redacted as the
        // message is; an ordinary failure is written as it is.
        let tmp = tempfile::tempdir().unwrap();
        let vault = Passwords::new(
            tmp.path().join("passwords.vault"),
            Arc::new(MemoryKeyring::new()),
        );
        assert!(vault.open());
        vault.set("ANTHROPIC_AUTH_TOKEN", TOKEN).unwrap();
        let sink = Sink::default();
        logged(&sink, || {
            let cause = anyhow::anyhow!("KeyError: no such key {TOKEN}");
            let failed = cause.context(format!("RuntimeError: the endpoint refused {TOKEN}"));
            tracing::error!("the model call failed: {failed:#}");
            let plain = anyhow::anyhow!("ValueError: nothing secret here");
            tracing::error!("an ordinary failure: {plain:#}");
        });
        let text = sink.text();
        assert!(!text.contains(TOKEN));
        assert!(
            text.contains("RuntimeError: the endpoint refused [password: ANTHROPIC_AUTH_TOKEN]"),
            "{text}"
        );
        assert!(text.contains("no such key [password: ANTHROPIC_AUTH_TOKEN]")); // the cause too
        assert!(text.contains("ValueError: nothing secret here"));
    }

    #[test]
    fn the_vault_opening_is_followed_once() {
        // The unlock route and the retry loop may both see the vault open: what follows runs once.
        let count = Arc::new(Mutex::new(0));
        let opened = {
            let count = count.clone();
            Opened::new(move || *count.lock().unwrap() += 1)
        };
        assert!(opened.opened());
        assert!(!opened.opened());
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[test]
    fn check_waits_for_a_locked_keyring() {
        // `make deploy` over SSH: a locked keyring is a warning, and so is what only needs a
        // password from it. A vault the key doesn't open is still a problem.
        let tmp = tempfile::tempdir().unwrap();
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let seeded = Passwords::new(tmp.path().join("passwords.vault"), ring.clone());
        assert!(seeded.open());
        seeded.set("HF_TOKEN", "hf_0123456789abcdef").unwrap();
        ring.set_locked(true);
        let vault = Passwords::new(tmp.path().join("passwords.vault"), ring.clone());
        assert!(!vault.open());
        let cfg = GhostdSettings {
            data_dir: tmp.path().to_path_buf(),
            config_dir: tmp.path().join("config"),
            ..Default::default()
        };
        let mut out = Vec::new();
        assert_eq!(
            check_passwords(&vault, &cfg, &mut out),
            Vec::<String>::new()
        );
        let printed = String::from_utf8(out).unwrap();
        assert!(
            printed.contains("warning: Your keyring is locked"),
            "{printed}"
        );
        let problems: Vec<String> = [
            "OPENROUTER_API_KEY is not set",
            "sign in to OpenRouter (OPENROUTER_API_KEY is not set)",
            "MY_MODEL is not set",
            "GHOST_TTS_ENGINE=elevenlabs but ELEVENLABS_API_KEY is not set",
            "app linear (http): not connected: needs a password",
            "app notes (stdio): not connected: command not found",
            "policy: bad rule",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let apps: BTreeSet<String> = ["linear".to_string()].into();
        let named: BTreeSet<String> = ["MY_MODEL".to_string()].into();
        let (real, waits) = keyring_waits(&vault, &problems, &apps, &named);
        assert_eq!(
            real,
            vec![
                "app notes (stdio): not connected: command not found",
                "policy: bad rule"
            ]
        );
        assert_eq!(waits.len(), 5);

        let broken = Passwords::new(
            tmp.path().join("passwords.vault"),
            Arc::new(MemoryKeyring::with_key(&[1u8; 32])),
        );
        assert!(!broken.open());
        assert!(!check_passwords(&broken, &cfg, &mut Vec::new()).is_empty());
        assert_eq!(
            keyring_waits(&broken, &problems, &apps, &BTreeSet::new()),
            (problems.clone(), vec![])
        );
    }

    #[test]
    fn in_use_follows_the_model_connection_and_the_voice() {
        fn conn(pairs: &[(&str, &str)]) -> ModelConnection {
            let env: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            ModelConnection::from_env(&env)
        }
        let proxy = conn(&[
            ("ANTHROPIC_BASE_URL", "http://x:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "t"),
        ]);
        assert_eq!(in_use(Some(&proxy), "kokoro"), ["ANTHROPIC_AUTH_TOKEN"]);
        let openrouter = conn(&[("GHOST_LLM_ENDPOINT", "openrouter")]);
        assert_eq!(
            in_use(Some(&openrouter), "elevenlabs"),
            ["OPENROUTER_API_KEY", "ELEVENLABS_API_KEY"]
        );
        let openai = conn(&[("GHOST_LLM_PROVIDER", "openai")]);
        assert_eq!(in_use(Some(&openai), "kokoro"), ["OPENAI_API_KEY"]);
        let ollama = conn(&[("GHOST_LLM_PROVIDER", "ollama")]);
        assert_eq!(in_use(Some(&ollama), "kokoro"), Vec::<String>::new()); // no key at all
        assert_eq!(in_use(None, "kokoro"), Vec::<String>::new());
    }

    fn run(
        args: &[&str],
        cfg: &GhostdSettings,
        ring: Arc<dyn Keyring>,
        stdin: &str,
    ) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut input = io::Cursor::new(stdin.as_bytes().to_vec());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = {
            let mut cli = Cli {
                stdin: &mut input,
                stdout: &mut out,
                stderr: &mut err,
                tty: false,
            };
            passwords_cli(&args, cfg, ring, &mut cli)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn ghostd_passwords_without_a_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let ring: Arc<MemoryKeyring> = Arc::new(MemoryKeyring::new());
        let (code, _, _) = run(
            &["set", "ANTHROPIC_AUTH_TOKEN"],
            &cfg,
            ring.clone(),
            &format!("{TOKEN}\n"),
        );
        assert_eq!(code, 0);
        let (code, printed, _) = run(&["list"], &cfg, ring.clone(), "");
        assert_eq!(code, 0);
        assert!(printed.contains("ANTHROPIC_AUTH_TOKEN: set"), "{printed}");
        assert!(printed.contains("The model connection"), "{printed}");
        assert!(!printed.contains(TOKEN));

        std::fs::write(
            cfg.config_dir.join("apps.env"),
            "GHOST_APP_X_KEY=x-key-0123456789\n",
        )
        .unwrap();
        let (code, printed, err) = run(&["import"], &cfg, ring.clone(), "");
        assert_eq!(code, 0, "{err}");
        assert!(printed.contains("imported: GHOST_APP_X_KEY"), "{printed}");
        assert!(printed.contains("removed"), "{printed}");
        assert!(!cfg.config_dir.join("apps.env").exists());

        let (code, printed, _) = run(&["delete", "GHOST_APP_X_KEY"], &cfg, ring.clone(), "");
        assert_eq!(code, 0);
        assert!(printed.contains("GHOST_APP_X_KEY: forgotten"));
        let (code, _, err) = run(&["bogus"], &cfg, ring.clone(), "");
        assert_eq!(code, 2);
        assert!(err.contains("usage: ghostd passwords"));
        let (code, _, err) = run(&["set", "HF_TOKEN"], &cfg, ring.clone(), "\n");
        assert_eq!(code, 2);
        assert!(err.contains("no value given; nothing saved"));
        ring.set_locked(true);
        let (code, _, err) = run(
            &["set", "HF_TOKEN"],
            &cfg,
            ring.clone(),
            "another-value-123\n",
        );
        assert_eq!(code, 1);
        assert!(err.contains("locked"), "{err}");
        let (code, printed, _) = run(&["unlock"], &cfg, ring.clone(), "");
        assert_eq!(code, 0);
        assert_eq!(printed, "vault: open\n");
    }

    #[test]
    fn a_running_ghost_is_asked_over_its_socket() {
        // A tiny HTTP answerer on a Unix socket stands in for the daemon's `/passwords`.
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("api.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = [0u8; 4096];
            let n = stream.read(&mut raw).unwrap();
            let request = String::from_utf8_lossy(&raw[..n]).into_owned();
            let body = json!({
                "vault": {"path": "~/.local/share/ghost/passwords.vault", "state": "open", "problem": null},
                "passwords": [
                    {"name": "HF_TOKEN", "set": true, "used_by": ["Voice model downloads"], "last_used": "2026-10-28T20:40:26Z"}
                ],
                "importable": [{"path": "~/.config/ghost/env"}],
            })
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            request
        });
        let cfg = GhostdSettings {
            api_socket: Some(sock),
            ..cfg_for(tmp.path())
        };
        let (code, printed, _) = run(&["list"], &cfg, Arc::new(MemoryKeyring::new()), "");
        assert_eq!(code, 0);
        assert_eq!(
            printed,
            "vault: ~/.local/share/ghost/passwords.vault (open)\n  HF_TOKEN: set (Voice model downloads, last used 2026-10-28T20:40)\n  to import: ~/.config/ghost/env\n"
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /passwords HTTP/1.1\r\n"));
    }
}
