//! ADR 0052: the wider tools of Ghost's own app, each through the gate as the session's
//! dispatcher submits it, with its happy path and its refusals (another agent's data, the
//! wrong caller, an outside client), against temp dirs and the fakes in the tree. Nothing
//! here opens the operator's folder, keyring, microphone or network.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use super::agents::{ASKING_MOST, AskAgent};
use super::outside::AgentRunner;
use super::*;
use crate::apps::{AppSpec, AuthFor, CALLER_ARG, SESSION_ARG};
use crate::catalog::Catalog;
use crate::conversations::models::{Profile, Role};
use crate::conversations::{Conversations, ConversationsOptions, SOURCE as CONVERSATIONS};
use crate::gate::{Request, Submission, WriteGate};
use crate::levels::Autonomy;
use crate::models::OutcomeStatus;
use crate::policy::AutonomyPolicy;
use crate::routes::RouteEngine;
use crate::skills::Skills;
use crate::waiting::{Items, Response};

const SECRET: &str = "sk-zebra-0123456789-recognisable";

/// Every app that signs in needs the operator's browser: no network, ever.
struct NoBrowser;

#[async_trait]
impl AuthFor for NoBrowser {
    async fn auth_for(
        &self,
        spec: &AppSpec,
        _http: reqwest::Client,
        _interactive: bool,
        _fresh: bool,
    ) -> Result<Option<rmcp::transport::auth::AuthClient<reqwest::Client>>, AppError> {
        Err(AppError::NeedsSignIn(format!(
            "{} needs you to sign in",
            spec.shown()
        )))
    }
}

/// A written turn with an agent, as the daemon's runner gives one; it may be held.
#[derive(Default)]
struct Runner {
    asked: Mutex<Vec<(String, String, String)>>,
    hold: Option<Arc<tokio::sync::Semaphore>>,
}

#[async_trait]
impl AgentRunner for Runner {
    async fn ask(&self, agent: &Profile, text: &str, from: &str) -> Result<String, String> {
        self.asked
            .lock()
            .unwrap()
            .push((agent.slug.clone(), text.to_string(), from.to_string()));
        if let Some(hold) = &self.hold {
            let _ = hold.acquire().await;
        }
        Ok(format!("{} did it.", agent.name))
    }
}

struct World {
    cfg: PathBuf,
    store: Arc<Store>,
    convs: Arc<Conversations>,
    apps: Arc<Apps>,
    waiting: Arc<WaitingList>,
    routes: Arc<RouteEngine>,
    server: GhostServer,
    gate: WriteGate,
    runner: Arc<Runner>,
    _attached: Attached,
    _tmp: tempfile::TempDir,
}

fn catalog_file(cfg: &Path) -> PathBuf {
    let record = |server: Value| json!({"server": server, "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active"}}});
    let body = json!({
        "fetched_at": 1.0,
        "records": {
            "app.linear/linear": record(json!({
                "name": "app.linear/linear", "title": "Linear",
                "description": "Issues and projects.", "version": "1.0.0",
                "remotes": [{"type": "streamable-http", "url": "http://127.0.0.1:9/mcp"}],
            })),
            "com.example/keyed": record(json!({
                "name": "com.example/keyed", "title": "Keyed",
                "description": "Needs a key.", "version": "1.0.0",
                "remotes": [{"type": "streamable-http", "url": "http://127.0.0.1:9/k",
                    "headers": [{"name": "Authorization", "description": "Your token", "isSecret": true, "isRequired": true}]}],
            })),
        },
    });
    let path = cfg.join("catalog.json");
    std::fs::write(&path, body.to_string()).unwrap();
    path
}

