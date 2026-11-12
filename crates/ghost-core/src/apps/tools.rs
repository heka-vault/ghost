//! Tools and results: what the model sees of an app's tool, how a result is fitted to its
//! budget, how the model's arguments are bounded, and the words that ask for a call.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::AppError;
use crate::redact::redact_text;

/// A string argument (ADR 0046; it was 16,000): a document an agent writes fits.
pub const MAX_TEXT_CHARS: usize = 200_000;
/// A list argument (it was 100).
pub const MAX_LIST_ITEMS: usize = 1_000;
/// One whole result as the model reads it, by default (`limits::RESULT_CHARS`).
pub const MAX_RESULT_TEXT: usize = crate::models::MAX_RESULT_TEXT;
pub use super::limits::{DESCRIPTION_MAX, MAX_RESULT_ITEMS};
/// A watch's read is not fitted to the model's budget, but it is capped.
pub const MAX_WATCH_BYTES: usize = 2_000_000;
/// The first sentence of a description, for the operator's list of an app's tools.
pub const BRIEF_MAX: usize = 240;
/// An argument short enough to name what a change is about.
pub const MAX_FIELD_CHARS: usize = 60;
/// The model API's limit; its pattern is `^[a-zA-Z0-9_-]{1,64}$`.
pub const MAX_TOOL_NAME: usize = 64;

const ABBREVIATIONS: [&str; 6] = ["e.g.", "i.e.", "etc.", "vs.", "cf.", "no."];
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?]\s").unwrap());
static TOOL_NAME_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9_-]+").unwrap());
static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").unwrap());
static LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z]+").unwrap());
static NAMING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|_)(id|ids|number|num|owner|org|repo|repository|project|team|workspace|channel|user|username|email|to|name|slug|key|path|file|page|database|board|list|issue|pr|branch|ref|account|customer|space)($|_)|Id$|Number$|Name$",
    )
    .unwrap()
});

/// The words that can ask for a change, read off its tool's name and its short arguments
/// (ADR 0017, ADR 0034): "close seventeen" asks for `issue_write` with `state=closed`.
/// Nouns ("issue", "file") and vague verbs ("write", "update", "set") are not among them,
/// so a request that names only the thing never counts as the yes.
pub const VERBS: &[&str] = &[
    "add",
    "append",
    "archive",
    "assign",
    "approve",
    "book",
    "block",
    "cancel",
    "close",
    "comment",
    "copy",
    "create",
    "decline",
    "delete",
    "dismiss",
    "draft",
    "edit",
    "email",
    "forward",
    "invite",
    "label",
    "lock",
    "merge",
    "message",
    "move",
    "mute",
    "open",
    "pin",
    "post",
    "publish",
    "push",
    "react",
    "remove",
    "rename",
    "reopen",
    "reply",
    "request",
    "resolve",
    "revert",
    "schedule",
    "send",
    "share",
    "star",
    "submit",
    "subscribe",
    "tag",
    "transfer",
    "trash",
    "unarchive",
    "unassign",
    "unlabel",
    "unlock",
    "unpin",
    "unshare",
    "unstar",
    "unsubscribe",
    "upload",
];
const STEMS: &[(&str, &str)] = &[
    ("closed", "close"),
    ("opened", "open"),
    ("merged", "merge"),
    ("deleted", "delete"),
    ("archived", "archive"),
    ("locked", "lock"),
    ("unlocked", "unlock"),
    ("assigned", "assign"),
    ("resolved", "resolve"),
    ("removed", "remove"),
    ("sent", "send"),
    ("starred", "star"),
    ("reopened", "reopen"),
    ("published", "publish"),
    ("labels", "label"),
    ("comments", "comment"),
    ("messages", "message"),
    ("drafts", "draft"),
    ("issues", ""),
    ("trashed", "trash"),
    ("reply", "reply"),
    ("replies", "reply"),
    ("approved", "approve"),
    ("declined", "decline"),
];
const ALIASES: &[(&str, &[&str])] = &[
    ("close", &["shut"]),
    ("delete", &["remove"]),
    ("send", &["email"]),
    ("comment", &["note"]),
];

/// What a tool is: one the app lists, or one Ghost offers over what the app advertises
/// besides tools (ADR 0046).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolKind {
    #[default]
    Tool,
    ListResources,
    ReadResource,
    ListPrompts,
    GetPrompt,
}

