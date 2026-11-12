//! Outside clients (ADR 0048): the tools on this computer the operator let use Ghost's own
//! app, each with a name and a token.
//!
//! A token is made once, shown once and never again: the store keeps its SHA-256 with the
//! client's name, when it was made and when it was last used. It is a password in Ghost's
//! sense, so it is never logged, never in the audit and never returned by a list. A token
//! says whose it is (`ghost.<name>.<secret>`), so a wrong one counts against that name
//! alone: five failures refuse the name for five minutes (the limits of ZeroClaw's pairing
//! guard, `crates/zeroclaw-config/src/pairing.rs`, MIT OR Apache-2.0), and a client with a
//! stale token cannot lock the others out. A token made after a lockout began lifts it.
//!
//! The one place a token is written is the file `ghostd mcp serve` reads
//! (`<data>/clients/<name>.token`, 0600 in a 0700 folder), and only when the operator asks
//! for a client that is launched as a command.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::store::{ClientRow, Store, StoreError};
use crate::time::utcnow;

/// Where the daemon listens for outside clients unless `GHOST_MCP_LISTEN` says otherwise:
/// this computer only.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:47824";
/// What every token starts with.
pub const TOKEN_PREFIX: &str = "ghost.";
/// The random part: 32 bytes.
pub const TOKEN_BYTES: usize = 32;
pub const NAME_MAX: usize = 40;
/// Wrong tokens before a name (or an address with no name) is refused for a while.
pub const MAX_FAILURES: u32 = 5;
pub const LOCKOUT: Duration = Duration::from_secs(300);
/// `last_used` is written at most this often per client.
pub const TOUCH_EVERY: Duration = Duration::from_secs(60);
/// The source of a client's items on the waiting list, and its name as a caller:
/// `client:<name>`.
pub const CLIENT_SOURCE: &str = "client:";
/// How many names and addresses the guard remembers.
const GUARD_MAX: usize = 1_000;
/// Compared against when no client has the name, so a miss takes as long as a hit.
const NO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The client a request was authenticated as. The listener puts it in the request's
/// extensions; the server reads the caller from it and from nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientId(pub String);

/// A client, as it may be shown: never its token or the token's hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<ClientRow> for ClientInfo {
    fn from(row: ClientRow) -> Self {
        Self {
            name: row.name,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }
}

/// A client just made: the one time its token exists outside the client's own config.
pub struct Created {
    pub client: ClientInfo,
    pub token: Zeroizing<String>,
    /// The file `ghostd mcp serve` reads, when one was asked for.
    pub token_file: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientsError {
    #[error("{0}")]
    Name(String),
    #[error("there is already a client called {0}; remove it first for a new token")]
    Taken(String),
    #[error("no client called {0}")]
    NotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("the token file could not be written: {0}")]
    Io(#[from] std::io::Error),
}

/// Why a request was refused. None of them says whether the name exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No bearer token.
    Missing,
    /// A token that is not a client's.
    Bad,
    /// Too many wrong tokens: refused without a look, for this many seconds more.
    Locked { retry_after: u64 },
}

/// Why a name will not do, or None: lowercase letters, digits and hyphens, starting with
/// a letter or a digit.
pub fn name_problem(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && name.len() <= NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    (!ok).then(|| {
        format!(
            "a client's name is lowercase letters, digits and hyphens, {NAME_MAX} at most \
             (claude-code, cursor), not {name:?}"
        )
    })
}

