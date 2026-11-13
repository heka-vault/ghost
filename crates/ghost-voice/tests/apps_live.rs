//! A running session keeps up with the apps (ADR 0046): the tool list the model is sent
//! follows an app that connects, changes or leaves from the next model call, and a call
//! to an app stops when the operator speaks or hangs up, so the model's stage is never
//! held by an app that takes minutes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use ghost_core::apps::{App, AppSpec, Apps, Session, SessionError};
use ghost_core::conversations::{Conversations, ConversationsOptions, GHOST, Profile};
use ghost_core::source::BoxFuture;
use ghost_core::{Store, WriteGate};
use ghost_llm::{Context, Event, LlmError, Message, Options, Tools};
use ghost_voice::conversation::ConversationRuntime;
use ghost_voice::frames::{Direction, Frame, Out, Stage};
use ghost_voice::intercept::no_action;
use ghost_voice::llm::{LlmStage, ModelPort, ScriptedModel, ScriptedReply};
use ghost_voice::tools::{ToolDispatcher, ToolsParams};
use ghost_voice::turns::TurnControl;
use rmcp::model::{CallToolResult, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// The scripted model, remembering the tools and the prompt each call was sent.
struct Recording {
    inner: Arc<ScriptedModel>,
    seen: Mutex<Vec<(Vec<String>, String)>>,
}

impl ModelPort for Recording {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        self.seen.lock().unwrap().push((
            tools.names().into_iter().map(String::from).collect(),
            context.system.clone(),
        ));
        self.inner.stream(context, tools, options)
    }

    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        self.inner.complete(system, user, max_tokens)
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

/// An app in process: its tools as given, `slow` never answering until it is dropped.
struct Notes {
    tools: Mutex<Vec<Tool>>,
    started: Mutex<Vec<String>>,
    dropped: Arc<Mutex<Vec<String>>>,
}

struct Dropped(String, Arc<Mutex<Vec<String>>>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.1.lock().unwrap().push(self.0.clone());
    }
}

