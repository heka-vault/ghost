//! An agent working with nobody there (ADR 0047): the run's tools go through the gate, a
//! change that would ask is set aside, the run is bounded and can be stopped, a model that
//! cannot be reached is tried again, and a session that is open meanwhile is not touched.
//! The model is scripted; there is no audio anywhere in this file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream::BoxStream;
use ghost_core::apps::Apps;
use ghost_core::conversations::models::Role;
use ghost_core::conversations::{
    Conversations, ConversationsOptions, SOURCE as CONVERSATIONS, conversations_source,
};
use ghost_core::fakes::{FakeWriter, changeset};
use ghost_core::gate::Request;
use ghost_core::ghost_app::{self, GhostServer};
use ghost_core::models::{FakeIssue, SetLabels};
use ghost_core::questions::{QState, Questions, via};
use ghost_core::schedules::jobs::AgentState;
use ghost_core::schedules::{
    AgentRunner, Ended, JobEdit, Jobs, OPERATOR, RunOutcome, RunRequest, RunStatus, Scheduler,
    SchedulerParts, Written, delivery, is_run_session,
};
use ghost_core::source::{BoxFuture, EventKind, Operation, Prepared, ReadTool};
use ghost_core::store::ProposalQuery;
use ghost_core::waiting::{Items, WaitingList};
use ghost_core::{
    AutonomyPolicy, Change, Events, ProposalStatus, Risk, Store, Target, ToolSource, WriteGate,
};
use ghost_llm::{Context, Event, LlmError, Message, Options, Tools};
use ghost_voice::conversation::{ConversationRuntime, Reach};
use ghost_voice::headless::{HeadlessParts, HeadlessRunner, LAST_STEP, retry_wait};
use ghost_voice::intercept::no_action;
use ghost_voice::llm::{ModelPort, ScriptedModel, ScriptedReply};
use ghost_voice::tools::{ToolDispatcher, ToolsParams};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

fn doc(name: &str) -> FakeIssue {
    FakeIssue {
        owner: "doc".into(),
        repo: name.into(),
        number: 1,
    }
}

/// An app-like source: a read, a change that runs (`flag`) and one the operator asks
/// about (`nuke`).
fn fake_source(writer: Arc<FakeWriter>, reads: Arc<AtomicUsize>) -> ToolSource {
    let echo = ReadTool {
        name: "echo".into(),
        description: "Echo.".into(),
        properties: json!({"x": {"type": "string"}}),
        required: vec![],
        handler: Arc::new(move |a| {
            let reads = reads.clone();
            Box::pin(async move {
                reads.fetch_add(1, Ordering::SeqCst);
                let mut m = Map::new();
                m.insert("echo".into(), Value::Object(a));
                Ok(m)
            })
        }),
        operation: String::new(),
    };
    let op = |name: &str, risk: Risk| Operation {
        name: name.into(),
        description: format!("{name}."),
        properties: json!({"name": {"type": "string"}}),
        required: vec!["name".into()],
        prepare: Arc::new(move |a| {
            Box::pin(async move {
                let name = a.get("name").and_then(Value::as_str).unwrap_or("");
                Ok(Prepared {
                    target: Target::FakeIssue(doc(name)),
                    changes: changeset(vec![Change::SetLabels(SetLabels {
                        add: vec!["flagged".into()],
                        remove: vec![],
                    })]),
                    rationale: String::new(),
                    asked_by: Vec::new(),
                })
            })
        }),
        writer: writer.clone(),
        risk,
        allowed: Arc::new(|_| true),
        asked_by: Vec::new(),
        tool_name: None,
    };
    ToolSource {
        name: "fake".into(),
        prompt: "FAKE FRAGMENT".into(),
        read_tools: vec![echo],
        operations: vec![op("flag", Risk::Low), op("nuke", Risk::High)],
        ..Default::default()
    }
}

/// The scripted model, with what each call was shown, and a number of calls that fail
/// first.
struct Watched {
    inner: Arc<ScriptedModel>,
    fail: Mutex<Vec<LlmError>>,
    seen: Mutex<Vec<(Context, Vec<String>)>>,
}

impl Watched {
    fn new(replies: Vec<ScriptedReply>) -> Arc<Self> {
        Self::slow(replies, Duration::ZERO)
    }

