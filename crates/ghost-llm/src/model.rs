//! The interface: one model, reached over the protocol its connection speaks.
//!
//! `Model::stream` is a session's turn: the reply's events as they come, every tool call
//! of the reply once it is whole, and the recovery when a reply hits the output cap. A
//! reply cut inside its tool calls delivers none of them (half a batch is worse than none):
//! the turn is flagged (`Event::CutOff`), the model is told, as the operator's side of the
//! conversation, to do it again in smaller steps (`CUT_OFF_NOTE`, `Event::Redo`), and the
//! redo's events follow in the same stream. A redo cut off too is said aloud instead
//! (`CUT_OFF_LINE`, `Event::Say`). A long answer cut short is only flagged: the operator
//! heard what there was of it.
//!
//! The key goes in a header and nowhere else: it is not in a `Debug`, not in an error, and
//! an error's detail (the first line of a refused request's body) is redacted before it is
//! kept.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use ghost_core::connection::{ANTHROPIC_VERSION, Protocol, Target};
use ghost_core::redact::{MIN_REDACT_CHARS, redact_text};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};
use tracing::warn;

use crate::context::{Context, Message, ToolCall, with_system_breakpoint};
use crate::reply::{Reply, Watch};
use crate::sse::Sse;
use crate::tools::Tools;

/// The output cap, tool calls included (`ghost_voice.config.LLM_MAX_TOKENS`): room for a
/// batch of tool calls. Don't lower it for brevity; the prompt keeps replies short.
pub const LLM_MAX_TOKENS: u32 = 2000;
/// Sonnet 5.5's thinking-off setting (the Messages API only).
pub const DEFAULT_THINKING: &str = "between_tools";
/// Interleaved thinking, needed to allow thinking between sequences of tool calls when
/// extended thinking is on; Pipecat sent it on every request.
pub const ANTHROPIC_BETA: &str = "interleaved-thinking-2025-05-14";
/// Told to the model (as the operator's side of the conversation; the Messages API has no
/// system turn mid-conversation) when its reply was cut off with tool calls in it.
pub const CUT_OFF_NOTE: &str = "Your last reply was cut off at the output limit before it \
                                finished, and nothing in it was done. Do it again in smaller \
                                steps: one or two tool calls per reply, and keep any text short.";
/// Said when the redo was cut off too.
pub const CUT_OFF_LINE: &str = "That was more than I can do in one go. Ask me for part of it.";
/// How much of a refused request's reason an error keeps.
const DETAIL_CHARS: usize = 160;

/// Models known to take an assistant prefill; the rest get a "." user turn when the
/// context ends with the assistant (Pipecat's list).
const PREFILL_SUPPORTED: [&str; 8] = [
    "claude-2",
    "claude-instant",
    "claude-3",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
];

/// Whether `model` takes a request that ends with an assistant message.
pub fn prefill_supported(model: &str) -> bool {
    PREFILL_SUPPORTED.iter().any(|p| model.starts_with(p))
}

/// A session's settings for a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Visible output tokens; a reasoning model gets headroom on top (`Target::output_cap`).
    pub max_tokens: u32,
    /// The Messages API's `thinking.type`; None sends none.
    pub thinking: Option<String>,
    /// The cache marker on the system prompt (the harness turns it off to measure).
    pub cache_system: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_tokens: LLM_MAX_TOKENS,
            thinking: Some(DEFAULT_THINKING.into()),
            cache_system: true,
        }
    }
}

/// What a reply cost, as the provider reported it. Output tokens are the final count, not a
/// sum of the stream's reports (Pipecat added `message_start`'s to `message_delta`'s).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

/// Why a reply stopped, in one vocabulary for both protocols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishReason {
    /// `end_turn` / `stop`.
    End,
    /// `tool_use` / `tool_calls`.
    ToolUse,
    /// `max_tokens` / `length`: the cap.
    MaxTokens,
    Other(String),
}