/// One tool as the app lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    /// The app's, in full and redacted: for the operator (`GET /apps`).
    pub description: String,
    pub schema: Map<String, Value>,
    pub read_only: bool,
    pub kind: ToolKind,
}

impl ToolInfo {
    pub fn new(name: &str, description: &str, schema: Map<String, Value>, read_only: bool) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            read_only,
            kind: ToolKind::Tool,
        }
    }

    /// The four reads Ghost offers over an app's resources and prompts, when the app
    /// advertises them. The pattern is Hermes Agent's utility tools (MIT,
    /// `tools/mcp_tool_schema.py`).
    pub(crate) fn utilities(resources: bool, prompts: bool) -> Vec<ToolInfo> {
        let object = |props: Value, required: &[&str]| -> Map<String, Value> {
            serde_json::json!({"type": "object", "properties": props, "required": required})
                .as_object()
                .cloned()
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        if resources {
            out.push(ToolInfo {
                kind: ToolKind::ListResources,
                ..ToolInfo::new(
                    "list_resources",
                    "List the documents and data this app offers to read, each with its address.",
                    object(serde_json::json!({}), &[]),
                    true,
                )
            });
            out.push(ToolInfo {
                kind: ToolKind::ReadResource,
                ..ToolInfo::new(
                    "read_resource",
                    "Read one of this app's documents by its address, from list_resources.",
                    object(
                        serde_json::json!({"uri": {"type": "string",
                            "description": "The address list_resources gave."}}),
                        &["uri"],
                    ),
                    true,
                )
            });
        }
        if prompts {
            out.push(ToolInfo {
                kind: ToolKind::ListPrompts,
                ..ToolInfo::new(
                    "list_prompts",
                    "List the ready-made prompts this app offers, with the arguments each takes.",
                    object(serde_json::json!({}), &[]),
                    true,
                )
            });
            out.push(ToolInfo {
                kind: ToolKind::GetPrompt,
                ..ToolInfo::new(
                    "get_prompt",
                    "Get one of this app's ready-made prompts by name, filled in with arguments.",
                    object(
                        serde_json::json!({
                            "name": {"type": "string", "description": "From list_prompts."},
                            "arguments": {"type": "object",
                                "description": "The prompt's arguments, as text."},
                        }),
                        &["name"],
                    ),
                    true,
                )
            });
        }
        out
    }

    /// What the model reads about the tool: the app's description up to
    /// [`DESCRIPTION_MAX`] characters (ADR 0046). It is the app's text, so it is data: the
    /// prompt says so, and a password in it was replaced when the tool was listed.
    pub fn offered_description(&self) -> String {
        let text = self.description.trim();
        if text.is_empty() {
            return self.name.replace('_', " ");
        }
        if text.chars().count() <= DESCRIPTION_MAX {
            return text.to_string();
        }
        format!("{}…", prefix_chars(text, DESCRIPTION_MAX - 1).trim_end())
    }

    /// How big the tool is on a turn: about what the model API is sent for it.
    pub fn bytes(&self) -> usize {
        self.name.len()
            + self.offered_description().len()
            + Value::Object(self.schema.clone()).to_string().len()
    }

    /// The first sentence: what the tool does, for the operator's list. An app's
    /// description is often a page aimed at agents.
    pub fn brief(&self) -> String {
        let b = brief(&self.description);
        if b.is_empty() {
            self.name.replace('_', " ")
        } else {
            b
        }
    }

    pub fn properties(&self) -> Map<String, Value> {
        match self.schema.get("properties") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        }
    }

    pub fn required(&self) -> Vec<String> {
        match self.schema.get("required") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn brief(text: &str) -> String {
    let mut first = text
        .trim()
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    for m in SENTENCE_END.find_iter(&first.clone()) {
        let head = &first[..m.start() + 1];
        let lower = head.to_lowercase();
        if ABBREVIATIONS.iter().any(|a| lower.ends_with(a)) {
            continue;
        }
        first = head.trim().to_string();
        break;
    }
    if first.chars().count() > BRIEF_MAX {
        first = format!("{}…", prefix_chars(&first, BRIEF_MAX - 1).trim_end());
    }
    first
}

/// How long a value is as the model reads it: its JSON, in characters.
fn json_chars(value: &Value) -> usize {
    value.to_string().chars().count()
}

/// Lists cut to [`MAX_RESULT_ITEMS`], nested too, each cut saying how many more there were.
fn cap_lists(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            let mut out: Vec<Value> = items.iter().take(MAX_RESULT_ITEMS).map(cap_lists).collect();
            if items.len() > MAX_RESULT_ITEMS {
                out.push(Value::String(format!(
                    "… {} more items",
                    items.len() - MAX_RESULT_ITEMS
                )));
            }
            Value::Array(out)
        }
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), cap_lists(v))).collect())
        }
        other => other.clone(),
    }
}

