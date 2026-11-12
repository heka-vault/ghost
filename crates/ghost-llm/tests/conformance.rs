//! Every shape of model connection Ghost supports, through the same turns (ADR 0056):
//! Anthropic direct, an Anthropic-compatible subscription proxy, OpenRouter on its
//! Anthropic and its OpenAI API, OpenAI, Ollama, LM Studio and a URL of the operator's,
//! each a local fake server speaking that provider's protocol at that provider's path with
//! its quirks. Each takes a spoken turn with a tool call, a streamed reply, a reply cut at
//! the cap, a rate limit, a timeout and a malformed chunk, and Ghost must do the same thing
//! on every one of them: deliver the reply whole, or fail with an error it can say
//! (`spoken_failure`), never go quiet.
//!
//! Nothing leaves the machine. The words are synthetic.

mod common;

use std::time::Duration;

use common::anthropic::{events as a_reply, text as a_text, tool as a_tool};
use common::openai::{call as o_call, chunk as o_chunk, reply as o_reply};
use common::{KEY, cut_offs, kinds, said, text, tool_calls};
use futures::StreamExt;
use ghost_core::connection::{Protocol, Target, reasoning};
use ghost_llm::capabilities::{self, learned};
use ghost_llm::{
    CUT_OFF_LINE, Context, Event, FinishReason, LlmError, Model, Options, ToolSchema, Tools,
    spoken_failure,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use zeroize::Zeroizing;

/// One provider shape: who it is, what it speaks, where it listens, how it wants the key.
#[derive(Clone, Copy, Debug)]
struct Shape {
    name: &'static str,
    provider: &'static str,
    protocol: Protocol,
    /// The base URL's path under the server (the provider's own).
    base: &'static str,
    model: &'static str,
    /// Wants `x-api-key`.
    x_api_key: bool,
    /// Wants `Authorization: Bearer`.
    bearer: bool,
    /// Sends a whole reply's tool calls in one chunk (Ollama).
    calls_in_one_chunk: bool,
    /// Sends SSE comment lines between chunks (OpenRouter's keep-alives).
    comments: bool,
}

const SHAPES: [Shape; 8] = [
    Shape {
        name: "Anthropic direct",
        provider: "anthropic",
        protocol: Protocol::Anthropic,
        base: "",
        model: "claude-sonnet-5-5",
        x_api_key: true,
        bearer: false,
        calls_in_one_chunk: false,
        comments: false,
    },
    Shape {
        name: "a subscription proxy (CLIProxy)",
        provider: "cliproxy",
        protocol: Protocol::Anthropic,
        base: "",
        model: "claude-sonnet-5-5",
        x_api_key: true,
        bearer: true,
        calls_in_one_chunk: false,
        comments: false,
    },
    Shape {
        name: "OpenRouter, Anthropic API",
        provider: "openrouter",
        protocol: Protocol::Anthropic,
        base: "/api",
        model: "anthropic/claude-sonnet-5.5",
        x_api_key: true,
        bearer: true,
        calls_in_one_chunk: false,
        comments: true,
    },
    Shape {
        name: "OpenRouter, OpenAI API",
        provider: "custom",
        protocol: Protocol::Openai,
        base: "/api/v1",
        model: "anthropic/claude-sonnet-5.5",
        x_api_key: false,
        bearer: true,
        calls_in_one_chunk: false,
        comments: true,
    },
    Shape {
        name: "OpenAI",
        provider: "openai",
        protocol: Protocol::Openai,
        base: "/v1",
        model: "gpt-5.1",
        x_api_key: false,
        bearer: true,
        calls_in_one_chunk: false,
        comments: false,
    },
    Shape {
        name: "Ollama",
        provider: "ollama",
        protocol: Protocol::Openai,
        base: "/v1",
        model: "qwen3:8b",
        x_api_key: false,
        bearer: true,
        calls_in_one_chunk: true,
        comments: false,
    },
    Shape {
        name: "LM Studio",
        provider: "lmstudio",
        protocol: Protocol::Openai,
        base: "/v1",
        model: "local-model",
        x_api_key: false,
        bearer: true,
        calls_in_one_chunk: false,
        comments: false,
    },
    Shape {
        name: "a custom URL",
        provider: "custom",
        protocol: Protocol::Anthropic,
        base: "/gateway",
        model: "claude-haiku-5-5",
        x_api_key: true,
        bearer: true,
        calls_in_one_chunk: false,
        comments: false,
    },
];

impl Shape {
    fn endpoint(&self) -> String {
        match self.protocol {
            Protocol::Anthropic => format!("{}/v1/messages", self.base),
            Protocol::Openai => format!("{}/chat/completions", self.base),
        }
    }

    fn target(&self, server: &MockServer, timeout: f64) -> Target {
        let cap_param = if self.provider == "openai" || reasoning(self.model).reasons {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        Target {
            provider: self.provider.into(),
            protocol: self.protocol,
            base_url: format!("{}{}", server.uri(), self.base),
            model: self.model.into(),
            api_key: Zeroizing::new(KEY.into()),
            headers: Vec::new(),
            timeout_secs: Some(timeout),
            cap_param,
            reasoning: match self.protocol {
                Protocol::Openai => reasoning(self.model),
                Protocol::Anthropic => Default::default(),
            },
        }
    }

    /// A streamed reply of `words`, in pieces.
    fn words(&self, words: &[&str]) -> String {
        match self.protocol {
            Protocol::Anthropic => {
                let blocks: Vec<_> = words.iter().map(|w| a_text(w)).collect();
                self.spaced(a_reply(&blocks, "end_turn"))
            }
            Protocol::Openai => {
                let mut chunks: Vec<String> =
                    words.iter().map(|w| o_chunk(Some(w), None, None)).collect();
                chunks.push(o_chunk(None, None, Some("stop")));
                self.spaced(o_reply(&chunks))
            }
        }
    }

    /// "Let me look." then a call to `get_issue` with `{"number": 7}`.
    fn tool_call(&self) -> String {
        match self.protocol {
            Protocol::Anthropic => self.spaced(a_reply(
                &[
                    a_text("Let me look."),
                    a_tool("get_issue", &[r#"{"numb"#, r#"er": 7}"#], "toolu_1"),
                ],
                "tool_use",
            )),
            Protocol::Openai if self.calls_in_one_chunk => self.spaced(o_reply(&[
                o_chunk(Some("Let me look."), None, None),
                o_chunk(
                    None,
                    Some(vec![o_call(
                        0,
                        Some("c1"),
                        Some("get_issue"),
                        Some(r#"{"number": 7}"#),
                    )]),
                    Some("tool_calls"),
                ),
            ])),
            Protocol::Openai => self.spaced(o_reply(&[
                o_chunk(Some("Let me look."), None, None),
                o_chunk(
                    None,
                    Some(vec![o_call(
                        0,
                        Some("c1"),
                        Some("get_issue"),
                        Some(r#"{"numb"#),
                    )]),
                    None,
                ),
                o_chunk(
                    None,
                    Some(vec![o_call(0, None, None, Some(r#"er": 7}"#))]),
                    None,
                ),
                o_chunk(None, None, Some("tool_calls")),
            ])),
        }
    }

    /// A reply stopped at the output cap inside its second tool call.
    fn cut(&self) -> String {
        match self.protocol {
            Protocol::Anthropic => self.spaced(a_reply(
                &[
                    a_tool("get_issue", &[r#"{"number": 1}"#], "toolu_1"),
                    a_tool("get_issue", &[r#"{"numb"#], "toolu_2"),
                ],
                "max_tokens",
            )),
            Protocol::Openai => self.spaced(o_reply(&[
                o_chunk(
                    None,
                    Some(vec![o_call(
                        0,
                        Some("c1"),
                        Some("get_issue"),
                        Some(r#"{"number": 1}"#),
                    )]),
                    None,
                ),
                o_chunk(
                    None,
                    Some(vec![o_call(
                        1,
                        Some("c2"),
                        Some("get_issue"),
                        Some(r#"{"numb"#),
                    )]),
                    None,
                ),
                o_chunk(None, None, Some("length")),
            ])),
        }
    }

    /// A reply whose second event is not JSON.
    fn malformed(&self) -> String {
        let good = match self.protocol {
            Protocol::Anthropic => common::sse(
                "message_start",
                &json!({"type": "message_start", "message": {"model": self.model, "usage": {}}}),
            ),
            Protocol::Openai => o_chunk(Some("Hel"), None, None),
        };
        format!("{good}data: {{\"choices\": [{{\"delta\": \n\n")
    }

    /// OpenRouter puts comment lines between events; the others don't.
    fn spaced(&self, body: String) -> String {
        if self.comments {
            body.replace("\n\n", "\n\n: OPENROUTER PROCESSING\n\n")
        } else {
            body
        }
    }

    /// A refusal body in the provider's own error shape.
    fn error_body(&self, kind: &str, message: &str) -> String {
        match self.protocol {
            Protocol::Anthropic => {
                json!({"type": "error", "error": {"type": kind, "message": message}}).to_string()
            }
            Protocol::Openai => {
                json!({"error": {"type": kind, "code": kind, "message": message}}).to_string()
            }
        }
    }
}

/// Answers in turn from a list of responses; past the end, the last again.
struct Seq(Vec<ResponseTemplate>, std::sync::atomic::AtomicUsize);

impl Respond for Seq {
    fn respond(&self, _r: &Request) -> ResponseTemplate {
        let i = self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0[i.min(self.0.len() - 1)].clone()
    }
}

fn stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

async fn server(shape: &Shape, replies: Vec<ResponseTemplate>) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(shape.endpoint()))
        .respond_with(Seq(replies, Default::default()))
        .mount(&s)
        .await;
    s
}

fn context() -> Context {
    Context::new("You are a voice assistant under test.").user("What does issue seven say?")
}

fn tools() -> Tools {
    Tools::new(vec![ToolSchema::new(
        "get_issue",
        "Reads an issue.",
        json!({"number": {"type": "integer"}}),
        vec!["number".into()],
    )])
}

/// A turn's events, or the first error, as the voice stage takes them.
async fn turn(shape: &Shape, s: &MockServer, timeout: f64) -> Result<Vec<Event>, LlmError> {
    let target = shape.target(s, timeout);
    capabilities::forget(&target);
    let model = Model::new(target).expect("a client");
    let mut out = Vec::new();
    let mut st = Box::pin(model.stream(context(), &tools(), &Options::default()));
    while let Some(item) = st.next().await {
        out.push(item?);
    }
    Ok(out)
}

/// Ghost's behaviour on a failure: an error the turn says, in a sentence, never a value.
fn says(e: &LlmError, words: &str, shape: &Shape) {
    let line = spoken_failure(e);
    assert!(
        line.contains(words),
        "{}: '{line}' should say '{words}' for {e}",
        shape.name
    );
    assert!(
        !line.contains(KEY) && !e.to_string().contains(KEY),
        "{}",
        shape.name
    );
}

#[tokio::test]
async fn every_shape_streams_a_reply_to_its_own_path_with_its_own_key_header() {
    for shape in &SHAPES {
        let s = server(
            shape,
            vec![stream(shape.words(&["Issue ", "seven ", "says hi."]))],
        )
        .await;
        let events = turn(shape, &s, 5.0).await.expect(shape.name);
        assert_eq!(text(&events), "Issue seven says hi.", "{}", shape.name);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Text(_)))
                .count(),
            3,
            "{}: the reply streams in its pieces",
            shape.name
        );
        assert!(
            matches!(events.last(), Some(Event::Finished(FinishReason::End))),
            "{}: {:?}",
            shape.name,
            kinds(&events)
        );
        let req = &s.received_requests().await.unwrap()[0];
        let has = |h: &str| req.headers.get(h).is_some();
        assert_eq!(
            has("x-api-key"),
            shape.x_api_key,
            "{}: x-api-key",
            shape.name
        );
        assert_eq!(has("authorization"), shape.bearer, "{}: bearer", shape.name);
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], shape.model, "{}", shape.name);
        assert_eq!(body["stream"], true, "{}", shape.name);
    }
}

#[tokio::test]
async fn every_shape_delivers_a_tool_call_whole_after_the_words() {
    for shape in &SHAPES {
        let s = server(shape, vec![stream(shape.tool_call())]).await;
        let events = turn(shape, &s, 5.0).await.expect(shape.name);
        assert_eq!(text(&events), "Let me look.", "{}", shape.name);
        let calls = tool_calls(&events);
        assert_eq!(calls.len(), 1, "{}: {:?}", shape.name, kinds(&events));
        assert_eq!(calls[0].1, "get_issue", "{}", shape.name);
        assert_eq!(calls[0].2["number"], 7, "{}", shape.name);
        assert!(
            matches!(events.last(), Some(Event::Finished(FinishReason::ToolUse))),
            "{}",
            shape.name
        );
    }
}

#[tokio::test]
async fn every_shape_redoes_a_cut_reply_then_says_so_when_cut_again() {
    for shape in &SHAPES {
        let s = server(shape, vec![stream(shape.cut()), stream(shape.cut())]).await;
        let events = turn(shape, &s, 5.0).await.expect(shape.name);
        assert!(
            tool_calls(&events).is_empty(),
            "{}: half a batch ran",
            shape.name
        );
        assert_eq!(cut_offs(&events), vec![2, 2], "{}", shape.name);
        assert!(kinds(&events).contains(&"Redo"), "{}", shape.name);
        assert_eq!(
            said(&events),
            vec![CUT_OFF_LINE.to_string()],
            "{}",
            shape.name
        );
    }
}

#[tokio::test]
async fn every_shape_waits_once_on_a_rate_limit_then_answers() {
    for shape in &SHAPES {
        let limited = ResponseTemplate::new(429)
            .insert_header("retry-after", "0")
            .set_body_string(shape.error_body("rate_limit_error", "slow down"));
        let s = server(shape, vec![limited, stream(shape.words(&["Here."]))]).await;
        let events = turn(shape, &s, 5.0).await.expect(shape.name);
        assert_eq!(text(&events), "Here.", "{}", shape.name);
        assert_eq!(
            s.received_requests().await.unwrap().len(),
            2,
            "{}",
            shape.name
        );
    }
}

#[tokio::test]
async fn every_shape_says_a_rate_limit_that_lasts() {
    for shape in &SHAPES {
        let limited = ResponseTemplate::new(429)
            .insert_header("retry-after", "0")
            .set_body_string(shape.error_body("rate_limit_error", "slow down"));
        let s = server(shape, vec![limited]).await;
        let e = turn(shape, &s, 5.0).await.expect_err(shape.name);
        assert!(
            matches!(e, LlmError::Status { status: 429, .. }),
            "{}: {e}",
            shape.name
        );
        says(&e, "limiting", shape);
        assert_eq!(
            s.received_requests().await.unwrap().len(),
            2,
            "{}: one retry only",
            shape.name
        );
    }
}

#[tokio::test]
async fn every_shape_says_an_overloaded_provider() {
    for shape in &SHAPES {
        let busy = ResponseTemplate::new(529)
            .set_body_string(shape.error_body("overloaded_error", "Overloaded"));
        let s = server(shape, vec![busy]).await;
        let e = turn(shape, &s, 5.0).await.expect_err(shape.name);
        says(&e, "overloaded", shape);
    }
}

#[tokio::test]
async fn every_shape_says_a_timeout() {
    for shape in &SHAPES {
        let slow = stream(shape.words(&["late"])).set_delay(Duration::from_millis(1500));
        let s = server(shape, vec![slow]).await;
        let e = turn(shape, &s, 0.3).await.expect_err(shape.name);
        assert!(
            matches!(e, LlmError::Unreachable { .. }),
            "{}: {e}",
            shape.name
        );
        says(&e, "too long", shape);
    }
}

#[tokio::test]
async fn every_shape_says_a_malformed_chunk() {
    for shape in &SHAPES {
        let s = server(shape, vec![stream(shape.malformed())]).await;
        let e = turn(shape, &s, 5.0).await.expect_err(shape.name);
        assert!(matches!(e, LlmError::Stream(_)), "{}: {e}", shape.name);
        says(&e, "broke off", shape);
    }
}

#[tokio::test]
async fn every_shape_says_a_refused_key_and_never_shows_it() {
    for shape in &SHAPES {
        let refused = ResponseTemplate::new(401).set_body_string(
            shape.error_body("authentication_error", &format!("invalid key {KEY}")),
        );
        let s = server(shape, vec![refused]).await;
        let e = turn(shape, &s, 5.0).await.expect_err(shape.name);
        says(&e, "refused Ghost's key", shape);
        assert_eq!(
            s.received_requests().await.unwrap().len(),
            1,
            "{}: no retry",
            shape.name
        );
    }
}

/// The Haiku 5.5 `between_tools` 400 of 2026-11-12, on every Anthropic-protocol shape:
/// the turn is sent again without the setting, and the route remembers.
#[tokio::test]
async fn a_refused_thinking_setting_is_dropped_and_remembered() {
    for shape in SHAPES.iter().filter(|s| s.protocol == Protocol::Anthropic) {
        let refused = ResponseTemplate::new(400).set_body_string(shape.error_body(
            "invalid_request_error",
            "thinking.type: between_tools is not supported for this model",
        ));
        let s = server(
            shape,
            vec![
                refused,
                stream(shape.words(&["Fine."])),
                stream(shape.words(&["Again."])),
            ],
        )
        .await;
        let events = turn(shape, &s, 5.0).await.expect(shape.name);
        assert_eq!(text(&events), "Fine.", "{}", shape.name);
        let target = shape.target(&s, 5.0);
        assert_eq!(
            learned(&target).rejects_thinking.as_deref(),
            Some("between_tools"),
            "{}",
            shape.name
        );
        // The next turn doesn't pay for the refusal.
        let model = Model::new(target.clone()).unwrap();
        let next: Vec<_> = model
            .stream(context(), &tools(), &Options::default())
            .collect()
            .await;
        assert!(next.iter().all(Result::is_ok), "{}", shape.name);
        let bodies = common::bodies(&s).await;
        assert_eq!(bodies.len(), 3, "{}", shape.name);
        assert_eq!(
            bodies[0]["thinking"]["type"], "between_tools",
            "{}",
            shape.name
        );
        assert!(bodies[1].get("thinking").is_none(), "{}", shape.name);
        assert!(bodies[2].get("thinking").is_none(), "{}", shape.name);
        capabilities::forget(&target);
    }
}

/// A reasoning model behind a server that doesn't take `reasoning_effort`.
#[tokio::test]
async fn a_refused_reasoning_effort_is_dropped_and_remembered() {
    for shape in SHAPES.iter().filter(|s| s.protocol == Protocol::Openai) {
        let mut shape = *shape;
        shape.model = "gpt-5.1";
        let refused = ResponseTemplate::new(400).set_body_string(shape.error_body(
            "invalid_request_error",
            "Unrecognized request argument supplied: reasoning_effort",
        ));
        let s = server(&shape, vec![refused, stream(shape.words(&["Fine."]))]).await;
        let events = turn(&shape, &s, 5.0).await.expect(shape.name);
        assert_eq!(text(&events), "Fine.", "{}", shape.name);
        let bodies = common::bodies(&s).await;
        assert_eq!(bodies[0]["reasoning_effort"], "none", "{}", shape.name);
        assert!(
            bodies[1].get("reasoning_effort").is_none(),
            "{}",
            shape.name
        );
        capabilities::forget(&shape.target(&s, 5.0));
    }
}

/// A 400 about something else is the operator's to see, not a field to drop.
#[tokio::test]
async fn another_bad_request_is_not_retried() {
    let shape = &SHAPES[0];
    let refused = ResponseTemplate::new(400).set_body_string(shape.error_body(
        "invalid_request_error",
        "messages: at least one message is required",
    ));
    let s = server(shape, vec![refused]).await;
    let e = turn(shape, &s, 5.0).await.expect_err("an error");
    says(&e, "refused the request", shape);
    assert_eq!(s.received_requests().await.unwrap().len(), 1);
}
