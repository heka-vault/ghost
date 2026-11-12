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

use crate::capabilities::{self, Capabilities};
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
    /// The Messages API's `output_config.effort` (`low`, `medium`, `high`); None sends
    /// none and the model's default holds (`medium` on Haiku 5.5, `high` on Sonnet 5.5).
    /// It sets how much a thinking model thinks before its first word or call.
    pub effort: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_tokens: LLM_MAX_TOKENS,
            thinking: Some(DEFAULT_THINKING.into()),
            cache_system: true,
            effort: None,
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
    /// A tool call has begun: its name is known, its arguments are still being written.
    /// The first sign a reply needs a tool, a second or more before the call is whole
    /// (`ToolCall`), so the voice can say it heard. Nothing runs on it.
    ToolStarting(String),
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

/// One request as the provider check sees it ([`Model::measure`]). Seconds from the call.
#[derive(Debug, Default)]
pub struct Measured {
    /// The first byte of the reply's body.
    pub first_byte: Option<f64>,
    /// The first piece of text.
    pub first_word: Option<f64>,
    pub total: Option<f64>,
    /// How many network chunks carried text: one means the reply came all at once.
    pub text_chunks: usize,
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<String>,
    /// As the provider answered, when it says.
    pub model: String,
    pub error: Option<LlmError>,
}

/// `reasoning_effort`, as the route's refused thinking setting on Chat Completions.
pub const REASONING_EFFORT: &str = "reasoning_effort";
/// The longest a rate limit or an overloaded provider is waited for, in a turn: the
/// operator is listening. Longer, and the turn says so instead.
pub const MOST_WAIT: Duration = Duration::from_secs(2);
/// The wait when the provider says none.
const DEFAULT_WAIT: Duration = Duration::from_millis(500);

/// Whether a failure is worth one more try in a turn: a rate limit, or an overloaded or
/// briefly failing provider. A refusal of the request itself is not.
pub fn comes_back(e: &LlmError) -> bool {
    match e {
        LlmError::Status { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504 | 529),
        _ => false,
    }
}

fn pause(asked: Option<Duration>) -> Duration {
    asked.unwrap_or(DEFAULT_WAIT).min(MOST_WAIT)
}

/// How long a refusal asked Ghost to wait: OpenRouter's
/// `error.metadata.retry_after_seconds` in the body, else the `Retry-After` header in
/// seconds. Adapted from Goose's `extract_retry_after`
/// (`crates/goose-providers/src/http_status.rs`, Apache-2.0); the HTTP-date form is not
/// read, and a wait past [`MOST_WAIT`] is cut to it by the caller anyway.
pub fn retry_after(headers: &HeaderMap, body: &[u8]) -> Option<Duration> {
    let finite =
        |s: f64| (s.is_finite() && s >= 0.0).then(|| Duration::from_secs_f64(s.min(3600.0)));
    let from_body = serde_json::from_slice::<Value>(body).ok().and_then(|v| {
        v["error"]["metadata"]["retry_after_seconds"]
            .as_f64()
            .and_then(finite)
    });
    from_body.or_else(|| {
        headers
            .get(reqwest::header::RETRY_AFTER)?
            .to_str()
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(finite)
    })
}

/// A field of Ghost's request the route refused.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Refused {
    Thinking(String),
    Effort,
    /// The Messages API's `output_config.effort`.
    OutputEffort,
    CacheMarkers,
}

impl Refused {
    fn name(&self) -> String {
        match self {
            Refused::Thinking(t) => format!("thinking {t}"),
            Refused::Effort => REASONING_EFFORT.into(),
            Refused::OutputEffort => "the effort setting".into(),
            Refused::CacheMarkers => "the cache markers".into(),
        }
    }

    fn apply(&self, c: &mut Capabilities) {
        match self {
            Refused::Thinking(t) => c.rejects_thinking = Some(t.clone()),
            Refused::Effort => c.rejects_thinking = Some(REASONING_EFFORT.into()),
            Refused::OutputEffort => c.rejects_effort = true,
            Refused::CacheMarkers => c.rejects_cache_markers = true,
        }
    }
}

/// The field an HTTP 400 is about, when it is one Ghost sent and can do without.
fn refused_field(e: &LlmError, body: &Value) -> Option<Refused> {
    let LlmError::Status {
        status: 400 | 422,
        detail,
    } = e
    else {
        return None;
    };
    let d = detail.to_lowercase();
    // Before the thinking setting: a refused effort's message may name thinking too.
    if body.get("output_config").is_some() && (d.contains("effort") || d.contains("output_config"))
    {
        return Some(Refused::OutputEffort);
    }
    if let Some(t) = body["thinking"]["type"].as_str()
        && (d.contains("thinking") || d.contains(t))
    {
        return Some(Refused::Thinking(t.to_string()));
    }
    if body.get(REASONING_EFFORT).is_some() && d.contains("reasoning") {
        return Some(Refused::Effort);
    }
    if d.contains("cache_control") && body.to_string().contains("cache_control") {
        return Some(Refused::CacheMarkers);
    }
    None
}

