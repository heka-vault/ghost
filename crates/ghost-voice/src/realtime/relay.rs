//! The faithful relay: what the realtime voice says of the agent's answer is checked word
//! by word against the answer as it is said (ADR 0063).
//!
//! The provider transcribes its own speech as it makes it ([`super::Event::Said`]). Both
//! texts are reduced to the same plain words (lower case, no punctuation, digits as words),
//! and the spoken ones must follow the answer's in order. A few words of filler may be
//! added ("okay", "so") and a few small words may be dropped ("the", "a"), never more than
//! [`MAX_FILLER`] and [`MAX_DROPPED`]; anything else is a divergence: a word that is not
//! the answer's, an inserted "not", a number that differs, a word of the answer missing.
//! The stage holds the relay's audio and lets it play only a little ahead of the words
//! verified ([`LEAD_WORDS`]), so a reading that strays is cut within a word or two and the
//! rest is said in Ghost's local voice ([`RelayCheck::rest`]).

/// Words the voice may add without changing what is said.
pub const FILLER: &[&str] = &[
    "okay", "ok", "so", "well", "alright", "right", "sure", "um", "uh", "oh", "and", "now", "here",
    "then", "just", "the", "a", "an",
];
/// Words of the answer the voice may leave out.
pub const DROPPABLE: &[&str] = &["the", "a", "an", "so", "just", "and", "that", "of"];
pub const MAX_FILLER: usize = 3;
pub const MAX_DROPPED: usize = 2;
/// How far ahead of the verified words the audio may play.
pub const LEAD_WORDS: usize = 2;
/// A spoken voice's pace, to turn words into seconds of audio.
pub const WORDS_PER_SEC: f64 = 2.6;

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
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn small_number(w: &str) -> Option<u64> {
    if let Some(i) = ONES.iter().position(|o| *o == w) {
        return Some(i as u64);
    }
    TENS.iter()
        .position(|t| !t.is_empty() && *t == w)
        .map(|i| i as u64 * 10)
}

fn is_number_word(w: &str) -> bool {
    small_number(w).is_some() || w == "hundred" || w == "thousand"
}

/// A run of number words as the digits it says: "four hundred twelve" and "four twelve"
/// are 412, "twelve hundred" is 1200, "twenty twenty six" is 2026.
fn fold(run: &[String]) -> String {
    if run.iter().any(|w| w == "hundred" || w == "thousand") {
        let (mut total, mut current) = (0u64, 0u64);
        for w in run {
            match w.as_str() {
                "hundred" => current = current.max(1) * 100,
                "thousand" => {
                    total += current.max(1) * 1000;
                    current = 0;
                }
                w => current += small_number(w).unwrap_or(0),
            }
        }
        return (total + current).to_string();
    }
    let mut out = String::new();
    let mut i = 0;
    while i < run.len() {
        let v = small_number(&run[i]).unwrap_or(0);
        if v >= 20
            && v.is_multiple_of(10)
            && let Some(next) = run.get(i + 1).and_then(|n| small_number(n))
            && (1..10).contains(&next)
        {
            out.push_str(&(v + next).to_string());
            i += 2;
            continue;
        }
        out.push_str(&v.to_string());
        i += 1;
    }
    out
}

