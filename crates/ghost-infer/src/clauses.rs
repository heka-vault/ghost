//! Hand Kokoro a clause at a time instead of a sentence (`ghost_voice.clauses`).
//!
//! Kokoro renders a whole chunk before any of it plays, about 0.23 s per second of speech,
//! so a reply that opens with a long sentence was silent for seconds. This aggregator
//! releases at the end of a sentence, as Pipecat's `SimpleTextAggregator` does, and also at
//! a clause boundary (a comma, colon, semicolon or dash followed by a space) once the
//! pending text has `min_words` words. The punctuation stays with the clause, so Kokoro
//! still reads its intonation.
//!
//! Pipecat decides a sentence boundary with the `sentencex` tokenizer after a lookahead:
//! punctuation starts a candidate, the first following non-space character checks it, and
//! an unresolved one is checked again when the following word ends. The lookahead is
//! ported as is; the boundary rule here is English only (terminal punctuation followed by
//! a space, not an abbreviation, an initial or a decimal), not `sentencex`'s rule set.

pub const CLAUSE_MARKS: &[char] = &[',', ';', ':', '—', '–'];
pub const SENTENCE_ENDING_PUNCTUATION: &[char] = &['.', '!', '?', ';', '…', '。', '？', '！'];
/// Marks that end a sentence outside the English rules whenever they appear.
const UNAMBIGUOUS: &[char] = &['。', '？', '！'];
const ABBREVIATIONS: &[&str] = &[
    "dr", "mr", "mrs", "ms", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "no", "inc",
    "ltd", "co", "corp", "mt", "ft", "u.s", "a.m", "p.m", "fig", "vol", "dept", "est", "approx",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookahead {
    Idle,
    AwaitingCharacter,
    AwaitingWord,
    InWord,
}

/// Where the first sentence of `text` ends, in bytes, or 0 (`match_endofsentence`).
pub fn match_endofsentence(text: &str) -> usize {
    let text = text.trim_end();
    if text.is_empty() {
        return 0;
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(at, ch)) in chars.iter().enumerate() {
        if UNAMBIGUOUS.contains(&ch) {
            return at + ch.len_utf8();
        }
        if !matches!(ch, '.' | '!' | '?' | '…') {
            continue;
        }
        let end = at + ch.len_utf8();
        let last = k + 1 == chars.len();
        if !last {
            let next = chars[k + 1].1;
            if !next.is_whitespace() {
                continue; // "3.5", "e.g.", "..." mid-run
            }
        }
        if ch == '.' {
            let word: String = text[..at]
                .chars()
                .rev()
                .take_while(|c| !c.is_whitespace())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let word = word
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '.')
                .to_lowercase();
            if ABBREVIATIONS.contains(&word.as_str())
                || (word.len() == 1 && word.chars().all(|c| c.is_alphabetic()))
            {
                if last {
                    return 0;
                } else {
                    continue;
                };
            }
        }
        // An ellipsis ("…" or "...") trails off unless a capitalized sentence follows it.
        let ellipsis = ch == '…' || (ch == '.' && k > 0 && chars[k - 1].1 == '.');
        if ellipsis && !last {
            let next_word = chars[k + 1..]
                .iter()
                .map(|(_, c)| *c)
                .find(|c| !c.is_whitespace());
            if next_word.is_none_or(|c| !c.is_uppercase()) {
                continue;
            }
        }
        return end;
    }
    if text.ends_with(SENTENCE_ENDING_PUNCTUATION) {
        return text.len();
    }
    0
}

/// `ClauseTextAggregator` with `aggregation_type=SENTENCE`.
#[derive(Debug, Clone)]
pub struct ClauseAggregator {
    text: String,
    lookahead: Lookahead,
    min_words: Option<usize>,
}

impl ClauseAggregator {
    /// `min_words`: release at a clause boundary once this many words are pending; `None`:
    /// whole sentences only.
    pub fn new(min_words: Option<usize>) -> Self {
        Self {
            text: String::new(),
            lookahead: Lookahead::Idle,
            min_words,
        }
    }

    /// What is pending.
    pub fn pending(&self) -> &str {
        self.text.trim_matches(' ')
    }

