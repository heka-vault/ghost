//! Passwords (ADR 0036): every API key, token and account password Ghost holds.
//!
//! The operator's word is *password*; the code says secret where it always has. They are
//! kept in Ghost's own vault, one file (`~/.local/share/ghost/passwords.vault`, 0600),
//! sealed with AES-256-GCM. The vault's key is in the operator's keyring through Secret
//! Service (GNOME Keyring), never on disk. The daemon opens the vault at start and serves
//! the values from memory; nothing writes a value anywhere else.
//!
//! A value goes in and never comes back out: there is no method here that hands a value to
//! a caller except at the point of use (`value`, and the mapping `environ` that an app's
//! `fill_secrets` reads when it connects), and every value is in the redactor
//! (`crate::redact`), so text from outside (an app's result, notice or error) and what Ghost
//! stores or logs is checked against it before anything reads it.
//!
//! Where the values live while the daemon runs:
//!
//! - In the environment the daemon keeps in step with the vault (`SharedEnviron`), so
//!   Ghost's own settings (the model connection, Jev, the ElevenLabs voice, the Hugging
//!   Face download) read them where they always did. The vault never touches the process
//!   environment itself. Nothing Ghost spawns inherits them: `redact::child_env` leaves
//!   every password out.
//! - An app that runs on this computer gets the ones its `env` names as environment
//!   variables. That is the one place a value leaves the vault.
//!
//! A locked keyring is not a failure: `open` says `locked`, and the daemon keeps trying.
//!
//! The vault, the file lock and the keyring are blocking: the daemon calls them off its
//! async threads (`spawn_blocking`). `SecretServiceKeyring` in particular drives zbus's
//! blocking API, which must not run inside an async task.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use aes_gcm::aead::{Aead, Generate, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use chrono::{DateTime, TimeDelta, Utc};
use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::redact;
pub use crate::redact::MIN_REDACT_CHARS;
use crate::time::{iso, parse_iso, utcnow};

pub const VAULT_FILE: &str = "passwords.vault";
pub const MAX_VALUE_CHARS: usize = 16_000;
/// `last_used` is written back at most this often per password, so a busy app is not a
/// write per call.
pub const LAST_USED_EVERY: TimeDelta = TimeDelta::minutes(10);
const AAD: &[u8] = b"ghost-passwords-v1";

/// The passwords Ghost itself reads, and what for (shown as "used by"). Every other name
/// is an app's (`${NAME}` in apps.json) or a webhook's (`hooks.json`), or one the operator
/// added by hand.
pub const GHOST_USES: [(&str, &str); 9] = [
    // The model connection's keys, one per provider (ADR 0037); GHOST_LLM_PASSWORD may
    // name another, which the Passwords page shows as the model's too.
    ("ANTHROPIC_AUTH_TOKEN", "The model connection"),
    ("ANTHROPIC_API_KEY", "The model connection"),
    ("OPENAI_API_KEY", "The model connection"),
    ("LLM_API_KEY", "The model connection"),
    (
        "OPENROUTER_API_KEY",
        "Jev, and the model when it goes through OpenRouter",
    ),
    ("TYPESAFE_API_KEY", "Jev"),
    ("ELEVENLABS_API_KEY", "The ElevenLabs voice"),
    ("HF_TOKEN", "Voice model downloads"),
    (
        "GITHUB_TOKEN",
        "An app you added by hand that names ${GITHUB_TOKEN}",
    ),
];
const SECRET_SUFFIXES: [&str; 9] = [
    "_TOKEN",
    "_KEY",
    "_SECRET",
    "_PASSWORD",
    "_PASS",
    "_PASSWD",
    "_PAT",
    "_API_KEY",
    "_CREDENTIALS",
];

/// What Ghost itself uses a password for, when it is one of `GHOST_USES`.
pub fn ghost_use(name: &str) -> Option<&'static str> {
    GHOST_USES.iter().find(|(n, _)| *n == name).map(|(_, u)| *u)
}

/// Whether a KEY in a plaintext env file is a password: one Ghost reads, one an app or a
/// webhook names (`referenced`), a key Ghost kept for an app, or a name shaped like one.
/// Ghost's own settings (`GHOST_*`) are not, except an app's key (`GHOST_APP_*`). Case
/// doesn't matter: `openai_api_key` is as much a password as `OPENAI_API_KEY`.
pub fn is_password_name(name: &str, referenced: &[&str]) -> bool {
    let upper = name.to_uppercase();
    if ghost_use(&upper).is_some()
        || referenced.iter().any(|r| r.to_uppercase() == upper)
        || upper.starts_with("GHOST_APP_")
    {
        return true;
    }
    if upper.starts_with("GHOST_") {
        return false;
    }
    SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordsError {
    /// The keyring is there but locked (before login, or headless).
    #[error("{0}")]
    KeyringLocked(String),
    /// No Secret Service on this session bus (no keyring daemon, no bus).
    #[error("{0}")]
    KeyringUnavailable(String),
    /// The vault file can't be read with the key the keyring holds. Never overwritten.
    #[error("{0}")]
    VaultBroken(String),
    /// A change while the vault isn't open (locked, no keyring, or broken).
    #[error("{0}")]
    VaultClosed(String),
    /// A name or value the vault doesn't take (Python's `ValueError`).
    #[error("{0}")]
    Invalid(String),
    /// The file or the lock beside it couldn't be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl PasswordsError {
    /// The variant's name, for a log line that must not carry the message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::KeyringLocked(_) => "KeyringLocked",
            Self::KeyringUnavailable(_) => "KeyringUnavailable",
            Self::VaultBroken(_) => "VaultBroken",
            Self::VaultClosed(_) => "VaultClosed",
            Self::Invalid(_) => "ValueError",
            Self::Io(_) => "OSError",
        }
    }
}

/// Whether the vault is open, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Locked,
    Unavailable,
    Broken,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Locked => "locked",
            Self::Unavailable => "unavailable",
            Self::Broken => "broken",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// --- the keyring -----------------------------------------------------------------------

/// The vault's key, as bytes that are wiped when dropped.
pub type VaultKey = Zeroizing<Vec<u8>>;

pub trait Keyring: Send + Sync {
    /// The vault's key, or None when the keyring has none yet. Errs `KeyringLocked` or
    /// `KeyringUnavailable`.
    fn load_key(&self) -> Result<Option<VaultKey>, PasswordsError>;

    fn store_key(&self, key: &[u8]) -> Result<(), PasswordsError>;