#[async_trait]
impl Session for Notes {
    async fn list_tools(&self) -> Result<Vec<Tool>, SessionError> {
        Ok(self.tools.lock().unwrap().clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        _arguments: Map<String, Value>,
    ) -> Result<CallToolResult, SessionError> {
        self.started.lock().unwrap().push(name.to_string());
        if name == "slow" {
            // What `rmcp` does when this future is dropped is tested in ghost-core
            // (`notifications/cancelled`); here the drop itself is the evidence.
            let _told = Dropped(name.to_string(), self.dropped.clone());
            std::future::pending::<()>().await;
        }
        Ok(CallToolResult::structured(json!({"ok": true})))
    }

    async fn close(&self) {}
}

fn tool(name: &str) -> Tool {
    let schema = json!({"type": "object", "properties": {}});
    Tool::new(
        name.to_string(),
        format!("Do {name}."),
        Arc::new(schema.as_object().unwrap().clone()),
    )
    .annotate(ToolAnnotations::new().read_only(true))
}

async fn notes(tools: &[&str]) -> (Arc<App>, Arc<Notes>) {
    let session = Arc::new(Notes {
        tools: Mutex::new(tools.iter().map(|t| tool(t)).collect()),
        started: Mutex::new(Vec::new()),
        dropped: Arc::new(Mutex::new(Vec::new())),
    });
    let app = Arc::new(App::new(
        AppSpec {
            title: "Notes".into(),
            ..AppSpec::at("notes", "memory://")
        },
        Arc::new(std::collections::HashMap::<String, String>::new()),
        None,
    ));
    app.attach(session.clone()).await.unwrap();
    (app, session)
}

struct Rig {
    stage: LlmStage,
    model: Arc<Recording>,
    apps: Arc<Apps>,
    turns: Arc<TurnControl>,
    context: ghost_voice::llm::SharedContext,
    out: Out,
    _frames: tokio::sync::mpsc::Receiver<ghost_voice::Pushed>,
    _tmp: tempfile::TempDir,
}

fn rig(script: Vec<Vec<ScriptedReply>>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("ghost");
    std::fs::create_dir_all(&cfg).unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let convs = Arc::new(
        Conversations::new(
            store.clone(),
            &cfg,
            ConversationsOptions {
                default_voice: "af_heart".into(),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let gate = Arc::new(WriteGate::new(store));
    let apps = Arc::new(Apps::new(
        Vec::new(),
        Arc::new(std::collections::HashMap::<String, String>::new()),
        None,
    ));
    // What the daemon gives a session: the apps as they are when asked.
    let reach = {
        let apps = apps.clone();
        Arc::new(move |_p: &Profile| apps.sources())
    };
    let rt = ConversationRuntime::new(
        convs,
        gate.clone(),
        "s1",
        Vec::new(),
        24000,
        20,
        GHOST,
        None,
        Some(reach),
    );
    let turns = TurnControl::new(CancellationToken::new());
    let dispatcher = ToolDispatcher::new(ToolsParams {
        sources: Vec::new(),
        gate,
        session_id: "s1".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: Some(turns.clone()),
        events: None,
        conversations: Some(rt.clone()),
        on_call: None,
        last_request: None,
        probe: None,
    });
    let model = Arc::new(Recording {
        inner: ScriptedModel::new(script, Duration::from_millis(1)),
        seen: Mutex::new(Vec::new()),
    });
    let context = Arc::new(Mutex::new(Context::new(rt.system_prompt(None))));
    let tools = dispatcher.schemas(); // built once, when the session opens
    let stage = LlmStage::new(
        model.clone(),
        context.clone(),
        tools,
        Options {
            max_tokens: 400,
            thinking: None,
            cache_system: true,
            effort: None,
        },
        dispatcher,
        turns.clone(),
        Arc::new(|_frames| Box::pin(async {})),
        Some(rt),
    );
    let (out, frames) = Out::capture();
    Rig {
        stage,
        model,
        apps,
        turns,
        context,
        out,
        _frames: frames,
        _tmp: tmp,
    }
}

async fn say(r: &mut Rig, text: &str) {
    r.turns.interrupt(); // a new turn, as the aggregator starts one
    r.stage
        .process(Frame::user_text(text, true), Direction::Down, &r.out)
        .await;
}

fn offered(r: &Rig, call: usize) -> Vec<String> {
    r.model.seen.lock().unwrap()[call].0.clone()
}

#[tokio::test]
async fn an_open_session_offers_what_the_apps_have_from_the_next_model_call() {
    let reply = || vec![ScriptedModel::say("Okay.")];
    let mut r = rig(vec![reply(), reply(), reply(), reply(), reply()]);
    say(&mut r, "hello").await;
    assert!(!offered(&r, 0).iter().any(|n| n.starts_with("notes_")));

    // An app connects while the session is open.
    let (app, session) = notes(&["search"]).await;
    r.apps.attach(app.clone());
    say(&mut r, "what is in my notes?").await;
    assert!(offered(&r, 1).contains(&"notes_search".to_string()));
    let prompt = r.model.seen.lock().unwrap()[1].1.clone();
    assert!(prompt.contains("Notes is connected"), "{prompt}");

    // Nothing changed: the same list and the same prompt, so the cache holds.
    say(&mut r, "and again").await;
    assert_eq!(offered(&r, 2), offered(&r, 1));
    assert_eq!(r.model.seen.lock().unwrap()[2].1, prompt);

    // The app's own tools change.
    session.tools.lock().unwrap().push(tool("create"));
    app.refresh().await.unwrap();
    say(&mut r, "make a note").await;
    assert!(offered(&r, 3).contains(&"notes_create".to_string()));

    // The app is disconnected.
    r.apps.remove("notes").await;
    say(&mut r, "thanks").await;
    assert!(!offered(&r, 4).iter().any(|n| n.starts_with("notes_")));
    assert!(
        !r.model.seen.lock().unwrap()[4]
            .1
            .contains("Notes is connected")
    );
    assert_eq!(offered(&r, 4), offered(&r, 0));
}

/// The last tool result in the context, as the model reads it.
fn last_result(r: &Rig) -> String {
    r.context
        .lock()
        .unwrap()
        .messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::Tool { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("a tool result")
}

async fn slow_call(script_text: &str) -> (Rig, Arc<Notes>) {
    let r = rig(vec![vec![
        ScriptedModel::call(script_text, "notes_slow", Map::new()),
        ScriptedModel::say("Never said."),
    ]]);
    let (app, session) = notes(&["slow"]).await;
    r.apps.attach(app);
    r.turns.interrupt();
    (r, session)
}

#[tokio::test]
async fn a_call_that_takes_minutes_stops_when_the_operator_speaks() {
    let (mut r, session) = slow_call("One moment.").await;
    let turns = r.turns.clone();
    let watching = session.clone();
    let barge_in = tokio::spawn(async move {
        while watching.started.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(
            turns.calling(),
            "a call in flight is not the operator gone quiet"
        );
        turns.interrupt(); // the operator spoke: the aggregator's interruption
    });
    let began = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(5),
        r.stage.process(
            Frame::user_text("look that up", true),
            Direction::Down,
            &r.out,
        ),
    )
    .await
    .expect("the stage is free again at once, not when the app answers");
    barge_in.await.unwrap();
    assert!(began.elapsed() < Duration::from_secs(2));
    // The app's request was dropped (the session tells the app it was cancelled) ...
    assert_eq!(*session.dropped.lock().unwrap(), vec!["slow".to_string()]);
    // ... the call still has a result in the context, saying what happened ...
    let result = last_result(&r);
    assert!(result.contains("slow was stopped"), "{result}");
    // ... and the model was not run again for a turn that is over.
    assert_eq!(r.model.seen.lock().unwrap().len(), 1);
    assert!(!r.turns.calling());
}

#[tokio::test]
async fn a_call_that_takes_minutes_stops_when_the_operator_hangs_up() {
    let (mut r, session) = slow_call("One moment.").await;
    let turns = r.turns.clone();
    let watching = session.clone();
    let hang_up = tokio::spawn(async move {
        while watching.started.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        turns.stop_calls(); // what `SessionControl::hang_up` does, the turn left as it is
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        r.stage.process(
            Frame::user_text("look that up", true),
            Direction::Down,
            &r.out,
        ),
    )
    .await
    .expect("the end is not queued behind the app");
    hang_up.await.unwrap();
    assert_eq!(*session.dropped.lock().unwrap(), vec!["slow".to_string()]);
    assert!(last_result(&r).contains("was stopped"));
    // Nobody is there to hear an answer: the model is not asked for one.
    assert_eq!(r.model.seen.lock().unwrap().len(), 1);
    // A later call is not stopped by the hang-up that is over.
    assert!(!r.turns.calls_token().is_cancelled());
}
