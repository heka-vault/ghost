//! The tools the model reaches (the port of `test_prompt_and_tools.py` and
//! `test_conversation_runtime.py`): what is offered, a read's round trip, a change at
//! each level, the dispatch to the active agent's reach, and the in-session side of
//! conversations (ADR 0014): prompt, seeded context, transfer frames, save-as, records
//! and compaction persistence. No audio.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ghost_core::conversations::{
    CompactionSource, Conversations, ConversationsOptions, GHOST, Profile, SOURCE,
    SUMMARY_TEMPLATE, conversations_source,
};
use ghost_core::fakes::{FakeWriter, changeset};
use ghost_core::gate::Request;
use ghost_core::models::{AppChange, AppSnapshot, AppTarget, FakeIssue, SetLabels};
use ghost_core::source::{Operation, Prepared, ReadTool, Writer};
use ghost_core::{
    AutonomyPolicy, Change, ChangeSet, Risk, Snapshot, Store, Target, ToolSource, WriteGate,
};
use ghost_llm::Message;
use ghost_voice::conversation::ConversationRuntime;
use ghost_voice::frames::Frame;
use ghost_voice::intercept::{SessionAction, no_action};
use ghost_voice::tools::{CHANGE_TOOL, SWITCH_TOOL, ToolDispatcher, ToolsParams};
use serde_json::{Map, Value, json};

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

// --- a fake source for the tools tests ------------------------------------------------

fn doc(name: &str) -> FakeIssue {
    FakeIssue {
        owner: "doc".into(),
        repo: name.into(),
        number: 1,
    }
}

fn fake_source(writer: Arc<FakeWriter>) -> ToolSource {
    let echo: ReadTool = ReadTool {
        name: "echo".into(),
        description: "Echo.".into(),
        properties: json!({"x": {"type": "string"}}),
        required: vec!["x".into()],
        handler: Arc::new(|a| {
            Box::pin(async move {
                let mut m = Map::new();
                m.insert("echo".into(), Value::Object(a));
                Ok(m)
            })
        }),
        operation: String::new(),
    };
    let op = |name: &str, risk: Risk| {
        let w = writer.clone();
        Operation {
            name: name.into(),
            description: format!("{name}."),
            properties: json!({"name": {"type": "string"}}),
            required: vec!["name".into()],
            prepare: Arc::new(move |a| {
                Box::pin(async move {
                    let name = a
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if name == "forbidden" {
                        anyhow::bail!("bad name");
                    }
                    Ok(Prepared {
                        target: Target::FakeIssue(doc(&name)),
                        changes: changeset(vec![Change::SetLabels(SetLabels {
                            add: vec!["flagged".into()],
                            remove: vec![],
                        })]),
                        rationale: String::new(),
                        asked_by: Vec::new(),
                    })
                })
            }),
            writer: w,
            risk,
            allowed: Arc::new(|t| !matches!(t, Target::FakeIssue(i) if i.repo == "other")),
            asked_by: Vec::new(),
            tool_name: None,
        }
    };
    ToolSource {
        name: "fake".into(),
        prompt: "FAKE FRAGMENT".into(),
        read_tools: vec![echo],
        operations: vec![
            op("flag", Risk::Low),
            op("nuke", Risk::High),
            op("never", Risk::Low),
        ],
        ..Default::default()
    }
}

struct Wired {
    tools: Arc<ToolDispatcher>,
    gate: Arc<WriteGate>,
    slept: Arc<Mutex<Vec<bool>>>,
    writer: Arc<FakeWriter>,
}

fn sleeper() -> (SessionAction, Arc<Mutex<Vec<bool>>>) {
    let slept = Arc::new(Mutex::new(Vec::new()));
    let s = slept.clone();
    let action: SessionAction = Arc::new(move || {
        let s = s.clone();
        Box::pin(async move {
            s.lock().unwrap().push(true);
        })
    });
    (action, slept)
}

fn wired() -> Wired {
    let writer = Arc::new(FakeWriter::default());
    for name in ["readme", "x", "y", "other"] {
        writer.seed(&doc(name));
    }
    let loaded = fake_source(writer.clone());
    let gate = Arc::new(
        WriteGate::new(Arc::new(Store::in_memory().unwrap())).with_autonomy(
            AutonomyPolicy::parse(
                "[permissions]\nblock = [\"fake.never\"]\nask = [\"fake.nuke\"]\n",
                "test",
            )
            .unwrap(),
        ),
    );
    gate.register("fake", loaded.operations.clone());
    let (on_sleep, slept) = sleeper();
    let tools = ToolDispatcher::new(ToolsParams {
        sources: vec![loaded],
        gate: gate.clone(),
        session_id: "s".into(),
        on_hang_up: on_sleep,
        on_hold: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    });
    Wired {
        tools,
        gate,
        slept,
        writer,
    }
}

