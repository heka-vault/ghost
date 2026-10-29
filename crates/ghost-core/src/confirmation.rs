//! Classify a spoken user turn as a clear yes, a clear no, or neither.
//!
//! The text comes from the pipeline's own transcript (ADR 0007), never from the model.
//! Two classifiers implement the same trait: `RegexClassifier` (the allowlist below, always
//! available) and Jev (when a key is configured; falls back to the regex). Both apply
//! `has_negation` first: a "no", "wait" or "cancel" anywhere in a short utterance is a no,
//! whatever else is said.

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};

const AFFIRMATIVE: &[&str] = &[
    "confirm",
    "confirmed",
    "yes",
    "yes confirm",
    "yes do it",
    "yes please",
    "yep",
    "yeah",
    "yeah do it",
    "yeah go ahead",
    "sure go ahead",
    "do it",
    "go ahead",
    "go for it",
    "approve",
    "approved",
    "ship it",
    "send it",
    "okay do it",
    "ok do it",
    "okay confirm",
    "ok confirm",
    "okay go ahead",
    "ok go ahead",
    "that's right",
    "correct",
    "proceed",
    "yes go ahead",
    "yes close it",
    "yeah close it",
    "close it",
    // how people answer "Okay to close infra 17?" without thinking about it
    "okay",
    "ok",
    "sure",
    "sure thing",
    "yup",
    "alright",
    "all right",
    "sounds good",
    "please do",
    "yes please do",
    "go on",
    "absolutely",
    "definitely",
    "fine",
    "that's fine",
    "do that",
    "yeah sure",
    "yes sure",
    "yeah okay",
    "yes okay",
    "okay sure",
    "okay yes",
    "okay yeah",
];

const NEGATIVE: &[&str] = &[
    "no",
    "nope",
    "cancel",
    "cancel that",
    "never mind",
    "nevermind",
    "stop",
    "don't",
    "do not",
    "abort",
    "wait",
    "hold on",
    "not yet",
    "no cancel",
    "no don't",
    "scratch that",
    "leave it",
];

const NEGATION_WORDS: &[&str] = &[
    "no", "don't", "dont", "cancel", "stop", "wait", "abort", "nope", "not",
];

const MAX_WORDS: usize = 5;

static NOT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z' ]+").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Affirmative,
    Negative,
    Unclear,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Affirmative => "affirmative",
            Verdict::Negative => "negative",
            Verdict::Unclear => "unclear",
        }
    }
}

/// What the operator is answering. Lets a classifier tell "yeah, close it" from "yeah,
/// the other one".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfirmationContext {
    /// The proposal's summary, e.g. `On acme/app#7 "Login": close the issue`.
    pub pending_action: String,
    /// What Ghost asked, when known.
    pub question: String,
}

#[async_trait]
pub trait ConfirmationClassifier: Send + Sync {
    /// How the audit names it ("regex", "jev", "ui").
    fn name(&self) -> &str;
    /// Which classifier actually judged the last turn, when one falls back to another.
    fn last_source(&self) -> Option<String> {
        None
    }
    async fn classify(&self, utterance: &str, context: &ConfirmationContext) -> Verdict;
}

pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let stripped = NOT_WORD.replace_all(lower.trim(), " ");
    SPACES.replace_all(&stripped, " ").trim().to_string()
}

/// True when a short utterance contains a negation word anywhere. The guard that bounds
/// a confident wrong yes from any classifier.
pub fn has_negation(text: &str) -> bool {
    let norm = normalize(text);
    let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    !words.is_empty()
        && words.len() <= MAX_WORDS
        && words.iter().any(|w| NEGATION_WORDS.contains(w))
}

pub fn classify_confirmation(text: &str) -> Verdict {
    let norm = normalize(text);
    if norm.is_empty() {
        return Verdict::Unclear;
    }
    let words: Vec<&str> = norm.split(' ').collect();
    if words.len() > MAX_WORDS {
        return Verdict::Unclear;
    }
    // A single wake-word or filler prefix is tolerated: "ghost, confirm".
    if matches!(words[0], "ghost" | "okay" | "ok") && words.len() > 1 {
        let rest = words[1..].join(" ");
        if NEGATIVE.contains(&rest.as_str()) {
            return Verdict::Negative;
        }
        if AFFIRMATIVE.contains(&rest.as_str()) {
            return Verdict::Affirmative;
        }
    }
    if NEGATIVE.contains(&norm.as_str()) {
        return Verdict::Negative;
    }
    if has_negation(&norm) {
        return Verdict::Negative;
    }
    if AFFIRMATIVE.contains(&norm.as_str()) {
        return Verdict::Affirmative;
    }
    Verdict::Unclear
}

/// The allowlist above. Deliberately conservative: only a short, unambiguous utterance
/// confirms. A long sentence that happens to contain "yes" does not.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

#[async_trait]
impl ConfirmationClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, utterance: &str, _context: &ConfirmationContext) -> Verdict {
        classify_confirmation(utterance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affirmatives() {
        for text in [
            "Confirm.",
            "yes",
            "Yes, confirm.",
            "Go ahead",
            "ghost confirm",
            "Okay, do it",
            "approved",
            // natural short answers to "Okay to close infra 17?" (ADR 0017)
            "Okay.",
            "sure",
            "Alright.",
            "Sounds good.",
            "Yeah, sure.",
        ] {
            assert_eq!(
                classify_confirmation(text),
                Verdict::Affirmative,
                "{text:?}"
            );
        }
    }

    #[test]
    fn negatives() {
        for text in [
            "no",
            "Cancel that",
            "wait",
            "never mind",
            "no, don't",
            "Ghost, cancel",
            "yes but wait",
            "okay no",
        ] {
            assert_eq!(classify_confirmation(text), Verdict::Negative, "{text:?}");
        }
    }

    #[test]
    fn unclear() {
        for text in [
            "",
            "yes I think the login page should also get a dark mode toggle before confirm",
            "what are the labels again",
            "confirm the second one and skip the first",
        ] {
            assert_eq!(classify_confirmation(text), Verdict::Unclear, "{text:?}");
        }
        assert!(has_negation("yes but wait"));
        assert!(!has_negation(
            "a long sentence that says no somewhere in it"
        ));
    }
}
