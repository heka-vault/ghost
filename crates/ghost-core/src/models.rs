//! The write model every source shares (ADR 0007).
//!
//! A source defines what it writes to (a `Target`), what that looked like (a `Snapshot`)
//! and the bounded steps it takes (`Change`s), and a writer applies them: an app's call,
//! an agent's files, Ghost's rules, an output. The gate records, applies and audits
//! through these shapes alone, so a call to an app and a local file get the same path.
//! The sources are all in-tree (ADR 0034), so the shapes are closed sums: a step a writer
//! does not know is a compile error, not a runtime one.
//!
//! There is no undo machinery (ADR 0015). Reversing a change is an ordinary change the
//! model proposes, through the same gate and permissions.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::conversations::models::CompactionSource;
use crate::request::{number_mentions, words};
use crate::time::utcnow;

pub const PROPOSAL_TTL_SECS: i64 = 60;
/// ADR 0030: a change at `confirm` waits this long on the waiting list unless the policy
/// file says otherwise (`[defaults] pending_ttl`).
pub const PENDING_TTL_SECS: i64 = 3600;
pub const MAX_STEPS: usize = 4;
/// A string in an app's result, so one issue body never fills the context.
pub const MAX_RESULT_TEXT: usize = 1200;
/// Ghost's own skills folder: what add writes and remove deletes.
pub const SKILLS_GHOST_SOURCE: &str = "ghost";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("a change set has between 1 and {MAX_STEPS} steps, not {0}")]
    Steps(usize),
}

// --- targets ------------------------------------------------------------------------

/// What a change to an app is about: the app, the tool, and the arguments short enough
/// to name it ("ghost-mock", "app", 17), which a request names it by and a rule matches on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppTarget {
    pub app: String,
    pub tool: String,
    #[serde(default)]
    pub fields: Map<String, Value>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTarget {
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTarget {
    pub name: String,
}

/// Ghost's AGENTS.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RulesTarget {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillTarget {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTarget {
    pub path: PathBuf,
}

/// A stored object whose type is gone (a retired ability, ADR 0022; a step the removed
/// undo feature wrote). The audit log keeps reading it by what the row holds; nothing can
/// write to it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retired {
    pub type_name: String,
    pub data: Map<String, Value>,
}

/// An issue-like target for the gate's tests, independent of any source.
#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeIssue {
    pub owner: String,
    pub repo: String,
    pub number: i64,
}

#[cfg(any(test, feature = "fakes"))]
impl FakeIssue {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Something a source can write to.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    App(AppTarget),
    Conversation(ConversationTarget),
    Output(OutputTarget),
    Rules(RulesTarget),
    Skill(SkillTarget),
    File(FileTarget),
    Retired(Retired),
    #[cfg(any(test, feature = "fakes"))]
    FakeIssue(FakeIssue),
}

static WHERE_WORDS: &[&str] = &[
    "owner",
    "org",
    "organization",
    "organisation",
    "repo",
    "repository",
    "branch",
    "ref",
    "base",
    "head",
    "workspace",
    "team",
    "account",
    "space",
    "site",
    "tenant",
    "region",
];
static ID_WORDS: &[&str] = &["name", "id", "slug", "login", "full"];
static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").unwrap());
static LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z]+").unwrap());
static ALNUM3: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9]{3,}").unwrap());
static SKILL_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._-]+").unwrap());

/// `owner`, `repo_name`, `base_branch`, `teamId`: every word of the argument's name,
/// past `name` and `id`, is a place.
pub fn says_where(key: &str) -> bool {
    let snake = CAMEL.replace_all(key, "${1}_${2}").to_lowercase();
    let parts: Vec<&str> = LETTERS
        .find_iter(&snake)
        .map(|m| m.as_str())
        .filter(|p| !ID_WORDS.contains(p))
        .collect();
    !parts.is_empty() && parts.iter().all(|p| WHERE_WORDS.contains(p))
}

/// Python's `str()` of a JSON value, as the audit and the spoken forms had it.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Python's `repr()` of a string, for the describe lines that used `!r`.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `12345` -> `12,345`, Python's `{:,}`.
pub fn commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

