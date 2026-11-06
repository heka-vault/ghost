//! Scoring what speech to text heard against what was said (`ghost-perf hear`).
//!
//! Deterministic and model-free: both sides go through [`normalize`] (lowercase, no
//! punctuation, fillers dropped, spoken numbers as digits), then a word alignment
//! ([`align`]) counts substitutions, deletions and insertions, each put in a class:
//! a number, one of the take's terms, a function word, or any other word. Over a set of
//! takes, [`paired_delta`] bootstraps the difference between two ways of hearing the same
//! audio, so a change is called better or worse only beyond the noise of the set.

use std::sync::LazyLock;

use regex::Regex;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9']+").unwrap());
static THOUSANDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d),(\d{3})").unwrap());

/// Hesitations, dropped from both sides: Ghost doesn't act on them, Parakeet keeps some
/// and not others, and #55 takes them out of the stored text. Counted apart
/// ([`fillers`]), never as errors.
pub const FILLERS: &[&str] = &["um", "uh", "uhm", "umm", "er", "erm", "hmm", "mm", "ah"];

/// Words that carry little content. Their errors are reported apart so a change that
/// trades "the" for "a" isn't read as one that loses issue numbers.
pub const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "it", "its", "is", "are", "was", "were", "be", "been", "as", "at", "in",
    "on", "of", "to", "and", "or", "but", "so", "that", "this", "these", "those", "i", "im", "me",
    "you", "we", "my", "your", "our", "with", "for", "from", "by", "do", "did", "have", "had",
    "can", "will", "what", "whats", "how", "any", "there", "theres",
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
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn ones(w: &str) -> Option<u64> {
    ONES.iter().position(|o| *o == w).map(|i| i as u64)
}

fn tens(w: &str) -> Option<u64> {
    TENS.iter()
        .position(|t| !t.is_empty() && *t == w)
        .map(|i| i as u64 * 10)
}

/// The words of a transcript as scored: lowercase, apostrophes and punctuation gone,
/// "sign-off" as two words, "ok" as "okay", fillers dropped, and every run of spoken
/// number words as digits ("four oh three" and "403" are both `403`; "one forty two",
/// "one hundred forty two" and "142" are `142`). Adjacent digit groups join ("4 0 3").
pub fn normalize(text: &str) -> Vec<String> {
    let lower = THOUSANDS
        .replace_all(&text.to_lowercase(), "$1$2")
        .replace(['-', '/'], " ");
    let raw: Vec<String> = WORD
        .find_iter(&lower)
        .map(|m| m.as_str().replace('\'', ""))
        .filter(|w| !w.is_empty() && !FILLERS.contains(&w.as_str()))
        .map(|w| if w == "ok" { "okay".to_string() } else { w })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if let Some((digits, used)) = number_run(&raw[i..]) {
            push_digits(&mut out, digits);
            i += used;
        } else {
            let w = raw[i].clone();
            if w.chars().all(|c| c.is_ascii_digit()) {
                push_digits(&mut out, w);
            } else {
                out.push(w);
            }
            i += 1;
        }
    }
    out
}

/// Digits join the digits before them: "4" "03" and "four oh three" score alike.
fn push_digits(out: &mut Vec<String>, digits: String) {
    match out.last_mut() {
        Some(last) if last.chars().all(|c| c.is_ascii_digit()) => last.push_str(&digits),
        _ => out.push(digits),
    }
}

