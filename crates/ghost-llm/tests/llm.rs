//! The Messages API path (the port of `packages/voice/tests/test_llm.py`'s client tests):
//! which address a call goes to, that keys never show, the system-prompt cache marker, the
//! one client per connection, every tool call of a reply, and a reply cut off at the
//! output cap. Nothing leaves the machine: a local server streams recorded replies.

mod common;

use common::anthropic::{events as reply, text as text_block, tool};
use common::*;
use ghost_core::connection::Protocol;
use ghost_llm::{
    CUT_OFF_LINE, CUT_OFF_NOTE, Context, Event, FinishReason, LLM_MAX_TOKENS, LlmError, Message,
    Options, ToolSchema, Tools, Usage, shared, with_system_breakpoint,
};
use serde_json::{Map, Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn context() -> Context {
    let mut ctx = Context::new("You are Ghost.").user("What needs my sign-off?");
    ctx.push(Message::assistant("Three."));
    ctx.push(Message::user("And the first one?"));
    ctx
}

fn tools() -> Tools {
    Tools::new(vec![ToolSchema::new("get_issue", "d", json!({}), vec![])])
}

fn options() -> Options {
    Options::default()
}

/// Cache markers in a request: on the system blocks and on message content blocks.
fn markers(params: &Value) -> usize {
    let mut n = params["system"]
        .as_array()
        .map(|b| {
            b.iter()
                .filter(|x| x.get("cache_control").is_some())
                .count()
        })
        .unwrap_or(0);
    for m in params["messages"].as_array().unwrap() {
        if let Some(blocks) = m["content"].as_array() {
            n += blocks
                .iter()
                .filter(|x| x.get("cache_control").is_some())
                .count();
        }
    }
    n
}

// --- where a call goes, and that keys never show ----------------------------------------

#[tokio::test]
async fn test_keys_never_show() {
    let body = json!({"type": "error", "error": {"type": "authentication_error",
        "message": format!("invalid x-api-key {KEY} for this request")}});
    let server = refusing(401, &body.to_string()).await;
    let m = model(anthropic_target(&server.uri()));
    assert!(!format!("{m:?}").contains(KEY));
    assert!(format!("{m:?}").contains("claude-sonnet-5-5"));
    let err = first_error(m.stream(context(), &tools(), &options())).await;
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains(KEY), "{shown}");
    assert!(shown.starts_with("HTTP 401: invalid x-api-key [password] for this request"));
    let p = m.probe("Say hi.", 16, None).await;
    let error = p.error.expect("the probe reports the refusal");
    assert!(
        !error.contains(KEY) && error.starts_with("HTTP 401"),
        "{error}"
    );
    assert_eq!(p.ttft, None);
}

#[tokio::test]
async fn test_the_anthropic_client_never_falls_back_to_the_environment() {
    // ANTHROPIC_BASE_URL points at a proxy in an older setup; the target's address is
    // what is asked, and only that.
    let server = serving([fixture("anthropic", "plain")]).await;
    let m = model(anthropic_target(&server.uri()));
    events(m.stream(context(), &tools(), &options())).await;
    let reqs = requests(&server).await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url.path(), "/v1/messages");
    // The one request reached the target's own address: the mock server recorded it.
}