impl FinishReason {
    pub fn parse(reason: &str) -> Self {
        match reason {
            "end_turn" | "stop" => FinishReason::End,
            "tool_use" | "tool_calls" => FinishReason::ToolUse,
            "max_tokens" | "length" => FinishReason::MaxTokens,
            other => FinishReason::Other(other.to_string()),
        }
    }
}

/// What a streamed turn yields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A piece of the reply's text, as it streams.
    Text(String),
    /// A piece of the model's thinking, when the API shows it.
    Thought(String),
    /// One complete tool call. Every call of a reply is delivered, after the reply's text
    /// and only when the reply is whole; none when it was cut.
    ToolCall(ToolCall),
    Usage(Usage),
    /// The reply ended; the last event of a reply that wasn't cut.
    Finished(FinishReason),
    /// The reply stopped at the output cap. `tool_calls` is how many it was making, the
    /// cut one included; none of them were delivered. The turn is flagged `cut_off`.
    CutOff {
        tool_calls: usize,
    },
    /// `CUT_OFF_NOTE` was appended as a user turn (after the cut reply's text as the
    /// assistant's) and the model asked again; the events that follow are the redo's.
    Redo,
    /// A line for Ghost to say (`CUT_OFF_LINE`): the redo was cut off too.
    Say(String),
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum LlmError {
    /// The request got no answer. The failure's kind only, never its text.
    #[error("{kind}: can't reach {host}")]
    Unreachable { host: String, kind: &'static str },
    /// A refused request: the status and the first line of the reason, redacted.
    #[error("HTTP {status}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") })]
    Status { status: u16, detail: String },
    /// An error the server streamed in the reply.
    #[error("{kind}: {message}")]
    Api { kind: String, message: String },
    /// The stream broke or carried something that isn't an event.
    #[error("the stream broke: {0}")]
    Stream(String),
    /// A header of the connection can't be sent.
    #[error("{0}")]
    Headers(String),
    /// The HTTP client couldn't be built.
    #[error("the client couldn't be built: {0}")]
    Client(String),
}

/// One tiny request, timed the way `make perf-llm` times it: from just before the
/// streaming call to its first content event (a text or tool block on the Messages API, a
/// chunk with text or a tool call on Chat Completions).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    /// Seconds to the first token.
    pub ttft: Option<f64>,
    pub total: Option<f64>,
    pub text: String,
    /// As the provider answered, when it says.
    pub model: String,
    pub error: Option<String>,
}

type Body = Pin<Box<dyn Stream<Item = reqwest::Result<Vec<u8>>> + Send>>;

/// Ghost's model, whichever protocol reaches it: a connection's target and the HTTP
/// client (its connection pool) for it. Cheap to clone; a clone shares the pool.
#[derive(Clone)]
pub struct Model {
    http: reqwest::Client,
    target: Arc<Target>,
}

impl fmt::Debug for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Target's Debug leaves the key and the headers out.
        f.debug_struct("Model")
            .field("target", &self.target)
            .finish()
    }
}

/// `scheme://host[:port]` of a URL, "it" when it has no host.
fn host(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "it".into();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        "it".into()
    } else {
        format!("{scheme}://{authority}")
    }
}

/// httpx's name for what went wrong, as the Python said it.
fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "ReadTimeout"
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_request() {
        "RequestError"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else {
        "HTTPError"
    }
}

fn header_name(k: &str) -> Result<HeaderName, LlmError> {
    HeaderName::from_bytes(k.as_bytes())
        .map_err(|_| LlmError::Headers(format!("'{k}' is not a header name")))
}

fn header_value(k: &str, v: &str, sensitive: bool) -> Result<HeaderValue, LlmError> {
    let mut value = HeaderValue::from_str(v)
        .map_err(|_| LlmError::Headers(format!("{k}'s value can't be sent as a header")))?;
    value.set_sensitive(sensitive);
    Ok(value)
}

