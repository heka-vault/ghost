//! What a report's words may carry: every string that goes into one passes here first.
//! Deterministic rules, no model (CLAUDE.md, "Deterministic first"):
//!
//! 1. the caller's redaction (the daemon's: every password value the vault holds, ADR 0036);
//! 2. a URL keeps its scheme, host and path: no user, no password, no query, no fragment,
//!    and a host that is an address becomes `[ip]`;
//! 3. an email address becomes `[email]`;
//! 4. the home folder becomes `~` (`$HOME`, and any `/home/<name>`, `/Users/<name>`, `/root`);
//! 5. the user's name and the computer's name, as whole words, become `[user]` and `[host]`;
//! 6. an IPv4 or IPv6 address becomes `[ip]`;
//! 7. a long run of letters and digits that looks like a key or a token becomes `[token]`
//!    (24 characters or more, with both letters and digits), for what the vault does not hold;
//! 8. control characters go, and the result is cut at a fixed length on a character boundary.

use std::sync::{Arc, LazyLock};

use regex::Regex;

/// The caller's own redaction: the daemon's is `ghost_core::redact::redact_text`.
pub type Redact = Arc<dyn Fn(&str) -> String + Send + Sync>;

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b([a-z][a-z0-9+.\-]{1,15})://([^\s"'<>()\[\]{}]+)"#).unwrap()
});
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap());
static HOME_ANY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:/home|/Users|/var/home)/[^/\s:]+|/root\b").unwrap());
static IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap());
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{1,4}\b|\b(?:[0-9a-f]{1,4}:){1,6}:[0-9a-f]{0,4}\b|::1\b")
        .unwrap()
});
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_\-]{24,}").unwrap());

/// The longest string a report keeps in its message.
pub const MESSAGE_CHARS: usize = 200;
/// The longest function or file name in a frame.
pub const FRAME_CHARS: usize = 160;

/// Who this process runs as, read once: the home folder, the user's name, the computer's
/// name. Never sent; only looked for.
#[derive(Clone)]
pub struct Scrubber {
    home: Option<String>,
    user: Option<String>,
    host: Option<String>,
    redact: Option<Redact>,
}

impl std::fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Scrubber")
    }
}

impl Scrubber {
    /// This process's own: `$HOME`, `$USER` (or `$LOGNAME`), the kernel's host name.
    pub fn from_env() -> Self {
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .map(|s| s.trim().to_string());
        Self::new(
            std::env::var("HOME").ok(),
            std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .ok(),
            host,
        )
    }

    pub fn new(home: Option<String>, user: Option<String>, host: Option<String>) -> Self {
        let keep = |s: Option<String>, min: usize| s.filter(|s| s.chars().count() >= min);
        Self {
            home: keep(home, 2).map(|h| h.trim_end_matches('/').to_string()),
            // A one- or two-letter name would turn every word into [user].
            user: keep(user, 3),
            host: keep(host, 3),
            redact: None,
        }
    }

    /// Add the caller's redaction, applied first.
    pub fn with_redact(mut self, redact: Redact) -> Self {
        self.redact = Some(redact);
        self
    }

    /// Every rule, then cut at `max` characters.
    pub fn text(&self, s: &str, max: usize) -> String {
        let mut out: String = match &self.redact {
            Some(r) => r(s),
            None => s.to_string(),
        };
        out = URL
            .replace_all(&out, |c: &regex::Captures| url(&c[1], &c[2]))
            .into_owned();
        out = EMAIL.replace_all(&out, "[email]").into_owned();
        if let Some(home) = &self.home
            && home != "/"
        {
            out = out.replace(home.as_str(), "~");
        }
        out = HOME_ANY.replace_all(&out, "~").into_owned();
        for (word, mark) in [(&self.user, "[user]"), (&self.host, "[host]")] {
            if let Some(w) = word {
                let re = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(w)));
                if let Ok(re) = re {
                    out = re.replace_all(&out, mark).into_owned();
                }
            }
        }
        out = IPV4.replace_all(&out, "[ip]").into_owned();
        out = IPV6.replace_all(&out, "[ip]").into_owned();
        out = TOKEN
            .replace_all(&out, |c: &regex::Captures| {
                let t = &c[0];
                let letters = t.chars().any(|c| c.is_ascii_alphabetic());
                let digits = t.chars().any(|c| c.is_ascii_digit());
                if letters && digits {
                    "[token]".to_string()
                } else {
                    t.to_string()
                }
            })
            .into_owned();
        cut(
            &out.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>(),
            max,
        )
    }
}