impl Target {
    /// Stable identity, e.g. `app:github:issue_write:acme/app/17` or `agent:ledger`.
    pub fn key(&self) -> String {
        match self {
            Target::App(t) => {
                let tail: Vec<String> = t.fields.values().map(py_str).collect();
                let base = format!("app:{}:{}", t.app, t.tool);
                if tail.is_empty() {
                    base
                } else {
                    format!("{base}:{}", tail.join("/"))
                }
            }
            Target::Conversation(t) => format!("agent:{}", t.slug),
            Target::Output(t) => format!("output:{}", t.name),
            Target::Rules(_) => "rules:ghost".into(),
            Target::Skill(t) => format!("skill:{}", t.name),
            Target::File(t) => format!("file:{}", t.path.display()),
            Target::Retired(r) => {
                let d = &r.data;
                if let (Some(o), Some(re), Some(n)) =
                    (d.get("owner"), d.get("repo"), d.get("number"))
                {
                    format!("retired:{}/{}#{}", py_str(o), py_str(re), py_str(n))
                } else {
                    let vals: Vec<String> = d.values().map(py_str).collect();
                    format!("retired:{}:{}", r.type_name, vals.join("/"))
                }
            }
            #[cfg(any(test, feature = "fakes"))]
            Target::FakeIssue(t) => format!("fake:{}#{}", t.full_name(), t.number),
        }
    }

    /// What a permission's target rule matches against: the repository, the directory.
    pub fn scope(&self) -> String {
        match self {
            Target::App(_) | Target::Rules(_) => match self {
                Target::Rules(_) => "ghost".into(),
                _ => self.key(),
            },
            Target::Conversation(t) => t.slug.clone(),
            Target::Output(t) => t.name.clone(),
            Target::Skill(_) => SKILLS_GHOST_SOURCE.into(),
            Target::File(t) => t
                .path
                .parent()
                .map(|p| {
                    if p.as_os_str().is_empty() {
                        ".".to_string()
                    } else {
                        p.display().to_string()
                    }
                })
                .unwrap_or_else(|| ".".into()),
            Target::Retired(r) => match (r.data.get("owner"), r.data.get("repo")) {
                (Some(o), Some(re)) => format!("{}/{}", py_str(o), py_str(re)),
                _ => self.key(),
            },
            #[cfg(any(test, feature = "fakes"))]
            Target::FakeIssue(t) => t.full_name(),
        }
    }

    /// How Ghost says it aloud.
    pub fn spoken(&self) -> String {
        match self {
            Target::App(t) => {
                let named: Vec<String> = t.fields.values().map(py_str).collect();
                let head = if t.title.is_empty() { &t.app } else { &t.title };
                format!("{head} {}", named.join(" ")).trim().to_string()
            }
            Target::Conversation(t) => t.slug.replace('-', " "),
            Target::Output(t) => format!("{} in Ghost's outputs", t.name),
            Target::Rules(_) => "Ghost's rules".into(),
            Target::Skill(t) => format!("the {} skill", t.name),
            Target::File(t) => t
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Target::Retired(_) => self.key().trim_start_matches("retired:").to_string(),
            #[cfg(any(test, feature = "fakes"))]
            Target::FakeIssue(t) => format!("{}#{}", t.full_name(), t.number),
        }
    }

    /// Words an operator would use to name it in a request ("17", "seventeen"). Empty
    /// means a request can't name it, so a confirm-level change on it always asks.
    pub fn mentions(&self) -> BTreeSet<String> {
        match self {
            Target::App(t) => {
                // The words that name what the change is about (ADR 0017): the issue's
                // number, the file's path, the label. Where it happens (the owner, the
                // repository, the branch) never counts on its own.
                let mut out = BTreeSet::new();
                for (k, v) in &t.fields {
                    if v.is_boolean() || says_where(k) {
                        continue;
                    }
                    let number = match v {
                        Value::Number(n) => n.as_i64(),
                        Value::String(s)
                            if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) =>
                        {
                            s.parse().ok()
                        }
                        _ => None,
                    };
                    if let Some(n) = number {
                        out.extend(number_mentions(n));
                    } else if let Value::String(s) = v {
                        let said = words(s).join(" ");
                        if !said.is_empty() {
                            out.insert(said); // as the transcript has it: "README.md" is "readme md"
                        }
                    }
                }
                out
            }
            Target::Conversation(t) => {
                let mut out: BTreeSet<String> = t
                    .slug
                    .split('-')
                    .filter(|w| w.len() > 2)
                    .map(String::from)
                    .collect();
                out.insert(self.spoken());
                out
            }
            Target::Output(t) => {
                let lower = t.name.to_lowercase();
                let stem = lower
                    .rsplit_once('.')
                    .map_or(lower.clone(), |(s, _)| s.to_string());
                let mut out = BTreeSet::from([lower.clone(), stem.clone()]);
                out.extend(ALNUM3.find_iter(&stem).map(|m| m.as_str().to_string()));
                out
            }
            Target::Rules(_) => BTreeSet::from(["rule".to_string(), "rules".to_string()]),
            Target::Skill(t) => {
                let mut out: BTreeSet<String> = SKILL_SEP
                    .split(&t.name)
                    .filter(|w| w.len() > 2)
                    .map(String::from)
                    .collect();
                out.insert(t.name.clone());
                out
            }
            Target::File(_) | Target::Retired(_) => BTreeSet::new(),
            #[cfg(any(test, feature = "fakes"))]
            Target::FakeIssue(t) => number_mentions(t.number),
        }
    }

    /// What a permission rule's argument patterns match (ADR 0034): an app's call
    /// arguments, or `target` for Ghost's own operations.
    pub fn rule_args(&self) -> Map<String, Value> {
        match self {
            Target::App(t) => t.fields.clone(),
            _ => Map::from_iter([("target".to_string(), Value::String(self.scope()))]),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.spoken())
    }
}

