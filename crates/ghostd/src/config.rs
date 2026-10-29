//! Every `GHOST_*` setting the daemon reads (`ghostd.config.GhostdSettings`): the same
//! names, defaults and derived values, read from the environment once at start. A setting
//! that doesn't parse is said by name; the two Jev keys are read as they are and never
//! appear in an error.

use std::collections::HashMap;
use std::path::PathBuf;

use thiserror::Error;

/// Every setting but the two Jev keys is read under this prefix.
pub const ENV_PREFIX: &str = "GHOST_";

/// `$XDG_RUNTIME_DIR/ghost` (`/tmp/ghost` without one), made 0700 if it isn't there.
pub fn runtime_dir() -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;

    let base =
        std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let dir = base.join("ghost");
    let _ = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir);
    dir
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Where the PipeWire installer and unit templates live: the repo's `infra/` for a binary
/// built in a checkout (found above the executable), `/usr/share/heka-ghost` otherwise.
fn default_infra_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            let infra = dir.join("infra");
            if infra.is_dir() && dir.join("Cargo.toml").is_file() {
                return infra;
            }
        }
    }
    PathBuf::from("/usr/share/heka-ghost")
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConfigError {
    #[error("{name} must be {kind}, got {value:?}")]
    Invalid {
        name: String,
        kind: &'static str,
        value: String,
    },
}

