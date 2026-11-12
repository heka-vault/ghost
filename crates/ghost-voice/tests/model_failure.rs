//! A spoken turn whose model call fails is said, never silent (ADR 0055).
//!
//! Before, the stage logged the error and pushed `Frame::Error`, which no stage reads: the
//! operator heard nothing, as if Ghost hadn't heard them. Now the stage says what failed in
//! a plain sentence (`ghost_llm::spoken_failure`) as the turn's reply. The model here is the
//! real client against a local server that refuses; the words are synthetic.

use std::sync::{Arc, Mutex};

use ghost_core::connection::{Protocol, Target};
use ghost_core::{Store, WriteGate};
use ghost_llm::{Context, Model, Options, Tools};
use ghost_voice::frames::{Direction, Frame, Out, Stage};
use ghost_voice::intercept::no_action;
use ghost_voice::llm::LlmStage;
use ghost_voice::tools::{ToolDispatcher, ToolsParams};
use ghost_voice::turns::TurnControl;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroize::Zeroizing;

async fn spoken_after(status: u16, body: &str) -> (Vec<String>, Vec<String>) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(status)
                .insert_header("retry-after", "0")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let target = Target {
        provider: "anthropic".into(),
        protocol: Protocol::Anthropic,
        base_url: server.uri(),
        model: "claude-sonnet-5-5".into(),
        api_key: Zeroizing::new("sk-ant-test-0123456789".into()),
        headers: Vec::new(),
        timeout_secs: Some(5.0),
        cap_param: "max_tokens",
        reasoning: Default::default(),
    };
    let model = Arc::new(Model::new(target).unwrap());
    let store = Arc::new(Store::in_memory().unwrap());
    let turns = TurnControl::new(CancellationToken::new());
    let dispatcher = ToolDispatcher::new(ToolsParams {
        sources: Vec::new(),
        gate: Arc::new(WriteGate::new(store)),
        session_id: "s1".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: Some(turns.clone()),
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    });
    let context = Arc::new(Mutex::new(Context::new("You are under test.")));
    let mut stage = LlmStage::new(
        model,
        context,
        Tools::default(),
        Options::default(),
        dispatcher,
        turns.clone(),
        Arc::new(|_frames| Box::pin(async {})),
        None,
    );
    let (out, mut frames) = Out::capture();
    turns.interrupt();
    stage
        .process(
            Frame::user_text("What is on today?", true),
            Direction::Down,
            &out,
        )
        .await;
    drop(out);
    let mut said = Vec::new();
    let mut errors = Vec::new();
    while let Ok(p) = frames.try_recv() {
        match p.frame {
            Frame::LlmText(t) => said.push(t),
            Frame::Error(e) => errors.push(e),
            _ => {}
        }
    }
    (said, errors)
}

#[tokio::test]
async fn a_rate_limited_turn_says_so() {
    let (said, errors) = spoken_after(
        429,
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
    )
    .await;
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("limiting how fast"), "{said:?}");
    assert_eq!(errors.len(), 1);
}

#[tokio::test]
async fn a_refused_key_is_said_without_the_key() {
    let (said, _) = spoken_after(
        401,
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key sk-ant-test-0123456789"}}"#,
    )
    .await;
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("refused Ghost's key"), "{said:?}");
    assert!(!said[0].contains("sk-ant"), "{said:?}");
}