fn pending(gate: &WriteGate, session: &str) -> bool {
    matches!(gate.pending(session, None), Ok(Some(_)))
}

#[test]
fn test_only_reads_change_and_sleep_are_exposed() {
    let w = wired();
    let schemas = w.tools.schemas();
    let mut names: Vec<&str> = schemas.names();
    names.sort_unstable();
    assert_eq!(names, vec!["bring_up_change", "change", "echo", "hang_up"]);
}

#[test]
fn test_denied_ops_are_not_offered() {
    let w = wired();
    let schemas = w.tools.schemas();
    let change = schemas.get("change").unwrap();
    assert!(change.description.contains("fake.flag") && change.description.contains("fake.nuke"));
    assert!(!change.description.contains("fake.never"));
}

#[tokio::test]
async fn test_read_tool_round_trip() {
    let w = wired();
    let r = w.tools.call("echo", args(json!({"x": "hi"})), "c").await;
    assert_eq!(Value::Object(r.result), json!({"echo": {"x": "hi"}}));
}

fn call(op: &str, name: &str) -> Map<String, Value> {
    args(json!({"ability": "fake", "operation": op, "args": {"name": name}}))
}

#[tokio::test]
async fn test_low_risk_change_is_done_at_once() {
    let w = wired();
    let r = w.tools.call(CHANGE_TOOL, call("flag", "readme"), "c").await;
    assert_eq!(r.status(), Some("done"));
    assert!(r.result["result"].as_str().unwrap().contains("add flagged"));
    assert!(r.result["state_now"].as_str().unwrap().contains("flagged"));
    assert!(!pending(&w.gate, "s"));
    assert_eq!(
        w.writer.issue(&doc("readme")).labels,
        vec!["needs-signoff", "flagged"]
    );
}

#[tokio::test]
async fn test_a_change_an_ask_rule_names_needs_confirmation() {
    let w = wired();
    let r = w.tools.call(CHANGE_TOOL, call("nuke", "readme"), "c").await;
    assert_eq!(r.status(), Some("needs_confirmation"));
    assert!(
        r.result["summary"]
            .as_str()
            .unwrap()
            .contains("add flagged")
    );
    assert!(pending(&w.gate, "s"));
    assert_eq!(w.writer.issue(&doc("readme")).labels, vec!["needs-signoff"]);
}

#[tokio::test]
async fn test_denied_op_errors_even_if_called() {
    let w = wired();
    let r = w
        .tools
        .call(CHANGE_TOOL, call("never", "readme"), "c")
        .await;
    assert!(r.error_text().is_some() && !pending(&w.gate, "s"));
}

#[tokio::test]
async fn test_change_rejects_disallowed_target_and_bad_args() {
    let w = wired();
    let r = w.tools.call(CHANGE_TOOL, call("flag", "other"), "c").await;
    assert!(r.error_text().unwrap().contains("not allowed"));
    let r = w
        .tools
        .call(CHANGE_TOOL, call("flag", "forbidden"), "c")
        .await;
    assert_eq!(Value::Object(r.result), json!({"error": "bad name"}));
}

#[tokio::test]
async fn test_dotted_operation_name_is_tolerated() {
    let w = wired();
    let r = w
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({"ability": "fake", "operation": "fake.flag", "args": {"name": "x"}})),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert!(
        w.writer
            .issue(&doc("x"))
            .labels
            .contains(&"flagged".to_string())
    );
    let r = w
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({"operation": "flag", "args": {"name": "y"}})),
            "c",
        )
        .await; // ability omitted
    assert_eq!(r.status(), Some("done"));
}

#[test]
fn test_no_undo_tool() {
    let w = wired();
    assert!(w.tools.schemas().get("undo").is_none());
}

#[tokio::test]
async fn test_hang_up_calls_back() {
    let w = wired();
    let r = w.tools.call("hang_up", Map::new(), "c").await;
    assert_eq!(*w.slept.lock().unwrap(), vec![true]);
    assert!(r.after.is_empty() && !r.run_llm); // the model's own goodbye is spoken, not a fixed word
}