/// The name as it is shown and said: `claude-code` is "Claude Code".
pub fn shown(name: &str) -> String {
    name.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The caller a client is, and the source of its items: `client:<name>`.
pub fn client_source(name: &str) -> String {
    format!("{CLIENT_SOURCE}{name}")
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The name a token says it is for, when it has the shape of one.
fn claimed(token: &str) -> Option<&str> {
    let (name, secret) = token.strip_prefix(TOKEN_PREFIX)?.split_once('.')?;
    (name_problem(name).is_none() && !secret.is_empty()).then_some(name)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy)]
struct Strikes {
    count: u32,
    last: Instant,
    /// Until when it is refused, and when that began on the wall clock.
    locked: Option<(Instant, DateTime<Utc>)>,
}

/// Wrong tokens, counted per name (or per address when a request names nobody).
struct Guard {
    max: u32,
    lockout: Duration,
    strikes: HashMap<String, Strikes>,
}

impl Guard {
    fn locked(&mut self, key: &str, now: Instant) -> Option<(u64, DateTime<Utc>)> {
        let (until, since) = self.strikes.get(key)?.locked?;
        if now >= until {
            self.strikes.remove(key);
            return None;
        }
        Some(((until - now).as_secs().max(1), since))
    }

    fn strike(&mut self, key: &str, now: Instant) {
        if self.strikes.len() >= GUARD_MAX && !self.strikes.contains_key(key) {
            // The oldest goes: a flood of made-up names cannot grow this without bound.
            if let Some(oldest) = self
                .strikes
                .iter()
                .min_by_key(|(_, s)| s.last)
                .map(|(k, _)| k.clone())
            {
                self.strikes.remove(&oldest);
            }
        }
        let s = self.strikes.entry(key.to_string()).or_insert(Strikes {
            count: 0,
            last: now,
            locked: None,
        });
        s.count += 1;
        s.last = now;
        if s.count >= self.max {
            s.locked = Some((now + self.lockout, utcnow()));
        }
    }

    fn clear(&mut self, key: &str) {
        self.strikes.remove(key);
    }
}

/// The clients: made, listed, removed and checked against the store, so one removed from
/// the command line is refused by the running daemon at its next request.
pub struct Clients {
    store: Arc<Store>,
    /// Where a token file goes: `<data>/clients`.
    dir: PathBuf,
    guard: Mutex<Guard>,
    touched: Mutex<HashMap<String, Instant>>,
}

impl Clients {
    pub fn new(store: Arc<Store>, dir: impl Into<PathBuf>) -> Self {
        Self {
            store,
            dir: dir.into(),
            guard: Mutex::new(Guard {
                max: MAX_FAILURES,
                lockout: LOCKOUT,
                strikes: HashMap::new(),
            }),
            touched: Mutex::new(HashMap::new()),
        }
    }

    /// Other limits, for a test.
    pub fn with_lockout(self, max: u32, lockout: Duration) -> Self {
        {
            let mut g = lock(&self.guard);
            g.max = max.max(1);
            g.lockout = lockout;
        }
        self
    }

    pub fn token_file(&self, name: &str) -> PathBuf {
        token_file(&self.dir, name)
    }

    /// A new client and its token. With `stdio`, the token is also written where
    /// `ghostd mcp serve --client <name>` reads it.
    pub fn create(&self, name: &str, stdio: bool) -> Result<Created, ClientsError> {
        let name = name.trim();
        if let Some(problem) = name_problem(name) {
            return Err(ClientsError::Name(problem));
        }
        let mut secret = Zeroizing::new([0u8; TOKEN_BYTES]);
        rand::rng().fill_bytes(&mut secret[..]);
        let token = Zeroizing::new(format!(
            "{TOKEN_PREFIX}{name}.{}",
            URL_SAFE_NO_PAD.encode(&secret[..])
        ));
        let now = utcnow();
        if !self.store.insert_client(name, &hash(&token), now)? {
            return Err(ClientsError::Taken(name.to_string()));
        }
        let token_file = if stdio {
            let path = self.token_file(name);
            if let Err(e) = write_token(&path, &token) {
                let _ = self.store.delete_client(name);
                return Err(e.into());
            }
            Some(path)
        } else {
            None
        };
        info!("clients: {name} added");
        Ok(Created {
            client: ClientInfo {
                name: name.to_string(),
                created_at: now,
                last_used_at: None,
            },
            token,
            token_file,
        })
    }

    pub fn list(&self) -> Result<Vec<ClientInfo>, ClientsError> {
        Ok(self
            .store
            .list_clients()?
            .into_iter()
            .map(ClientInfo::from)
            .collect())
    }

    /// Take a client away: its token stops working at once, and its token file goes.
    pub fn remove(&self, name: &str) -> Result<(), ClientsError> {
        let name = name.trim();
        if name_problem(name).is_some() || !self.store.delete_client(name)? {
            return Err(ClientsError::NotFound(name.to_string()));
        }
        match std::fs::remove_file(self.token_file(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(
                "clients: {name}'s token file was not removed: {:?}",
                e.kind()
            ),
        }
        lock(&self.touched).remove(name);
        info!("clients: {name} removed");
        Ok(())
    }

    /// Whose request this is, from its `Authorization` header. `peer` is the address it
    /// came from, which a request naming nobody is counted against.
    pub fn authenticate(
        &self,
        authorization: Option<&str>,
        peer: &str,
    ) -> Result<String, AuthError> {
        let token = authorization
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let name = token.and_then(claimed);
        let key = match name {
            Some(n) => client_source(n),
            None => format!("addr:{peer}"),
        };
        let now = Instant::now();
        let row = match name.map(|n| self.store.get_client(n)) {
            Some(Ok(row)) => row,
            Some(Err(e)) => {
                warn!("clients: the store could not be read: {e}");
                None
            }
            None => None,
        };
        // Looked up on its own line: the guard's lock is let go before it is taken again.
        let locked = lock(&self.guard).locked(&key, now);
        if let Some((retry_after, since)) = locked {
            // A token made after the lockout began is a new client under the old name.
            if !row.as_ref().is_some_and(|r| r.created_at > since) {
                return Err(AuthError::Locked { retry_after });
            }
            lock(&self.guard).clear(&key);
        }
        let Some(token) = token else {
            lock(&self.guard).strike(&key, now);
            return Err(AuthError::Missing);
        };
        let presented = hash(token);
        let expected = row.as_ref().map_or(NO_HASH, |r| r.token_hash.as_str());
        let same: bool = presented.as_bytes().ct_eq(expected.as_bytes()).into();
        let (true, Some(row)) = (same, row) else {
            lock(&self.guard).strike(&key, now);
            return Err(AuthError::Bad);
        };
        lock(&self.guard).clear(&key);
        let due = {
            let mut touched = lock(&self.touched);
            let due = touched
                .get(&row.name)
                .is_none_or(|t| now.duration_since(*t) >= TOUCH_EVERY);
            if due {
                touched.insert(row.name.clone(), now);
            }
            due
        };
        if due && let Err(e) = self.store.touch_client(&row.name, utcnow()) {
            warn!("clients: last used not written: {e}");
        }
        Ok(row.name)
    }
}

/// Where a client's token file is, under the clients folder.
pub fn token_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.token"))
}

fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(token.as_bytes())?;
    f.write_all(b"\n")
}