// --- snapshots ----------------------------------------------------------------------

/// A change to an app is not read before or after (ADR 0034). After the call, the
/// read-back is what the app answered (trimmed, untrusted like any result); `checked`
/// false says the target itself was not read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub version: String,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub result: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub version: String,
    pub exists: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub job: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub memory: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub turns: i64,
}

/// What `ConversationSnapshot::of` hashes: the agent as its files and row have it.
pub struct ConversationState<'a> {
    pub name: &'a str,
    pub aliases: &'a [String],
    pub job: &'a str,
    pub voice: Option<&'a str>,
    pub speed: Option<f64>,
    pub persona: &'a str,
    pub memory: &'a str,
    pub notes: &'a str,
    pub turns: i64,
    pub deleted: bool,
    pub retired: bool,
}

impl ConversationSnapshot {
    pub fn missing() -> Self {
        Self {
            version: "missing".into(),
            exists: false,
            name: String::new(),
            aliases: Vec::new(),
            job: String::new(),
            voice: None,
            speed: None,
            retired: false,
            persona: String::new(),
            memory: String::new(),
            notes: String::new(),
            turns: 0,
        }
    }

    pub fn of(s: ConversationState<'_>) -> Self {
        let mut h = Sha256::new();
        let speed = s.speed.map(|v| format!("{v}")).unwrap_or_default();
        for part in [
            s.name,
            &s.aliases.join(","),
            s.job,
            s.voice.unwrap_or(""),
            &speed,
            s.persona,
            s.memory,
            s.notes,
        ] {
            h.update(part.as_bytes());
            h.update(b"\0");
        }
        h.update(if s.retired {
            b"retired".as_slice()
        } else {
            b"active".as_slice()
        });
        h.update(s.turns.to_string().as_bytes());
        h.update(if s.deleted {
            b"deleted".as_slice()
        } else {
            b"live".as_slice()
        });
        let digest = format!("{:x}", h.finalize());
        Self {
            version: digest[..16].to_string(),
            exists: !s.deleted,
            name: s.name.to_string(),
            aliases: s.aliases.to_vec(),
            job: s.job.to_string(),
            voice: s.voice.map(String::from),
            speed: s.speed,
            retired: s.retired,
            persona: s.persona.to_string(),
            memory: s.memory.to_string(),
            notes: s.notes.to_string(),
            turns: s.turns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSnapshot {
    pub version: String,
    pub exists: bool,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulesSnapshot {
    pub version: String,
    #[serde(default)]
    pub rules: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSnapshot {
    pub version: String,
    #[serde(default)]
    pub present: bool,
    #[serde(default)]
    pub description: String,
}

/// The file's length and mtime, so an append is refused if something else wrote in
/// between (the version check catches that).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub version: String,
    pub exists: bool,
    pub length: u64,
    /// The last line, for the spoken form.
    #[serde(default)]
    pub tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredSnapshot {
    pub version: String,
    pub type_name: String,
    pub data: Map<String, Value>,
}

#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FakeIssueSnap {
    pub version: String,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
}

/// What the target looked like when the proposal was prepared. `version` is compared
/// again right before execution: if the target changed in between, the proposal is stale.
#[derive(Debug, Clone, PartialEq)]
pub enum Snapshot {
    App(AppSnapshot),
    Conversation(ConversationSnapshot),
    Output(OutputSnapshot),
    Rules(RulesSnapshot),
    Skill(SkillSnapshot),
    File(FileSnapshot),
    Retired(RetiredSnapshot),
    #[cfg(any(test, feature = "fakes"))]
    FakeIssue(FakeIssueSnap),
}

impl Snapshot {
    pub fn version(&self) -> &str {
        match self {
            Snapshot::App(s) => &s.version,
            Snapshot::Conversation(s) => &s.version,
            Snapshot::Output(s) => &s.version,
            Snapshot::Rules(s) => &s.version,
            Snapshot::Skill(s) => &s.version,
            Snapshot::File(s) => &s.version,
            Snapshot::Retired(s) => &s.version,
            #[cfg(any(test, feature = "fakes"))]
            Snapshot::FakeIssue(s) => &s.version,
        }
    }

    pub fn spoken(&self) -> String {
        match self {
            Snapshot::App(s) => match &s.result {
                Some(r) if !r.is_empty() => {
                    let text = Value::Object(r.clone()).to_string();
                    if text.chars().count() <= MAX_RESULT_TEXT {
                        text
                    } else {
                        format!("{}…", prefix_chars(&text, MAX_RESULT_TEXT - 1))
                    }
                }
                _ => "not checked".into(),
            },
            Snapshot::Conversation(s) => {
                if !s.exists {
                    return "does not exist".into();
                }
                let mut bits = vec![format!("{} turns on record", s.turns)];
                if !s.job.is_empty() {
                    bits.insert(0, s.job.clone());
                }
                if let Some(v) = &s.voice {
                    bits.push(format!("voice {v}"));
                }
                if s.retired {
                    bits.push("retired".into());
                }
                if !s.notes.trim().is_empty() {
                    bits.push(format!("{} notes", s.notes.matches('\n').count() + 1));
                }
                format!("{}: {}", s.name, bits.join(", "))
            }
            Snapshot::Output(s) => {
                if s.exists {
                    format!("{} bytes", commas(s.size))
                } else {
                    "not there yet".into()
                }
            }
            Snapshot::Rules(s) => {
                let lines = if s.rules.trim().is_empty() {
                    0
                } else {
                    s.rules.matches('\n').count() + 1
                };
                format!("{lines} line(s) of rules")
            }
            Snapshot::Skill(s) => if s.present {
                "installed"
            } else {
                "not installed"
            }
            .into(),
            Snapshot::File(s) => {
                if !s.exists {
                    "does not exist".into()
                } else if !s.tail.is_empty() {
                    format!("{} characters, ending {}", s.length, py_repr(&s.tail))
                } else {
                    "empty".into()
                }
            }
            Snapshot::Retired(s) => {
                let mut parts = Vec::new();
                if let Some(state) = s.data.get("state") {
                    parts.push(py_str(state));
                }
                if let Some(Value::Array(labels)) = s.data.get("labels") {
                    let names: Vec<String> = labels.iter().map(py_str).collect();
                    let joined = if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    };
                    parts.push(format!("labels {joined}"));
                }
                if parts.is_empty() {
                    "as recorded".into()
                } else {
                    parts.join(", ")
                }
            }
            #[cfg(any(test, feature = "fakes"))]
            Snapshot::FakeIssue(s) => {
                let labels = if s.labels.is_empty() {
                    "none".to_string()
                } else {
                    s.labels.join(", ")
                };
                format!("{}, labels {labels}", s.state)
            }
        }
    }
}

// --- changes ------------------------------------------------------------------------

/// One call to an app: the tool and the arguments beyond the ones in the target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppChange {
    pub tool: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

/// Hire: a new agent, with a persona and a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Create {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub job: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub persona: String,
    /// The first memory: a handoff from a retired agent, or a summary of this session.
    #[serde(default)]
    pub memory: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rename {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// None: unchanged.
    #[serde(default)]
    pub job: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetVoice {
    /// None: back to Ghost's voice.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetPersona {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendNote {
    pub text: String,
}

/// The whole notes file. Produced by `forget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetNotes {
    pub text: String,
}

/// Replace the rolling summary (a compaction, or an edit in the app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetMemory {
    pub text: String,
    #[serde(default)]
    pub through_turn_id: Option<i64>,
    #[serde(default)]
    pub source: CompactionSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteFile {
    pub content: String,
    #[serde(default)]
    pub replace: bool,
}

/// The operator's words, appended to AGENTS.md as they were said.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddRule {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddSkill {
    pub source_path: String,
    #[serde(default)]
    pub replace: bool,
}

/// A skill fetched from GitHub and shown to the operator, copied in from where it was
/// staged, with where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallSkill {
    pub staged_path: String,
    /// owner/repo
    pub source: String,
    pub skill_path: String,
    pub commit: String,
    pub folder_hash: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendText {
    pub text: String,
}

#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SetLabels {
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddComment {
    pub body: String,
}

#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetState {
    pub state: String,
}

/// One bounded step.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    App(AppChange),
    Create(Create),
    Retire,
    Rehire,
    Rename(Rename),
    SetVoice(SetVoice),
    SetPersona(SetPersona),
    AppendNote(AppendNote),
    SetNotes(SetNotes),
    SetMemory(SetMemory),
    Reset,
    Delete,
    WriteFile(WriteFile),
    AddRule(AddRule),
    AddSkill(AddSkill),
    InstallSkill(InstallSkill),
    RemoveSkill,
    AppendText(AppendText),
    Retired(Retired),
    #[cfg(any(test, feature = "fakes"))]
    SetLabels(SetLabels),
    #[cfg(any(test, feature = "fakes"))]
    AddComment(AddComment),
    #[cfg(any(test, feature = "fakes"))]
    SetState(SetState),
}