    fn slow(replies: Vec<ScriptedReply>, ttfb: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: ScriptedModel::new(vec![replies], ttfb),
            fail: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn failing(self: Arc<Self>, errors: Vec<LlmError>) -> Arc<Self> {
        *self.fail.lock().unwrap() = errors;
        self
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn tools_at(&self, i: usize) -> Vec<String> {
        self.seen.lock().unwrap()[i].1.clone()
    }

    fn context_at(&self, i: usize) -> Context {
        self.seen.lock().unwrap()[i].0.clone()
    }
}

impl ModelPort for Watched {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        self.seen.lock().unwrap().push((
            context.clone(),
            tools.names().into_iter().map(String::from).collect(),
        ));
        let mut fail = self.fail.lock().unwrap();
        if !fail.is_empty() {
            let e = fail.remove(0);
            return Box::pin(futures::stream::iter(vec![Err(e)]));
        }
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
        "scripted".into()
    }
}

fn unreachable() -> LlmError {
    LlmError::Unreachable {
        host: "api.example".into(),
        kind: "ConnectError",
    }
}

struct World {
    store: Arc<Store>,
    convs: Arc<Conversations>,
    gate: Arc<WriteGate>,
    writer: Arc<FakeWriter>,
    reads: Arc<AtomicUsize>,
    waiting: Arc<WaitingList>,
    apps: Arc<Apps>,
    jobs: Arc<Jobs>,
    questions: Arc<Questions>,
    reach: Reach,
    waits: Arc<Mutex<Vec<Duration>>>,
    _ghost: ghost_app::Attached,
}

async fn hire(gate: &WriteGate, name: &str) {
    let op = gate.operation(CONVERSATIONS, "create").unwrap();
    let prepared = (op.prepare)(args(json!({"name": name, "job": "the books"})))
        .await
        .unwrap();
    gate.submit(Request::new(
        "setup",
        CONVERSATIONS,
        "create",
        prepared.target,
        prepared.changes,
        "",
    ))
    .await
    .unwrap();
}

async fn world(tmp: &Path) -> World {
    let cfg = tmp.join("ghost");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("AGENTS.md"), "UK dates, always.").unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let convs =
        Arc::new(Conversations::new(store.clone(), &cfg, ConversationsOptions::default()).unwrap());
    convs.ensure_ghost().unwrap();
    let gate = Arc::new(WriteGate::new(store.clone()).with_autonomy(
        AutonomyPolicy::parse("[permissions]\nask = [\"fake.nuke\"]\n", "test").unwrap(),
    ));
    gate.register(CONVERSATIONS, conversations_source(&convs).operations);
    hire(&gate, "Ledger").await;
    std::fs::write(
        convs.dir_for("ledger").join("persona.md"),
        "Dry, exact, never rounds.",
    )
    .unwrap();
    let writer = Arc::new(FakeWriter::default());
    for name in ["readme", "archive"] {
        writer.seed(&doc(name));
    }
    let reads = Arc::new(AtomicUsize::new(0));
    let fake = fake_source(writer.clone(), reads.clone());
    gate.register("fake", fake.operations.clone());
    // Ghost's own app, in process, as the daemon serves it (ADR 0045), with the schedules.
    let waiting = Arc::new(WaitingList::new(Items::new(store.clone())).with_changes(gate.clone()));
    let apps = Arc::new(Apps::new(
        Vec::new(),
        Arc::new(HashMap::<String, String>::new()),
        None,
    ));
    let jobs = Arc::new(
        Jobs::new(store.clone())
            .with_default_tz("UTC")
            .with_agents({
                let convs = convs.clone();
                Arc::new(move |slug: &str| match convs.get(slug) {
                    Ok(Some(p)) if p.retired() => AgentState::Retired,
                    Ok(Some(_)) => AgentState::Known,
                    _ => AgentState::Unknown,
                })
            }),
    );
    let questions =
        Arc::new(Questions::new(store.clone(), waiting.clone()).with_jobs(jobs.clone()));
    let ghost = ghost_app::attach(
        &apps,
        GhostServer::new(ghost_app::Parts {
            store: store.clone(),
            waiting: waiting.clone(),
            apps: Arc::downgrade(&apps),
            status: Arc::new(Map::new),
        })
        .with_schedules(jobs.clone())
        .with_questions(questions.clone()),
    )
    .await
    .unwrap();
    gate.register(ghost_app::NAME, ghost.app.operations());
    let reach: Reach = {
        let apps = apps.clone();
        Arc::new(move |_p| {
            let mut out = apps.sources();
            out.push(fake.clone());
            out
        })
    };
    World {
        store,
        convs,
        gate,
        writer,
        reads,
        waiting,
        apps,
        jobs,
        questions,
        reach,
        waits: Arc::new(Mutex::new(Vec::new())),
        _ghost: ghost,
    }
}

impl World {
    fn runner(&self, model: &Arc<Watched>) -> HeadlessRunner {
        let model = model.clone();
        let waits = self.waits.clone();
        HeadlessRunner::new(HeadlessParts {
            conversations: self.convs.clone(),
            gate: self.gate.clone(),
            reach: self.reach.clone(),
            model: Arc::new(move || Ok(model.clone() as Arc<dyn ModelPort>)),
            options: Options::default(),
            skills: None,
        })
        // A retry's wait is noted, not slept.
        .with_wait(Arc::new(move |d| {
            waits.lock().unwrap().push(d);
            Box::pin(async {})
        }))
    }

    fn transcript(&self, slug: &str) -> Vec<(Role, String, String)> {
        self.convs
            .turns(slug, 100)
            .unwrap()
            .into_iter()
            .map(|t| (t.role, t.session_id, t.text))
            .collect()
    }

    fn flagged(&self, name: &str) -> bool {
        self.writer
            .issue(&doc(name))
            .labels
            .iter()
            .any(|l| l == "flagged")
    }
}

fn request(agent: &str, instruction: &str) -> RunRequest {
    RunRequest {
        run_id: "r1".into(),
        agent: agent.into(),
        name: "morning check".into(),
        instruction: instruction.into(),
        data: None,
        changed: None,
        skills: Vec::new(),
        max_turns: 30,
        max_runtime: Duration::from_secs(600),
        cancel: CancellationToken::new(),
    }
}

fn change(operation: &str, name: &str) -> ScriptedReply {
    ScriptedModel::call(
        "",
        "change",
        args(json!({"ability": "fake", "operation": operation, "args": {"name": name}})),
    )
}

// --- the run -------------------------------------------------------------------------

