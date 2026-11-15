//! The environment Ghost's own settings read (ADR 0036, ADR 0037): the one the daemon was
//! started in, with the daemon's own over it. The daemon's own (`SharedEnviron`) holds the
//! vault's values, kept in step with it, and the settings applied since the start (Settings ›
//! Model saves them live). One function resolves it, for the session's model and for what the
//! control API reports, so the desktop says what Ghost is really using.
//!
//! A setting cleared since the start is kept over it as "", which every reader of Ghost's
//! settings takes as unset: the process environment is never changed.
//!
//! A provider's key or address that comes only from the environment Ghost was started in is
//! offered, never used, until the person chooses it (#137, ADR 0037's amendment of
//! 2026-11-14): [`OFFERED`] names them, [`FROM_ENV`] is the choice, and [`resolve`] leaves out
//! what was not chosen, so every reader of Ghost's settings sees the same.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

/// The person's choice to use a provider's key or address from the environment Ghost was
/// started in: the variables' names, comma-separated (`ANTHROPIC_API_KEY`), which is what the
/// first run and Settings › Model write to `settings.env`; `1` for every one of them, the
/// developer's switch; `0` (or `none`) for none, which also ends what an update kept.
pub const FROM_ENV: &str = "GHOST_LLM_FROM_ENV";

/// The variables Ghost reads by default without the person naming them anywhere in Ghost:
/// the model providers' keys, the older address, Jev's keys and the ElevenLabs voice's key.
/// Another tool's or a shell's (a coding agent's `ANTHROPIC_API_KEY`, a proxy's
/// `ANTHROPIC_BASE_URL`), so one that only the environment Ghost was started in holds is
/// offered and never used until [`FROM_ENV`] chooses it. A name the person wrote in Ghost
/// (`GHOST_LLM_PASSWORD`, `GHOST_REALTIME_PASSWORD`, an app's `${NAME}`, a route's) is theirs.
pub const OFFERED: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "LLM_API_KEY",
    "TYPESAFE_API_KEY",
    "ELEVENLABS_API_KEY",
];

/// Whether `name` is one of [`OFFERED`].
pub fn is_offered(name: &str) -> bool {
    OFFERED.contains(&name)
}

/// What [`FROM_ENV`] says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FromEnv {
    /// Not set: nothing is chosen, and what an update kept stays kept.
    #[default]
    Unset,
    /// `1`: every offered variable (a developer's switch).
    All,
    /// `0`: none, and not what an update kept either.
    Nothing,
    /// These variables.
    Names(BTreeSet<String>),
}

impl FromEnv {
    pub fn parse(value: Option<&str>) -> Self {
        let v = value.unwrap_or("").trim();
        match v.to_ascii_lowercase().as_str() {
            "" => Self::Unset,
            "1" | "all" | "true" | "yes" | "on" => Self::All,
            "0" | "none" | "false" | "no" | "off" => Self::Nothing,
            _ => Self::Names(
                v.split([',', ' ', ';'])
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(String::from)
                    .collect(),
            ),
        }
    }

    pub fn accepts(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Names(n) => n.contains(name),
            Self::Unset | Self::Nothing => false,
        }
    }

    /// The choice the resolved settings make: a value saved since the start over the start's.
    pub fn of(started: &HashMap<String, String>, live: &HashMap<String, String>) -> Self {
        Self::parse(
            live.get(FROM_ENV)
                .or_else(|| started.get(FROM_ENV))
                .map(String::as_str),
        )
    }
}

/// The offered variables the environment Ghost was started in holds and Ghost's own do not,
/// and what an update kept of them. Names only, never a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inherited {
    /// Offered variables set at the start whose value is not `settings.env`'s.
    pub names: BTreeSet<String>,
    /// Those an install that already ran on them keeps using after the update that brought
    /// this rule, until the person chooses (`ghostd::environment_keys`).
    pub kept: BTreeSet<String>,
}

pub type SharedInherited = Arc<RwLock<Inherited>>;

impl Inherited {
    /// From the environment at the start and `settings.env` (`file`), which a launcher loads
    /// into it: a value the file holds is Ghost's own, not inherited.
    pub fn from_start(started: &HashMap<String, String>, file: &BTreeMap<String, String>) -> Self {
        let names = OFFERED
            .iter()
            .filter(|k| match started.get(**k) {
                Some(v) if !v.trim().is_empty() => file.get(**k) != Some(v),
                _ => false,
            })
            .map(|k| k.to_string())
            .collect();
        Self {
            names,
            kept: BTreeSet::new(),
        }
    }

