//! The conversation as Ghost keeps it, and its shape on each wire.
//!
//! One shape whichever protocol reaches the model: the system prompt, then user turns, the
//! assistant's replies with their tool calls, and each tool's result. In JSON it is
//! Pipecat's universal context, which is Chat Completions' message shape and what the
//! Python sessions kept, so a context stored there reads here (`Message::from_json`).
//!
//! The Messages API gets it converted as Pipecat's Anthropic adapter converted it: tool
//! calls as `tool_use` blocks, results as `tool_result` blocks in a user turn, consecutive
//! turns of one role merged into one, no empty content (`(empty)`), a trailing user turn
//! (".") for a model that takes no assistant prefill, and a cache marker on the last two
//! user turns. The system prompt's own marker is `with_system_breakpoint`. Chat Completions
//! gets the messages as they are, behind one system message.

use serde_json::{Map, Value, json};
use thiserror::Error;

/// What an empty text becomes on the Messages API, which refuses empty content.
pub const EMPTY: &str = "(empty)";

/// The cache marker (`cache_control`) the Messages API takes.
pub fn cache_control() -> Value {
    json!({"type": "ephemeral"})
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContextError {
    #[error("a message without a role")]
    NoRole,
    #[error("a message with the role '{0}'")]
    Role(String),
    #[error("a tool call whose arguments aren't a JSON object")]
    Arguments,
}

/// One call the model made: its id (the provider's, or `call_<index>` for a server that
/// sends none), the tool's name, and the arguments as the JSON object they parsed to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Map<String, Value>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    /// The arguments as Chat Completions carries them: one JSON string.
    pub fn arguments_json(&self) -> String {
        Value::Object(self.arguments.clone()).to_string()
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "type": "function",
            "function": {"name": self.name, "arguments": self.arguments_json()},
        })
    }

    fn from_json(v: &Value) -> Result<Self, ContextError> {
        let function = &v["function"];
        let arguments = match function.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::String(s)) => parse_arguments(s).ok_or(ContextError::Arguments)?,
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err(ContextError::Arguments),
        };
        Ok(Self {
            id: str_of(&v["id"]),
            name: str_of(&function["name"]),
            arguments,
        })
    }
}

/// A JSON object from the text a model streamed as a call's arguments: `{}` for nothing,
/// None when it isn't JSON or isn't an object.
pub(crate) fn parse_arguments(text: &str) -> Option<Map<String, Value>> {
    if text.trim().is_empty() {
        return Some(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// A message's text: a string, or the text parts of a list of content blocks.
fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i["text"].as_str().or_else(|| i.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// One turn of the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    User(String),
    /// What the model said and the calls it made in the same reply.
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    /// A tool's answer to one call, as the model is shown it (JSON text).
    Tool {
        tool_call_id: String,
        content: String,
    },
    /// A system note mid-conversation (a compaction summary, a transfer's handoff). The
    /// Messages API has no system turn there: it is sent as the operator's side.
    System(String),
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message::User(text.into())
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Message::Assistant {
            text: text.into(),
            tool_calls: Vec::new(),
        }
    }

    /// A reply with its tool calls.
    pub fn reply(text: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message::Assistant {
            text: text.into(),
            tool_calls,
        }
    }

    /// A tool's result for `tool_call_id`, as JSON text.
    pub fn result(tool_call_id: impl Into<String>, result: &Value) -> Self {
        Message::Tool {
            tool_call_id: tool_call_id.into(),
            content: result.to_string(),
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Message::System(text.into())
    }

    pub fn role(&self) -> &'static str {
        match self {
            Message::User(_) => "user",
            Message::Assistant { .. } => "assistant",
            Message::Tool { .. } => "tool",
            Message::System(_) => "system",
        }
    }

    /// The universal shape (Chat Completions'), as the Python sessions kept it.
    pub fn to_json(&self) -> Value {
        match self {
            Message::User(t) => json!({"role": "user", "content": t}),
            Message::System(t) => json!({"role": "system", "content": t}),
            Message::Assistant { text, tool_calls } => {
                let mut m = Map::new();
                m.insert("role".into(), json!("assistant"));
                if !text.is_empty() || tool_calls.is_empty() {
                    m.insert("content".into(), json!(text));
                }
                if !tool_calls.is_empty() {
                    m.insert(
                        "tool_calls".into(),
                        Value::Array(tool_calls.iter().map(ToolCall::to_json).collect()),
                    );
                }
                Value::Object(m)
            }
            Message::Tool {
                tool_call_id,
                content,
            } => json!({"role": "tool", "tool_call_id": tool_call_id, "content": content}),
        }
    }

    pub fn from_json(v: &Value) -> Result<Self, ContextError> {
        let role = v["role"].as_str().ok_or(ContextError::NoRole)?;
        let content = v.get("content").unwrap_or(&Value::Null);
        Ok(match role {
            "user" => Message::User(content_text(content)),
            "system" | "developer" => Message::System(content_text(content)),
            "assistant" => Message::Assistant {
                text: content_text(content),
                tool_calls: v["tool_calls"]
                    .as_array()
                    .map(|calls| calls.iter().map(ToolCall::from_json).collect())
                    .transpose()?
                    .unwrap_or_default(),
            },
            "tool" => Message::Tool {
                tool_call_id: str_of(&v["tool_call_id"]),
                content: match content {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                },
            },
            other => return Err(ContextError::Role(other.to_string())),
        })
    }
}

/// The conversation: the system prompt and the turns so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub system: String,
    pub messages: Vec<Message>,
}

/// The Messages API's shape of a context: `system` (a string; `with_system_breakpoint`
/// makes it a block with a cache marker) and `messages`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnthropicContext {
    pub system: Value,
    pub messages: Vec<Value>,
}