async fn world_with(policy: &str, runner: Runner) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("ghost");
    std::fs::create_dir_all(&cfg).unwrap();
    // The operator's own: an app Ghost has not connected, a key of their own, a hook.
    std::fs::write(
        cfg.join("apps.json"),
        "{\n  \"apps\": {\"keep\": {\"command\": \"keep-mcp\"}},\n  \"theirs\": 1\n}\n",
    )
    .unwrap();
    std::fs::write(cfg.join("hooks.json"), "{\"gh\": {\"theirs\": true}}\n").unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let convs =
        Arc::new(Conversations::new(store.clone(), &cfg, ConversationsOptions::default()).unwrap());
    convs.ensure_ghost().unwrap();
    let gate = WriteGate::new(store.clone())
        .with_autonomy(AutonomyPolicy::parse(policy, "<test>").unwrap());
    gate.register(
        CONVERSATIONS,
        crate::conversations::conversations_source(&convs).operations,
    );
    for name in ["Ledger", "Brand"] {
        let op = gate.operation(CONVERSATIONS, "create").unwrap();
        let p = (op.prepare)(json!({"name": name}).as_object().cloned().unwrap())
            .await
            .unwrap();
        gate.submit(Request::new(
            "setup",
            CONVERSATIONS,
            "create",
            p.target,
            p.changes,
            "",
        ))
        .await
        .unwrap();
    }
    let apps = Arc::new(Apps::new(
        vec![AppSpec::runs("keep", "keep-mcp")],
        Arc::new(HashMap::<String, String>::new()),
        Some(Arc::new(NoBrowser) as Arc<dyn AuthFor>),
    ));
    let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
    let routes = Arc::new(RouteEngine::new(Items::new(store.clone())).unwrap());
    let skills = Arc::new(Skills::new(&cfg.join("skills"), None));
    let runner = Arc::new(runner);
    let policy_text = policy.to_string();
    crate::redact::also_redact(
        "wide-tests",
        [("OPENAI_API_KEY".to_string(), SECRET.to_string())],
    );
    let server = GhostServer::new(Parts {
        store: store.clone(),
        waiting: waiting.clone(),
        apps: Arc::downgrade(&apps),
        status: Arc::new(|| {
            json!({"version": "2026.11.0", "model": {"provider": "anthropic", "model": "claude"}})
                .as_object()
                .cloned()
                .unwrap()
        }),
    })
    .with_more(More {
        agents: Some(convs.clone()),
        config_dir: Some(cfg.clone()),
        catalog: Some(Arc::new(Catalog::new(catalog_file(&cfg)))),
        forget: None,
        watches: None,
        refresh: None,
        routes: Some(ready(routes.clone())),
        policy: Some(Arc::new(move || {
            AutonomyPolicy::parse(&policy_text, "<test>").unwrap()
        })),
        settings: Some(Arc::new(|| {
            BTreeMap::from([
                ("GHOST_TTS_VOICE".to_string(), "af_heart".to_string()),
                ("OPENAI_API_KEY".to_string(), SECRET.to_string()),
                (
                    "GHOST_PHONE_LISTEN".to_string(),
                    "10.0.0.7:47823".to_string(),
                ),
                ("GHOST_NOTE".to_string(), format!("Bearer {SECRET}")),
            ])
        })),
        skills: Some(skills),
        runner: Some(runner.clone() as Arc<dyn AgentRunner>),
    });
    let attached = attach(&apps, server.clone()).await.unwrap();
    apps.attach(super::tests::wordy("wordy").await);
    gate.register(NAME, attached.app.operations());
    World {
        cfg,
        store,
        convs,
        apps,
        waiting,
        routes,
        server,
        gate,
        runner,
        _attached: attached,
        _tmp: tmp,
    }
}

async fn world() -> World {
    world_with("", Runner::default()).await
}

/// A change as the session's dispatcher submits it: the agent and the session set.
async fn change(w: &World, agent: &str, session: &str, tool: &str, args: Value) -> Submission {
    let op = w.gate.operation(NAME, tool).unwrap();
    let mut args = args.as_object().cloned().unwrap();
    args.insert(CALLER_ARG.into(), json!(agent));
    args.insert(SESSION_ARG.into(), json!(session));
    let prepared = (op.prepare)(args).await.unwrap();
    let mut req = Request::new(
        session,
        NAME,
        tool,
        prepared.target,
        prepared.changes,
        "test",
    );
    req.conversation = Some(agent.to_string());
    w.gate.submit(req).await.unwrap()
}

fn done(sub: &Submission) -> Result<Map<String, Value>, String> {
    let outcome = sub.outcome.as_ref().expect("it ran");
    match outcome.status {
        OutcomeStatus::Success => match &outcome.readback {
            Some(crate::models::Snapshot::App(a)) => Ok(a.result.clone().unwrap_or_default()),
            _ => Ok(Map::new()),
        },
        _ => Err(outcome.error.clone().unwrap_or_default()),
    }
}

/// A read as the model calls it, with the agent the dispatcher names.
async fn read(w: &World, agent: &str, tool: &str, args: Value) -> Map<String, Value> {
    let src = w
        .apps
        .sources()
        .into_iter()
        .find(|s| s.name == NAME)
        .unwrap();
    let t = src
        .read_tools
        .iter()
        .find(|t| t.name == tool)
        .unwrap_or_else(|| panic!("no read {tool}"));
    let mut args = args.as_object().cloned().unwrap();
    args.insert(CALLER_ARG.into(), json!(agent));
    (t.handler)(args).await.unwrap()
}

