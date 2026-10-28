//! What the model tests share: a server that streams recorded replies in turn, a target
//! pointed at it, and builders for the streams the Python tests wrote inline.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use futures::{Stream, StreamExt};
use ghost_core::connection::{Protocol, Target, reasoning};
use ghost_llm::{Event, LlmError, Model, ToolCall};
use serde_json::{Map, Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use zeroize::Zeroizing;

/// A key that must never show.
pub const KEY: &str = "sk-ant-0123456789abcdef";

/// Streams `bodies` to successive requests, in order; a request past the last gets an
/// empty stream.
pub struct Replies {
    bodies: Vec<String>,
    served: AtomicUsize,
}

impl Replies {
    pub fn new<I, S>(bodies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            bodies: bodies.into_iter().map(Into::into).collect(),
            served: AtomicUsize::new(0),
        }
    }
}

impl Respond for Replies {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let i = self.served.fetch_add(1, Ordering::SeqCst);
        let body = self.bodies.get(i).cloned().unwrap_or_default();
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }
}

/// A server answering every POST with the replies in turn.
pub async fn serving<I, S>(bodies: I) -> MockServer
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Replies::new(bodies))
        .mount(&server)
        .await;
    server
}

/// A server answering every POST with `status` and `body` (JSON or text).
pub async fn refusing(status: u16, body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    server
}

pub fn target(protocol: Protocol, base_url: &str, model: &str) -> Target {
    Target {
        provider: match protocol {
            Protocol::Anthropic => "anthropic".into(),
            Protocol::Openai => "ollama".into(),
        },
        protocol,
        base_url: base_url.into(),
        model: model.into(),
        api_key: Zeroizing::new(KEY.into()),
        headers: Vec::new(),
        timeout_secs: Some(5.0),
        cap_param: "max_tokens",
        reasoning: reasoning(model),
    }
}

pub fn anthropic_target(base_url: &str) -> Target {
    target(Protocol::Anthropic, base_url, "claude-sonnet-5-5")
}

pub fn openai_target(base_url: &str) -> Target {
    target(Protocol::Openai, base_url, "test-model")
}

pub fn model(target: Target) -> Model {
    Model::new(target).expect("a client")
}

/// A recorded stream from `tests/fixtures/<protocol>/<name>.sse`.
pub fn fixture(protocol: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{protocol}/{name}.sse",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Every event of a stream, which must not fail.
pub async fn events(stream: impl Stream<Item = Result<Event, LlmError>>) -> Vec<Event> {
    stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|e| e.expect("an event"))
        .collect()
}

/// The first item of a stream, expected to be an error.
pub async fn first_error(stream: impl Stream<Item = Result<Event, LlmError>>) -> LlmError {
    let items: Vec<_> = stream.collect().await;
    items
        .into_iter()
        .find_map(Result::err)
        .expect("an error in the stream")
}

/// The JSON bodies the server received, in order.
pub async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("recorded requests")
        .iter()
        .map(|r| serde_json::from_slice(&r.body).expect("a JSON body"))
        .collect()
}

pub async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.expect("recorded requests")
}

/// The tool calls delivered: (id, name, arguments).
pub fn tool_calls(events: &[Event]) -> Vec<(String, String, Map<String, Value>)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ToolCall(ToolCall {
                id,
                name,
                arguments,
            }) => Some((id.clone(), name.clone(), arguments.clone())),
            _ => None,
        })
        .collect()
}

pub fn text(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// Each event's kind, in order.
pub fn kinds(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            Event::Text(_) => "Text",
            Event::Thought(_) => "Thought",
            Event::ToolCall(_) => "ToolCall",
            Event::Usage(_) => "Usage",
            Event::Finished(_) => "Finished",
            Event::CutOff { .. } => "CutOff",
            Event::Redo => "Redo",
            Event::Say(_) => "Say",
        })
        .collect()
}

pub fn cut_offs(events: &[Event]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CutOff { tool_calls } => Some(*tool_calls),
            _ => None,
        })
        .collect()
}