impl Context {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) -> &mut Self {
        self.messages.push(message);
        self
    }

    /// With a user turn appended.
    pub fn user(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::user(text));
        self
    }

    /// Append what the model answered: its text and the calls it made.
    pub fn push_reply(&mut self, text: impl Into<String>, tool_calls: Vec<ToolCall>) {
        self.messages.push(Message::reply(text, tool_calls));
    }

    /// Append a tool's result for one call.
    pub fn push_result(&mut self, tool_call_id: impl Into<String>, result: &Value) {
        self.messages.push(Message::result(tool_call_id, result));
    }

    /// The turns in the universal shape.
    pub fn to_json(&self) -> Vec<Value> {
        self.messages.iter().map(Message::to_json).collect()
    }

    /// A context from turns in the universal shape. An initial system message becomes
    /// the system prompt when `system` is empty, as Pipecat's adapters took it.
    pub fn from_json(system: impl Into<String>, messages: &[Value]) -> Result<Self, ContextError> {
        let mut out = Self::new(system);
        for (i, v) in messages.iter().enumerate() {
            let m = Message::from_json(v)?;
            if i == 0
                && out.system.is_empty()
                && let Message::System(text) = &m
            {
                out.system = text.clone();
                continue;
            }
            out.messages.push(m);
        }
        Ok(out)
    }

    /// Chat Completions' messages: the system prompt first, then the turns as they are.
    pub fn openai(&self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        if !self.system.is_empty() {
            out.push(json!({"role": "system", "content": self.system}));
        }
        out.extend(self.to_json());
        out
    }

    /// The Messages API's messages (Pipecat's Anthropic adapter, step for step). `cache`
    /// marks the last two user turns; `ensure_last_user` appends a "." user turn after a
    /// trailing assistant turn, for a model without assistant prefill.
    pub fn anthropic(&self, cache: bool, ensure_last_user: bool) -> AnthropicContext {
        let mut messages: Vec<Value> = Vec::with_capacity(self.messages.len());
        for m in &self.messages {
            let converted = anthropic_message(m);
            match messages.last_mut() {
                // Merge consecutive messages with the same role.
                Some(last) if last["role"] == converted["role"] => {
                    let mut blocks = blocks(last["content"].take());
                    blocks.extend(blocks_of(converted["content"].clone()));
                    last["content"] = Value::Array(blocks);
                }
                _ => messages.push(converted),
            }
        }
        if ensure_last_user && messages.last().is_some_and(|m| m["role"] == "assistant") {
            messages.push(json!({"role": "user", "content": [{"type": "text", "text": "."}]}));
        }
        // Avoid empty content in messages.
        for m in &mut messages {
            match &mut m["content"] {
                Value::String(s) if s.is_empty() => *s = EMPTY.into(),
                Value::Array(items) if items.is_empty() => {
                    items.push(json!({"type": "text", "text": EMPTY}));
                }
                _ => {}
            }
        }
        if cache {
            with_cache_control_markers(&mut messages);
        }
        AnthropicContext {
            system: json!(self.system),
            messages,
        }
    }
}

fn anthropic_message(m: &Message) -> Value {
    match m {
        // Anthropic doesn't support system input messages mid-conversation.
        Message::User(t) | Message::System(t) => json!({"role": "user", "content": t}),
        Message::Assistant { text, tool_calls } if tool_calls.is_empty() => {
            json!({"role": "assistant", "content": text})
        }
        Message::Assistant { text, tool_calls } => {
            let mut content = Vec::with_capacity(tool_calls.len() + 1);
            if !text.is_empty() {
                content.push(json!({"type": "text", "text": text}));
            }
            for c in tool_calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": c.id,
                    "name": c.name,
                    "input": c.arguments,
                }));
            }
            json!({"role": "assistant", "content": content})
        }
        Message::Tool {
            tool_call_id,
            content,
        } => json!({
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": tool_call_id, "content": content}],
        }),
    }
}