/// The token `ghostd mcp serve` sends, from the file `add` wrote. A file anyone else can
/// read is refused.
pub fn read_token(dir: &Path, name: &str) -> std::io::Result<Zeroizing<String>> {
    use std::os::unix::fs::PermissionsExt;
    let path = token_file(dir, name);
    let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} can be read by others (mode {mode:o}); it must be 600",
                path.display()
            ),
        ));
    }
    Ok(Zeroizing::new(
        std::fs::read_to_string(&path)?.trim().to_string(),
    ))
}

/// What the operator pastes into the other tool: shown once, with the token in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippets {
    /// The command that connects Claude Code.
    pub claude_code: String,
    /// The `mcp.json` other tools read (Cursor's `~/.cursor/mcp.json`, and the like).
    pub config_json: String,
}

/// For a tool that connects to an address: `address` is the listener's `host:port`.
pub fn http_snippets(token: &str, address: &str) -> Snippets {
    let url = format!("http://{address}/mcp");
    let config = serde_json::json!({
        "mcpServers": {"ghost": {
            "type": "http",
            "url": url,
            "headers": {"Authorization": format!("Bearer {token}")},
        }}
    });
    Snippets {
        claude_code: format!(
            "claude mcp add --transport http ghost {url} --header \"Authorization: Bearer {token}\""
        ),
        config_json: serde_json::to_string_pretty(&config).unwrap_or_default(),
    }
}

/// For a tool that only starts commands: it runs `ghostd mcp serve`, which reads the token
/// from its file, so the config holds no token.
pub fn stdio_snippets(name: &str, ghostd: &str) -> Snippets {
    let config = serde_json::json!({
        "mcpServers": {"ghost": {
            "command": ghostd,
            "args": ["mcp", "serve", "--client", name],
        }}
    });
    Snippets {
        claude_code: format!("claude mcp add ghost -- {ghostd} mcp serve --client {name}"),
        config_json: serde_json::to_string_pretty(&config).unwrap_or_default(),
    }
}