/// The daemon's settings. Field docs are the Python module's.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostdSettings {
    pub data_dir: PathBuf,

    /// Autonomy (ADR 0011): the operator's per-operation levels. Missing file = every
    /// operation falls back to its declared risk (labels/comments announce, close
    /// confirms). Template: infra/policy.example.toml, seeded by `make env`.
    pub policy_path: PathBuf,

    /// Confirmation classifier. auto: Jev when TYPESAFE_API_KEY or OPENROUTER_API_KEY is
    /// set, else the regex allowlist; on: Jev or fail at startup; off: regex only.
    pub jev: String,
    pub jev_threshold: f64,
    pub jev_deadline_secs: f64,
    /// `TYPESAFE_API_KEY`, no prefix.
    pub typesafe_api_key: String,
    /// `OPENROUTER_API_KEY`, no prefix.
    pub openrouter_api_key: String,

    // State
    pub db_path: PathBuf,
    /// Per-turn latency records (ghost_voice.latency), timings only; rotated at
    /// GHOST_LATENCY_MAX_MB. Default: data_dir/latency.jsonl. `ghostd latency` summarizes.
    pub latency_log: Option<PathBuf>,

    /// Ghost's folder (ADR 0035): everything the operator edits lives under config_dir:
    /// AGENTS.md, apps.json, watches.json, hooks.json, skills/, out/, and every agent's
    /// agent.json, persona.md, memory.md and notes.md under agents/<slug>/. The transcript
    /// and the registry mirror are in the database.
    pub config_dir: PathBuf,
    /// Apps (ADR 0034): a sign-in's browser comes back to 127.0.0.1 on this port, only while
    /// a sign-in is open. Fixed, because an app's registration names it exactly.
    pub signin_port: u16,
    /// The catalog's copy of the app registry, refreshed at most hourly.
    pub catalog_path: PathBuf,
    /// Skills (ADR 0023, ADR 0035): the operator's SKILL.md folders, then Ghost's own; the
    /// first folder with a name wins.
    pub skills_dir: PathBuf,
    pub home_skills_dir: PathBuf,

    /// Summarized turns older than the retention window are pruned daily.
    pub turn_retention_days: i64,

    /// Control API (ADR 0013): a user-owned Unix socket; no TCP. The desktop app, the
    /// CLI and tests talk to it. Empty disables it. Default: $XDG_RUNTIME_DIR/ghost/api.sock.
    pub api_socket: Option<PathBuf>,
    pub settings_path: PathBuf,
    pub env_path: PathBuf,
    pub vault_env_path: PathBuf,
    /// Where the PipeWire installer and unit templates live: the repo's infra/ in a
    /// checkout, /usr/share/heka-ghost in the Debian package.
    pub infra_dir: PathBuf,

    /// Phone listener (ADR 0016): plain HTTP with signed requests on one LAN address,
    /// host:port (e.g. 192.168.1.20:8765). Empty = off. Not the control API. The paired
    /// device and its secret are in data_dir/phone.json (0600). With ufw on, allow this
    /// TCP port and UDP from the LAN (aiortc picks random UDP ports).
    pub phone_listen: String,

    /// Webhook listener (ADR 0027): POST /hooks/<name> only, for the hooks hooks.json
    /// declares (ADR 0035); host:port, comma separated. Empty = off. Every delivery is
    /// signed, parsed by declaration and checked at the source before it is an item.
    pub hooks_listen: String,

    /// Wake. transcript: local VAD + Parakeet while passive, looking for a phrase
    /// (ADR 0010); onnx: the trained "hey ghost" detector, that phrase only, with no
    /// transcript fallback (ADR 0025, ghost_voice.wakeword); file: `ghostd wake` /
    /// SIGUSR1 only. `ghostd wake` works in every mode.
    pub wake_mode: String,
    /// Comma-separated; empty = ghost_voice.wake.DEFAULT_WAKE_PHRASES.
    pub wake_phrases: String,
    /// Phonetic near-miss threshold for the wake phrase; 0 = exact only. 0.8 also took "he
    /// set off", "her guests" and "hey guys" (2.1 false wakes an hour of read speech); 0.9
    /// had none and kept the "he ghost" spellings (docs/benchmarks/2026-10-21-wake.md).
    pub wake_fuzzy: f64,
    /// onnx mode. The model; None: hey_ghost.onnx under the voice cache's wake/ folder.
    pub wake_model_path: Option<PathBuf>,
    /// The detector's score (0-1) that counts as hearing "hey ghost". Unset: 0.3 with the
    /// check on, where it only decides how often Parakeet runs (0.1 to 0.3 woke on the
    /// same takes); 0.7 with it off, where it is the whole decision: every "hey ghost" at
    /// the desk and no false wakes in 6 h of read speech, where 0.5 had two
    /// (docs/benchmarks/2026-10-22-wake-detector.md).
    pub wake_threshold: Option<f64>,
    /// Parakeet confirms each hit (match_wake at wake_fuzzy) before Ghost wakes; false:
    /// the detector alone decides, and the chime plays the moment it fires.
    pub wake_verify: bool,
    pub wake_cooldown_secs: f64,

    /// Models for addressing Ghost by name with a request ("Ghost, let's work on …"): this
    /// file, or None for every ghost_*.onnx beside hey_ghost.onnx (one per opener).
    pub wake_address_model_path: Option<PathBuf>,
    /// Parakeet checks their hits; unset: as wake_verify. The check lets them fire at a
    /// low score: on the replay the per-opener models heard far more at 0.3 with it than
    /// at 0.7 without it, and it turned down every false hit, so "hey ghost" can wake at
    /// once while "Ghost, …" is checked (docs/benchmarks/2026-10-23-ghost-openers.md).
    pub wake_address_verify: Option<bool>,
    /// Their score that counts as a hit. Unset: 0.3 when they are checked, 0.7 when not.
    pub wake_address_threshold: Option<f64>,

    // Audio
    /// Matched against sink-input application names.
    pub duck_apps: Vec<String>,
    pub duck_level: f64,
    pub chime: bool,

    /// Coexist with nerd-dictation (F9): its toggle script writes this file while active.
    pub dictation_flag: PathBuf,
}