/// One group of number words from the front of `ws`: its value and how many words it took.
/// "four hundred and three", "twenty seven", "forty", "twelve", "two thousand twenty six".
fn group(ws: &[String]) -> Option<(u64, usize)> {
    let mut i = 0;
    let mut total = 0u64;
    let mut current: Option<u64> = None;
    let mut any = false;
    while i < ws.len() {
        let w = ws[i].as_str();
        if let Some(t) = tens(w) {
            if current.is_some_and(|c| c % 100 != 0) {
                break; // "one forty": a new group
            }
            let mut v = t;
            if let Some(o) = ws
                .get(i + 1)
                .and_then(|n| ones(n))
                .filter(|o| (1..10).contains(o))
            {
                v += o;
                i += 1;
            }
            current = Some(current.unwrap_or(0) + v);
            any = true;
            i += 1;
        } else if let Some(o) = ones(w) {
            if current.is_some_and(|c| c % 100 != 0) || (current == Some(0) && o == 0) {
                break;
            }
            if o == 0 && current.is_some() {
                break;
            }
            current = Some(current.unwrap_or(0) + o);
            any = true;
            i += 1;
            if o == 0 {
                break; // "zero" is a digit on its own
            }
        } else if w == "hundred" && current.is_some_and(|c| c > 0 && c < 100) {
            current = Some(current.unwrap_or(0) * 100);
            i += 1;
            // "four hundred and three": the "and" belongs to the number
            if ws.get(i).is_some_and(|n| n == "and")
                && ws
                    .get(i + 1)
                    .is_some_and(|n| ones(n).is_some_and(|o| o > 0) || tens(n).is_some())
            {
                i += 1;
            }
        } else if w == "thousand" && current.is_some_and(|c| c > 0 && c < 1000) {
            total += current.unwrap_or(0) * 1000;
            current = None;
            i += 1;
        } else {
            break;
        }
    }
    any.then_some((total + current.unwrap_or(0), i))
}

/// A run of number groups said one after another, as digits: "one forty two" is 1 then
/// 42, so `142`; "four oh three" is 4, 0, 3; "twenty twenty six" is `2026`. "oh" is a
/// zero only after a group has started, so "oh, close it" keeps its "oh".
fn number_run(ws: &[String]) -> Option<(String, usize)> {
    let mut digits = String::new();
    let mut i = 0;
    loop {
        if i > 0 && ws.get(i).is_some_and(|w| w == "oh") {
            digits.push('0');
            i += 1;
            continue;
        }
        match group(&ws[i..]) {
            Some((v, used)) => {
                digits.push_str(&v.to_string());
                i += used;
            }
            None => break,
        }
    }
    (i > 0).then_some((digits, i))
}

/// How many fillers a transcript has, for the report (never an error).
pub fn fillers(text: &str) -> usize {
    WORD.find_iter(&text.to_lowercase())
        .filter(|m| FILLERS.contains(&m.as_str()))
        .count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Number,
    Term,
    Function,
    Content,
}

impl Class {
    pub const ALL: [Class; 4] = [Class::Number, Class::Term, Class::Function, Class::Content];