#[tokio::test]
async fn test_the_tool_list_never_names_a_tool_twice_or_badly() {
    // The model API refuses the whole list over one name twice or one it can't take, so
    // every turn failed. Ghost's own tools keep their names; a later clash isn't offered.
    fn answer(tag: &'static str) -> ReadTool {
        ReadTool {
            name: String::new(),
            description: String::new(),
            properties: json!({}),
            required: vec![],
            handler: Arc::new(move |_| {
                Box::pin(async move {
                    let mut m = Map::new();
                    m.insert("from".into(), json!(tag));
                    Ok(m)
                })
            }),
            operation: String::new(),
        }
    }
    fn named(name: &str, description: &str, tag: &'static str) -> ReadTool {
        ReadTool {
            name: name.into(),
            description: description.into(),
            ..answer(tag)
        }
    }
    let skills = ToolSource {
        name: "skills".into(),
        read_tools: vec![named("read_skill", "Ghost's.", "skills")],
        ..Default::default()
    };
    let app = ToolSource {
        name: "read".into(),
        read_tools: vec![
            named("read_skill", "An app's.", "app"),
            named("hang_up", "An app's.", "app"),
            named("bad.name", "Bad.", "app"),
            named(&"x".repeat(65), "Long.", "app"),
        ],
        ..Default::default()
    };
    let other = ToolSource {
        name: "other".into(),
        read_tools: vec![named("read_skill", "Another.", "other")],
        ..Default::default()
    };
    let gate = Arc::new(WriteGate::new(Arc::new(Store::in_memory().unwrap())));
    let tools = ToolDispatcher::new(ToolsParams {
        sources: vec![app, other, skills],
        gate,
        session_id: "s".into(),
        on_hang_up: no_action(),
        on_hold: None,
        events: None,
        conversations: None,
        on_call: None,
        last_request: None,
        probe: None,
    });
    let schemas = tools.schemas();
    let names = schemas.names();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(names.len(), unique.len(), "{names:?}");
    let ok = regex::Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").unwrap();
    assert!(names.iter().all(|n| ok.is_match(n)), "{names:?}");
    assert_eq!(schemas.get("read_skill").unwrap().description, "Ghost's.");
    let r = tools.call("read_skill", Map::new(), "c").await;
    assert_eq!(Value::Object(r.result), json!({"from": "skills"}));
    let r = tools.call("bad.name", Map::new(), "c").await;
    assert!(r.error_text().unwrap().contains("not available"));
}

// --- the in-session side of conversations ----------------------------------------------

/// Ghost's folder (ADR 0035), with an AGENTS.md when `rules` is given.
fn layout(tmp: &Path, rules: &str) -> PathBuf {
    let cfg = tmp.join("ghost");
    std::fs::create_dir_all(&cfg).unwrap();
    if !rules.is_empty() {
        std::fs::write(cfg.join("AGENTS.md"), rules).unwrap();
    }
    cfg
}

struct World {
    convs: Arc<Conversations>,
    gate: Arc<WriteGate>,
    rt: Arc<ConversationRuntime>,
    tools: Arc<ToolDispatcher>,
}

fn record_call(rt: &Arc<ConversationRuntime>) -> ghost_voice::tools::OnCall {
    let rt = rt.clone();
    Arc::new(move |name, a| rt.record_call(name, a))
}

