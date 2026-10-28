//! Every password value this process holds, replaced wherever text is stored, logged or
//! handed on (ADR 0036). Deterministic: plain substring matches, longest first, no model.
//!
//! The vault (`passwords`) feeds this with its values; a plaintext file not yet imported
//! feeds it too (`also_redact`). A value shorter than `MIN_REDACT_CHARS` is kept but not
//! looked for: "1234" would turn every page number into a password.

use std::collections::BTreeSet;
use std::sync::{LazyLock, RwLock};

use serde_json::Value;
use zeroize::Zeroizing;

pub const MIN_REDACT_CHARS: usize = 8;
/// A token kept with its scheme (`Bearer abc…`) is also looked for as `abc…`, which is
/// what an app would echo.
const SCHEMES: [&str; 5] = ["Bearer ", "Basic ", "Token ", "token ", "bearer "];

/// A source's passwords: (name, value).
type Held = Vec<(String, Zeroizing<String>)>;

#[derive(Default)]
struct Redactor {
    /// Per source, in the order sources first registered; a source registering again keeps
    /// its place, as a Python dict would.
    sets: Vec<(String, Held)>,
    /// (value, name), longest value first, so a value that contains another is replaced
    /// whole. The first name wins when two passwords share a value.
    pairs: Vec<(Zeroizing<String>, String)>,
}

impl Redactor {
    fn use_values(&mut self, source: &str, values: Vec<(String, String)>) {
        let values: Vec<(String, Zeroizing<String>)> = values
            .into_iter()
            .map(|(n, v)| (n, Zeroizing::new(v)))
            .collect();
        match self.sets.iter_mut().find(|(s, _)| s == source) {
            Some(slot) => slot.1 = values,
            None => self.sets.push((source.to_string(), values)),
        }
        let mut pairs: Vec<(Zeroizing<String>, String)> = Vec::new();
        for (_, vals) in &self.sets {
            for (name, value) in vals {
                for form in forms(value) {
                    if form.len() >= MIN_REDACT_CHARS && !pairs.iter().any(|(v, _)| **v == form) {
                        pairs.push((Zeroizing::new(form), name.clone()));
                    }
                }
            }
        }
        pairs.sort_by_key(|(v, _)| std::cmp::Reverse(v.len())); // stable: ties keep order
        self.pairs = pairs;
    }

    fn names(&self) -> BTreeSet<String> {
        self.sets
            .iter()
            .flat_map(|(_, v)| v.iter().map(|(n, _)| n.clone()))
            .collect()
    }

    fn text(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (value, name) in &self.pairs {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[password: {name}]"));
            }
        }
        out
    }

    fn walk(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (self.text(k), self.walk(v)))
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.walk(v)).collect()),
            other => other.clone(),
        }
    }
}

fn forms(value: &str) -> Vec<String> {
    let mut out = vec![value.to_string()];
    for scheme in SCHEMES {
        if let Some(rest) = value.strip_prefix(scheme) {
            out.push(rest.trim().to_string());
        }
    }
    out
}

static REDACTOR: LazyLock<RwLock<Redactor>> = LazyLock::new(|| RwLock::new(Redactor::default()));

/// Values that are passwords: the vault's, or a plaintext file's not imported yet. They
/// are redacted, and left out of `child_env`, all the same. A source registering again
/// replaces its earlier set.
pub fn also_redact(source: &str, values: impl IntoIterator<Item = (String, String)>) {
    REDACTOR
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .use_values(source, values.into_iter().collect());
}

/// The names of every password this process holds.
pub fn password_names() -> BTreeSet<String> {
    REDACTOR.read().unwrap_or_else(|e| e.into_inner()).names()
}

/// `s` with every password value replaced by `[password: NAME]`.
pub fn redact_text(s: &str) -> String {
    let r = REDACTOR.read().unwrap_or_else(|e| e.into_inner());
    if r.pairs.is_empty() {
        s.to_string()
    } else {
        r.text(s)
    }
}

/// `v` with every password value replaced: strings, and the strings (and keys) inside
/// objects and arrays.
pub fn redact_json(v: &Value) -> Value {
    let r = REDACTOR.read().unwrap_or_else(|e| e.into_inner());
    if r.pairs.is_empty() {
        v.clone()
    } else {
        r.walk(v)
    }
}

/// An environment for a process Ghost starts: `base`, less every password.
pub fn child_env(base: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let held = password_names();
    base.into_iter()
        .filter(|(k, _)| !held.contains(k))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // The redactor is process-global, so these tests share it and use their own source
    // names and distinct values.

    #[test]
    fn every_value_in_a_nested_result_is_replaced() {
        also_redact(
            "test-nested",
            [("GITHUB_TOKEN".into(), "ghp_nestedvalue1234".into())],
        );
        let v = json!({"ok": "token ghp_nestedvalue1234 here", "list": ["ghp_nestedvalue1234", 3], "ghp_nestedvalue1234": true});
        let out = redact_json(&v);
        assert_eq!(out["ok"], "token [password: GITHUB_TOKEN] here");
        assert_eq!(out["list"][0], "[password: GITHUB_TOKEN]");
        assert!(out.get("[password: GITHUB_TOKEN]").is_some());
        assert_eq!(redact_text("plain"), "plain");
    }

    #[test]
    fn a_value_inside_another_is_replaced_whole_and_schemes_are_stripped() {
        also_redact(
            "test-whole",
            [
                ("SHORT_KEY".into(), "insidewhole".into()),
                ("LONG_KEY".into(), "Bearer insidewhole-and-more".into()),
            ],
        );
        assert_eq!(
            redact_text("got insidewhole-and-more back"),
            "got [password: LONG_KEY] back"
        );
        assert_eq!(
            redact_text("got insidewhole back"),
            "got [password: SHORT_KEY] back"
        );
        assert_eq!(
            redact_text("Bearer insidewhole-and-more"),
            "[password: LONG_KEY]"
        );
    }

    #[test]
    fn a_short_value_is_held_but_not_looked_for() {
        also_redact("test-short", [("PIN".into(), "1234".into())]);
        assert!(password_names().contains("PIN"));
        assert_eq!(redact_text("page 1234"), "page 1234");
    }

    #[test]
    fn a_source_registering_again_drops_what_it_no_longer_holds() {
        also_redact("test-again", [("GONE".into(), "gonevalue9876".into())]);
        assert_eq!(redact_text("gonevalue9876"), "[password: GONE]");
        also_redact("test-again", []);
        assert_eq!(redact_text("gonevalue9876"), "gonevalue9876");
        assert!(!password_names().contains("GONE"));
    }

    #[test]
    fn child_env_leaves_every_password_out() {
        also_redact("test-env", [("ENV_SECRET".into(), "envsecretvalue".into())]);
        let base = vec![
            ("PATH".to_string(), "/bin".to_string()),
            ("ENV_SECRET".to_string(), "envsecretvalue".to_string()),
        ];
        let env = child_env(base);
        assert_eq!(env, vec![("PATH".to_string(), "/bin".to_string())]);
    }
}
