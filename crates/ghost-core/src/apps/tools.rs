//! Tools and results: what the model sees of an app's tool, how a result is sized for a
//! spoken turn, how the model's arguments are bounded, and the words that ask for a call.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::AppError;
use crate::redact::redact_text;

/// A string argument of a change.
pub const MAX_TEXT_CHARS: usize = 16_000;
/// A list argument of a change.
pub const MAX_LIST_ITEMS: usize = 100;
/// A string in a result, so one issue body never fills the context.
pub const MAX_RESULT_TEXT: usize = crate::models::MAX_RESULT_TEXT;
pub const MAX_RESULT_ITEMS: usize = 30;
/// A watch's read is not trimmed for speech, but it is capped.
pub const MAX_WATCH_BYTES: usize = 2_000_000;
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

/// One tool as the app lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    /// The app's, in full: for the operator (`GET /apps`).
    pub description: String,
    pub schema: Map<String, Value>,
    pub read_only: bool,
}

impl ToolInfo {
    pub fn new(name: &str, description: &str, schema: Map<String, Value>, read_only: bool) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            read_only,
        }
    }

    /// What the model sees. An app's description is often a page of instructions aimed at
    /// agents ("show this notice word for word"); the first sentence says what the tool
    /// does, which is all the model needs to pick it.
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

/// Results sized for a spoken turn: long strings cut, long lists cut, nested too.
pub(crate) fn trim(value: &Value) -> Value {
    match value {
        Value::String(s) => {
            if s.chars().count() <= MAX_RESULT_TEXT {
                value.clone()
            } else {
                Value::String(format!("{}…", prefix_chars(s, MAX_RESULT_TEXT)))
            }
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = items.iter().take(MAX_RESULT_ITEMS).map(trim).collect();
            if items.len() > MAX_RESULT_ITEMS {
                out.push(Value::String(format!(
                    "… {} more",
                    items.len() - MAX_RESULT_ITEMS
                )));
            }
            Value::Array(out)
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), trim(v))).collect()),
        other => other.clone(),
    }
}

/// The whole result as an object: structured content when the app gives it, else the
/// text. `trim` sizes it for the model.
pub(crate) fn result_to_dict(result: &CallToolResult) -> Result<Map<String, Value>, AppError> {
    let texts: Vec<&str> = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .filter(|t| !t.is_empty())
        .collect();
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
        return Ok(match structured {
            Value::Object(m) => m.clone(),
            other => Map::from_iter([("result".to_string(), other.clone())]),
        });
    }
    if texts.len() == 1 {
        return Ok(match serde_json::from_str::<Value>(texts[0]) {
            Ok(Value::Object(m)) => m,
            Ok(parsed) => Map::from_iter([("result".to_string(), parsed)]),
            Err(_) => Map::from_iter([("text".to_string(), Value::String(texts[0].to_string()))]),
        });
    }
    Ok(Map::from_iter([(
        "text".to_string(),
        Value::String(texts.join("\n")),
    )]))
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

/// `name` as the model API accepts it: other characters become `_`, and a name too long
/// keeps its start and a short hash of the whole, so two long names stay two.
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
    fn results_are_trimmed_for_a_spoken_turn() {
        let v = json!({"text": "x".repeat(2000), "items": (0..40).collect::<Vec<_>>()});
        let t = trim(&v);
        assert_eq!(
            t["text"].as_str().unwrap().chars().count(),
            MAX_RESULT_TEXT + 1
        );
        let items = t["items"].as_array().unwrap();
        assert_eq!(items.len(), MAX_RESULT_ITEMS + 1);
        assert_eq!(items.last().unwrap(), "… 10 more");
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
    }
}