    /// The inherited variables Ghost leaves out now: not chosen, not saved since, not kept.
    pub fn withheld(
        &self,
        started: &HashMap<String, String>,
        live: &HashMap<String, String>,
    ) -> BTreeSet<String> {
        let choice = FromEnv::of(started, live);
        self.names
            .iter()
            .filter(|n| !live.contains_key(*n) && !choice.accepts(n))
            .filter(|n| choice == FromEnv::Nothing || !self.kept.contains(*n))
            .cloned()
            .collect()
    }
}

/// What Ghost's settings read now: `started` (the process environment at start, which the
/// launcher filled from `settings.env`) less what it only offers (`inherited`'s withheld),
/// then `live` over it.
pub fn resolve(
    started: &HashMap<String, String>,
    live: &HashMap<String, String>,
    inherited: &Inherited,
) -> HashMap<String, String> {
    let mut env = started.clone();
    for name in inherited.withheld(started, live) {
        env.remove(&name);
    }
    env.extend(live.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// A `ghostd` command's view of Ghost's settings without a daemon (`ghostd check`): the
/// process environment less what it only offers, the vault's values over it.
pub fn as_ghost_reads(
    process: &HashMap<String, String>,
    vault: &HashMap<String, String>,
    file: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    resolve(process, vault, &Inherited::from_start(process, file))
}

/// A setting saved while Ghost runs: set, or cleared (`None` or ""). Cleared, it is kept as ""
/// when the start's environment holds it, so the start's value no longer counts.
pub fn apply(
    started: &HashMap<String, String>,
    live: &mut HashMap<String, String>,
    key: &str,
    value: Option<&str>,
) {
    match value {
        Some(v) if !v.is_empty() => {
            live.insert(key.to_string(), v.to_string());
        }
        _ if started.get(key).is_some_and(|v| !v.trim().is_empty()) => {
            live.insert(key.to_string(), String::new());
        }
        _ => {
            live.remove(key);
        }
    }
}

/// Whether the value Ghost reads under `key` comes only from the environment it was started
/// in: not the vault, not a setting saved since, and not `settings.env` (`file`, which the
/// launcher loads into that environment). The name only; never a value.
pub fn from_start(
    key: &str,
    started: &HashMap<String, String>,
    live: &HashMap<String, String>,
    file: &BTreeMap<String, String>,
) -> bool {
    if live.contains_key(key) {
        return false;
    }
    match started.get(key) {
        Some(v) if !v.trim().is_empty() => file.get(key) != Some(v),
        _ => false,
    }
}

/// Set by a launcher that has loaded `settings.env` into the environment it starts `ghostd`
/// in (`make dev` and the make targets, the user unit, the desktop app): the daemon then reads
/// the file no more, so it is loaded once and a launcher's own choice (a `./.env` in place of
/// it, the file over the login session's exports) stands.
pub const SETTINGS_LOADED: &str = "GHOST_SETTINGS_LOADED";

/// What `settings.env` gives a `ghostd` command, under the process environment it was started
/// with ([`settings_file_under`]). Names, never a value, except in `added`, which goes into the
/// process environment only.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SettingsFile {
    /// A launcher loaded the file already ([`SETTINGS_LOADED`]): nothing is added.
    pub already_loaded: bool,
    /// The file's settings the process environment does not hold: these apply.
    pub added: BTreeMap<String, String>,
    /// Names the file sets that the process environment also holds: the export wins.
    pub overridden: Vec<String>,
    /// Names in the file that are passwords' (ADR 0036): never taken from a settings file.
    pub left_out: Vec<String>,
}

/// A name that is a password's, by its ending; the desktop's launcher leaves the same out.
pub fn looks_like_a_password(name: &str) -> bool {
    const ENDINGS: [&str; 6] = [
        "_API_KEY",
        "_AUTH_TOKEN",
        "_TOKEN",
        "_SECRET",
        "_PASSWORD",
        "_PASSPHRASE",
    ];
    ENDINGS.iter().any(|e| name.ends_with(e))
}

/// The one precedence between `settings.env` (`file`) and the environment a `ghostd` command
/// was started in (`process`): **the file's value applies unless the process environment holds
/// the name, and then the process environment's value wins, an empty one included.**
///
/// The file is what the operator saved in Ghost (the desktop's pages write it); a variable in
/// the process environment is there because someone put it there for this start (a shell's
/// `export`, a unit's `Environment=`, a test harness), which is the one way to override a saved
/// setting without editing it, and an empty one is how a listener is turned off for one start.
/// This is the environment the daemon's sessions resolve with `resolve` and that `from_start`
/// compares against the file, so what Settings › Model reports as "from the environment Ghost
/// was started in" is exactly what was exported over the file. A password's name in the file
/// is left out (ADR 0036). When a launcher already loaded the file ([`SETTINGS_LOADED`]),
/// nothing is read again.
pub fn settings_file_under(
    process: &HashMap<String, String>,
    file: &BTreeMap<String, String>,
) -> SettingsFile {
    if process
        .get(SETTINGS_LOADED)
        .is_some_and(|v| !v.trim().is_empty())
    {
        return SettingsFile {
            already_loaded: true,
            ..SettingsFile::default()
        };
    }
    let mut out = SettingsFile::default();
    for (k, v) in file {
        if looks_like_a_password(k) {
            out.left_out.push(k.clone());
        } else if process.contains_key(k) {
            out.overridden.push(k.clone());
        } else {
            out.added.insert(k.clone(), v.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn none() -> Inherited {
        Inherited::default()
    }

    fn file(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn settings_env_fills_what_the_process_environment_does_not_set() {
        // A plain `ghostd run` from a shell: the file's settings apply, and what the shell
        // exported for this start wins over them.
        let process = map(&[
            ("HOME", "/home/x"),
            ("ANTHROPIC_BASE_URL", "http://shell"),
            ("GHOST_MCP_LISTEN", ""), // exported empty: off, on purpose
        ]);
        let f = file(&[
            ("ANTHROPIC_BASE_URL", "http://file"),
            ("GHOST_LLM_MODEL", "m1"),
            ("GHOST_MCP_LISTEN", "127.0.0.1:5151"),
            ("ANTHROPIC_API_KEY", "not-a-setting-0123"),
        ]);
        let s = settings_file_under(&process, &f);
        assert_eq!(s.added, file(&[("GHOST_LLM_MODEL", "m1")]));
        assert_eq!(s.overridden, vec!["ANTHROPIC_BASE_URL", "GHOST_MCP_LISTEN"]);
        assert_eq!(
            s.left_out,
            vec!["ANTHROPIC_API_KEY"],
            "a password is never a setting"
        );
        // And from_start (PR #109) still names exactly the export as from the environment.
        let mut started = process.clone();
        started.extend(s.added.clone());
        let live = HashMap::new();
        assert!(from_start("ANTHROPIC_BASE_URL", &started, &live, &f));
        assert!(!from_start("GHOST_LLM_MODEL", &started, &live, &f));
    }

    #[test]
    fn a_launcher_that_loaded_settings_env_is_not_loaded_over() {
        // make dev, the unit, the desktop app: they loaded the file and said so.
        let process = map(&[(SETTINGS_LOADED, "1"), ("GHOST_LLM_MODEL", "m1")]);
        let f = file(&[("GHOST_LLM_MODEL", "m1"), ("GHOST_LLM_FAST_MODEL", "f")]);
        let s = settings_file_under(&process, &f);
        assert!(s.already_loaded);
        assert!(s.added.is_empty());
    }

    #[test]
    fn the_daemons_own_values_win_over_the_start() {
        let started = map(&[
            ("ANTHROPIC_BASE_URL", "http://a"),
            ("GHOST_LLM_MODEL", "m1"),
        ]);
        let live = map(&[("GHOST_LLM_MODEL", "m2"), ("ANTHROPIC_AUTH_TOKEN", "t")]);
        let env = resolve(&started, &live, &Inherited::default());
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://a");
        assert_eq!(env["GHOST_LLM_MODEL"], "m2");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "t");
    }

    #[test]
    fn a_setting_cleared_since_the_start_stays_cleared() {
        let started = map(&[("GHOST_LLM_MODEL", "m1")]);
        let mut live = HashMap::new();
        apply(&started, &mut live, "GHOST_LLM_MODEL", None);
        assert_eq!(resolve(&started, &live, &none())["GHOST_LLM_MODEL"], "");
        apply(&started, &mut live, "GHOST_LLM_MODEL", Some("m3"));
        assert_eq!(resolve(&started, &live, &none())["GHOST_LLM_MODEL"], "m3");
        // Nothing at the start: cleared is simply gone.
        apply(&started, &mut live, "GHOST_LLM_FAST_MODEL", Some("f"));
        apply(&started, &mut live, "GHOST_LLM_FAST_MODEL", Some(""));
        assert!(!live.contains_key("GHOST_LLM_FAST_MODEL"));
    }

    #[test]
    fn only_what_nothing_of_ghosts_holds_is_from_the_start() {
        let started = map(&[
            ("ANTHROPIC_BASE_URL", "http://a"),
            ("ANTHROPIC_AUTH_TOKEN", "t"),
            ("GHOST_LLM_MODEL", "m1"),
            ("GHOST_LLM_PROVIDER", "custom"),
        ]);
        let live = map(&[("ANTHROPIC_AUTH_TOKEN", "t")]); // the vault holds it too
        let file: BTreeMap<String, String> = [("GHOST_LLM_MODEL".to_string(), "m1".to_string())]
            .into_iter()
            .collect();
        assert!(from_start("ANTHROPIC_BASE_URL", &started, &live, &file));
        assert!(!from_start("ANTHROPIC_AUTH_TOKEN", &started, &live, &file));
        assert!(!from_start("GHOST_LLM_MODEL", &started, &live, &file));
        assert!(from_start("GHOST_LLM_PROVIDER", &started, &live, &file));
        assert!(!from_start("GHOST_LLM_BASE_URL", &started, &live, &file));
        // settings.env saying something else than the start: the start's value is used.
        let other: BTreeMap<String, String> = [("GHOST_LLM_MODEL".to_string(), "m9".to_string())]
            .into_iter()
            .collect();
        assert!(from_start("GHOST_LLM_MODEL", &started, &live, &other));
    }

    // --- #137: a key from the environment is offered, never used, until chosen ------------

    #[test]
    fn a_providers_key_from_the_start_is_offered_and_not_used() {
        let started = map(&[
            ("ANTHROPIC_BASE_URL", "http://proxy.lan:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-shell-0123456789"),
            ("GHOST_LLM_MODEL", "m1"),
            ("HOME", "/home/x"),
        ]);
        let inherited = Inherited::from_start(&started, &file(&[]));
        assert_eq!(
            inherited
                .names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"]
        );
        let env = resolve(&started, &HashMap::new(), &inherited);
        assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
        // Ghost's own settings (GHOST_*) and everything else stay.
        assert_eq!(env["GHOST_LLM_MODEL"], "m1");
        assert_eq!(env["HOME"], "/home/x");
    }

    #[test]
    fn the_choice_names_the_variables_it_takes() {
        let mut started = map(&[
            ("ANTHROPIC_API_KEY", "sk-a-0123456789"),
            ("OPENAI_API_KEY", "sk-o-0123456789"),
        ]);
        let inherited = Inherited::from_start(&started, &file(&[]));
        // Saved since the start (Settings › Model's Use it): this one only.
        let live = map(&[(FROM_ENV, "ANTHROPIC_API_KEY")]);
        let env = resolve(&started, &live, &inherited);
        assert!(env.contains_key("ANTHROPIC_API_KEY"));
        assert!(!env.contains_key("OPENAI_API_KEY"));
        // The developer's switch: all of them.
        started.insert(FROM_ENV.into(), "1".into());
        let env = resolve(&started, &HashMap::new(), &inherited);
        assert!(env.contains_key("ANTHROPIC_API_KEY") && env.contains_key("OPENAI_API_KEY"));
        // Cleared on the page: the start's switch no longer counts.
        let live = map(&[(FROM_ENV, "")]);
        assert!(!resolve(&started, &live, &inherited).contains_key("OPENAI_API_KEY"));
    }

    #[test]
    fn what_settings_env_or_the_vault_holds_is_ghosts_own() {
        let started = map(&[
            ("ANTHROPIC_BASE_URL", "http://proxy.lan:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-shell-0123456789"),
        ]);
        let f = file(&[("ANTHROPIC_BASE_URL", "http://proxy.lan:8317")]);
        let inherited = Inherited::from_start(&started, &f);
        assert_eq!(
            inherited.names.len(),
            1,
            "the file's address is Ghost's own"
        );
        let live = map(&[("ANTHROPIC_AUTH_TOKEN", "sk-vault-0123456789")]);
        let env = resolve(&started, &live, &inherited);
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "sk-vault-0123456789");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://proxy.lan:8317");
    }

    #[test]
    fn what_an_update_kept_holds_until_the_person_says_no() {
        let started = map(&[("ANTHROPIC_API_KEY", "sk-a-0123456789")]);
        let mut inherited = Inherited::from_start(&started, &file(&[]));
        inherited.kept.insert("ANTHROPIC_API_KEY".into());
        assert!(resolve(&started, &HashMap::new(), &inherited).contains_key("ANTHROPIC_API_KEY"));
        let live = map(&[(FROM_ENV, "0")]);
        assert!(!resolve(&started, &live, &inherited).contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn the_choice_reads_as_written() {
        assert_eq!(FromEnv::parse(None), FromEnv::Unset);
        assert_eq!(FromEnv::parse(Some(" 1 ")), FromEnv::All);
        assert_eq!(FromEnv::parse(Some("none")), FromEnv::Nothing);
        let names = FromEnv::parse(Some("ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL"));
        assert!(names.accepts("ANTHROPIC_BASE_URL") && !names.accepts("OPENAI_API_KEY"));
    }

    #[test]
    fn the_choice_is_not_a_password() {
        // It goes in settings.env, which never takes a password (ADR 0036).
        assert!(!looks_like_a_password(FROM_ENV));
        assert!(!crate::passwords::is_password_name(FROM_ENV, &[]));
    }
}