pub fn said(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Say(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

pub fn index_of(kinds: &[&str], kind: &str) -> usize {
    kinds
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_else(|| panic!("no {kind} in {kinds:?}"))
}

pub fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// A streamed Messages API reply, as the Python tests built one: its content blocks,
/// then the stop reason.
pub mod anthropic {
    use super::*;

    pub enum Block {
        Text(String),
        Tool {
            name: String,
            parts: Vec<String>,
            id: String,
        },
    }

    pub fn text(s: &str) -> Block {
        Block::Text(s.into())
    }

    /// A `tool_use` block whose arguments stream in `parts`.
    pub fn tool(name: &str, parts: &[&str], id: &str) -> Block {
        Block::Tool {
            name: name.into(),
            parts: parts.iter().map(|p| (*p).to_string()).collect(),
            id: id.into(),
        }
    }

    pub fn events(blocks: &[Block], stop: &str) -> String {
        let mut out = sse(
            "message_start",
            &json!({"type": "message_start", "message": {"id": "msg_t", "type": "message",
                "role": "assistant", "model": "claude-sonnet-5-5", "content": [],
                "usage": {"input_tokens": 8, "output_tokens": 1}}}),
        );
        for (i, block) in blocks.iter().enumerate() {
            match block {
                Block::Text(s) => {
                    out += &sse(
                        "content_block_start",
                        &json!({"type": "content_block_start", "index": i,
                            "content_block": {"type": "text", "text": ""}}),
                    );
                    out += &sse(
                        "content_block_delta",
                        &json!({"type": "content_block_delta", "index": i,
                            "delta": {"type": "text_delta", "text": s}}),
                    );
                }
                Block::Tool { name, parts, id } => {
                    let id = if id.is_empty() {
                        format!("toolu_{name}_{i}")
                    } else {
                        id.clone()
                    };
                    out += &sse(
                        "content_block_start",
                        &json!({"type": "content_block_start", "index": i,
                            "content_block": {"type": "tool_use", "id": id, "name": name,
                            "input": {}}}),
                    );
                    for p in parts {
                        out += &sse(
                            "content_block_delta",
                            &json!({"type": "content_block_delta", "index": i,
                                "delta": {"type": "input_json_delta", "partial_json": p}}),
                        );
                    }
                }
            }
            out += &sse(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": i}),
            );
        }
        out += &sse(
            "message_delta",
            &json!({"type": "message_delta", "delta": {"stop_reason": stop,
                "stop_sequence": null}, "usage": {"output_tokens": 400}}),
        );
        out += &sse("message_stop", &json!({"type": "message_stop"}));
        out
    }
}

/// Chat Completions chunks, as the Python tests built them.
pub mod openai {
    use super::*;

    pub fn chunk(content: Option<&str>, calls: Option<Vec<Value>>, finish: Option<&str>) -> String {
        let mut delta = Map::new();
        if let Some(c) = content {
            delta.insert("content".into(), json!(c));
        }
        if let Some(calls) = calls {
            delta.insert("tool_calls".into(), Value::Array(calls));
        }
        let v = json!({
            "id": "c1", "object": "chat.completion.chunk", "created": 0, "model": "test-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        });
        format!("data: {v}\n\n")
    }

    pub fn call(index: usize, id: Option<&str>, name: Option<&str>, args: Option<&str>) -> Value {
        let mut function = Map::new();
        if let Some(n) = name {
            function.insert("name".into(), json!(n));
        }
        if let Some(a) = args {
            function.insert("arguments".into(), json!(a));
        }
        let mut out = Map::new();
        out.insert("index".into(), json!(index));
        out.insert("function".into(), Value::Object(function));
        if let Some(id) = id {
            out.insert("id".into(), json!(id));
            out.insert("type".into(), json!("function"));
        }
        Value::Object(out)
    }

    pub fn done() -> String {
        "data: [DONE]\n\n".into()
    }

    /// The chunks, then `[DONE]`.
    pub fn reply(chunks: &[String]) -> String {
        chunks.concat() + &done()
    }

    /// The car on 2026-10-24: two comments whole, the third cut mid-arguments.
    pub fn cut_reply() -> String {
        reply(&[
            chunk(
                None,
                Some(vec![call(
                    0,
                    Some("t1"),
                    Some("change"),
                    Some(r#"{"op": "comment", "n": 390}"#),
                )]),
                None,
            ),
            chunk(
                None,
                Some(vec![call(
                    1,
                    Some("t2"),
                    Some("change"),
                    Some(r#"{"op": "comment", "n": 389}"#),
                )]),
                None,
            ),
            chunk(
                None,
                Some(vec![call(
                    2,
                    Some("t3"),
                    Some("change"),
                    Some(r#"{"op": "comm"#),
                )]),
                None,
            ),
            chunk(None, None, Some("length")),
        ])
    }
}
