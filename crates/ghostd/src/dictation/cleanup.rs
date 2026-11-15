//! What Parakeet read, made into dictated text (ADR 0077): deterministic, no model, no list
//! the operator writes yet. Applied to dictated text only: a session's turn keeps its
//! fillers, which tell the turn logic a thought is not finished.
//!
//! In order: control characters out (so a line break still separates words), fillers out
//! with the capital and the sentence's end handed on, a word said three or more times in a
//! row kept once, and the final sanitizer, the last thing before the clipboard.

/// Said while thinking, never meant as text. Matched whole, any case but all capitals
/// ("ER" is a hospital, "UM" a university).
const FILLERS: [&str; 10] = [
    "um", "umm", "uh", "uhh", "uhm", "er", "erm", "hmm", "hm", "mmm",
];

/// A word said this many times in a row is kept once. Two stays: English says "that that"
/// and "had had".
const REPEAT_RUN: usize = 3;

/// The dictated text, cleaned. Empty when nothing is left.
pub fn clean(read: &str) -> String {
    let tokens: Vec<String> = sanitize(read)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let tokens = drop_fillers(tokens);
    let tokens = collapse_repeats(tokens);
    sanitize(&glue(tokens))
}

/// The last step before the clipboard: CR, LF, tab, vertical tab, form feed, NEL and the
/// Unicode line and paragraph separators become a space; NUL, ESC, DEL and every other
/// control character, and the bidirectional controls that reorder what a terminal shows,
/// go; runs of spaces become one, and the ends are trimmed. Nothing here can end a line, so
/// a paste can never submit a command.
pub fn sanitize(text: &str) -> String {
    let mapped: String = text
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                Some(' ')
            }
            c if c.is_control() => None,
            '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn is_filler(word: &str) -> bool {
    let lower = word.to_lowercase();
    let all_caps = word.chars().count() > 1 && word.chars().all(|c| c.is_uppercase());
    !all_caps && FILLERS.contains(&lower.as_str())
}

fn ends_sentence(token: &str) -> bool {
    token.ends_with(['.', '?', '!'])
}

fn capitalised(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut done = false;
    for c in token.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Fillers out. A capital at the start of a sentence passes to the next word ("Um, so I
/// think" → "So I think"); a sentence that ended on the filler ends on the word before it
/// ("I think, uh." → "I think."); a filler between commas takes them both ("I was, um,
/// going" → "I was going").
fn drop_fillers(tokens: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut capital_next = false;
    for token in tokens {
        let (_, word, trail) = split(&token);
        if word.is_empty() || !is_filler(word) {
            let token = if capital_next && !word.is_empty() {
                capital_next = false;
                capitalised(&token)
            } else {
                token
            };
            out.push(token);
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
                prev.push(end);
            }
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
fn collapse_repeats(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let (lead, word, _) = split(&tokens[i]);
        let key = word.to_lowercase();
        let mut j = i + 1;
        while !key.is_empty() && j < tokens.len() && split(&tokens[j]).1.to_lowercase() == key {
            j += 1;
        }
        if j - i >= REPEAT_RUN {
            let (_, _, trail) = split(&tokens[j - 1]);
            out.push(format!("{lead}{word}{trail}"));
        } else {
            out.extend(tokens[i..j].iter().cloned());
        }
        i = j;
    }
    out
}

/// The words with one space between, a lone mark that closes a clause joined to the word
/// before it.
fn glue(tokens: Vec<String>) -> String {
    let mut out = String::new();
    for t in tokens {
        let closes = !t.is_empty()
            && t.chars()
                .all(|c| matches!(c, ',' | '.' | ';' | ':' | '?' | '!'));
        if !out.is_empty() && !closes {
            out.push(' ');
        }
        out.push_str(&t);
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
            assert_eq!(clean(read), want, "{read:?}");
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
        assert_eq!(sanitize(&out), out, "the sanitizer is the last word");
    }
}
