//! Ghost's own documentation, in the binary: a curated set, split by heading, searched by
//! counting words. No model and no network; the text is the build's.

use std::sync::LazyLock;

/// A section handed back whole is at most this long; a longer one is cut, and says so.
pub const SECTION_MAX: usize = 8_000;
pub const EXCERPT_MAX: usize = 320;
pub const SEARCH_RESULTS: usize = 5;

/// The documents an agent may read, by the name it asks for them with. The ADRs, the
/// design system, the original product documents and the direction draft are left out:
/// they are how Ghost was decided, not how it works.
const SOURCES: [(&str, &str); 9] = [
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
    pub excerpt: String,
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
            .map(|(_, _, s)| Hit {
                document: s.document.clone(),
                heading: s.heading.clone(),
                excerpt: excerpt(&s.text, &terms),
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

/// The line of a section that holds the most of the words, cut to `EXCERPT_MAX` around
/// the first of them.
fn excerpt(text: &str, terms: &[String]) -> String {
    let best = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let lower = l.to_lowercase();
            let n = terms.iter().filter(|t| lower.contains(t.as_str())).count();
            (n, l)
        })
        .fold(None::<(usize, &str)>, |best, (n, l)| match best {
            Some((b, _)) if b >= n => best,
            _ => Some((n, l)),
        })
        .map(|(_, l)| l.trim())
        .unwrap_or("");
    let chars: Vec<char> = best.chars().collect();
    if chars.len() <= EXCERPT_MAX {
        return best.to_string();
    }
    let lower: Vec<char> = best.to_lowercase().chars().collect();
    let first = terms
        .iter()
        .filter_map(|t| {
            let t: Vec<char> = t.chars().collect();
            lower.windows(t.len()).position(|w| w == t.as_slice())
        })
        .min()
        .unwrap_or(0);
    // A lower-cased character is not always one character; stay inside the line.
    let start = first.saturating_sub(EXCERPT_MAX / 4).min(chars.len());
    let end = (start + EXCERPT_MAX).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
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
        // The glossary is one table: a search still lands on the row that has the word.
        let hits = d.search("wisp mascot", SEARCH_RESULTS);
        assert_eq!(hits[0].document, "glossary");
        assert!(hits[0].excerpt.contains("Wisp"), "{}", hits[0].excerpt);
    }
}