fn short(v: &Value) -> String {
    if let Value::Array(items) = v {
        let parts: Vec<String> = items.iter().map(py_str).collect();
        return if parts.is_empty() {
            "none".into()
        } else {
            parts.join(", ")
        };
    }
    let s = py_str(v);
    if s.chars().count() <= 80 {
        s
    } else {
        format!("{}…", prefix_chars(&s, 79))
    }
}

impl Change {
    pub fn describe(&self) -> String {
        match self {
            Change::App(c) => {
                let what = c.tool.replace('_', " ");
                let shown: Vec<String> = c
                    .arguments
                    .iter()
                    .map(|(k, v)| format!("{k} {}", short(v)))
                    .collect();
                if shown.is_empty() {
                    what
                } else {
                    format!("{what}: {}", shown.join(", "))
                }
            }
            Change::Create(c) => {
                let job = if c.job.trim().is_empty() {
                    String::new()
                } else {
                    format!(" for {}", c.job)
                };
                let seed = if c.memory.trim().is_empty() {
                    ""
                } else {
                    " with a handoff memory"
                };
                format!("hire {}{job}{seed}", py_repr(&c.name))
            }
            Change::Retire => "retire the agent".into(),
            Change::Rehire => "bring the agent back".into(),
            Change::Rename(c) => {
                let extra = if c.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (also {})", c.aliases.join(", "))
                };
                let job = c
                    .job
                    .as_ref()
                    .map(|j| format!(", job: {j}"))
                    .unwrap_or_default();
                format!("rename to {}{extra}{job}", py_repr(&c.name))
            }
            Change::SetVoice(c) => match &c.voice {
                None => "voice: Ghost's".into(),
                Some(v) => {
                    let speed = c.speed.map(|s| format!(" at {s}x")).unwrap_or_default();
                    format!("voice {v}{speed}")
                }
            },
            Change::SetPersona(c) => {
                let t = c.text.trim();
                if t.is_empty() {
                    "clear persona".into()
                } else {
                    format!("persona: {}", py_repr(&prefix_chars(t, 60)))
                }
            }
            Change::AppendNote(c) => format!("remember: {}", c.text.trim()),
            Change::SetNotes(_) => "update notes".into(),
            Change::SetMemory(c) => if c.source == CompactionSource::Edit {
                "edit memory"
            } else {
                "compact"
            }
            .into(),
            Change::Reset => "reset: forget everything".into(),
            Change::Delete => "delete the agent".into(),
            Change::WriteFile(c) => {
                let verb = if c.replace {
                    "replace it with"
                } else {
                    "write"
                };
                format!("{verb} {} bytes", commas(c.content.len() as u64))
            }
            Change::AddRule(c) => format!("add the rule: {}", c.text.trim()),
            Change::AddSkill(c) => {
                format!(
                    "copy {} into Ghost's skills{}",
                    c.source_path,
                    if c.replace { " (replacing it)" } else { "" }
                )
            }
            Change::InstallSkill(c) => format!(
                "install it from {} at {} into Ghost's skills{}",
                c.source,
                prefix_chars(&c.commit, 7),
                if c.replace { " (replacing it)" } else { "" }
            ),
            Change::RemoveSkill => "remove it from Ghost's skills".into(),
            Change::AppendText(c) => format!("append: {}", c.text.trim()),
            Change::Retired(r) => format!("{} (no longer supported)", r.type_name),
            #[cfg(any(test, feature = "fakes"))]
            Change::SetLabels(c) => {
                let mut parts = Vec::new();
                if !c.add.is_empty() {
                    parts.push(format!("add {}", c.add.join(", ")));
                }
                if !c.remove.is_empty() {
                    parts.push(format!("remove {}", c.remove.join(", ")));
                }
                format!("labels: {}", parts.join("; "))
            }
            #[cfg(any(test, feature = "fakes"))]
            Change::AddComment(c) => format!("comment: {}", c.body),
            #[cfg(any(test, feature = "fakes"))]
            Change::SetState(c) => format!("{} the issue", c.state),
        }
    }

    pub fn rule_args(&self) -> Map<String, Value> {
        match self {
            Change::App(c) => c.arguments.clone(),
            Change::InstallSkill(c) => {
                Map::from_iter([("source".to_string(), Value::String(c.source.clone()))])
            }
            _ => Map::new(),
        }
    }
}

