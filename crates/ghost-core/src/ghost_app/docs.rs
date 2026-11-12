//! Ghost's own documentation, in the binary: a curated set, split by heading, searched by
//! counting words. No model and no network; the text is the build's.

use std::sync::LazyLock;

/// A section handed back whole is at most this long; a longer one is cut, and says so.
pub const SECTION_MAX: usize = 8_000;
/// An excerpt: whole sentences or list items, this many characters at the most.
pub const EXCERPT_MAX: usize = 400;
/// A section this short comes back whole in a search result, so no second call is needed.
/// Five of them are 6,000 characters, a quarter of an app's result budget.
pub const WHOLE_MAX: usize = 1_200;
pub const SEARCH_RESULTS: usize = 5;

/// The documents an agent may read, by the name it asks for them with. The ADRs, the
/// design system, the original product documents and the direction draft are left out:
/// they are how Ghost was decided, not how it works.
const SOURCES: [(&str, &str); 10] = [
    ("readme", include_str!("../../../../README.md")),
    ("glossary", include_str!("../../../../GLOSSARY.md")),
    (
        "architecture",
        include_str!("../../../../docs/architecture.md"),
    ),
    ("apps", include_str!("../../../../docs/apps.md")),
    (
        "connect-ghost",
        include_str!("../../../../docs/connect-ghost.md"),
    ),
    ("tools", include_str!("../../../../docs/tools.md")),
    ("skills", include_str!("../../../../docs/skills.md")),
    ("folder", include_str!("../../../../docs/folder.md")),
    (
        "performance",
        include_str!("../../../../docs/performance.md"),
    ),
    (
        "phone-protocol",
        include_str!("../../../../docs/phone-protocol.md"),
    ),
];

/// Words too common to tell one section from another.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "how", "does", "what", "with", "that", "this", "from", "can",
    "its", "you", "your", "about", "when", "where", "which", "who", "why", "was", "will", "has",
    "have", "not", "but", "any", "all", "ghost", "into", "out", "do", "is", "it", "to", "of", "in",
    "on", "an", "or", "be", "as", "at", "by", "if", "me", "my", "we", "so", "up",
];

static DOCS: LazyLock<Docs> = LazyLock::new(|| Docs::parse(&SOURCES));

/// A whole document by its name, as it is in the build: what the `ghost://docs/<name>`
/// resource returns (ADR 0048).
pub fn whole(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    let name = name.trim_end_matches(".md");
    SOURCES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// One heading of a document and the text under it, up to the next heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub document: String,
    pub heading: String,
    pub text: String,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub document: String,
    pub heading: String,
    /// The best sentences or items around the words; the whole section when it is short.
    pub excerpt: String,
    /// `excerpt` is the whole section.
    pub whole: bool,
}

/// What `read` hands back: one section, or a document's headings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Outline {
        document: String,
        headings: Vec<String>,
    },
    Section {
        document: String,
        heading: String,
        text: String,
        /// The section was longer than `SECTION_MAX` and this is its start.
        cut: bool,
    },
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DocsError {
    #[error("no document named {name}; there are: {known}")]
    NoDocument { name: String, known: String },
    #[error("{document} has no section {section}; it has: {known}")]
    NoSection {
        document: String,
        section: String,
        known: String,
    },
}

#[derive(Debug, Default)]
pub struct Docs {
    sections: Vec<Section>,
}