/// `scheme://[user[:pass]@]host[:port][/path][?query][#fragment]` → `scheme://host[:port]/path`.
fn url(scheme: &str, rest: &str) -> String {
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority.rsplit('@').next().unwrap_or("");
    let bare = host
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or(host);
    let bare_no_port = bare.split(':').next().unwrap_or(bare);
    let host = if IPV4.is_match(bare_no_port) || bare.contains(':') && !bare.contains('.') {
        "[ip]".to_string()
    } else {
        host.to_string()
    };
    format!("{}://{host}{path}", scheme.to_lowercase())
}

/// At most `max` characters, cut on a character boundary.
pub fn cut(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"`[^`]*`|"[^"]*""#).unwrap());

/// The part of a message that is the code's own words. What follows the first `": "` is
/// usually a value the code formatted in (an error's text, a parser's input), and so is
/// anything between backticks or double quotes (a slice panic names the string it was
/// cutting): both may be anyone's words, so the first is dropped and the second emptied.
/// A quote left open cuts the message there.
pub fn head(s: &str) -> String {
    let first = s.split(": ").next().unwrap_or(s);
    let emptied = QUOTED.replace_all(first, |c: &regex::Captures| {
        let q = &c[0][..1];
        format!("{q}\u{2026}{q}")
    });
    // Every pair is whole now, so a quote that opens and never closes is the last one.
    let mut open: Option<(usize, char)> = None;
    for (i, ch) in emptied.char_indices() {
        match open {
            None if ch == '`' || ch == '"' => open = Some((i, ch)),
            Some((_, q)) if ch == q => open = None,
            _ => {}
        }
    }
    match open {
        Some((i, _)) => emptied[..i].trim_end().to_string(),
        None => emptied.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> Scrubber {
        Scrubber::new(
            Some("/home/alice".into()),
            Some("alice".into()),
            Some("alices-laptop".into()),
        )
    }

    #[test]
    fn a_home_path_becomes_a_tilde() {
        assert_eq!(
            s().text("can't open /home/alice/.config/ghost/apps.json", 200),
            "can't open ~/.config/ghost/apps.json"
        );
        assert_eq!(s().text("/home/bob/x and /root/y", 200), "~/x and ~/y");
    }

    #[test]
    fn a_urls_query_user_and_fragment_go() {
        let out = s().text(
            "GET https://user:hunter2pass@api.example.com/v1/items?token=abc123secret#frag failed",
            200,
        );
        assert_eq!(out, "GET https://api.example.com/v1/items failed");
        assert_eq!(
            s().text("http://192.168.1.20:8080/x?y=1", 200),
            "http://[ip]/x"
        );
    }

    #[test]
    fn emails_names_hosts_and_addresses_go() {
        let out = s().text(
            "alice@example.com on alices-laptop as Alice from 10.0.0.7 and fe80::1:2:3:4",
            200,
        );
        assert!(!out.contains("example.com"), "{out}");
        assert!(!out.to_lowercase().contains("alice"), "{out}");
        assert!(!out.contains("10.0.0.7"), "{out}");
        assert!(!out.contains("fe80"), "{out}");
    }

    #[test]
    fn a_token_shaped_string_goes_but_rust_paths_stay() {
        let out = s().text(
            "key sk-ant-api03-AbCdEf1234567890GhIjKl rejected in ghostd::daemon::Daemon::run",
            200,
        );
        assert!(!out.contains("AbCdEf1234567890"), "{out}");
        assert!(out.contains("ghostd::daemon::Daemon::run"), "{out}");
    }

    #[test]
    fn the_callers_redaction_runs_first() {
        let r = s().with_redact(Arc::new(|t: &str| t.replace("pw-xyz", "[password: K]")));
        assert_eq!(r.text("bad pw-xyz", 200), "bad [password: K]");
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let out = s().text(&"é".repeat(500), 10);
        assert_eq!(out.chars().count(), 10);
    }

    #[test]
    fn head_keeps_what_comes_before_a_value() {
        assert_eq!(
            head("called `Result::unwrap()` on an `Err` value: Api { message: \"secret\" }"),
            "called `\u{2026}` on an `\u{2026}` value"
        );
    }

    #[test]
    fn head_empties_what_the_code_quoted() {
        // A slice panic's location is Ghost's own code and its message names the string.
        assert_eq!(
            head(
                "byte index 3 is not a char boundary; it is inside 'é' (bytes 2..4) of `héllo there, my words`"
            ),
            "byte index 3 is not a char boundary; it is inside 'é' (bytes 2..4) of `\u{2026}`"
        );
        assert_eq!(
            head("no agent named \"Ledger\" here"),
            "no agent named \"\u{2026}\" here"
        );
        // Left open (the runtime cut a long string): nothing after the quote is kept.
        assert_eq!(head("can't read `what was said and never"), "can't read");
    }
}