/// One to four steps, applied in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChangeSet {
    steps: Vec<Change>,
}

impl ChangeSet {
    pub fn new(steps: Vec<Change>) -> Result<Self, ModelError> {
        if steps.is_empty() || steps.len() > MAX_STEPS {
            return Err(ModelError::Steps(steps.len()));
        }
        Ok(Self { steps })
    }

    pub fn one(step: Change) -> Self {
        Self { steps: vec![step] }
    }

    pub fn steps(&self) -> &[Change] {
        &self.steps
    }

    pub fn describe(&self) -> String {
        self.steps
            .iter()
            .map(Change::describe)
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn rule_args(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for s in &self.steps {
            out.extend(s.rule_args());
        }
        out
    }
}

// --- proposals ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Claimed,
    Cancelled,
    Expired,
    Stale,
    Executed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Claimed => "claimed",
            ProposalStatus::Cancelled => "cancelled",
            ProposalStatus::Expired => "expired",
            ProposalStatus::Stale => "stale",
            ProposalStatus::Executed => "executed",
        }
    }

    /// A stored status. Ghost once had an undo feature that marked a reversed proposal
    /// "undone"; the write did happen, and its reversal is its own row, so it reads back
    /// as executed.
    pub fn from_db(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "claimed" => Some(Self::Claimed),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            "stale" => Some(Self::Stale),
            "executed" | "undone" => Some(Self::Executed),
            _ => None,
        }
    }
}

