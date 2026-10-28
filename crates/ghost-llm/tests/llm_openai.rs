//! The Chat Completions path (the port of `packages/voice/tests/test_llm_openai.py`):
//! tool calls translated both ways, every call of a reply delivered however the server
//! streams them, and a reply cut off at the output cap handled as on the Messages API
//! path. A local server streams recorded replies; nothing leaves the machine.

mod common;

use std::collections::HashMap;

use common::openai::{call, chunk, cut_reply, reply};
use common::*;
use ghost_core::connection::{ModelConnection, Protocol, REASONING_HEADROOM};
use ghost_llm::{
    CUT_OFF_LINE, CUT_OFF_NOTE, Context, Event, FinishReason, LlmError, Model, Options, ToolSchema,
    Tools, Usage,
};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn tools() -> Tools {
    Tools::new(vec![ToolSchema::new(
        "github_list_issues",
        "List issues",
        json!({"repo": {"type": "string"}}),
        vec!["repo".into()],
    )])
}

fn context() -> Context {
    Context::new("You are Ghost.").user("What's open in ghost and infra?")
}

fn options() -> Options {
    Options::default()
}

/// A model from the connection the environment describes, pointed at `base_url`.
fn connected(env: &[(&str, &str)], base_url: &str) -> Model {
    let env: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let mut t = ModelConnection::from_env(&env).target(&env);
    t.base_url = base_url.into();
    model(t)
}

// --- translation ----------------------------------------------------------------------------

#[tokio::test]
async fn test_the_request_carries_the_system_prompt_tools_and_the_cap() {
    let server = serving([reply(&[
        chunk(Some("Hi."), None, None),
        chunk(None, None, Some("stop")),
    ])])
    .await;
    let m = model(openai_target(&server.uri()));
    events(m.stream(context(), &tools(), &options())).await;
    let reqs = requests(&server).await;
    assert_eq!(reqs[0].url.path(), "/chat/completions");
    assert_eq!(
        reqs[0]
            .headers
            .get("authorization")
            .map(|v| v.to_str().unwrap()),
        Some(format!("Bearer {KEY}").as_str())
    );
    let req = &bodies(&server).await[0];
    assert_eq!(req["model"], "test-model");
    assert_eq!(req["stream"], true);
    assert_eq!(req["stream_options"], json!({"include_usage": true}));
    assert_eq!(req["max_tokens"], 2000);
    assert_eq!(
        req["messages"][0],
        json!({"role": "system", "content": "You are Ghost."})
    );
    assert_eq!(req["tools"][0]["type"], "function");
    assert_eq!(req["tools"][0]["function"]["name"], "github_list_issues");
    assert_eq!(
        req["tools"][0]["function"]["parameters"]["required"],
        json!(["repo"])
    );
    assert!(req.get("thinking").is_none() && req.get("system").is_none());
}

#[tokio::test]
async fn test_a_tool_call_and_its_result_go_back_in_openai_shape() {
    // The context keeps Pipecat's universal shape; the assistant's call and the tool's
    // answer are written as Chat Completions messages on the next request.
    let history = vec![
        json!({"role": "user", "content": "What's open in ghost?"}),
        json!({"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function",
            "function": {"name": "github_list_issues", "arguments": "{\"repo\": \"ghost\"}"}}]}),
        json!({"role": "tool", "tool_call_id": "call_1", "content": "{\"count\": 3}"}),
    ];
    let ctx = Context::from_json("You are Ghost.", &history).unwrap();
    let server = serving([reply(&[
        chunk(Some("Three."), None, None),
        chunk(None, None, Some("stop")),
    ])])
    .await;
    let evs = events(model(openai_target(&server.uri())).stream(ctx, &tools(), &options())).await;
    let sent = &bodies(&server).await[0]["messages"];
    assert_eq!(
        sent[2]["tool_calls"][0]["function"]["name"],
        "github_list_issues"
    );
    assert_eq!(
        sent[2]["tool_calls"][0]["function"]["arguments"],
        "{\"repo\":\"ghost\"}"
    );
    assert_eq!(
        sent[3],
        json!({"role": "tool", "tool_call_id": "call_1", "content": "{\"count\": 3}"})
    );
    assert_eq!(text(&evs), "Three.");
}

// --- every tool call of a reply -------------------------------------------------------------