fn world(tmp: &Path) -> World {
    let store = Arc::new(Store::in_memory().unwrap());
    let convs = Arc::new(
        Conversations::new(
            store.clone(),
            layout(tmp, ""),
            ConversationsOptions {
                default_voice: "af_heart".into(),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let gate = Arc::new(WriteGate::new(store).with_autonomy(
        AutonomyPolicy::parse(&format!("[{SOURCE}]\nset_persona = 'announce'\n"), "test").unwrap(),
    ));
    let loaded = conversations_source(&convs);
    gate.register(SOURCE, loaded.operations.clone());
    let rt = ConversationRuntime::new(
        convs.clone(),
        gate.clone(),
        "s1",
        vec!["FRAG".into()],
        24000,
        2,
        GHOST,
        None,
        None,
    );
    let tools = ToolDispatcher::new(ToolsParams {
        sources: vec![loaded],
        gate: gate.clone(),
        session_id: "s1".into(),
        on_hang_up: no_action(),
        on_hold: None,
        events: None,
        conversations: Some(rt.clone()),
        on_call: Some(record_call(&rt)),
        last_request: None,
        probe: None,
    });
    World {
        convs,
        gate,
        rt,
        tools,
    }
}

async fn create(gate: &WriteGate, name: &str, extra: Value) {
    let op = gate.operation(SOURCE, "create").unwrap();
    let mut a = args(extra);
    a.insert("name".into(), json!(name));
    let prepared = (op.prepare)(a).await.unwrap();
    gate.submit(Request::new(
        "setup",
        SOURCE,
        "create",
        prepared.target,
        prepared.changes,
        "",
    ))
    .await
    .unwrap();
}

fn turns(convs: &Conversations, slug: &str) -> Vec<String> {
    convs
        .turns(slug, 100)
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect()
}

fn user_text(m: &Message) -> &str {
    match m {
        Message::User(t) => t,
        _ => "",
    }
}

#[test]
fn test_prompt_and_seed_follow_the_active_conversation() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    assert_eq!(w.rt.active().slug, GHOST);
    let prompt = w.rt.system_prompt(None);
    assert!(prompt.contains("You are Ghost") && prompt.contains("FRAG"));
    assert!(w.rt.initial_messages(None).is_empty());
    assert!(matches!(
        &w.rt.start_frames()[0],
        Frame::VoiceChange { voice, .. } if voice == "af_heart"
    ));

    w.convs
        .files
        .write(GHOST, "memory.md", "We were triaging acme/app.")
        .unwrap();
    w.convs
        .files
        .write(GHOST, "notes.md", "Sam owns payroll")
        .unwrap();
    w.rt.record("user", "hello");
    w.rt.record("assistant", "Hi.");
    let msgs = w.rt.initial_messages(None);
    let prefix = SUMMARY_TEMPLATE.split('{').next().unwrap();
    assert!(user_text(&msgs[0]).starts_with(prefix));
    assert!(user_text(&msgs[0]).contains("Sam owns payroll"));
    assert!(matches!(msgs.last(), Some(Message::User(_))));
    assert!(
        user_text(msgs.last().unwrap()).ends_with("Operator: hello\nYou: Hi.\n(end of the record)")
    );
}

#[tokio::test]
async fn test_switch_tool_transfers_with_announce_tone_voice_and_context() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    assert!(w.tools.schemas().get(SWITCH_TOOL).is_some());
    create(
        &w.gate,
        "Accounting",
        json!({"voice": "am_adam", "speed": 1.1}),
    )
    .await;
    w.convs
        .files
        .write("accounting", "persona.md", "Dry, precise, hates surprises.")
        .unwrap();
    w.convs
        .files
        .write("accounting", "memory.md", "Reconciling September.")
        .unwrap();
    w.rt.record("user", "patch me to accounting");

    let r = w
        .tools
        .call(
            SWITCH_TOOL,
            args(json!({"name": "the accounting team", "handoff": "Q3 invoices"})),
            "c",
        )
        .await;
    assert_eq!(
        Value::Object(r.result.clone()),
        json!({"status": "transferred", "to": "Accounting"})
    );
    assert!(!r.run_llm);
    assert!(!w.rt.settled()); // held until the new line has greeted

    let kinds: Vec<&str> = r.after.iter().map(Frame::kind).collect();
    assert_eq!(
        kinds,
        vec![
            "Speak",
            "AudioOut",
            "VoiceChange",
            "Marker",
            "SystemPrompt",
            "ContextUpdate"
        ]
    );
    assert!(matches!(&r.after[0], Frame::Speak(t) if t == "Transferring you to Accounting."));
    assert!(matches!(
        &r.after[2],
        Frame::VoiceChange { voice, speed } if voice == "am_adam" && (*speed - 1.1).abs() < 1e-9
    ));
    assert!(matches!(&r.after[3], Frame::Marker(s) if s == "accounting"));
    let Frame::SystemPrompt(prompt) = &r.after[4] else {
        panic!()
    };
    assert!(prompt.contains("You are Accounting, one of the operator's agents"));
    assert!(prompt.contains("hates surprises") && prompt.contains("FRAG"));
    let Frame::ContextUpdate { messages, run_llm } = &r.after[5] else {
        panic!()
    };
    assert!(*run_llm);
    assert!(user_text(&messages[0]).contains("Reconciling September"));
    let last = user_text(messages.last().unwrap());
    assert!(last.contains("transferred to you from Ghost") && last.contains("Q3 invoices"));
    assert_eq!(w.rt.active().slug, "accounting");

    // assistant text is attributed by the marker, not by the clock: what the tap
    // reports before the marker is Ghost's, after it Accounting's
    w.rt.record("assistant", "Transferring you to Accounting.");
    assert!(!w.rt.settled()); // Ghost's announce line is not the greeting
    w.rt.on_marker("accounting");
    w.rt.record(
        "assistant",
        "Accounting here. We were reconciling September.",
    );
    assert!(w.rt.settled());
    assert_eq!(
        turns(&w.convs, GHOST),
        vec![
            "patch me to accounting",
            "switch_conversation(handoff=Q3 invoices, name=the accounting team)",
            "transferred to Accounting",
            "Transferring you to Accounting.",
        ]
    );
    assert_eq!(
        turns(&w.convs, "accounting"),
        vec![
            "transferred from Ghost",
            "Accounting here. We were reconciling September.",
        ]
    );

    // back to Ghost
    let r = w
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "ghost"})), "c")
        .await;
    assert!(matches!(&r.after[0], Frame::Speak(t) if t == "Back to Ghost."));
    assert_eq!(w.rt.active().slug, GHOST);
}