fn default_autonomy() -> String {
    "confirm".into()
}

/// Immutable once stored. Only `status` changes, through the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub session_id: String,
    pub ability: String,
    pub operation: String,
    pub target: Target,
    pub changes: ChangeSet,
    pub rationale: String,
    pub snapshot: Snapshot,
    pub preview: String,
    /// The level the permissions resolved to; audit, not control.
    #[serde(default = "default_autonomy")]
    pub autonomy: String,
    /// The agent (ADR 0014) the session was with when the model proposed this.
    #[serde(default)]
    pub conversation: Option<String>,
    /// The words that make the operator's plain request the yes (ADR 0017), read off
    /// this call (ADR 0034); empty falls back to the operation's own.
    #[serde(default)]
    pub asked_by: Vec<String>,
    #[serde(with = "crate::time::json")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "crate::time::json")]
    pub expires_at: DateTime<Utc>,
    #[serde(default = "default_status")]
    pub status: ProposalStatus,
}

fn default_status() -> ProposalStatus {
    ProposalStatus::Pending
}

/// What a proposal is made from.
pub struct Draft {
    pub session_id: String,
    pub ability: String,
    pub operation: String,
    pub target: Target,
    pub changes: ChangeSet,
    pub rationale: String,
    pub snapshot: Snapshot,
    pub preview: String,
}