    /// Ask the keyring to unlock (it may show its own prompt). True when it is.
    fn unlock(&self) -> Result<bool, PasswordsError>;
}

/// The vault's key in the default collection of Secret Service (GNOME Keyring's login
/// keyring), as one item Ghost owns: the same label and attributes the Python daemon
/// stored, so the two find each other's key.
#[derive(Debug, Default, Clone, Copy)]
pub struct SecretServiceKeyring;

impl SecretServiceKeyring {
    pub const ATTRIBUTES: [(&'static str, &'static str); 2] =
        [("application", "ghost"), ("ghost", "passwords-vault-key")];
    pub const LABEL: &'static str = "Ghost passwords vault key";

    fn attributes() -> HashMap<&'static str, &'static str> {
        Self::ATTRIBUTES.into_iter().collect()
    }

    /// The session bus, the service and its default collection, for one call. No bus or
    /// no service is `KeyringUnavailable`; a locked collection is told apart inside `f`.
    fn with_collection<T>(
        &self,
        f: impl FnOnce(&secret_service::blocking::Collection<'_>) -> Result<T, PasswordsError>,
    ) -> Result<T, PasswordsError> {
        use secret_service::EncryptionType;
        use secret_service::blocking::SecretService;

        let service = SecretService::connect(EncryptionType::Dh).map_err(|e| {
            PasswordsError::KeyringUnavailable(format!(
                "no keyring on the session bus ({})",
                ss_kind(&e)
            ))
        })?;
        let collection = service.get_default_collection().map_err(|e| {
            PasswordsError::KeyringUnavailable(format!("no default keyring ({})", ss_kind(&e)))
        })?;
        f(&collection)
    }
}

/// A Secret Service error's kind, for a message; never its text.
fn ss_kind(e: &secret_service::Error) -> &'static str {
    use secret_service::Error as E;
    match e {
        E::Crypto(_) => "Crypto",
        E::Zbus(_) => "Zbus",
        E::ZbusFdo(_) => "ZbusFdo",
        E::Zvariant(_) => "Zvariant",
        E::Locked => "Locked",
        E::NoResult => "NoResult",
        E::Prompt => "Prompt",
        E::PromptDisconnected => "PromptDisconnected",
        E::Unavailable => "Unavailable",
        _ => "Error",
    }
}

/// A Secret Service error met while the collection is in hand.
fn ss_error(e: secret_service::Error) -> PasswordsError {
    match e {
        secret_service::Error::Locked => {
            PasswordsError::KeyringLocked("the keyring is locked".into())
        }
        other => PasswordsError::KeyringUnavailable(format!(
            "the keyring didn't answer ({})",
            ss_kind(&other)
        )),
    }
}

impl Keyring for SecretServiceKeyring {
    fn load_key(&self) -> Result<Option<VaultKey>, PasswordsError> {
        self.with_collection(|collection| {
            if collection.is_locked().map_err(ss_error)? {
                return Err(PasswordsError::KeyringLocked(
                    "the keyring is locked".into(),
                ));
            }
            let items = collection
                .search_items(Self::attributes())
                .map_err(ss_error)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            let encoded = Zeroizing::new(item.get_secret().map_err(ss_error)?);
            let key = BASE64_STANDARD.decode(&*encoded).map_err(|_| {
                PasswordsError::KeyringUnavailable(
                    "the keyring's item isn't a key Ghost wrote".into(),
                )
            })?;
            Ok(Some(Zeroizing::new(key)))
        })
    }

    fn store_key(&self, key: &[u8]) -> Result<(), PasswordsError> {
        self.with_collection(|collection| {
            if collection.is_locked().map_err(ss_error)? {
                return Err(PasswordsError::KeyringLocked(
                    "the keyring is locked".into(),
                ));
            }
            let encoded = Zeroizing::new(BASE64_STANDARD.encode(key));
            collection
                .create_item(
                    Self::LABEL,
                    Self::attributes(),
                    encoded.as_bytes(),
                    true,
                    "text/plain",
                )
                .map_err(ss_error)?;
            Ok(())
        })
    }

    fn unlock(&self) -> Result<bool, PasswordsError> {
        self.with_collection(|collection| {
            if collection.is_locked().map_err(ss_error)? {
                // The keyring's own prompt, in the operator's session.
                collection.unlock().map_err(ss_error)?;
            }
            Ok(!collection.is_locked().map_err(ss_error)?)
        })
    }
}

/// A keyring for tests and tools: holds the key in memory, can be locked or absent.
#[derive(Debug, Default)]
pub struct MemoryKeyring {
    key: Mutex<Option<VaultKey>>,
    locked: AtomicBool,
    unavailable: AtomicBool,
    unlock_fails: AtomicBool,
}

impl MemoryKeyring {
    /// Available, unlocked, no key yet.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_key(key: &[u8]) -> Self {
        let ring = Self::new();
        ring.set_key(Some(key));
        ring
    }

    pub fn locked() -> Self {
        let ring = Self::new();
        ring.set_locked(true);
        ring
    }

    pub fn unavailable() -> Self {
        let ring = Self::new();
        ring.set_available(false);
        ring
    }

    /// The key it holds, for a test that checks one was made; never a password.
    pub fn key(&self) -> Option<VaultKey> {
        lock(&self.key).clone()
    }

    pub fn set_key(&self, key: Option<&[u8]>) {
        *lock(&self.key) = key.map(|k| Zeroizing::new(k.to_vec()));
    }

    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::SeqCst);
    }

    pub fn set_available(&self, available: bool) {
        self.unavailable.store(!available, Ordering::SeqCst);
    }

    /// Whether `unlock` works (a dismissed prompt doesn't).
    pub fn set_unlock_works(&self, works: bool) {
        self.unlock_fails.store(!works, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), PasswordsError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(PasswordsError::KeyringUnavailable("no keyring".into()));
        }
        if self.locked.load(Ordering::SeqCst) {
            return Err(PasswordsError::KeyringLocked(
                "the keyring is locked".into(),
            ));
        }
        Ok(())
    }
}

impl Keyring for MemoryKeyring {
    fn load_key(&self) -> Result<Option<VaultKey>, PasswordsError> {
        self.check()?;
        Ok(self.key())
    }

    fn store_key(&self, key: &[u8]) -> Result<(), PasswordsError> {
        self.check()?;
        self.set_key(Some(key));
        Ok(())
    }