impl Docs {
    /// The documents the build carries.
    pub fn built_in() -> &'static Docs {
        &DOCS
    }

    pub fn parse(sources: &[(&str, &str)]) -> Self {
        let mut sections = Vec::new();
        for (name, text) in sources {
            sections.extend(split(name, text));
        }
        Self { sections }
    }

    pub fn documents(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for s in &self.sections {
            if !out.contains(&s.document.as_str()) {
                out.push(&s.document);
            }
        }
        out
    }

    /// The best sections for `query`: each word counts where it is found, more in a
    /// heading, and a section holding every word comes before one holding some.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let terms = terms(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, usize, &Section)> = Vec::new();
        for (at, s) in self.sections.iter().enumerate() {
            let heading = s.heading.to_lowercase();
            let body = s.text.to_lowercase();
            let mut score = 0;
            let mut found = 0;
            for t in &terms {
                let in_heading = heading.matches(t.as_str()).count();
                let in_body = body.matches(t.as_str()).count().min(5);
                let in_name = usize::from(s.document.contains(t.as_str()));
                if in_heading + in_body + in_name > 0 {
                    found += 1;
                }
                score += in_heading * 6 + in_body + in_name * 2;
            }
            if found == 0 {
                continue;
            }
            if found == terms.len() {
                score += 10 * terms.len();
            }
            scored.push((score, at, s));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, s)| {
                let whole = s.text.chars().count() <= WHOLE_MAX;
                Hit {
                    document: s.document.clone(),
                    heading: s.heading.clone(),
                    excerpt: if whole {
                        s.text.clone()
                    } else {
                        excerpt(&s.text, &terms)
                    },
                    whole,
                }
            })
            .collect()
    }

    /// One section by its heading (any case; a heading that contains the words asked for
    /// when none is exactly them), or the document's headings when no section is named.
    pub fn read(&self, name: &str, section: Option<&str>) -> Result<Read, DocsError> {
        let name = name.trim().to_lowercase();
        let name = name.trim_end_matches(".md");
        let of: Vec<&Section> = self
            .sections
            .iter()
            .filter(|s| s.document == name)
            .collect();
        if of.is_empty() {
            return Err(DocsError::NoDocument {
                name: name.to_string(),
                known: self.documents().join(", "),
            });
        }
        let headings = || of.iter().map(|s| s.heading.clone()).collect::<Vec<_>>();
        let Some(wanted) = section.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Read::Outline {
                document: name.to_string(),
                headings: headings(),
            });
        };
        let wanted = wanted.trim_start_matches('#').trim().to_lowercase();
        let found = of
            .iter()
            .find(|s| s.heading.to_lowercase() == wanted)
            .or_else(|| {
                of.iter()
                    .find(|s| s.heading.to_lowercase().contains(&wanted))
            });
        let Some(s) = found else {
            return Err(DocsError::NoSection {
                document: name.to_string(),
                section: wanted,
                known: headings().join("; "),
            });
        };
        let cut = s.text.chars().count() > SECTION_MAX;
        Ok(Read::Section {
            document: s.document.clone(),
            heading: s.heading.clone(),
            text: if cut {
                s.text.chars().take(SECTION_MAX).collect()
            } else {
                s.text.clone()
            },
            cut,
        })
    }
}

