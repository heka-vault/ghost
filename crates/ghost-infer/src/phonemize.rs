//! `phonemizer.phonemize(text, "en-us", preserve_punctuation=True, with_stress=True)` with
//! the espeak backend, as `kokoro_onnx.tokenizer.Tokenizer.phonemize` calls it: the
//! punctuation is lifted out of the text (`Punctuation.preserve`), each piece goes to
//! espeak-ng, the pieces are cleaned (`EspeakBackend._postprocess_line`), and the marks go
//! back (`Punctuation.restore`). Every quirk is kept, since the phonemes have to be the
//! same string: the pieces are cut by `str.split` on the mark's text, so a decimal point
//! inside a number can be cut at instead of the full stop that matched.

use std::sync::LazyLock;

use regex::Regex;

use crate::{Result, espeak::Espeak};

/// `Punctuation.default_marks()`.
pub const DEFAULT_MARKS: &str = ";:,.!?¡¿—…\"«»“”(){}[]";
const DECIMAL_SEPARATORS: &str = ",.";

static UNDERSCORES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_+").expect("fixed"));

#[derive(Debug, Clone, PartialEq)]
struct Mark {
    /// The line it came from.
    index: usize,
    mark: String,
    /// Begin, End, In the middle, or Alone.
    position: char,
}

fn is_mark(chars: &[char], i: usize) -> bool {
    let c = chars[i];
    if !DEFAULT_MARKS.contains(c) {
        return false;
    }
    if DECIMAL_SEPARATORS.contains(c) {
        // A separator between two digits is part of a number.
        let before = i > 0 && chars[i - 1].is_ascii_digit();
        let after = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
        return !(before && after);
    }
    true
}

/// The runs `Punctuation._marks_re` matches: maximal runs of whitespace and marks that
/// hold at least one mark, as `(start, end)` in chars.
fn mark_runs(chars: &[char]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !(chars[i].is_whitespace() || is_mark(chars, i)) {
            i += 1;
            continue;
        }
        let start = i;
        let mut has_mark = false;
        while i < chars.len() && (chars[i].is_whitespace() || is_mark(chars, i)) {
            has_mark |= is_mark(chars, i);
            i += 1;
        }
        if has_mark {
            // Trailing whitespace belongs to the run only up to where the regex's greedy
            // `\s*` stops: at the end of the run. Leading whitespace is taken too.
            runs.push((start, i));
        }
    }
    runs
}

/// `Punctuation._preserve_line`: the line's pieces between marks and the marks.
fn preserve_line(line: &str, num: usize) -> (Vec<String>, Vec<Mark>) {
    let chars: Vec<char> = line.chars().collect();
    let runs = mark_runs(&chars);
    if runs.is_empty() {
        return (vec![line.to_string()], Vec::new());
    }
    let text_of = |(s, e): (usize, usize)| chars[s..e].iter().collect::<String>();
    if runs.len() == 1 && runs[0] == (0, chars.len()) {
        return (
            Vec::new(),
            vec![Mark {
                index: num,
                mark: line.to_string(),
                position: 'A',
            }],
        );
    }
    let mut marks = Vec::with_capacity(runs.len());
    for (k, run) in runs.iter().enumerate() {
        let text = text_of(*run);
        let position = if k == 0 && line.starts_with(&text) {
            'B'
        } else if k == runs.len() - 1 && line.ends_with(&text) {
            'E'
        } else {
            'I'
        };
        marks.push(Mark {
            index: num,
            mark: text,
            position,
        });
    }
    let mut preserved = Vec::with_capacity(marks.len() + 1);
    let mut rest = line.to_string();
    for mark in &marks {
        match rest.split_once(mark.mark.as_str()) {
            Some((prefix, suffix)) => {
                preserved.push(prefix.to_string());
                rest = suffix.to_string();
            }
            None => {
                preserved.push(rest.clone());
                rest = String::new();
            }
        }
    }
    preserved.push(rest);
    (preserved, marks)
}

/// `Punctuation.preserve` over lines: the non-empty pieces and every mark.
fn preserve(lines: &[&str]) -> (Vec<String>, Vec<Mark>) {
    let mut text = Vec::new();
    let mut marks = Vec::new();
    for (num, line) in lines.iter().enumerate() {
        let (pieces, line_marks) = preserve_line(line, num);
        text.extend(pieces.into_iter().filter(|p| !p.is_empty()));
        marks.extend(line_marks);
    }
    (text, marks)
}

/// `EspeakBackend._postprocess_line` with stress kept, no tie, the default separator
/// (words by a space, no phone separator) and `strip=False`: every word ends with a space.
fn postprocess_line(line: &str) -> String {
    let line = line.trim().replace('\n', " ").replace("  ", " ");
    let line = UNDERSCORES.replace_all(&line, "_").replace("_ ", " ");
    if line.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for word in line.split(' ') {
        let mut word = word.trim().to_string();
        word.push('_');
        out.push_str(&word.replace('_', ""));
        out.push(' ');
    }
    out
}