    fn unlock(&self) -> Result<bool, PasswordsError> {
        let available = !self.unavailable.load(Ordering::SeqCst);
        if available && !self.unlock_fails.load(Ordering::SeqCst) {
            self.set_locked(false);
        }
        Ok(available && !self.locked.load(Ordering::SeqCst))
    }
}

// --- the file --------------------------------------------------------------------------

/// `payload` sealed for the file: the envelope the Python daemon writes, byte for byte
/// (`json.dumps` spacing, a newline at the end), around AES-256-GCM over the payload's
/// JSON with `ghost-passwords-v1` as the associated data.
pub fn seal(key: &[u8], payload: &impl Serialize) -> Result<Vec<u8>, PasswordsError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| {
        PasswordsError::VaultClosed("the key in your keyring isn't one Ghost can seal with".into())
    })?;
    let nonce: Nonce<<Aes256Gcm as aes_gcm::AeadCore>::NonceSize> = Generate::generate();
    let plain =
        Zeroizing::new(serde_json::to_vec(payload).map_err(|e| {
            PasswordsError::VaultClosed(format!("the vault couldn't be written: {e}"))
        })?);
    let data = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plain,
                aad: AAD,
            },
        )
        .map_err(|_| PasswordsError::VaultClosed("the vault couldn't be sealed".into()))?;
    Ok(format!(
        "{{\"ghost_passwords\": 1, \"cipher\": \"AES-256-GCM\", \"nonce\": \"{}\", \"data\": \"{}\"}}\n",
        BASE64_STANDARD.encode(nonce),
        BASE64_STANDARD.encode(data)
    )
    .into_bytes())
}

/// The payload inside a sealed file. The decrypted bytes are wiped once parsed; the
/// caller drops what it took from the map as soon as it has copied the values out.
pub fn unseal(key: &[u8], raw: &[u8]) -> Result<serde_json::Map<String, Value>, PasswordsError> {
    let damaged = || PasswordsError::VaultBroken("the vault file is damaged".into());
    let wrong_key =
        || PasswordsError::VaultBroken("the key in your keyring doesn't open this vault".into());
    let envelope: Value = serde_json::from_slice(raw).map_err(|_| damaged())?;
    let envelope = envelope.as_object().ok_or_else(damaged)?;
    if envelope.get("ghost_passwords") != Some(&Value::from(1))
        || envelope.get("cipher").and_then(Value::as_str) != Some("AES-256-GCM")
    {
        return Err(PasswordsError::VaultBroken(
            "not a vault Ghost can read".into(),
        ));
    }
    let field = |name: &str| -> Result<Vec<u8>, PasswordsError> {
        let text = envelope
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(damaged)?;
        BASE64_STANDARD.decode(text).map_err(|_| damaged())
    };
    let nonce = field("nonce")?;
    let data = field("data")?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| wrong_key())?;
    let nonce = Nonce::<<Aes256Gcm as aes_gcm::AeadCore>::NonceSize>::try_from(&nonce[..])
        .map_err(|_| wrong_key())?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &data,
                    aad: AAD,
                },
            )
            .map_err(|_| wrong_key())?,
    );
    let payload: Value = serde_json::from_slice(&plain).map_err(|_| damaged())?;
    match payload {
        Value::Object(map) => Ok(map),
        _ => Err(damaged()),
    }
}

fn when(raw: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = raw?.as_str()?;
    if s.is_empty() {
        return None;
    }
    parse_iso(s)
}

/// One password as the file holds it.
#[derive(Serialize)]
struct StoredEntry {
    value: String,
    added: String,
    updated: String,
    last_used: Option<String>,
}

#[derive(Serialize)]
struct StoredPayload {
    passwords: BTreeMap<String, StoredEntry>,
}

struct Entry {
    value: Zeroizing<String>,
    added: DateTime<Utc>,
    updated: DateTime<Utc>,
    last_used: Option<DateTime<Utc>>,
    /// The `last_used` the file holds.
    saved_use: Option<DateTime<Utc>>,
}

impl Entry {
    fn dump(&self) -> StoredEntry {
        StoredEntry {
            value: self.value.to_string(),
            added: iso(&self.added),
            updated: iso(&self.updated),
            last_used: self.last_used.as_ref().map(iso),
        }
    }
}

impl fmt::Debug for Entry {
    /// Never the value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("added", &self.added)
            .field("updated", &self.updated)
            .field("last_used", &self.last_used)
            .field("saved_use", &self.saved_use)
            .finish_non_exhaustive()
    }
}

/// What may be said about a password: never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PasswordMeta {
    pub name: String,
    #[serde(with = "crate::time::json")]
    pub added: DateTime<Utc>,
    #[serde(with = "crate::time::json")]
    pub updated: DateTime<Utc>,
    #[serde(with = "crate::time::json::option")]
    pub last_used: Option<DateTime<Utc>>,
}

/// An environment kept in step with the vault: the daemon's own, in memory, where Ghost's
/// settings read their passwords. The vault adds and removes its values there; it never
/// touches the process environment.
pub type SharedEnviron = Arc<Mutex<HashMap<String, String>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Inner {
    entries: BTreeMap<String, Entry>,
    key: Option<VaultKey>,
    state: State,
    problem: Option<String>,
}

/// The vault, open in memory.
///
/// `environ`: an environment kept in step with the vault (the daemon passes the one its
/// own settings read); none for a tool that only edits the file.
///
/// More than one process may have the vault open (the daemon, `ghostd check` during a
/// deploy, `ghostd passwords`, `ghostd perf`), and the daemon changes it from more than
/// one thread (the import runs off the event loop). Every change holds the object's lock
/// and an `flock` on `passwords.vault.lock` beside the file, reads the file again, then
/// changes and saves, so no process writes over what another saved.
pub struct Passwords {
    path: PathBuf,
    keyring: Arc<dyn Keyring>,
    environ: Option<SharedEnviron>,
    inner: Mutex<Inner>,
}

impl fmt::Debug for Passwords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = lock(&self.inner);
        write!(
            f,
            "Passwords({}, {}, {} kept)",
            self.path.display(),
            inner.state,
            inner.entries.len()
        )
    }
}

