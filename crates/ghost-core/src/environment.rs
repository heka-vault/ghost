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

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
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
