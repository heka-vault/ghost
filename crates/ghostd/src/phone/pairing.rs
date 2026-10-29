//! The one paired phone and the pairing code that admits it (ADR 0016).
//!
//! The device record, secret included, lives in `phone.json` under the data dir, mode
//! 0600. The pairing code lives only in memory: six digits, 120 s, five tries, single use.
//! Neither the secret nor the code is ever logged.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use ghost_api::models::PhoneNetwork;
use ghost_core::time::{iso, parse_iso, utcnow};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use super::networks::{network_str, parse_network};
use super::signing::{Secret, new_secret};
use super::{Clock, mono};

pub const CODE_TTL_SECS: f64 = 120.0;
pub const CODE_TRIES: u32 = 5;
/// Write last_seen to disk at most this often.
pub const LAST_SEEN_PERSIST_SECS: f64 = 60.0;
pub const NETWORKS: [&str; 2] = ["home", "tunnel"];

/// Wall-clock time, for `paired_at` and `last_seen`.
pub type WallClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

fn now() -> WallClock {
    Arc::new(utcnow)
}

#[derive(Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub secret: Secret,
    pub paired_at: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
    /// The listen address the last verified request came in on, and its network (ADR 0020).
    pub last_address: Option<String>,
    pub last_network: Option<PhoneNetwork>,
}

impl fmt::Debug for Device {
    /// Never print the secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Device(id={:?}, name={:?})", self.id, self.name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("ValueError")]
    Json(#[from] serde_json::Error),
    #[error("KeyError")]
    Missing(&'static str),
    #[error("TypeError")]
    Type(&'static str),
    #[error("ValueError")]
    Value(&'static str),
}

impl DeviceError {
    /// The Python exception's name, which is what the log line says.
    fn kind(&self) -> &'static str {
        match self {
            DeviceError::Json(_) | DeviceError::Value(_) => "ValueError",
            DeviceError::Missing(_) => "KeyError",
            DeviceError::Type(_) => "TypeError",
        }
    }
}

impl Device {
    /// The file's text: the fields in the record's order, two-space indented, as Python's
    /// `json.dump(indent=2)` wrote them.
    pub fn to_json(&self) -> String {
        fn s(v: &str) -> String {
            serde_json::to_string(v).unwrap_or_default()
        }
        fn opt(v: Option<&str>) -> String {
            v.map_or_else(|| "null".to_string(), s)
        }
        let last_seen = self.last_seen.as_ref().map(iso);
        format!(
            "{{\n  \"id\": {},\n  \"name\": {},\n  \"secret\": {},\n  \"paired_at\": {},\n  \"last_seen\": {},\n  \"last_address\": {},\n  \"last_network\": {}\n}}",
            s(&self.id),
            s(&self.name),
            s(self.secret.as_str()),
            s(&iso(&self.paired_at)),
            opt(last_seen.as_deref()),
            opt(self.last_address.as_deref()),
            opt(self.last_network.map(network_str)),
        )
    }

    pub fn from_json(text: &str) -> Result<Self, DeviceError> {
        let d: Value = serde_json::from_str(text)?;
        let obj = d.as_object().ok_or(DeviceError::Type("not an object"))?;
        let text_of = |key: &'static str| -> Result<String, DeviceError> {
            let v = obj.get(key).ok_or(DeviceError::Missing(key))?;
            match v {
                Value::String(s) => Ok(s.clone()),
                Value::Null => Err(DeviceError::Type(key)),
                other => Ok(other.to_string()),
            }
        };
        let secret = Secret::parse(&text_of("secret")?).ok_or(DeviceError::Value("secret"))?;
        let paired_at = parse_iso(&text_of("paired_at")?).ok_or(DeviceError::Value("paired_at"))?;
        let last_seen = match obj.get("last_seen") {
            Some(Value::String(s)) if !s.is_empty() => {
                Some(parse_iso(s).ok_or(DeviceError::Value("last_seen"))?)
            }
            _ => None,
        };
        let last_address = match obj.get("last_address") {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        let last_network = obj
            .get("last_network")
            .and_then(Value::as_str)
            .and_then(parse_network);
        Ok(Self {
            id: text_of("id")?,
            name: text_of("name")?,
            secret,
            paired_at,
            last_seen,
            last_address,
            last_network,
        })
    }
}

struct StoreState {
    device: Option<Device>,
    persisted_seen: f64,
}

/// `phone.json`: at most one device. Pairing again replaces it.
pub struct PhoneStore {
    path: PathBuf,
    clock: WallClock,
    monotonic: Clock,
    state: Mutex<StoreState>,
}

impl fmt::Debug for PhoneStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhoneStore({})", self.path.display())
    }
}

impl PhoneStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_clocks(path, now(), mono())
    }

    pub fn with_clocks(path: impl Into<PathBuf>, clock: WallClock, monotonic: Clock) -> Self {
        let path = path.into();
        let device = Self::load(&path);
        let persisted_seen = monotonic();
        Self {
            path,
            clock,
            monotonic,
            state: Mutex::new(StoreState {
                device,
                persisted_seen,
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(path: &Path) -> Option<Device> {
        if !path.exists() {
            return None;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                warn!("phone: ignoring unreadable {}: OSError", path.display());
                return None;
            }
        };
        match Device::from_json(&text) {
            Ok(d) => Some(d),
            Err(e) => {
                warn!(
                    "phone: ignoring unreadable {}: {}",
                    path.display(),
                    e.kind()
                );
                None
            }
        }
    }

    fn save(&self, device: Option<&Device>) {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

        if let Some(parent) = self.path.parent() {
            let _ = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent);
        }
        let Some(device) = device else {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!("phone: could not remove {}: {e}", self.path.display()),
            }
            return;
        };
        let tmp = self.path.with_extension("tmp");
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| {
                f.write_all(device.to_json().as_bytes())?;
                f.flush()
            })
            .and_then(|()| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            warn!("phone: could not write {}: {e}", self.path.display());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn device(&self) -> Option<Device> {
        self.lock().device.clone()
    }

    pub fn secret_for(&self, device_id: &str) -> Option<Secret> {
        let state = self.lock();
        let d = state.device.as_ref()?;
        let same: bool = d.id.as_bytes().ct_eq(device_id.as_bytes()).into();
        if !same {
            return None;
        }
        Some(d.secret.clone())
    }

    pub fn pair(&self, name: &str) -> Device {
        let mut raw = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut raw);
        let device = Device {
            id: hex::encode(raw),
            name: name.to_string(),
            secret: new_secret(),
            paired_at: (self.clock)(),
            last_seen: None,
            last_address: None,
            last_network: None,
        };
        let mut state = self.lock();
        state.device = Some(device.clone());
        self.save(state.device.as_ref());
        info!("phone: paired {:?} ({})", name, device.id);
        device
    }

    pub fn forget(&self) -> bool {
        let mut state = self.lock();
        let had = state.device.is_some();
        state.device = None;
        self.save(None);
        if had {
            info!("phone: forgot the paired phone");
        }
        had
    }

    /// Note a verified request, the listen address it came in on and that address's
    /// network. Kept in memory and written at most once a minute, or at once when the
    /// phone moved between addresses (home to tunnel, or back).
    pub fn seen(&self, address: Option<&str>, network: Option<PhoneNetwork>) {
        let mut state = self.lock();
        let Some(d) = state.device.as_mut() else {
            return;
        };
        let moved = (address, network) != (d.last_address.as_deref(), d.last_network);
        d.last_seen = Some((self.clock)());
        d.last_address = address.map(String::from);
        d.last_network = network;
        let now = (self.monotonic)();
        if moved || now - state.persisted_seen >= LAST_SEEN_PERSIST_SECS {
            state.persisted_seen = now;
            self.save(state.device.as_ref());
        }
    }

    pub fn flush(&self) {
        let state = self.lock();
        if state.device.is_some() {
            self.save(state.device.as_ref());
        }
    }
}