#[tokio::test]
async fn test_switch_no_match_and_already_here() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    let r = w
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "marketing"})), "c")
        .await;
    assert_eq!(r.status(), Some("no_match"));
    assert_eq!(r.result["known"], json!(["Ghost"]));
    let r = w
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "ghost"})), "c")
        .await;
    assert_eq!(r.status(), Some("already_here"));
    assert!(r.after.is_empty());
}

#[tokio::test]
async fn test_switch_cancels_pending_and_policy_can_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    create(&w.gate, "Legal", json!({})).await;
    // a confirm-level change is waiting; the transfer drops it
    let op = w.gate.operation(SOURCE, "set_persona").unwrap();
    let prepared = (op.prepare)(args(json!({"_active": GHOST, "text": "x"})))
        .await
        .unwrap();
    w.gate
        .prepare(Request::new(
            "s1",
            SOURCE,
            "set_persona",
            prepared.target,
            prepared.changes,
            "",
        ))
        .await
        .unwrap();
    assert!(pending(&w.gate, "s1"));
    let _ = w
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "legal"})), "c")
        .await;
    assert!(!pending(&w.gate, "s1"));
    assert_eq!(w.rt.active().slug, "legal");

    w.gate.set_autonomy(
        AutonomyPolicy::parse(&format!("[{SOURCE}]\nswitch = 'deny'\n"), "test").unwrap(),
    );
    let r = w
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "ghost"})), "c")
        .await;
    assert!(r.error_text().unwrap().contains("not allowed"));
    assert_eq!(w.rt.active().slug, "legal");
}

fn context_update(after: &[Frame]) -> &Vec<Message> {
    after
        .iter()
        .find_map(|f| match f {
            Frame::ContextUpdate { messages, .. } => Some(messages),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn test_save_as_creates_switches_and_carries_turns() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    w.rt.record("user", "let's go over the invoices");
    w.rt.record("assistant", "Sure, which ones?");
    w.rt.record("user", "save this as accounting");
    let r = w
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({"ability": SOURCE, "operation": "create", "args": {"name": "Accounting"}})),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert_eq!(w.rt.active().slug, "accounting");
    assert_eq!(
        &turns(&w.convs, "accounting")[..3],
        [
            "let's go over the invoices",
            "Sure, which ones?",
            "save this as accounting"
        ]
    );
    assert_eq!(turns(&w.convs, GHOST), vec!["transferred to Accounting"]);
    let messages = context_update(&r.after);
    assert!(
        user_text(&messages[0])
            .contains("Operator: let's go over the invoices\nYou: Sure, which ones?")
    );
    assert!(user_text(messages.last().unwrap()).contains("was just hired"));
}

#[tokio::test]
async fn test_read_tool_and_change_records_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    let r = w.tools.call("list_agents", Map::new(), "c").await;
    assert_eq!(r.result["agents"][0]["name"], json!("Ghost"));
    assert_eq!(turns(&w.convs, GHOST), vec!["list_agents()"]);
}

#[tokio::test]
async fn test_summary_applied_persists_memory_and_marks_turns() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    for i in 0..6 {
        w.rt.record("user", &format!("u{i}"));
        w.rt.record("assistant", &format!("a{i}"));
    }
    let before = w.convs.last_turn_id(GHOST).unwrap().unwrap();
    w.rt.on_summary_applied(&[
        Message::user(SUMMARY_TEMPLATE.replace("{summary}", "Six exchanges.")),
        Message::user("u5"),
        Message::assistant("a5"),
    ])
    .await;
    assert_eq!(
        w.convs.files.memory(GHOST).unwrap().trim(),
        "Six exchanges."
    );
    let versions = w.convs.compactions(GHOST).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].source, CompactionSource::Auto);
    // keep_messages=2: the last two spoken turns stay raw
    let recent: Vec<String> = w
        .convs
        .resume(GHOST)
        .unwrap()
        .recent
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert_eq!(recent, vec!["u5", "a5"]);
    assert_eq!(versions[0].through_turn_id, Some(before - 2));
    // not a summary message: nothing happens
    w.rt.on_summary_applied(&[Message::user("just a turn")])
        .await;
    assert_eq!(w.convs.compactions(GHOST).unwrap().len(), 1);
}

// --- every agent's reach: one set of permissions (ADR 0034, ADR 0035) ----------------

/// Like a connected GitHub app's writer: applies a label to an app target.
#[derive(Default)]
struct RepoWriter {
    applied: Mutex<Vec<(String, i64, String)>>,
}

#[async_trait]
impl Writer for RepoWriter {
    async fn fetch(&self, _target: &Target) -> anyhow::Result<Snapshot> {
        Ok(Snapshot::App(AppSnapshot {
            version: "1".into(),
            checked: false,
            result: None,
        }))
    }