#[tokio::test]
async fn a_run_calls_its_tools_through_the_gate_and_ends_with_its_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![
        ScriptedModel::call("", "echo", args(json!({"x": "main"}))),
        change("flag", "readme"),
        ScriptedModel::say("The readme is flagged for review."),
    ]);
    let out = w
        .runner(&model)
        .run(request("ledger", "Flag the readme if it is out of date."))
        .await;

    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.answer, "The readme is flagged for review.");
    assert_eq!((out.model_calls, out.tool_calls, out.retries), (3, 2, 0));
    assert!(!out.notified && out.set_aside.is_empty());
    // The read was called and the change was made, by the gate: it is in the audit, under
    // the run's own session and the agent it ran as.
    assert_eq!(w.reads.load(Ordering::SeqCst), 1);
    assert!(w.flagged("readme"));
    let (audit, _) = w
        .store
        .list_proposals(&ProposalQuery {
            limit: 10,
            ability: Some("fake"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].operation, "flag");
    assert_eq!(audit[0].status, ProposalStatus::Executed);
    assert_eq!(audit[0].conversation.as_deref(), Some("ledger"));
    assert_eq!(audit[0].session_id, "run-r1");
    assert!(is_run_session(&audit[0].session_id));

    // The prompt is the agent's own, for writing: its name and job, the rules, its
    // persona, the apps' paragraphs, and nothing about a voice or a call.
    let system = model.context_at(0).system;
    assert!(system.contains("You are Ledger, one of the operator's agents, hired for the books"));
    assert!(system.contains("UK dates, always."));
    assert!(system.contains("Dry, exact, never rounds."));
    assert!(system.contains("FAKE FRAGMENT") && system.contains("ghost_docs_search"));
    assert!(system.contains("answer exactly [SILENT]"));
    for spoken in [
        "spoken aloud",
        "hang_up",
        "call hold",
        "thinking sound",
        "switch_conversation",
    ] {
        assert!(!system.contains(spoken), "{spoken}");
    }
    // The same tools as a session, less the session's own.
    let offered = model.tools_at(0);
    for tool in [
        "echo",
        "change",
        "ghost_notify",
        "ghost_schedule_manage",
        "ghost_docs_search",
    ] {
        assert!(offered.contains(&tool.to_string()), "{tool}: {offered:?}");
    }
    for tool in ["hang_up", "hold", "switch_conversation", "bring_up_change"] {
        assert!(!offered.contains(&tool.to_string()), "{tool}");
    }
    // What it was asked is one message, marked.
    let asked = match model.context_at(0).messages.last().unwrap() {
        Message::User(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        asked,
        "[scheduled run: morning check]\nFlag the readme if it is out of date."
    );

    // Its turns are in the agent's transcript, marked, under the run's session.
    let kept = w.transcript("ledger");
    let run: Vec<_> = kept.iter().filter(|(_, s, _)| s == "run-r1").collect();
    assert_eq!(run.len(), 4);
    assert_eq!(
        (run[0].0, run[0].2.as_str()),
        (
            Role::System,
            "[scheduled run: morning check] Flag the readme if it is out of date."
        )
    );
    assert_eq!((run[1].0, run[1].2.as_str()), (Role::Call, "echo(x=main)"));
    assert_eq!(run[2].0, Role::Call);
    assert_eq!(
        (run[3].0, run[3].2.as_str()),
        (
            Role::Assistant,
            "[scheduled run: morning check] The readme is flagged for review."
        )
    );
    // So when the operator asks later, the agent has what it said.
    let later: Vec<String> = w
        .convs
        .resume("ledger")
        .unwrap()
        .messages()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert!(
        later
            .iter()
            .any(|m| m.contains("You: [scheduled run: morning check] The readme is flagged"))
    );
    assert!(w.transcript("ghost").iter().all(|(_, s, _)| s != "run-r1"));
}

#[tokio::test]
async fn a_change_that_would_ask_is_set_aside_on_the_waiting_list_and_the_run_carries_on() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![
        change("nuke", "archive"),
        change("flag", "readme"),
        ScriptedModel::say("Flagged the readme. Clearing the archive is waiting for your yes."),
    ]);
    // The instruction asks for it in so many words: in a session that would be the yes
    // (ADR 0017). Here nobody said it; a schedule's text is not the operator's turn.
    let out = w
        .runner(&model)
        .run(request("ledger", "nuke archive, then flag readme"))
        .await;

    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.set_aside.len(), 1);
    assert!(out.set_aside[0].contains("archive"));
    assert!(out.answer.contains("waiting for your yes"));
    // Not done, and waiting: on the list, by id, attached to no session.
    assert!(!w.flagged("archive"));
    assert!(w.flagged("readme")); // the run carried on
    let waiting = w.gate.waiting(None).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].operation, "nuke");
    assert_eq!(waiting[0].status, ProposalStatus::Pending);
    assert!(w.gate.pending("run-r1", None).unwrap().is_none());
    let events: Vec<String> = w
        .store
        .events(&waiting[0].id)
        .unwrap()
        .into_iter()
        .map(|e| e.event)
        .collect();
    assert_eq!(events, vec!["prepared", "set_aside"]);
    let items = w.waiting.refresh(None).unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.source == "gate" && i.title.starts_with("Waiting for your yes:"))
    );
    // The model was told so, not that it was done.
    let told = match &model.context_at(1).messages.last().unwrap() {
        Message::Tool { content, .. } => content.clone(),
        other => panic!("{other:?}"),
    };
    assert!(told.contains("\"status\":\"set_aside\"") && told.contains("waiting list"));
    // The operator's Confirm, later, applies it.
    let done = w
        .gate
        .confirm_id(&waiting[0].id, "ui", None)
        .await
        .unwrap()
        .expect("applied");
    assert_eq!(done.proposal_id, waiting[0].id);
    assert!(w.flagged("archive"));
}