#[tokio::test]
async fn test_the_request_carries_the_key_the_version_and_the_beta() {
    let server = serving([
        fixture("anthropic", "plain"),
        fixture("anthropic", "plain"),
        fixture("anthropic", "plain"),
    ])
    .await;
    let mut t = anthropic_target(&server.uri());
    t.headers = vec![("X-Title".into(), "Ghost".into())];
    events(model(t.clone()).stream(context(), &tools(), &options())).await;
    t.provider = "openrouter".into();
    events(model(t.clone()).stream(context(), &tools(), &options())).await;
    t.api_key = Default::default();
    events(model(t).stream(context(), &tools(), &options())).await;
    let reqs = requests(&server).await;
    let header = |i: usize, name: &str| {
        reqs[i]
            .headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    };
    assert_eq!(header(0, "x-api-key").as_deref(), Some(KEY));
    assert_eq!(
        header(0, "anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    assert_eq!(
        header(0, "anthropic-beta").as_deref(),
        Some("interleaved-thinking-2025-05-14")
    );
    assert_eq!(header(0, "x-title").as_deref(), Some("Ghost"));
    assert_eq!(header(0, "authorization"), None); // Anthropic itself: the key header only
    assert_eq!(header(1, "x-api-key").as_deref(), Some(KEY));
    assert_eq!(header(1, "authorization"), Some(format!("Bearer {KEY}"))); // a proxy, OpenRouter
    assert_eq!(header(2, "x-api-key"), None); // no key: nothing sent for one
    assert_eq!(header(2, "authorization"), None);
}

#[tokio::test]
async fn test_a_refusal_keeps_the_first_line_redacted() {
    let body = format!(
        "Upstream refused the key {KEY}\nTraceback (most recent call last):\n  secret stack"
    );
    let server = refusing(502, &body).await;
    let m = model(anthropic_target(&server.uri()));
    let err = first_error(m.stream(context(), &tools(), &options())).await;
    assert_eq!(
        err,
        LlmError::Status {
            status: 502,
            detail: "Upstream refused the key [password]".into()
        }
    );
    assert!(!err.to_string().contains("stack"));
}

#[tokio::test]
async fn test_an_error_event_in_the_stream_is_an_error() {
    let stream = sse(
        "message_start",
        &json!({"type": "message_start", "message": {"model": "m", "usage": {"input_tokens": 1}}}),
    ) + &sse(
        "error",
        &json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    );
    let server = serving([stream]).await;
    let err =
        first_error(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert_eq!(
        err,
        LlmError::Api {
            kind: "overloaded_error".into(),
            message: "Overloaded".into()
        }
    );
}

#[tokio::test]
async fn test_an_unreachable_address_names_its_kind_not_its_text() {
    let m = model(anthropic_target("http://127.0.0.1:9")); // nothing listens on 9
    let err = first_error(m.stream(context(), &tools(), &options())).await;
    assert!(
        matches!(&err, LlmError::Unreachable { host, kind: "ConnectError" } if host == "http://127.0.0.1:9"),
        "{err:?}"
    );
}

// --- the system-prompt cache marker ----------------------------------------------------

#[test]
fn test_the_system_prompt_carries_a_cache_marker() {
    let params =
        model(anthropic_target("http://localhost:1")).body(&context(), &tools(), &options());
    assert_eq!(
        params["system"],
        json!([{"type": "text", "text": "You are Ghost.", "cache_control": {"type": "ephemeral"}}])
    );
    // Pipecat's own two markers stay on the last two user messages: three of four.
    assert_eq!(markers(&params), 3);
    assert_eq!(params["tools"][0]["name"], "get_issue"); // tools render before system: cached too
    assert_eq!(params["thinking"], json!({"type": "between_tools"}));
    assert_eq!(params["max_tokens"], 2000);
    assert_eq!(params["stream"], true);
    assert_eq!(params["model"], "claude-sonnet-5-5");
}

#[test]
fn test_without_the_marker_the_system_prompt_is_a_string() {
    let options = Options {
        cache_system: false,
        ..Options::default()
    };
    let params = model(anthropic_target("http://localhost:1")).body(&context(), &tools(), &options);
    assert_eq!(params["system"], "You are Ghost.");
    assert_eq!(markers(&params), 2);
}

#[test]
fn test_with_system_breakpoint_leaves_other_shapes_alone() {
    let mut empty: Map<String, Value> = Map::new();
    empty.insert("system".into(), json!(""));
    with_system_breakpoint(&mut empty);
    assert_eq!(empty["system"], json!(""));
    let blocks = json!([{"type": "text", "text": "x"}]);
    let mut given: Map<String, Value> = Map::new();
    given.insert("system".into(), blocks.clone());
    with_system_breakpoint(&mut given);
    assert_eq!(given["system"], blocks);
    let mut none: Map<String, Value> = Map::new();
    with_system_breakpoint(&mut none);
    assert!(none.get("system").is_none());
}

#[test]
fn test_a_trailing_assistant_turn_gets_a_user_turn_for_a_model_without_prefill() {
    let mut ctx = Context::new("S").user("Q");
    ctx.push(Message::assistant("A"));
    let m = model(anthropic_target("http://localhost:1"));
    let params = m.body(&ctx, &tools(), &options());
    let last = params["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["text"], ".");
    let mut t = anthropic_target("http://localhost:1");
    t.model = "claude-sonnet-4-5".into();
    let params = model(t).body(&ctx, &tools(), &options());
    assert_eq!(params["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn test_one_client_per_endpoint_and_key() {
    let t = anthropic_target("http://localhost:1");
    let a = shared(&t).unwrap();
    let b = shared(&t).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    let other = anthropic_target("http://localhost:2");
    assert!(!std::sync::Arc::ptr_eq(&a, &shared(&other).unwrap()));
    let mut key = t.clone();
    key.api_key = zeroize::Zeroizing::new("sk-other-9876543210".into());
    assert!(!std::sync::Arc::ptr_eq(&a, &shared(&key).unwrap()));
}

// --- every tool call of a reply, and a reply cut off at the output cap --------------------

#[tokio::test]
async fn test_every_tool_call_in_a_reply_runs() {
    // The prompt asks for independent lookups together; Pipecat alone ran only the last.
    let server = serving([reply(
        &[
            text_block("Checking."),
            tool("list_issues", &[r#"{"repo": "#, r#""ghost"}"#], "t1"),
            tool("list_issues", &[r#"{"repo": "ecosystem"}"#], "t2"),
            tool("issue_read", &[], "t3"),
        ],
        "tool_use",
    )])
    .await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    let calls: Vec<(String, String, Value)> = tool_calls(&evs)
        .into_iter()
        .map(|(id, name, args)| (id, name, Value::Object(args)))
        .collect();
    assert_eq!(
        calls,
        vec![
            ("t1".into(), "list_issues".into(), json!({"repo": "ghost"})),
            (
                "t2".into(),
                "list_issues".into(),
                json!({"repo": "ecosystem"})
            ),
            ("t3".into(), "issue_read".into(), json!({})),
        ]
    );
    assert_eq!(text(&evs), "Checking.");
    assert_eq!(evs.last(), Some(&Event::Finished(FinishReason::ToolUse)));
    assert!(cut_offs(&evs).is_empty());
}

#[tokio::test]
async fn test_one_tool_call_runs_as_pipecat_hands_it_over() {
    let server = serving([fixture("anthropic", "one_tool_call")]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    let calls = tool_calls(&evs);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].0.as_str(), calls[0].1.as_str()),
        ("t1", "issue_read")
    );
    assert_eq!(calls[0].2["n"], 8);
    // Two lookups in one reply, from the recording.
    let server = serving([fixture("anthropic", "two_tool_calls")]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    let calls = tool_calls(&evs);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].2["repo"], "ghost");
    assert_eq!(calls[1].2["repo"], "infra");
    assert_eq!(text(&evs), "Checking.");
}

#[tokio::test]
async fn test_an_answer_without_tools_is_untouched() {
    let server = serving([fixture("anthropic", "plain")]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(text(&evs), "Three open.");
    assert!(cut_offs(&evs).is_empty());
    assert_eq!(kinds(&evs), vec!["Text", "Text", "Usage", "Finished"]);
    assert_eq!(evs.last(), Some(&Event::Finished(FinishReason::End)));
}

#[tokio::test]
async fn test_a_reply_cut_off_inside_its_tool_calls_is_redone_in_smaller_steps() {
    // The car on 2026-10-24: four comments in one reply at a 400-token cap. Two complete,
    // the third cut mid-arguments; Pipecat saw `max_tokens` and ran none, silently.
    let whole = reply(
        &[tool("change", &[r#"{"op": "comment", "n": 390}"#], "t1")],
        "tool_use",
    );
    let server = serving([fixture("anthropic", "cut_mid_arguments"), whole]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    let k = kinds(&evs);
    let redo = index_of(&k, "Redo");
    assert!(!k[..redo].contains(&"ToolCall")); // half a batch is not run
    assert_eq!(cut_offs(&evs), vec![3]);
    assert!(index_of(&k, "Finished") < index_of(&k, "CutOff") && index_of(&k, "CutOff") < redo);
    // The redo asked for one comment, and that one is delivered.
    assert_eq!(tool_calls(&evs[redo..]).len(), 1);
    assert_eq!(evs.last(), Some(&Event::Finished(FinishReason::ToolUse)));
    assert!(said(&evs).is_empty());
    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 2);
    let last = sent[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "user");
    let note = last["content"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(note["text"], CUT_OFF_NOTE);
    assert!(CUT_OFF_NOTE.contains("smaller steps"));
}

#[tokio::test]
async fn test_a_redo_cut_off_again_is_said_not_retried() {
    let cut = || {
        reply(
            &[
                tool("change", &[r#"{"n": 1}"#], "t1"),
                tool("change", &[r#"{"n": "#], "t2"),
            ],
            "max_tokens",
        )
    };
    let server = serving([cut(), cut()]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert_eq!(
        kinds(&evs)
            .iter()
            .filter(|k| !matches!(**k, "Usage"))
            .copied()
            .collect::<Vec<_>>(),
        vec!["Finished", "CutOff", "Redo", "Finished", "CutOff", "Say"]
    );
    assert_eq!(cut_offs(&evs), vec![2, 2]);
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(said(&evs), vec![CUT_OFF_LINE.to_string()]);
    assert_eq!(bodies(&server).await.len(), 2);
}

#[tokio::test]
async fn test_a_redo_that_completes_lets_the_next_cut_be_redone() {
    let cut = || reply(&[tool("change", &[r#"{"n": "#], "t1")], "max_tokens");
    let whole = || reply(&[tool("change", &[r#"{"n": 1}"#], "t1")], "tool_use");
    let server = serving([cut(), whole(), cut(), whole()]).await;
    let m = model(anthropic_target(&server.uri()));
    let first = events(m.stream(context(), &tools(), &options())).await;
    assert_eq!(kinds(&first).iter().filter(|k| **k == "Redo").count(), 1);
    assert!(said(&first).is_empty());
    let second = events(m.stream(context(), &tools(), &options())).await;
    assert_eq!(kinds(&second).iter().filter(|k| **k == "Redo").count(), 1);
    assert!(said(&second).is_empty());
    assert_eq!(tool_calls(&second).len(), 1);
    assert_eq!(bodies(&server).await.len(), 4);
}

#[tokio::test]
async fn test_a_long_answer_cut_short_is_only_flagged() {
    let server = serving([fixture("anthropic", "max_tokens_text")]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(cut_offs(&evs), vec![0]);
    assert_eq!(text(&evs), "Fifty-four open issues. The first");
    let k = kinds(&evs);
    assert!(!k.contains(&"Redo") && !k.contains(&"Say"));
    assert_eq!(bodies(&server).await.len(), 1);
}

#[tokio::test]
async fn test_a_stream_that_ends_inside_a_block_is_cut() {
    // The connection dropped mid-arguments: no stop, no message_delta.
    let partial = sse(
        "message_start",
        &json!({"type": "message_start", "message": {"model": "m", "usage": {"input_tokens": 1, "output_tokens": 1}}}),
    ) + &sse(
        "content_block_start",
        &json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "t1", "name": "change", "input": {}}}),
    ) + &sse(
        "content_block_delta",
        &json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"n\": "}}),
    );
    let whole = reply(&[tool("change", &[r#"{"n": 1}"#], "t1")], "tool_use");
    let server = serving([partial, whole]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert_eq!(cut_offs(&evs), vec![1]);
    let k = kinds(&evs);
    assert!(k.contains(&"Redo"));
    assert_eq!(tool_calls(&evs).len(), 1);
}

#[tokio::test]
async fn test_a_cut_replys_text_goes_back_as_the_assistants_turn() {
    let cut = reply(
        &[text_block("On it."), tool("change", &[r#"{"n": "#], "t1")],
        "max_tokens",
    );
    let whole = reply(&[tool("change", &[r#"{"n": 1}"#], "t1")], "tool_use");
    let server = serving([cut, whole]).await;
    events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let sent = bodies(&server).await;
    let messages = sent[1]["messages"].as_array().unwrap();
    let n = messages.len();
    assert_eq!(messages[n - 2]["role"], "assistant");
    assert_eq!(messages[n - 2]["content"], "On it.");
    assert_eq!(messages[n - 1]["content"][0]["text"], CUT_OFF_NOTE);
}

#[test]
fn test_the_output_cap_leaves_room_for_a_batch_of_tool_calls() {
    const _: () = assert!(LLM_MAX_TOKENS >= 1500);
    assert_eq!(Options::default().max_tokens, LLM_MAX_TOKENS);
}

#[tokio::test]
async fn test_usage_counts_cache_reads_and_output_once() {
    let server = serving([fixture("anthropic", "usage_cache_reads")]).await;
    let evs =
        events(model(anthropic_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    let usage = evs.iter().find_map(|e| match e {
        Event::Usage(u) => Some(u.clone()),
        _ => None,
    });
    assert_eq!(
        usage,
        Some(Usage {
            input_tokens: 12,
            output_tokens: 7, // the final count, not message_start's 1 added to it
            cache_read_tokens: Some(2900),
            cache_write_tokens: Some(0),
            reasoning_tokens: None,
        })
    );
}

// --- the interface ----------------------------------------------------------------------

#[tokio::test]
async fn test_the_probe_times_the_first_token() {
    let server = serving([fixture("anthropic", "plain"), fixture("anthropic", "plain")]).await;
    let m = model(anthropic_target(&server.uri()));
    let p = m.probe("Say hi.", 16, None).await;
    assert_eq!(p.error, None);
    assert_eq!(p.text, "Three open.");
    assert_eq!(p.model, "claude-sonnet-5-5");
    let (ttft, total) = (p.ttft.unwrap(), p.total.unwrap());
    assert!(ttft <= total);
    let p = m.probe("Say hi.", 16, Some("between_tools")).await;
    assert_eq!(p.error, None);
    let sent = bodies(&server).await;
    assert_eq!(sent[0]["max_tokens"], 16);
    assert_eq!(sent[0]["stream"], true);
    assert!(sent[0].get("thinking").is_none() && sent[0].get("system").is_none());
    assert_eq!(
        sent[0]["messages"],
        json!([{"role": "user", "content": "Say hi."}])
    );
    assert_eq!(sent[1]["thinking"], json!({"type": "between_tools"}));
}

#[tokio::test]
async fn test_complete_returns_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5",
            "content": [{"type": "text", "text": "Hi "}, {"type": "text", "text": "there."}],
            "stop_reason": "end_turn", "usage": {"input_tokens": 5, "output_tokens": 3}
        })))
        .mount(&server)
        .await;
    let m = model(anthropic_target(&server.uri()));
    assert_eq!(
        m.complete("Be brief.", "Hello?", 64).await.unwrap(),
        "Hi there."
    );
    let sent = bodies(&server).await;
    assert_eq!(sent[0]["system"], "Be brief.");
    assert_eq!(sent[0]["max_tokens"], 64);
    assert_eq!(
        sent[0]["messages"],
        json!([{"role": "user", "content": "Hello?"}])
    );
    assert!(sent[0].get("stream").is_none());
    assert_eq!(m.protocol(), Protocol::Anthropic);
}