impl Default for GhostdSettings {
    fn default() -> Self {
        let home = home();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        Self {
            data_dir: home.join(".local/share/ghost"),
            policy_path: home.join(".config/ghost/policy.toml"),
            jev: "auto".into(),
            jev_threshold: 0.95,
            jev_deadline_secs: 0.5,
            typesafe_api_key: String::new(),
            openrouter_api_key: String::new(),
            db_path: home.join(".local/share/ghost/ghost.db"),
            latency_log: None,
            config_dir: home.join(".config/ghost"),
            signin_port: 47823,
            catalog_path: home.join(".cache/ghost/catalog.json"),
            skills_dir: home.join(".config/ghost/skills"),
            home_skills_dir: home.join(".agents/skills"),
            turn_retention_days: 90,
            api_socket: None,
            settings_path: home.join(".config/ghost/settings.env"),
            env_path: home.join(".config/ghost/env"),
            vault_env_path: home.join(".config/heka-vault/ghost.env"),
            infra_dir: default_infra_dir(),
            phone_listen: String::new(),
            hooks_listen: String::new(),
            wake_mode: "transcript".into(),
            wake_phrases: String::new(),
            wake_fuzzy: 0.9,
            wake_model_path: None,
            wake_threshold: None,
            wake_verify: true,
            wake_cooldown_secs: 2.0,
            wake_address_model_path: None,
            wake_address_verify: None,
            wake_address_threshold: None,
            duck_apps: vec!["spotify".into()],
            duck_level: 0.25,
            chime: true,
            dictation_flag: runtime.join("dictation-audio-duck.state"),
        }
    }
}

/// The environment, keys uppercased: pydantic-settings matched them without regard to case.
struct Env(HashMap<String, String>);

impl Env {
    fn new(env: &HashMap<String, String>) -> Self {
        Self(
            env.iter()
                .map(|(k, v)| (k.to_uppercase(), v.clone()))
                .collect(),
        )
    }

    fn raw(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    fn string(&self, name: &str, into: &mut String) {
        if let Some(v) = self.raw(name) {
            *into = v.to_string();
        }
    }

    fn path(&self, name: &str, into: &mut PathBuf) {
        if let Some(v) = self.raw(name) {
            *into = PathBuf::from(v);
        }
    }

    /// An empty value is "unset" for an optional path: "Empty disables it".
    fn opt_path(&self, name: &str, into: &mut Option<PathBuf>) {
        if let Some(v) = self.raw(name) {
            *into = (!v.trim().is_empty()).then(|| PathBuf::from(v));
        }
    }

    fn float(&self, name: &str, into: &mut f64) -> Result<(), ConfigError> {
        if let Some(v) = self.raw(name) {
            *into = parse_float(name, v)?;
        }
        Ok(())
    }

    fn opt_float(&self, name: &str, into: &mut Option<f64>) -> Result<(), ConfigError> {
        if let Some(v) = self.raw(name) {
            *into = if v.trim().is_empty() {
                None
            } else {
                Some(parse_float(name, v)?)
            };
        }
        Ok(())
    }

    fn bool(&self, name: &str, into: &mut bool) -> Result<(), ConfigError> {
        if let Some(v) = self.raw(name) {
            *into = parse_bool(name, v)?;
        }
        Ok(())
    }

    fn opt_bool(&self, name: &str, into: &mut Option<bool>) -> Result<(), ConfigError> {
        if let Some(v) = self.raw(name) {
            *into = if v.trim().is_empty() {
                None
            } else {
                Some(parse_bool(name, v)?)
            };
        }
        Ok(())
    }

    fn int<T: std::str::FromStr>(&self, name: &str, into: &mut T) -> Result<(), ConfigError> {
        if let Some(v) = self.raw(name) {
            *into = v.trim().parse().map_err(|_| ConfigError::Invalid {
                name: name.into(),
                kind: "a whole number",
                value: v.into(),
            })?;
        }
        Ok(())
    }

    /// A list, as pydantic-settings read it (a JSON array); a comma-separated list is
    /// taken too, so `GHOST_DUCK_APPS=spotify,firefox` works as the name suggests.
    fn list(&self, name: &str, into: &mut Vec<String>) -> Result<(), ConfigError> {
        let Some(v) = self.raw(name) else {
            return Ok(());
        };
        let text = v.trim();
        if text.starts_with('[') {
            let parsed: Vec<String> =
                serde_json::from_str(text).map_err(|_| ConfigError::Invalid {
                    name: name.into(),
                    kind: "a JSON list of names",
                    value: v.into(),
                })?;
            *into = parsed;
        } else {
            *into = text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
        }
        Ok(())
    }
}

fn parse_float(name: &str, v: &str) -> Result<f64, ConfigError> {
    v.trim().parse().map_err(|_| ConfigError::Invalid {
        name: name.into(),
        kind: "a number",
        value: v.into(),
    })
}

/// pydantic's reading of a boolean from text.
fn parse_bool(name: &str, v: &str) -> Result<bool, ConfigError> {
    match v.trim().to_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid {
            name: name.into(),
            kind: "true or false",
            value: v.into(),
        }),
    }
}