impl Model {
    /// A client of its own for `target` (a connection being tried out). Sessions share one
    /// through `shared`.
    pub fn new(target: Target) -> Result<Self, LlmError> {
        let mut builder = reqwest::Client::builder().user_agent("ghost");
        if let Some(secs) = target.timeout_secs.filter(|s| *s > 0.0) {
            let t = Duration::from_secs_f64(secs);
            builder = builder.connect_timeout(t).read_timeout(t);
        }
        let http = builder
            .build()
            .map_err(|e| LlmError::Client(error_kind(&e).into()))?;
        Ok(Self::with_client(http, target))
    }

    pub fn with_client(http: reqwest::Client, target: Target) -> Self {
        Self {
            http,
            target: Arc::new(target),
        }
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn protocol(&self) -> Protocol {
        self.target.protocol
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.target.base_url.trim_end_matches('/'))
    }

    /// The request's headers: the operator's as written, then Ghost's. The key as
    /// `connection::models_request` sends it: `x-api-key` on the Messages API (and a
    /// bearer for a provider that isn't Anthropic itself), a bearer on Chat Completions.
    fn headers(&self) -> Result<HeaderMap, LlmError> {
        let mut out = HeaderMap::new();
        out.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        for (k, v) in &self.target.headers {
            out.insert(header_name(k)?, header_value(k, v, false)?);
        }
        let key: &str = &self.target.api_key;
        match self.target.protocol {
            Protocol::Anthropic => {
                out.insert(
                    "anthropic-version",
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
                out.insert("anthropic-beta", HeaderValue::from_static(ANTHROPIC_BETA));
                if !key.is_empty() {
                    out.insert("x-api-key", header_value("the key", key, true)?);
                    if self.target.provider != "anthropic" {
                        out.insert(
                            "authorization",
                            header_value("the key", &format!("Bearer {key}"), true)?,
                        );
                    }
                }
            }
            Protocol::Openai => {
                if !key.is_empty() {
                    out.insert(
                        "authorization",
                        header_value("the key", &format!("Bearer {key}"), true)?,
                    );
                }
            }
        }
        Ok(out)
    }

    /// The protocol's shape of the tools.
    fn wire_tools(&self, tools: &Tools) -> Vec<Value> {
        match self.target.protocol {
            Protocol::Anthropic => tools.anthropic(),
            Protocol::Openai => tools.openai(),
        }
    }

    /// The request body for a session's turn, as `stream` sends it (`stream: true`).
    pub fn body(&self, context: &Context, tools: &Tools, options: &Options) -> Value {
        self.turn_body(context, &self.wire_tools(tools), options)
    }

    fn turn_body(&self, context: &Context, tools: &[Value], options: &Options) -> Value {
        let t = &*self.target;
        let mut p = Map::new();
        p.insert("model".into(), json!(t.model));
        p.insert("stream".into(), json!(true));
        match t.protocol {
            Protocol::Anthropic => {
                p.insert("max_tokens".into(), json!(options.max_tokens));
                if let Some(thinking) = &options.thinking {
                    p.insert("thinking".into(), json!({"type": thinking}));
                }
                let conv = context.anthropic(true, !prefill_supported(&t.model));
                if !context.system.is_empty() {
                    p.insert("system".into(), conv.system);
                }
                p.insert("messages".into(), Value::Array(conv.messages));
                if !tools.is_empty() {
                    p.insert("tools".into(), Value::Array(tools.to_vec()));
                }
                if options.cache_system {
                    with_system_breakpoint(&mut p);
                }
            }
            Protocol::Openai => {
                p.insert("stream_options".into(), json!({"include_usage": true}));
                p.insert("messages".into(), Value::Array(context.openai()));
                if !tools.is_empty() {
                    p.insert("tools".into(), Value::Array(tools.to_vec()));
                }
                p.extend(t.chat_params(options.max_tokens));
            }
        }
        Value::Object(p)
    }

    fn path(&self) -> &'static str {
        match self.target.protocol {
            Protocol::Anthropic => "/v1/messages",
            Protocol::Openai => "/chat/completions",
        }
    }

