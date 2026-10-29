//! Wake by transcript (`ghost_voice.wake`): a wake phrase as a contiguous word sequence,
//! case and punctuation ignored, with a phonetic near-miss rule for Parakeet's spellings,
//! and the name opening a sentence for "Ghost, …" (ADR 0025). A match returns the words
//! that followed the phrase, so "hey ghost, what needs me?" becomes the first turn.
//!
//! Transcripts that do not contain a phrase are dropped on the spot by the caller: not
//! logged, not stored, not sent anywhere.

use std::sync::LazyLock;

use regex::Regex;

use super::difflib;

pub const DEFAULT_WAKE_PHRASES: &[&str] = &[
    "hey ghost",
    "yo ghost",
    "okay ghost",
    "ok ghost",
    "ghost you there",
    "ghost are you there",
    "ghost wake up",
];

/// Words that may come before the name when the operator addresses Ghost ("so, Ghost, …").
/// Not "the" or "a": "the ghost story" isn't talking to Ghost.
pub const OPENING_LEAD: &[&str] = &[
    "so", "okay", "ok", "hey", "and", "um", "uh", "well", "oh", "alright", "right", "yo",
];

static NOT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s]").expect("fixed"));
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").expect("fixed"));
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?]+\s+").expect("fixed"));
/// What Parakeet writes for "Ghost" as the first word of an utterance, in the operator's
/// voice: "Coach" (2026-10-24), "goes", "coast". The g/k onset and the o vowel come through;
/// the end doesn't. A word of that shape counts as the name only when an opener follows it.
static NAME_SHAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[gkc]h?o[a-z]{0,4}$").expect("fixed"));

/// A phrase found in a transcript and what was said after it ("" if nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeMatch {
    pub phrase: String,
    pub remainder: String,
}

impl WakeMatch {
    pub fn new(phrase: &str, remainder: &str) -> Self {
        Self {
            phrase: phrase.to_string(),
            remainder: remainder.to_string(),
        }
    }
}