impl GhostdSettings {
    /// The settings the process environment gives.
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_env(&std::env::vars().collect())
    }

    /// The settings `env` gives over the defaults: `GHOST_<FIELD>` for each field, and the
    /// two Jev keys under their own names.
    pub fn from_env(env: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let env = Env::new(env);
        let mut s = Self::default();
        env.path("GHOST_DATA_DIR", &mut s.data_dir);
        env.path("GHOST_POLICY_PATH", &mut s.policy_path);
        env.string("GHOST_JEV", &mut s.jev);
        env.float("GHOST_JEV_THRESHOLD", &mut s.jev_threshold)?;
        env.float("GHOST_JEV_DEADLINE_SECS", &mut s.jev_deadline_secs)?;
        env.string("TYPESAFE_API_KEY", &mut s.typesafe_api_key);
        env.string("OPENROUTER_API_KEY", &mut s.openrouter_api_key);
        env.path("GHOST_DB_PATH", &mut s.db_path);
        env.opt_path("GHOST_LATENCY_LOG", &mut s.latency_log);
        env.path("GHOST_CONFIG_DIR", &mut s.config_dir);
        env.int("GHOST_SIGNIN_PORT", &mut s.signin_port)?;
        env.path("GHOST_CATALOG_PATH", &mut s.catalog_path);
        env.path("GHOST_SKILLS_DIR", &mut s.skills_dir);
        env.path("GHOST_HOME_SKILLS_DIR", &mut s.home_skills_dir);
        env.int("GHOST_TURN_RETENTION_DAYS", &mut s.turn_retention_days)?;
        env.opt_path("GHOST_API_SOCKET", &mut s.api_socket);
        env.path("GHOST_SETTINGS_PATH", &mut s.settings_path);
        env.path("GHOST_ENV_PATH", &mut s.env_path);
        env.path("GHOST_VAULT_ENV_PATH", &mut s.vault_env_path);
        env.path("GHOST_INFRA_DIR", &mut s.infra_dir);
        env.string("GHOST_PHONE_LISTEN", &mut s.phone_listen);
        env.string("GHOST_HOOKS_LISTEN", &mut s.hooks_listen);
        env.string("GHOST_WAKE_MODE", &mut s.wake_mode);
        env.string("GHOST_WAKE_PHRASES", &mut s.wake_phrases);
        env.float("GHOST_WAKE_FUZZY", &mut s.wake_fuzzy)?;
        env.opt_path("GHOST_WAKE_MODEL_PATH", &mut s.wake_model_path);
        env.opt_float("GHOST_WAKE_THRESHOLD", &mut s.wake_threshold)?;
        env.bool("GHOST_WAKE_VERIFY", &mut s.wake_verify)?;
        env.float("GHOST_WAKE_COOLDOWN_SECS", &mut s.wake_cooldown_secs)?;
        env.opt_path(
            "GHOST_WAKE_ADDRESS_MODEL_PATH",
            &mut s.wake_address_model_path,
        );
        env.opt_bool("GHOST_WAKE_ADDRESS_VERIFY", &mut s.wake_address_verify)?;
        env.opt_float(
            "GHOST_WAKE_ADDRESS_THRESHOLD",
            &mut s.wake_address_threshold,
        )?;
        env.list("GHOST_DUCK_APPS", &mut s.duck_apps)?;
        env.float("GHOST_DUCK_LEVEL", &mut s.duck_level)?;
        env.bool("GHOST_CHIME", &mut s.chime)?;
        env.path("GHOST_DICTATION_FLAG", &mut s.dictation_flag);
        Ok(s)
    }

    pub fn latency_path(&self) -> PathBuf {
        self.latency_log
            .clone()
            .unwrap_or_else(|| self.data_dir.join("latency.jsonl"))
    }

    pub fn detector_threshold(&self) -> f64 {
        if let Some(t) = self.wake_threshold {
            return t;
        }
        if self.wake_verify { 0.3 } else { 0.7 }
    }

    pub fn address_verify(&self) -> bool {
        self.wake_address_verify.unwrap_or(self.wake_verify)
    }

    pub fn address_threshold(&self) -> f64 {
        if let Some(t) = self.wake_address_threshold {
            return t;
        }
        if self.address_verify() { 0.3 } else { 0.7 }
    }

    /// The vault's file (ADR 0036).
    pub fn vault_path(&self) -> PathBuf {
        self.data_dir.join(ghost_core::passwords::VAULT_FILE)
    }

    /// Where the paired phone is kept (ADR 0016).
    pub fn phone_path(&self) -> PathBuf {
        self.data_dir.join("phone.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_detector_threshold_follows_the_check() {
        // With Parakeet's check the threshold only decides how often it runs; without it the
        // threshold is the whole decision, so it sits higher unless the operator set one.
        let s = GhostdSettings::default();
        assert_eq!(s.detector_threshold(), 0.3);
        let unchecked = GhostdSettings {
            wake_verify: false,
            ..Default::default()
        };
        assert_eq!(unchecked.detector_threshold(), 0.7);
        let set = GhostdSettings {
            wake_verify: false,
            wake_threshold: Some(0.5),
            ..Default::default()
        };
        assert_eq!(set.detector_threshold(), 0.5);
        assert_eq!(unchecked.address_threshold(), 0.7);
        let address = GhostdSettings {
            wake_address_threshold: Some(0.6),
            ..Default::default()
        };
        assert_eq!(address.address_threshold(), 0.6);
        let checked = GhostdSettings {
            wake_verify: false,
            wake_address_verify: Some(true),
            ..Default::default()
        };
        assert_eq!(
            (checked.detector_threshold(), checked.address_threshold()),
            (0.7, 0.3)
        );
    }

    #[test]
    fn the_defaults_are_the_python_daemons() {
        let s = GhostdSettings::default();
        let home = home();
        assert_eq!(s.data_dir, home.join(".local/share/ghost"));
        assert_eq!(s.policy_path, home.join(".config/ghost/policy.toml"));
        assert_eq!(s.jev, "auto");
        assert_eq!(s.jev_threshold, 0.95);
        assert_eq!(s.jev_deadline_secs, 0.5);
        assert_eq!(s.signin_port, 47823);
        assert_eq!(s.turn_retention_days, 90);
        assert_eq!(s.wake_mode, "transcript");
        assert_eq!(s.wake_fuzzy, 0.9);
        assert!(s.wake_verify);
        assert_eq!(s.wake_cooldown_secs, 2.0);
        assert_eq!(s.duck_apps, vec!["spotify"]);
        assert_eq!(s.duck_level, 0.25);
        assert!(s.chime);
        assert_eq!(s.phone_listen, "");
        assert_eq!(s.hooks_listen, "");
        assert_eq!(s.api_socket, None);
        assert_eq!(
            s.latency_path(),
            home.join(".local/share/ghost/latency.jsonl")
        );
        assert_eq!(
            s.vault_path(),
            home.join(".local/share/ghost/passwords.vault")
        );
        assert_eq!(s.phone_path(), home.join(".local/share/ghost/phone.json"));
        assert!(s.dictation_flag.ends_with("dictation-audio-duck.state"));
    }

    #[test]
    fn every_setting_reads_from_its_ghost_variable() {
        let s = GhostdSettings::from_env(&env(&[
            ("GHOST_DATA_DIR", "/tmp/d"),
            ("GHOST_JEV", "off"),
            ("GHOST_JEV_THRESHOLD", "0.8"),
            ("TYPESAFE_API_KEY", "ts-key"),
            ("OPENROUTER_API_KEY", "or-key"),
            ("GHOST_LATENCY_LOG", "/tmp/l.jsonl"),
            ("GHOST_SIGNIN_PORT", "4000"),
            ("GHOST_TURN_RETENTION_DAYS", "7"),
            ("GHOST_API_SOCKET", "/tmp/api.sock"),
            ("GHOST_PHONE_LISTEN", "192.168.1.20:8765,10.8.0.4:8765"),
            ("GHOST_WAKE_MODE", "onnx"),
            ("GHOST_WAKE_THRESHOLD", "0.5"),
            ("GHOST_WAKE_VERIFY", "false"),
            ("GHOST_WAKE_ADDRESS_VERIFY", "yes"),
            ("GHOST_DUCK_APPS", "[\"spotify\", \"firefox\"]"),
            ("GHOST_DUCK_LEVEL", "0.5"),
            ("GHOST_CHIME", "0"),
        ]))
        .unwrap();
        assert_eq!(s.data_dir, PathBuf::from("/tmp/d"));
        assert_eq!(s.jev, "off");
        assert_eq!(s.jev_threshold, 0.8);
        assert_eq!(s.typesafe_api_key, "ts-key");
        assert_eq!(s.openrouter_api_key, "or-key");
        assert_eq!(s.latency_path(), PathBuf::from("/tmp/l.jsonl"));
        assert_eq!(s.signin_port, 4000);
        assert_eq!(s.turn_retention_days, 7);
        assert_eq!(s.api_socket, Some(PathBuf::from("/tmp/api.sock")));
        assert_eq!(s.phone_listen, "192.168.1.20:8765,10.8.0.4:8765");
        assert_eq!(s.wake_mode, "onnx");
        assert_eq!(s.wake_threshold, Some(0.5));
        assert!(!s.wake_verify);
        assert_eq!(s.wake_address_verify, Some(true));
        assert_eq!(s.duck_apps, vec!["spotify", "firefox"]);
        assert_eq!(s.duck_level, 0.5);
        assert!(!s.chime);
        // Case doesn't matter, and a plain list reads too.
        let s = GhostdSettings::from_env(&env(&[
            ("ghost_wake_fuzzy", "0.8"),
            ("GHOST_DUCK_APPS", "spotify, vlc"),
        ]))
        .unwrap();
        assert_eq!(s.wake_fuzzy, 0.8);
        assert_eq!(s.duck_apps, vec!["spotify", "vlc"]);
    }

    #[test]
    fn an_empty_optional_is_off() {
        let s = GhostdSettings::from_env(&env(&[
            ("GHOST_API_SOCKET", ""),
            ("GHOST_WAKE_THRESHOLD", ""),
            ("GHOST_WAKE_ADDRESS_VERIFY", ""),
            ("GHOST_WAKE_MODEL_PATH", ""),
        ]))
        .unwrap();
        assert_eq!(s.api_socket, None);
        assert_eq!(s.wake_threshold, None);
        assert_eq!(s.wake_address_verify, None);
        assert_eq!(s.wake_model_path, None);
    }

    #[test]
    fn a_setting_that_does_not_parse_is_said_by_name() {
        let err = GhostdSettings::from_env(&env(&[("GHOST_WAKE_FUZZY", "high")])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "GHOST_WAKE_FUZZY must be a number, got \"high\""
        );
        let err = GhostdSettings::from_env(&env(&[("GHOST_CHIME", "maybe")])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "GHOST_CHIME must be true or false, got \"maybe\""
        );
        let err = GhostdSettings::from_env(&env(&[("GHOST_SIGNIN_PORT", "80000")])).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("GHOST_SIGNIN_PORT must be a whole number")
        );
    }

    #[test]
    fn the_runtime_dir_is_under_xdg_runtime_dir() {
        // Only the shape: the environment is the process's.
        assert!(runtime_dir().ends_with("ghost"));
    }
}
