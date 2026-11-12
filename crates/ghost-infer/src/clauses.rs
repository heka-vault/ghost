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

/// Words a reply's first piece may end before: each starts a phrase of its own, so the
/// voice can stop there as it would at a comma. Few on purpose: "or", "for" and "with" join
/// names and set phrases ("Redis or Postgres", "waiting for triage") too often.
pub const PHRASE_STARTS: &[&str] = &[
    "and", "but", "so", "because", "while", "though", "although", "since", "unless",
    "until",
];

/// Words that say a number: a cut after one would split "one forty two and ...".
const NUMBER_WORDS: &[&str] = &[
    "zero", "oh", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    "hundred", "thousand", "million", "billion",
];

/// Whether a reply's first piece may end after `word`: a lowercase word that is not a
/// number (digits or words), so the cut is never inside a name or a number.
fn ends_a_first_piece(word: &str) -> bool {
    let Some(c) = word.chars().next() else {
        return false;
    };
    c.is_lowercase()
        && word
            .chars()
            .all(|c| c.is_alphabetic() || c == '\'' || c == '\u{2019}' || c == '-')
        && !NUMBER_WORDS.contains(&word)
}

/// `ClauseTextAggregator` with `aggregation_type=SENTENCE`.
#[derive(Debug, Clone)]
pub struct ClauseAggregator {
    text: String,
    lookahead: Lookahead,
    min_words: Option<usize>,
    /// A reply's first piece may be cut once this many words are pending (0: no), at a
    /// clause mark or before a word of [`PHRASE_STARTS`], so its audio starts sooner.
    first_words: usize,
    /// Something of this reply was released (the first piece is behind it).
    released: bool,
    /// The last piece released was cut early.
    cut_early: bool,
}

impl ClauseAggregator {
    /// `min_words`: release at a clause boundary once this many words are pending; `None`:
    /// whole sentences only.
    pub fn new(min_words: Option<usize>) -> Self {
        Self {
            text: String::new(),
            lookahead: Lookahead::Idle,
            min_words,
            first_words: 0,
            released: false,
            cut_early: false,
        }
    }

    /// Cut each reply's first piece early, once `words` words are pending, at a clause mark
    /// or before a phrase's first word (0: no early cut). Kokoro renders a whole piece
    /// before any of it plays, so the first piece's length is the wait; the rest renders
    /// while it plays. Applies only when clauses are cut at all (`min_words` is some).
    pub fn with_first_clause(mut self, words: usize) -> Self {
        self.first_words = words;
        self
    }

    /// The last piece released was a reply's first, cut early before the usual rule
    /// would have cut it (`with_first_clause`).
    pub fn cut_early(&self) -> bool {
        self.cut_early
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
        self.released = false; // the next text is another reply's
        self.cut_early = false;
        Some(result.trim_matches(' ').to_string())
    }

    /// An interruption: drop what was pending.
    pub fn reset(&mut self) {
        self.text.clear();
        self.lookahead = Lookahead::Idle;
        self.released = false;
        self.cut_early = false;
    }

    fn check(&mut self, ch: char) -> Option<String> {
        if !self.released
            && self.first_words > 0
            && let Some(first) = self.first_piece(ch)
        {
            self.released = true;
            self.cut_early = true;
            return Some(first);
        }
        let out = self.check_inner(ch);
        if out.is_some() {
            self.released = true;
            self.cut_early = false;
        }
        out
    }

    /// A reply's first piece, cut early (`with_first_clause`): at a clause mark once
    /// `first_words` words are pending, or before a phrase's first word once that many
    /// words come before it and the word before it is no name or number. None when the
    /// usual rule would cut here anyway, or when a sentence check is pending.
    fn first_piece(&mut self, ch: char) -> Option<String> {
        let min_words = self.min_words?;
        if !ch.is_whitespace() || self.lookahead != Lookahead::Idle {
            return None;
        }
        let before = &self.text[..self.text.len() - ch.len_utf8()];
        if before.ends_with(char::is_whitespace) {
            return None;
        }
        let words: Vec<&str> = before.split_whitespace().collect();
        let (&last, rest) = words.split_last()?;
        // At a clause mark: "Three are waiting, ..."
        let prev = before.chars().last()?;
        let dash = last == "-";
        if CLAUSE_MARKS.contains(&prev) || dash {
            if words.len() < self.first_words || words.len() >= min_words {
                return None; // too short, or `clause` cuts it as it is
            }
            let piece = before.trim().to_string();
            self.text.clear();
            return Some(piece);
        }
        // Before a phrase's first word: "I found three issues waiting | and two ..."
        if rest.len() < self.first_words || !PHRASE_STARTS.contains(&last) {
            return None;
        }
        if !rest.last().is_some_and(|w| ends_a_first_piece(w)) {
            return None;
        }
        let cut = before.len() - last.len();
        let piece = before[..cut].trim().to_string();
        self.text = self.text[cut..].to_string();
        Some(piece)
    }