/// `value` in document order until `left` characters are spent: what comes first is kept
/// whole, the string the budget ends in is cut there, and what follows is left out.
fn take(value: &Value, left: &mut usize) -> Value {
    match value {
        Value::String(s) => {
            let n = s.chars().count();
            if n + 2 <= *left {
                *left -= n + 2;
                return value.clone();
            }
            let keep = left.saturating_sub(3);
            *left = 0;
            Value::String(format!("{}…", prefix_chars(s, keep)))
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                if *left == 0 {
                    out.push(Value::String(format!("… {} more items", items.len() - i)));
                    break;
                }
                out.push(take(item, left));
                *left = left.saturating_sub(1);
            }
            Value::Array(out)
        }
        Value::Object(m) => {
            let mut out = Map::new();
            for (i, (k, v)) in m.iter().enumerate() {
                if *left == 0 {
                    out.insert(
                        "…".into(),
                        Value::String(format!("{} more fields", m.len() - i)),
                    );
                    break;
                }
                *left = left.saturating_sub(k.chars().count() + 4);
                out.insert(k.clone(), take(v, left));
            }
            Value::Object(out)
        }
        other => {
            *left = left.saturating_sub(other.to_string().len());
            other.clone()
        }
    }
}

/// The key of the note a cut result carries.
pub const CUT_NOTE: &str = "_cut";

/// One result fitted to its budget (ADR 0046): whole when it is within `budget`
/// characters, else cut at the end, with a note saying how much was dropped so the model
/// can ask for less rather than take the cut for the whole. Lists keep
/// [`MAX_RESULT_ITEMS`] items within the budget.
pub(crate) fn fit(value: &Value, budget: usize) -> Value {
    let whole = json_chars(value);
    let capped = cap_lists(value);
    if json_chars(&capped) <= budget {
        return capped;
    }
    let mut left = budget;
    let cut = take(&capped, &mut left);
    let kept = json_chars(&cut);
    let note = Value::String(format!(
        "cut to fit: about {} of {whole} characters were dropped from the end. Ask for less \
         (one item, a page, a narrower search) to read the rest.",
        whole.saturating_sub(kept)
    ));
    match cut {
        Value::Object(mut m) => {
            m.insert(CUT_NOTE.into(), note);
            Value::Object(m)
        }
        other => Value::Object(Map::from_iter([
            ("result".to_string(), other),
            (CUT_NOTE.to_string(), note),
        ])),
    }
}

/// What came back besides text, by kind and size: an image, a sound, a file. The model
/// is told it is there; the bytes are not sent to it.
fn describe_block(block: &Value) -> Option<Value> {
    let kind = block.get("type").and_then(Value::as_str)?;
    let bytes_of = |b64: Option<&Value>| -> usize {
        b64.and_then(Value::as_str).map_or(0, |s| s.len() / 4 * 3)
    };
    let field = |key: &str| block.get(key).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    match kind {
        "text" => return None,
        "image" | "audio" => {
            out.insert("kind".into(), Value::String(kind.into()));
            out.insert("type".into(), field("mimeType"));
            out.insert("bytes".into(), Value::from(bytes_of(block.get("data"))));
        }
        "resource" => {
            let inner = block.get("resource")?;
            if inner.get("text").is_some() {
                return None; // read as text
            }
            out.insert("kind".into(), Value::String("file".into()));
            out.insert(
                "address".into(),
                inner.get("uri").cloned().unwrap_or(Value::Null),
            );
            out.insert(
                "type".into(),
                inner.get("mimeType").cloned().unwrap_or(Value::Null),
            );
            out.insert("bytes".into(), Value::from(bytes_of(inner.get("blob"))));
        }
        "resource_link" => {
            out.insert("kind".into(), Value::String("link".into()));
            out.insert("address".into(), field("uri"));
            out.insert("name".into(), field("name"));
            out.insert("type".into(), field("mimeType"));
            out.insert("bytes".into(), field("size"));
        }
        other => {
            out.insert("kind".into(), Value::String(other.into()));
        }
    }
    out.retain(|_, v| !v.is_null());
    Some(Value::Object(out))
}