    /// Nothing from the vault, and not this connection's own key either, leaves in text.
    fn redact(&self, s: &str) -> String {
        let mut out = redact_text(s);
        let key: &str = &self.target.api_key;
        if key.len() >= MIN_REDACT_CHARS && out.contains(key) {
            out = out.replace(key, "[password]");
        }
        out
    }

    /// A refused request's reason: its JSON message or its first line, redacted, short.
    fn detail(&self, body: &[u8]) -> String {
        let text = String::from_utf8_lossy(body);
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .or_else(|| v["message"].as_str())
                    .or_else(|| v["error"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.lines().next().unwrap_or("").to_string());
        let message = self.redact(message.trim());
        message.chars().take(DETAIL_CHARS).collect()
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        let url = self.url(self.path());
        let resp = self
            .http
            .post(&url)
            .headers(self.headers()?)
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable {
                host: host(&url),
                kind: error_kind(&e),
            })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(LlmError::Status {
                status: status.as_u16(),
                detail: self.detail(&body),
            });
        }
        Ok(resp)
    }

    async fn open(&self, body: &Value) -> Result<Body, LlmError> {
        let resp = self.send(body).await?;
        Ok(Box::pin(resp.bytes_stream().map(|r| r.map(|b| b.to_vec()))))
    }

    /// A session's turn: the reply's events, every tool call once the reply is whole, the
    /// cap's recovery. The stream owns its context; the caller appends the reply's text
    /// and calls to its own as the events say.
    pub fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> impl Stream<Item = Result<Event, LlmError>> + Send + use<> {
        let turn = Turn {
            model: self.clone(),
            context,
            tools: self.wire_tools(tools),
            options: options.clone(),
            phase: Phase::Start,
            body: None,
            sse: Sse::default(),
            watch: Watch::new(self.target.protocol),
            queue: VecDeque::new(),
            redoing: false,
        };
        futures::stream::unfold(turn, |mut turn| async move {
            let item = turn.next().await?;
            Some((item, turn))
        })
    }

    /// One short answer, not streamed (a watch's model step).
    pub async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let t = &*self.target;
        let body = match t.protocol {
            Protocol::Anthropic => {
                let mut p = Map::new();
                p.insert("model".into(), json!(t.model));
                p.insert("max_tokens".into(), json!(max_tokens));
                if !system.is_empty() {
                    p.insert("system".into(), json!(system));
                }
                p.insert(
                    "messages".into(),
                    json!([{"role": "user", "content": user}]),
                );
                Value::Object(p)
            }
            Protocol::Openai => {
                let mut messages = Vec::new();
                if !system.is_empty() {
                    messages.push(json!({"role": "system", "content": system}));
                }
                messages.push(json!({"role": "user", "content": user}));
                let mut p = Map::new();
                p.insert("model".into(), json!(t.model));
                p.insert("messages".into(), Value::Array(messages));
                p.extend(t.chat_params(max_tokens));
                Value::Object(p)
            }
        };
        let resp = self.send(&body).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|_| LlmError::Stream("an answer that isn't JSON".into()))?;
        Ok(match t.protocol {
            Protocol::Anthropic => v["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default(),
            Protocol::Openai => v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        })
    }

    /// Stream one tiny request and time its first token. The probe reports, it doesn't
    /// fail: an error is in `Probe::error`, in a few words and never a header or a key.
    pub async fn probe(&self, prompt: &str, max_tokens: u32, thinking: Option<&str>) -> Probe {
        let t = &*self.target;
        let mut p = Map::new();
        p.insert("model".into(), json!(t.model));
        p.insert(
            "messages".into(),
            json!([{"role": "user", "content": prompt}]),
        );
        p.insert("stream".into(), json!(true));
        match t.protocol {
            Protocol::Anthropic => {
                p.insert("max_tokens".into(), json!(max_tokens));
                if let Some(thinking) = thinking {
                    p.insert("thinking".into(), json!({"type": thinking}));
                }
            }
            Protocol::Openai => p.extend(t.chat_params(max_tokens)),
        }
        let body = Value::Object(p);
        let mut probe = Probe::default();
        let began = Instant::now();
        let mut watch = Watch::new(t.protocol);
        let result = async {
            let mut body = self.open(&body).await?;
            let mut sse = Sse::default();
            let mut live = VecDeque::new();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| LlmError::Stream(error_kind(&e).into()))?;
                for ev in sse.push(&chunk) {
                    watch.note(&ev, &mut live)?;
                    if watch.saw_content && probe.ttft.is_none() {
                        probe.ttft = Some(began.elapsed().as_secs_f64());
                    }
                }
            }
            if let Some(ev) = sse.finish() {
                watch.note(&ev, &mut live)?;
            }
            Ok::<(), LlmError>(())
        }
        .await;
        match result {
            Ok(()) => probe.total = Some(began.elapsed().as_secs_f64()),
            Err(e) => probe.error = Some(e.to_string()),
        }
        probe.model = watch.model().to_string();
        probe.text = watch.text().trim().to_string();
        probe
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Start,
    Streaming,
    Done,
}