#[tokio::test]
async fn test_every_tool_call_in_a_streamed_reply_runs() {
    let server = serving([reply(&[
        chunk(Some("Checking."), None, None),
        chunk(
            None,
            Some(vec![call(
                0,
                Some("call_a"),
                Some("github_list_issues"),
                Some(""),
            )]),
            None,
        ),
        chunk(
            None,
            Some(vec![call(0, None, None, Some(r#"{"repo": "#))]),
            None,
        ),
        chunk(
            None,
            Some(vec![call(0, None, None, Some(r#""ghost"}"#))]),
            None,
        ),
        chunk(
            None,
            Some(vec![call(
                1,
                Some("call_b"),
                Some("github_list_issues"),
                Some(r#"{"repo": "infra"}"#),
            )]),
            None,
        ),
        chunk(None, None, Some("tool_calls")),
    ])])
    .await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let calls: Vec<(String, String, Value)> = tool_calls(&evs)
        .into_iter()
        .map(|(id, name, args)| (id, name, Value::Object(args)))
        .collect();
    assert_eq!(
        calls,
        vec![
            (
                "call_a".into(),
                "github_list_issues".into(),
                json!({"repo": "ghost"})
            ),
            (
                "call_b".into(),
                "github_list_issues".into(),
                json!({"repo": "infra"})
            ),
        ]
    );
    assert_eq!(text(&evs), "Checking.");
    assert_eq!(evs.last(), Some(&Event::Finished(FinishReason::ToolUse)));
}

#[tokio::test]
async fn test_several_calls_in_one_chunk_all_run() {
    // Ollama sends a reply's calls whole, in one chunk; Pipecat read only the first.
    let server = serving([fixture("openai", "one_chunk_tool_calls")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let args: Vec<Value> = tool_calls(&evs)
        .into_iter()
        .map(|c| Value::Object(c.2))
        .collect();
    assert_eq!(
        args,
        vec![json!({"repo": "ghost"}), json!({"repo": "infra"})]
    );
    assert_eq!(tool_calls(&evs)[1].0, "call_b");
    // The recording of two calls streamed by index runs both too.
    let server = serving([fixture("openai", "two_tool_calls")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert_eq!(tool_calls(&evs).len(), 2);
    let server = serving([fixture("openai", "one_tool_call")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert_eq!(tool_calls(&evs).len(), 1);
    assert_eq!(tool_calls(&evs)[0].2["repo"], "ghost");
}

#[tokio::test]
async fn test_a_server_without_call_ids_gets_stable_ones() {
    let server = serving([reply(&[
        chunk(
            None,
            Some(vec![call(
                0,
                None,
                Some("github_list_issues"),
                Some(r#"{"repo": "ghost"}"#),
            )]),
            None,
        ),
        chunk(None, None, Some("tool_calls")),
    ])])
    .await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let calls = tool_calls(&evs);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].0.as_str(), calls[0].1.as_str()),
        ("call_0", "github_list_issues")
    );
    assert_eq!(calls[0].2["repo"], "ghost");
}

#[tokio::test]
async fn test_a_name_repeated_in_every_chunk_is_not_doubled() {
    let server = serving([reply(&[
        chunk(
            None,
            Some(vec![call(
                0,
                Some("c"),
                Some("github_list_issues"),
                Some(r#"{"repo": "#),
            )]),
            None,
        ),
        chunk(
            None,
            Some(vec![call(
                0,
                None,
                Some("github_list_issues"),
                Some(r#""ghost"}"#),
            )]),
            None,
        ),
        chunk(None, None, Some("tool_calls")),
    ])])
    .await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let calls = tool_calls(&evs);
    assert_eq!(
        (calls[0].0.as_str(), calls[0].1.as_str()),
        ("c", "github_list_issues")
    );
    assert_eq!(calls[0].2["repo"], "ghost");
}

#[tokio::test]
async fn test_an_answer_without_tools_is_untouched() {
    let server = serving([fixture("openai", "plain")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(text(&evs), "Three open.");
    assert!(cut_offs(&evs).is_empty());
    assert_eq!(evs.last(), Some(&Event::Finished(FinishReason::End)));
}

#[tokio::test]
async fn test_a_call_whose_arguments_arent_json_is_dropped_not_cut() {
    let server = serving([reply(&[
        chunk(
            None,
            Some(vec![call(
                0,
                Some("a"),
                Some("github_list_issues"),
                Some("not json"),
            )]),
            None,
        ),
        chunk(
            None,
            Some(vec![call(
                1,
                Some("b"),
                Some("github_list_issues"),
                Some(r#"{"repo": "ghost"}"#),
            )]),
            None,
        ),
        chunk(None, None, Some("tool_calls")),
    ])])
    .await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert_eq!(tool_calls(&evs).len(), 1);
    assert_eq!(tool_calls(&evs)[0].0, "b");
    assert!(cut_offs(&evs).is_empty());
}

// --- a reply cut off at the output cap ------------------------------------------------------

#[tokio::test]
async fn test_a_reply_cut_off_inside_its_tool_calls_is_redone_in_smaller_steps() {
    let whole = reply(&[
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
        chunk(None, None, Some("tool_calls")),
    ]);
    let server = serving([fixture("openai", "cut_mid_arguments"), whole]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let k = kinds(&evs);
    let redo = index_of(&k, "Redo");
    assert!(!k[..redo].contains(&"ToolCall")); // half a batch is not run; Pipecat alone would have run two
    assert_eq!(cut_offs(&evs), vec![3]);
    // The reply's end first, so the caller closes it before the note lands.
    assert!(index_of(&k, "Finished") < index_of(&k, "CutOff") && index_of(&k, "CutOff") < redo);
    assert_eq!(tool_calls(&evs[redo..]).len(), 1);
    assert!(said(&evs).is_empty());
    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 2);
    let last = sent[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last, json!({"role": "user", "content": CUT_OFF_NOTE}));
}

#[tokio::test]
async fn test_a_stream_that_ends_inside_a_call_without_a_reason_is_cut() {
    let partial = chunk(
        None,
        Some(vec![call(0, Some("t1"), Some("change"), Some(r#"{"n": "#))]),
        None,
    );
    let whole = reply(&[
        chunk(
            None,
            Some(vec![call(
                0,
                Some("t1"),
                Some("change"),
                Some(r#"{"n": 1}"#),
            )]),
            None,
        ),
        chunk(None, None, Some("tool_calls")),
    ]);
    let server = serving([partial, whole]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert_eq!(cut_offs(&evs), vec![1]);
    assert!(kinds(&evs).contains(&"Redo"));
    assert_eq!(tool_calls(&evs).len(), 1);
}

#[tokio::test]
async fn test_a_redo_cut_off_again_is_said_not_retried() {
    let server = serving([cut_reply(), cut_reply()]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(cut_offs(&evs), vec![3, 3]);
    assert_eq!(kinds(&evs).iter().filter(|k| **k == "Redo").count(), 1);
    assert_eq!(said(&evs), vec![CUT_OFF_LINE.to_string()]);
    assert_eq!(bodies(&server).await.len(), 2);
}

#[tokio::test]
async fn test_a_long_answer_cut_short_is_only_flagged() {
    let server = serving([fixture("openai", "length_text")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    assert!(tool_calls(&evs).is_empty());
    assert_eq!(cut_offs(&evs), vec![0]);
    assert_eq!(text(&evs), "Fifty-four open issues. The first");
    let k = kinds(&evs);
    assert!(!k.contains(&"Redo") && !k.contains(&"Say"));
    assert_eq!(bodies(&server).await.len(), 1);
}

#[tokio::test]
async fn test_an_error_object_in_the_stream_is_an_error() {
    let server = serving([format!(
        "data: {}\n\n",
        json!({"error": {"message": "model 'x' not found", "type": "invalid_request_error"}})
    )])
    .await;
    let err =
        first_error(model(openai_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert_eq!(
        err,
        LlmError::Api {
            kind: "invalid_request_error".into(),
            message: "model 'x' not found".into()
        }
    );
}

#[tokio::test]
async fn test_a_refusal_names_the_status_not_the_key() {
    let body = json!({"error": {"message": format!("Incorrect API key provided: {KEY}"), "type": "invalid_request_error"}});
    let server = refusing(401, &body.to_string()).await;
    let err =
        first_error(model(openai_target(&server.uri())).stream(context(), &tools(), &options()))
            .await;
    assert_eq!(
        err.to_string(),
        "HTTP 401: Incorrect API key provided: [password]"
    );
}

// --- the interface ----------------------------------------------------------------------

#[tokio::test]
async fn test_the_protocol_picks_the_client() {
    let server = serving([fixture("openai", "plain")]).await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_MODEL", "qwen3:8b"),
        ],
        &server.uri(),
    );
    assert_eq!(m.protocol(), Protocol::Openai);
    assert_eq!(m.target().model, "qwen3:8b");
    events(m.stream(context(), &tools(), &options())).await;
    let reqs = requests(&server).await;
    assert_eq!(reqs[0].url.path(), "/chat/completions");
    assert_eq!(reqs[0].headers.get("authorization"), None); // Ollama takes no key
    let req = &bodies(&server).await[0];
    assert_eq!(req["max_tokens"], 2000);
    assert_eq!(req["model"], "qwen3:8b");
}

#[tokio::test]
async fn test_openai_itself_gets_its_cap_field() {
    let server = serving([fixture("openai", "plain")]).await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-4.1"),
            ("OPENAI_API_KEY", "s"),
        ],
        &server.uri(),
    );
    events(m.stream(context(), &tools(), &options())).await;
    let req = &bodies(&server).await[0];
    assert_eq!(req["max_completion_tokens"], 2000);
    assert!(req.get("max_tokens").is_none());
}

#[tokio::test]
async fn test_a_reasoning_model_is_asked_for_its_lowest_effort_with_room_to_reason() {
    // The request for a session on gpt-5.5 at OpenAI. Untested live.
    let server = serving([fixture("openai", "plain")]).await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "gpt-5.5"),
            ("OPENAI_API_KEY", "k"),
        ],
        &server.uri(),
    );
    events(m.stream(context(), &tools(), &options())).await;
    let req = &bodies(&server).await[0];
    assert_eq!(req["reasoning_effort"], "none");
    assert_eq!(req["max_completion_tokens"], 2000 + REASONING_HEADROOM);
    assert!(req.get("max_tokens").is_none());
}

#[tokio::test]
async fn test_a_one_shot_and_a_probe_carry_the_effort_too() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hi."}, "finish_reason": "stop"}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(Replies::new([fixture("openai", "plain")]))
        .mount(&server)
        .await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "openai"),
            ("GHOST_LLM_MODEL", "o4-mini"),
            ("OPENAI_API_KEY", "k"),
        ],
        &server.uri(),
    );
    assert_eq!(m.complete("Be brief.", "Hello?", 16).await.unwrap(), "Hi.");
    let p = m.probe("Say hi.", 16, None).await;
    assert_eq!(p.error, None);
    let sent = bodies(&server).await;
    for req in &sent {
        assert_eq!(req["reasoning_effort"], "low");
        assert_eq!(req["max_completion_tokens"], 16 + REASONING_HEADROOM);
        assert!(req.get("max_tokens").is_none());
    }
    assert_eq!(
        sent[0]["messages"],
        json!([{"role": "system", "content": "Be brief."}, {"role": "user", "content": "Hello?"}])
    );
    assert!(sent[0].get("stream").is_none());
    assert_eq!(sent[1]["stream"], true);
}

#[tokio::test]
async fn test_a_model_that_doesnt_reason_gets_no_effort() {
    let server = serving([fixture("openai", "plain")]).await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_MODEL", "qwen3:8b"),
        ],
        &server.uri(),
    );
    events(m.stream(context(), &tools(), &options())).await;
    let req = &bodies(&server).await[0];
    assert!(req.get("reasoning_effort").is_none());
    assert_eq!(req["max_tokens"], 2000);
}

#[tokio::test]
async fn test_the_probe_times_the_first_token() {
    let server = serving([reply(&[
        chunk(Some("Hello"), None, None),
        chunk(Some(" there."), None, None),
        chunk(None, None, Some("stop")),
    ])])
    .await;
    let m = connected(
        &[
            ("GHOST_LLM_PROVIDER", "ollama"),
            ("GHOST_LLM_MODEL", "qwen3:8b"),
        ],
        &server.uri(),
    );
    let p = m.probe("Say hi.", 16, None).await;
    assert_eq!(p.error, None);
    assert_eq!(p.text, "Hello there.");
    assert_eq!(p.model, "test-model");
    let (ttft, total) = (p.ttft.unwrap(), p.total.unwrap());
    assert!(ttft <= total);
    let req = &bodies(&server).await[0];
    assert_eq!(req["max_tokens"], 16);
    assert_eq!(
        req["messages"],
        json!([{"role": "user", "content": "Say hi."}])
    );
}

#[tokio::test]
async fn test_usage_carries_the_cached_prompt_tokens() {
    let server = serving([fixture("openai", "usage_cached")]).await;
    let evs =
        events(model(openai_target(&server.uri())).stream(context(), &tools(), &options())).await;
    let usage = evs.iter().find_map(|e| match e {
        Event::Usage(u) => Some(u.clone()),
        _ => None,
    });
    assert_eq!(
        usage,
        Some(Usage {
            input_tokens: 3000,
            output_tokens: 12,
            cache_read_tokens: Some(2816),
            cache_write_tokens: None,
            reasoning_tokens: Some(0),
        })
    );
}