/// One written token as plain words: lower case, punctuation gone, digits kept as digits.
fn token_words(token: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = token.to_lowercase();
    let mut cur = String::new();
    let mut digits = false;
    let chars: Vec<char> = lower.chars().collect();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for (i, c) in chars.iter().enumerate() {
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_ascii_digit() {
            if !digits {
                flush(&mut cur, &mut out);
                digits = true;
            }
            cur.push(*c);
        } else if *c == ',' && digits && next_digit {
            // 1,200
        } else if *c == '.' && digits && next_digit {
            flush(&mut cur, &mut out);
            out.push("point".into());
        } else if c.is_alphabetic() {
            if digits {
                flush(&mut cur, &mut out);
                digits = false;
            }
            cur.push(*c);
        } else if *c == '\'' || *c == '’' {
            // it's -> its, on both sides
        } else {
            flush(&mut cur, &mut out);
            digits = false;
            match c {
                '%' => out.push("percent".into()),
                '&' => out.push("and".into()),
                _ => {}
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// A text as plain words, and for each the index of the written token it came from.
/// Numbers come out as digits whether they were written or spelled.
pub fn words(text: &str) -> Vec<(String, usize)> {
    let mut raw: Vec<(String, usize)> = Vec::new();
    for (i, token) in text.split_whitespace().enumerate() {
        for w in token_words(token) {
            raw.push((w, i));
        }
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < raw.len() {
        if is_number_word(&raw[k].0) {
            let start = k;
            let mut run = Vec::new();
            while k < raw.len() {
                let w = &raw[k].0;
                if is_number_word(w) {
                    run.push(w.clone());
                    k += 1;
                } else if w == "and"
                    && run.iter().any(|r| r == "hundred" || r == "thousand")
                    && raw.get(k + 1).is_some_and(|n| is_number_word(&n.0))
                {
                    k += 1; // four hundred and twelve
                } else {
                    break;
                }
            }
            out.push((fold(&run), raw[start].1));
        } else {
            out.push(raw[k].clone());
            k += 1;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// So far the words follow the answer.
    Following,
    /// A word that is not the answer's: the reading strayed at the answer's word `at`.
    Diverged { at: usize, heard: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Every word of the answer was said, in order.
    Verified,
    /// The reading stopped short; the answer from word `at` was not said.
    Incomplete {
        at: usize,
    },
    Diverged {
        at: usize,
        heard: String,
    },
}

pub struct RelayCheck {
    text: String,
    expected: Vec<(String, usize)>,
    at: usize,
    filler: usize,
    dropped: usize,
    buf: String,
    done_words: usize,
    diverged: Option<(usize, String)>,
}

impl RelayCheck {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            expected: words(text),
            at: 0,
            filler: 0,
            dropped: 0,
            buf: String::new(),
            done_words: 0,
            diverged: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many of the answer's words were heard as written.
    pub fn verified(&self) -> usize {
        self.at
    }

    pub fn total(&self) -> usize {
        self.expected.len()
    }

    pub fn complete(&self) -> bool {
        self.diverged.is_none() && self.at >= self.expected.len()
    }

    fn take(&mut self, w: &str) -> Verdict {
        if let Some((at, heard)) = &self.diverged {
            return Verdict::Diverged {
                at: *at,
                heard: heard.clone(),
            };
        }
        let expect = |i: usize| self.expected.get(i).map(|(w, _)| w.as_str());
        if expect(self.at) == Some(w) {
            self.at += 1;
            return Verdict::Following;
        }
        // A small word of the answer left out.
        if let Some(skipped) = expect(self.at)
            && DROPPABLE.contains(&skipped)
            && expect(self.at + 1) == Some(w)
            && self.dropped < MAX_DROPPED
        {
            self.dropped += 1;
            self.at += 2;
            return Verdict::Following;
        }
        // A word of filler added.
        if FILLER.contains(&w) && self.filler < MAX_FILLER {
            self.filler += 1;
            return Verdict::Following;
        }
        self.diverged = Some((self.at, w.to_string()));
        Verdict::Diverged {
            at: self.at,
            heard: w.to_string(),
        }
    }

    /// The next piece of what the voice said. Only whole words are judged: a word cut by
    /// the delta waits for the next.
    pub fn feed(&mut self, delta: &str) -> Verdict {
        self.buf.push_str(delta);
        let ends_clean = self.buf.chars().last().is_some_and(|c| {
            c.is_whitespace() || (c.is_ascii_punctuation() && c != '\'' && c != '%')
        });
        let mut all = words(&self.buf);
        if !ends_clean {
            // the last written token may go on
            let last = all.last().map(|(_, i)| *i);
            all.retain(|(_, i)| Some(*i) != last);
        }
        let mut verdict = Verdict::Following;
        let new: Vec<String> = all
            .iter()
            .skip(self.done_words)
            .map(|(w, _)| w.clone())
            .collect();
        self.done_words = all.len();
        for w in new {
            verdict = self.take(&w);
            if matches!(verdict, Verdict::Diverged { .. }) {
                break;
            }
        }
        verdict
    }

    /// The voice finished: its whole transcript, when the provider sends one, settles the
    /// last word.
    pub fn finish(&mut self, whole: Option<&str>) -> Outcome {
        match whole {
            // The provider's whole transcript is the record: judged afresh from the start.
            Some(whole) => {
                let mut fresh = RelayCheck::new(&self.text);
                fresh.feed(whole);
                fresh.feed(" ");
                self.at = fresh.at;
                self.filler = fresh.filler;
                self.dropped = fresh.dropped;
                self.diverged = fresh.diverged;
            }
            None => {
                self.feed(" ");
            }
        }
        if let Some((at, heard)) = &self.diverged {
            return Outcome::Diverged {
                at: *at,
                heard: heard.clone(),
            };
        }
        // What is left may only be small words.
        let rest_small = self.expected[self.at.min(self.expected.len())..]
            .iter()
            .all(|(w, _)| DROPPABLE.contains(&w.as_str()));
        if self.at >= self.expected.len()
            || rest_small && self.expected.len() - self.at <= MAX_DROPPED
        {
            self.at = self.expected.len();
            Outcome::Verified
        } else {
            Outcome::Incomplete { at: self.at }
        }
    }

    /// The answer from its word `at` on, as written: what the local voice says when the
    /// reading strays or stops short.
    pub fn rest(&self, at: usize) -> String {
        let Some((_, token)) = self.expected.get(at) else {
            return String::new();
        };
        self.text
            .split_whitespace()
            .skip(*token)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The answer before its word `at`, as written: what was heard of it.
    pub fn head(&self, at: usize) -> String {
        let end = self
            .expected
            .get(at)
            .map_or(usize::MAX, |(_, token)| *token);
        self.text
            .split_whitespace()
            .take(end)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Seconds of the relay's audio that may have played, given the words verified.
    pub fn allowed_secs(&self) -> f64 {
        if self.complete() {
            return f64::INFINITY;
        }
        (self.at + LEAD_WORDS) as f64 / WORDS_PER_SEC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(answer: &str, spoken: &[&str]) -> (Vec<Verdict>, Outcome) {
        let mut c = RelayCheck::new(answer);
        let v = spoken.iter().map(|d| c.feed(d)).collect();
        let whole = spoken.concat();
        (v, c.finish(Some(&whole)))
    }

    #[test]
    fn the_answer_read_as_written_is_verified_in_pieces() {
        let (_, o) = said(
            "Three issues need you; the oldest is #412, from Monday.",
            &[
                "Three iss",
                "ues need you. The oldest is 412",
                ", from Monday.",
            ],
        );
        assert_eq!(o, Outcome::Verified);
    }

    #[test]
    fn digits_and_words_for_a_number_are_the_same_reading() {
        let (_, o) = said(
            "Issue 412 has 3 comments.",
            &["Issue four hundred twelve has three comments."],
        );
        assert_eq!(o, Outcome::Verified);
        let (_, o) = said("It is 1,200 dollars.", &["It's 1200 dollars."]);
        assert!(
            matches!(o, Outcome::Diverged { .. }),
            "it's is not it is: {o:?}"
        );
        let (_, o) = said("It's 1,200 dollars.", &["It's twelve hundred dollars."]);
        assert_eq!(o, Outcome::Verified);
        let (_, o) = said("Issue #412 is open.", &["Issue four twelve is open."]);
        assert_eq!(o, Outcome::Verified);
        let (_, o) = said("Issue 412 is open.", &["Issue 421 is open."]);
        assert!(
            matches!(o, Outcome::Diverged { .. }),
            "a different number is caught: {o:?}"
        );
    }

    #[test]
    fn a_little_filler_is_allowed_and_a_word_left_out_is_not() {
        let (_, o) = said("I closed issue 12.", &["Okay, so I closed issue 12."]);
        assert_eq!(o, Outcome::Verified);
        let (_, o) = said("I did not close issue 12.", &["I did close issue 12."]);
        assert!(matches!(o, Outcome::Diverged { at: 2, .. }), "{o:?}");
    }

    #[test]
    fn an_inserted_not_or_a_softened_refusal_is_a_divergence() {
        let (v, _) = said(
            "Your permissions block that.",
            &["Your permissions don't block that."],
        );
        assert!(v.iter().any(|v| matches!(v, Verdict::Diverged { .. })));
        let (_, o) = said(
            "That change was blocked by your permissions.",
            &["That change was made."],
        );
        assert!(matches!(o, Outcome::Diverged { .. }), "{o:?}");
    }

    #[test]
    fn a_reading_that_stops_short_leaves_the_rest_to_say() {
        let mut c = RelayCheck::new("The build failed on main, and two tests are red.");
        c.feed("The build failed on main, ");
        let o = c.finish(Some("The build failed on main,"));
        let Outcome::Incomplete { at } = o else {
            panic!("{o:?}")
        };
        assert_eq!(c.rest(at), "and two tests are red.");
    }

    #[test]
    fn the_rest_after_a_divergence_starts_at_the_word_that_was_not_said() {
        let mut c = RelayCheck::new("Ledger says the invoice was paid on Friday.");
        let v = c.feed("Ledger says the invoice was sent ");
        let Verdict::Diverged { at, heard } = v else {
            panic!("{v:?}")
        };
        assert_eq!(heard, "sent");
        assert_eq!(c.rest(at), "paid on Friday.");
    }

    #[test]
    fn audio_may_run_only_a_little_ahead_of_the_words_verified() {
        let mut c = RelayCheck::new("alpha bravo charlie delta echo foxtrot golf hotel");
        assert!((c.allowed_secs() - LEAD_WORDS as f64 / WORDS_PER_SEC).abs() < 1e-9);
        c.feed("alpha bravo charlie ");
        assert!((c.allowed_secs() - 5.0 / WORDS_PER_SEC).abs() < 1e-9);
        c.finish(Some("alpha bravo charlie delta echo foxtrot golf hotel"));
        assert!(c.allowed_secs().is_infinite());
    }
}