    pub fn name(self) -> &'static str {
        match self {
            Class::Number => "number",
            Class::Term => "term",
            Class::Function => "function word",
            Class::Content => "other word",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Hit(String),
    Sub(String, String),
    Del(String),
    Ins(String),
}

/// The fewest edits from `reference` to `hyp` (Levenshtein), as the edits themselves.
/// Among alignments with as few edits, the one with the most words matched wins, as a
/// person lining the two up would choose ("read me issue 403" against "read issue 404
/// now" is a dropped "me", a wrong number and an added "now", not three substitutions);
/// then the one whose substitutions keep a number for a number; remaining ties go to a
/// substitution, then a deletion, so it is the same every run.
pub fn align(reference: &[String], hyp: &[String]) -> Vec<Op> {
    let (n, m) = (reference.len(), hyp.len());
    // (edits, -matches, number-for-word substitutions): smaller is better, in that order
    type Cost = (usize, isize, usize);
    let mut d: Vec<Vec<Cost>> = vec![vec![(0, 0, 0); m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = (i, 0, 0);
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = (j, 0, 0);
    }
    let is_number = |w: &str| w.chars().all(|c| c.is_ascii_digit());
    let diag = |c: Cost, r: &str, h: &str| -> Cost {
        if r == h {
            (c.0, c.1 - 1, c.2)
        } else {
            (
                c.0 + 1,
                c.1,
                c.2 + usize::from(is_number(r) != is_number(h)),
            )
        }
    };
    let gap = |c: Cost| -> Cost { (c.0 + 1, c.1, c.2) };
    for i in 1..=n {
        for j in 1..=m {
            d[i][j] = diag(d[i - 1][j - 1], &reference[i - 1], &hyp[j - 1])
                .min(gap(d[i - 1][j]))
                .min(gap(d[i][j - 1]));
        }
    }
    let (mut i, mut j) = (n, m);
    let mut ops = Vec::with_capacity(n.max(m));
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let same = reference[i - 1] == hyp[j - 1];
            if d[i][j] == diag(d[i - 1][j - 1], &reference[i - 1], &hyp[j - 1]) {
                ops.push(if same {
                    Op::Hit(reference[i - 1].clone())
                } else {
                    Op::Sub(reference[i - 1].clone(), hyp[j - 1].clone())
                });
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && d[i][j] == gap(d[i - 1][j]) {
            ops.push(Op::Del(reference[i - 1].clone()));
            i -= 1;
        } else {
            ops.push(Op::Ins(hyp[j - 1].clone()));
            j -= 1;
        }
    }
    ops.reverse();
    ops
}

/// The terms of a take, normalized: each a run of words ("heka vault" is two).
pub fn term_words(terms: &[String]) -> Vec<Vec<String>> {
    terms
        .iter()
        .map(|t| normalize(t))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Which positions of `words` belong to one of `terms`.
fn term_positions(words: &[String], terms: &[Vec<String>]) -> Vec<bool> {
    let mut mark = vec![false; words.len()];
    for t in terms {
        if t.len() > words.len() {
            continue;
        }
        for start in 0..=words.len() - t.len() {
            if words[start..start + t.len()] == t[..] {
                mark[start..start + t.len()]
                    .iter_mut()
                    .for_each(|m| *m = true);
            }
        }
    }
    mark
}

fn class_of(word: &str, is_term: bool) -> Class {
    if is_term {
        Class::Term
    } else if word.chars().all(|c| c.is_ascii_digit()) {
        Class::Number
    } else if FUNCTION_WORDS.contains(&word) {
        Class::Function
    } else {
        Class::Content
    }
}

fn count_runs(words: &[String], term: &[String]) -> usize {
    if term.is_empty() || term.len() > words.len() {
        return 0;
    }
    (0..=words.len() - term.len())
        .filter(|&s| words[s..s + term.len()] == term[..])
        .count()
}

/// Errors in one class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassCount {
    /// Words of this class in the reference.
    pub words: usize,
    pub sub: usize,
    pub del: usize,
    /// Words of this class the transcript has that weren't said.
    pub ins: usize,
}

impl ClassCount {
    pub fn errors(&self) -> usize {
        self.sub + self.del + self.ins
    }
}

/// One transcript scored against its reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Score {
    pub ref_words: usize,
    pub hyp_words: usize,
    pub sub: usize,
    pub del: usize,
    pub ins: usize,
    pub by_class: [ClassCount; 4],
    /// Occurrences of the take's terms in the reference, and how many the transcript has.
    pub terms_said: usize,
    pub terms_heard: usize,
    /// The take's terms in the transcript more often than they were said.
    pub terms_inserted: usize,
    pub fillers_heard: usize,
}

impl Score {
    pub fn errors(&self) -> usize {
        self.sub + self.del + self.ins
    }

    pub fn class(&self, c: Class) -> &ClassCount {
        &self.by_class[c as usize]
    }
}

/// `hyp` against `reference`, the take's `terms` marking which words are terms.
pub fn score(reference: &str, hyp: &str, terms: &[String]) -> Score {
    let r = normalize(reference);
    let h = normalize(hyp);
    let tw = term_words(terms);
    let r_terms = term_positions(&r, &tw);
    let h_terms = term_positions(&h, &tw);
    let mut s = Score {
        ref_words: r.len(),
        hyp_words: h.len(),
        fillers_heard: fillers(hyp),
        ..Score::default()
    };
    for (i, w) in r.iter().enumerate() {
        s.by_class[class_of(w, r_terms[i]) as usize].words += 1;
    }
    let (mut ri, mut hi) = (0usize, 0usize);
    for op in align(&r, &h) {
        match op {
            Op::Hit(_) => {
                ri += 1;
                hi += 1;
            }
            Op::Sub(rw, _) => {
                s.sub += 1;
                s.by_class[class_of(&rw, r_terms[ri]) as usize].sub += 1;
                ri += 1;
                hi += 1;
            }
            Op::Del(rw) => {
                s.del += 1;
                s.by_class[class_of(&rw, r_terms[ri]) as usize].del += 1;
                ri += 1;
            }
            Op::Ins(hw) => {
                s.ins += 1;
                s.by_class[class_of(&hw, h_terms[hi]) as usize].ins += 1;
                hi += 1;
            }
        }
    }
    for t in &tw {
        let (said, heard) = (count_runs(&r, t), count_runs(&h, t));
        s.terms_said += said;
        s.terms_heard += heard.min(said);
        s.terms_inserted += heard.saturating_sub(said);
    }
    s
}

/// A small deterministic generator for the bootstrap (xorshift64*), so a report is the
/// same every time it is rendered from the same results.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n.max(1)
    }
}