impl Passwords {
    pub fn new(path: impl Into<PathBuf>, keyring: Arc<dyn Keyring>) -> Self {
        Self {
            path: path.into(),
            keyring,
            environ: None,
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                key: None,
                state: State::Locked,
                problem: Some("not opened yet".into()),
            }),
        }
    }

    /// The vault with an environment to keep in step.
    pub fn with_environ(mut self, environ: SharedEnviron) -> Self {
        self.environ = Some(environ);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn keyring(&self) -> Arc<dyn Keyring> {
        Arc::clone(&self.keyring)
    }

    pub fn state(&self) -> State {
        lock(&self.inner).state
    }

    /// Why the vault isn't open, when it isn't.
    pub fn problem(&self) -> Option<String> {
        lock(&self.inner).problem.clone()
    }

    // --- opening -----------------------------------------------------------------------

    pub fn is_open(&self) -> bool {
        self.state() == State::Open
    }

    /// Read the vault. True when it is open; a vault that doesn't exist yet is open and
    /// empty, and needs the keyring only when the first password is saved. On false,
    /// `state` and `problem` say why; nothing was read and nothing is written.
    pub fn open(&self) -> bool {
        let mut inner = lock(&self.inner);
        if !self.path.exists() {
            inner.entries.clear();
            return self.opened(&mut inner);
        }
        let key = match self.keyring.load_key() {
            Ok(Some(key)) => key,
            Ok(None) => {
                return self.closed(
                    &mut inner,
                    State::Broken,
                    "your keyring has no key for this vault".into(),
                );
            }
            Err(PasswordsError::KeyringLocked(_)) => {
                return self.closed(&mut inner, State::Locked, "Your keyring is locked".into());
            }
            Err(PasswordsError::KeyringUnavailable(e)) => {
                return self.closed(
                    &mut inner,
                    State::Unavailable,
                    format!("Ghost can't reach your keyring: {e}"),
                );
            }
            Err(e) => return self.closed(&mut inner, State::Broken, e.to_string()),
        };
        match self.read(&key) {
            Ok(entries) => {
                inner.entries = entries;
                inner.key = Some(key);
                self.opened(&mut inner)
            }
            Err(e) => self.closed(&mut inner, State::Broken, e.to_string()),
        }
    }

    /// Ask the keyring to unlock, then open. True when the vault is open.
    pub fn unlock(&self) -> bool {
        if let Err(e) = self.keyring.unlock() {
            // A dismissed prompt and the like.
            info!("passwords: the keyring didn't unlock ({})", e.kind());
        }
        self.open()
    }

    fn opened(&self, inner: &mut Inner) -> bool {
        inner.state = State::Open;
        inner.problem = None;
        self.publish(inner);
        true
    }

    fn closed(&self, inner: &mut Inner, state: State, problem: String) -> bool {
        if (state, Some(&problem)) != (inner.state, inner.problem.as_ref()) {
            // Once, not every retry.
            warn!("passwords: vault not open: {problem}");
        }
        inner.state = state;
        inner.problem = Some(problem);
        false
    }

    fn read(&self, key: &[u8]) -> Result<BTreeMap<String, Entry>, PasswordsError> {
        let raw = fs::read(&self.path)?;
        let payload = unseal(key, &raw)?;
        let mut out = BTreeMap::new();
        let Some(Value::Object(passwords)) = payload.get("passwords") else {
            return Ok(out);
        };
        for (name, raw) in passwords {
            let Some(raw) = raw.as_object() else {
                continue;
            };
            let Some(value) = raw.get("value").and_then(Value::as_str) else {
                continue;
            };
            let added = when(raw.get("added")).unwrap_or_else(utcnow);
            let used = when(raw.get("last_used"));
            out.insert(
                name.clone(),
                Entry {
                    value: Zeroizing::new(value.to_string()),
                    added,
                    updated: when(raw.get("updated")).unwrap_or(added),
                    last_used: used,
                    saved_use: used,
                },
            );
        }
        Ok(out)
    }

    // --- reading -----------------------------------------------------------------------

    pub fn contains(&self, name: &str) -> bool {
        lock(&self.inner).entries.contains_key(name)
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn names(&self) -> Vec<String> {
        lock(&self.inner).entries.keys().cloned().collect()
    }

    pub fn info(&self) -> Vec<PasswordMeta> {
        lock(&self.inner)
            .entries
            .iter()
            .map(|(n, e)| PasswordMeta {
                name: n.clone(),
                added: e.added,
                updated: e.updated,
                last_used: e.last_used,
            })
            .collect()
    }

    /// Whether the vault holds exactly `value` under `name`: a check (the import's
    /// read-back), not a use, and never the value itself. Constant-time.
    pub fn holds(&self, name: &str, value: &str) -> bool {
        let inner = lock(&self.inner);
        match inner.entries.get(name) {
            Some(e) => e.value.as_bytes().ct_eq(value.as_bytes()).into(),
            None => false,
        }
    }

    /// The value, at its point of use: an app's header or environment, a webhook's
    /// check. Notes the use.
    pub fn value(&self, name: &str) -> Option<Zeroizing<String>> {
        let mut inner = lock(&self.inner);
        let value = inner.entries.get(name)?.value.clone();
        self.note_used_locked(&mut inner, &[name]);
        Some(value)
    }

    /// Ghost filled these in just now. Kept for "last used"; written back now and then,
    /// over the file as it is now (another process may have changed it).
    pub fn note_used(&self, names: &[&str]) {
        let mut inner = lock(&self.inner);
        self.note_used_locked(&mut inner, names);
    }

    fn note_used_locked(&self, inner: &mut Inner, names: &[&str]) {
        let now = utcnow();
        let mut due = false;
        for name in names {
            let Some(e) = inner.entries.get_mut(*name) else {
                continue;
            };
            e.last_used = Some(now);
            if e.saved_use
                .is_none_or(|saved| now - saved >= LAST_USED_EVERY)
            {
                due = true;
            }
        }
        if !(due && inner.state == State::Open && inner.key.is_some()) {
            return;
        }
        let written = self
            .file_lock()
            .map_err(PasswordsError::from)
            .and_then(|_held| {
                self.refresh(inner)?;
                self.save(inner)
            });
        if let Err(e) = written {
            // "last used" is never worth a failure.
            debug!("passwords: last used not saved ({})", e.kind());
        }
    }

    /// An environment for `fill_secrets` and the webhook checks: the vault's values first
    /// (each read noted as a use), then `base` (the process environment when none) for
    /// the rest.
    pub fn environ(&self, base: Option<HashMap<String, String>>) -> VaultEnviron<'_> {
        let base = base.unwrap_or_else(|| {
            std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect()
        });
        VaultEnviron { vault: self, base }
    }

    // --- changing ----------------------------------------------------------------------

    pub fn set(&self, name: &str, value: &str) -> Result<(), PasswordsError> {
        self.set_many([(name, value)])
    }

    /// Save passwords, replacing any with the same name. One write for all of them.
    pub fn set_many<N: Into<String>, V: Into<String>>(
        &self,
        values: impl IntoIterator<Item = (N, V)>,
    ) -> Result<(), PasswordsError> {
        let values: Vec<(String, Zeroizing<String>)> = values
            .into_iter()
            .map(|(n, v)| (n.into(), Zeroizing::new(v.into())))
            .collect();
        for (name, value) in &values {
            check_name(name)?;
            check_value(value)?;
        }
        let mut inner = lock(&self.inner);
        self.require_open(&inner)?;
        {
            let _held = self.file_lock()?;
            self.refresh(&mut inner)?;
            let now = utcnow();
            for (name, value) in &values {
                match inner.entries.get_mut(name) {
                    None => {
                        inner.entries.insert(
                            name.clone(),
                            Entry {
                                value: value.clone(),
                                added: now,
                                updated: now,
                                last_used: None,
                                saved_use: None,
                            },
                        );
                    }
                    Some(e) if *e.value != **value => {
                        e.value = value.clone();
                        e.updated = now;
                    }
                    Some(_) => {}
                }
            }
            self.save(&mut inner)?;
        }
        if let Some(env) = &self.environ {
            let mut env = lock(env);
            for (name, value) in &values {
                env.insert(name.clone(), value.to_string());
            }
        }
        Ok(())
    }

    /// Forget passwords. Returns the names that were there.
    pub fn delete(&self, names: &[&str]) -> Result<Vec<String>, PasswordsError> {
        let mut inner = lock(&self.inner);
        self.require_open(&inner)?;
        let gone = {
            let _held = self.file_lock()?;
            self.refresh(&mut inner)?;
            let gone: Vec<String> = names
                .iter()
                .filter(|n| inner.entries.contains_key(**n))
                .map(|n| n.to_string())
                .collect();
            if gone.is_empty() {
                return Ok(gone);
            }
            for n in &gone {
                inner.entries.remove(n);
            }
            self.save(&mut inner)?;
            gone
        };
        if let Some(env) = &self.environ {
            let mut env = lock(env);
            for n in &gone {
                env.remove(n);
            }
        }
        Ok(gone)
    }

    fn require_open(&self, inner: &Inner) -> Result<(), PasswordsError> {
        if inner.state != State::Open {
            return Err(PasswordsError::VaultClosed(
                inner
                    .problem
                    .clone()
                    .unwrap_or_else(|| "the vault isn't open".into()),
            ));
        }
        Ok(())
    }

    /// An exclusive `flock` on `<vault>.lock`, held across a read, a change and a save,
    /// so two processes' changes don't write over each other. Released when dropped.
    fn file_lock(&self) -> std::io::Result<fs::File> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let lock_path = self
            .path
            .with_file_name(format!("{}.lock", name.unwrap_or_default()));
        if let Some(parent) = lock_path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;
        rustix::fs::flock(&file, FlockOperation::LockExclusive)?;
        Ok(file)
    }

    /// Read the file again before a change (under `file_lock`): another process may have
    /// written it since it was read here. The uses noted here are kept; a password
    /// another process forgot is forgotten here too.
    fn refresh(&self, inner: &mut Inner) -> Result<(), PasswordsError> {
        if !self.path.exists() {
            return Ok(());
        }
        let key = match inner.key.clone() {
            Some(key) => key,
            None => match self.keyring.load_key() {
                Ok(Some(key)) => key,
                Ok(None) => {
                    return Err(PasswordsError::VaultClosed(
                        "your keyring has no key for this vault".into(),
                    ));
                }
                Err(PasswordsError::KeyringLocked(_)) => {
                    return Err(PasswordsError::VaultClosed("Your keyring is locked".into()));
                }
                Err(e) => return Err(PasswordsError::VaultClosed(e.to_string())),
            },
        };
        let mut fresh = match self.read(&key) {
            Ok(fresh) => fresh,
            Err(PasswordsError::Io(e)) => return Err(PasswordsError::Io(e)),
            Err(e) => return Err(PasswordsError::VaultClosed(e.to_string())),
        };
        inner.key = Some(key);
        for (name, e) in fresh.iter_mut() {
            if let Some(mine) = inner.entries.get(name).and_then(|m| m.last_used)
                && e.last_used.is_none_or(|theirs| mine > theirs)
            {
                e.last_used = Some(mine);
            }
        }
        if let Some(env) = &self.environ {
            let mut env = lock(env);
            for name in inner.entries.keys() {
                if !fresh.contains_key(name) {
                    env.remove(name);
                }
            }
        }
        inner.entries = fresh;
        Ok(())
    }

    fn key_for_write(&self, inner: &mut Inner) -> Result<VaultKey, PasswordsError> {
        if let Some(key) = &inner.key {
            return Ok(key.clone());
        }
        let loaded = (|| -> Result<VaultKey, PasswordsError> {
            if let Some(key) = self.keyring.load_key()? {
                return Ok(key);
            }
            if self.path.exists() {
                // Never seal over a vault another key opens.
                return Err(PasswordsError::VaultClosed(
                    "your keyring has no key for this vault".into(),
                ));
            }
            let fresh: Zeroizing<[u8; 32]> = Zeroizing::new(Generate::generate());
            let key = Zeroizing::new(fresh.to_vec());
            self.keyring.store_key(&key)?;
            Ok(key)
        })();
        let key = match loaded {
            Ok(key) => key,
            Err(PasswordsError::KeyringLocked(_)) => {
                self.closed(inner, State::Locked, "Your keyring is locked".into());
                return Err(PasswordsError::VaultClosed("Your keyring is locked".into()));
            }
            Err(PasswordsError::KeyringUnavailable(e)) => {
                self.closed(
                    inner,
                    State::Unavailable,
                    format!("Ghost can't reach your keyring: {e}"),
                );
                return Err(PasswordsError::VaultClosed(e));
            }
            Err(e) => return Err(e),
        };
        inner.key = Some(key.clone());
        Ok(key)
    }

    fn save(&self, inner: &mut Inner) -> Result<(), PasswordsError> {
        let key = self.key_for_write(inner)?;
        let payload = StoredPayload {
            passwords: inner
                .entries
                .iter()
                .map(|(n, e)| (n.clone(), e.dump()))
                .collect(),
        };
        let sealed = seal(&key, &payload)?;
        drop(payload);
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut tmp = tempfile::Builder::new()
            .prefix(&format!(".{name}."))
            .permissions(fs::Permissions::from_mode(0o600))
            .tempfile_in(parent)?;
        tmp.as_file_mut().write_all(&sealed)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        for e in inner.entries.values_mut() {
            e.saved_use = e.last_used;
        }
        self.publish(inner);
        Ok(())
    }

    // --- what the rest of the process knows --------------------------------------------

    /// Every value into the redactor, and into the kept environment.
    fn publish(&self, inner: &Inner) {
        let values: Vec<(String, String)> = inner
            .entries
            .iter()
            .map(|(n, e)| (n.clone(), e.value.to_string()))
            .collect();
        if let Some(env) = &self.environ {
            let mut env = lock(env);
            for (name, value) in &values {
                env.insert(name.clone(), value.clone());
            }
        }
        redact::also_redact("vault", values);
    }
}

