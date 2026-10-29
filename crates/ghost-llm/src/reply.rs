//! What one streamed reply contained, watched event by event: its text, its complete tool
//! calls in order, whether the cap fell inside one, why it stopped, and what it cost.
//!
//! The Messages API streams `tool_use` blocks, then `message_delta` with the stop reason.
//! Chat Completions streams tool calls by `index` (one chunk may carry several, whole),
//! and the last choice says `finish_reason`; `length` is the cap. A stream that ends
//! inside a call's arguments is a cut too, whatever the reason said.

use std::collections::{BTreeMap, VecDeque};

use ghost_core::connection::Protocol;
use serde_json::Value;
use tracing::warn;

use crate::context::{ToolCall, parse_arguments};
use crate::model::{Event, LlmError, Usage};
use crate::sse::SseEvent;

#[derive(Debug, Default)]
struct ToolUse {
    id: String,
    name: String,
    json: String,
}

impl ToolUse {
    fn parse(&self) -> Option<ToolCall> {
        let arguments = parse_arguments(&self.json)?;
        Some(ToolCall::new(self.id.clone(), self.name.clone(), arguments))
    }
}

/// One reply, once its stream has ended.
#[derive(Debug, Default)]
pub(crate) struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub cut_call: bool,
    pub stop_reason: Option<String>,
    pub usage: Option<Usage>,
    pub model: String,
    cut_reasons: &'static [&'static str],
}

impl Reply {
    pub fn cut_off(&self) -> bool {
        self.stop_reason
            .as_deref()
            .is_some_and(|r| self.cut_reasons.contains(&r))
    }

    /// Tool calls a cut reply was making, the cut one included.
    pub fn lost(&self) -> usize {
        self.calls.len() + usize::from(self.cut_call)
    }
}

/// Watches a reply's events as they stream, emitting the live ones (text, thought) and
/// keeping the rest for `finish`.
#[derive(Debug)]
pub(crate) struct Watch {
    protocol: Protocol,
    reply: Reply,
    /// The Messages API's open `tool_use` block.
    open: Option<ToolUse>,
    /// Chat Completions' calls by index.
    by_index: BTreeMap<usize, ToolUse>,
    done: bool,
    /// A content event has been seen: the probe's first token.
    pub saw_content: bool,
}

impl Watch {
    pub fn new(protocol: Protocol) -> Self {
        let cut_reasons: &'static [&'static str] = match protocol {
            Protocol::Anthropic => &["max_tokens"],
            Protocol::Openai => &["length", "max_tokens"],
        };
        Self {
            protocol,
            reply: Reply {
                cut_reasons,
                ..Reply::default()
            },
            open: None,
            by_index: BTreeMap::new(),
            done: false,
            saw_content: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.reply.text
    }

    pub fn model(&self) -> &str {
        &self.reply.model
    }

    /// Note one event. Live events go to `out`; an error the server streamed is returned.
    pub fn note(&mut self, ev: &SseEvent, out: &mut VecDeque<Event>) -> Result<(), LlmError> {
        if self.done {
            return Ok(());
        }
        match self.protocol {
            Protocol::Anthropic => self.note_anthropic(ev, out),
            Protocol::Openai => self.note_openai(ev, out),
        }
    }

    fn data(ev: &SseEvent) -> Result<Value, LlmError> {
        serde_json::from_str(&ev.data)
            .map_err(|_| LlmError::Stream("an event that isn't JSON".into()))
    }