    async fn apply(&self, target: &Target, change: &Change, _key: &str) -> anyhow::Result<()> {
        let Target::App(t) = target else {
            anyhow::bail!("not an app target")
        };
        let Change::App(c) = change else {
            anyhow::bail!("not an app change")
        };
        self.applied.lock().unwrap().push((
            t.fields["repo"].as_str().unwrap().to_string(),
            t.fields["number"].as_i64().unwrap(),
            c.arguments["tag"].as_str().unwrap().to_string(),
        ));
        Ok(())
    }
}

/// Like a connected GitHub app (ADR 0034): every agent reaches it, under the operator's
/// one set of permissions (ADR 0035).
fn repo_source(writer: Arc<RepoWriter>, name: &str) -> ToolSource {
    let list = ReadTool {
        name: format!("{name}_list_issues"),
        description: "List.".into(),
        properties: json!({"repo": {"type": "string"}}),
        required: vec![],
        handler: Arc::new(|a| {
            Box::pin(async move {
                let repos = match a.get("repo").and_then(Value::as_str) {
                    Some(r) if !r.is_empty() => json!([r]),
                    _ => json!(["acme/app", "acme/infra"]),
                };
                let mut m = Map::new();
                m.insert("repos".into(), repos);
                Ok(m)
            })
        }),
        operation: "list_issues".into(),
    };
    let n = name.to_string();
    let set_labels = Operation {
        name: "set_labels".into(),
        description: "Label.".into(),
        properties: json!({"repo": {"type": "string"}, "number": {"type": "integer"}}),
        required: vec!["number".into()],
        prepare: Arc::new(move |a| {
            let n = n.clone();
            Box::pin(async move {
                let mut fields = Map::new();
                fields.insert("repo".into(), a["repo"].clone());
                fields.insert("number".into(), json!(a["number"].as_i64().unwrap_or(0)));
                let mut arguments = Map::new();
                arguments.insert("tag".into(), json!("bug"));
                Ok(Prepared {
                    target: Target::App(AppTarget {
                        app: n,
                        tool: "set_labels".into(),
                        fields,
                        title: String::new(),
                    }),
                    changes: ChangeSet::one(Change::App(AppChange {
                        tool: "set_labels".into(),
                        arguments,
                    })),
                    rationale: String::new(),
                    asked_by: Vec::new(),
                })
            })
        }),
        writer,
        risk: Risk::Medium,
        allowed: Arc::new(|_| true),
        asked_by: Vec::new(),
        tool_name: Some(format!("{name}_set_labels")),
    };
    ToolSource {
        name: name.into(),
        prompt: format!("{} is connected.", name.to_uppercase()),
        read_tools: vec![list],
        operations: vec![set_labels],
        ..Default::default()
    }
}

const RULES: &str = "# Ghost\n\nUK dates.\n";
const BLOCK_SECRET: &str = "[permissions]\nblock = [\"github.*(repo=acme/secret)\"]\n";

struct Agents {
    convs: Arc<Conversations>,
    gate: Arc<WriteGate>,
    rt: Arc<ConversationRuntime>,
    tools: Arc<ToolDispatcher>,
    writer: Arc<RepoWriter>,
}