#[tokio::test]
async fn a_run_is_bounded_by_its_turns_and_its_last_one_has_no_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    // A model that would look things up for ever.
    let model = Watched::new(vec![ScriptedModel::call(
        "Still looking.",
        "echo",
        args(json!({"x": "again"})),
    )]);
    let out = w
        .runner(&model)
        .run(RunRequest {
            max_turns: 4,
            ..request("ghost", "Look into it.")
        })
        .await;
    assert_eq!(out.ended, Ended::Done);
    assert_eq!((out.model_calls, out.tool_calls), (4, 3));
    assert_eq!(w.reads.load(Ordering::SeqCst), 3);
    assert!(!model.tools_at(2).is_empty());
    assert!(model.tools_at(3).is_empty());
    assert!(matches!(
        model.context_at(3).messages.last(),
        Some(Message::User(t)) if t == LAST_STEP
    ));
    assert_eq!(out.answer, "Still looking.");
}

#[tokio::test(start_paused = true)]
async fn a_run_is_bounded_by_the_clock_and_can_be_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let slow = || {
        Watched::slow(
            vec![ScriptedModel::say("Too late.")],
            Duration::from_secs(900),
        )
    };
    let model = slow();
    let out = w
        .runner(&model)
        .run(request("ghost", "Take your time."))
        .await;
    assert_eq!(out.ended, Ended::TimedOut);
    assert_eq!(out.model_calls, 1);
    assert!(out.answer.is_empty());

    // Cancelled while the model is thinking.
    let model = slow();
    let cancel = CancellationToken::new();
    let run = {
        let runner = w.runner(&model);
        let r = RunRequest {
            cancel: cancel.clone(),
            ..request("ghost", "Take your time.")
        };
        tokio::spawn(async move { runner.run(r).await })
    };
    tokio::time::sleep(Duration::from_secs(5)).await;
    cancel.cancel();
    assert_eq!(run.await.unwrap().ended, Ended::Cancelled);
    // Neither left anything in the transcript: the scheduler notes what did not finish.
    assert!(w.transcript("ghost").is_empty());
}

/// A source with one read, `name`, run by `handler`.
fn one_read(
    source: &str,
    name: &str,
    handler: impl Fn() -> BoxFuture<'static, Result<Map<String, Value>, String>> + Send + Sync + 'static,
) -> ToolSource {
    let handler = Arc::new(handler);
    ToolSource {
        name: source.into(),
        prompt: format!("{} FRAGMENT", source.to_uppercase()),
        read_tools: vec![ReadTool {
            name: name.into(),
            description: "Look.".into(),
            properties: json!({}),
            required: vec![],
            handler: Arc::new(move |_| {
                let handler = handler.clone();
                Box::pin(async move { handler().await.map_err(|e| anyhow::anyhow!(e)) })
            }),
            operation: String::new(),
        }],
        ..Default::default()
    }
}

fn runner_over(w: &World, model: &Arc<Watched>, reach: Reach) -> HeadlessRunner {
    let model = model.clone();
    HeadlessRunner::new(HeadlessParts {
        conversations: w.convs.clone(),
        gate: w.gate.clone(),
        reach,
        model: Arc::new(move || Ok(model.clone() as Arc<dyn ModelPort>)),
        options: Options::default(),
        skills: None,
    })
}

#[tokio::test]
async fn a_stopped_run_stops_its_calls_to_apps_and_each_still_has_a_result() {
    // ADR 0046 gave a call 300 seconds and a way to stop it. A run's calls are made under
    // the run's own token, so a run that is cancelled tells the app and gets the call's
    // result back at once, where dropping the run would leave it with no outcome.
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let began = Arc::new(AtomicUsize::new(0));
    let told = Arc::new(AtomicUsize::new(0));
    let slow = {
        let (began, told) = (began.clone(), told.clone());
        one_read("slow", "slow_report", move || {
            let (began, told) = (began.clone(), told.clone());
            Box::pin(async move {
                // What a call to a connected app does: it waits on the app and on this.
                let stop = ghost_core::apps::current_cancel().expect("the run's token");
                began.fetch_add(1, Ordering::SeqCst);
                stop.cancelled().await;
                told.fetch_add(1, Ordering::SeqCst);
                Err("the call was stopped".to_string())
            })
        })
    };
    let reach: Reach = Arc::new(move |_p| vec![slow.clone()]);
    let model = Watched::new(vec![
        ScriptedModel::call("", "slow_report", Map::new()),
        ScriptedModel::say("Never said."),
    ]);
    let cancel = CancellationToken::new();
    let run = {
        let runner = runner_over(&w, &model, reach);
        let r = RunRequest {
            cancel: cancel.clone(),
            ..request("ghost", "Fetch the report.")
        };
        tokio::spawn(async move { runner.run(r).await })
    };
    while began.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let at = std::time::Instant::now();
    cancel.cancel();
    let out = run.await.unwrap();
    assert_eq!(out.ended, Ended::Cancelled);
    assert_eq!(told.load(Ordering::SeqCst), 1, "the call was told to stop");
    assert_eq!(out.tool_calls, 1, "and came back with a result");
    assert_eq!(model.calls(), 1, "the model is not called again");
    assert!(
        at.elapsed() < ghost_voice::headless::STOP_GRACE,
        "nothing waited out the grace"
    );
}

#[tokio::test(start_paused = true)]
async fn a_call_that_does_not_stop_when_told_is_left_behind_after_the_grace() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let deaf = one_read("deaf", "deaf_wait", || {
        Box::pin(async {
            std::future::pending::<()>().await;
            Ok(Map::new())
        })
    });
    let reach: Reach = Arc::new(move |_p| vec![deaf.clone()]);
    let model = Watched::new(vec![ScriptedModel::call("", "deaf_wait", Map::new())]);
    let out = runner_over(&w, &model, reach)
        .run(RunRequest {
            max_runtime: Duration::from_secs(60),
            ..request("ghost", "Wait for it.")
        })
        .await;
    assert_eq!(out.ended, Ended::TimedOut);
    assert_eq!(out.model_calls, 1);
}

