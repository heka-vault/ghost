//! An agent's profile, its transcript's turns and its compactions, as the store holds them.
//! The gate's shapes for hire, retire and the rest are `Change` variants in `models`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

use crate::time::utcnow;

/// Agent zero, where every wake lands.
pub const GHOST: &str = "ghost";

pub const MAX_NAME: usize = 80;
pub const MAX_JOB: usize = 200;
pub const MAX_PERSONA: usize = 4000;
pub const MAX_MEMORY: usize = 20000;
pub const MAX_NOTES: usize = 20000;
pub const MAX_NOTE: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Call,
    System,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Call => "call",
            Role::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "call" => Some(Role::Call),
            "system" => Some(Role::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompactionSource {
    #[default]
    Auto,
    Manual,
    Edit,
}

impl CompactionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionSource::Auto => "auto",
            CompactionSource::Manual => "manual",
            CompactionSource::Edit => "edit",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }
}

/// `"Brand design (EMEA)"` -> `"brand-design-emea"`. Fixed at creation (ADR 0014).
pub fn slugify(name: &str) -> String {
    let ascii: String = name.nfkd().filter(char::is_ascii).collect();
    let mut out = String::new();
    let mut dash = false;
    for c in ascii.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(MAX_NAME).collect();
    if cut.is_empty() {
        "agent".to_string()
    } else {
        cut
    }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
    /// What it was hired for, in a line.
    pub job: String,
    /// None: Ghost's voice (ADR 0035).
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
    /// Off the roster and voice routing; files kept.
    pub retired_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl Profile {
    pub fn new(slug: impl Into<String>, name: impl Into<String>) -> Self {
        let now = utcnow();
        Self {
            id: new_id(),
            slug: slug.into(),
            name: name.into(),
            aliases: Vec::new(),
            job: String::new(),
            voice: None,
            speed: None,
            created_at: now,
            updated_at: now,
            last_active_at: None,
            retired_at: None,
            deleted_at: None,
        }
    }

    pub fn is_ghost(&self) -> bool {
        self.slug == GHOST
    }

    pub fn retired(&self) -> bool {
        self.retired_at.is_some()
    }

    pub fn spoken_names(&self) -> Vec<&str> {
        std::iter::once(self.name.as_str())
            .chain(self.aliases.iter().map(String::as_str))
            .collect()
    }

    /// What the folder's `agent.json` carries: identity, never state.
    pub fn agent_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("id".into(), json!(self.id));
        out.insert("name".into(), json!(self.name));
        if !self.aliases.is_empty() {
            out.insert("aliases".into(), json!(self.aliases));
        }
        if !self.job.is_empty() {
            out.insert("job".into(), json!(self.job));
        }
        if let Some(v) = &self.voice {
            out.insert("voice".into(), json!(v));
        }
        if let Some(s) = self.speed {
            out.insert("speed".into(), json!(s));
        }
        Value::Object(out)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnRecord {
    pub id: i64,
    /// The agent's id.
    pub conversation: String,
    pub session_id: String,
    pub role: Role,
    pub text: String,
    pub at: DateTime<Utc>,
    /// Set once a summary covers this turn.
    pub compaction_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    pub id: String,
    /// The agent's id.
    pub conversation: String,
    pub summary: String,
    pub through_turn_id: Option<i64>,
    pub at: DateTime<Utc>,
    pub source: CompactionSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_ascii_lowercase_dashed_and_bounded() {
        assert_eq!(slugify("Brand design (EMEA)"), "brand-design-emea");
        assert_eq!(slugify("  Ledger  "), "ledger");
        assert_eq!(slugify("Café Ünïcode"), "cafe-unicode");
        assert_eq!(slugify("!!!"), "agent");
        assert_eq!(slugify(&"x".repeat(100)).len(), MAX_NAME);
    }

    #[test]
    fn agent_json_carries_identity_only() {
        let mut p = Profile::new("ledger", "Ledger");
        p.job = "the books".into();
        let v = p.agent_json();
        assert_eq!(v["name"], "Ledger");
        assert_eq!(v["job"], "the books");
        assert!(v.get("aliases").is_none());
        assert!(v.get("created_at").is_none());
    }
}