/// Ghost and a hired agent reach one github app; the operator's permissions block every
/// call about acme/secret, whichever agent makes it.
fn agents(tmp: &Path) -> Agents {
    let store = Arc::new(Store::in_memory().unwrap());
    let writer = Arc::new(RepoWriter::default());
    let convs = Arc::new(
        Conversations::new(
            store.clone(),
            layout(tmp, RULES),
            ConversationsOptions {
                default_voice: "af_heart".into(),
                voices: vec!["af_heart".into(), "am_adam".into(), "bf_emma".into()],
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let gate = Arc::new(
        WriteGate::new(store).with_autonomy(AutonomyPolicy::parse(BLOCK_SECRET, "test").unwrap()),
    );
    let agents = conversations_source(&convs);
    gate.register(SOURCE, agents.operations.clone());
    let github = repo_source(writer.clone(), "github");
    gate.register("github", github.operations.clone());
    let (g, a) = (github.clone(), agents.clone());
    let rt = ConversationRuntime::new(
        convs.clone(),
        gate.clone(),
        "s1",
        vec!["FRAG".into()],
        24000,
        20,
        GHOST,
        None,
        Some(Arc::new(move |_p: &Profile| vec![g.clone(), a.clone()])),
    );
    let tools = ToolDispatcher::new(ToolsParams {
        sources: vec![github, agents],
        gate: gate.clone(),
        session_id: "s1".into(),
        on_hang_up: no_action(),
        on_hold: None,
        events: None,
        conversations: Some(rt.clone()),
        on_call: Some(record_call(&rt)),
        last_request: Some(Arc::new(|| {
            ("label app seventeen bug".into(), String::new())
        })),
        probe: None,
    });
    Agents {
        convs,
        gate,
        rt,
        tools,
        writer,
    }
}

async fn hire(gate: &WriteGate, name: &str, extra: Value) {
    create(gate, name, extra).await;
}

#[tokio::test]
async fn test_every_agent_reaches_the_same_apps_and_reads_ghosts_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let a = agents(tmp.path());
    hire(&a.gate, "Brand", json!({"job": "the look"})).await;
    // Ghost: the app, the rules, the roster of agents by name and job
    let prompt = a.rt.system_prompt(None);
    assert!(prompt.contains("GITHUB is connected.") && !prompt.contains("FRAG"));
    assert!(prompt.contains("standing instructions, for every agent:\n# Ghost\n\nUK dates."));
    assert!(prompt.contains("other agents, for transfers:\n- Brand: the look"));
    assert!(!prompt.to_lowercase().contains("department"));
    let r = a.tools.call("github_list_issues", Map::new(), "c").await;
    assert_eq!(
        Value::Object(r.result),
        json!({"repos": ["acme/app", "acme/infra"]})
    );

    let brand = a.convs.get("brand").unwrap().unwrap();
    a.rt.switch(&brand, "", "").await;
    let prompt = a.rt.system_prompt(None);
    assert!(prompt.contains("GITHUB is connected.")); // every agent reaches the app
    assert!(prompt.contains("You are Brand, one of the operator's agents, hired for the look"));
    assert!(prompt.contains("UK dates.")); // Ghost's AGENTS.md is in every agent's prompt
    assert!(!prompt.contains("for transfers")); // only Ghost lists them
    assert!(!prompt.to_lowercase().contains("department"));
    let r = a
        .tools
        .call(
            "github_set_labels",
            args(json!({"repo": "acme/app", "number": 17})),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert_eq!(
        *a.writer.applied.lock().unwrap(),
        vec![("acme/app".to_string(), 17, "bug".to_string())]
    );
}

#[tokio::test]
async fn test_what_the_permissions_block_is_refused_for_every_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let a = agents(tmp.path());
    hire(&a.gate, "Brand", json!({})).await;
    for slug in [GHOST, "brand"] {
        let p = a.convs.get(slug).unwrap().unwrap();
        a.rt.switch(&p, "", "").await;
        let r = a
            .tools
            .call(
                "github_list_issues",
                args(json!({"repo": "acme/secret"})),
                "c",
            )
            .await;
        assert!(r.error_text().unwrap().contains("blocked"));
        let r = a
            .tools
            .call(
                "github_set_labels",
                args(json!({"repo": "acme/secret", "number": 17})),
                "c",
            )
            .await;
        assert!(r.error_text().unwrap().contains("blocked"));
        assert!(a.writer.applied.lock().unwrap().is_empty());
        assert!(!pending(&a.gate, "s1"));
    }
}

#[tokio::test]
async fn test_a_voice_hired_agent_and_the_tools_follow() {
    let tmp = tempfile::tempdir().unwrap();
    let a = agents(tmp.path());
    a.rt.record("user", "hire someone for the release notes");
    let r = a
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({
                "ability": SOURCE,
                "operation": "create",
                "args": {"name": "Release Notes", "job": "release notes", "voice": "am_adam"},
            })),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert_eq!(a.rt.active().slug, "release-notes");
    let agent = a.convs.get("release-notes").unwrap().unwrap();
    assert_eq!(agent.job, "release notes");
    assert_eq!(agent.voice.as_deref(), Some("am_adam"));
    assert!(
        a.rt.system_prompt(None).contains(
            "You are Release Notes, one of the operator's agents, hired for release notes"
        )
    );
    let names: Vec<String> = a.rt.sources_for(None).into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["github", SOURCE]);
}

#[tokio::test]
async fn test_a_transfer_names_an_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let a = agents(tmp.path());
    hire(&a.gate, "Maya", json!({"aliases": ["the brand agent"]})).await;
    assert_eq!(a.rt.resolve("Maya").unwrap().slug, "maya");
    assert_eq!(a.rt.resolve("the brand agent").unwrap().slug, "maya");
    assert!(a.rt.resolve("marketing").is_none());
    let r = a
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "marketing"})), "c")
        .await;
    assert_eq!(r.status(), Some("no_match"));
    assert_eq!(r.result["known"], json!(["Ghost", "Maya"]));
    let r = a
        .tools
        .call(SWITCH_TOOL, args(json!({"name": "maya"})), "c")
        .await;
    assert_eq!(
        Value::Object(r.result),
        json!({"status": "transferred", "to": "Maya"})
    );
    assert_eq!(a.rt.active().slug, "maya");
}