#[tokio::test]
async fn a_run_is_offered_the_tools_as_they_are_before_each_model_call() {
    // ADR 0046: an app that connects in the middle of a run is offered from the next
    // model call, its paragraph joins the prompt, and a call to it goes through.
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let looked = Arc::new(AtomicUsize::new(0));
    let late = {
        let looked = looked.clone();
        one_read("late", "late_look", move || {
            looked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Map::new()) })
        })
    };
    let reach: Reach = {
        let (inner, reads) = (w.reach.clone(), w.reads.clone());
        Arc::new(move |p| {
            let mut out = inner(p);
            // It connects once the run has made its first read.
            if reads.load(Ordering::SeqCst) > 0 {
                out.push(late.clone());
            }
            out
        })
    };
    let model = Watched::new(vec![
        ScriptedModel::call("", "echo", args(json!({"x": "one"}))),
        ScriptedModel::call("", "late_look", Map::new()),
        ScriptedModel::say("Looked at both."),
    ]);
    let out = runner_over(&w, &model, reach)
        .run(request("ghost", "Look around."))
        .await;
    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.answer, "Looked at both.");
    assert!(!model.tools_at(0).contains(&"late_look".to_string()));
    assert!(model.tools_at(1).contains(&"late_look".to_string()));
    assert!(!model.context_at(0).system.contains("LATE FRAGMENT"));
    assert!(model.context_at(1).system.contains("LATE FRAGMENT"));
    // Unchanged after that: the prompt is not rebuilt on every call.
    assert_eq!(model.context_at(1).system, model.context_at(2).system);
    assert_eq!(looked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_model_that_cannot_be_reached_is_tried_again_three_times_at_the_most() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    assert_eq!(
        (1..=6).map(|n| retry_wait(n).as_secs()).collect::<Vec<_>>(),
        vec![30, 60, 120, 240, 300, 300]
    );
    // Twice down, then there.
    let model = Watched::new(vec![ScriptedModel::say("Back.")])
        .failing(vec![unreachable(), LlmError::Stream("reset".into())]);
    let out = w.runner(&model).run(request("ghost", "Check.")).await;
    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.answer, "Back.");
    assert_eq!((out.model_calls, out.retries), (3, 2));
    assert_eq!(
        *w.waits.lock().unwrap(),
        vec![Duration::from_secs(30), Duration::from_secs(60)]
    );
    // The context the retry was given is the one the failed call was.
    assert_eq!(
        model.context_at(2).messages.len(),
        model.context_at(0).messages.len()
    );

    // Down for good: three more tries, then it says so.
    w.waits.lock().unwrap().clear();
    let model = Watched::new(vec![ScriptedModel::say("Never.")]).failing(vec![
        unreachable(),
        unreachable(),
        unreachable(),
        unreachable(),
    ]);
    let out = w.runner(&model).run(request("ghost", "Check.")).await;
    assert!(matches!(&out.ended, Ended::Transient(e) if e.contains("can't reach api.example")));
    assert_eq!((out.model_calls, out.retries), (4, 3));
    assert_eq!(
        w.waits
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect::<Vec<_>>(),
        vec![30, 60, 120]
    );

    // A refused key is not a passing failure: no second try.
    w.waits.lock().unwrap().clear();
    let model = Watched::new(vec![ScriptedModel::say("Never.")]).failing(vec![LlmError::Status {
        status: 401,
        detail: "invalid x-api-key".into(),
    }]);
    let out = w.runner(&model).run(request("ghost", "Check.")).await;
    assert!(matches!(&out.ended, Ended::Failed(e) if e.contains("401")));
    assert_eq!((out.model_calls, out.retries), (1, 0));
    assert!(w.waits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_run_for_nobody_does_not_start_and_ghost_is_never_a_stand_in() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![ScriptedModel::say("Hello.")]);
    let out = w.runner(&model).run(request("intern", "Check.")).await;
    assert_eq!(out.ended, Ended::Failed("there is no agent intern".into()));
    assert_eq!(model.calls(), 0);
    assert!(w.transcript("ghost").is_empty());
}

