//! The operator's own request as the yes for a `confirm`-level change (ADR 0017).
//!
//! "Close infra 17" already says yes to closing infra 17; asking "Okay to close infra 17?"
//! after it is ceremony. When the gate holds a confirm-level proposal, it reads the
//! operator's words that led to it (the pipeline's transcript, never the model's text) and
//! executes at once when they plainly ask for exactly that:
//!
//! - a word for the operation (`Operation.asked_by`: "close", "shut" for close; an app's
//!   come from the tool's name and short arguments, ADR 0034). One with none always asks;
//! - the target, named in the request ("17", "seventeen", `Target::mentions`), or "it",
//!   "this", "that one" right after Ghost's last reply named it. What names it is what the
//!   change is about: the repository or the branch it happens in never does;
//! - nothing hedged or negated: no "no", "don't", "not", "wait", "should", "maybe", "if";
//! - not a question, except a polite request ("can you reopen seventeen?").
//!
//! The operation word must be said as is: "close 17" asks, "is 17 closed" does not.
//! Otherwise Ghost asks once, as before. This is the operator's request, observed: the
//! model proposing the change is never evidence that the operator asked for it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9']+").unwrap());

const HEDGES: &[&str] = &[
    "no",
    "not",
    "don't",
    "dont",
    "never",
    "wait",
    "stop",
    "cancel",
    "should",
    "shouldn't",
    "maybe",
    "might",
    "if",
    "whether",
    "think",
    "probably",
];
const POINTERS: &[&str] = &["it", "this", "that", "one"];
const FILLERS: &[&str] = &[
    "hey", "ghost", "um", "uh", "so", "okay", "ok", "yeah", "and", "please",
];
/// "Can you close 17?" is a request, however the transcript punctuates it.
const POLITE: &[(&str, &str)] = &[
    ("can", "you"),
    ("could", "you"),
    ("would", "you"),
    ("will", "you"),
    ("can", "we"),
];

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "_", "_", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// The words of a transcript: lowercase, letters, digits and apostrophes.
pub fn words(text: &str) -> Vec<String> {
    WORD.find_iter(&text.to_lowercase())
        .map(|m| m.as_str().to_string())
        .collect()
}

fn under_100(n: u32) -> String {
    if n < 20 {
        return ONES[n as usize].to_string();
    }
    let (tens, ones) = (n / 10, n % 10);
    if ones == 0 {
        TENS[tens as usize].to_string()
    } else {
        format!("{} {}", TENS[tens as usize], ONES[ones as usize])
    }
}

/// How a number can come back from STT: "142", "one forty two", "one hundred forty two".
pub fn number_mentions(n: i64) -> BTreeSet<String> {
    let mut out = BTreeSet::from([n.to_string()]);
    if (0..100).contains(&n) {
        out.insert(under_100(n as u32));
    } else if (100..1000).contains(&n) {
        let (hundreds, rest) = ((n / 100) as u32, (n % 100) as u32);
        let h = ONES[hundreds as usize];
        if rest != 0 {
            let r = under_100(rest);
            out.insert(format!("{h} hundred {r}"));
            out.insert(format!("{h} hundred and {r}"));
            out.insert(format!("{h} {r}"));
            if rest < 10 {
                out.insert(format!("{h} oh {r}"));
            }
        } else {
            out.insert(format!("{h} hundred"));
        }
    }
    out
}

fn contains(haystack: &[String], phrase: &str) -> bool {
    let needle: Vec<&str> = phrase.split_whitespace().collect();
    let n = needle.len();
    if n == 0 || haystack.len() < n {
        return false;
    }
    haystack
        .windows(n)
        .any(|w| w.iter().map(String::as_str).eq(needle.iter().copied()))
}

