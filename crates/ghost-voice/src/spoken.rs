//! What reaches the voice is speech, not a document.
//!
//! The model is told to answer in plain sentences, but tool results and app notices pull
//! it toward Markdown now and then (a Mobbin notice once put a `---` rule in a reply, which
//! Kokoro cannot say). This filter runs on each chunk of text the TTS is about to say,
//! after clause aggregation: it drops lines with nothing to say, unwraps links and
//! emphasis, and strips list and heading markers. It never rewrites words.

use std::sync::LazyLock;

use regex::Regex;

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`+([^`]*)`+").unwrap());
static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:#{1,6}\s+|[-*+]\s+|\d+[.)]\s+|>\s+)").unwrap());

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `**text**` / `__text__` -> text, only at word edges: `x**2` and `a_b__c` are not
/// emphasis. (Rust's regex has no look-around, so the edges are checked by hand.)
fn unemphasize(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let edge = |c: char| is_word(c) || c == '*' || c == '_';
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < n {
        let mark = if chars[i..].starts_with(&['*', '*']) {
            Some('*')
        } else if chars[i..].starts_with(&['_', '_']) {
            Some('_')
        } else {
            None
        };
        if let Some(m) = mark {
            let before_ok = i == 0 || !edge(chars[i - 1]);
            let inner = i + 2;
            if before_ok && inner < n && !chars[inner].is_whitespace() {
                // The closing pair: the shortest inner text, ending on a non-space, with
                // nothing word-like after the pair.
                let mut found = None;
                let mut j = inner + 1;
                while j + 1 < n {
                    if chars[j] == m
                        && chars[j + 1] == m
                        && !chars[j - 1].is_whitespace()
                        && (j + 2 >= n || !edge(chars[j + 2]))
                    {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(j) = found {
                    out.extend(&chars[inner..j]);
                    i = j + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The sayable part of `text`; "" when there is none.
pub fn spoken(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let line = LINK.replace_all(line, "$1");
        let line = unemphasize(&line);
        let line = CODE.replace_all(&line, "$1");
        let line = MARKER.replace(&line, "");
        let line = line.replace('|', " ");
        if !line.chars().any(|c| c.is_alphanumeric()) {
            continue; // a rule, a blank, a row of pipes
        }
        lines.push(line);
    }
    let mut out = lines.join("\n");
    // Keep the chunk's trailing space: the aggregator joins chunks with it.
    if text.ends_with(' ') && !out.is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// `spoken` on every chunk the TTS service is about to say.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpokenTextFilter;

impl SpokenTextFilter {
    pub fn filter(&self, text: &str) -> String {
        spoken(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drops_lines_with_nothing_to_say() {
        assert_eq!(spoken("---"), "");
        assert_eq!(spoken("***\n"), "");
        assert_eq!(spoken("| --- | --- |"), "");
    }

    #[test]
    fn test_unwraps_links_emphasis_and_code() {
        let text = "**Your usage is high**. See [the plan](https://example.com/billing) and `needs-triage`.";
        assert_eq!(
            spoken(text),
            "Your usage is high. See the plan and needs-triage."
        );
    }

    #[test]
    fn test_strips_list_heading_and_quote_markers() {
        assert_eq!(
            spoken("## Two apps\n- Subway\n- lululemon\n1. first\n> said"),
            "Two apps\nSubway\nlululemon\nfirst\nsaid"
        );
    }

    #[test]
    fn test_leaves_plain_speech_and_arithmetic_alone() {
        assert_eq!(
            spoken("Subway and lululemon both have one. "),
            "Subway and lululemon both have one. "
        );
        assert_eq!(
            spoken("3 * 4 is 12, minus 2 - 1."),
            "3 * 4 is 12, minus 2 - 1."
        );
        assert_eq!(spoken(""), "");
    }

    #[test]
    fn test_the_notice_that_broke_kokoro() {
        let reply = "Subway and lululemon both have one.\n\n---\n\n**Your Mobbin AI usage is high this \
                     month**\n\nUsage is unlimited during beta. [View usage](https://mobbin.com/x)";
        assert_eq!(
            spoken(reply),
            "Subway and lululemon both have one.\nYour Mobbin AI usage is high this month\n\
             Usage is unlimited during beta. View usage"
        );
    }

    #[test]
    fn test_emphasis_markers_only_at_word_edges() {
        assert_eq!(spoken("x**2 + y**2 = z**2"), "x**2 + y**2 = z**2");
        assert_eq!(spoken("a_b__c__d"), "a_b__c__d");
        assert_eq!(
            spoken("**Bold** start and __under__ score."),
            "Bold start and under score."
        );
    }
}