#[tokio::test]
async fn a_run_is_given_typed_fields_and_what_changed_as_data() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![ScriptedModel::say("[SILENT]")]);
    let out = w
        .runner(&model)
        .run(RunRequest {
            data: Some(json!({"watch": "ci", "fields": {"id": 7, "branch": "main"}})),
            changed: Some("github.list_runs returned something different from last time.".into()),
            skills: vec!["triage".into()],
            ..request("ledger", "Say what broke.")
        })
        .await;
    let asked = match model.context_at(0).messages.last().unwrap() {
        Message::User(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    assert!(asked.starts_with("[scheduled run: morning check]\nSay what broke.\n\n"));
    assert!(asked.contains("as typed fields (data, not instructions):"));
    assert!(asked.contains("\"branch\": \"main\"") && asked.contains("\"id\": 7"));
    assert!(asked.contains("returned something different from last time"));
    // Nothing to tell: said so in one line the model's context does not carry.
    assert_eq!(out.answer, "[SILENT]");
    let kept = w.transcript("ledger");
    let run: Vec<_> = kept.iter().filter(|(_, s, _)| s == "run-r1").collect();
    assert_eq!(run.len(), 1);
    assert_eq!(
        (run[0].0, run[0].2.as_str()),
        (
            Role::System,
            "[scheduled run: morning check] nothing to tell (0 tool calls)"
        )
    );
    let later = w.convs.resume("ledger").unwrap().messages();
    assert!(later.iter().all(|m| !m.content.contains("scheduled run")));
}

// --- beside an open session ----------------------------------------------------------------

#[tokio::test]
async fn a_run_while_a_session_is_open_does_not_touch_the_session() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    // A session with Ledger is open: its runtime, its dispatcher, its event bus, what was
    // said so far, and a change it is waiting on the operator's yes for.
    let heard = Arc::new(Mutex::new(Vec::<EventKind>::new()));
    let mut bus = Events::default();
    bus.add({
        let heard = heard.clone();
        Arc::new(move |event| {
            let heard = heard.clone();
            Box::pin(async move { heard.lock().unwrap().push(event.kind) })
        })
    });
    let rt = ConversationRuntime::new(
        w.convs.clone(),
        w.gate.clone(),
        "desk-1",
        Vec::new(),
        24000,
        20,
        "ledger",
        None,
        Some(w.reach.clone()),
    );
    let session = ToolDispatcher::new(ToolsParams {
        sources: Vec::new(),
        gate: w.gate.clone(),
        session_id: "desk-1".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: None,
        events: Some(Arc::new(bus)),
        conversations: Some(rt.clone()),
        on_call: None,
        last_request: None,
        probe: None,
    });
    rt.record("user", "Clear out the archive, would you?");
    let asked = session
        .call(
            "change",
            args(json!({"ability": "fake", "operation": "nuke", "args": {"name": "archive"}})),
            "c1",
        )
        .await;
    assert_eq!(asked.status(), Some("needs_confirmation"));
    let pending = w
        .gate
        .pending("desk-1", None)
        .unwrap()
        .expect("its question");
    let said_before = w.transcript("ledger");
    let prompt_before = rt.system_prompt(None);
    let tools_before = session.schemas();

    // Meanwhile a run of the same agent makes a change of its own, asks for one that
    // needs a yes, and tells the operator something.
    let model = Watched::new(vec![
        change("flag", "readme"),
        change("nuke", "readme"),
        ScriptedModel::call(
            "",
            "ghost_notify",
            args(json!({"title": "The readme needs you"})),
        ),
        ScriptedModel::say("Done what I could."),
    ]);
    let out = w
        .runner(&model)
        .run(request("ledger", "Tidy the docs."))
        .await;
    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.set_aside.len(), 1);

    // The session's question is still its own, attached and unanswered; the run's is
    // beside it on the list, not in its place.
    let still = w
        .gate
        .pending("desk-1", None)
        .unwrap()
        .expect("still asked");
    assert_eq!(still.id, pending.id);
    assert_eq!(still.status, ProposalStatus::Pending);
    assert_eq!(w.gate.waiting(None).unwrap().len(), 2);
    assert!(!w.flagged("archive"));
    // Nothing reached the session's bus, its agent, its prompt or its tools.
    assert!(heard.lock().unwrap().is_empty());
    assert_eq!(rt.active().slug, "ledger");
    assert!(rt.settled());
    assert_eq!(rt.system_prompt(None), prompt_before);
    assert_eq!(session.schemas(), tools_before);
    // What was said in the session is as it was; the run's turns are beside it, under
    // their own session id.
    let said_after = w.transcript("ledger");
    let of_session = |all: &[(Role, String, String)]| -> Vec<(Role, String, String)> {
        all.iter()
            .filter(|(_, s, _)| s == "desk-1")
            .cloned()
            .collect()
    };
    assert_eq!(of_session(&said_after), of_session(&said_before));
    assert!(said_after.iter().any(|(_, s, _)| s == "run-r1"));
    // And the operator's yes, in the session, still answers the session's question.
    let done = w
        .gate
        .on_user_turn("desk-1", "yes, go ahead", None, "")
        .await
        .unwrap()
        .1
        .expect("applied");
    assert_eq!(done.proposal_id, pending.id);
    assert!(w.flagged("archive"));
    assert!(w.flagged("readme")); // the run's ask is still waiting
    assert_eq!(w.gate.waiting(None).unwrap().len(), 1);
}

// --- with the scheduler -------------------------------------------------------------------

fn schedule(w: &World, agent: &str) -> ghost_core::schedules::Job {
    w.jobs
        .create(
            &JobEdit {
                name: Some("build check".into()),
                agent: Some(agent.into()),
                instruction: Some("Check the build and say if it is red.".into()),
                when: Some(Written {
                    every: Some("30m".into()),
                    ..Written::default()
                }),
                ..JobEdit::default()
            },
            OPERATOR,
            ghost_core::time::utcnow(),
        )
        .unwrap()
}

fn scheduler(w: &World, model: &Arc<Watched>) -> Arc<Scheduler> {
    Scheduler::new(SchedulerParts {
        jobs: w.jobs.clone(),
        runner: Arc::new(w.runner(model)),
        waiting: w.waiting.clone(),
        reader: None,
        conversations: Some(w.convs.clone()),
    })
}

fn agents_items(w: &World, slug: &str) -> Vec<String> {
    w.waiting
        .items
        .open(None)
        .unwrap()
        .into_iter()
        .filter(|i| i.source == format!("agent:{slug}"))
        .map(|i| i.title)
        .collect()
}