/// A turn in flight: the request (and its redo), the stream, and the events queued.
struct Turn {
    model: Model,
    context: Context,
    tools: Vec<Value>,
    options: Options,
    phase: Phase,
    body: Option<Body>,
    sse: Sse,
    watch: Watch,
    queue: VecDeque<Event>,
    /// The reply in flight is the redo of a cut one.
    redoing: bool,
}

impl Turn {
    async fn next(&mut self) -> Option<Result<Event, LlmError>> {
        loop {
            if let Some(e) = self.queue.pop_front() {
                return Some(Ok(e));
            }
            match self.phase {
                Phase::Done => return None,
                Phase::Start => {
                    let body = self
                        .model
                        .turn_body(&self.context, &self.tools, &self.options);
                    match self.model.open(&body).await {
                        Ok(b) => {
                            self.body = Some(b);
                            self.sse = Sse::default();
                            self.watch = Watch::new(self.model.target.protocol);
                            self.phase = Phase::Streaming;
                        }
                        Err(e) => {
                            self.phase = Phase::Done;
                            return Some(Err(e));
                        }
                    }
                }
                Phase::Streaming => {
                    let chunk = match self.body.as_mut() {
                        Some(b) => b.next().await,
                        None => None,
                    };
                    match chunk {
                        Some(Ok(bytes)) => {
                            for ev in self.sse.push(&bytes) {
                                if let Err(e) = self.watch.note(&ev, &mut self.queue) {
                                    self.phase = Phase::Done;
                                    return Some(Err(e));
                                }
                            }
                        }
                        Some(Err(e)) => {
                            self.phase = Phase::Done;
                            return Some(Err(LlmError::Stream(error_kind(&e).into())));
                        }
                        None => {
                            self.body = None;
                            if let Some(ev) = self.sse.finish()
                                && let Err(e) = self.watch.note(&ev, &mut self.queue)
                            {
                                self.phase = Phase::Done;
                                return Some(Err(e));
                            }
                            let watch = std::mem::replace(
                                &mut self.watch,
                                Watch::new(self.model.target.protocol),
                            );
                            self.finish_reply(watch.finish());
                        }
                    }
                }
            }
        }
    }