    /// Add text; the sentences and clauses it completes, in order.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for ch in text.chars() {
            self.text.push(ch);
            if let Some(done) = self.check(ch) {
                out.push(done);
            }
        }
        out
    }

    /// Whatever is left, at the end of a reply.
    pub fn flush(&mut self) -> Option<String> {
        if self.text.is_empty() {
            return None;
        }
        let result = std::mem::take(&mut self.text);
        self.lookahead = Lookahead::Idle;
        Some(result.trim_matches(' ').to_string())
    }

    /// An interruption: drop what was pending.
    pub fn reset(&mut self) {
        self.text.clear();
        self.lookahead = Lookahead::Idle;
    }

    fn check(&mut self, ch: char) -> Option<String> {
        let is_punctuation = SENTENCE_ENDING_PUNCTUATION.contains(&ch);
        let mut result = None;
        if self.advance_lookahead(ch, is_punctuation) {
            let candidate = if is_punctuation {
                &self.text[..self.text.len() - ch.len_utf8()]
            } else {
                &self.text
            };
            let eos = match_endofsentence(candidate);
            if eos > 0 {
                result = Some(self.text[..eos].trim_matches(' ').to_string());
                self.text = self.text[eos..].to_string();
                self.lookahead = Lookahead::Idle;
            }
        }
        if is_punctuation {
            self.lookahead = Lookahead::AwaitingCharacter;
        }
        if result.is_some() {
            return result;
        }
        self.clause(ch)
    }

    fn advance_lookahead(&mut self, ch: char, is_punctuation: bool) -> bool {
        match self.lookahead {
            Lookahead::Idle => false,
            Lookahead::AwaitingCharacter => {
                if ch.is_whitespace() || is_punctuation {
                    return false;
                }
                self.lookahead = if ch.is_alphanumeric() {
                    Lookahead::InWord
                } else {
                    Lookahead::AwaitingWord
                };
                true
            }
            Lookahead::AwaitingWord => {
                if ch.is_alphanumeric() {
                    self.lookahead = Lookahead::InWord;
                }
                false
            }
            Lookahead::InWord => {
                if ch.is_whitespace() || is_punctuation {
                    self.lookahead = Lookahead::Idle;
                    return true;
                }
                false
            }
        }
    }

    fn clause(&mut self, ch: char) -> Option<String> {
        let min_words = self.min_words?;
        // A boundary is a mark followed by whitespace: "Three are waiting, the ..." but
        // not "3,000" or "owner/app:142". A spaced hyphen counts as a dash.
        if !ch.is_whitespace() {
            return None;
        }
        let chars: Vec<char> = self.text.chars().collect();
        if chars.len() < 2 {
            return None;
        }
        let prev = chars[chars.len() - 2];
        let dash = prev == '-' && chars.len() >= 3 && chars[chars.len() - 3] == ' ';
        if !CLAUSE_MARKS.contains(&prev) && !dash {
            return None;
        }
        let clause = self.text[..self.text.len() - ch.len_utf8()]
            .trim()
            .to_string();
        if clause.split_whitespace().count() < min_words {
            return None;
        }
        self.text.clear();
        // Don't keep a pending sentence check from before the clause (e.g. "Dr. Smith,").
        self.lookahead = Lookahead::Idle;
        Some(clause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(min_words: Option<usize>, pieces: &[&str]) -> Vec<String> {
        let mut agg = ClauseAggregator::new(min_words);
        let mut out = Vec::new();
        for p in pieces {
            out.extend(agg.push(p));
        }
        out.extend(agg.flush());
        out
    }

    #[test]
    fn sentences_are_released_at_the_first_lookahead_character() {
        let mut agg = ClauseAggregator::new(None);
        assert!(agg.push("Hello.").is_empty());
        assert!(agg.push(" ").is_empty());
        assert_eq!(agg.push("N"), vec!["Hello.".to_string()]);
        assert_eq!(agg.pending(), "N");
        assert_eq!(agg.push("ext one. And").len(), 1);
        assert_eq!(agg.flush(), Some("And".to_string()));
    }

    #[test]
    fn abbreviations_and_decimals_stay_buffered() {
        assert_eq!(
            run(None, &["Dr. Smith replied: not yet. Okay."]),
            vec!["Dr. Smith replied: not yet.", "Okay."]
        );
        assert_eq!(
            run(None, &["$29.99 a month, billed yearly."]),
            vec!["$29.99 a month, billed yearly."]
        );
        assert_eq!(
            run(None, &["Wait... are you sure?"]),
            vec!["Wait... are you sure?"]
        );
        assert_eq!(
            run(None, &["Wait... Are you sure?"]),
            vec!["Wait...", "Are you sure?"]
        );
    }

    #[test]
    fn clauses_release_after_min_words() {
        // "Three are waiting," is three words: under min_words it waits for the next clause.
        assert_eq!(
            run(
                Some(4),
                &["Three are waiting, the first one is red, and that's it."]
            ),
            vec!["Three are waiting, the first one is red,", "and that's it."]
        );
        assert_eq!(
            run(
                Some(3),
                &["Three are waiting, the first one is red, and that's it."]
            ),
            vec![
                "Three are waiting,",
                "the first one is red,",
                "and that's it."
            ]
        );
        assert_eq!(
            run(Some(4), &["Yes, it is. Done."]),
            vec!["Yes, it is.", "Done."]
        );
        assert_eq!(
            run(Some(4), &["Owner/app:142 is 3,000 lines long."]),
            vec!["Owner/app:142 is 3,000 lines long."]
        );
        assert_eq!(
            run(Some(2), &["Right - let's go."]),
            vec!["Right -", "let's go."]
        );
    }

    #[test]
    fn streamed_pieces_join_up() {
        let out = run(
            Some(4),
            &[
                "Hang on",
                ", Ghost. I",
                " need you to look",
                " at the build.",
            ],
        );
        assert_eq!(
            out,
            vec!["Hang on, Ghost.", "I need you to look at the build."]
        );
    }

    #[test]
    fn end_of_sentence_positions() {
        assert_eq!(match_endofsentence("Done."), 5);
        assert_eq!(match_endofsentence("Hello. Next"), 6);
        assert_eq!(match_endofsentence("Dr. Smith"), 0);
        assert_eq!(match_endofsentence("you and I. Did"), 0);
        assert_eq!(match_endofsentence("It is 3.5 wide"), 0);
        assert_eq!(match_endofsentence("Sure? Yes"), 5);
        assert_eq!(match_endofsentence(""), 0);
        assert_eq!(match_endofsentence("a; b"), 0);
        assert_eq!(match_endofsentence("a;"), 2);
    }
}