    fn check_inner(&mut self, ch: char) -> Option<String> {
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

    fn first(first_words: usize, min_words: Option<usize>, text: &str) -> Vec<String> {
        // Streamed a word at a time, as a model writes.
        let mut agg = ClauseAggregator::new(min_words).with_first_clause(first_words);
        let mut out = Vec::new();
        for piece in text.split_inclusive(' ') {
            out.extend(agg.push(piece));
        }
        out.extend(agg.flush());
        out
    }

    #[test]
    fn the_first_clause_of_a_reply_is_cut_early_at_a_comma() {
        // Three words and a comma are enough for the first piece; the rest of the reply
        // follows the usual rule (four words before a comma).
        assert_eq!(
            first(3, Some(4), "Three are waiting, the first one is red, and that's it."),
            vec![
                "Three are waiting,",
                "the first one is red,",
                "and that's it."
            ]
        );
        // Only the first: a second short clause waits for the usual four words.
        assert_eq!(
            first(3, Some(4), "Yes it is, one is red, two are blue."),
            vec!["Yes it is,", "one is red, two are blue."]
        );
    }

    #[test]
    fn the_first_clause_is_cut_before_a_conjunction() {
        assert_eq!(
            first(
                4,
                Some(4),
                "I found three issues waiting and two of them need your sign-off today."
            ),
            vec![
                "I found three issues waiting",
                "and two of them need your sign-off today."
            ]
        );
        assert_eq!(
            first(
                4,
                Some(4),
                "The build failed last night because the cache was cold."
            ),
            vec![
                "The build failed last night",
                "because the cache was cold."
            ]
        );
    }

    #[test]
    fn the_first_clause_is_not_cut_inside_a_name_or_a_number() {
        // A capitalized word before the conjunction is a name ("Redis and Postgres").
        assert_eq!(
            first(3, Some(4), "Choose between Redis and Postgres for the cache."),
            vec!["Choose between Redis and Postgres for the cache."]
        );
        // A number, in digits or in words, before it.
        assert_eq!(
            first(3, Some(4), "Issue one forty two and the webhook are next."),
            vec!["Issue one forty two and the webhook are next."]
        );
        assert_eq!(
            first(3, Some(4), "We closed 42 and kept the rest open."),
            vec!["We closed 42 and kept the rest open."]
        );
        // After a number's noun is a phrase's end like any other.
        assert_eq!(
            first(3, Some(4), "There are 3,000 users and most are active."),
            vec!["There are 3,000 users", "and most are active."]
        );
        // "3,000" and "10:30" are not clause marks, early or not.
        assert_eq!(
            first(2, Some(4), "At 10:30 the build ran."),
            vec!["At 10:30 the build ran."]
        );
        // Too few words before the break: no cut.
        assert_eq!(
            first(3, Some(4), "Sure, approved it."),
            vec!["Sure, approved it."]
        );
        assert_eq!(
            first(4, Some(4), "Rain and snow fell all day long."),
            vec!["Rain and snow fell all day long."]
        );
    }

    #[test]
    fn every_reply_has_its_own_first_clause_and_off_is_the_old_rule() {
        let mut agg = ClauseAggregator::new(Some(4)).with_first_clause(3);
        let mut out = agg.push("Three are waiting, the rest wait. ");
        out.extend(agg.flush());
        // The next reply (after a flush) is cut early again.
        out.extend(agg.push("Two are open, one is closed."));
        out.extend(agg.flush());
        assert_eq!(
            out,
            vec![
                "Three are waiting,",
                "the rest wait.",
                "Two are open,",
                "one is closed."
            ]
        );
        // A sentence released first counts as the reply's first piece.
        assert_eq!(
            first(3, Some(4), "Done. Three are waiting, and more."),
            vec!["Done.", "Three are waiting, and more."]
        );
        // Off (0): what the aggregator did before.
        assert_eq!(
            first(0, Some(4), "Three are waiting, the first one is red, and that's it."),
            run(
                Some(4),
                &["Three are waiting, the first one is red, and that's it."]
            )
        );
        // Whole sentences only (`None`): no early cut either.
        assert_eq!(
            first(3, None, "Three are waiting, the first one is red."),
            vec!["Three are waiting, the first one is red."]
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