/// The mapping `environ` returns: the vault's values, each read noted as a use, then the
/// base environment for the rest.
pub struct VaultEnviron<'a> {
    vault: &'a Passwords,
    base: HashMap<String, String>,
}

impl VaultEnviron<'_> {
    /// The value under `key`: the vault's (a use) or the base environment's.
    pub fn get(&self, key: &str) -> Option<Zeroizing<String>> {
        if let Some(v) = self.vault.value(key) {
            return Some(v);
        }
        self.base.get(key).cloned().map(Zeroizing::new)
    }

    /// Whether `key` is there; asking isn't using.
    pub fn contains_key(&self, key: &str) -> bool {
        self.vault.contains(key) || self.base.contains_key(key)
    }

    /// The vault's names, then the base environment's that the vault doesn't hold.
    pub fn keys(&self) -> Vec<String> {
        let mut out = self.vault.names();
        let seen: std::collections::BTreeSet<&String> = out.iter().collect();
        let mut rest: Vec<String> = self
            .base
            .keys()
            .filter(|k| !seen.contains(k))
            .cloned()
            .collect();
        rest.sort();
        out.extend(rest);
        out
    }

    pub fn len(&self) -> usize {
        self.keys().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn check_name(name: &str) -> Result<(), PasswordsError> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some('A'..='Z'))
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        return Err(PasswordsError::Invalid(
            "a password's name is capital letters, digits and _, starting with a letter".into(),
        ));
    }
    Ok(())
}