/// What Ghost says when a turn's model call failed, in plain words for the operator:
/// never silence (ADR 0056). Never a key, a body or an address.
pub fn spoken_failure(e: &LlmError) -> String {
    match e {
        LlmError::Unreachable { kind, .. } if *kind == "ReadTimeout" => {
            "The model took too long to answer. Try again in a moment.".into()
        }
        LlmError::Unreachable { .. } => {
            "I can't reach the model right now. Check the model connection in Settings.".into()
        }
        LlmError::Status { status, .. } => match status {
            401 | 403 => "The model provider refused Ghost's key. Check the model connection in Settings.".into(),
            402 => "The model provider says the account is out of credit.".into(),
            404 => "The model provider doesn't offer that model. Check the model connection in Settings.".into(),
            408 => "The model took too long to answer. Try again in a moment.".into(),
            429 => "The model provider is limiting how fast I can ask. Try again in a moment.".into(),
            503 | 529 => "The model provider is overloaded right now. Try again in a moment.".into(),
            s if *s >= 500 => "The model provider had an error. Try again in a moment.".into(),
            _ => "The model provider refused the request. The reason is in Ghost's log.".into(),
        },
        LlmError::Api { kind, .. } => {
            let k = kind.to_lowercase();
            if k.contains("overloaded") {
                "The model provider is overloaded right now. Try again in a moment.".into()
            } else if k.contains("rate") {
                "The model provider is limiting how fast I can ask. Try again in a moment.".into()
            } else {
                "The model stopped with an error partway. Try again in a moment.".into()
            }
        }
        LlmError::Stream(_) => {
            "The model's answer broke off partway. Try again in a moment.".into()
        }
        LlmError::Headers(_) | LlmError::Client(_) => {
            "Ghost can't send to the model with these settings. Check the model connection in Settings.".into()
        }
    }
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
        self.shaped_body(context, tools, options, &Capabilities::default())
    }

    /// The turn's body as the route takes it (ADR 0056): a thinking setting, a
    /// `reasoning_effort` or cache markers the route refused before are left out.
    fn shaped_body(
        &self,
        context: &Context,
        tools: &[Value],
        options: &Options,
        caps: &Capabilities,
    ) -> Value {
        let t = &*self.target;
        let markers = !caps.rejects_cache_markers;
        let mut p = Map::new();
        p.insert("model".into(), json!(t.model));
        p.insert("stream".into(), json!(true));
        match t.protocol {
            Protocol::Anthropic => {
                p.insert("max_tokens".into(), json!(options.max_tokens));
                if let Some(thinking) = &options.thinking
                    && caps.takes_thinking(thinking)
                {
                    p.insert("thinking".into(), json!({"type": thinking}));
                }
                if let Some(effort) = &options.effort
                    && !caps.rejects_effort
                {
                    p.insert("output_config".into(), json!({"effort": effort}));
                }
                let conv = context.anthropic(markers, !prefill_supported(&t.model));
                if !context.system.is_empty() {
                    p.insert("system".into(), conv.system);
                }
                p.insert("messages".into(), Value::Array(conv.messages));
                if !tools.is_empty() {
                    p.insert("tools".into(), Value::Array(tools.to_vec()));
                }
                if options.cache_system && markers {
                    with_system_breakpoint(&mut p);
                }
            }
            Protocol::Openai => {
                p.insert("stream_options".into(), json!({"include_usage": true}));
                p.insert("messages".into(), Value::Array(context.openai()));
                if !tools.is_empty() {
                    p.insert("tools".into(), Value::Array(tools.to_vec()));
                }
                let mut params = t.chat_params(options.max_tokens);
                if !caps.takes_thinking(REASONING_EFFORT) {
                    params.remove(REASONING_EFFORT);
                }
                p.extend(params);
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

    /// One request; a refusal carries how long the provider asked Ghost to wait.
    async fn send_once(
        &self,
        body: &Value,
    ) -> Result<reqwest::Response, (LlmError, Option<Duration>)> {
        let url = self.url(self.path());
        let headers = self.headers().map_err(|e| (e, None))?;
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                (
                    LlmError::Unreachable {
                        host: host(&url),
                        kind: error_kind(&e),
                    },
                    None,
                )
            })?;
        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.bytes().await.unwrap_or_default();
            let wait = retry_after(&headers, &body);
            return Err((
                LlmError::Status {
                    status: status.as_u16(),
                    detail: self.detail(&body),
                },
                wait,
            ));
        }
        Ok(resp)
    }

    /// One request, with one more try after a rate limit or an overloaded provider
    /// (ADR 0056).
    async fn send(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        match self.send_once(body).await {
            Ok(r) => Ok(r),
            Err((e, wait)) if comes_back(&e) => {
                let wait = pause(wait);
                warn!("model: {e}; trying once more in {:.1}s", wait.as_secs_f64());
                tokio::time::sleep(wait).await;
                self.send_once(body).await.map_err(|(e, _)| e)
            }
            Err((e, _)) => Err(e),
        }
    }

    async fn open(&self, body: &Value) -> Result<Body, LlmError> {
        let resp = self.send(body).await?;
        Ok(Box::pin(resp.bytes_stream().map(|r| r.map(|b| b.to_vec()))))
    }

    /// A turn's request as the route takes it (ADR 0056): what the route refused before
    /// is left out; a field it refuses now (HTTP 400 naming the thinking setting,
    /// `reasoning_effort` or the cache markers) is left out and the request sent again,
    /// once per field, and the route remembers; a rate limit or an overloaded provider is
    /// tried once more after the wait it asked for (two seconds at most). Anything else is
    /// the error, for the caller to say.
    async fn open_turn(
        &self,
        context: &Context,
        tools: &[Value],
        options: &Options,
    ) -> Result<Body, LlmError> {
        let mut caps = capabilities::learned(&self.target);
        let mut waited = false;
        for _ in 0..4 {
            let body = self.shaped_body(context, tools, options, &caps);
            match self.send_once(&body).await {
                Ok(resp) => {
                    return Ok(Box::pin(resp.bytes_stream().map(|r| r.map(|b| b.to_vec()))));
                }
                Err((e, wait)) => {
                    if let Some(field) = refused_field(&e, &body) {
                        warn!(
                            "model: {} refused {}; sending it without, from now on",
                            self.target.model,
                            field.name()
                        );
                        capabilities::learn(&self.target, |c| field.apply(c));
                        field.apply(&mut caps);
                        continue;
                    }
                    if comes_back(&e) && !waited {
                        waited = true;
                        let wait = pause(wait);
                        warn!("model: {e}; trying once more in {:.1}s", wait.as_secs_f64());
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(LlmError::Stream(
            "the provider kept refusing the request".into(),
        ))
    }

    /// One request sent exactly as given, timed from the call: its first byte, its first
    /// word, and what came back. No retry, no capability applied: the provider check
    /// (`crate::check`) wants to see what the route does with each field.
    pub async fn measure(&self, context: &Context, tools: &Tools, options: &Options) -> Measured {
        self.measure_shaped(context, tools, options, &Capabilities::default())
            .await
    }

    /// [`Model::measure`], with what the route refused already left out.
    pub async fn measure_shaped(
        &self,
        context: &Context,
        tools: &Tools,
        options: &Options,
        caps: &Capabilities,
    ) -> Measured {
        let body = self.shaped_body(context, &self.wire_tools(tools), options, caps);
        let began = Instant::now();
        let mut m = Measured::default();
        let mut watch = Watch::new(self.target.protocol);
        let result = async {
            let resp = self.send_once(&body).await.map_err(|(e, _)| e)?;
            let mut stream = resp.bytes_stream();
            let mut sse = Sse::default();
            let mut live = VecDeque::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| LlmError::Stream(error_kind(&e).into()))?;
                m.first_byte.get_or_insert(began.elapsed().as_secs_f64());
                let mut texts = 0;
                for ev in sse.push(&chunk) {
                    watch.note(&ev, &mut live)?;
                    for e in live.drain(..) {
                        if matches!(e, Event::Text(ref t) if !t.is_empty()) {
                            texts += 1;
                        }
                    }
                }
                if texts > 0 {
                    m.first_word.get_or_insert(began.elapsed().as_secs_f64());
                    m.text_chunks += 1;
                }
            }
            if let Some(ev) = sse.finish() {
                watch.note(&ev, &mut live)?;
            }
            Ok::<(), LlmError>(())
        }
        .await;
        m.total = Some(began.elapsed().as_secs_f64());
        let reply = watch.finish();
        m.model = reply.model.clone();
        m.text = reply.text.trim().to_string();
        m.calls = reply.calls;
        m.usage = reply.usage;
        m.stop_reason = reply.stop_reason;
        if let Err(e) = result {
            m.error = Some(e);
        }
        m
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

    /// Write the prompt cache for a turn on `context` before the turn comes (a session's
    /// opening, while the greeting is spoken): the turn's own tools, system prompt, history
    /// and thinking setting, with a placeholder user turn after the history that carries no
    /// cache marker, `max_tokens: 0` and not streamed, so the provider writes the cache and
    /// answers nothing. A provider that refuses no output (a 400) is asked again for one
    /// token. The Messages API only: Chat Completions providers cache by themselves.
    pub async fn warm(
        &self,
        context: &Context,
        tools: &Tools,
        options: &Options,
    ) -> Result<Usage, LlmError> {
        if self.target.protocol != Protocol::Anthropic {
            return Ok(Usage::default());
        }
        let mut ctx = context.clone();
        ctx.push(Message::user(WARM_PLACEHOLDER));
        // Shaped as the route's real turns are (ADR 0056), or the prefix warmed is not the
        // one read: a setting the route refused is left out here too.
        let caps = capabilities::learned(&self.target);
        let mut body = self.shaped_body(&ctx, &self.wire_tools(tools), options, &caps);
        body["stream"] = json!(false);
        if let Some(last) = body["messages"]
            .as_array_mut()
            .and_then(|m| m.last_mut())
            .and_then(|m| m["content"].as_array_mut())
            .and_then(|b| b.last_mut())
            .and_then(|b| b.as_object_mut())
        {
            last.remove("cache_control"); // the placeholder is never read back
        }
        let mut sent = None;
        for cap in [0u32, 1] {
            body["max_tokens"] = json!(cap);
            match self.send(&body).await {
                Ok(resp) => {
                    sent = Some(resp);
                    break;
                }
                Err(LlmError::Status { status: 400, .. }) if cap == 0 => continue,
                Err(e) => return Err(e),
            }
        }
        let Some(resp) = sent else {
            return Err(LlmError::Stream("no answer".into()));
        };
        let v: Value = resp
            .json()
            .await
            .map_err(|_| LlmError::Stream("an answer that isn't JSON".into()))?;
        let mut usage = Usage::default();
        crate::reply::note_anthropic_usage(&v["usage"], &mut usage);
        Ok(usage)
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
                    match self
                        .model
                        .open_turn(&self.context, &self.tools, &self.options)
                        .await
                    {
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

/// The user turn a warm-up ends on: read during prefill, never answered.
pub const WARM_PLACEHOLDER: &str = "(warming up)";

static CLIENTS: LazyLock<Mutex<HashMap<String, Arc<Model>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The process's client for `target`, one per connection (its fingerprint: protocol,
/// address, key, headers, timeout), so a session doesn't open a new connection on its
/// first call. `Model::new` is the one-off for a connection being tried out.
pub fn shared(target: &Target) -> Result<Arc<Model>, LlmError> {
    let key = target.fingerprint();
    let mut clients = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = clients.get(&key) {
        if m.target.model == target.model && m.target.provider == target.provider {
            return Ok(Arc::clone(m));
        }
        // The same connection, another model: the fast model beside the main one (ADR
        // 0050), or a model the operator has just chosen. The connection is shared; the
        // model is the target's.
        return Ok(Arc::new(Model::with_client(m.http.clone(), target.clone())));
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

    fn anthropic_target(model: &str) -> Target {
        let mut env = std::collections::HashMap::new();
        env.insert("GHOST_LLM_PROVIDER".to_string(), "anthropic".to_string());
        env.insert("GHOST_LLM_MODEL".to_string(), model.to_string());
        ghost_core::connection::ModelConnection::from_env(&env).target(&env)
    }

    #[test]
    fn an_effort_goes_out_as_output_config_and_a_route_that_refuses_it_learns() {
        // The fast model's thinking depth (`Options::effort`, 2026-11-12): sent as the
        // Messages API's `output_config.effort`, left out when unset, and dropped for good
        // on a route that answers it with a 400, as a refused thinking setting is.
        let model = Model::new(anthropic_target("claude-haiku-5-5")).unwrap();
        let mut ctx = Context::new("You are Ghost.");
        ctx.push(Message::user("hi"));
        let mut options = Options::default();
        let body = model.body(&ctx, &Tools::default(), &options);
        assert!(body.get("output_config").is_none(), "{body}");
        options.effort = Some("low".into());
        let body = model.body(&ctx, &Tools::default(), &options);
        assert_eq!(body["output_config"], json!({"effort": "low"}));
        let refused = LlmError::Status {
            status: 400,
            detail: "output_config.effort: not supported for this model".into(),
        };
        let r = refused_field(&refused, &body).expect("the effort is the field refused");
        assert_eq!(r, Refused::OutputEffort);
        let mut caps = Capabilities::default();
        r.apply(&mut caps);
        let shaped = model.shaped_body(&ctx, &[], &options, &caps);
        assert!(shaped.get("output_config").is_none(), "{shaped}");
        assert_eq!(shaped["thinking"], json!({"type": "between_tools"}));
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