    fn note_anthropic(&mut self, ev: &SseEvent, out: &mut VecDeque<Event>) -> Result<(), LlmError> {
        let v = Self::data(ev)?;
        let kind = v["type"].as_str().unwrap_or(ev.event.as_str());
        match kind {
            "message_start" => {
                let message = &v["message"];
                if let Some(m) = message["model"].as_str() {
                    self.reply.model = m.to_string();
                }
                if let Some(u) = message.get("usage") {
                    anthropic_usage(u, self.reply.usage.get_or_insert_with(Usage::default));
                }
            }
            "content_block_start" => {
                self.saw_content = true;
                let block = &v["content_block"];
                if block["type"] == "tool_use" {
                    self.open = Some(ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        json: String::new(),
                    });
                }
            }
            "content_block_delta" => {
                self.saw_content = true;
                let delta = &v["delta"];
                if let Some(t) = delta["text"].as_str() {
                    self.reply.text.push_str(t);
                    out.push_back(Event::Text(t.to_string()));
                } else if let Some(p) = delta["partial_json"].as_str() {
                    if let Some(open) = &mut self.open {
                        open.json.push_str(p);
                    }
                } else if let Some(t) = delta["thinking"].as_str() {
                    out.push_back(Event::Thought(t.to_string()));
                }
            }
            "content_block_stop" => self.close_anthropic(),
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.reply.stop_reason = Some(r.to_string());
                }
                if let Some(u) = v.get("usage") {
                    anthropic_usage(u, self.reply.usage.get_or_insert_with(Usage::default));
                }
                if self.open.take().is_some() {
                    self.reply.cut_call = true; // the message ended inside a block
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                return Err(LlmError::Api {
                    kind: v["error"]["type"].as_str().unwrap_or("error").to_string(),
                    message: v["error"]["message"].as_str().unwrap_or("").to_string(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn close_anthropic(&mut self) {
        let Some(call) = self.open.take() else {
            return;
        };
        match call.parse() {
            Some(c) => self.reply.calls.push(c),
            None => self.reply.cut_call = true, // the stream ended inside the arguments
        }
    }

    fn note_openai(&mut self, ev: &SseEvent, out: &mut VecDeque<Event>) -> Result<(), LlmError> {
        if ev.data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let v = Self::data(ev)?;
        if let Some(err) = v.get("error") {
            return Err(LlmError::Api {
                kind: err["type"]
                    .as_str()
                    .or_else(|| err["code"].as_str())
                    .unwrap_or("error")
                    .to_string(),
                message: err["message"].as_str().unwrap_or("").to_string(),
            });
        }
        if let Some(m) = v["model"].as_str().filter(|m| !m.is_empty()) {
            self.reply.model = m.to_string();
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.reply.usage = Some(openai_usage(u)); // the latest snapshot wins
        }
        for choice in v["choices"].as_array().into_iter().flatten() {
            if !matches!(choice["index"].as_u64(), None | Some(0)) {
                continue;
            }
            let delta = &choice["delta"];
            if let Some(t) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                self.saw_content = true;
                self.reply.text.push_str(t);
                out.push_back(Event::Text(t.to_string()));
            }
            for tc in delta["tool_calls"].as_array().into_iter().flatten() {
                self.saw_content = true;
                let index = tc["index"]
                    .as_u64()
                    .map_or(self.by_index.len(), |i| i as usize);
                let call = self.by_index.entry(index).or_default();
                if let Some(id) = tc["id"].as_str().filter(|id| !id.is_empty())
                    && call.id.is_empty()
                {
                    call.id = id.to_string();
                }
                let function = &tc["function"];
                // The name comes once; a server that repeats it in every chunk is not
                // allowed to turn `list_issues` into `list_issueslist_issues`.
                if let Some(name) = function["name"].as_str().filter(|n| !n.is_empty())
                    && name != call.name
                {
                    call.name.push_str(name);
                }
                if let Some(args) = function["arguments"].as_str().filter(|a| !a.is_empty()) {
                    call.json.push_str(args);
                }
            }
            if let Some(r) = choice["finish_reason"].as_str() {
                self.reply.stop_reason = Some(r.to_string());
            }
        }
        Ok(())
    }

    /// The stream has ended: the reply, with every call parsed and the cut normalised.
    pub fn finish(mut self) -> Reply {
        match self.protocol {
            Protocol::Anthropic => {
                if self.open.take().is_some() {
                    self.reply.cut_call = true; // the stream ended inside a block
                }
            }
            Protocol::Openai => {
                for (index, mut call) in std::mem::take(&mut self.by_index) {
                    if call.id.is_empty() {
                        call.id = format!("call_{index}");
                    }
                    match (call.name.is_empty(), call.parse()) {
                        (false, Some(c)) => self.reply.calls.push(c),
                        _ => self.reply.cut_call = true,
                    }
                }
            }
        }
        if self.reply.cut_call && self.reply.stop_reason.is_none() {
            // The stream ended inside a call's arguments.
            self.reply.stop_reason = Some(self.reply.cut_reasons[0].to_string());
        } else if self.reply.cut_call && !self.reply.cut_off() {
            warn!("model: a tool call's arguments weren't JSON; it was not run");
            self.reply.cut_call = false;
        }
        self.reply
    }
}

fn anthropic_usage(u: &Value, into: &mut Usage) {
    if let Some(n) = u["input_tokens"].as_u64() {
        into.input_tokens = n;
    }
    if let Some(n) = u["output_tokens"].as_u64() {
        into.output_tokens = n; // cumulative: the last report is the count
    }
    if let Some(n) = u["cache_read_input_tokens"].as_u64() {
        into.cache_read_tokens = Some(n);
    }
    if let Some(n) = u["cache_creation_input_tokens"].as_u64() {
        into.cache_write_tokens = Some(n);
    }
}

fn openai_usage(u: &Value) -> Usage {
    Usage {
        input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: u["prompt_tokens_details"]["cached_tokens"].as_u64(),
        cache_write_tokens: u["prompt_tokens_details"]["cache_write_tokens"].as_u64(),
        reasoning_tokens: u["completion_tokens_details"]["reasoning_tokens"].as_u64(),
    }
}
