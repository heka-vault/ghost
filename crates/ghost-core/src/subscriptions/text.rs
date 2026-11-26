//! What another program prints, made safe to show (ADR 0078): escape and control sequences
//! taken out, lines cut and bounded, a sign-in address found only on its vendor's own
//! domains, a waiting prompt for a pasted code noticed, and a pasted code never shown again
//! when the program echoes it. Pure, so it is tested without a program.
//!
//! Everything here is someone else's text: it is shown as data on the operator's desktop and
//! never logged, stored, audited or given to a model.

use std::collections::VecDeque;

use zeroize::Zeroizing;

/// The most lines a job keeps.
pub const LINES_MAX: usize = 200;
/// The most characters of one line shown.
pub const LINE_MAX: usize = 300;
/// The longest address taken as a sign-in link.
pub const URL_MAX: usize = 4096;
/// The longest line held while it is still being written.
const PENDING_MAX: usize = 8192;
/// What stands in for a pasted code the program echoed.
pub const HIDDEN: &str = "••••••";

/// A byte stream from a program, as lines a person can read.
#[derive(Default)]
pub struct Screen {
    state: Parse,
    /// The bytes of the line being written, escapes already out.
    line: Vec<u8>,
    /// An OSC sequence's bytes while it is read (a hyperlink's target among them).
    osc: Vec<u8>,
    lines: VecDeque<String>,
    /// The code the operator pasted, held only to hide its echo.
    code: Option<Zeroizing<String>>,
    /// Lines cut because there were too many.
    pub dropped: usize,
    /// The domains a sign-in address may be on.
    domains: &'static [&'static str],
    /// The sign-in address, whole, once one is printed (or linked) on those domains.
    pub url: Option<String>,
    /// A code the program shows for the operator to type elsewhere.
    pub device_code: Option<String>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Parse {
    #[default]
    Text,
    /// After ESC.
    Escape,
    /// In `ESC [ …` until its final byte.
    Csi,
    /// In `ESC ] …` until BEL or ESC \.
    Osc,
    /// In OSC, after an ESC (expecting `\`).
    OscEnd,
    /// In `ESC P|X|^|_ …` until ESC \.
    Str,
    StrEnd,
    /// After `ESC (`, `ESC )` and the like: one more byte.
    Charset,
}

impl Screen {
    /// A screen whose sign-in address may be on `domains` only.
    pub fn new(domains: &'static [&'static str]) -> Self {
        Self {
            domains,
            ..Self::default()
        }
    }

    /// Hide `code` wherever the program echoes it from now on.
    pub fn hide(&mut self, code: Zeroizing<String>) {
        self.code = Some(code);
    }

    /// Forget the pasted code (the program ended).
    pub fn forget_code(&mut self) {
        self.code = None;
    }

    /// Take in what the program wrote.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            Parse::Text => match b {
                0x1b => self.state = Parse::Escape,
                b'\n' | b'\r' => self.end_line(),
                b'\t' => self.push(b' '),
                0x9b => self.state = Parse::Csi,
                0x00..=0x1f | 0x7f => {} // other controls: a bell, a backspace
                _ => self.push(b),
            },
            Parse::Escape => {
                self.state = match b {
                    b'[' => Parse::Csi,
                    b']' => {
                        self.osc.clear();
                        Parse::Osc
                    }
                    b'P' | b'X' | b'^' | b'_' => Parse::Str,
                    b'(' | b')' | b'*' | b'+' | b'#' | b'%' => Parse::Charset,
                    _ => Parse::Text,
                }
            }
            Parse::Charset => self.state = Parse::Text,
            Parse::Csi => {
                if (0x40..=0x7e).contains(&b) {
                    self.state = Parse::Text;
                }
            }
            Parse::Osc => match b {
                0x07 => self.end_osc(),
                0x1b => self.state = Parse::OscEnd,
                _ => {
                    if self.osc.len() < URL_MAX + 16 {
                        self.osc.push(b);
                    }
                }
            },
            Parse::OscEnd => {
                // ESC \ ends it; anything else ends it too, and starts over.
                self.end_osc();
                if b != b'\\' {
                    self.byte(0x1b);
                    self.byte(b);
                }
            }
            Parse::Str => {
                if b == 0x1b {
                    self.state = Parse::StrEnd;
                }
            }
            Parse::StrEnd => self.state = Parse::Text,
        }
    }

    fn end_osc(&mut self) {
        self.state = Parse::Text;
        let osc = String::from_utf8_lossy(&self.osc).into_owned();
        self.osc.clear();
        // `8;params;target`: a hyperlink. Its text is printed too; the target is kept apart.
        if let Some(rest) = osc.strip_prefix("8;")
            && let Some((_, target)) = rest.split_once(';')
            && !target.is_empty()
            && self.url.is_none()
        {
            self.url = sign_in_url(target, self.domains);
        }
    }

    fn push(&mut self, b: u8) {
        if self.line.len() < PENDING_MAX {
            self.line.push(b);
        }
    }

    fn end_line(&mut self) {
        let raw = String::from_utf8_lossy(&self.line).into_owned();
        self.line.clear();
        self.notice(&raw);
        let shown = self.shown(&raw);
        if shown.is_empty() || self.lines.back() == Some(&shown) {
            return; // a blank line, or the same line drawn again
        }
        if self.lines.len() == LINES_MAX {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(shown);
    }

    /// The line as shown: the code hidden, an address cut to where it goes, bounded.
    fn shown(&self, raw: &str) -> String {
        let mut s = raw.trim().to_string();
        if let Some(code) = &self.code {
            s = hide_code(&s, code);
        }
        s = shorten_urls(&s);
        let cut: String = s.chars().take(LINE_MAX).collect();
        let cut = if s.chars().count() > LINE_MAX {
            format!("{cut}…")
        } else {
            cut
        };
        crate::redact::redact_text(&cut)
    }

    /// An address or a device code in a line the program wrote (never after a code was
    /// pasted: what follows it is the program's answer, not a code to show).
    fn notice(&mut self, raw: &str) {
        if self.url.is_none() {
            self.url = sign_in_url(raw, self.domains);
        }
        if self.device_code.is_none() && self.code.is_none() {
            self.device_code = device_code(raw);
        }
    }

    /// Take in the line still being written, for an address printed with no line's end yet.
    pub fn notice_pending(&mut self) {
        let raw = String::from_utf8_lossy(&self.line).into_owned();
        self.notice(&raw);
    }

    /// Every line so far, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    /// The line being written, not yet ended (a prompt waits there), as shown.
    pub fn pending(&self) -> String {
        let raw = String::from_utf8_lossy(&self.line).into_owned();
        self.shown(&raw)
    }
}