fn id_of(w: &World, slug: &str) -> String {
    w.convs.get(slug).unwrap().unwrap().id
}

// --- 1. recall ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_finds_an_agents_own_words_and_ghost_finds_anyones() {
    let w = world().await;
    let (ledger, brand) = (id_of(&w, "ledger"), id_of(&w, "brand"));
    w.store
        .add_turn(
            &ledger,
            "s1",
            Role::User,
            "what did we decide about the turn grace in August",
        )
        .unwrap();
    w.store
        .add_turn(&ledger, "s1", Role::Assistant, "Half a second of grace.")
        .unwrap();
    w.store
        .add_turn(
            &ledger,
            "run-r1",
            Role::System,
            "[scheduled run: books] grace check",
        )
        .unwrap();
    w.store
        .add_turn(&brand, "s2", Role::User, "the logo needs grace")
        .unwrap();
    let mine = read(&w, "ledger", "ghost_recall", json!({"query": "turn grace"})).await;
    let results = mine["results"].as_array().unwrap();
    assert_eq!(results[0]["agent"], "ledger");
    assert_eq!(results[0]["said_by"], "the operator");
    assert_eq!(results[0]["kind"], "session");
    assert!(results[0]["text"].as_str().unwrap().contains("[grace]"));
    assert_eq!(results[0]["after"][0]["text"], "Half a second of grace.");
    // Every hit is Ledger's; a run's turn says it is one.
    let all_mine = read(&w, "ledger", "ghost_recall", json!({"query": "grace"})).await;
    let hits = all_mine["results"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    assert!(hits.iter().all(|h| h["agent"] == "ledger"));
    assert!(hits.iter().any(|h| h["kind"] == "scheduled run"));
    // Another agent's transcript is not Ledger's to read.
    let refused = read(
        &w,
        "ledger",
        "ghost_recall",
        json!({"query": "logo", "agent": "brand"}),
    )
    .await;
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Ghost reads any")
    );
    // Ghost reads any agent's, or all.
    let ghost = read(
        &w,
        "ghost",
        "ghost_recall",
        json!({"query": "logo", "agent": "brand"}),
    )
    .await;
    assert_eq!(ghost["results"][0]["agent"], "brand");
    let every = read(
        &w,
        "ghost",
        "ghost_recall",
        json!({"query": "grace", "agent": "all"}),
    )
    .await;
    assert_eq!(every["results"].as_array().unwrap().len(), 4);
    // With no agent named by the dispatcher, nothing.
    let src = w
        .apps
        .sources()
        .into_iter()
        .find(|s| s.name == NAME)
        .unwrap();
    let t = src
        .read_tools
        .iter()
        .find(|t| t.name == "ghost_recall")
        .unwrap();
    let nobody = (t.handler)(json!({"query": "grace"}).as_object().cloned().unwrap())
        .await
        .unwrap();
    assert!(nobody["error"].as_str().unwrap().contains("which agent"));
}