#[tokio::test]
async fn what_the_agent_told_the_operator_itself_is_not_delivered_a_second_time() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let job = schedule(&w, "ledger");
    // The model names another agent as the caller; the dispatcher's word is the one kept.
    let model = Watched::new(vec![
        ScriptedModel::call(
            "",
            "ghost_notify",
            args(
                json!({"title": "The build is red on main", "urgency": "call", "_agent": "ghost"}),
            ),
        ),
        ScriptedModel::say("I told them the build is red."),
    ]);
    let s = scheduler(&w, &model);
    w.jobs
        .request_run(&job.id, ghost_core::time::utcnow())
        .unwrap();
    s.drain().await;

    assert_eq!(agents_items(&w, "ledger"), vec!["The build is red on main"]);
    assert!(agents_items(&w, "ghost").is_empty());
    let run = &w.jobs.runs(&job.id, 1).unwrap()[0];
    assert_eq!(run.status, RunStatus::Ok);
    assert_eq!(run.delivery.as_deref(), Some(delivery::BY_AGENT));
    assert_eq!(run.answer.as_deref(), Some("I told them the build is red."));
    assert_eq!((run.model_calls, run.tool_calls), (2, 1));

    // A run that only answers is delivered once, as its answer.
    let model = Watched::new(vec![ScriptedModel::say("Green again.")]);
    let s = scheduler(&w, &model);
    w.jobs
        .request_run(&job.id, ghost_core::time::utcnow())
        .unwrap();
    s.drain().await;
    assert_eq!(
        agents_items(&w, "ledger"),
        vec!["The build is red on main", "build check: Green again."]
    );
    assert_eq!(
        w.jobs.runs(&job.id, 1).unwrap()[0].delivery.as_deref(),
        Some(delivery::DELIVERED)
    );
}

#[tokio::test]
async fn a_run_that_fails_is_told_and_noted_for_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let job = schedule(&w, "ledger");
    let model = Watched::new(vec![ScriptedModel::say("Never.")]).failing(vec![LlmError::Status {
        status: 401,
        detail: "invalid x-api-key".into(),
    }]);
    let s = scheduler(&w, &model);
    w.jobs
        .request_run(&job.id, ghost_core::time::utcnow())
        .unwrap();
    s.drain().await;
    let run = &w.jobs.runs(&job.id, 1).unwrap()[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.delivery.as_deref(), Some(delivery::FAILURE));
    assert_eq!(
        agents_items(&w, "ledger"),
        vec!["build check did not finish: HTTP 401: invalid x-api-key"]
    );
    assert!(
        w.transcript("ledger")
            .iter()
            .any(|(role, _, text)| *role == Role::Assistant
                && text.starts_with("[scheduled run: build check] did not finish: HTTP 401"))
    );
}

/// ADR 0018: what the two schedule tools cost on every turn of every session, in the
/// cached prefix. Run with `--no-capture` to read the numbers; the bound keeps a later
/// argument from growing it unnoticed.
#[tokio::test]
async fn what_the_schedule_tools_add_to_every_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let ghost = w.convs.get("ghost").unwrap().unwrap();
    let tools = ToolDispatcher::new(ToolsParams {
        sources: (w.reach)(&ghost),
        gate: w.gate.clone(),
        session_id: "s".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    })
    .schemas();
    let bytes = |name: &str| {
        serde_json::to_string(&tools.get(name).expect(name).anthropic())
            .unwrap()
            .len()
    };
    let (list, manage) = (
        bytes("ghost_schedules_list"),
        bytes("ghost_schedule_manage"),
    );
    println!(
        "ghost_schedules_list: {list} bytes; ghost_schedule_manage: {manage} bytes; \
         +2 tools, +{} bytes",
        list + manage
    );
    assert!(list + manage < 3_200, "{}", list + manage);
}