/// Content as a list of blocks: a string becomes one text block.
fn blocks(content: Value) -> Vec<Value> {
    match content {
        Value::Array(items) => items,
        Value::String(s) => {
            vec![json!({"type": "text", "text": if s.is_empty() { EMPTY.into() } else { s }})]
        }
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn blocks_of(content: Value) -> Vec<Value> {
    blocks(content)
}

/// Cache markers on the most recent two user messages: the last one tells the API to cache
/// the prompt up to there, the one before to read what the previous turn cached.
fn with_cache_control_markers(messages: &mut [Value]) {
    let mut marked = 0;
    for m in messages.iter_mut().rev() {
        if m["role"] != "user" {
            continue;
        }
        let mut items = blocks(m["content"].take());
        if let Some(last) = items.last_mut() {
            last["cache_control"] = cache_control();
        }
        m["content"] = Value::Array(items);
        marked += 1;
        if marked == 2 {
            break;
        }
    }
}

/// Turn a string system prompt into one text block carrying a cache marker. Other shapes
/// (empty, already blocks) are left alone.
pub fn with_system_breakpoint(params: &mut Map<String, Value>) {
    if let Some(Value::String(system)) = params.get("system")
        && !system.is_empty()
    {
        let block = json!([{"type": "text", "text": system, "cache_control": cache_control()}]);
        params.insert("system".into(), block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Map<String, Value> {
        parse_arguments(s).unwrap()
    }

    #[test]
    fn the_universal_shape_round_trips() {
        let mut ctx = Context::new("You are Ghost.").user("What's open in ghost?");
        ctx.push_reply(
            "",
            vec![ToolCall::new(
                "call_1",
                "github_list_issues",
                args(r#"{"repo": "ghost"}"#),
            )],
        );
        ctx.push_result("call_1", &json!({"count": 3}));
        ctx.push_reply("Three.", vec![]);
        let stored = ctx.to_json();
        assert_eq!(
            stored[1],
            json!({"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "github_list_issues", "arguments": "{\"repo\":\"ghost\"}"}}]})
        );
        assert_eq!(
            stored[2],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "{\"count\":3}"})
        );
        assert_eq!(stored[3], json!({"role": "assistant", "content": "Three."}));
        let back = Context::from_json("You are Ghost.", &stored).unwrap();
        assert_eq!(back, ctx);
        // An initial system message is the system prompt.
        let mut with_system = vec![json!({"role": "system", "content": "Sys."})];
        with_system.extend(stored);
        assert_eq!(Context::from_json("", &with_system).unwrap().system, "Sys.");
        assert_eq!(
            Message::from_json(&json!({"role": "weird"})),
            Err(ContextError::Role("weird".into()))
        );
    }

    #[test]
    fn the_messages_api_shape_merges_roles_and_fills_empties() {
        let mut ctx = Context::new("S").user("Q");
        ctx.push_reply(
            "Checking.",
            vec![
                ToolCall::new("t1", "a", Map::new()),
                ToolCall::new("t2", "b", args(r#"{"n": 1}"#)),
            ],
        );
        ctx.push_result("t1", &json!({"ok": true}));
        ctx.push_result("t2", &json!({"ok": true}));
        ctx.push(Message::system("note"));
        ctx.push_reply("", vec![]);
        let a = ctx.anthropic(false, true);
        assert_eq!(a.system, json!("S"));
        assert_eq!(a.messages[0], json!({"role": "user", "content": "Q"}));
        assert_eq!(
            a.messages[1]["content"],
            json!([
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "t1", "name": "a", "input": {}},
                {"type": "tool_use", "id": "t2", "name": "b", "input": {"n": 1}},
            ])
        );
        // Two results and a system note: one user turn with three blocks.
        let user = &a.messages[2];
        assert_eq!(user["role"], "user");
        let blocks = user["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "t1");
        assert_eq!(blocks[2], json!({"type": "text", "text": "note"}));
        assert_eq!(
            a.messages[3],
            json!({"role": "assistant", "content": EMPTY})
        );
        // A trailing assistant turn gets a "." user turn when asked for.
        assert_eq!(a.messages[4]["role"], "user");
        assert_eq!(ctx.anthropic(false, false).messages.len(), 4);
    }

    #[test]
    fn cache_markers_sit_on_the_last_two_user_turns() {
        let ctx = Context::new("S").user("one").user("two").user("three");
        // Three consecutive user turns merge into one, so one marker.
        let a = ctx.anthropic(true, true);
        assert_eq!(a.messages.len(), 1);
        assert_eq!(
            a.messages[0]["content"][2]["cache_control"],
            cache_control()
        );
        assert!(a.messages[0]["content"][0].get("cache_control").is_none());
        let mut ctx = Context::new("S").user("one");
        ctx.push_reply("r", vec![]);
        ctx.push(Message::user("two"));
        ctx.push_reply("r", vec![]);
        ctx.push(Message::user("three"));
        let a = ctx.anthropic(true, true);
        let marked: Vec<usize> = a
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m["content"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|x| x.get("cache_control").is_some()))
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(marked, vec![2, 4]);
        assert_eq!(a.messages[0], json!({"role": "user", "content": "one"}));
    }

    #[test]
    fn arguments_parse_to_an_object_or_nothing() {
        assert_eq!(parse_arguments(""), Some(Map::new()));
        assert_eq!(parse_arguments("  "), Some(Map::new()));
        assert_eq!(parse_arguments(r#"{"n": 8}"#).unwrap()["n"], 8);
        assert_eq!(parse_arguments(r#"{"n": "#), None);
        assert_eq!(parse_arguments("[1]"), None);
    }
}
