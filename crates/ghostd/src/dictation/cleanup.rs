//! What Parakeet read, made into dictated text (ADR 0077): deterministic, no model, no list
//! the operator writes yet. Applied to dictated text only: a session's turn keeps its
//! fillers, which tell the turn logic a thought is not finished.
//!
//! In order: control characters out (so a line break still separates words), fillers out
//! with the capital and the sentence's end handed on, a word said three or more times in a
//! row kept once, and the final sanitizer, the last thing before the clipboard.

use zeroize::Zeroizing;

/// Said while thinking, never meant as text. Matched whole, any case but all capitals
/// ("ER" is a hospital, "UM" a university).
const FILLERS: [&str; 10] = [
    "um", "umm", "uh", "uhh", "uhm", "er", "erm", "hmm", "hm", "mmm",
];

/// A word said this many times in a row is kept once. Two stays: English says "that that"
/// and "had had".
const REPEAT_RUN: usize = 3;

/// Room for what a token may gain (a moved full stop, a capital that is longer).
const SLACK: usize = 8;

type Tokens = Zeroizing<Vec<String>>;

/// The dictated text, cleaned. Empty when nothing is left.
///
/// Every string made on the way is wiped when it is dropped: the sanitized copy, each token
/// (sized once with room to spare, so none grows and leaves a copy), and the joined text.
/// Comparisons are made without making lowercase copies.
pub fn clean(read: &str) -> Zeroizing<String> {
    let sane = sanitize(read);
    let mut tokens: Tokens = Zeroizing::new(Vec::with_capacity(sane.len() / 2 + 1));
    for t in sane.split(' ').filter(|t| !t.is_empty()) {
        let mut owned = String::with_capacity(t.len() + SLACK);
        owned.push_str(t);
        tokens.push(owned);
    }
    drop(sane);
    let tokens = drop_fillers(tokens);
    let tokens = collapse_repeats(tokens);
    sanitize(&glue(tokens))
}

/// The last step before the clipboard: CR, LF, tab, vertical tab, form feed, NEL and the
/// Unicode line and paragraph separators become a space; NUL, ESC, DEL and every other
/// control character, and the bidirectional controls that reorder what a terminal shows,
/// go; runs of spaces become one, and the ends are trimmed. Nothing here can end a line, so
/// a paste can never submit a command. One pass into a string sized for the input, which it
/// never outgrows.
pub fn sanitize(text: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(text.len()));
    let mut space = false;
    for c in text.chars() {
        let c = match c {
            '\r' | '\n' | '\t' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => ' ',
            c if c.is_control() => continue,
            '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => continue,
            c => c,
        };
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// A token's leading punctuation, its word, and its trailing punctuation.
fn split(token: &str) -> (&str, &str, &str) {
    let start = token
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, _)| i);
    let Some(start) = start else {
        return (token, "", "");
    };
    let end = token
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(token.len());
    (&token[..start], &token[start..end], &token[end..])
}