impl Proposal {
    pub fn new(d: Draft) -> Self {
        let now = utcnow();
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            session_id: d.session_id,
            ability: d.ability,
            operation: d.operation,
            target: d.target,
            changes: d.changes,
            rationale: d.rationale,
            snapshot: d.snapshot,
            preview: d.preview,
            autonomy: default_autonomy(),
            conversation: None,
            asked_by: Vec::new(),
            created_at: now,
            expires_at: now + Duration::seconds(PROPOSAL_TTL_SECS),
            status: ProposalStatus::Pending,
        }
    }

    pub fn idempotency_key(&self) -> String {
        format!("ghost:{}", self.id)
    }

    pub fn is_expired(&self, now: Option<DateTime<Utc>>) -> bool {
        now.unwrap_or_else(utcnow) >= self.expires_at
    }
}

// --- outcomes -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeStatus {
    Success,
    Partial,
    Failed,
}

/// The verified result the agent speaks. Never a guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub proposal_id: String,
    pub target: Target,
    pub status: OutcomeStatus,
    pub applied: Vec<String>,
    #[serde(default)]
    pub failed_step: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub readback: Option<Snapshot>,
}

impl Outcome {
    pub fn spoken(&self) -> String {
        let now = self
            .readback
            .as_ref()
            .map_or_else(|| "unknown state".to_string(), Snapshot::spoken);
        match self.status {
            OutcomeStatus::Success => format!("Done. {} now: {now}.", self.target),
            OutcomeStatus::Partial => format!(
                "Partly done on {}. Applied {}; {} failed: {}. It now: {now}.",
                self.target,
                self.applied.join(", "),
                self.failed_step.as_deref().unwrap_or("None"),
                self.error.as_deref().unwrap_or("None"),
            ),
            OutcomeStatus::Failed => format!(
                "Nothing changed on {}. {}",
                self.target,
                self.error.as_deref().unwrap_or("None")
            ),
        }
    }