/// `text` with every run of the code, or of a long part of it, shown as [`HIDDEN`]. A program
/// echoes what was typed at its prompt; a person reads the screen, so the echo is never shown.
pub fn hide_code(text: &str, code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        return text.to_string();
    }
    let mut out = text.replace(code, HIDDEN);
    // An echo drawn in pieces, or cut by the line's end: any word of six characters or more
    // that is part of the code, or holds it.
    let words: Vec<String> = out.split_whitespace().map(str::to_string).collect();
    for w in words {
        let bare = w.trim_matches(|c: char| c == '>' || c == ':' || c == '"' || c == '\'');
        if bare.chars().count() >= 6 && (code.contains(bare) || bare.contains(code)) {
            out = out.replace(bare, HIDDEN);
        }
    }
    out
}

/// Every `https://` address in `text`, as it was written.
fn addresses(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c.is_control() || "\"'<>`".contains(c))
            .unwrap_or(tail.len());
        let found = tail[..end].trim_end_matches(['.', ',', ';', ')', ']', '}']);
        if found.len() > "https://".len() {
            out.push(found);
        }
        rest = &tail[end.max(1)..];
    }
    out
}

/// Whether `host` is one of `domains` or under one of them.
pub fn on_domains(host: &str, domains: &[&str]) -> bool {
    let host = host.to_ascii_lowercase();
    domains
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// The first sign-in address in `text` that is https, on one of `domains`, with no user or
/// password in it and no port but https's own. Kept whole for the link (its query is the
/// sign-in's), and never logged.
pub fn sign_in_url(text: &str, domains: &[&str]) -> Option<String> {
    for a in addresses(text) {
        if a.len() > URL_MAX {
            continue;
        }
        let Ok(u) = url::Url::parse(a) else { continue };
        if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
            continue;
        }
        if u.port().is_some_and(|p| p != 443) {
            continue;
        }
        let Some(url::Host::Domain(host)) = u.host() else {
            continue;
        };
        if on_domains(host, domains) {
            return Some(u.to_string());
        }
    }
    None
}

/// Each address in `line` cut to its scheme, host and path with `…`: the screen says where it
/// goes; the whole address (its query is the sign-in's) is only the job's link.
pub fn shorten_urls(line: &str) -> String {
    let mut out = line.to_string();
    for a in addresses(line) {
        let short = match url::Url::parse(a) {
            Ok(u) if u.query().is_some() || u.fragment().is_some() => {
                format!(
                    "{}://{}{}…",
                    u.scheme(),
                    u.host_str().unwrap_or(""),
                    u.path()
                )
            }
            Ok(_) => continue,
            Err(_) => "https://…".to_string(),
        };
        out = out.replace(a, &short);
    }
    out
}

/// Whether the program waits for a code to be pasted: a prompt, left open, that asks for a
/// code (`Paste code here if prompted >`, `Enter the code:`).
pub fn asks_for_code(prompt: &str) -> bool {
    let p = prompt.trim().to_lowercase();
    if p.is_empty() || p.chars().count() > 160 || !p.contains("code") {
        return false;
    }
    p.contains("paste") || p.ends_with(':') || p.ends_with('>') || p.ends_with('?')
}

/// A code the program shows for the operator to type on another device (`ABCD-EFGH`), from a
/// line that talks about a code; None when there is none.
pub fn device_code(line: &str) -> Option<String> {
    if !line.to_lowercase().contains("code") {
        return None;
    }
    line.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|w| {
            let parts: Vec<&str> = w.split('-').collect();
            parts.len() == 2
                && parts.iter().all(|p| {
                    (4..=6).contains(&p.len())
                        && p.chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                })
        })
        .map(str::to_string)
}

/// A pasted code Ghost will type: one line of printable characters, 512 at most. Its words
/// are never said back: a refusal names the rule only.
pub fn check_code(code: &str) -> Result<(), &'static str> {
    let c = code.trim();
    if c.is_empty() {
        return Err("The code is empty.");
    }
    if c.chars().count() > 512 {
        return Err("That is longer than a sign-in code.");
    }
    if c.chars().any(char::is_control) {
        return Err("A code is one line, with no control characters.");
    }
    Ok(())
}