struct Code {
    code: String,
    /// Monotonic.
    expires: f64,
    tries: u32,
}

/// At most one live code. `mint` replaces it; `redeem` spends it on success, and a
/// fifth wrong try spends it too.
pub struct PairingCodes {
    ttl: f64,
    tries: u32,
    clock: Clock,
    wall: WallClock,
    code: Option<Code>,
}

impl Default for PairingCodes {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingCodes {
    pub fn new() -> Self {
        Self {
            ttl: CODE_TTL_SECS,
            tries: CODE_TRIES,
            clock: mono(),
            wall: now(),
            code: None,
        }
    }

    pub fn with_ttl(mut self, ttl: f64) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn with_tries(mut self, tries: u32) -> Self {
        self.tries = tries;
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// A fresh six-digit code and when it expires.
    pub fn mint(&mut self) -> (String, DateTime<Utc>) {
        let code = format!("{:06}", rand::random_range(0..1_000_000u32));
        let expires_at = (self.wall)() + Duration::milliseconds((self.ttl * 1000.0) as i64);
        self.code = Some(Code {
            code: code.clone(),
            expires: (self.clock)() + self.ttl,
            tries: 0,
        });
        (code, expires_at)
    }

    pub fn redeem(&mut self, code: &str) -> bool {
        let Some(c) = self.code.as_mut() else {
            return false;
        };
        if (self.clock)() >= c.expires {
            self.code = None;
            return false;
        }
        let right: bool = c.code.as_bytes().ct_eq(code.as_bytes()).into();
        if right {
            self.code = None;
            return true;
        }
        c.tries += 1;
        if c.tries >= self.tries {
            self.code = None;
        }
        false
    }

    pub fn active(&self) -> bool {
        self.code
            .as_ref()
            .is_some_and(|c| (self.clock)() < c.expires)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_round_trips_in_pythons_shape() {
        let device = Device {
            id: "0011223344556677".into(),
            name: "Pixel 9".into(),
            secret: Secret::new(),
            paired_at: utcnow(),
            last_seen: None,
            last_address: None,
            last_network: None,
        };
        let text = device.to_json();
        assert!(text.starts_with("{\n  \"id\": \"0011223344556677\",\n  \"name\": \"Pixel 9\","));
        assert!(text.contains("\"last_network\": null\n}"));
        let back = Device::from_json(&text).unwrap();
        assert_eq!(back, device);
        assert_eq!(
            format!("{back:?}"),
            "Device(id=\"0011223344556677\", name=\"Pixel 9\")"
        );
        assert!(!format!("{back:?}").contains(device.secret.as_str()));

        let seen = Device {
            last_seen: Some(utcnow()),
            last_address: Some("10.8.0.4:8765".into()),
            last_network: Some(PhoneNetwork::Tunnel),
            ..device
        };
        let back = Device::from_json(&seen.to_json()).unwrap();
        assert_eq!(back.last_network, Some(PhoneNetwork::Tunnel));
        assert_eq!(back.last_address.as_deref(), Some("10.8.0.4:8765"));
        assert_eq!(back.last_seen, seen.last_seen);
        // A network the file doesn't know is none.
        let odd = seen.to_json().replace("\"tunnel\"", "\"mesh\"");
        assert_eq!(Device::from_json(&odd).unwrap().last_network, None);
        assert!(Device::from_json("{}").is_err());
        assert!(Device::from_json("[]").is_err());
    }

    #[test]
    fn an_unreadable_file_is_ignored_and_pairing_writes_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phone.json");
        std::fs::write(&path, "{").unwrap();
        let store = PhoneStore::new(&path);
        assert!(store.device().is_none());
        let d = store.pair("p");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(store.secret_for(&d.id).is_some());
        assert!(store.secret_for("nope").is_none());
        assert!(store.forget());
        assert!(!path.exists());
        assert!(!store.forget());
        store.seen(Some("a:1"), None); // no device: nothing to note
        assert!(store.device().is_none());
    }

    #[test]
    fn last_seen_is_written_on_the_minute_or_on_a_move() {
        let t = Arc::new(Mutex::new(1000.0));
        let clock: Clock = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap())
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phone.json");
        let store = PhoneStore::with_clocks(&path, now(), clock);
        store.pair("p");
        store.seen(Some("127.0.0.1:0"), Some(PhoneNetwork::Home));
        let saved = PhoneStore::new(&path).device().unwrap();
        assert_eq!(saved.last_address.as_deref(), Some("127.0.0.1:0")); // the first is a move
        *t.lock().unwrap() += 10.0;
        store.seen(Some("127.0.0.1:0"), Some(PhoneNetwork::Home));
        let again = PhoneStore::new(&path).device().unwrap();
        assert_eq!(again.last_seen, saved.last_seen); // within the minute: memory only
        assert_ne!(store.device().unwrap().last_seen, saved.last_seen);
        *t.lock().unwrap() += 60.0;
        store.seen(Some("127.0.0.1:0"), Some(PhoneNetwork::Home));
        assert_ne!(
            PhoneStore::new(&path).device().unwrap().last_seen,
            saved.last_seen
        );
        store.seen(Some("127.0.0.2:0"), Some(PhoneNetwork::Tunnel));
        assert_eq!(
            PhoneStore::new(&path).device().unwrap().last_network,
            Some(PhoneNetwork::Tunnel)
        );
        store.flush();
    }

    #[test]
    fn a_code_is_six_digits_and_spent_by_tries_or_time() {
        let t = Arc::new(Mutex::new(1000.0));
        let clock: Clock = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap())
        };
        let mut codes = PairingCodes::new().with_clock(clock);
        assert!(!codes.active());
        let (code, _) = codes.mint();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(codes.active());
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        for _ in 0..4 {
            assert!(!codes.redeem(&wrong));
        }
        assert!(codes.active());
        assert!(!codes.redeem(&wrong)); // the fifth spends it
        assert!(!codes.active());
        assert!(!codes.redeem(&code));
        let (code, _) = codes.mint();
        *t.lock().unwrap() += 121.0;
        assert!(!codes.active());
        assert!(!codes.redeem(&code));
    }
}