pub fn check_value(value: &str) -> Result<(), PasswordsError> {
    if value.trim().is_empty() {
        return Err(PasswordsError::Invalid("a password can't be empty".into()));
    }
    if value.chars().count() > MAX_VALUE_CHARS {
        return Err(PasswordsError::Invalid(format!(
            "a password is at most {MAX_VALUE_CHARS} characters"
        )));
    }
    if value.contains('\0') || value.contains('\n') {
        return Err(PasswordsError::Invalid("a password is one line".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The vault (ADR 0036): sealed on disk, its key in the keyring, values only at the
    //! point of use, and every value redacted from what comes back. A fake keyring
    //! throughout: these tests never touch the operator's keyring or files.
    //!
    //! The redactor is process-global and the vault publishes under one source ("vault"),
    //! so the tests that read it back run one per process (`cargo nextest`).

    use super::*;
    use crate::redact::{child_env, redact_json, redact_text};
    use serde_json::json;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    const TOKEN: &str = "ghp_0123456789abcdefABCDEF";
    const OTHER: &str = "sk-ant-api03-zyxwvutsrqponm";

    fn vault_path(tmp: &TempDir) -> PathBuf {
        tmp.path().join("passwords.vault")
    }

    fn vault(
        tmp: &TempDir,
        keyring: Option<Arc<MemoryKeyring>>,
        environ: Option<SharedEnviron>,
    ) -> Passwords {
        let keyring = keyring.unwrap_or_default();
        let mut v = Passwords::new(vault_path(tmp), keyring);
        if let Some(env) = environ {
            v = v.with_environ(env);
        }
        assert!(v.open());
        v
    }

    fn env_map(env: &SharedEnviron) -> HashMap<String, String> {
        lock(env).clone()
    }

    #[test]
    fn test_a_new_vault_is_open_and_needs_the_keyring_only_to_save() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::locked());
        let v = Passwords::new(vault_path(&tmp), ring);
        assert!(v.open() && v.is_open() && v.is_empty());
        assert!(matches!(
            v.set("GITHUB_TOKEN", TOKEN),
            Err(PasswordsError::VaultClosed(_))
        ));
        assert_eq!(v.state(), State::Locked);
        assert!(!vault_path(&tmp).exists());
    }

    #[test]
    fn test_saved_sealed_and_read_back() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let v = vault(&tmp, Some(ring.clone()), None);
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        let path = vault_path(&tmp);
        let raw = fs::read(&path).unwrap();
        let raw_text = String::from_utf8_lossy(&raw);
        // Names are sealed too.
        assert!(!raw_text.contains(TOKEN) && !raw_text.contains("GITHUB_TOKEN"));
        let envelope: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(envelope["cipher"], "AES-256-GCM");
        assert_eq!(envelope["ghost_passwords"], 1);
        assert!(raw_text.ends_with("}\n"));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(ring.key().map(|k| k.len()), Some(32));

        let again = Passwords::new(&path, ring);
        assert!(again.open());
        assert_eq!(again.names(), vec!["GITHUB_TOKEN"]);
        assert_eq!(
            again.value("GITHUB_TOKEN").map(|v| v.to_string()),
            Some(TOKEN.into())
        );
    }

    #[test]
    fn test_a_locked_keyring_waits_and_a_wrong_key_never_overwrites() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        vault(&tmp, Some(ring.clone()), None)
            .set("GITHUB_TOKEN", TOKEN)
            .unwrap();
        let path = vault_path(&tmp);
        let before = fs::read(&path).unwrap();

        ring.set_locked(true);
        let locked = Passwords::new(&path, ring.clone());
        assert!(!locked.open());
        assert_eq!(locked.state(), State::Locked);
        assert_eq!(locked.problem().as_deref(), Some("Your keyring is locked"));
        assert!(locked.names().is_empty());
        ring.set_locked(false);
        assert!(locked.open());
        assert_eq!(locked.names(), vec!["GITHUB_TOKEN"]);

        let wrong = Passwords::new(&path, Arc::new(MemoryKeyring::with_key(&[1u8; 32])));
        assert!(!wrong.open());
        assert_eq!(wrong.state(), State::Broken);
        assert!(matches!(
            wrong.set("X_TOKEN", "a-value-long-enough"),
            Err(PasswordsError::VaultClosed(_))
        ));
        // The keyring lost the key.
        let gone = Passwords::new(&path, Arc::new(MemoryKeyring::new()));
        assert!(!gone.open());
        assert_eq!(gone.state(), State::Broken);
        assert_eq!(fs::read(&path).unwrap(), before);

        let absent = Passwords::new(&path, Arc::new(MemoryKeyring::unavailable()));
        assert!(!absent.open());
        assert_eq!(absent.state(), State::Unavailable);
    }

    #[test]
    fn test_unlock_asks_the_keyring_then_opens() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        vault(&tmp, Some(ring.clone()), None)
            .set("GITHUB_TOKEN", TOKEN)
            .unwrap();
        ring.set_locked(true);
        let v = Passwords::new(vault_path(&tmp), ring);
        assert!(!v.open());
        assert!(v.unlock() && v.is_open());
    }

    #[test]
    fn test_names_and_metadata_never_the_value() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        let info = v.info();
        assert_eq!(info.len(), 1);
        let meta = &info[0];
        assert_eq!(meta.name, "GITHUB_TOKEN");
        assert!(meta.last_used.is_none());
        assert!(!format!("{meta:?}").contains(TOKEN));
        assert!(!format!("{v:?}").contains(TOKEN));
        assert!(format!("{v:?}").contains("open, 1 kept"));
        let inner = lock(&v.inner);
        assert!(!format!("{:?}", inner.entries["GITHUB_TOKEN"]).contains(TOKEN));
    }

    #[test]
    fn test_replace_delete_and_the_environment_kept_in_step() {
        let tmp = TempDir::new().unwrap();
        let env: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
        let v = vault(&tmp, None, Some(env.clone()));
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        assert_eq!(
            env_map(&env),
            HashMap::from([("GITHUB_TOKEN".to_string(), TOKEN.to_string())])
        );
        v.set("GITHUB_TOKEN", OTHER).unwrap();
        assert_eq!(env_map(&env)["GITHUB_TOKEN"], OTHER);
        assert_eq!(
            v.delete(&["GITHUB_TOKEN", "NOPE"]).unwrap(),
            vec!["GITHUB_TOKEN"]
        );
        assert!(env_map(&env).is_empty() && v.names().is_empty());
    }

    #[test]
    fn test_bad_names_and_values_are_refused() {
        for (name, value) in [
            ("lower", TOKEN),
            ("1ABC", TOKEN),
            ("A", ""),
            ("A", "  "),
            ("A", "two\nlines"),
        ] {
            let tmp = TempDir::new().unwrap();
            assert!(
                matches!(
                    vault(&tmp, None, None).set(name, value),
                    Err(PasswordsError::Invalid(_))
                ),
                "{name}={value:?} was taken"
            );
        }
    }

    #[test]
    fn test_a_write_from_another_process_is_not_lost() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let daemon = vault(&tmp, Some(ring.clone()), None);
        daemon.set("GITHUB_TOKEN", TOKEN).unwrap();
        let cli = vault(&tmp, Some(ring.clone()), None);
        cli.set("HF_TOKEN", "hf_abcdefghijklmnop").unwrap();
        daemon.set("OPENROUTER_API_KEY", OTHER).unwrap();
        assert!(Passwords::new(vault_path(&tmp), ring).open());
        assert_eq!(
            daemon.names(),
            vec!["GITHUB_TOKEN", "HF_TOKEN", "OPENROUTER_API_KEY"]
        );
    }

    /// The daemon and a `ghostd check` or `ghostd passwords` each have the vault open. A
    /// deleted password stays deleted and a new one stays, whichever writes last.
    #[test]
    fn test_two_processes_never_write_over_each_others_changes() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let env: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
        let daemon = vault(&tmp, Some(ring.clone()), Some(env.clone()));
        daemon
            .set_many([("GITHUB_TOKEN", TOKEN), ("HF_TOKEN", "hf_abcdefghijklmnop")])
            .unwrap();
        let other = vault(&tmp, Some(ring.clone()), None);
        assert_eq!(other.names(), vec!["GITHUB_TOKEN", "HF_TOKEN"]);

        other.delete(&["HF_TOKEN"]).unwrap();
        other.set("OPENROUTER_API_KEY", OTHER).unwrap();
        // The daemon fills in GITHUB_TOKEN: "last used" is written back over the file as
        // it is now, not over the daemon's stale copy.
        assert_eq!(
            daemon.value("GITHUB_TOKEN").map(|v| v.to_string()),
            Some(TOKEN.into())
        );
        let fresh = vault(&tmp, Some(ring.clone()), None);
        assert_eq!(fresh.names(), vec!["GITHUB_TOKEN", "OPENROUTER_API_KEY"]);
        assert!(fresh.info()[0].last_used.is_some());
        assert_eq!(daemon.names(), vec!["GITHUB_TOKEN", "OPENROUTER_API_KEY"]);
        assert!(!env_map(&env).contains_key("HF_TOKEN")); // forgotten here too

        // A change made by the daemon keeps the other's, and the other way round.
        daemon
            .set("ELEVENLABS_API_KEY", "el-0123456789abcdef")
            .unwrap();
        other.delete(&["GITHUB_TOKEN"]).unwrap();
        assert_eq!(
            vault(&tmp, Some(ring), None).names(),
            vec!["ELEVENLABS_API_KEY", "OPENROUTER_API_KEY"]
        );
    }

    #[test]
    fn test_changes_from_many_threads_and_two_instances_all_land() {
        let tmp = TempDir::new().unwrap();
        let ring = Arc::new(MemoryKeyring::new());
        let a = vault(&tmp, Some(ring.clone()), None);
        let b = vault(&tmp, Some(ring.clone()), None);
        a.set("SEED_TOKEN", TOKEN).unwrap();

        fn work(v: &Passwords, prefix: &str) {
            for i in 0..15 {
                v.set(
                    &format!("{prefix}_{i}_TOKEN"),
                    &format!("value-{prefix}-{i}-0123456789"),
                )
                .unwrap();
                v.note_used(&["SEED_TOKEN"]);
            }
        }

        std::thread::scope(|s| {
            s.spawn(|| work(&a, "A"));
            s.spawn(|| work(&b, "B"));
            s.spawn(|| work(&a, "C"));
        });
        let names = vault(&tmp, Some(ring), None).names();
        assert_eq!(names.len(), 46);
        assert!(names.iter().any(|n| n == "SEED_TOKEN"));
        assert!(tmp.path().join("passwords.vault.lock").exists());
    }

    #[test]
    fn test_use_is_noted_through_the_environ_mapping() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        let env = v.environ(Some(HashMap::from([(
            "PATH".to_string(),
            "/bin".to_string(),
        )])));
        assert!(env.contains_key("GITHUB_TOKEN") && env.contains_key("PATH"));
        assert!(!env.contains_key("NOPE"));
        assert_eq!(env.keys(), vec!["GITHUB_TOKEN", "PATH"]);
        assert!(v.info()[0].last_used.is_none()); // asking isn't using
        // `apps.fill_secrets` isn't ported yet; it reads the mapping the way this does.
        assert_eq!(
            env.get("GITHUB_TOKEN").map(|t| format!("Bearer {}", *t)),
            Some(format!("Bearer {TOKEN}"))
        );
        assert!(v.info()[0].last_used.is_some());
        let reread = Passwords::new(vault_path(&tmp), v.keyring());
        assert!(reread.open() && reread.info()[0].last_used.is_some()); // written back
        assert!(env.get("NOPE").is_none());
        assert_eq!(env.get("PATH").map(|p| p.to_string()), Some("/bin".into()));
    }

    #[test]
    fn test_redaction_replaces_every_value_in_nested_results() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set_many([
            ("GITHUB_TOKEN", TOKEN.to_string()),
            ("APP_KEY", format!("Bearer {OTHER}")),
            ("PIN", "1234".to_string()),
        ])
        .unwrap();
        let result = json!({
            "text": format!("your token is {TOKEN}."),
            "items": [{"auth": OTHER}, [TOKEN]],
            TOKEN: "as a key",
            "n": 3,
        });
        let out = redact_json(&result);
        assert_eq!(out["text"], "your token is [password: GITHUB_TOKEN].");
        // The token inside "Bearer …".
        assert_eq!(out["items"][0]["auth"], "[password: APP_KEY]");
        assert_eq!(out["items"][1], json!(["[password: GITHUB_TOKEN]"]));
        assert_eq!(out["[password: GITHUB_TOKEN]"], "as a key");
        assert_eq!(out["n"], 3);
        // A value shorter than the minimum isn't looked for: page 1234 stays page 1234.
        assert_eq!(redact_text("page 1234"), "page 1234");
        assert!(!serde_json::to_string(&out).unwrap().contains(TOKEN));
    }

    #[test]
    fn test_a_value_inside_another_is_replaced_whole() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set_many([
            ("SHORT_KEY", "abcdefgh12"),
            ("LONG_KEY", "abcdefgh12345678"),
        ])
        .unwrap();
        assert_eq!(
            redact_text("x abcdefgh12345678 y abcdefgh12"),
            "x [password: LONG_KEY] y [password: SHORT_KEY]"
        );
    }

    #[test]
    fn test_deleted_passwords_stop_being_redacted() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        v.delete(&["GITHUB_TOKEN"]).unwrap();
        assert_eq!(redact_text(TOKEN), TOKEN);
    }

    #[test]
    fn test_child_env_leaves_every_password_out() {
        let tmp = TempDir::new().unwrap();
        let v = vault(&tmp, None, None);
        v.set("GITHUB_TOKEN", TOKEN).unwrap();
        redact::also_redact(
            "legacy",
            [("OLD_TOKEN".to_string(), "legacy-value-1234".to_string())],
        );
        let env = child_env([
            ("PATH".to_string(), "/bin".to_string()),
            ("GITHUB_TOKEN".to_string(), TOKEN.to_string()),
            ("OLD_TOKEN".to_string(), "x".to_string()),
            ("HOME".to_string(), "/h".to_string()),
        ]);
        assert_eq!(
            env,
            vec![
                ("PATH".to_string(), "/bin".to_string()),
                ("HOME".to_string(), "/h".to_string())
            ]
        );
        assert_eq!(redact_text("legacy-value-1234"), "[password: OLD_TOKEN]");
    }

    #[test]
    fn test_which_names_are_passwords() {
        for (name, expected) in [
            ("ANTHROPIC_AUTH_TOKEN", true),
            ("GHOST_APP_MOBBIN_KEY", true),
            ("MCP_PROXY_KEY", true),
            ("SOME_PASSWORD", true),
            ("SMTP_PASS", true),
            ("DB_PASSWD", true),
            ("GOOGLE_CREDENTIALS", true),
            ("openai_api_key", true),
            ("Github_Token", true),
            ("ghost_app_x_key", true),
            ("ghost_llm_model", false),
            ("ANTHROPIC_BASE_URL", false),
            ("GHOST_LLM_MODEL", false),
            ("GHOST_JEV", false),
            ("CUSTOM", false),
        ] {
            assert_eq!(is_password_name(name, &[]), expected, "{name}");
            assert!(is_password_name("CUSTOM", &["CUSTOM"]));
            assert!(is_password_name("custom", &["CUSTOM"]));
        }
    }

    #[test]
    fn a_vault_the_python_daemon_wrote_opens() {
        // The envelope and payload the Python daemon writes, sealed here with its layout
        // and read back through `unseal`; and ours parses as JSON with the same fields.
        let key = [7u8; 32];
        let sealed = seal(&key, &json!({"passwords": {"A_TOKEN": {"value": "v", "added": "2026-10-28T20:40:26.093494+00:00", "updated": "2026-10-28T20:40:26.093494+00:00", "last_used": null}}})).unwrap();
        let text = String::from_utf8(sealed.clone()).unwrap();
        assert!(
            text.starts_with("{\"ghost_passwords\": 1, \"cipher\": \"AES-256-GCM\", \"nonce\": \"")
        );
        assert!(text.ends_with("\"}\n"));
        let payload = unseal(&key, &sealed).unwrap();
        assert_eq!(payload["passwords"]["A_TOKEN"]["value"], "v");
        assert!(matches!(
            unseal(&[8u8; 32], &sealed),
            Err(PasswordsError::VaultBroken(m)) if m == "the key in your keyring doesn't open this vault"
        ));
        assert!(matches!(
            unseal(&key, b"{\"ghost_passwords\": 2}"),
            Err(PasswordsError::VaultBroken(m)) if m == "not a vault Ghost can read"
        ));
        assert!(matches!(
            unseal(&key, b"nonsense"),
            Err(PasswordsError::VaultBroken(m)) if m == "the vault file is damaged"
        ));
    }
}