/// The key under which a result says what it carried besides text.
pub const ALSO_RETURNED: &str = "also_returned";

/// The whole result as an object: structured content as the JSON the app gave, else the
/// text (a text file it embedded counts), and under [`ALSO_RETURNED`] what else came
/// back, by kind and size. `fit` sizes it for the model.
pub(crate) fn result_to_dict(result: &CallToolResult) -> Result<Map<String, Value>, AppError> {
    let blocks: Vec<Value> = result
        .content
        .iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .collect();
    let owned: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b.get("type").and_then(Value::as_str) {
            Some("text") => b.get("text").and_then(Value::as_str).map(String::from),
            Some("resource") => b
                .get("resource")
                .and_then(|r| r.get("text"))
                .and_then(Value::as_str)
                .map(String::from),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect();
    let texts: Vec<&str> = owned.iter().map(String::as_str).collect();
    let others: Vec<Value> = blocks.iter().filter_map(describe_block).collect();
    let with_others = |mut m: Map<String, Value>| -> Map<String, Value> {
        if !others.is_empty() {
            m.insert(ALSO_RETURNED.into(), Value::Array(others.clone()));
        }
        m
    };
    if result.is_error == Some(true) {
        // An app's error is its text too: a password in it is replaced (ADR 0036).
        let text = redact_text(&texts.join(" "));
        return Err(AppError::Tool(if text.is_empty() {
            "the tool reported an error".into()
        } else {
            text
        }));
    }
    if let Some(structured) = &result.structured_content
        && super::spec::truthy(structured)
    {
        return Ok(with_others(match structured {
            Value::Object(m) => m.clone(),
            other => Map::from_iter([("result".to_string(), other.clone())]),
        }));
    }
    if texts.len() == 1 {
        return Ok(with_others(match serde_json::from_str::<Value>(texts[0]) {
            Ok(Value::Object(m)) => m,
            Ok(parsed) => Map::from_iter([("result".to_string(), parsed)]),
            Err(_) => Map::from_iter([("text".to_string(), Value::String(texts[0].to_string()))]),
        }));
    }
    if texts.is_empty() && !others.is_empty() {
        return Ok(with_others(Map::new()));
    }
    Ok(with_others(Map::from_iter([(
        "text".to_string(),
        Value::String(texts.join("\n")),
    )])))
}

/// The plain type a schema names, looking through `anyOf`/`oneOf` for an optional
/// argument (`integer | null`).
fn schema_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(k)) => return Some(k.clone()),
        Some(Value::Array(kinds)) => {
            return kinds
                .iter()
                .filter_map(Value::as_str)
                .find(|k| *k != "null")
                .map(String::from);
        }
        _ => {}
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(options)) = schema.get(key) {
            for option in options {
                if option.is_object()
                    && let Some(k) = schema_type(option)
                    && k != "null"
                {
                    return Some(k);
                }
            }
        }
    }
    None
}

/// The model's argument in the type the tool's schema says, where that is plain.
fn coerce(value: Value, schema: &Value) -> Value {
    let kind = schema_type(schema);
    match (kind.as_deref(), &value) {
        (Some("integer"), Value::String(s)) => {
            let t = s.trim();
            if !t.trim_start_matches('-').is_empty()
                && t.trim_start_matches('-')
                    .chars()
                    .all(|c| c.is_ascii_digit())
                && let Ok(n) = t.parse::<i64>()
            {
                return Value::from(n);
            }
            value
        }
        (Some("number"), Value::String(s)) => match s.trim().parse::<f64>() {
            Ok(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(value),
            Err(_) => value,
        },
        (Some("boolean"), Value::String(s)) => Value::Bool(matches!(
            s.trim().to_lowercase().as_str(),
            "true" | "yes" | "1"
        )),
        (Some("array"), Value::String(s)) => Value::Array(
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| Value::String(p.to_string()))
                .collect(),
        ),
        (Some("string"), Value::Number(n)) => Value::String(n.to_string()),
        _ => value,
    }
}