fn words(text: &str) -> Vec<String> {
    NOT_WORD
        .replace_all(&text.to_lowercase(), " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// `text` from its `k`-th word (as `words` counts them) on, as it was written: the request
/// keeps its case and punctuation ("let's", not "let s").
fn after(text: &str, k: usize) -> String {
    let starts: Vec<usize> = WORD.find_iter(text).map(|m| m.start()).collect();
    match starts.get(k) {
        Some(&at) => text[at..].trim().to_string(),
        None => String::new(),
    }
}

/// Comma-separated phrases from the environment, normalised; empty -> defaults.
pub fn parse_phrases(spec: Option<&str>) -> Vec<String> {
    let defaults = || DEFAULT_WAKE_PHRASES.iter().map(|p| p.to_string()).collect();
    let Some(spec) = spec else { return defaults() };
    if spec.trim().is_empty() {
        return defaults();
    }
    let out: Vec<String> = spec
        .split(',')
        .map(words)
        .filter(|w| !w.is_empty())
        .map(|w| w.join(" "))
        .collect();
    if out.is_empty() { defaults() } else { out }
}

const PHONETIC_SUBS: &[(&str, &str)] = &[
    ("ph", "f"),
    ("gh", "g"),
    ("ck", "k"),
    ("wh", "w"),
    ("th", "t"),
    ("dg", "j"),
    ("c", "k"),
    ("q", "k"),
    ("x", "ks"),
    ("z", "s"),
];

/// A crude phonetic key: consonant skeleton after the first letter, common English
/// confusions folded. "ghost" -> "gst", "goes" and "guys" -> "gs", "hey" and "he" -> "h".
fn phonetic(word: &str) -> String {
    let mut w = word.to_lowercase();
    for (a, b) in PHONETIC_SUBS {
        w = w.replace(a, b);
    }
    let mut chars = w.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.push(first);
    let mut last = first;
    for ch in chars {
        if "aeiouyh".contains(ch) || ch == last {
            continue;
        }
        out.push(ch);
        last = ch;
    }
    out
}

fn similar(a: &str, b: &str) -> f64 {
    difflib::ratio(a, b)
}

/// Find the first wake phrase as a contiguous word sequence in `text`.
///
/// Case and punctuation are ignored, so "Ghost, you there?" matches "ghost you there".
/// Longer phrases are tried first so "hey ghost" wins over a plain "ghost" if both are
/// configured.
///
/// With `fuzzy` > 0, a window of the same word count whose phonetic key is at least that
/// similar to the phrase's also matches. 0 disables it. 0.9 (ghostd's default) takes
/// spellings with the phrase's own sounds, "he ghost" for "hey ghost"; 0.8 also takes "hey
/// guys" and "he goes", which a webcam mic needed, but they are ordinary speech too: 2.1
/// false wakes an hour of read speech, none at 0.9 (docs/benchmarks/2026-10-21-wake.md).
pub fn match_wake(text: &str, phrases: &[impl AsRef<str>], fuzzy: f64) -> Option<WakeMatch> {
    let words = words(text);
    if words.is_empty() {
        return None;
    }
    let mut ordered: Vec<&str> = phrases.iter().map(AsRef::as_ref).collect();
    ordered.sort_by_key(|p| std::cmp::Reverse(p.split_whitespace().count()));
    for phrase in &ordered {
        let pw: Vec<&str> = phrase.split_whitespace().collect();
        let n = pw.len();
        if n == 0 || words.len() < n {
            continue;
        }
        for i in 0..=words.len() - n {
            if words[i..i + n].iter().zip(&pw).all(|(w, p)| w == p) {
                return Some(WakeMatch::new(phrase, &after(text, i + n)));
            }
        }
    }
    if fuzzy <= 0.0 {
        return None;
    }
    let mut best: Option<(f64, WakeMatch)> = None;
    for phrase in &ordered {
        let pw: Vec<&str> = phrase.split_whitespace().collect();
        let n = pw.len();
        if n == 0 || words.len() < n {
            continue;
        }
        let keys: Vec<String> = pw.iter().map(|w| phonetic(w)).collect();
        let key = keys.concat();
        // The distinctive word ("ghost"), which must be present: the first longest key.
        let anchor = keys.iter().fold(&keys[0], |best, k| {
            if k.chars().count() > best.chars().count() {
                k
            } else {
                best
            }
        });
        for i in 0..=words.len() - n {
            let window: Vec<String> = words[i..i + n].iter().map(|w| phonetic(w)).collect();
            let closest = window
                .iter()
                .map(|w| similar(w, anchor))
                .fold(f64::NEG_INFINITY, f64::max);
            if closest < 0.66 {
                continue; // "see you there" is not "ghost you there"
            }
            let score = similar(&window.concat(), &key);
            if score >= fuzzy && best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, WakeMatch::new(phrase, &after(text, i + n))));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// `words` begin with one of `openers`, each word exact or a phonetic near-miss.
fn opener_follows(words: &[String], openers: &[impl AsRef<str>]) -> bool {
    openers.iter().any(|opener| {
        let ow = self::words(opener.as_ref());
        words.len() >= ow.len()
            && words
                .iter()
                .zip(&ow)
                .all(|(w, o)| w == o || similar(&phonetic(w), &phonetic(o)) >= 0.8)
    })
}

/// `phrase` opening a sentence, or after one interjection ("so, Ghost, …"): the operator
/// addressing Ghost by name. The words after it are the request. Any sentence counts, since
/// what was heard can start a sentence before they turned to Ghost ("Let me try again.
/// Ghost, I need you …").
///
/// Unlike `match_wake` the name inside a sentence ("the ghost story") is no match. `fuzzy`
/// compares phonetic keys as `match_wake` does: at 0.8 "goes" is "ghost". With `openers`, a
/// first word merely shaped like the name (`NAME_SHAPE`, "Coach") also counts when one of
/// them follows it: the detector heard the name, and the opener's words are the part
/// Parakeet gets right.
pub fn match_opening(
    text: &str,
    phrase: &str,
    fuzzy: f64,
    openers: &[impl AsRef<str>],
) -> Option<WakeMatch> {
    let words = words(text);
    let pw: Vec<&str> = phrase.split_whitespace().collect();
    let n = pw.len();
    let key: String = pw.iter().map(|w| phonetic(w)).collect();
    let mut sentences = vec![0usize];
    for m in SENTENCE_END.find_iter(text) {
        sentences.push(self::words(&text[..m.end()]).len());
    }
    let mut seen = Vec::new();
    for s in sentences {
        if seen.contains(&s) {
            continue;
        }
        seen.push(s);
        let starts: Vec<usize> = if s < words.len() && OPENING_LEAD.contains(&words[s].as_str()) {
            vec![s, s + 1]
        } else {
            vec![s]
        };
        for i in starts {
            if i + n > words.len() {
                break;
            }
            let window = &words[i..i + n];
            let exact = window.iter().zip(&pw).all(|(w, p)| w == p);
            let near = fuzzy > 0.0
                && similar(
                    &window.iter().map(|w| phonetic(w)).collect::<String>(),
                    &key,
                ) >= fuzzy;
            if exact || near {
                return Some(WakeMatch::new(phrase, &after(text, i + n)));
            }
            let shaped = n == 1 && !openers.is_empty() && NAME_SHAPE.is_match(&window[0]);
            if shaped && opener_follows(&words[i + 1..], openers) {
                return Some(WakeMatch::new(phrase, &after(text, i + 1)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    //! `packages/voice/tests/test_wake.py`, the matcher's cases.

    use super::*;

    const PHRASES: &[&str] = &[
        "hey ghost",
        "ghost you there",
        "ghost are you there",
        "okay ghost",
    ];
    const NONE: &[&str] = &[];

    #[test]
    fn test_matches() {
        for (text, phrase, remainder) in [
            ("Hey Ghost.", "hey ghost", ""),
            ("hey, ghost!", "hey ghost", ""),
            ("Ghost, you there?", "ghost you there", ""),
            ("Ghost, are you there?", "ghost are you there", ""),
            (
                "Okay Ghost, what needs my sign-off?",
                "okay ghost",
                "what needs my sign-off?",
            ),
            ("um hey ghost what's pending", "hey ghost", "what's pending"),
        ] {
            let m = match_wake(text, PHRASES, 0.0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(m, WakeMatch::new(phrase, remainder), "{text}");
        }
    }

    #[test]
    fn test_non_matches() {
        for text in [
            "",
            "the ghost of christmas past",
            "hey, is the ghost build green",
            "you there ghost",
            "ghostly",
            "hey ghosts",
        ] {
            assert_eq!(match_wake(text, PHRASES, 0.0), None, "{text}");
        }
    }

    #[test]
    fn test_longest_phrase_wins() {
        let m = match_wake("hey ghost", &["ghost", "hey ghost"], 0.0).unwrap();
        assert_eq!(m.phrase, "hey ghost");
    }

    #[test]
    fn test_parse_phrases_defaults_and_normalises() {
        let defaults: Vec<String> = DEFAULT_WAKE_PHRASES.iter().map(|p| p.to_string()).collect();
        assert_eq!(parse_phrases(None), defaults);
        assert_eq!(parse_phrases(Some("  ,  ")), defaults);
        assert_eq!(
            parse_phrases(Some("Hey Ghost, Ghost: you there?")),
            vec!["hey ghost".to_string(), "ghost you there".to_string()]
        );
    }

    #[test]
    fn test_fuzzy_matches_asr_spellings() {
        for (heard, phrase, remainder) in [
            ("Hey guys.", "hey ghost", ""), // what Parakeet made of "hey ghost" through the AEC
            ("There he goes.", "hey ghost", ""),
            ("Hey ghosts.", "hey ghost", ""),
            ("He goes what a fear is.", "hey ghost", "what a fear is."),
            ("okay goes", "okay ghost", ""),
        ] {
            let m =
                match_wake(heard, DEFAULT_WAKE_PHRASES, 0.8).unwrap_or_else(|| panic!("{heard}"));
            assert_eq!(m, WakeMatch::new(phrase, remainder), "{heard}");
        }
    }

    #[test]
    fn test_fuzzy_rejects_unrelated_speech() {
        for heard in [
            "There you go.",
            "We go.",
            "No see you there.",
            "see you there",
            "hey there",
            "hey toast",
            "the ghost",
            "he gets it",
            "",
        ] {
            assert_eq!(
                match_wake(heard, DEFAULT_WAKE_PHRASES, 0.8),
                None,
                "{heard}"
            );
        }
    }

    #[test]
    fn test_strict_fuzzy_rejects_measured_false_wakes() {
        // Each woke Ghost at 0.8 in the 2026-10-21 measurements: LibriSpeech, then the
        // room's confusable sentences.
        for heard in [
            "He set off abruptly for the bull",
            "As she awaited her guests, she surveyed the table",
            "He sat down, weak, bewildered",
            "Here, they said is a rascal who has been harrying our coast",
            "Hey guys, what's up?",
            "He goes to the gym every morning.",
            "Hey Gus, how have you been?",
            "Okay, coast is clear.",
            "Hey goat cheese or cheddar.",
        ] {
            assert_eq!(
                match_wake(heard, DEFAULT_WAKE_PHRASES, 0.9),
                None,
                "{heard}"
            );
        }
    }

    #[test]
    fn test_strict_fuzzy_keeps_same_sound_spellings() {
        for heard in ["He ghost.", "Hay ghost, what's new?", "Ok, ghost."] {
            assert!(
                match_wake(heard, DEFAULT_WAKE_PHRASES, 0.9).is_some(),
                "{heard}"
            );
        }
    }

    #[test]
    fn test_fuzzy_off_by_default() {
        assert_eq!(match_wake("Hey guys.", DEFAULT_WAKE_PHRASES, 0.0), None);
    }

    #[test]
    fn test_exact_match_wins_over_fuzzy() {
        let m = match_wake("hey ghost close it", DEFAULT_WAKE_PHRASES, 0.8);
        assert_eq!(m, Some(WakeMatch::new("hey ghost", "close it")));
    }

    #[test]
    fn test_addressing_ghost_opens_with_the_name() {
        for (heard, asked) in [
            (
                "Ghost, what is the status of the infra repo?",
                "what is the status of the infra repo?",
            ),
            (
                "So, Ghost, let's work on the wake word.",
                "let's work on the wake word.",
            ),
            ("Ghost.", ""),
        ] {
            assert_eq!(
                match_opening(heard, "ghost", 0.9, NONE),
                Some(WakeMatch::new("ghost", asked)),
                "{heard}"
            );
        }
    }

    #[test]
    fn test_the_name_must_open_what_was_said() {
        for heard in [
            "The ghost story was good.", // the name inside a sentence
            "We should fix that bug in Ghost.",
            "Coast, I need you.", // not the name's sound
            "",
        ] {
            assert_eq!(match_opening(heard, "ghost", 0.8, NONE), None, "{heard}");
        }
    }

    #[test]
    fn test_the_name_may_open_a_later_sentence() {
        // The check reads from where the operator started talking, which can be a sentence
        // before they turned to Ghost.
        let m = match_opening(
            "Let me try again. Ghost, I need you to look.",
            "ghost",
            0.9,
            NONE,
        );
        assert_eq!(m, Some(WakeMatch::new("ghost", "I need you to look.")));
        assert_eq!(
            match_opening("I read it. The ghost story was good.", "ghost", 0.9, NONE),
            None
        );
    }

    const OPENERS: &[&str] = &["i need you", "let's work on", "what's the status of"];

    #[test]
    fn test_a_word_shaped_like_the_name_counts_when_an_opener_follows() {
        for (heard, asked) in [
            // Parakeet's own spelling of the operator's "Ghost" on 2026-10-24.
            (
                "Coach, I need you to work on the build.",
                "I need you to work on the build.",
            ),
            (
                "Coast, let's work on the wake word.",
                "let's work on the wake word.",
            ),
            (
                "Go. What's the status of the infra repo?",
                "What's the status of the infra repo?",
            ),
            ("So, goes, I need you.", "I need you."),
        ] {
            assert_eq!(
                match_opening(heard, "ghost", 0.9, OPENERS),
                Some(WakeMatch::new("ghost", asked)),
                "{heard}"
            );
        }
    }

    #[test]
    fn test_the_names_shape_alone_is_not_the_name() {
        for heard in [
            "Coach, can you check the build.", // the name's shape, but not an opener
            "Dave, I need you to look at this.", // an opener, but nothing like the name
            "I need you to work on some things.", // no name at all
            "Close the door, I need you to.",
        ] {
            assert_eq!(match_opening(heard, "ghost", 0.9, OPENERS), None, "{heard}");
            assert_eq!(match_opening(heard, "ghost", 0.9, NONE), None, "{heard}");
        }
    }

    #[test]
    fn test_a_looser_opening_takes_the_names_near_spellings() {
        // The check stays strict; splitting off the request may take "goes" for "ghost".
        assert_eq!(match_opening("Goes, I need you.", "ghost", 0.9, NONE), None);
        let loose = match_opening("Goes, I need you.", "ghost", 0.8, NONE);
        assert_eq!(loose, Some(WakeMatch::new("ghost", "I need you.")));
    }

    #[test]
    fn phonetic_keys_fold_the_confusions() {
        assert_eq!(phonetic("ghost"), "gst");
        assert_eq!(phonetic("goes"), "gs");
        assert_eq!(phonetic("guys"), "gs");
        assert_eq!(phonetic("hey"), "h");
        assert_eq!(phonetic("he"), "h");
        assert_eq!(phonetic(""), "");
    }
}
