//! What can be typed (ADR 0077 as amended 2026-11-25). GNOME's RemoteDesktop portal types a
//! keysym only when the keyboard layout has a key for it and drops any other silently, with
//! no error (measured on GNOME 50.1: é, ü, €, Cyrillic, CJK, an emoji and an em dash all
//! vanished). Ghost's dictation is English only (the operator's choice of the same day, and
//! Parakeet decodes English tokens only), so before a key is sent: typographic punctuation
//! becomes its plain form, an accented letter its plain letter (café becomes cafe), and
//! anything else outside printable ASCII is left out. The clipboard is the way only when
//! nothing typable is left.

use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use super::cleanup::sanitize;

/// How the text reaches the window.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Type it, made English: every character has a key.
    Type(Zeroizing<String>),
    /// Nothing typable is left: the clipboard, and "Press Ctrl+V".
    Clipboard,
}

/// The plain form of typographic punctuation a model or Parakeet may write. One character
/// may become several ("…" is three dots).
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => "'",
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => "\"",
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => "-",
        '\u{2026}' => "...",
        // A fraction's slash (½ decomposes to 1, it, 2): never "12".
        '\u{2044}' | '\u{2215}' => "/",
        // Latin letters with no decomposition into a plain one.
        '\u{df}' => "ss",
        '\u{c6}' => "AE",
        '\u{e6}' => "ae",
        '\u{d8}' => "O",
        '\u{f8}' => "o",
        '\u{152}' => "OE",
        '\u{153}' => "oe",
        '\u{141}' => "L",
        '\u{142}' => "l",
        '\u{110}' => "D",
        '\u{111}' => "d",
        '\u{de}' => "Th",
        '\u{fe}' => "th",
        '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{2009}' | '\u{200A}' => " ",
        _ => return None,
    })
}

/// Sanitized (no control character, no line break: nothing typed can submit or run
/// anything), typographic punctuation made plain, accents taken off (NFKD, the marks left
/// out), anything else outside printable ASCII left out.
pub fn plan(text: &str) -> Plan {
    let sane = sanitize(text);
    let mut out = Zeroizing::new(String::with_capacity(sane.len() + 8));
    for c in sane.chars() {
        if let Some(p) = plain(c) {
            out.push_str(p);
        } else if c == ' ' || c.is_ascii_graphic() {
            out.push(c);
        } else {
            // é is e and a mark, ß is ss, ﬁ is fi; Cyrillic, CJK and an emoji have no ASCII.
            for d in c.nfkd() {
                if let Some(p) = plain(d) {
                    out.push_str(p);
                } else if d == ' ' || d.is_ascii_graphic() {
                    out.push(d);
                }
            }
        }
    }
    // A plain space may have doubled a space; sanitize again collapses it.
    let out = sanitize(&out);
    if out.is_empty() {
        Plan::Clipboard
    } else {
        Plan::Type(out)
    }
}

/// The keysym for a printable ASCII character: X11 keysyms equal their Latin-1 code point.
pub fn keysym(c: char) -> Option<i32> {
    (c == ' ' || c.is_ascii_graphic()).then_some(c as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(t: &str) -> String {
        match plan(t) {
            Plan::Type(s) => s.to_string(),
            Plan::Clipboard => panic!("went to the clipboard: {t:?}"),
        }
    }

    #[test]
    fn plain_text_is_typed_as_it_is() {
        assert_eq!(
            typed("Hello, world! Testing 1, 2, 3. (Quotes \"double\" and 'single'.)"),
            "Hello, world! Testing 1, 2, 3. (Quotes \"double\" and 'single'.)"
        );
    }

    #[test]
    fn typographic_punctuation_becomes_plain() {
        assert_eq!(
            typed("It\u{2019}s \u{201C}fine\u{201D} \u{2014} really\u{2026} 3\u{2013}4"),
            "It's \"fine\" - really... 3-4"
        );
        assert_eq!(typed("a\u{00A0}b  \u{202F}c"), "a b c");
    }

    #[test]
    fn nothing_typed_can_end_a_line() {
        assert_eq!(typed("rm -rf /\nls\r\n\tdone"), "rm -rf / ls done");
        assert_eq!(typed("a\u{1b}[31mb\u{0}c"), "a[31mbc");
    }

    #[test]
    fn english_only_accents_come_off_and_other_scripts_are_left_out() {
        // Each of these vanished when typed in the 2026-11-25 spike; now the text is typed.
        assert_eq!(typed("Caf\u{e9} au lait"), "Cafe au lait");
        assert_eq!(
            typed("Z\u{fc}rich, \u{c6}r\u{f8}sk\u{f8}bing"),
            "Zurich, AEroskobing"
        );
        assert_eq!(typed("stra\u{df}e and \u{fb01}le"), "strasse and file");
        assert_eq!(typed("costs 5 \u{20ac}"), "costs 5");
        assert_eq!(
            typed("Tokyo \u{6771}\u{4eac} and nice \u{1f642}"),
            "Tokyo and nice"
        );
        // Parakeet's misreading in an English dictation: the stray letters go, the rest stays.
        assert_eq!(typed("on \u{41a}\u{43e}\u{433} main"), "on main");
        assert_eq!(typed("\u{bd} cup"), "1/2 cup");
    }

    #[test]
    fn only_nothing_typable_goes_to_the_clipboard() {
        assert_eq!(plan("\u{6771}\u{4eac}"), Plan::Clipboard);
        assert_eq!(plan("\u{1f642}"), Plan::Clipboard);
    }

    #[test]
    fn empty_text_is_not_typed() {
        assert_eq!(plan(""), Plan::Clipboard);
        assert_eq!(plan(" \n\t "), Plan::Clipboard);
    }

    #[test]
    fn keysyms_are_the_ascii_code() {
        assert_eq!(keysym('A'), Some(0x41));
        assert_eq!(keysym(' '), Some(0x20));
        assert_eq!(keysym('~'), Some(0x7e));
        assert_eq!(keysym('\n'), None);
        assert_eq!(keysym('\u{e9}'), None);
    }
}