#[tokio::test]
async fn an_agent_sets_up_its_own_schedule_from_a_run_or_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![
        ScriptedModel::call(
            "",
            "ghost_schedule_manage",
            args(json!({
                "action": "create", "name": "nine o'clock build",
                "instruction": "Check the build on main; if it is green answer [SILENT].",
                "cron": "0 9 * * *", "tz": "Europe/London",
            })),
        ),
        ScriptedModel::say("It will check the build every morning at nine."),
    ]);
    let out = w
        .runner(&model)
        .run(request(
            "ledger",
            "Check the build every morning at nine and tell me if it is red.",
        ))
        .await;
    assert_eq!(out.ended, Ended::Done);
    let made = w.jobs.find("nine o'clock build").unwrap().expect("made");
    assert_eq!(made.agent, "ledger");
    assert_eq!(made.created_by, "agent:ledger");
    assert_eq!(made.options.tz, "Europe/London");
    // Through the gate: it is in the audit as the agent's change.
    let (audit, _) = w
        .store
        .list_proposals(&ProposalQuery {
            limit: 10,
            ability: Some("ghost"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(audit[0].operation, "schedule_manage");
    assert_eq!(audit[0].conversation.as_deref(), Some("ledger"));
    let _ = (&w.apps, RunOutcome::ended(Ended::Done));
}

// --- asking the operator (ADR 0051) ----------------------------------------------------

#[tokio::test]
async fn a_run_that_asks_ends_and_the_answer_starts_exactly_one_follow_up_run() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![
        ScriptedModel::call(
            "",
            "ghost_ask",
            args(
                json!({"question": "Which account should the refund come from?",
                        "choices": ["Business", "Personal"]}),
            ),
        ),
        ScriptedModel::say("[SILENT]"),
    ]);
    let out = w
        .runner(&model)
        .run(request("ledger", "Refund the duplicate charge."))
        .await;
    // The run did not wait: it ended, silent, with the question on the list.
    assert_eq!(out.ended, Ended::Done);
    assert_eq!(out.answer, "[SILENT]");
    assert_eq!(model.calls(), 2);
    let open = w.questions.open(None).unwrap();
    assert_eq!(open.len(), 1);
    let q = &open[0];
    assert_eq!(q.asker, "agent:ledger");
    assert_eq!(q.who, "Ledger");
    assert_eq!(q.asked_in.as_deref(), Some("run-r1"));
    assert!(q.follow_up);
    assert!(w.jobs.queued().unwrap().is_empty());
    let item = w.waiting.items.get(&q.key).unwrap().unwrap();
    assert!(item.needs_alerting(None));

    // The operator answers on the desktop: one follow-up run, queued for Ledger.
    let answered = w
        .questions
        .answer(&q.key, "business", via::DESKTOP, None)
        .unwrap();
    assert_eq!(answered.state, QState::Answered);
    assert_eq!(answered.choice.as_deref(), Some("Business"));
    assert_eq!(w.jobs.queued().unwrap().len(), 1);
    // The same answer again starts nothing more.
    w.questions
        .answer(&q.key, "business", via::DESKTOP, None)
        .unwrap();
    assert_eq!(w.jobs.queued().unwrap().len(), 1);

    // It runs as any run does, with the question and the answer as what it is asked.
    let next = Watched::new(vec![ScriptedModel::say(
        "Refunded from the business account.",
    )]);
    let scheduler = Scheduler::new(SchedulerParts {
        jobs: w.jobs.clone(),
        runner: Arc::new(w.runner(&next)),
        waiting: w.waiting.clone(),
        reader: None,
        conversations: Some(w.convs.clone()),
    });
    scheduler.drain().await;
    assert_eq!(next.calls(), 1);
    let asked = match next.context_at(0).messages.last().unwrap() {
        Message::User(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        asked.contains("You asked the operator: \"Which account should the refund come from?\"")
    );
    assert!(asked.contains("The operator answered (desktop): \"business\""));
    assert!(asked.contains("That is the choice \"Business\"."));
    // Its answer is delivered as any run's is, and there was exactly one.
    let runs = w.jobs.recent_runs(10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Ok);
    assert!(
        w.waiting
            .items
            .open(None)
            .unwrap()
            .iter()
            .any(|i| i.source == "agent:ledger"
                && i.fields.get("detail").and_then(Value::as_str)
                    == Some("Refunded from the business account."))
    );
    assert!(w.jobs.queued().unwrap().is_empty());
}

#[tokio::test]
async fn a_declined_question_starts_no_run() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let model = Watched::new(vec![
        ScriptedModel::call("", "ghost_ask", args(json!({"question": "Pay it now?"}))),
        ScriptedModel::say("[SILENT]"),
    ]);
    w.runner(&model)
        .run(request("ledger", "Pay the bill."))
        .await;
    let q = w.questions.open(None).unwrap().pop().unwrap();
    w.questions.decline(&q.key, via::PHONE, None).unwrap();
    assert!(w.jobs.queued().unwrap().is_empty());
}

#[tokio::test]
async fn answer_is_refused_in_a_run_whatever_the_model_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let mut a = ghost_core::questions::Ask::new("agent:brand", "Brand", "Logo A or B?");
    a.agent = Some("brand".into());
    let q = w.questions.ask(a, None).unwrap();
    let model = Watched::new(vec![
        ScriptedModel::call(
            "",
            "ghost_answer",
            args(json!({"key": q.key, "text": "A", "_heard": "run-r1", "_session": "s1"})),
        ),
        ScriptedModel::say("Done."),
    ]);
    w.runner(&model)
        .run(request("ghost", "Answer what you can."))
        .await;
    assert!(w.questions.get(&q.key).unwrap().unwrap().is_open());
    let result = format!("{:?}", model.context_at(1).messages);
    assert!(result.contains("nobody is here to answer"), "{result}");
}

/// ADR 0018: what the question tools cost on every turn, in the cached prefix.
#[tokio::test]
async fn what_the_question_tools_add_to_every_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let ghost = w.convs.get("ghost").unwrap().unwrap();
    let tools = ToolDispatcher::new(ToolsParams {
        sources: (w.reach)(&ghost),
        gate: w.gate.clone(),
        session_id: "s".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    })
    .schemas();
    let bytes = |name: &str| {
        serde_json::to_string(&tools.get(name).expect(name).anthropic())
            .unwrap()
            .len()
    };
    let names = [
        "ghost_ask",
        "ghost_answer",
        "ghost_question",
        "ghost_questions_list",
        "ghost_question_cancel",
    ];
    let each: Vec<usize> = names.iter().map(|n| bytes(n)).collect();
    let total: usize = each.iter().sum();
    println!("{names:?}: {each:?} bytes; +5 tools, +{total} bytes");
    assert!(total < 3_200, "{total}");
}

/// ADR 0018, ADR 0052: what Ghost's own app costs on every turn of every session and run,
/// all of its advertised tools together, in the cached prefix. Its new tools are found by
/// search (`ghost_tools_search`), so this stays bounded however many it has.
#[tokio::test]
async fn what_ghosts_own_app_costs_every_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path()).await;
    let ghost = w.convs.get("ghost").unwrap().unwrap();
    let tools = ToolDispatcher::new(ToolsParams {
        sources: (w.reach)(&ghost),
        gate: w.gate.clone(),
        session_id: "s".into(),
        on_hang_up: no_action(),
        on_hold: None,
        turns: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    })
    .schemas();
    let own: Vec<&str> = tools
        .names()
        .into_iter()
        .filter(|n| n.starts_with("ghost_"))
        .collect();
    let bytes: usize = own
        .iter()
        .map(|n| {
            serde_json::to_string(&tools.get(n).unwrap().anthropic())
                .unwrap()
                .len()
        })
        .sum();
    println!("OWN advertised={} bytes={bytes} names={own:?}", own.len());
}
