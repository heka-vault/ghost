//! What can be typed (ADR 0077 as amended 2026-11-25). GNOME's RemoteDesktop portal types a
//! keysym only when the keyboard layout has a key for it and drops any other silently, with
//! no error (measured on GNOME 50.1: é, ü, €, Cyrillic, CJK, an emoji and an em dash all
//! vanished). So the text is checked before a single key is sent: typographic punctuation
//! becomes its plain form, and if anything outside printable ASCII is still left, nothing is
//! typed and the whole text goes to the clipboard. Typing half a sentence with holes is never
//! right.

use zeroize::Zeroizing;

use super::cleanup::sanitize;

/// How the text reaches the window.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Every character has a key: type it.
    Type(Zeroizing<String>),
    /// Something has no key: the clipboard, and "Press Ctrl+V".
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
        '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{2009}' | '\u{200A}' => " ",
        _ => return None,
    })
}

/// Sanitized (no control character, no line break: nothing typed can submit or run
/// anything), typographic punctuation made plain, then checked: printable ASCII only.
pub fn plan(text: &str) -> Plan {
    let sane = sanitize(text);
    let mut out = Zeroizing::new(String::with_capacity(sane.len() + 8));
    for c in sane.chars() {
        if let Some(p) = plain(c) {
            out.push_str(p);
        } else if c == ' ' || c.is_ascii_graphic() {
            out.push(c);
        } else {
            return Plan::Clipboard;
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
    fn a_character_without_a_key_sends_the_whole_text_to_the_clipboard() {
        // Each of these vanished when typed in the 2026-11-25 spike.
        for t in [
            "Caf\u{e9} au lait",
            "Z\u{fc}rich",
            "costs 5 \u{20ac}",
            "Tokyo \u{6771}\u{4eac}",
            "Moscow \u{41c}\u{43e}\u{441}\u{43a}\u{432}\u{430}",
            "nice \u{1f642}",
            "\u{bd} cup",
        ] {
            assert_eq!(plan(t), Plan::Clipboard, "{t:?}");
        }
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