#[tokio::test]
async fn test_retiring_the_active_agent_returns_to_ghost() {
    let tmp = tempfile::tempdir().unwrap();
    let a = agents(tmp.path());
    hire(&a.gate, "Brand", json!({})).await;
    let brand = a.convs.get("brand").unwrap().unwrap();
    a.rt.switch(&brand, "", "").await;
    let r = a
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({"ability": SOURCE, "operation": "retire", "args": {}})),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert!(a.convs.get("brand").unwrap().unwrap().retired());
    assert_eq!(a.rt.active().slug, GHOST);
    assert!(matches!(&r.after[0], Frame::Speak(t) if t == "Back to Ghost."));
    assert!(a.convs.resolve("brand", false).unwrap().is_none());
    // retiring another agent from Ghost stays put
    hire(&a.gate, "Social", json!({})).await;
    let r = a
        .tools
        .call(
            CHANGE_TOOL,
            args(json!({"ability": SOURCE, "operation": "retire", "args": {"conversation": "social"}})),
            "c",
        )
        .await;
    assert_eq!(r.status(), Some("done"));
    assert_eq!(a.rt.active().slug, GHOST);
}

#[tokio::test]
async fn test_tools_connected_after_the_session_was_built_are_dispatched() {
    // Every read goes through one catch-all dispatched to the active agent's reach, so an
    // app connected since is there at once (ADR 0034).
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let convs = Arc::new(
        Conversations::new(
            store.clone(),
            layout(tmp.path(), ""),
            ConversationsOptions {
                default_voice: "af_heart".into(),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let gate = Arc::new(WriteGate::new(store).with_autonomy(AutonomyPolicy::default()));
    let agents = conversations_source(&convs);
    gate.register(SOURCE, agents.operations.clone());
    let figma = ToolSource {
        name: "figma".into(),
        read_tools: vec![ReadTool {
            name: "figma_get_file".into(),
            description: "x".into(),
            properties: json!({}),
            required: vec![],
            handler: Arc::new(|_| {
                Box::pin(async {
                    let mut m = Map::new();
                    m.insert("ok".into(), json!(true));
                    Ok(m)
                })
            }),
            operation: "get_file".into(),
        }],
        operations: vec![Operation {
            name: "export".into(),
            description: "x".into(),
            properties: json!({}),
            required: vec![],
            prepare: Arc::new(|_| Box::pin(async { anyhow::bail!("unused") })),
            writer: Arc::new(RepoWriter::default()),
            risk: Risk::Medium,
            allowed: Arc::new(|_| true),
            asked_by: vec![],
            tool_name: Some("figma_export".into()),
        }],
        ..Default::default()
    };
    let connected: Arc<Mutex<Vec<ToolSource>>> = Arc::new(Mutex::new(Vec::new())); // figma connects later
    let (c, a) = (connected.clone(), agents.clone());
    let rt = ConversationRuntime::new(
        convs.clone(),
        gate.clone(),
        "s",
        vec![],
        24000,
        20,
        GHOST,
        None,
        Some(Arc::new(move |_p: &Profile| {
            let mut out = c.lock().unwrap().clone();
            out.push(a.clone());
            out
        })),
    );
    let tools = ToolDispatcher::new(ToolsParams {
        sources: vec![agents],
        gate: gate.clone(),
        session_id: "s".into(),
        on_hang_up: no_action(),
        on_hold: None,
        events: None,
        conversations: Some(rt.clone()),
        on_call: None,
        last_request: None,
        probe: None,
    });
    *connected.lock().unwrap() = vec![figma.clone()];
    gate.register("figma", figma.operations.clone());
    let advertised: Vec<String> = rt
        .tool_schemas(&rt.sources_for(None))
        .unwrap()
        .names()
        .into_iter()
        .map(String::from)
        .collect();
    assert!(advertised.contains(&"figma_get_file".to_string()));
    assert!(advertised.contains(&"figma_export".to_string()));
    let r = tools.call("figma_get_file", Map::new(), "c").await;
    assert_eq!(Value::Object(r.result), json!({"ok": true}));
    gate.set_autonomy(
        AutonomyPolicy::parse("[permissions]\nblock = [\"figma\"]\n", "test").unwrap(),
    );
    let advertised: Vec<String> = rt
        .tool_schemas(&rt.sources_for(None))
        .unwrap()
        .names()
        .into_iter()
        .map(String::from)
        .collect();
    assert!(!advertised.contains(&"figma_get_file".to_string()));
    let r = tools.call("figma_get_file", Map::new(), "c").await;
    assert!(r.error_text().unwrap().contains("not available here"));
}
