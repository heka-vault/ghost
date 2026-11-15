//! A provider's key from the environment Ghost was started in is offered, never used, until
//! the person chooses it (#137, ADR 0037's amendment of 2026-11-14). The rule itself is
//! `ghost_core::environment` (`OFFERED`, `FROM_ENV`, `resolve`); this is the daemon's part:
//! what it was started with, and the one decision an update makes for an install that already
//! ran on such a key.
//!
//! **The update.** Before this rule a key in the start's environment was used without a word.
//! An install that answered with one would go quiet after the update. So, once, at the first
//! start that has a store and an open vault: when Ghost has answered before (a turn in the
//! store) and the model connection is ready only through variables the start's environment
//! holds, those names are kept (in memory, named in the store's checkpoint, never a value),
//! and a note on the waiting list says so with the two ways out on Settings › Model: Use it
//! (writes `GHOST_LLM_FROM_ENV`, the person's choice) or choose another connection, or Don't
//! use it (`GHOST_LLM_FROM_ENV=0`). Nothing is written to `settings.env` by Ghost itself: the
//! choice stays the person's. The decision is made once, so a later start from another shell
//! keeps nothing it did not keep then.

use std::collections::{BTreeSet, HashMap};

use ghost_core::connection::ModelConnection;
use ghost_core::environment::{FromEnv, Inherited, resolve};
use tracing::{info, warn};

use crate::config::GhostdSettings;
use crate::daemon::{Daemon, lock};

/// The waiting list's key for the note (ADR 0056's kind: shown, never said).
pub const NOTE_KEY: &str = "model:from-environment";
/// Where the update's decision is kept (`checkpoints`): the kept names, comma-separated,
/// or "" for none. Its presence is the "decided".
const CHECKPOINT: (&str, &str) = ("ghost", "environment-keys");

/// The offered variables only the environment Ghost was started in holds: the start's
/// environment against `settings.env`, which a launcher loads into it.
pub fn inherited_at_start(cfg: &GhostdSettings, started: &HashMap<String, String>) -> Inherited {
    let file = ghost_core::envfile::read_env(&cfg.settings_path).unwrap_or_default();
    Inherited::from_start(started, &file)
}

/// The names the model connection reads now: its password, and the older address when it
/// is the connection's.
fn reads(conn: &ModelConnection, env: &HashMap<String, String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if !conn.password.is_empty() {
        out.insert(conn.password.clone());
    }
    if let Some(base) = env.get("ANTHROPIC_BASE_URL").map(|b| b.trim())
        && !base.is_empty()
        && conn.base_url_override.trim() == base
    {
        out.insert("ANTHROPIC_BASE_URL".to_string());
    }
    out
}

fn ready(env: &HashMap<String, String>) -> bool {
    ModelConnection::from_env(env)
        .problems(env, false)
        .is_empty()
}

/// What the update keeps: the variables the connection in use reads only from the start's
/// environment, when Ghost has `answered` before and the connection is ready only with them.
pub fn kept_by_update(
    started: &HashMap<String, String>,
    live: &HashMap<String, String>,
    inherited: &Inherited,
    answered: bool,
) -> BTreeSet<String> {
    if !answered || FromEnv::of(started, live) == FromEnv::Nothing {
        return BTreeSet::new();
    }
    let as_before = resolve(started, live, &Inherited::default());
    let now = resolve(started, live, inherited);
    if ready(&now) || !ready(&as_before) {
        return BTreeSet::new();
    }
    let conn = ModelConnection::from_env(&as_before);
    let withheld = inherited.withheld(started, live);
    reads(&conn, &as_before)
        .into_iter()
        .filter(|n| withheld.contains(n))
        .collect()
}