/// The sentences of a request: split after `.`, `!` or `?` followed by whitespace.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?')
            && bytes.get(i + 1).is_some_and(u8::is_ascii_whitespace)
        {
            out.push(&text[start..=i]);
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            start = j;
            i = j;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Whether `request` plainly asks for this operation on this target.
pub fn asked_for<'a>(
    request: &str,
    verbs: impl IntoIterator<Item = &'a str>,
    mentions: impl IntoIterator<Item = &'a str>,
    context: &str,
) -> bool {
    let said = words(request);
    let verbs: Vec<String> = verbs.into_iter().map(str::to_lowercase).collect();
    // Compared word by word, as the transcript is: "README.md" is "readme md".
    let mentions: Vec<String> = mentions
        .into_iter()
        .map(|m| words(m).join(" "))
        .filter(|m| !m.is_empty())
        .collect();
    if said.is_empty() || verbs.is_empty() || mentions.is_empty() {
        return false;
    }
    if said.iter().any(|w| HEDGES.contains(&w.as_str())) {
        return false;
    }
    if !said.iter().any(|w| verbs.contains(w)) {
        return false;
    }
    // Judge the sentence that asks: "Okay, that's fine. Can we close seventeen, please?"
    let trimmed = request.trim();
    let asking = sentences(trimmed)
        .into_iter()
        .find(|s| words(s).iter().any(|w| verbs.contains(w)))
        .unwrap_or(trimmed);
    if asking.ends_with('?') {
        let lead: Vec<String> = words(asking)
            .into_iter()
            .filter(|w| !FILLERS.contains(&w.as_str()))
            .take(2)
            .collect();
        let polite = lead.len() == 2 && POLITE.contains(&(lead[0].as_str(), lead[1].as_str()));
        if !polite {
            return false;
        }
    }
    if mentions.iter().any(|m| contains(&said, m)) {
        return true;
    }
    let ghost = words(context);
    said.iter().any(|w| POINTERS.contains(&w.as_str()))
        && mentions.iter().any(|m| contains(&ghost, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOSE: [&str; 2] = ["close", "shut"];

    fn n17() -> Vec<String> {
        number_mentions(17).into_iter().collect()
    }

    #[test]
    fn number_mentions_cover_how_stt_writes_them() {
        let n17 = number_mentions(17);
        assert!(n17.contains("17") && n17.contains("seventeen"));
        let n142 = number_mentions(142);
        for m in [
            "142",
            "one forty two",
            "one hundred forty two",
            "one hundred and forty two",
        ] {
            assert!(n142.contains(m), "{m}");
        }
        assert!(number_mentions(105).contains("one oh five"));
        assert!(number_mentions(300).contains("three hundred"));
        assert_eq!(number_mentions(1234).len(), 1);
    }

    #[test]
    fn asked_for_follows_the_operators_words() {
        let cases: &[(&str, &str, bool)] = &[
            ("Close infra 17.", "", true),
            (
                "close seventeen with a note saying we won't do it",
                "",
                true,
            ),
            ("go ahead and close one forty two", "", false), // wrong number for 17
            ("can you close 17", "", true),
            (
                "close it",
                "Infra 17 is the Discord webhook rotation.",
                true,
            ),
            (
                "yeah close that one",
                "Infra 17 is the Discord webhook rotation.",
                true,
            ),
            ("close it", "App 151 is the dark mode toggle.", false), // "it" isn't 17
            ("close it", "", false),
            // hedged, negated, asked: Ghost asks
            ("should we close 17", "", false),
            ("maybe close 17", "", false),
            ("don't close 17", "", false),
            (
                "Not closes, we're not gonna do it.",
                "Close infra 17?",
                false,
            ),
            ("close 17?", "", false),
            ("Hey, can you um reopen infra number seventeen?", "", false), // reopen isn't close
            ("Hey, can you um close infra number seventeen?", "", true),
            ("could you close 17?", "", true),
            (
                "Okay, that's fine. Can we close number seventeen, please?",
                "",
                true,
            ),
            ("That's fine. Is 17 closed?", "", false),
            ("is 17 closed", "", false),
            ("is 17 closed?", "", false),
            ("what happens if you close 17?", "", false),
            ("I think we close 17", "", false),
            // no operation word: it was inferred
            ("we're not gonna do infra 17", "", false),
            ("label 17 bug", "", false),
        ];
        for (request, context, expect) in cases {
            let mentions = n17();
            let got = asked_for(request, CLOSE, mentions.iter().map(String::as_str), context);
            assert_eq!(got, *expect, "{request:?} with context {context:?}");
        }
    }

    #[test]
    fn an_operation_without_words_never_asks_for_itself() {
        let mentions = n17();
        assert!(!asked_for(
            "close 17",
            [],
            mentions.iter().map(String::as_str),
            ""
        ));
    }

    #[test]
    fn sentences_split_after_punctuation_and_space() {
        assert_eq!(
            sentences("Okay. Close 17, please?"),
            vec!["Okay.", "Close 17, please?"]
        );
        assert_eq!(sentences("one sentence"), vec!["one sentence"]);
        assert_eq!(
            sentences("v1.2 is out. close 17."),
            vec!["v1.2 is out.", "close 17."]
        );
    }
}