#[tokio::test]
async fn transcript_read_pages_back_and_memory_read_is_ones_own() {
    let w = world().await;
    let ledger = id_of(&w, "ledger");
    for i in 0..5 {
        w.store
            .add_turn(&ledger, "s1", Role::User, &format!("turn {i}"))
            .unwrap();
    }
    let page = read(&w, "ledger", "ghost_transcript_read", json!({"limit": 2})).await;
    let texts: Vec<&str> = page["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["turn 3", "turn 4"]);
    let older = read(
        &w,
        "ledger",
        "ghost_transcript_read",
        json!({"limit": 2, "before": page["next_before"]}),
    )
    .await;
    assert_eq!(older["turns"][1]["text"], "turn 2");
    let refused = read(
        &w,
        "brand",
        "ghost_transcript_read",
        json!({"agent": "ledger"}),
    )
    .await;
    assert!(refused["error"].is_string());

    std::fs::write(
        w.convs.dir_for("ledger").join("memory.md"),
        "Pays on the 5th.",
    )
    .unwrap();
    std::fs::write(w.convs.dir_for("ledger").join("persona.md"), "Dry.").unwrap();
    let mem = read(&w, "ledger", "ghost_memory_read", json!({})).await;
    assert_eq!(mem["memory"], "Pays on the 5th.");
    assert_eq!(mem["persona"], "Dry.");
    let refused = read(&w, "brand", "ghost_memory_read", json!({"agent": "ledger"})).await;
    assert!(refused["error"].as_str().unwrap().contains("own memory"));
    let ghost = read(&w, "ghost", "ghost_memory_read", json!({"agent": "ledger"})).await;
    assert_eq!(ghost["memory"], "Pays on the 5th.");
}

#[tokio::test]
async fn an_outside_client_recalls_only_the_turns_of_runs_it_started() {
    let w = world().await;
    let ledger = id_of(&w, "ledger");
    let mine = format!("run-ask-cursor-{}", "a".repeat(32));
    let other = format!("run-ask-cursor-bot-{}", "b".repeat(32));
    w.store
        .add_turn(
            &ledger,
            &mine,
            Role::System,
            "[asked by Cursor, an outside tool] the invoice",
        )
        .unwrap();
    w.store
        .add_turn(
            &ledger,
            &other,
            Role::System,
            "[asked by Cursor Bot] the invoice too",
        )
        .unwrap();
    w.store
        .add_turn(&ledger, "s1", Role::User, "the invoice is private")
        .unwrap();
    let client = w.server.for_caller(Caller::Named("client:cursor".into()));
    let found = client
        .do_recall(super::recall::RecallArgs {
            query: "invoice".into(),
            ..Default::default()
        })
        .unwrap();
    let hits = found["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["kind"], "asked by an outside tool");
    let page = client
        .do_transcript_read(super::recall::TranscriptArgs::default())
        .unwrap();
    assert_eq!(page["turns"].as_array().unwrap().len(), 1);
}

// --- 2. apps -----------------------------------------------------------------------------

#[tokio::test]
async fn connecting_an_app_asks_first_and_keeps_the_operators_other_entries() {
    let w = world().await;
    // The catalog is Ghost's copy, read with no network.
    let found = read(
        &w,
        "ledger",
        "ghost_apps_catalog_search",
        json!({"query": "linear"}),
    )
    .await;
    assert_eq!(found["apps"][0]["app"], "app.linear/linear");
    assert_eq!(
        w.gate.level(NAME, "app_connect", None, None).unwrap(),
        Autonomy::Confirm
    );
    let sub = change(&w, "ledger", "s1", "app_connect", json!({"app": "linear"})).await;
    // No rule names it: it waits for the operator's yes, and nothing changed yet.
    assert_eq!(sub.autonomy, Autonomy::Confirm);
    assert!(sub.outcome.is_none());
    assert!(
        !std::fs::read_to_string(w.cfg.join("apps.json"))
            .unwrap()
            .contains("linear")
    );
    // The operator says yes: the entry is written beside theirs, and it needs a sign-in.
    let outcome = w
        .gate
        .confirm_pending("s1", "test", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.status,
        OutcomeStatus::Success,
        "{:?}",
        outcome.error
    );
    let text = std::fs::read_to_string(w.cfg.join("apps.json")).unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["apps"]["keep"]["command"], "keep-mcp");
    assert_eq!(file["theirs"], 1);
    assert_eq!(file["apps"]["linear"]["url"], "http://127.0.0.1:9/mcp");
    let parsed = crate::apps::parse_apps_json(&text, "apps.json").unwrap();
    assert!(parsed.names().contains(&"linear"));
    assert!(
        !parsed.problems.iter().any(|p| p.contains("linear")),
        "{:?}",
        parsed.problems
    );
    assert_eq!(w.apps.state("linear"), "needs_sign_in");
    let items: Vec<String> = w
        .waiting
        .items
        .open(Some(Response::Tell))
        .unwrap()
        .into_iter()
        .filter(|i| i.source == "agent:ledger")
        .map(|i| i.title)
        .collect();
    assert_eq!(items, ["Linear needs you to sign in"]);

    // One that needs a key: nothing written, the operator told.
    let w2 = world_with(
        "[permissions]\nallow = [\"ghost.app_connect\"]\n",
        Runner::default(),
    )
    .await;
    let sub = change(
        &w2,
        "ledger",
        "s1",
        "app_connect",
        json!({"app": "com.example/keyed"}),
    )
    .await;
    let out = done(&sub).unwrap();
    assert_eq!(out["status"], "needs the operator");
    assert!(
        !std::fs::read_to_string(w2.cfg.join("apps.json"))
            .unwrap()
            .contains("keyed")
    );
    // A key an agent tries to pass is refused.
    let sub = change(
        &w2,
        "ledger",
        "s1",
        "app_connect",
        json!({"app": "com.example/keyed", "settings": {"Authorization": "abc"}}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("typed by the operator"));
}

#[tokio::test]
async fn disconnecting_asks_first_and_app_tools_reads_an_apps_list() {
    let w = world_with(
        "[permissions]\nallow = [\"ghost.app_disconnect\"]\n",
        Runner::default(),
    )
    .await;
    let tools = read(&w, "ledger", "ghost_app_tools", json!({"name": "wordy"})).await;
    assert_eq!(tools["tools"][0]["name"], "wordy_search");
    assert!(
        read(&w, "ledger", "ghost_app_tools", json!({"name": "ghost"})).await["error"].is_string()
    );
    assert_eq!(
        world()
            .await
            .gate
            .level(NAME, "app_disconnect", None, None)
            .unwrap(),
        Autonomy::Confirm
    );
    let sub = change(
        &w,
        "ledger",
        "s1",
        "app_disconnect",
        json!({"name": "keep"}),
    )
    .await;
    assert_eq!(done(&sub).unwrap()["status"], "disconnected");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(w.cfg.join("apps.json")).unwrap()).unwrap();
    assert!(file["apps"].get("keep").is_none());
    assert_eq!(file["theirs"], 1);
    assert!(w.apps.spec("keep").is_none());
}

// --- 3. watches --------------------------------------------------------------------------

fn decl() -> Value {
    json!({
        "app": "wordy", "read": "search", "every": 60,
        "fields": {"t": {"at": "text", "type": "str"}},
        "key": "w:{t}", "title": "Wordy said {t}", "response": "record",
    })
}

#[tokio::test]
async fn an_agent_makes_a_watch_with_a_yes_and_manages_only_its_own() {
    let w = world().await;
    std::fs::write(
        w.cfg.join("watches.json"),
        json!({"theirs": decl()}).to_string(),
    )
    .unwrap();
    let hooks = std::fs::read_to_string(w.cfg.join("hooks.json")).unwrap();
    let sub = change(
        &w,
        "ledger",
        "s1",
        "watch_manage",
        json!({"action": "create", "name": "said", "watch": decl()}),
    )
    .await;
    assert_eq!(sub.autonomy, Autonomy::Confirm);
    let outcome = w
        .gate
        .confirm_pending("s1", "test", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.status,
        OutcomeStatus::Success,
        "{:?}",
        outcome.error
    );
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(w.cfg.join("watches.json")).unwrap())
            .unwrap();
    assert_eq!(file["said"]["made_by"], "agent:ledger");
    assert_eq!(file["theirs"]["app"], "wordy", "the operator's entry kept");
    // Pausing one's own is quiet.
    let sub = change(
        &w,
        "ledger",
        "s1",
        "watch_manage",
        json!({"action": "pause", "name": "said"}),
    )
    .await;
    assert_eq!(sub.autonomy, Autonomy::Quiet);
    assert_eq!(done(&sub).unwrap()["state"], "paused");
    let list = read(&w, "brand", "ghost_watches_list", json!({})).await;
    let rows = list["watches"].as_array().unwrap();
    assert!(
        rows.iter().any(|r| r["name"] == "said"
            && r["state"] == "paused"
            && r["made_by"] == "agent:ledger")
    );
    assert!(
        rows.iter()
            .any(|r| r["name"] == "theirs" && r["made_by"] == "operator")
    );
    // Brand may not touch Ledger's; nobody touches the operator's; Ghost may Ledger's.
    let sub = change(
        &w,
        "brand",
        "s1",
        "watch_manage",
        json!({"action": "remove", "name": "said"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("ledger's watch"));
    let sub = change(
        &w,
        "ghost",
        "s1",
        "watch_manage",
        json!({"action": "remove", "name": "theirs"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("operator's own"));
    let sub = change(
        &w,
        "ghost",
        "s1",
        "watch_manage",
        json!({"action": "remove", "name": "said"}),
    )
    .await;
    assert_eq!(done(&sub).unwrap()["status"], "removed");
    // hooks.json is never touched.
    assert_eq!(
        std::fs::read_to_string(w.cfg.join("hooks.json")).unwrap(),
        hooks
    );
}

#[tokio::test]
async fn a_watch_that_would_fail_the_check_is_refused_whole() {
    let w = world_with(
        "[permissions]\nallow = [\"ghost.watch_manage\"]\n",
        Runner::default(),
    )
    .await;
    let before = "{\"theirs\": 1}\n"; // a problem of the operator's own, left as it is
    std::fs::write(w.cfg.join("watches.json"), before).unwrap();
    let mut refusals = Vec::new();
    for (k, v) in [
        ("key", json!("w:{nope}")),
        ("every", json!(1)),
        ("read", json!("write_things")),
        ("app", json!("nowhere")),
        ("jev", json!({})),
    ] {
        let mut bad = decl();
        bad[k] = v;
        let sub = change(
            &w,
            "ledger",
            "s1",
            "watch_manage",
            json!({"action": "create", "name": "bad", "watch": bad}),
        )
        .await;
        refusals.push(done(&sub).unwrap_err());
    }
    assert!(refusals[0].contains("not a field"), "{refusals:?}");
    assert!(refusals[1].contains("every"), "{refusals:?}");
    assert!(refusals[2].contains("read-only"), "{refusals:?}");
    assert!(refusals[3].contains("not a connected app"), "{refusals:?}");
    assert!(
        refusals[4].contains("not something an agent's watch sets"),
        "{refusals:?}"
    );
    assert_eq!(
        std::fs::read_to_string(w.cfg.join("watches.json")).unwrap(),
        before
    );
}

// --- 4. routes and quiet; 5. permissions and settings --------------------------------------

#[tokio::test]
async fn quiet_is_set_and_ended_and_routes_read_carries_no_password() {
    let w = world().await;
    let sub = change(&w, "ledger", "s1", "quiet_until", json!({"until": "45m"})).await;
    assert_eq!(sub.autonomy, Autonomy::Quiet);
    done(&sub).unwrap();
    assert!(w.routes.dnd_until().is_some());
    let routes = read(&w, "ledger", "ghost_routes_read", json!({})).await;
    assert!(routes["do_not_disturb_until"].is_string());
    assert!(!Value::Object(routes).to_string().contains(SECRET));
    done(&change(&w, "ledger", "s1", "quiet_off", json!({})).await).unwrap();
    assert!(w.routes.dnd_until().is_none());
    let sub = change(
        &w,
        "ledger",
        "s1",
        "quiet_until",
        json!({"until": "next year"}),
    )
    .await;
    assert!(done(&sub).is_err());
}

#[tokio::test]
async fn settings_and_permissions_are_read_and_never_a_password_or_an_address() {
    let w = world_with(
        "[permissions]\nblock = [\"github.delete_*\"]\nask = [\"ghost.notify\"]\n",
        Runner::default(),
    )
    .await;
    let perms = read(&w, "ledger", "ghost_permissions_read", json!({})).await;
    assert_eq!(perms["mode"], "open");
    assert_eq!(perms["rules"]["block"], json!(["github.delete_*"]));
    assert_eq!(perms["rules"]["ask"], json!(["ghost.notify"]));
    let settings = read(&w, "ledger", "ghost_settings_read", json!({})).await;
    let said = Value::Object(settings.clone()).to_string();
    assert!(!said.contains(SECRET), "{said}");
    assert!(!said.contains("10.0.0.7"), "{said}");
    assert!(settings["settings"].get("OPENAI_API_KEY").is_none());
    assert_eq!(settings["settings"]["GHOST_TTS_VOICE"], "af_heart");
    assert_eq!(settings["model"]["provider"], "anthropic");
}

#[tokio::test]
async fn no_tool_changes_a_permission() {
    let w = world().await;
    let tools = w.server.tools();
    for t in &tools {
        let n = t.name.to_lowercase();
        let about_permissions =
            n.contains("permission") || n.contains("policy") || n.contains("rule");
        assert!(!about_permissions || n == "permissions_read", "{n}");
    }
    // The one that reads them is a read, so it is never a change through the gate.
    let t = tools
        .into_iter()
        .find(|t| t.name == "permissions_read")
        .unwrap();
    assert_eq!(t.annotations.and_then(|a| a.read_only_hint), Some(true));
    assert!(w.gate.operation(NAME, "permissions_read").is_err());
}

#[tokio::test]
async fn no_tool_reads_or_writes_the_other_tools_on_this_computer() {
    // ADR 0068: what Claude Code, Cursor and Codex keep is read, imported and written by
    // the operator's own click or `ghostd setup`, never by an agent: no tool for it, and
    // `app_connect` stays the only way an agent connects an app (asking first, ADR 0052).
    let w = world().await;
    for t in w.server.tools() {
        let n = t.name.to_lowercase();
        for word in [
            "other_tool",
            "setup",
            "import",
            "link",
            "neighbour",
            "cursor",
            "claude",
            "codex",
        ] {
            assert!(!n.contains(word), "{n}");
        }
        let about = t.description.as_deref().unwrap_or_default().to_lowercase();
        assert!(
            !about.contains("mcp.json") && !about.contains("config.toml"),
            "{n}: {about}"
        );
    }
}

#[tokio::test]
async fn no_tool_turns_listening_on_or_off() {
    // ADR 0057: the microphone is the operator's switch. An agent never turns it back on,
    // and never off either: a model told by an app's text to mute Ghost would leave the
    // operator talking to nothing. Its state is a field of `status`, a read.
    let w = world().await;
    for t in w.server.tools() {
        let n = t.name.to_lowercase();
        assert!(
            !(n.contains("listen") || n.contains("mic") || n.contains("mute")),
            "{n}"
        );
    }
}

// --- 6. outputs --------------------------------------------------------------------------

#[tokio::test]
async fn an_output_is_read_a_part_at_a_time_and_deleted_only_inside_out() {
    let w = world().await;
    let out = w.cfg.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let long = "a".repeat(super::outputs_tools::READ_CHARS + 10);
    std::fs::write(out.join("report.md"), &long).unwrap();
    let first = read(
        &w,
        "ledger",
        "ghost_output_read",
        json!({"name": "report.md"}),
    )
    .await;
    assert_eq!(first["next_offset"], super::outputs_tools::READ_CHARS);
    let rest = read(
        &w,
        "ledger",
        "ghost_output_read",
        json!({"name": "report.md", "offset": first["next_offset"]}),
    )
    .await;
    assert_eq!(rest["text"].as_str().unwrap().len(), 10);
    for bad in ["../apps.json", "apps.json", ".hidden.md"] {
        let r = read(&w, "ledger", "ghost_output_read", json!({"name": bad})).await;
        assert!(r["error"].is_string(), "{bad}");
    }
    let sub = change(
        &w,
        "ledger",
        "s1",
        "output_delete",
        json!({"name": "report.md"}),
    )
    .await;
    assert_eq!(done(&sub).unwrap()["status"], "deleted");
    assert!(!out.join("report.md").exists());
    let sub = change(
        &w,
        "ledger",
        "s1",
        "output_delete",
        json!({"name": "../apps.json"}),
    )
    .await;
    assert!(done(&sub).is_err());
    assert!(w.cfg.join("apps.json").exists());
    // The wider outputs: a page, its styles, a calendar file; never a binary.
    for name in [
        "page.htm",
        "site.css",
        "app.js",
        "data.tsv",
        "meeting.ics",
        "notes.yaml",
    ] {
        assert!(crate::outputs::check_name(name).is_ok(), "{name}");
    }
    assert!(crate::outputs::check_name("tool.exe").is_err());
    assert_eq!(crate::outputs::MAX_BYTES, 1024 * 1024);
}

// --- 7. agents working together ----------------------------------------------------------

#[tokio::test]
async fn one_agent_asks_another_once_and_not_a_third() {
    let w = world().await;
    let sub = change(
        &w,
        "ghost",
        "s1",
        "ask_agent",
        json!({"agent": "Ledger", "text": "Is the October invoice paid?"}),
    )
    .await;
    let out = done(&sub).unwrap();
    assert_eq!(out["answer"], "Ledger did it.");
    assert_eq!(
        *w.runner.asked.lock().unwrap(),
        vec![(
            "ledger".to_string(),
            "Is the October invoice paid?".to_string(),
            "agent:ghost".to_string()
        )]
    );
    // Asked this way, it cannot ask a third.
    let asked = format!("run-agentask-{}", "c".repeat(32));
    let sub = change(
        &w,
        "ledger",
        &asked,
        "ask_agent",
        json!({"agent": "brand", "text": "Help?"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("cannot ask a third"));
    // Nor itself, nor nobody.
    let sub = change(
        &w,
        "ledger",
        "s1",
        "ask_agent",
        json!({"agent": "ledger", "text": "x"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("that is you"));
    let sub = change(
        &w,
        "ledger",
        "s1",
        "ask_agent",
        json!({"agent": "intern", "text": "x"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("no agent called intern"));
    assert_eq!(w.runner.asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_session_has_three_agents_working_at_the_most() {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    let w = Arc::new(
        world_with(
            "",
            Runner {
                hold: Some(hold.clone()),
                ..Runner::default()
            },
        )
        .await,
    );
    let ask = |session: &str| {
        let w = w.clone();
        let session = session.to_string();
        tokio::spawn(async move {
            w.server
                .do_ask_agent(AskAgent {
                    agent: "brand".into(),
                    text: "Draft it.".into(),
                    caller: Some("ghost".into()),
                    session: Some(session),
                })
                .await
        })
    };
    let running: Vec<_> = (0..ASKING_MOST).map(|_| ask("s1")).collect();
    while w.runner.asked.lock().unwrap().len() < ASKING_MOST {
        tokio::task::yield_now().await;
    }
    let refused = ask("s1").await.unwrap().unwrap_err();
    assert!(refused.contains("already working"), "{refused}");
    // Another session is not held up by this one's.
    let other = ask("s2");
    while w.runner.asked.lock().unwrap().len() < ASKING_MOST + 1 {
        tokio::task::yield_now().await;
    }
    hold.add_permits(10);
    for r in running {
        assert!(r.await.unwrap().is_ok());
    }
    assert!(other.await.unwrap().is_ok());
    // Their slots were given back.
    assert!(ask("s1").await.unwrap().is_ok());
}

// --- 8. skills ---------------------------------------------------------------------------

#[tokio::test]
async fn an_agent_writes_a_skill_with_a_yes_and_never_a_script() {
    let w = world_with(
        "[permissions]\nallow = [\"ghost.skill_write\"]\n",
        Runner::default(),
    )
    .await;
    assert_eq!(
        world()
            .await
            .gate
            .level(NAME, "skill_write", None, None)
            .unwrap(),
        Autonomy::Confirm
    );
    let body = format!(
        "# Invoices\n\n1. Check the ledger.\n{}",
        "More. ".repeat(2_000)
    );
    let sub = change(
        &w,
        "ledger",
        "s1",
        "skill_write",
        json!({"name": "invoice-reminders", "description": "Chasing unpaid invoices.", "body": body}),
    )
    .await;
    assert_eq!(done(&sub).unwrap()["status"], "written");
    let folder = w.cfg.join("skills").join("invoice-reminders");
    let files: BTreeSet<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, BTreeSet::from(["SKILL.md".to_string()]));
    let skills = Skills::new(&w.cfg.join("skills"), None);
    let skill = skills.get("invoice-reminders").unwrap();
    assert_eq!(skill.description, "Chasing unpaid invoices.");
    // Read back whole: past the old 6,000 characters.
    assert!(skill.body().unwrap().chars().count() > 6_000);
    // Again without replace: refused; a bad name: refused.
    let sub = change(
        &w,
        "ledger",
        "s1",
        "skill_write",
        json!({"name": "invoice-reminders", "description": "x", "body": "y"}),
    )
    .await;
    assert!(done(&sub).unwrap_err().contains("already a skill"));
    let sub = change(
        &w,
        "ledger",
        "s1",
        "skill_write",
        json!({"name": "../evil", "description": "x", "body": "y"}),
    )
    .await;
    assert!(done(&sub).is_err());
}

// --- 10. the tool budget; 11. outside clients ---------------------------------------------

#[tokio::test]
async fn ghosts_own_past_its_core_is_found_by_search_then_offered() {
    let w = world().await;
    let src = w
        .apps
        .sources()
        .into_iter()
        .find(|s| s.name == NAME)
        .unwrap();
    let all: Vec<String> = src
        .read_tools
        .iter()
        .map(|t| t.name.clone())
        .chain(src.operations.iter().filter_map(|o| o.tool_name.clone()))
        .collect();
    let advertised: BTreeSet<&String> = all.iter().filter(|n| !src.deferred.contains(n)).collect();
    let core: Vec<String> = CORE.iter().map(|t| format!("ghost_{t}")).collect();
    assert!(
        advertised.iter().all(|n| core.contains(n)),
        "{advertised:?}"
    );
    assert!(src.deferred.contains(&"ghost_watch_manage".to_string()));
    // Found: offered by name from the next call.
    let found = read(
        &w,
        "ledger",
        "ghost_tools_search",
        json!({"query": "watch poll"}),
    )
    .await;
    assert!(
        found["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "ghost_watch_manage"),
        "{found:?}"
    );
    let src = w
        .apps
        .sources()
        .into_iter()
        .find(|s| s.name == NAME)
        .unwrap();
    assert!(!src.deferred.contains(&"ghost_watch_manage".to_string()));
}

#[tokio::test]
async fn an_outside_client_is_offered_the_reads_it_may_have_and_none_of_the_rest() {
    let w = world().await;
    let outside = w
        .server
        .for_outside(outside::Outside::new(Arc::new(WriteGate::new(
            w.store.clone(),
        ))));
    let names: BTreeSet<String> = outside.tools().iter().map(|t| t.name.to_string()).collect();
    for offered in ["recall", "transcript_read", "watches_list", "routes_read"] {
        assert!(names.contains(offered), "{offered}");
    }
    for never in NOT_FOR_OUTSIDE {
        assert!(!names.contains(never), "{never}");
    }
}
