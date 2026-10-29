//! The files an agent owns, in its folder under Ghost's `agents/` (ADR 0035).
//!
//! `agent.json` is who it is (id, name, aliases, job, its own voice when it has one);
//! `persona.md` is how it speaks; `memory.md` is the current rolling summary;
//! `notes.md` is what the operator asked to be remembered, one line each, verbatim.
//! The operator may open and edit any of them; Ghost writes them only through the
//! gate (`ConversationWriter`). Where a slug's folder is comes from the registry
//! (`locate`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Map, Value};

pub const AGENT: &str = "agent.json";
pub const PERSONA: &str = "persona.md";
pub const MEMORY: &str = "memory.md";
pub const NOTES: &str = "notes.md";

/// Where a slug's folder is; the registry decides.
pub type Locate = Arc<dyn Fn(&str) -> PathBuf + Send + Sync>;

pub struct ConversationFiles {
    locate: Locate,
}

impl ConversationFiles {
    pub fn new(locate: Locate) -> Self {
        Self { locate }
    }

    pub fn dir(&self, slug: &str) -> PathBuf {
        (self.locate)(slug)
    }

    pub fn path(&self, slug: &str, name: &str) -> PathBuf {
        self.dir(slug).join(name)
    }

    /// The file's text, or "" when there is no such file.
    pub fn read(&self, slug: &str, name: &str) -> io::Result<String> {
        let p = self.path(slug, name);
        if p.exists() {
            std::fs::read_to_string(p)
        } else {
            Ok(String::new())
        }
    }

    /// Text with a trailing newline; blank text empties the file if it exists and makes
    /// none otherwise.
    pub fn write(&self, slug: &str, name: &str, text: &str) -> io::Result<()> {
        let d = self.dir(slug);
        std::fs::create_dir_all(&d)?;
        let p = self.path(slug, name);
        if !text.trim().is_empty() {
            if text.ends_with('\n') {
                std::fs::write(p, text)
            } else {
                std::fs::write(p, format!("{text}\n"))
            }
        } else if p.exists() {
            std::fs::write(p, "")
        } else {
            Ok(())
        }
    }

    /// `agent.json`, as Python's `json.dumps(data, indent=2)` wrote it: identity keys
    /// first in their order, ASCII only, a newline at the end.
    pub fn write_agent(&self, slug: &str, data: &Map<String, Value>) -> io::Result<()> {
        let d = self.dir(slug);
        std::fs::create_dir_all(&d)?;
        let text = serde_json::to_string_pretty(&AgentJson(data))?;
        std::fs::write(d.join(AGENT), format!("{}\n", ensure_ascii(&text)))
    }

    pub fn persona(&self, slug: &str) -> io::Result<String> {
        self.read(slug, PERSONA)
    }

    pub fn memory(&self, slug: &str) -> io::Result<String> {
        self.read(slug, MEMORY)
    }

    pub fn notes(&self, slug: &str) -> io::Result<String> {
        self.read(slug, NOTES)
    }

    pub fn note_lines(&self, slug: &str) -> io::Result<Vec<String>> {
        Ok(self
            .notes(slug)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    }

    pub fn ensure(&self, slug: &str) -> io::Result<()> {
        std::fs::create_dir_all(self.dir(slug))
    }
}

/// The identity an agent folder carries, or None when there is none or it is broken.
pub fn read_agent_json(folder: &Path) -> Option<Map<String, Value>> {
    let file = folder.join(AGENT);
    if !file.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(file).ok()?;
    match serde_json::from_str(&text).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The order `Profile::agent_json` built its keys in (Python kept insertion order);
/// anything else follows, sorted.
const AGENT_KEYS: [&str; 6] = ["id", "name", "aliases", "job", "voice", "speed"];

struct AgentJson<'a>(&'a Map<String, Value>);

impl Serialize for AgentJson<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for k in AGENT_KEYS {
            if let Some(v) = self.0.get(k) {
                m.serialize_entry(k, v)?;
            }
        }
        for (k, v) in self.0 {
            if !AGENT_KEYS.contains(&k.as_str()) {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

/// `json.dumps` escapes every non-ASCII character (`ensure_ascii`); so does this, so the
/// bytes on disk do not change with the port.
fn ensure_ascii(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(root: &Path) -> ConversationFiles {
        let root = root.to_path_buf();
        ConversationFiles::new(Arc::new(move |slug| root.join(slug)))
    }

    #[test]
    fn agent_json_is_written_as_python_did() {
        let tmp = tempfile::tempdir().unwrap();
        let f = files(tmp.path());
        let mut data = Map::new();
        data.insert("speed".into(), json!(1.2));
        data.insert("name".into(), json!("Café"));
        data.insert("id".into(), json!("abc"));
        data.insert("aliases".into(), json!(["one", "two"]));
        f.write_agent("cafe", &data).unwrap();
        let text = std::fs::read_to_string(tmp.path().join("cafe").join(AGENT)).unwrap();
        assert_eq!(
            text,
            "{\n  \"id\": \"abc\",\n  \"name\": \"Caf\\u00e9\",\n  \"aliases\": [\n    \"one\",\n    \"two\"\n  ],\n  \"speed\": 1.2\n}\n"
        );
        assert_eq!(read_agent_json(&tmp.path().join("cafe")).unwrap(), data);
        assert!(read_agent_json(&tmp.path().join("nowhere")).is_none());
    }

    #[test]
    fn blank_text_empties_an_existing_file_and_makes_none() {
        let tmp = tempfile::tempdir().unwrap();
        let f = files(tmp.path());
        f.write("a", NOTES, "  ").unwrap();
        assert!(!f.path("a", NOTES).exists());
        f.write("a", NOTES, "one\n two ").unwrap();
        assert_eq!(f.note_lines("a").unwrap(), vec!["one", "two"]);
        f.write("a", NOTES, "").unwrap();
        assert!(f.path("a", NOTES).exists());
        assert_eq!(f.notes("a").unwrap(), "");
    }
}