    /// The reply has ended: its calls when it is whole, or the recovery when it was cut.
    fn finish_reply(&mut self, reply: Reply) {
        if let Some(usage) = &reply.usage {
            self.queue.push_back(Event::Usage(usage.clone()));
        }
        let reason = reply
            .stop_reason
            .as_deref()
            .map_or(FinishReason::End, FinishReason::parse);
        if !reply.cut_off() {
            self.redoing = false;
            for call in reply.calls {
                self.queue.push_back(Event::ToolCall(call));
            }
            self.queue.push_back(Event::Finished(reason));
            self.phase = Phase::Done;
            return;
        }
        // Pushed after the reply's end, so the caller closes the cut reply before the
        // note lands behind it.
        self.queue.push_back(Event::Finished(reason));
        let lost = reply.lost();
        self.queue.push_back(Event::CutOff { tool_calls: lost });
        let cap = self.model.target.output_cap(self.options.max_tokens);
        self.phase = Phase::Done;
        if lost == 0 {
            // A long answer, cut short: the operator heard what there was of it.
            warn!("model: reply cut off at {cap} output tokens (text only)");
            return;
        }
        if self.redoing {
            self.redoing = false;
            warn!(
                "model: the redo was cut off at {cap} output tokens too ({lost} tool calls); \
                 saying so"
            );
            self.queue.push_back(Event::Say(CUT_OFF_LINE.into()));
            return;
        }
        self.redoing = true;
        warn!(
            "model: reply cut off at {cap} output tokens with {lost} tool calls in it; none \
             ran, asking for it in smaller steps"
        );
        if !reply.text.is_empty() {
            self.context.push(Message::assistant(reply.text));
        }
        self.context.push(Message::user(CUT_OFF_NOTE));
        self.queue.push_back(Event::Redo);
        self.phase = Phase::Start;
    }
}

static CLIENTS: LazyLock<Mutex<HashMap<String, Arc<Model>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The process's client for `target`, one per connection (its fingerprint: protocol,
/// address, key, headers, timeout), so a session doesn't open a new connection on its
/// first call. `Model::new` is the one-off for a connection being tried out.
pub fn shared(target: &Target) -> Result<Arc<Model>, LlmError> {
    let key = target.fingerprint();
    let mut clients = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = clients.get(&key) {
        return Ok(Arc::clone(m));
    }
    let model = Arc::new(Model::new(target.clone())?);
    clients.insert(key, Arc::clone(&model));
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_named_without_its_path_or_credentials() {
        assert_eq!(
            host("https://api.anthropic.com/v1"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            host("http://localhost:11434/v1/chat"),
            "http://localhost:11434"
        );
        assert_eq!(host("http://user:pw@proxy:8317/x"), "http://proxy:8317");
        assert_eq!(host("nonsense"), "it");
    }

    #[test]
    fn prefill_follows_the_list() {
        assert!(prefill_supported("claude-sonnet-4-5-20250929"));
        assert!(!prefill_supported("claude-sonnet-5-5"));
        assert!(!prefill_supported("anthropic/claude-sonnet-5.5"));
    }

    #[test]
    fn finish_reasons_share_one_vocabulary() {
        assert_eq!(FinishReason::parse("end_turn"), FinishReason::End);
        assert_eq!(FinishReason::parse("stop"), FinishReason::End);
        assert_eq!(FinishReason::parse("tool_calls"), FinishReason::ToolUse);
        assert_eq!(FinishReason::parse("length"), FinishReason::MaxTokens);
        assert_eq!(
            FinishReason::parse("content_filter"),
            FinishReason::Other("content_filter".into())
        );
    }

    #[test]
    fn errors_read_as_the_python_said_them() {
        let e = LlmError::Status {
            status: 401,
            detail: "invalid x-api-key".into(),
        };
        assert_eq!(e.to_string(), "HTTP 401: invalid x-api-key");
        let e = LlmError::Status {
            status: 502,
            detail: String::new(),
        };
        assert_eq!(e.to_string(), "HTTP 502");
        let e = LlmError::Unreachable {
            host: "http://localhost:11434".into(),
            kind: "ConnectError",
        };
        assert_eq!(
            e.to_string(),
            "ConnectError: can't reach http://localhost:11434"
        );
    }
}