fn verb_words(text: &str) -> Vec<String> {
    let snake = CAMEL.replace_all(text, "${1}_${2}").to_lowercase();
    LETTERS
        .find_iter(&snake)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// The words that ask for this call (ADR 0017): the action verbs in the tool's name and in
/// its short string arguments, with their usual synonyms. Empty: always ask.
pub fn verbs_of(tool: &str, args: &Map<String, Value>) -> Vec<String> {
    let mut words = verb_words(tool);
    for v in args.values() {
        if let Value::String(s) = v
            && s.chars().count() <= 20
            && !s.trim().contains(' ')
        {
            words.extend(
                LETTERS
                    .find_iter(&s.to_lowercase())
                    .map(|m| m.as_str().to_string()),
            );
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |w: &str| {
        if !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    };
    for w in words {
        let w = STEMS
            .iter()
            .find(|(from, _)| *from == w)
            .map(|(_, to)| to.to_string())
            .unwrap_or(w);
        if VERBS.contains(&w.as_str()) {
            push(&w);
            if let Some((_, aliases)) = ALIASES.iter().find(|(v, _)| *v == w) {
                for a in *aliases {
                    push(a);
                }
            }
        }
    }
    out
}

/// `name` with a short hash of `whole` on its end, within the model API's length: the
/// name a tool gets when the plain one is taken. The idea is Goose's
/// `provider_safe_tool_name` (Apache-2.0,
/// `crates/goose/src/agents/extension_manager/lease.rs`): a rewritten name carries a hash
/// of the original, so `db.query` and `db_query`, or two names that share their first 64
/// characters, stay two tools.
pub fn hashed_tool_name(name: &str, whole: &str) -> String {
    let clean = TOOL_NAME_BAD.replace_all(name, "_").to_string();
    let digest = format!("{:x}", Sha256::digest(whole.as_bytes()));
    let head: String = clean.chars().take(MAX_TOOL_NAME - 9).collect();
    format!("{head}_{}", &digest[..8])
}

/// `name` as the model API accepts it: other characters become `_`, and a name too long
/// keeps its start and a short hash of the whole, so two long names stay two. Two names
/// that come out the same are told apart where they meet (`App::offered`).
pub fn model_tool_name(name: &str) -> String {
    let clean = TOOL_NAME_BAD.replace_all(name, "_").to_string();
    let clean = if clean.is_empty() {
        "tool".to_string()
    } else {
        clean
    };
    if clean.len() <= MAX_TOOL_NAME {
        return clean;
    }
    let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
    format!("{}_{}", &clean[..MAX_TOOL_NAME - 9], &digest[..8])
}

/// The model's arguments without Ghost's own (`rationale`, anything `_`-led).
pub(crate) fn clean(args: &Map<String, Value>) -> Map<String, Value> {
    args.iter()
        .filter(|(k, _)| *k != "rationale" && !k.starts_with('_'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// An argument that says what a change is about: a number, or an id or a name by its
/// argument's name (`issue_number`, `owner`, `repo`, `channel_id`). The rest (`method`,
/// `state`) is what the change does.
fn names_it(key: &str, v: &Value) -> bool {
    if v.is_boolean() || !NAMING.is_match(key) {
        return false;
    }
    match v {
        Value::Number(n) => n.is_i64() || n.is_u64(),
        Value::String(s) => {
            let n = s.chars().count();
            n > 0 && n <= MAX_FIELD_CHARS && !s.chars().any(char::is_whitespace)
        }
        _ => false,
    }
}

/// What a prepared call is made of: the arguments that name what it is about, the rest,
/// and the words that ask for it.
pub(crate) struct Bounded {
    pub fields: Map<String, Value>,
    pub rest: Map<String, Value>,
    pub verbs: Vec<String>,
}

/// The model's arguments, checked against the tool's own schema and bounded.
pub(crate) fn bound(tool: &ToolInfo, raw: &Map<String, Value>) -> Result<Bounded, AppError> {
    let args = clean(raw);
    let props = tool.properties();
    let additional = tool.schema.get("additionalProperties");
    if !props.is_empty() && additional != Some(&Value::Bool(true)) {
        let unknown: BTreeSet<&str> = args
            .keys()
            .map(String::as_str)
            .filter(|k| !props.contains_key(*k))
            .collect();
        if !unknown.is_empty() {
            return Err(AppError::Value(format!(
                "{} has no argument {}",
                tool.name,
                unknown.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    let empty = Value::Object(Map::new());
    let args: Map<String, Value> = args
        .into_iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| {
            let schema = props.get(&k).unwrap_or(&empty);
            (k, coerce(v, schema))
        })
        .collect();
    let missing: Vec<String> = tool
        .required()
        .into_iter()
        .filter(|r| !args.contains_key(r))
        .collect();
    if !missing.is_empty() {
        return Err(AppError::Value(format!(
            "{} needs {}",
            tool.name,
            missing.join(", ")
        )));
    }
    for (k, v) in &args {
        if let Value::String(s) = v
            && s.chars().count() > MAX_TEXT_CHARS
        {
            return Err(AppError::Value(format!(
                "{k} is too long ({} chars, at most {MAX_TEXT_CHARS})",
                s.chars().count()
            )));
        }
        if let Value::Array(a) = v
            && a.len() > MAX_LIST_ITEMS
        {
            return Err(AppError::Value(format!(
                "{k} has too many items ({}, at most {MAX_LIST_ITEMS})",
                a.len()
            )));
        }
    }
    let verbs = verbs_of(&tool.name, &args);
    let mut fields = Map::new();
    let mut rest = Map::new();
    for (k, v) in args {
        if names_it(&k, &v) {
            fields.insert(k, v);
        } else {
            rest.insert(k, v);
        }
    }
    Ok(Bounded {
        fields,
        rest,
        verbs,
    })
}

#[cfg(test)]
mod unit {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_brief_is_the_first_sentence_past_abbreviations() {
        assert_eq!(
            brief("Get an issue. Show this notice word for word."),
            "Get an issue."
        );
        assert_eq!(
            brief("Look things up, e.g. issues. Then more."),
            "Look things up, e.g. issues."
        );
        assert_eq!(brief("First line\nSecond line"), "First line");
        let long = "a".repeat(300);
        assert_eq!(brief(&long).chars().count(), BRIEF_MAX);
    }

    #[test]
    fn a_result_within_its_budget_is_whole() {
        // An issue body of 20,000 characters and 150 comments: what 1,200 and 30 cut.
        let v = json!({"body": "x".repeat(20_000), "items": (0..150).collect::<Vec<_>>()});
        assert_eq!(fit(&v, 24_000), v);
    }

    #[test]
    fn a_result_past_its_budget_is_cut_at_the_end_and_says_how_much() {
        let v = json!({"a_title": "Crash on start", "body": "x".repeat(50_000), "z": "last"});
        let t = fit(&v, 24_000);
        assert_eq!(t["a_title"], "Crash on start"); // what comes first is whole
        let body = t["body"].as_str().unwrap();
        assert!(body.ends_with('…') && body.chars().count() < 24_000);
        let size = t.to_string().chars().count();
        assert!(size <= 24_000 + 300, "{size}");
        let note = t[CUT_NOTE].as_str().unwrap();
        assert!(note.contains("of 50") && note.contains("dropped"), "{note}");
        // a result that is not an object keeps its shape under `result`
        let t = fit(&json!("y".repeat(100)), 40);
        assert!(t["result"].as_str().unwrap().ends_with('…'));
        assert!(t[CUT_NOTE].is_string());
    }

    #[test]
    fn a_long_list_keeps_two_hundred_items_within_the_budget() {
        let v = json!({"items": (0..500).collect::<Vec<_>>()});
        let t = fit(&v, 24_000);
        let items = t["items"].as_array().unwrap();
        assert_eq!(items.len(), MAX_RESULT_ITEMS + 1);
        assert_eq!(items.last().unwrap(), "… 300 more items");
        assert!(t.get(CUT_NOTE).is_none()); // the list's own note says it
        // and a list of big things stops where the budget does
        let big: Vec<Value> = (0..100)
            .map(|i| json!({"n": i, "body": "b".repeat(1_000)}))
            .collect();
        let t = fit(&json!({"issues": big}), 10_000);
        let issues = t["issues"].as_array().unwrap();
        assert!(issues.len() < 20, "{}", issues.len());
        let last = issues.last().unwrap().as_str().unwrap();
        assert!(last.ends_with("more items"), "{last}");
        assert!(t[CUT_NOTE].as_str().unwrap().contains("dropped"));
    }

    #[test]
    fn what_came_back_besides_text_is_named_with_its_size() {
        use rmcp::model::ContentBlock;
        let result = CallToolResult::success(vec![
            ContentBlock::text("A chart of this week."),
            ContentBlock::image("QUJDREVGR0g=", "image/png"),
        ]);
        let d = result_to_dict(&result).unwrap();
        assert_eq!(d["text"], "A chart of this week.");
        assert_eq!(
            d[ALSO_RETURNED],
            json!([{"kind": "image", "type": "image/png", "bytes": 9}])
        );
        // an image alone is still an answer, not an empty one
        let only = CallToolResult::success(vec![ContentBlock::image("QUJD", "image/jpeg")]);
        let d = result_to_dict(&only).unwrap();
        assert_eq!(d[ALSO_RETURNED][0]["kind"], "image");
        assert!(d.get("text").is_none());
        // a text file the app embedded is read as text
        let file = CallToolResult::success(vec![ContentBlock::embedded_text(
            "file:///notes.md",
            "the notes",
        )]);
        assert_eq!(result_to_dict(&file).unwrap()["text"], "the notes");
    }

    #[test]
    fn a_description_reaches_the_model_up_to_its_cap() {
        let t = ToolInfo::new(
            "search",
            "Search issues. Use `repo:` to narrow. Returns at most 30.",
            Map::new(),
            true,
        );
        assert_eq!(t.offered_description(), t.description); // not the first sentence
        assert_eq!(t.brief(), "Search issues.");
        let long = ToolInfo::new("x", &"d".repeat(3_000), Map::new(), true);
        let said = long.offered_description();
        assert_eq!(said.chars().count(), DESCRIPTION_MAX);
        assert!(said.ends_with('…'));
        let bare = ToolInfo::new("list_issues", "", Map::new(), true);
        assert_eq!(bare.offered_description(), "list issues");
    }

    #[test]
    fn arguments_are_coerced_to_the_schemas_plain_type() {
        assert_eq!(coerce(json!("42"), &json!({"type": "integer"})), json!(42));
        assert_eq!(
            coerce(
                json!("42"),
                &json!({"anyOf": [{"type": "integer"}, {"type": "null"}]})
            ),
            json!(42)
        );
        assert_eq!(coerce(json!("x"), &json!({"type": "integer"})), json!("x"));
        assert_eq!(coerce(json!("1.5"), &json!({"type": "number"})), json!(1.5));
        assert_eq!(
            coerce(json!("Yes"), &json!({"type": "boolean"})),
            json!(true)
        );
        assert_eq!(
            coerce(json!("a, b,,c"), &json!({"type": "array"})),
            json!(["a", "b", "c"])
        );
        assert_eq!(coerce(json!(7), &json!({"type": "string"})), json!("7"));
        assert_eq!(coerce(json!(true), &json!({"type": "string"})), json!(true));
    }

    #[test]
    fn a_long_tool_name_keeps_its_start_and_a_hash() {
        assert_eq!(model_tool_name("issue.read"), "issue_read");
        assert_eq!(model_tool_name(""), "tool");
        let long = "a_very_long_tool_name_".repeat(4);
        let a = model_tool_name(&long);
        let b = model_tool_name(&format!("{long}x"));
        assert_eq!(a.len(), MAX_TOOL_NAME);
        assert_ne!(a, b);
        assert!(a.starts_with("a_very_long_tool_name_"));
        let ok = Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").unwrap();
        // a name another tool has is told apart by a hash of its whole
        let h = hashed_tool_name("github_issue_read", "github_issue/read");
        assert!(h.starts_with("github_issue_read_"));
        assert_eq!(h.len(), "github_issue_read_".len() + 8);
        assert_ne!(
            h,
            hashed_tool_name("github_issue_read", "github/issue_read")
        );
        let long = "n".repeat(200);
        for name in ["db.query", "größe", long.as_str(), "a b/c"] {
            assert!(ok.is_match(&model_tool_name(name)), "{name}");
            let hashed = hashed_tool_name(&model_tool_name(name), name);
            assert!(ok.is_match(&hashed), "{name}");
        }
    }
}