/// The same word in any case, compared without a lowercase copy.
fn same_word(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

fn is_filler(word: &str) -> bool {
    let all_caps = word.chars().count() > 1 && word.chars().all(|c| c.is_uppercase());
    !all_caps && FILLERS.iter().any(|f| same_word(word, f))
}

fn ends_sentence(token: &str) -> bool {
    token.ends_with(['.', '?', '!'])
}

/// The token with its word's first letter a capital, when the word begins with a letter
/// ("5mg" stays "5mg"); `None` when there is nothing to change.
fn capitalised(token: &str) -> Option<String> {
    let (lead, word, trail) = split(token);
    let first = word
        .chars()
        .next()
        .filter(|c| c.is_alphabetic() && c.is_lowercase())?;
    let mut out = String::with_capacity(token.len() + SLACK);
    out.push_str(lead);
    out.extend(first.to_uppercase());
    out.push_str(&word[first.len_utf8()..]);
    out.push_str(trail);
    Some(out)
}

/// Fillers out. A capital at the start of a sentence passes to the next word ("Um, so I
/// think" → "So I think"), when that word begins with a letter; a sentence that ended on
/// the filler ends on the word before it, and what follows begins a sentence ("We finished,
/// uh. next item." → "We finished. Next item."); a filler between commas takes them both
/// ("I was, um, going" → "I was going").
fn drop_fillers(tokens: Tokens) -> Tokens {
    let mut tokens = tokens;
    let mut out: Tokens = Zeroizing::new(Vec::with_capacity(tokens.len()));
    let mut capital_next = false;
    for token in tokens.drain(..) {
        let token = Zeroizing::new(token);
        let (_, word, trail) = split(&token);
        if word.is_empty() || !is_filler(word) {
            if word.is_empty() {
                let mut t = String::with_capacity(token.len() + SLACK);
                t.push_str(&token);
                out.push(t);
                continue;
            }
            let kept = if capital_next {
                capital_next = false;
                capitalised(&token)
            } else {
                None
            };
            out.push(kept.unwrap_or_else(|| {
                let mut t = String::with_capacity(token.len() + SLACK);
                t.push_str(&token);
                t
            }));
            continue;
        }
        let starts = out.last().is_none_or(|t| ends_sentence(t));
        if starts && word.chars().next().is_some_and(char::is_uppercase) {
            capital_next = true;
        }
        if let Some(end) = trail.chars().find(|c| matches!(c, '.' | '?' | '!')) {
            if let Some(prev) = out.last_mut()
                && !ends_sentence(prev)
            {
                let kept = prev.trim_end_matches([',', ';', ':']).len();
                prev.truncate(kept);
                prev.push(end); // within the token's room: no copy left behind
            }
            capital_next = true; // a sentence ended here: the next word begins one
        } else if trail.contains(',')
            && let Some(prev) = out.last_mut()
            && prev.ends_with(',')
        {
            prev.pop();
        }
    }
    out
}

/// A word said three or more times in a row, kept once: the first one's capital and
/// leading punctuation, the last one's trailing punctuation.
fn collapse_repeats(tokens: Tokens) -> Tokens {
    let mut out: Tokens = Zeroizing::new(Vec::with_capacity(tokens.len()));
    let mut i = 0;
    while i < tokens.len() {
        let (lead, word, _) = split(&tokens[i]);
        let mut j = i + 1;
        while !word.is_empty() && j < tokens.len() && same_word(split(&tokens[j]).1, word) {
            j += 1;
        }
        if j - i >= REPEAT_RUN {
            let (_, _, trail) = split(&tokens[j - 1]);
            let mut t = String::with_capacity(lead.len() + word.len() + trail.len());
            t.push_str(lead);
            t.push_str(word);
            t.push_str(trail);
            out.push(t);
        } else {
            for t in &tokens[i..j] {
                let mut c = String::with_capacity(t.len());
                c.push_str(t);
                out.push(c);
            }
        }
        i = j;
    }
    out
}

/// The words with one space between, a lone mark that closes a clause joined to the word
/// before it, in a string sized once.
fn glue(tokens: Tokens) -> Zeroizing<String> {
    let size = tokens.iter().map(|t| t.len() + 1).sum();
    let mut out = Zeroizing::new(String::with_capacity(size));
    for t in tokens.iter() {
        let closes = !t.is_empty()
            && t.chars()
                .all(|c| matches!(c, ',' | '.' | ';' | ':' | '?' | '!'));
        if !out.is_empty() && !closes {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cleanup_table() {
        let table = [
            // fillers
            (
                "Um, so I think we should ship.",
                "So I think we should ship.",
            ),
            (
                "So, um, I think we should ship.",
                "So I think we should ship.",
            ),
            ("I was, um, going to say", "I was going to say"),
            ("I think, uh.", "I think."),
            ("Done. Uh, next thing.", "Done. Next thing."),
            ("Hmm. Let me see.", "Let me see."),
            ("uh hmm um", ""),
            ("Uhm, erm, the plan", "The plan"),
            ("We went to the ER today.", "We went to the ER today."),
            ("UM is a fine school.", "UM is a fine school."),
            ("Is it ready, er?", "Is it ready?"),
            // a capital passes only to a word that begins with a letter (finding 9)
            ("Um, 5mg daily.", "5mg daily."),
            ("Uh, \"quoted\" word", "\"Quoted\" word"),
            // a sentence end moved off a filler capitalises what follows it (finding 9)
            ("We finished, uh. next item.", "We finished. Next item."),
            ("Hummus and umbrellas", "Hummus and umbrellas"),
            // repeats
            ("I I I think so.", "I think so."),
            ("I, I, I think so.", "I think so."),
            ("The the the cat sat.", "The cat sat."),
            ("I said that that was fine.", "I said that that was fine."),
            ("It had had a long day.", "It had had a long day."),
            ("no no no no.", "no."),
            // spacing and capitals after removals
            ("  Well   um   then  ", "Well then"),
            ("Um. Okay.", "Okay."),
            ("Ok , fine", "Ok, fine"),
            // controls
            ("first line\nsecond line", "first line second line"),
            ("a\r\nb\tc", "a b c"),
            ("rm -rf\u{0}\u{1b}[31m red", "rm -rf[31m red"),
            ("one\u{2028}two\u{2029}three", "one two three"),
            ("bidi \u{202E}evil\u{202C} text", "bidi evil text"),
            ("del\u{7f}ete", "delete"),
            ("", ""),
        ];
        for (read, want) in table {
            assert_eq!(clean(read).as_str(), want, "{read:?}");
        }
    }

    #[test]
    fn nothing_that_ends_a_line_or_is_a_control_survives() {
        let mut all = String::new();
        for c in (0u32..0x2100).filter_map(char::from_u32) {
            all.push(c);
            all.push('x');
        }
        let out = clean(&all);
        assert!(!out.chars().any(|c| c.is_control()), "a control survived");
        assert!(!out.contains(['\u{2028}', '\u{2029}', '\u{202E}', '\u{2066}']));
        assert!(!out.contains("  "));
        assert_eq!(
            sanitize(&out).as_str(),
            out.as_str(),
            "the sanitizer is the last word"
        );
    }
}