/// A document's sections: a `#` line outside a code fence starts one. Text before the
/// first heading belongs to a section named for the document.
fn split(name: &str, text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut heading = name.to_string();
    let mut body = String::new();
    let mut fenced = false;
    let close = |heading: &str, body: &mut String, out: &mut Vec<Section>| {
        let text = body.trim();
        if !text.is_empty() {
            out.push(Section {
                document: name.to_string(),
                heading: heading.to_string(),
                text: text.to_string(),
            });
        }
        body.clear();
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let title = line.trim_start_matches('#');
        if !fenced && line.starts_with('#') && title.starts_with(' ') {
            close(&heading, &mut body, &mut out);
            heading = title.trim().to_string();
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    close(&heading, &mut body, &mut out);
    out
}

/// The query's words, lower-case: letters and digits, two characters at least, without
/// the ones every section has.
fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !STOP_WORDS.contains(w))
        .collect::<Vec<&str>>()
    {
        if !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// One piece of a section an excerpt is made of: a sentence of a paragraph, or a list
/// item, a table row or a line of code whole. `block` tells which paragraph or list it is
/// in: an excerpt never runs from one into another.
struct Piece<'a> {
    text: &'a str,
    block: usize,
    /// Whether this piece and the one after it are sentences of one paragraph (joined by
    /// a space; otherwise by a line break).
    flows: bool,
}

fn is_item(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with('|')
        || t.split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The sentences of a paragraph: one ends after `.`, `?` or `!` (and any closing quote,
/// bracket or emphasis), where a space and then a capital letter, a digit, a quote, a
/// bracket or a backtick follow.
fn sentences(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut from = 0;
    let mut i = 0;
    while i < chars.len() {
        if !matches!(chars[i].1, '.' | '?' | '!') {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && matches!(chars[j].1, ')' | '"' | '\'' | '”' | '’' | '*' | '_') {
            j += 1;
        }
        let starts = chars.get(j + 1).map(|&(_, c)| c).is_some_and(|n| {
            n.is_uppercase() || n.is_ascii_digit() || matches!(n, '`' | '"' | '“' | '(' | '*')
        });
        if j < chars.len() && chars[j].1 == ' ' && starts {
            let end = chars[j].0;
            out.push(text[from..end].trim());
            from = end;
        }
        i = j;
    }
    let rest = text[from..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// A section as the pieces an excerpt is made of, in order.
fn pieces(text: &str) -> Vec<Piece<'_>> {
    fn flush<'a>(
        text: &'a str,
        para: &mut Option<(usize, usize)>,
        out: &mut Vec<Piece<'a>>,
        block: &mut usize,
    ) {
        if let Some((a, b)) = para.take() {
            let all = sentences(&text[a..b]);
            let n = all.len();
            for (k, sentence) in all.into_iter().enumerate() {
                out.push(Piece {
                    text: sentence,
                    block: *block,
                    flows: k + 1 < n,
                });
            }
            *block += 1;
        }
    }
    let mut out: Vec<Piece<'_>> = Vec::new();
    let mut block = 0;
    let mut fenced = false;
    // The paragraph so far, as a byte range of `text`.
    let mut para: Option<(usize, usize)> = None;
    // Whether the last piece was a list item or a line of code, so the next one joins it.
    let mut listing = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at + (line.len() - line.trim_start().len());
        let end = at + line.trim_end().len();
        at += line.len();
        let t = line.trim();
        if t.starts_with("```") {
            flush(text, &mut para, &mut out, &mut block);
            if listing {
                block += 1;
                listing = false;
            }
            fenced = !fenced;
            continue;
        }
        if t.is_empty() {
            flush(text, &mut para, &mut out, &mut block);
            if listing {
                block += 1;
                listing = false;
            }
            continue;
        }
        if fenced || is_item(line) || t.starts_with('>') {
            flush(text, &mut para, &mut out, &mut block);
            out.push(Piece {
                text: t,
                block,
                flows: false,
            });
            listing = true;
            continue;
        }
        if listing {
            // A wrapped list item: its next line, part of the same item.
            if let Some(last) = out.last_mut() {
                let from = last.text.as_ptr() as usize - text.as_ptr() as usize;
                last.text = &text[from..end];
                continue;
            }
        }
        para = Some(match para {
            Some((a, _)) => (a, end),
            None => (start, end),
        });
    }
    flush(text, &mut para, &mut out, &mut block);
    out
}

fn matches_in(text: &str, terms: &[String]) -> usize {
    let lower = text.to_lowercase();
    terms.iter().filter(|t| lower.contains(t.as_str())).count()
}

/// The best-matching whole sentences or list items of a section, `EXCERPT_MAX`
/// characters at the most: the piece that holds the most of the words, then the pieces
/// after it and before it in the same paragraph or list while they fit, so it ends where
/// a sentence or an item does. A piece longer than that on its own (a long table row, a
/// very long sentence) is cut at clauses around the first word, with `…` where it was cut.
fn excerpt(text: &str, terms: &[String]) -> String {
    let all = pieces(text);
    let best = all
        .iter()
        .enumerate()
        .map(|(i, p)| (matches_in(p.text, terms), i))
        .fold(None::<(usize, usize)>, |best, (n, i)| match best {
            Some((b, _)) if b >= n => best,
            _ => Some((n, i)),
        })
        .map(|(_, i)| i);
    let Some(best) = best else {
        return String::new();
    };
    let len = |p: &Piece<'_>| p.text.chars().count() + 1;
    if len(&all[best]) > EXCERPT_MAX {
        return clause_around(all[best].text, terms);
    }
    let block = all[best].block;
    let (mut first, mut last) = (best, best);
    let mut used = len(&all[best]);
    loop {
        if let Some(p) = all
            .get(last + 1)
            .filter(|p| p.block == block && used + len(p) <= EXCERPT_MAX)
        {
            used += len(p);
            last += 1;
            continue;
        }
        if let Some(p) = first
            .checked_sub(1)
            .map(|i| &all[i])
            .filter(|p| p.block == block && used + len(p) <= EXCERPT_MAX)
        {
            used += len(p);
            first -= 1;
            continue;
        }
        break;
    }
    let mut out = String::new();
    for (i, p) in all.iter().enumerate().take(last + 1).skip(first) {
        out.push_str(p.text);
        if i < last {
            out.push(if p.flows { ' ' } else { '\n' });
        }
    }
    out
}

/// A piece too long for an excerpt: the clauses (cut after `, `, `; `, `: ` or ` | `)
/// around the first word it holds, up to `EXCERPT_MAX`, with `…` where it was cut.
fn clause_around(text: &str, terms: &[String]) -> String {
    let lower = text.to_lowercase();
    // Lower-casing can move a byte offset; a word is looked for in the text itself too.
    let hit = terms
        .iter()
        .filter_map(|t| {
            lower
                .find(t.as_str())
                .filter(|&i| text.is_char_boundary(i))
        })
        .min()
        .unwrap_or(0);
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for mark in [", ", "; ", ": ", " | "] {
        cuts.extend(text.match_indices(mark).map(|(i, m)| i + m.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let size = |a: usize, b: usize| text[cuts[a]..cuts[b]].chars().count();
    // The clause the word is in, then whole clauses after and before it while they fit.
    let mut a = cuts.iter().rposition(|&c| c <= hit).unwrap_or(0);
    if a == cuts.len() - 1 {
        a -= 1;
    }
    let mut b = a + 1;
    while b + 1 < cuts.len() && size(a, b + 1) <= EXCERPT_MAX {
        b += 1;
    }
    while a > 0 && size(a - 1, b) <= EXCERPT_MAX {
        a -= 1;
    }
    let mut cut_start = a > 0;
    let mut cut_end = b + 1 < cuts.len();
    let mut body = text[cuts[a]..cuts[b]].trim().to_string();
    if body.chars().count() > EXCERPT_MAX {
        // One clause longer than an excerpt on its own: the words around the hit.
        let at = text[cuts[a]..hit.max(cuts[a])].trim_start().chars().count();
        let chars: Vec<char> = body.chars().collect();
        let mut from = at.saturating_sub(EXCERPT_MAX / 4);
        while from > 0 && from < at && chars[from - 1] != ' ' {
            from += 1; // forward to the start of a word
        }
        let mut to = (from + EXCERPT_MAX - 2).min(chars.len());
        while to < chars.len() && to > at + 1 && chars[to] != ' ' {
            to -= 1; // back to the end of a word
        }
        cut_start |= from > 0;
        cut_end |= to < chars.len();
        body = chars[from..to].iter().collect::<String>().trim().to_string();
    }
    let body = body.trim_end_matches([',', ';', ':', '|', ' ']);
    let mut out = String::new();
    if cut_start {
        out.push('…');
    }
    out.push_str(body);
    if cut_end {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "Intro line.\n\n# Apps\n\nYou connect an app once.\n\n## Permissions\n\n\
        Rules live in policy.toml. A block rule refuses a call.\n\n```toml\n# not a heading\n\
        mode = \"open\"\n```\n\n## Watches\n\nA watch polls a read.\n";

    fn docs() -> Docs {
        Docs::parse(&[
            ("apps", DOC),
            ("folder", "# Folder\n\nWatches are declared here.\n"),
        ])
    }

    #[test]
    fn a_document_splits_at_its_headings_and_not_inside_a_fence() {
        let d = docs();
        let headings: Vec<&str> = d
            .sections
            .iter()
            .filter(|s| s.document == "apps")
            .map(|s| s.heading.as_str())
            .collect();
        assert_eq!(headings, vec!["apps", "Apps", "Permissions", "Watches"]);
        let Read::Section { text, cut, .. } = d.read("apps", Some("permissions")).unwrap() else {
            panic!("a section");
        };
        assert!(text.contains("# not a heading") && text.contains("block rule"));
        assert!(!cut);
    }

    #[test]
    fn search_ranks_a_heading_over_a_mention_and_gives_an_excerpt() {
        let d = docs();
        let hits = d.search("How do watches work?", 5);
        assert_eq!(hits[0].document, "apps");
        assert_eq!(hits[0].heading, "Watches");
        assert_eq!(hits[1].document, "folder");
        assert_eq!(hits[1].excerpt, "Watches are declared here.");
        let hits = d.search("BLOCK rule policy", 5);
        assert_eq!(hits[0].heading, "Permissions");
        assert!(hits[0].excerpt.contains("block rule"));
        assert!(d.search("the and for", 5).is_empty());
        assert!(d.search("zebra", 5).is_empty());
    }

    #[test]
    fn read_gives_an_outline_or_one_section_and_names_what_is_missing() {
        let d = docs();
        assert_eq!(
            d.read("Apps.md", None).unwrap(),
            Read::Outline {
                document: "apps".into(),
                headings: vec![
                    "apps".into(),
                    "Apps".into(),
                    "Permissions".into(),
                    "Watches".into()
                ],
            }
        );
        let e = d.read("nope", None).unwrap_err();
        assert_eq!(
            e.to_string(),
            "no document named nope; there are: apps, folder"
        );
        let e = d.read("folder", Some("zebra")).unwrap_err();
        assert!(e.to_string().contains("it has: Folder"));
    }

    #[test]
    fn a_long_section_is_cut_and_says_so() {
        let long = format!("# Big\n\n{}\n", "word ".repeat(3_000));
        let d = Docs::parse(&[("big", &long)]);
        let Read::Section { text, cut, .. } = d.read("big", Some("Big")).unwrap() else {
            panic!("a section");
        };
        assert!(cut);
        assert_eq!(text.chars().count(), SECTION_MAX);
    }

    #[test]
    fn a_long_line_is_excerpted_around_the_word() {
        let line = format!("{} needle {}", "a".repeat(600), "b".repeat(600));
        let out = excerpt(&line, &["needle".into()]);
        assert!(out.contains("needle"));
        assert!(out.starts_with('…') && out.ends_with('…'));
        assert!(out.chars().count() <= EXCERPT_MAX + 2);
    }

    #[test]
    fn an_excerpt_is_whole_sentences_and_a_short_section_comes_whole() {
        let filler = "This sentence says nothing about the matter at hand at all. ".repeat(20);
        let long = format!(
            "# Turns\n\n{filler}A turn ends after a short grace, e.g. 0.4 s. When the \
             operator keeps talking, the turn reopens. {filler}\n\n- one item\n- the grace \
             item, which is short.\n\n# Short\n\nA short section about grace.\n"
        );
        let d = Docs::parse(&[("voice", &long)]);
        let hits = d.search("grace reopens", 5);
        let turns = hits.iter().find(|h| h.heading == "Turns").unwrap();
        assert!(!turns.whole);
        let e = &turns.excerpt;
        assert!(e.chars().count() <= EXCERPT_MAX, "{e}");
        // The best sentence and its neighbours, from a sentence's start to a sentence's
        // end; "e.g. 0.4 s." is not a sentence end.
        assert!(e.contains("When the operator keeps talking, the turn reopens."), "{e}");
        assert!(e.contains("A turn ends after a short grace, e.g. 0.4 s."), "{e}");
        assert!(e.ends_with('.') && !e.contains('…'), "{e}");
        assert!(
            e.starts_with("This sentence") || e.starts_with("A turn"),
            "{e}"
        );
        // A short section is the result: no second call needed.
        let short = hits.iter().find(|h| h.heading == "Short").unwrap();
        assert!(short.whole);
        assert_eq!(short.excerpt, "A short section about grace.");
        // A list item is an excerpt's unit too, whole and on its own line.
        let list = long.replace("reopens", "goes on");
        let d = Docs::parse(&[("voice", &list)]);
        let e = excerpt(&d.sections[0].text, &["item".into()]);
        assert_eq!(e, "- one item\n- the grace item, which is short.");
    }

    /// What an agent is shown in a search result is the text as written: every line of a
    /// non-whole excerpt is a run of whole pieces of its section.
    fn ends_on_a_boundary(section: &str, excerpt: &str) {
        if excerpt.contains('…') {
            return; // a piece too long on its own, cut at clauses and marked
        }
        for line in excerpt.lines() {
            let at = section
                .find(line)
                .unwrap_or_else(|| panic!("{line:?} is not in the section"));
            let after = section[at + line.len()..].chars().next();
            assert!(
                after.is_none_or(char::is_whitespace),
                "{line:?} stops mid-sentence"
            );
        }
    }

    #[test]
    fn the_build_carries_the_curated_documents() {
        let d = Docs::built_in();
        assert_eq!(
            d.documents(),
            vec![
                "readme",
                "glossary",
                "architecture",
                "apps",
                "connect-ghost",
                "tools",
                "skills",
                "folder",
                "performance",
                "phone-protocol"
            ]
        );
        // What an agent is most likely to be asked: each finds its own page.
        let hits = d.search("permissions block ask quiet", SEARCH_RESULTS);
        assert!(hits.iter().any(|h| h.document == "apps"), "{hits:?}");
        let hits = d.search("waiting list", SEARCH_RESULTS);
        assert!(!hits.is_empty());
        assert!(matches!(
            d.read("glossary", Some("Glossary")),
            Ok(Read::Section { .. })
        ));
        // What the operator asks an agent, and where it lands (ADR 0052).
        let top = |q: &str| d.search(q, 3);
        let hits = top("can you check the build every morning?");
        assert!(
            hits.iter()
                .any(|h| h.heading.to_lowercase().contains("schedule")),
            "{hits:?}"
        );
        let hits = top("how do I connect Claude Code?");
        assert_eq!(hits[0].document, "connect-ghost", "{hits:?}");
        let hits = top("why did you ask me before closing that issue?");
        assert!(
            hits.iter()
                .any(|h| h.heading.contains("asked")
                    || h.heading.to_lowercase().contains("permission")),
            "{hits:?}"
        );
        let hits = top("what tools do agents have to remember what was said");
        assert!(hits.iter().any(|h| h.document == "tools"), "{hits:?}");
        // The glossary is one table: a search still lands on the row that has the word.
        let hits = d.search("wisp mascot", SEARCH_RESULTS);
        assert_eq!(hits[0].document, "glossary");
        assert!(hits[0].excerpt.contains("Wisp"), "{}", hits[0].excerpt);
        // Every excerpt of the documented examples ends where a sentence or an item does,
        // and the results stay well inside an app's result budget.
        for q in [
            "can you check the build every morning?",
            "how do I connect Claude Code?",
            "why did you ask me before closing that issue?",
            "what tools do agents have to remember what was said",
            "permissions block ask quiet",
            "waiting list",
        ] {
            let hits = d.search(q, SEARCH_RESULTS);
            let mut size = 0;
            for h in &hits {
                let section = d
                    .sections
                    .iter()
                    .find(|s| s.document == h.document && s.heading == h.heading)
                    .unwrap();
                if h.whole {
                    assert!(h.excerpt.chars().count() <= WHOLE_MAX);
                } else {
                    assert!(h.excerpt.chars().count() <= EXCERPT_MAX, "{q}: {h:?}");
                    ends_on_a_boundary(&section.text, &h.excerpt);
                }
                size += h.excerpt.len();
            }
            assert!(size <= SEARCH_RESULTS * WHOLE_MAX * 2, "{q}: {size}");
        }
    }
}