/// `Punctuation.restore` with the default separator and `strip=False`.
fn restore(text: Vec<String>, marks: Vec<Mark>) -> Vec<String> {
    let mut text = text;
    let mut marks = marks;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while !text.is_empty() || !marks.is_empty() {
        if marks.is_empty() {
            for mut line in text.drain(..) {
                if !line.ends_with(' ') {
                    line.push(' ');
                }
                out.push(line);
            }
        } else if text.is_empty() {
            out.push(marks.drain(..).map(|m| m.mark).collect::<String>());
        } else {
            let current = marks[0].clone();
            if current.index == pos {
                marks.remove(0);
                let mark = current.mark;
                if text[0].ends_with(' ') {
                    text[0].pop();
                }
                match current.position {
                    'B' => text[0] = format!("{mark}{}", text[0]),
                    'E' => {
                        let first = text.remove(0);
                        let sep = if mark.ends_with(' ') { "" } else { " " };
                        out.push(format!("{first}{mark}{sep}"));
                        pos += 1;
                    }
                    'A' => {
                        let sep = if mark.ends_with(' ') { "" } else { " " };
                        out.push(format!("{mark}{sep}"));
                        pos += 1;
                    }
                    _ => {
                        if text.len() == 1 {
                            text[0].push_str(&mark);
                        } else {
                            let first = text.remove(0);
                            text[0] = format!("{first}{mark}{}", text[0]);
                        }
                    }
                }
            } else {
                out.push(text.remove(0));
                pos += 1;
            }
        }
    }
    out
}

/// `phonemizer.phonemize(text, language, backend="espeak", preserve_punctuation=True,
/// with_stress=True)` for one string: lines joined by a newline, as it returns them.
pub fn phonemize(espeak: &Espeak, text: &str, language: &str) -> Result<String> {
    let lines: Vec<&str> = text
        .trim_matches('\n')
        .split('\n')
        .map(|l| l.trim_matches('\n'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let (pieces, marks) = preserve(&lines);
    let mut phonemized = Vec::with_capacity(pieces.len());
    for piece in &pieces {
        phonemized.push(postprocess_line(&espeak.text_to_phonemes(piece, language)?));
    }
    Ok(restore(phonemized, marks).join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(line: &str) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        mark_runs(&chars)
            .into_iter()
            .map(|(s, e)| chars[s..e].iter().collect())
            .collect()
    }

    #[test]
    fn marks_are_found_with_their_spaces_and_decimals_are_kept() {
        assert_eq!(
            runs("hello, my world!"),
            vec![", ".to_string(), "!".to_string()]
        );
        assert_eq!(runs("It costs 3.5 dollars."), vec![".".to_string()]);
        assert_eq!(
            runs("Wait... are you sure?"),
            vec!["... ".to_string(), "?".to_string()]
        );
        assert_eq!(runs("no marks here"), Vec::<String>::new());
        assert_eq!(
            runs("$29.99 a month, billed yearly."),
            vec![", ".to_string(), ".".to_string()]
        );
        assert_eq!(
            runs("one, two,three"),
            vec![", ".to_string(), ",".to_string()]
        );
    }

    #[test]
    fn preserve_splits_like_phonemizer() {
        let (pieces, marks) = preserve_line("hello, my world!", 0);
        assert_eq!(pieces, vec!["hello", "my world", ""]);
        assert_eq!(
            marks
                .iter()
                .map(|m| (m.mark.as_str(), m.position))
                .collect::<Vec<_>>(),
            vec![(", ", 'I'), ("!", 'E')]
        );
        // The decimal point is cut at by str.split, the way phonemizer does it.
        let (pieces, marks) = preserve_line("It costs 3.5 dollars.", 0);
        assert_eq!(pieces, vec!["It costs 3", "5 dollars."]);
        assert_eq!(marks[0].position, 'E');
        let (pieces, marks) = preserve_line("...", 0);
        assert!(pieces.is_empty() && marks[0].position == 'A');
        let (pieces, marks) = preserve_line("\"ship it\" and left.", 0);
        assert_eq!(pieces, vec!["", "ship it", "and left", ""]);
        assert_eq!(
            marks.iter().map(|m| m.position).collect::<Vec<_>>(),
            vec!['B', 'I', 'E']
        );
    }

    #[test]
    fn postprocess_drops_phone_separators_and_ends_words_with_a_space() {
        assert_eq!(postprocess_line("h_ə_l_ˈoʊ w_ˈɜː_l_d"), "həlˈoʊ wˈɜːld ");
        assert_eq!(postprocess_line(""), "");
        assert_eq!(postprocess_line(" a__b_ c "), "ab c ");
    }

    #[test]
    fn restore_puts_the_marks_back() {
        let marks = vec![
            Mark {
                index: 0,
                mark: ", ".into(),
                position: 'I',
            },
            Mark {
                index: 0,
                mark: "!".into(),
                position: 'E',
            },
        ];
        let out = restore(vec!["həlˈoʊ ".into(), "maɪ wˈɜːld ".into()], marks);
        assert_eq!(out, vec!["həlˈoʊ, maɪ wˈɜːld! ".to_string()]);
        let out = restore(vec!["a ".into()], vec![]);
        assert_eq!(out, vec!["a ".to_string()]);
        // Nothing phonemized: the marks alone, with no word separator after them.
        let out = restore(
            vec![],
            vec![Mark {
                index: 0,
                mark: "...".into(),
                position: 'A',
            }],
        );
        assert_eq!(out, vec!["...".to_string()]);
    }

    #[test]
    fn phonemizes_with_punctuation_kept() {
        let espeak = Espeak::init(None).expect("libespeak-ng with its data");
        let out = phonemize(espeak, "Hello, my world!", "en-us").unwrap();
        assert!(out.contains(", "), "{out}");
        assert!(out.ends_with("! "), "{out}");
        assert!(!out.contains('_'), "{out}");
        assert_eq!(phonemize(espeak, "", "en-us").unwrap(), "");
    }
}