/// "A", "A and B", "A, B and C".
pub fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl Daemon {
    /// The update's decision (module doc): read once it is made, made once the store is there
    /// and the vault is not locked (a locked vault's key would look missing).
    pub(crate) fn settle_environment_keys(&self) {
        let Some(store) = self.late.store.get().cloned() else {
            return;
        };
        let decided = match store.get_checkpoint(CHECKPOINT.0, CHECKPOINT.1) {
            Ok(d) => d,
            Err(e) => {
                warn!("model: what an update kept from the environment could not be read ({e})");
                return;
            }
        };
        let kept: BTreeSet<String> = match decided {
            Some(names) => names
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(String::from)
                .collect(),
            None => {
                if self.passwords().is_some_and(|v| !v.is_open()) {
                    return; // decided when it opens (`passwords_opened`)
                }
                let answered = store.any_turn().unwrap_or(false);
                let live = lock(&self.environ).clone();
                let kept =
                    kept_by_update(&self.started_env, &live, &self.inherited_now(), answered);
                let line = kept.iter().cloned().collect::<Vec<_>>().join(",");
                if let Err(e) = store.set_checkpoint(CHECKPOINT.0, CHECKPOINT.1, &line) {
                    warn!("model: the update's decision could not be kept ({e})");
                }
                if !kept.is_empty() {
                    let names: Vec<String> = kept.iter().cloned().collect();
                    info!(
                        "model: kept {} from the environment Ghost was started in, as before \
                         this update, until the operator chooses on Settings › Model",
                        and_list(&names)
                    );
                }
                kept
            }
        };
        let mut inherited = self.inherited.write().unwrap_or_else(|e| e.into_inner());
        inherited.kept = kept
            .into_iter()
            .filter(|n| inherited.names.contains(n))
            .collect();
    }

    /// The note while Ghost uses a key the update kept and the person has not chosen yet.
    pub(crate) fn environment_keys_note(&self) -> Option<String> {
        let inherited = self.inherited_now();
        if inherited.kept.is_empty() {
            return None;
        }
        let live = lock(&self.environ).clone();
        let choice = FromEnv::of(&self.started_env, &live);
        if choice == FromEnv::Nothing {
            return None;
        }
        let waiting: Vec<String> = inherited
            .kept
            .iter()
            .filter(|n| !choice.accepts(n) && !live.contains_key(*n))
            .cloned()
            .collect();
        if waiting.is_empty() {
            return None;
        }
        // Only while the connection in use still reads them.
        let env = self.env();
        let read = reads(&ModelConnection::from_env(&env), &env);
        if !waiting.iter().any(|n| read.contains(n)) {
            return None;
        }
        let one = waiting.len() == 1;
        Some(format!(
            "Ghost is using {} from the environment it was started in, as it did before this \
             update. Nobody chose {} for Ghost. On Settings › Model, use {} or choose another \
             connection.",
            and_list(&waiting),
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
        ))
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

    fn shell() -> HashMap<String, String> {
        map(&[
            ("ANTHROPIC_BASE_URL", "http://proxy.lan:8317"),
            ("ANTHROPIC_AUTH_TOKEN", "sk-shell-0123456789abcdef"),
            ("HOME", "/home/x"),
        ])
    }

    #[test]
    fn an_install_that_answered_on_the_shells_key_keeps_it() {
        let started = shell();
        let inherited = Inherited::from_start(&started, &Default::default());
        let kept = kept_by_update(&started, &HashMap::new(), &inherited, true);
        assert_eq!(
            kept.into_iter().collect::<Vec<_>>(),
            ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"]
        );
    }

    #[test]
    fn a_new_install_keeps_nothing() {
        let started = shell();
        let inherited = Inherited::from_start(&started, &Default::default());
        assert!(kept_by_update(&started, &HashMap::new(), &inherited, false).is_empty());
    }

    #[test]
    fn a_connection_ready_on_ghosts_own_keeps_nothing() {
        // The vault holds the key and settings.env the address: the shell's are not needed.
        let started = shell();
        let file = [(
            "ANTHROPIC_BASE_URL".to_string(),
            "http://proxy.lan:8317".to_string(),
        )]
        .into_iter()
        .collect();
        let inherited = Inherited::from_start(&started, &file);
        let live = map(&[("ANTHROPIC_AUTH_TOKEN", "sk-vault-0123456789")]);
        assert!(kept_by_update(&started, &live, &inherited, true).is_empty());
    }

    #[test]
    fn a_person_who_said_none_keeps_nothing() {
        let mut started = shell();
        started.insert(ghost_core::environment::FROM_ENV.into(), "0".into());
        let inherited = Inherited::from_start(&started, &Default::default());
        assert!(kept_by_update(&started, &HashMap::new(), &inherited, true).is_empty());
    }

    fn started_daemon(
        tmp: &std::path::Path,
        store: std::sync::Arc<ghost_core::store::Store>,
    ) -> Daemon {
        let d = std::sync::Arc::try_unwrap(crate::daemon::tests::daemon(tmp))
            .ok()
            .unwrap()
            .with_started_env(shell());
        d.late.store.set(store).ok();
        d
    }

    fn answered_store() -> std::sync::Arc<ghost_core::store::Store> {
        let store = std::sync::Arc::new(ghost_core::store::Store::in_memory().unwrap());
        store
            .add_turn(
                "ghost",
                "s1",
                ghost_core::conversations::Role::User,
                "a synthetic turn",
            )
            .unwrap();
        store
    }

    #[test]
    fn an_update_keeps_what_the_install_ran_on_once_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let store = answered_store();
        let d = started_daemon(tmp.path(), store.clone());
        d.settle_environment_keys();
        let env = d.env();
        assert!(env.contains_key("ANTHROPIC_AUTH_TOKEN") && env.contains_key("ANTHROPIC_BASE_URL"));
        let note = d
            .environment_keys_note()
            .expect("a note while nobody chose them");
        assert!(
            note.contains("ANTHROPIC_AUTH_TOKEN and ANTHROPIC_BASE_URL"),
            "{note}"
        );
        assert!(!note.contains("sk-shell"), "a name, never a value");
        // The person chooses them (Settings › Model's Use them): the note goes.
        let mut values = std::collections::BTreeMap::new();
        values.insert(
            ghost_core::environment::FROM_ENV.to_string(),
            Some("ANTHROPIC_AUTH_TOKEN,ANTHROPIC_BASE_URL".to_string()),
        );
        d.apply_model(&values);
        assert!(d.environment_keys_note().is_none());
        assert!(d.env().contains_key("ANTHROPIC_AUTH_TOKEN"));
        // Or says no: Ghost stops using them.
        values.insert(
            ghost_core::environment::FROM_ENV.to_string(),
            Some("0".to_string()),
        );
        d.apply_model(&values);
        assert!(!d.env().contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(d.environment_keys_note().is_none());
    }

    #[test]
    fn the_decision_is_made_once_so_a_later_shell_keeps_nothing() {
        // The first start with this rule decided nothing (Ghost ran on its own vault then);
        // a later start from a shell that exports a key keeps nothing.
        let tmp = tempfile::tempdir().unwrap();
        let store = answered_store();
        store
            .set_checkpoint(CHECKPOINT.0, CHECKPOINT.1, "")
            .unwrap();
        let d = started_daemon(tmp.path(), store);
        d.settle_environment_keys();
        assert!(!d.env().contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(!d.env().contains_key("ANTHROPIC_BASE_URL"));
        assert!(d.environment_keys_note().is_none());
    }

    #[test]
    fn a_fresh_install_started_from_a_shell_keeps_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(ghost_core::store::Store::in_memory().unwrap());
        let d = started_daemon(tmp.path(), store.clone());
        d.settle_environment_keys();
        assert!(!d.env().contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert_eq!(
            store.get_checkpoint(CHECKPOINT.0, CHECKPOINT.1).unwrap(),
            Some(String::new()),
            "decided: nothing kept"
        );
    }

    #[test]
    fn names_read_as_a_sentence() {
        let n = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(and_list(&n(&["A"])), "A");
        assert_eq!(and_list(&n(&["A", "B"])), "A and B");
        assert_eq!(and_list(&n(&["A", "B", "C"])), "A, B and C");
    }
}