    /// Terse form for the model to paraphrase after an autonomous write.
    pub fn summary(&self) -> String {
        match self.status {
            OutcomeStatus::Success => {
                format!(
                    "Applied on {}: {}.",
                    self.target,
                    self.applied.join("; ").trim_end_matches('.')
                )
            }
            OutcomeStatus::Partial => format!(
                "Partly applied on {}: {}. Failed: {} ({}).",
                self.target,
                self.applied.join("; "),
                self.failed_step.as_deref().unwrap_or("None"),
                self.error.as_deref().unwrap_or("None"),
            ),
            OutcomeStatus::Failed => format!(
                "Nothing changed on {}: {}",
                self.target,
                self.error.as_deref().unwrap_or("None")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_app_target_names_what_a_change_is_about_not_where() {
        let t = Target::App(AppTarget {
            app: "github".into(),
            tool: "issue_write".into(),
            fields: Map::from_iter([
                ("owner".to_string(), json!("acme")),
                ("repo".to_string(), json!("app")),
                ("number".to_string(), json!(142)),
                ("path".to_string(), json!("README.md")),
                ("draft".to_string(), json!(true)),
            ]),
            title: "GitHub".into(),
        });
        assert_eq!(
            t.key(),
            "app:github:issue_write:True/142/acme/README.md/app"
        );
        assert_eq!(t.spoken(), "GitHub True 142 acme README.md app");
        let m = t.mentions();
        assert!(m.contains("142") && m.contains("one forty two") && m.contains("readme md"));
        assert!(!m.contains("acme") && !m.contains("app"));
        assert!(says_where("repo_name") && says_where("teamId") && says_where("base_branch"));
        assert!(!says_where("number") && !says_where("path") && !says_where("id"));
    }

    #[test]
    fn ghosts_own_targets_speak_and_scope_as_before() {
        let c = Target::Conversation(ConversationTarget {
            slug: "brand-design-emea".into(),
        });
        assert_eq!(c.key(), "agent:brand-design-emea");
        assert_eq!(c.spoken(), "brand design emea");
        assert!(c.mentions().contains("brand") && c.mentions().contains("brand design emea"));
        assert_eq!(c.rule_args()["target"], "brand-design-emea");
        let o = Target::Output(OutputTarget {
            name: "Week-in-review.html".into(),
        });
        assert!(o.mentions().contains("week-in-review.html") && o.mentions().contains("review"));
        assert_eq!(Target::Rules(RulesTarget {}).scope(), "ghost");
        let s = Target::Skill(SkillTarget {
            name: "pr.review_notes".into(),
        });
        assert!(s.mentions().contains("review") && s.mentions().contains("pr.review_notes"));
        assert_eq!(s.scope(), "ghost");
        let f = Target::File(FileTarget {
            path: "/home/me/notes.md".into(),
        });
        assert_eq!(
            (f.key(), f.scope(), f.spoken()),
            (
                "file:/home/me/notes.md".into(),
                "/home/me".into(),
                "notes.md".into()
            )
        );
    }

    #[test]
    fn snapshots_and_changes_describe_themselves() {
        let s = Snapshot::Output(OutputSnapshot {
            version: "v".into(),
            exists: true,
            size: 12345,
        });
        assert_eq!(s.spoken(), "12,345 bytes");
        let s = Snapshot::Rules(RulesSnapshot {
            version: "v".into(),
            rules: "a\nb".into(),
        });
        assert_eq!(s.spoken(), "2 line(s) of rules");
        let c = Change::Create(Create {
            name: "Ledger".into(),
            job: "the books".into(),
            ..Create {
                name: String::new(),
                aliases: vec![],
                job: String::new(),
                voice: None,
                speed: None,
                persona: String::new(),
                memory: "x".into(),
            }
        });
        assert_eq!(
            c.describe(),
            "hire 'Ledger' for the books with a handoff memory"
        );
        let c = Change::SetVoice(SetVoice {
            voice: Some("af_heart".into()),
            speed: Some(1.15),
        });
        assert_eq!(c.describe(), "voice af_heart at 1.15x");
        let c = Change::App(AppChange {
            tool: "issue_write".into(),
            arguments: Map::from_iter([("labels".to_string(), json!(["bug", "p1"]))]),
        });
        assert_eq!(c.describe(), "issue write: labels bug, p1");
        let c = Change::WriteFile(WriteFile {
            content: "x".repeat(2048),
            replace: true,
        });
        assert_eq!(c.describe(), "replace it with 2,048 bytes");
        let cs = ChangeSet::new(vec![Change::Retire, Change::Rehire]).unwrap();
        assert_eq!(cs.describe(), "retire the agent; bring the agent back");
        assert_eq!(ChangeSet::new(vec![]).unwrap_err(), ModelError::Steps(0));
        assert_eq!(
            ChangeSet::new(vec![Change::Retire; 5]).unwrap_err(),
            ModelError::Steps(5)
        );
    }

    #[test]
    fn a_proposal_expires_after_its_ttl_and_keys_its_operation() {
        let p = Proposal::new(Draft {
            session_id: "s".into(),
            ability: "x".into(),
            operation: "y".into(),
            target: Target::Rules(RulesTarget {}),
            changes: ChangeSet::one(Change::AddRule(AddRule {
                text: "be brief".into(),
            })),
            rationale: String::new(),
            snapshot: Snapshot::Rules(RulesSnapshot {
                version: "v".into(),
                rules: String::new(),
            }),
            preview: "p".into(),
        });
        assert_eq!(p.idempotency_key(), format!("ghost:{}", p.id));
        assert!(!p.is_expired(None));
        assert!(p.is_expired(Some(p.created_at + Duration::seconds(PROPOSAL_TTL_SECS))));
        assert_eq!(p.autonomy, "confirm");
        assert_eq!(
            ProposalStatus::from_db("undone"),
            Some(ProposalStatus::Executed)
        );
    }

    #[test]
    fn outcomes_say_what_happened() {
        let target = Target::Conversation(ConversationTarget {
            slug: "ledger".into(),
        });
        let ok = Outcome {
            proposal_id: "p".into(),
            target: target.clone(),
            status: OutcomeStatus::Success,
            applied: vec!["retire the agent.".into()],
            failed_step: None,
            error: None,
            readback: None,
        };
        assert_eq!(ok.spoken(), "Done. ledger now: unknown state.");
        assert_eq!(ok.summary(), "Applied on ledger: retire the agent.");
        let bad = Outcome {
            status: OutcomeStatus::Failed,
            error: Some("boom".into()),
            ..ok.clone()
        };
        assert_eq!(bad.spoken(), "Nothing changed on ledger. boom");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a\nb"), "'a\\nb'");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1000), "1,000");
        assert_eq!(commas(1234567), "1,234,567");
    }
}