/// Said when a command was asked for and Ghost runs from an AppImage.
pub const APPIMAGE_NOTE: &str = "Ghost is running from an AppImage, which is mounted at a new \
path each time it starts, so another tool cannot be given a command to run that keeps working. \
These connect by address instead. A tool that can only start a command needs Ghost installed \
from the .deb.";

/// Whether this process runs from an AppImage (`APPIMAGE` or `APPDIR` set by its
/// runtime), whose `ghostd` is on a temporary mount.
pub fn in_appimage(var: impl Fn(&str) -> Option<String>) -> bool {
    ["APPIMAGE", "APPDIR"]
        .iter()
        .any(|k| var(k).is_some_and(|v| !v.trim().is_empty()))
}

/// The `ghostd` another tool can be told to run, and that will still be there tomorrow:
/// this binary, unless it runs from an AppImage. The AppImage's launcher is the desktop
/// app and does not hand its arguments to `ghostd`, so it is no command either; there is
/// no stable one, and a command client connects by address instead.
pub fn stable_ghostd(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if in_appimage(var) {
        return None;
    }
    Some(
        std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "ghostd".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clients(tmp: &Path) -> Clients {
        Clients::new(Arc::new(Store::in_memory().unwrap()), tmp.join("clients"))
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    #[test]
    fn a_token_is_shown_once_and_only_its_hash_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let c = clients(tmp.path());
        let made = c.create("claude-code", false).unwrap();
        assert!(made.token.starts_with("ghost.claude-code."));
        assert_eq!(made.token.len(), "ghost.claude-code.".len() + 43);
        assert!(made.token_file.is_none());
        let row = c.store.get_client("claude-code").unwrap().unwrap();
        assert_eq!(row.token_hash, hash(&made.token));
        assert!(!row.token_hash.contains(&made.token[..]));
        let listed = c.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "claude-code");
        assert!(listed[0].last_used_at.is_none());
        assert_eq!(
            c.authenticate(Some(&bearer(&made.token)), "127.0.0.1"),
            Ok("claude-code".into())
        );
        assert!(c.list().unwrap()[0].last_used_at.is_some());
        // The same name again is refused: a new token means removing the old client.
        assert!(matches!(
            c.create("claude-code", false),
            Err(ClientsError::Taken(_))
        ));
    }

    #[test]
    fn names_are_plain_and_said_as_words() {
        for bad in [
            "",
            "Claude",
            "a b",
            "-x",
            "x-",
            "a/b",
            "a.b",
            &"x".repeat(41),
        ] {
            assert!(name_problem(bad).is_some(), "{bad:?}");
        }
        assert!(name_problem("claude-code").is_none());
        assert!(name_problem("cursor2").is_none());
        assert_eq!(shown("claude-code"), "Claude Code");
        assert_eq!(client_source("cursor"), "client:cursor");
    }

    #[test]
    fn no_token_a_wrong_token_and_a_removed_client_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let c = clients(tmp.path());
        let made = c.create("cursor", false).unwrap();
        assert_eq!(c.authenticate(None, "127.0.0.1"), Err(AuthError::Missing));
        assert_eq!(
            c.authenticate(Some("Basic abc"), "127.0.0.1"),
            Err(AuthError::Missing)
        );
        assert_eq!(
            c.authenticate(Some("Bearer ghost.cursor.nope"), "127.0.0.1"),
            Err(AuthError::Bad)
        );
        assert_eq!(
            c.authenticate(Some("Bearer not-a-token"), "127.0.0.1"),
            Err(AuthError::Bad)
        );
        // Another client's name with this one's secret is not this client.
        let other = made.token.replace("ghost.cursor.", "ghost.codex.");
        assert_eq!(
            c.authenticate(Some(&bearer(&other)), "127.0.0.1"),
            Err(AuthError::Bad)
        );
        c.remove("cursor").unwrap();
        assert_eq!(
            c.authenticate(Some(&bearer(&made.token)), "127.0.0.1"),
            Err(AuthError::Bad)
        );
        assert!(matches!(c.remove("cursor"), Err(ClientsError::NotFound(_))));
    }

    #[test]
    fn five_wrong_tokens_lock_that_name_and_no_other() {
        let tmp = tempfile::tempdir().unwrap();
        let c = clients(tmp.path());
        let cursor = c.create("cursor", false).unwrap();
        let codex = c.create("codex", false).unwrap();
        for _ in 0..MAX_FAILURES {
            assert_eq!(
                c.authenticate(Some("Bearer ghost.cursor.wrong"), "127.0.0.1"),
                Err(AuthError::Bad)
            );
        }
        // Locked: even the right token is refused, without a look.
        let locked = c.authenticate(Some(&bearer(&cursor.token)), "127.0.0.1");
        assert!(
            matches!(locked, Err(AuthError::Locked { retry_after }) if retry_after > 0 && retry_after <= 300),
            "{locked:?}"
        );
        // The other client is untouched, from the same address.
        assert_eq!(
            c.authenticate(Some(&bearer(&codex.token)), "127.0.0.1"),
            Ok("codex".into())
        );
        // A new token under the name, made after the lockout began, lifts it.
        c.remove("cursor").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let again = c.create("cursor", false).unwrap();
        assert_eq!(
            c.authenticate(Some(&bearer(&again.token)), "127.0.0.1"),
            Ok("cursor".into())
        );
    }

    #[test]
    fn requests_with_no_name_are_counted_against_the_address_and_a_lockout_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let c = clients(tmp.path()).with_lockout(2, Duration::from_millis(40));
        let made = c.create("cursor", false).unwrap();
        assert_eq!(c.authenticate(None, "10.0.0.9"), Err(AuthError::Missing));
        assert_eq!(c.authenticate(None, "10.0.0.9"), Err(AuthError::Missing));
        assert!(matches!(
            c.authenticate(None, "10.0.0.9"),
            Err(AuthError::Locked { .. })
        ));
        // A named client from that address still gets in; another address is not locked.
        assert!(
            c.authenticate(Some(&bearer(&made.token)), "10.0.0.9")
                .is_ok()
        );
        assert_eq!(c.authenticate(None, "10.0.0.8"), Err(AuthError::Missing));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(c.authenticate(None, "10.0.0.9"), Err(AuthError::Missing));
    }

    #[test]
    fn the_token_file_is_private_and_goes_with_the_client() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let c = clients(tmp.path());
        let made = c.create("codex", true).unwrap();
        let path = made.token_file.clone().unwrap();
        assert_eq!(path, tmp.path().join("clients/codex.token"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(
            *read_token(&tmp.path().join("clients"), "codex").unwrap(),
            *made.token
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_token(&tmp.path().join("clients"), "codex").is_err());
        c.remove("codex").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn the_snippets_are_ready_to_paste() {
        let http = http_snippets("ghost.cursor.abc", "127.0.0.1:47824");
        assert_eq!(
            http.claude_code,
            "claude mcp add --transport http ghost http://127.0.0.1:47824/mcp --header \"Authorization: Bearer ghost.cursor.abc\""
        );
        let parsed: serde_json::Value = serde_json::from_str(&http.config_json).unwrap();
        assert_eq!(
            parsed["mcpServers"]["ghost"]["headers"]["Authorization"],
            "Bearer ghost.cursor.abc"
        );
        let stdio = stdio_snippets("cursor", "ghostd");
        assert_eq!(
            stdio.claude_code,
            "claude mcp add ghost -- ghostd mcp serve --client cursor"
        );
        assert!(!stdio.config_json.contains("ghost.cursor"));
    }

    #[test]
    fn an_appimage_has_no_ghostd_a_tool_can_keep_running() {
        let none = |_: &str| None;
        assert!(!in_appimage(none));
        assert!(stable_ghostd(none).is_some());
        for key in ["APPIMAGE", "APPDIR"] {
            let set = move |k: &str| (k == key).then(|| "/tmp/.mount_GhostAb12".to_string());
            assert!(in_appimage(set), "{key}");
            assert_eq!(stable_ghostd(set), None, "{key}");
        }
        // Set but empty is not an AppImage.
        assert!(!in_appimage(|_: &str| Some(String::new())));
    }
}
