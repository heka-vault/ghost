//! The environment Ghost's own settings read (ADR 0036, ADR 0037): the one the daemon was
//! started in, with the daemon's own over it. The daemon's own (`SharedEnviron`) holds the
//! vault's values, kept in step with it, and the settings applied since the start (Settings ›
//! Model saves them live). One function resolves it, for the session's model and for what the
//! control API reports, so the desktop says what Ghost is really using.
//!
//! A setting cleared since the start is kept over it as "", which every reader of Ghost's
//! settings takes as unset: the process environment is never changed.

use std::collections::{BTreeMap, HashMap};

/// What Ghost's settings read now: `started` (the process environment at start, which the
/// launcher filled from `settings.env`), then `live` over it.
pub fn resolve(
    started: &HashMap<String, String>,
    live: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = started.clone();
    env.extend(live.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
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
        let env = resolve(&started, &live);
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://a");
        assert_eq!(env["GHOST_LLM_MODEL"], "m2");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "t");
    }

    #[test]
    fn a_setting_cleared_since_the_start_stays_cleared() {
        let started = map(&[("GHOST_LLM_MODEL", "m1")]);
        let mut live = HashMap::new();
        apply(&started, &mut live, "GHOST_LLM_MODEL", None);
        assert_eq!(resolve(&started, &live)["GHOST_LLM_MODEL"], "");
        apply(&started, &mut live, "GHOST_LLM_MODEL", Some("m3"));
        assert_eq!(resolve(&started, &live)["GHOST_LLM_MODEL"], "m3");
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
}