/// The change in a rate from `a` to `b` over paired items, each item `(errors, words)`
/// under both: the point estimate and a 95% bootstrap interval, resampling items.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Delta {
    pub a: f64,
    pub b: f64,
    pub delta: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Better,
    Same,
    Worse,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Better => "better",
            Verdict::Same => "within noise",
            Verdict::Worse => "worse",
        }
    }
}

impl Delta {
    /// Lower is better for every rate here (errors, empties, made-up words).
    pub fn verdict(&self) -> Verdict {
        if self.high < 0.0 {
            Verdict::Better
        } else if self.low > 0.0 {
            Verdict::Worse
        } else {
            Verdict::Same
        }
    }
}

fn rate(items: &[(f64, f64)]) -> f64 {
    let (e, w) = items
        .iter()
        .fold((0.0, 0.0), |(e, w), (ie, iw)| (e + ie, w + iw));
    if w > 0.0 { e / w } else { 0.0 }
}

/// `a[i]` and `b[i]` are one item (a take) heard two ways: `(errors, words)`.
pub fn paired_delta(a: &[(f64, f64)], b: &[(f64, f64)], resamples: usize, seed: u64) -> Delta {
    assert_eq!(a.len(), b.len(), "paired items");
    let (ra, rb) = (rate(a), rate(b));
    let n = a.len();
    if n == 0 {
        return Delta {
            a: 0.0,
            b: 0.0,
            delta: 0.0,
            low: 0.0,
            high: 0.0,
        };
    }
    let mut rng = Rng::new(seed);
    let mut deltas: Vec<f64> = (0..resamples)
        .map(|_| {
            let (mut ea, mut wa, mut eb, mut wb) = (0.0, 0.0, 0.0, 0.0);
            for _ in 0..n {
                let k = rng.below(n);
                ea += a[k].0;
                wa += a[k].1;
                eb += b[k].0;
                wb += b[k].1;
            }
            let ra = if wa > 0.0 { ea / wa } else { 0.0 };
            let rb = if wb > 0.0 { eb / wb } else { 0.0 };
            rb - ra
        })
        .collect();
    deltas.sort_by(f64::total_cmp);
    let at =
        |q: f64| deltas[((q * (deltas.len() - 1) as f64).round() as usize).min(deltas.len() - 1)];
    Delta {
        a: ra,
        b: rb,
        delta: rb - ra,
        low: at(0.025),
        high: at(0.975),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Vec<String> {
        normalize(s)
    }

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn spoken_numbers_and_digits_normalize_alike() {
        for (said, heard) in [
            ("Read me issue four oh three.", "Read me issue 403."),
            ("Close issue one forty two", "close issue 142"),
            ("one hundred forty two", "142"),
            ("four hundred and three", "403"),
            ("four hundred three", "403"),
            ("issue twenty seven", "issue 27"),
            ("issue fifty two", "Issue 52."),
            ("four one three", "413"),
            ("twenty twenty six", "2026"),
            ("two thousand twenty six", "2026"),
            ("twelve hundred", "1,200"),
            ("issue four oh three", "issue 4 0 3"),
            ("sixty", "60"),
        ] {
            assert_eq!(n(said), n(heard), "{said:?} against {heard:?}");
        }
    }

    #[test]
    fn oh_is_a_zero_only_inside_a_number() {
        assert_eq!(n("Oh, close it."), w("oh close it"));
        assert_eq!(n("four oh nine"), w("409"));
    }

    #[test]
    fn a_correction_keeps_both_numbers_apart() {
        assert_eq!(
            n("Close issue twelve, no wait, thirteen."),
            w("close issue 12 no wait 13")
        );
        assert_eq!(n("forty, sorry, forty one"), w("40 sorry 41"));
    }

    #[test]
    fn fillers_punctuation_and_case_go() {
        assert_eq!(
            n("Um, what's waiting for, uh, triage?"),
            w("whats waiting for triage")
        );
        assert_eq!(n("sign-off"), w("sign off"));
        assert_eq!(n("OK."), w("okay"));
        assert_eq!(fillers("Um, what's waiting for, uh, triage?"), 2);
    }

    #[test]
    fn alignment_counts_each_kind_of_edit() {
        let s = score("read me issue four oh three", "read issue 404 now", &[]);
        assert_eq!((s.sub, s.del, s.ins), (1, 1, 1));
        assert_eq!(s.ref_words, 4);
        assert_eq!(s.class(Class::Number).sub, 1);
        assert_eq!(s.class(Class::Function).del, 1); // "me"
        assert_eq!(s.class(Class::Content).ins, 1); // "now"
    }

    #[test]
    fn a_perfect_read_has_no_errors() {
        let s = score(
            "What needs my sign-off today?",
            "what needs my sign off today",
            &[],
        );
        assert_eq!(s.errors(), 0);
        assert_eq!(s.ref_words, 6);
    }

    #[test]
    fn an_empty_read_deletes_every_word_and_silence_counts_inserts() {
        let s = score("Yes.", "", &[]);
        assert_eq!((s.del, s.ref_words, s.hyp_words), (1, 1, 0));
        let s = score("", "Thank you.", &[]);
        assert_eq!((s.ins, s.ref_words, s.hyp_words), (2, 0, 2));
    }

    #[test]
    fn terms_are_their_own_class_with_hits_and_insertions() {
        let terms = vec!["heka vault".to_string(), "GitHub".to_string()];
        let s = score(
            "Any new issues on heka vault ghost?",
            "any new issues on hekavault ghost",
            &terms,
        );
        assert_eq!(s.terms_said, 1);
        assert_eq!(s.terms_heard, 0);
        assert_eq!(s.class(Class::Term).words, 2);
        assert!(s.class(Class::Term).errors() >= 2);
        let s = score("open the repo", "open the GitHub repo", &terms);
        assert_eq!(s.terms_inserted, 1);
        assert_eq!(s.class(Class::Term).ins, 1);
    }

    #[test]
    fn the_bootstrap_is_deterministic_and_calls_a_clear_change() {
        let a: Vec<(f64, f64)> = (0..40)
            .map(|i| (if i % 2 == 0 { 2.0 } else { 1.0 }, 8.0))
            .collect();
        let b: Vec<(f64, f64)> = (0..40).map(|_| (0.0, 8.0)).collect();
        let d1 = paired_delta(&a, &b, 2000, 7);
        let d2 = paired_delta(&a, &b, 2000, 7);
        assert_eq!(d1, d2);
        assert!(d1.delta < 0.0);
        assert_eq!(d1.verdict(), Verdict::Better);
        assert_eq!(paired_delta(&b, &a, 2000, 7).verdict(), Verdict::Worse);
        let same = paired_delta(&a, &a, 2000, 7);
        assert_eq!(same.verdict(), Verdict::Same);
        assert_eq!(same.delta, 0.0);
    }

    #[test]
    fn noise_is_not_a_change() {
        // one take of forty differs by one word: not a measured change
        let a: Vec<(f64, f64)> = (0..40).map(|_| (1.0, 8.0)).collect();
        let mut b = a.clone();
        b[3].0 = 0.0;
        assert_eq!(paired_delta(&a, &b, 2000, 7).verdict(), Verdict::Same);
    }
}
