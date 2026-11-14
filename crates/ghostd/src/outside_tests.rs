//! The listener for outside tools (ADR 0048), with a real client over HTTP on a port the
//! system picks: who gets in, what a client may do, what the audit says, and what is not
//! reachable at all. A temp store and no keyring; nothing here is the operator's.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use ghost_core::apps::Apps;
use ghost_core::conversations::models::Profile;
use ghost_core::gate::WriteGate;
use ghost_core::ghost_app::outside::AgentRunner;
use ghost_core::ghost_app::{self, Attached, NAME, Parts};
use ghost_core::models::ProposalStatus;
use ghost_core::policy::AutonomyPolicy;
use ghost_core::questions::{Questions, via};
use ghost_core::store::{ProposalQuery, Store};
use ghost_core::waiting::{Items, Response as Tell};
use rmcp::model::{CallToolRequestParams, ProtocolVersion, ReadResourceRequestParams};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Map;

use super::*;
use crate::mcp::{Target, bridge, with_idempotency_key};

type Client = RunningService<RoleClient, ()>;

struct World {
    _tmp: tempfile::TempDir,
    store: Arc<Store>,
    gate: Arc<WriteGate>,
    waiting: Arc<WaitingList>,
    clients: Arc<Clients>,
    host: OutsideHost,
    url: String,
    questions: Option<Arc<Questions>>,
    _attached: Attached,
    _apps: Arc<Apps>,
}

fn status() -> Map<String, Value> {
    json!({
        "version": "2026.11.0",
        "phase": "passive",
        "session": {"open": true, "origin": "desk"},
        "agent": "ledger",
    })
    .as_object()
    .cloned()
    .unwrap()
}

/// What the desk would do, scripted.
struct FakeDesk(Said, Mutex<Vec<String>>);

#[async_trait]
impl DeskVoice for FakeDesk {
    async fn say(&self, line: &str, _item: &Item) -> Said {
        self.1.lock().unwrap().push(line.to_string());
        self.0.clone()
    }
}

struct Echo;

#[async_trait]
impl AgentRunner for Echo {
    async fn ask(&self, agent: &Profile, text: &str, from: &str) -> Result<String, String> {
        Ok(format!("{} heard {text} from {from}", agent.name))
    }
}

async fn world_with(policy: &str, listen: &str, shape: impl FnOnce(Outside) -> Outside) -> World {
    world_full(policy, listen, shape, false).await
}

async fn world_full(
    policy: &str,
    listen: &str,
    shape: impl FnOnce(Outside) -> Outside,
    asking: bool,
) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let gate = Arc::new(
        WriteGate::new(store.clone())
            .with_autonomy(AutonomyPolicy::parse(policy, "<test>").unwrap()),
    );
    // The daemon's bus, as far as the feed needs it: a waiting-list change is an event.
    let (bus, events) = broadcast::channel::<String>(64);
    let waiting = Arc::new(
        WaitingList::new(Items::new(store.clone()))
            .with_changes(gate.clone())
            .with_on_change(Arc::new({
                let bus = bus.clone();
                move || {
                    let _ = bus.send(r#"{"type":"waiting","count":0}"#.to_string());
                }
            })),
    );
    let apps = Arc::new(Apps::new(
        Vec::new(),
        Arc::new(HashMap::<String, String>::new()),
        None,
    ));
    let mut server = GhostServer::new(Parts {
        store: store.clone(),
        waiting: waiting.clone(),
        apps: Arc::downgrade(&apps),
        status: Arc::new(status),
    });
    let questions = asking.then(|| {
        // As the daemon does (ADR 0051): a question asked or settled is a bus event.
        let q = Arc::new(Questions::new(store.clone(), waiting.clone()));
        q.on_change(Arc::new(move |q| {
            let event = ghost_api::models::Event::Question {
                question: ghost_api::questions::question_view(q),
            };
            let _ = bus.send(serde_json::to_string(&event).unwrap());
        }));
        q
    });
    if let Some(q) = &questions {
        server = server.with_questions(q.clone());
    }
    // Everything Ghost's own agents reach (ADR 0052), so the line an outside client is
    // held to is tested against all of it.
    let cfg = tmp.path().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    server = server.with_more(ghost_app::More {
        agents: Some(Arc::new(
            ghost_core::conversations::Conversations::new(store.clone(), &cfg, Default::default())
                .unwrap(),
        )),
        config_dir: Some(cfg.clone()),
        catalog: Some(Arc::new(ghost_core::catalog::Catalog::new(
            tmp.path().join("catalog.json"),
        ))),
        routes: Some(ghost_app::ready(Arc::new(
            ghost_core::routes::RouteEngine::new(Items::new(store.clone())).unwrap(),
        ))),
        policy: Some(Arc::new(AutonomyPolicy::default)),
        settings: Some(Arc::new(Default::default)),
        skills: Some(Arc::new(ghost_core::skills::Skills::new(
            &cfg.join("skills"),
            None,
        ))),
        runner: Some(Arc::new(Echo)),
        ..ghost_app::More::default()
    });
    let attached = ghost_app::attach(&apps, server.clone()).await.unwrap();
    gate.register(NAME, attached.app.operations());
    for (slug, name, job) in [("ghost", "Ghost", ""), ("ledger", "Ledger", "The books")] {
        if store.get_conversation(slug, true).unwrap().is_some() {
            continue; // Ghost is made when its folder is read
        }
        let mut p = Profile::new(slug, name);
        p.job = job.to_string();
        store.upsert_conversation(&p).unwrap();
    }
    let clients = Arc::new(Clients::new(store.clone(), tmp.path().join("clients")));
    let host = OutsideHost::new(
        listen,
        clients.clone(),
        Some(&server),
        shape(Outside::new(gate.clone())),
    );
    host.start(&waiting, events).await;
    let url = format!("http://127.0.0.1:{}{PATH}", host.port());
    World {
        _tmp: tmp,
        store,
        gate,
        waiting,
        clients,
        host,
        url,
        questions,
        _attached: attached,
        _apps: apps,
    }
}

async fn world(policy: &str) -> World {
    world_with(policy, "127.0.0.1:0", |o| o).await
}

impl World {
    fn token(&self, name: &str) -> String {
        self.clients.create(name, false).unwrap().token.to_string()
    }

    async fn client(&self, name: &str, mode: ClientLifecycleMode) -> Client {
        connect(&self.url, &self.token(name), mode).await.unwrap()
    }

    fn items(&self) -> Vec<(String, String, String)> {
        self.waiting
            .items
            .open(Some(Tell::Tell))
            .unwrap()
            .into_iter()
            .map(|i| (i.source, i.key, i.title))
            .collect()
    }
}

/// The two lifecycles a client may speak: the session-based one and the stateless one.
fn lifecycles() -> Vec<ClientLifecycleMode> {
    vec![
        ClientLifecycleMode::Initialize,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    ]
}

async fn connect(url: &str, token: &str, mode: ClientLifecycleMode) -> Result<Client, String> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .auth_header(token.to_string()),
    );
    ().serve_with_lifecycle(transport, mode)
        .await
        .map_err(|e| format!("{e:?}"))
}

/// A tool call: what it answered, or what it refused with.
async fn call(client: &Client, tool: &str, args: Value) -> Result<Value, String> {
    let params = CallToolRequestParams::new(tool.to_string())
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await.map_err(|e| e.to_string())?;
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    if result.is_error == Some(true) {
        return Err(text);
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

const HELLO: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;

/// One raw request, as a tool that is not a real client might send it.
async fn post(url: &str, token: Option<&str>, more: &[(&str, &str)]) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(HELLO);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    for (k, v) in more {
        req = req.header(*k, *v);
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn no_token_and_a_wrong_token_are_refused_and_repeated_ones_lock_the_name_out() {
    let w = world("").await;
    let good = w.token("claude-code");
    let other = w.token("cursor");

    let r = post(&w.url, None, &[]).await;
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers()["www-authenticate"], "Bearer");
    let r = post(&w.url, Some("ghost.claude-code.not-the-token"), &[]).await;
    assert_eq!(r.status(), 401);
    let body = r.text().await.unwrap();
    assert_eq!(body, r#"{"error":"unauthorized"}"#, "nothing about why");
    assert_eq!(post(&w.url, Some(&good), &[]).await.status(), 200);

    // Five wrong tokens for the name: it is refused for five minutes, right token or not.
    for _ in 0..5 {
        let r = post(&w.url, Some("ghost.claude-code.wrong"), &[]).await;
        assert_eq!(r.status(), 401);
    }
    let r = post(&w.url, Some(&good), &[]).await;
    assert_eq!(r.status(), 429);
    let wait: u64 = r.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=300).contains(&wait), "{wait}");
    // Another client is not locked out with it.
    assert_eq!(post(&w.url, Some(&other), &[]).await.status(), 200);
}

#[tokio::test]
async fn a_browser_and_a_foreign_host_are_refused_whatever_token_they_carry() {
    let w = world("").await;
    let token = w.token("claude-code");
    // A page that reached this port (DNS rebinding) sends an Origin; a tool does not.
    for origin in ["https://evil.example", "http://localhost:3000", "null"] {
        let r = post(&w.url, Some(&token), &[("origin", origin)]).await;
        assert_eq!(r.status(), 403, "{origin}");
    }
    // The name the request was sent to must be this computer's (rmcp's allow-list).
    let r = post(&w.url, Some(&token), &[("host", "evil.example")]).await;
    assert_eq!(r.status(), 403);
    assert_eq!(post(&w.url, Some(&token), &[]).await.status(), 200);
}

#[tokio::test]
async fn nothing_but_the_one_path_is_served() {
    let w = world("").await;
    let token = w.token("claude-code");
    let base = w.url.trim_end_matches(PATH);
    // Not the control API, the phone listener or the webhook listener.
    for path in [
        "/",
        "/status",
        "/passwords",
        "/policy",
        "/clients",
        "/apps",
        "/events",
        "/pair",
        "/hello",
        "/hooks/ci",
    ] {
        for token in [None, Some(token.as_str())] {
            let mut req = reqwest::Client::new().get(format!("{base}{path}"));
            if let Some(t) = token {
                req = req.header("authorization", format!("Bearer {t}"));
            }
            assert_eq!(req.send().await.unwrap().status(), 404, "{path}");
        }
    }
}

/// The tools an outside client has, and no other.
const TOOLS: [&str; 17] = [
    "agent_ask",
    "agents_list",
    "apps_list",
    "audit_recent",
    "docs_read",
    "docs_search",
    "events_wait",
    "notify",
    "recall",
    "resolve",
    "routes_read",
    "say",
    "status",
    "transcript_read",
    "waiting_ack",
    "waiting_snooze",
    "watches_list",
];

#[tokio::test]
async fn a_client_reads_ghost_in_both_lifecycles() {
    let w = world("").await;
    for (n, mode) in lifecycles().into_iter().enumerate() {
        let c = w.client(&format!("tool-{n}"), mode.clone()).await;
        let names: Vec<String> = c
            .list_all_tools()
            .await
            .unwrap()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names, TOOLS, "{mode:?}");

        let s = call(&c, "status", json!({})).await.unwrap();
        assert_eq!(s["version"], "2026.11.0");
        let agents = call(&c, "agents_list", json!({})).await.unwrap();
        assert_eq!(
            agents["agents"],
            json!([
                {"agent": "ghost", "name": "Ghost", "job": "", "in_session": false},
                {"agent": "ledger", "name": "Ledger", "job": "The books", "in_session": true},
            ])
        );
        let found = call(&c, "docs_search", json!({"query": "waiting list"}))
            .await
            .unwrap();
        assert!(!found["results"].as_array().unwrap().is_empty());

        // The documentation and the status are resources too.
        let uris: BTreeSet<String> = c
            .list_all_resources()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.uri)
            .collect();
        assert!(uris.contains("ghost://status") && uris.contains("ghost://docs/apps"));
        assert!(
            uris.iter()
                .all(|u| u == "ghost://status" || u.starts_with("ghost://docs/"))
        );
        let doc = c
            .read_resource(ReadResourceRequestParams::new("ghost://docs/glossary"))
            .await
            .unwrap();
        assert!(
            serde_json::to_string(&doc)
                .unwrap()
                .contains("Waiting list")
        );
        let st = c
            .read_resource(ReadResourceRequestParams::new("ghost://status"))
            .await
            .unwrap();
        assert!(serde_json::to_string(&st).unwrap().contains("2026.11.0"));
        assert!(
            c.read_resource(ReadResourceRequestParams::new("ghost://policy"))
                .await
                .is_err()
        );
        let _ = c.cancel().await;
    }
    // Reads leave nothing in the audit.
    let (rows, _) = w.store.list_proposals(&ProposalQuery::page(50, 0)).unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn a_removed_client_is_refused_at_once() {
    let w = world("").await;
    for (n, mode) in lifecycles().into_iter().enumerate() {
        let name = format!("tool-{n}");
        let c = w.client(&name, mode).await;
        assert!(call(&c, "status", json!({})).await.is_ok());
        w.clients.remove(&name).unwrap();
        assert!(call(&c, "status", json!({})).await.is_err());
        let _ = c.cancel().await;
    }
}

#[tokio::test]
async fn a_notify_is_the_clients_own_item_and_only_it_resolves_it() {
    let w = world("").await;
    for (n, mode) in lifecycles().into_iter().enumerate() {
        let (me, them) = (format!("claude-code-{n}"), format!("cursor-{n}"));
        let mine = w.client(&me, mode.clone()).await;
        let theirs = w.client(&them, mode).await;

        // A client that names someone else as the caller is not believed.
        let told = call(
            &mine,
            "notify",
            json!({"title": "the build finished", "_agent": "ledger"}),
        )
        .await
        .unwrap();
        let key = told["key"].as_str().unwrap().to_string();
        assert!(key.starts_with(&format!("client:{me}:")), "{key}");
        let shown = ghost_core::ghost_app::clients::shown(&me);
        assert_eq!(
            w.items(),
            [(
                format!("client:{me}"),
                key.clone(),
                format!("From {shown}: the build finished")
            )]
        );

        // Another client cannot clear it; nor can it by guessing an agent's key.
        let e = call(&theirs, "resolve", json!({"key": key}))
            .await
            .unwrap_err();
        assert!(e.contains("not an open item of yours"), "{e}");
        assert_eq!(w.items().len(), 1);
        let done = call(&mine, "resolve", json!({"key": key})).await.unwrap();
        assert_eq!(done["status"], "resolved");
        assert!(w.items().is_empty());

        // Every change has a row in the audit that names the client: who, what, how it ended.
        let (rows, _) = w.store.list_proposals(&ProposalQuery::page(50, 0)).unwrap();
        let of = |session: &str| -> Vec<(String, &'static str)> {
            let mut out: Vec<(String, &'static str)> = rows
                .iter()
                .filter(|p| p.session_id == session && p.ability == NAME)
                .map(|p| (p.operation.clone(), p.status.as_str()))
                .collect();
            out.sort();
            out
        };
        assert_eq!(
            of(&format!("client:{me}")),
            [
                ("notify".to_string(), "executed"),
                ("resolve".to_string(), "executed")
            ]
        );
        // The refused resolve is a row too, failed.
        assert_eq!(of(&format!("client:{them}")).len(), 1);
        assert!(
            rows.iter()
                .all(|p| p.rationale.contains("an outside client"))
        );
        let _ = mine.cancel().await;
        let _ = theirs.cancel().await;
    }
    // A token is a password: no row, event or item has one.
    let dump = format!(
        "{:?}",
        w.store.list_proposals(&ProposalQuery::page(50, 0)).unwrap()
    );
    assert!(!dump.contains("ghost.claude-code-"), "{dump}");
}

#[tokio::test]
async fn a_blocked_tool_is_refused_and_an_ask_one_waits_for_the_operator() {
    let policy = "[permissions]\nblock = [\"ghost.waiting_ack\", \"ghost.audit_recent\"]\nask = [\"ghost.notify\"]\n";
    let w = world(policy).await;
    for (n, mode) in lifecycles().into_iter().enumerate() {
        let name = format!("claude-code-{n}");
        let c = w.client(&name, mode).await;
        let names: Vec<String> = c
            .list_all_tools()
            .await
            .unwrap()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(
            !names
                .iter()
                .any(|t| t == "waiting_ack" || t == "audit_recent")
        );

        // Blocked: a change and a read, both refused, and nothing happens.
        let e = call(&c, "waiting_ack", json!({"keys": ["x"]}))
            .await
            .unwrap_err();
        assert!(e.contains("blocked by the operator's permissions"), "{e}");
        let e = call(&c, "audit_recent", json!({})).await.unwrap_err();
        assert!(e.contains("blocked by the operator's permissions"), "{e}");

        // Ask: it has not run. It is on the waiting list for the operator's yes.
        let told = call(&c, "notify", json!({"title": "deploy is ready"}))
            .await
            .unwrap();
        assert_eq!(told["status"], "waiting for the operator");
        let id = told["id"].as_str().unwrap().to_string();
        let items = w.items();
        assert!(!items.iter().any(|(s, _, _)| *s == format!("client:{name}")));
        assert!(
            items
                .iter()
                .any(|(s, k, _)| s == "gate" && *k == format!("change:{id}")),
            "{items:?}"
        );
        let p = w.store.get_proposal(&id).unwrap().unwrap();
        assert_eq!(p.status, ProposalStatus::Pending);
        assert_eq!(p.session_id, format!("client:{name}"));

        // The operator says yes (a click in the desktop): now it runs, as the client's.
        let outcome = w.gate.confirm_id(&id, "ui", None).await.unwrap().unwrap();
        assert_eq!(outcome.status, ghost_core::models::OutcomeStatus::Success);
        w.waiting.refresh(None).unwrap();
        let shown = ghost_core::ghost_app::clients::shown(&name);
        assert!(
            w.items()
                .iter()
                .any(|(s, _, t)| *s == format!("client:{name}")
                    && *t == format!("From {shown}: deploy is ready"))
        );
        let _ = c.cancel().await;
    }
}

#[tokio::test]
async fn each_client_has_a_budget_of_calls_and_a_smaller_one_of_things_told() {
    let w = world("").await;
    let teller = w.client("teller", ClientLifecycleMode::Initialize).await;
    for n in 0..10 {
        call(&teller, "notify", json!({"title": format!("thing {n}")}))
            .await
            .unwrap();
    }
    let e = call(&teller, "notify", json!({"title": "one more"}))
        .await
        .unwrap_err();
    assert!(
        e.contains("10 a minute") && e.contains("Try again in"),
        "{e}"
    );
    assert_eq!(w.items().len(), 10);
    // It may still read.
    assert!(call(&teller, "status", json!({})).await.is_ok());

    let reader = w.client("reader", ClientLifecycleMode::Initialize).await;
    for _ in 0..60 {
        call(&reader, "apps_list", json!({})).await.unwrap();
    }
    let e = call(&reader, "apps_list", json!({})).await.unwrap_err();
    assert!(
        e.contains("60 a minute") && e.contains("Try again in"),
        "{e}"
    );
    // A resource is a call like any other.
    assert!(
        reader
            .read_resource(ReadResourceRequestParams::new("ghost://status"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_client_asks_and_learns_the_answer_and_never_reads_anothers_question() {
    // ADR 0051: ask returns the key at once; events_wait says the question was settled;
    // question reads the answer. Another client learns only that one was asked, and no
    // client can answer one.
    let w = world_full("", "127.0.0.1:0", |o| o, true).await;
    let qs = w.questions.clone().unwrap();
    let claude = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let cursor_tool = w.client("cursor", ClientLifecycleMode::Initialize).await;
    let names: BTreeSet<String> = claude
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    for t in ["ask", "question", "questions_list", "question_cancel"] {
        assert!(names.contains(t), "{t}");
    }
    assert!(!names.contains("answer"), "an outside tool never answers");
    let refused = call(
        &claude,
        "answer",
        json!({"key": "question:x", "text": "yes"}),
    )
    .await;
    assert!(refused.is_err());

    let start = call(&claude, "events_wait", json!({"timeout_s": 0.1}))
        .await
        .unwrap()["cursor"]
        .as_u64()
        .unwrap();
    let asked = call(
        &claude,
        "ask",
        json!({"question": "Deploy the release branch now?", "choices": ["Yes", "No"]}),
    )
    .await
    .unwrap();
    let key = asked["key"].as_str().unwrap().to_string();
    assert_eq!(asked["state"], "open");
    assert_eq!(asked["asked_by"], "Claude Code");
    // On the operator's list, said as the tool's.
    let item = w.waiting.items.get(&key).unwrap().unwrap();
    assert_eq!(item.source, "client:claude-code");
    assert!(item.title.starts_with("Claude Code asks: Deploy"));

    // The operator answers from the phone.
    qs.answer(&key, "yes, go", via::PHONE, None).unwrap();
    let got = call(
        &claude,
        "events_wait",
        json!({"since": start, "timeout_s": 5}),
    )
    .await
    .unwrap();
    let settled = got["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "question" && e["state"] == "answered")
        .cloned()
        .unwrap_or_else(|| panic!("{got}"));
    assert_eq!(settled["key"], key.as_str());
    assert_eq!(settled["yours"], true);
    assert_eq!(settled["answer"], "yes, go");
    let read = call(&claude, "question", json!({"key": key}))
        .await
        .unwrap();
    assert_eq!(read["state"], "answered");
    assert_eq!(read["answer"], "yes, go");
    assert_eq!(read["choice"], "Yes");
    assert_eq!(read["answered_via"], "phone");

    // Another client: that it happened, never its words or the answer.
    let other = call(
        &cursor_tool,
        "events_wait",
        json!({"since": start, "timeout_s": 5}),
    )
    .await
    .unwrap();
    for e in other["events"].as_array().unwrap() {
        if e["type"] == "question" {
            assert_eq!(e["yours"], false);
            assert!(e.get("answer").is_none() && e.get("title").is_none(), "{e}");
        }
    }
    let theirs = call(&cursor_tool, "question", json!({"key": key})).await;
    assert!(theirs.unwrap_err().contains("that you asked"));
    let list = call(&cursor_tool, "questions_list", json!({}))
        .await
        .unwrap();
    assert_eq!(list["questions"], json!([]));

    // A client that waits holds the request until the answer comes.
    let waiting = tokio::spawn({
        let peer = claude.peer().clone();
        async move {
            let params = CallToolRequestParams::new("ask").with_arguments(
                json!({"question": "Merge it?", "wait_s": 30})
                    .as_object()
                    .cloned()
                    .unwrap(),
            );
            peer.call_tool(params).await.unwrap()
        }
    });
    let mut open = None;
    for _ in 0..200 {
        open = qs.open(None).unwrap().pop();
        if open.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    qs.answer(&open.unwrap().key, "not yet", via::DESKTOP, None)
        .unwrap();
    let got = waiting.await.unwrap();
    let got: Value = serde_json::from_str(&got.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(got["answer"], "not yet");
    let _ = claude.cancel().await;
    let _ = cursor_tool.cancel().await;
    w.host.stop().await;
}

#[tokio::test]
async fn events_wait_returns_on_a_waiting_list_change_and_at_its_timeout() {
    let w = world("").await;
    for (n, mode) in lifecycles().into_iter().enumerate() {
        let watcher = w.client(&format!("watcher-{n}"), mode.clone()).await;
        let sender_name = format!("sender-{n}");
        let sender = w.client(&sender_name, mode).await;

        let began = std::time::Instant::now();
        let quiet = call(&watcher, "events_wait", json!({"timeout_s": 0.3}))
            .await
            .unwrap();
        assert_eq!(quiet["timed_out"], true);
        assert_eq!(quiet["events"], json!([]));
        assert!(began.elapsed() >= Duration::from_millis(300));
        let cursor = quiet["cursor"].as_u64().unwrap();

        // A long wait returns as soon as something changes.
        let waiting = tokio::spawn({
            let watcher = watcher.peer().clone();
            async move {
                let params = CallToolRequestParams::new("events_wait").with_arguments(
                    json!({"since": cursor, "timeout_s": 30})
                        .as_object()
                        .cloned()
                        .unwrap(),
                );
                watcher.call_tool(params).await.unwrap()
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let began = std::time::Instant::now();
        let told = call(&sender, "notify", json!({"title": "tests passed"}))
            .await
            .unwrap();
        let key = told["key"].as_str().unwrap().to_string();
        let got = waiting.await.unwrap();
        assert!(began.elapsed() < Duration::from_secs(10));
        let text = got.content[0].as_text().unwrap().text.clone();
        let got: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(got["timed_out"], false);
        let e = &got["events"][0];
        assert_eq!(
            (e["type"].as_str(), e["change"].as_str(), e["key"].as_str()),
            (Some("waiting"), Some("added"), Some(key.as_str()))
        );
        // Another client's item: that it exists, never what it says.
        assert_eq!(e["yours"], false);
        assert!(e.get("title").is_none(), "{e}");

        // The sender waits for the operator's acknowledgment of what it sent.
        let mine = call(
            &sender,
            "events_wait",
            json!({"since": cursor, "timeout_s": 5}),
        )
        .await
        .unwrap();
        assert_eq!(mine["events"][0]["yours"], true);
        assert!(
            mine["events"][0]["title"]
                .as_str()
                .unwrap()
                .ends_with("tests passed")
        );
        let cursor = mine["cursor"].as_u64().unwrap();
        w.waiting.acknowledge(&key, None, None).unwrap();
        let heard = call(
            &sender,
            "events_wait",
            json!({"since": cursor, "timeout_s": 5}),
        )
        .await
        .unwrap();
        assert_eq!(heard["events"][0]["change"], "acknowledged");
        assert_eq!(heard["events"][0]["key"], key.as_str());
        let _ = watcher.cancel().await;
        let _ = sender.cancel().await;
    }
}

#[tokio::test]
async fn nothing_an_outside_client_must_not_reach_is_offered() {
    let w = world("").await;
    let c = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let tools = c.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, TOOLS);
    // No tool about passwords, the policy file, permissions, apps to connect, clients or
    // their tokens, memory, settings or audio: by its name, and by the name of any argument
    // it takes. (`recall` and `transcript_read` are offered, over the turns of the runs the
    // client itself started, ADR 0052: tested in `a_client_recalls_only_its_own_runs`.)
    let forbidden = [
        "password",
        "passwords",
        "secret",
        "secrets",
        "vault",
        "keyring",
        "policy",
        "permission",
        "permissions",
        "token",
        "tokens",
        "transcripts",
        "turn",
        "turns",
        "settings",
        "skill",
        "quiet",
        "output",
        "audio",
        "microphone",
        "passive",
        "connect",
        "disconnect",
        "signin",
        "client",
        "clients",
        "rule",
        "rules",
        "persona",
        "memory",
        "watch",
        "hook",
        "catalog",
        "confirm",
        // ADR 0067: error reports are the operator's switch.
        "report",
        "reports",
        "telemetry",
    ];
    for t in &tools {
        let arguments: Vec<String> = t
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        let said = format!("{} {}", t.name, arguments.join(" ")).to_lowercase();
        let words: BTreeSet<&str> = said
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        for word in forbidden {
            assert!(!words.contains(word), "{} offers {word}: {said}", t.name);
        }
    }
    // The only changes are the six that tell the operator, mark the list or talk to an agent.
    let changes: Vec<&str> = tools
        .iter()
        .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) != Some(true))
        .map(|t| t.name.as_ref())
        .collect();
    assert_eq!(
        changes,
        [
            "agent_ask",
            "notify",
            "resolve",
            "say",
            "waiting_ack",
            "waiting_snooze"
        ]
    );
    // A tool Ghost has for itself is not one a client can call, the wider ones of
    // ADR 0052 included.
    for tool in [
        "passwords_list",
        "policy_read",
        "clients_add",
        "change",
        "waiting_list",
    ]
    .into_iter()
    .chain(ghost_app::NOT_FOR_OUTSIDE)
    {
        assert!(call(&c, tool, json!({})).await.is_err(), "{tool}");
    }
    // Nothing a read says holds a token or its hash, and the status never a key.
    let token = w.token("another");
    let hash = w.store.get_client("another").unwrap().unwrap().token_hash;
    for tool in ["status", "apps_list", "audit_recent", "agents_list"] {
        let said = call(&c, tool, json!({})).await.unwrap().to_string();
        assert!(!said.contains(&token) && !said.contains(&hash), "{tool}");
    }
    // The resources are the documentation and the status, nothing else.
    for r in c.list_all_resources().await.unwrap() {
        assert!(
            r.uri == "ghost://status" || r.uri.starts_with("ghost://docs/"),
            "{}",
            r.uri
        );
    }
}

#[tokio::test]
async fn say_speaks_when_the_desk_is_free_and_is_an_item_either_way() {
    let desk = Arc::new(FakeDesk(Said::Speaking, Mutex::new(Vec::new())));
    let w = world_with("", "127.0.0.1:0", {
        let desk = desk.clone();
        |o| o.with_desk(desk)
    })
    .await;
    let c = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let said = call(&c, "say", json!({"text": "the build finished"}))
        .await
        .unwrap();
    assert_eq!(said["status"], "saying it at the desk");
    assert_eq!(
        *desk.1.lock().unwrap(),
        ["From Claude Code: the build finished"]
    );
    assert_eq!(w.items().len(), 1);
    assert_eq!(w.items()[0].0, "client:claude-code");

    // A busy desk, a quiet time, or an operator who has not let Ghost speak first.
    let w = world("").await;
    let c = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let said = call(&c, "say", json!({"text": "the build finished"}))
        .await
        .unwrap();
    assert_eq!(said["status"], "on the waiting list");
    assert_eq!(w.items()[0].2, "From Claude Code: the build finished");
    let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
    assert_eq!(rows[0].operation, "say");
    assert_eq!(rows[0].session_id, "client:claude-code");
    assert!(call(&c, "say", json!({"text": "  "})).await.is_err());
}

#[tokio::test]
async fn the_desk_stays_silent_before_the_daemon_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let d = crate::daemon::tests::daemon(tmp.path());
    let store = Arc::new(Store::in_memory().unwrap());
    let waiting = WaitingList::new(Items::new(store));
    waiting
        .observe("client:x", &ghost_core::waiting::ItemDraft::new("k", "t"))
        .unwrap();
    let item = waiting.items.get("k").unwrap().unwrap();
    let said = Desk(Arc::downgrade(&d)).say("hello", &item).await;
    assert!(matches!(said, Said::Busy(_)), "{said:?}");
}

#[tokio::test]
async fn agent_ask_is_gated_and_says_so_until_a_runner_is_plugged_in() {
    let w = world("").await;
    let c = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let e = call(
        &c,
        "agent_ask",
        json!({"agent": "ledger", "text": "the October total?"}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("Not available yet"), "{e}");
    let e = call(&c, "agent_ask", json!({"agent": "nobody", "text": "hi"}))
        .await
        .unwrap_err();
    assert!(
        e.contains("no agent called nobody") && e.contains("ledger"),
        "{e}"
    );
    let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|p| p.operation == "agent_ask" && p.session_id == "client:claude-code")
    );

    // With a runner plugged in, the agent's written answer comes back, under either
    // name of the agent, and the audit has it as the client's.
    let w = world_with("", "127.0.0.1:0", |o| o.with_runner(Arc::new(Echo))).await;
    let c = w.client("cursor", ClientLifecycleMode::Initialize).await;
    let out = call(
        &c,
        "agent_ask",
        json!({"agent": "Ledger", "text": "the October total?"}),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({"agent": "ledger", "answer": "Ledger heard the October total? from client:cursor"})
    );
    let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
    assert_eq!(rows[0].status, ProposalStatus::Executed);
    assert_eq!(rows[0].session_id, "client:cursor");

    // The operator's `ask` holds for it as for any change: nothing runs until their yes.
    let w = world_with(
        "[permissions]\nask = [\"ghost.agent_ask\"]\n",
        "127.0.0.1:0",
        |o| o.with_runner(Arc::new(Echo)),
    )
    .await;
    let c = w.client("cursor", ClientLifecycleMode::Initialize).await;
    let out = call(&c, "agent_ask", json!({"agent": "ledger", "text": "hi"}))
        .await
        .unwrap();
    assert_eq!(out["status"], "waiting for the operator");
}

#[tokio::test]
async fn a_port_that_is_taken_is_a_problem_to_read_not_a_crash() {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let w = world_with("", &address, |o| o).await;
    let problem = w.host.problem().unwrap();
    assert!(
        problem.contains(&address) && problem.contains("AddrInUse"),
        "{problem}"
    );
    assert_eq!(w.host.port(), 0);
    // Turned off is not a problem; an address that does not read is one.
    let off = world_with("", "", |o| o).await;
    assert!(off.host.problem().is_none() && off.host.addresses().is_empty());
    let bad = world_with("", "not-an-address", |o| o).await;
    assert!(bad.host.problem().unwrap().contains("host:port"));
}

#[test]
fn only_this_computer_is_loopback() {
    for host in ["127.0.0.1", "localhost", "::1", "127.0.0.53"] {
        assert!(is_loopback(host), "{host}");
    }
    for host in ["0.0.0.0", "192.168.1.20", "10.8.0.4", "::", "ghost.lan"] {
        assert!(!is_loopback(host), "{host}");
    }
    assert_eq!(
        parse_addresses(" 127.0.0.1:47824 , [::1]:47824 ")
            .unwrap()
            .len(),
        2
    );
    assert!(parse_addresses("").unwrap().is_empty());
    assert!(parse_addresses("localhost").is_err());
}

// --- the bridge ---------------------------------------------------------------------------------

/// A client that only starts commands: its stdin and stdout are the two pipes.
async fn bridged(
    url: &str,
    token: &str,
    retry_for: Duration,
) -> (Result<Client, String>, tokio::task::JoinHandle<()>) {
    let (client_io, bridge_io) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(bridge_io);
    let target = Target {
        url: url.to_string(),
        token: zeroize::Zeroizing::new(token.to_string()),
        retry_for,
    };
    let task = tokio::spawn(async move {
        let _ = bridge(read, write, target).await;
    });
    let client = ().serve(client_io).await.map_err(|e| format!("{e:?}"));
    (client, task)
}

#[tokio::test]
async fn the_bridge_round_trips_a_tool_call_as_its_client() {
    let w = world("").await;
    let token = w.clients.create("codex", true).unwrap().token.to_string();
    let (client, task) = bridged(&w.url, &token, Duration::from_secs(5)).await;
    let c = client.unwrap();
    let names: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(names, TOOLS);
    let told = call(&c, "notify", json!({"title": "review is done"}))
        .await
        .unwrap();
    assert!(told["key"].as_str().unwrap().starts_with("client:codex:"));
    assert_eq!(w.items()[0].2, "From Codex: review is done");
    let s = call(&c, "status", json!({})).await.unwrap();
    assert_eq!(s["version"], "2026.11.0");
    let _ = c.cancel().await;
    task.abort();
}

#[tokio::test]
async fn the_bridge_waits_for_a_daemon_that_starts_later_and_says_so_when_none_does() {
    // Nothing is listening yet: the client's hello waits, and is answered once Ghost is up.
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = free.local_addr().unwrap().to_string();
    drop(free);
    let url = format!("http://{address}{PATH}");
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(tmp.path().join("ghost.db")).unwrap());
    let token = Clients::new(store.clone(), tmp.path().join("clients"))
        .create("codex", true)
        .unwrap()
        .token
        .to_string();
    let starting = tokio::spawn({
        let (url, token) = (url.clone(), token.clone());
        async move { bridged(&url, &token, Duration::from_secs(10)).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let late = late_world(store, &address, tmp.path()).await;
    let (client, task) = starting.await.unwrap();
    let c = client.unwrap();
    assert!(call(&c, "status", json!({})).await.is_ok());

    // Ghost restarts: the next call is answered by the new one, with no new hello from
    // the client.
    late.host.stop().await;
    drop(late);
    let store = Arc::new(Store::open(tmp.path().join("ghost.db")).unwrap());
    let again = late_world(store, &address, tmp.path()).await;
    assert!(
        call(&c, "status", json!({})).await.is_ok(),
        "after a restart"
    );
    let _ = c.cancel().await;
    task.abort();
    again.host.stop().await;

    // No Ghost at all: a clear error after a short wait, not a hang.
    let began = std::time::Instant::now();
    let (client, task) = bridged(&url, &token, Duration::from_millis(600)).await;
    let e = client.expect_err("no daemon to say hello to");
    assert!(began.elapsed() < Duration::from_secs(8));
    assert!(e.contains("Ghost is not answering"), "{e}");
    task.abort();
}

/// A daemon's outside side over a store on disk, on a given address.
struct Late {
    host: OutsideHost,
    _attached: Attached,
    _apps: Arc<Apps>,
}

async fn late_world(store: Arc<Store>, address: &str, tmp: &std::path::Path) -> Late {
    let gate = Arc::new(WriteGate::new(store.clone()));
    let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
    let apps = Arc::new(Apps::new(
        Vec::new(),
        Arc::new(HashMap::<String, String>::new()),
        None,
    ));
    let server = GhostServer::new(Parts {
        store: store.clone(),
        waiting: waiting.clone(),
        apps: Arc::downgrade(&apps),
        status: Arc::new(status),
    });
    let attached = ghost_app::attach(&apps, server.clone()).await.unwrap();
    gate.register(NAME, attached.app.operations());
    let clients = Arc::new(Clients::new(store, tmp.join("clients")));
    let host = OutsideHost::new(address, clients, Some(&server), Outside::new(gate.clone()));
    let (_bus, events) = broadcast::channel::<String>(8);
    host.start(&waiting, events).await;
    assert!(host.problem().is_none(), "{:?}", host.problem());
    Late {
        host,
        _attached: attached,
        _apps: apps,
    }
}

#[tokio::test]
async fn the_bridge_does_not_wait_on_a_token_ghost_refuses() {
    let w = world("").await;
    let began = std::time::Instant::now();
    let (client, task) = bridged(&w.url, "ghost.codex.removed", Duration::from_secs(20)).await;
    let e = client.expect_err("refused");
    assert!(e.contains("refused this client's token"), "{e}");
    assert!(began.elapsed() < Duration::from_secs(5));
    task.abort();
}

/// The bridge resends a call whose send failed part way, and the daemon may have run it:
/// the key it puts on the call makes the resend answer with the first call's result.
#[tokio::test]
async fn a_change_sent_again_with_its_key_is_made_once() {
    use ghost_core::ghost_app::outside::IDEMPOTENCY_KEY;
    let w = world("").await;
    let c = connect(&w.url, &w.token("codex"), ClientLifecycleMode::Initialize)
        .await
        .unwrap();
    let keyed = |key: &str, title: &str| {
        let mut meta = rmcp::model::RequestMetaObject::default();
        meta.insert(IDEMPOTENCY_KEY.to_string(), json!(key));
        let mut p = CallToolRequestParams::new("notify")
            .with_arguments(json!({"title": title}).as_object().cloned().unwrap());
        p.meta = Some(meta);
        p
    };
    let text = |r: rmcp::model::CallToolResult| -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect()
    };
    let first = text(
        c.call_tool(keyed("k-1", "the build finished"))
            .await
            .unwrap(),
    );
    let again = text(
        c.call_tool(keyed("k-1", "the build finished"))
            .await
            .unwrap(),
    );
    assert_eq!(first, again);
    let notified = |w: &World| {
        let (rows, _) = w.store.list_proposals(&ProposalQuery::page(50, 0)).unwrap();
        rows.iter().filter(|p| p.operation == "notify").count()
    };
    assert_eq!(notified(&w), 1, "made once");
    // Another key, or none, is another call.
    c.call_tool(keyed("k-2", "the build finished"))
        .await
        .unwrap();
    assert_eq!(notified(&w), 2);
    call(&c, "notify", json!({"title": "and again"}))
        .await
        .unwrap();
    assert_eq!(notified(&w), 3);
    // Another client's same key is its own.
    let other = connect(&w.url, &w.token("cursor"), ClientLifecycleMode::Initialize)
        .await
        .unwrap();
    other
        .call_tool(keyed("k-1", "the build finished"))
        .await
        .unwrap();
    assert_eq!(notified(&w), 4);
    let _ = c.cancel().await;
    let _ = other.cancel().await;
    w.host.stop().await;
}

#[test]
fn the_bridge_keys_every_tool_call_once_and_keeps_a_clients_own_key() {
    let call: rmcp::model::ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "say", "arguments": {"text": "hi"}}
    }))
    .unwrap();
    let keyed = with_idempotency_key(call);
    let key = |m: &rmcp::model::ClientJsonRpcMessage| {
        serde_json::to_value(m).unwrap()["params"]["_meta"]["ghost/idempotency-key"].clone()
    };
    let k = key(&keyed);
    assert!(k.as_str().is_some_and(|s| s.len() == 32), "{k}");
    // A resend is the same message: the same key. Keying it again changes nothing.
    assert_eq!(key(&with_idempotency_key(keyed.clone())), k);
    // A key the client chose is kept; a message that is not a tool call is left alone.
    let own: rmcp::model::ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 8, "method": "tools/call",
        "params": {"name": "say", "_meta": {"ghost/idempotency-key": "mine"}}
    }))
    .unwrap();
    assert_eq!(key(&with_idempotency_key(own)), json!("mine"));
    let list: rmcp::model::ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/list"
    }))
    .unwrap();
    assert_eq!(key(&with_idempotency_key(list)), Value::Null);
}

// --- the command line ---------------------------------------------------------------------------

fn cli(args: &[&str], cfg: &crate::GhostdSettings) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = crate::mcp::mcp_cli_in(&args, cfg, &mut out, &mut err, &|_| None);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn inside_an_appimage_a_command_client_is_given_the_address_instead() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = crate::GhostdSettings {
        data_dir: tmp.path().join("data"),
        db_path: tmp.path().join("data/ghost.db"),
        ..crate::GhostdSettings::default()
    };
    let args: Vec<String> = ["add", "codex", "--command"]
        .iter()
        .map(|a| (*a).to_string())
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let appimage = |k: &str| (k == "APPIMAGE").then(|| "/home/op/Ghost.AppImage".to_string());
    let code = crate::mcp::mcp_cli_in(&args, &cfg, &mut out, &mut err, &appimage);
    let out = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert!(out.contains("running from an AppImage"), "{out}");
    // No command naming the temporary mount, and no token file: the address, with the
    // token, as for any tool that connects to one.
    assert!(!out.contains("mcp serve"), "{out}");
    assert!(!out.contains(".mount_"), "{out}");
    assert!(
        out.contains("--transport http ghost http://127.0.0.1:47824/mcp"),
        "{out}"
    );
    assert!(!cfg.clients_dir().join("codex.token").exists());
    // Outside an AppImage the same command is the stdio bridge.
    let (code, out, _) = cli(&["add", "cursor", "--command"], &cfg);
    assert_eq!(code, 0);
    assert!(out.contains("mcp serve --client cursor"), "{out}");
}

#[test]
fn the_command_line_reads_the_listeners_address_from_ghosts_settings_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("settings.env");
    std::fs::write(
        &file,
        "GHOST_MCP_LISTEN=127.0.0.1:5151\nGHOST_CHIME=false\n",
    )
    .unwrap();
    let env = |more: &[(&str, &str)]| -> HashMap<String, String> {
        let mut env = HashMap::from([(
            "GHOST_SETTINGS_PATH".to_string(),
            file.to_string_lossy().into_owned(),
        )]);
        env.extend(
            more.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
        );
        env
    };
    let cfg = crate::mcp::settings_over(env(&[])).unwrap();
    assert_eq!(cfg.mcp_listen, "127.0.0.1:5151");
    assert_eq!(crate::mcp::connect_address(&cfg).unwrap(), "127.0.0.1:5151");
    // What the environment says wins, as it does for the daemon; an empty value is off.
    let cfg = crate::mcp::settings_over(env(&[("GHOST_MCP_LISTEN", "0.0.0.0:6161")])).unwrap();
    assert_eq!(crate::mcp::connect_address(&cfg).unwrap(), "127.0.0.1:6161");
    let cfg = crate::mcp::settings_over(env(&[("GHOST_MCP_LISTEN", "")])).unwrap();
    assert!(crate::mcp::connect_address(&cfg).is_none());
    // No file at all is the defaults.
    let none = HashMap::from([(
        "GHOST_SETTINGS_PATH".to_string(),
        tmp.path()
            .join("missing.env")
            .to_string_lossy()
            .into_owned(),
    )]);
    assert_eq!(
        crate::mcp::settings_over(none).unwrap().mcp_listen,
        "127.0.0.1:47824"
    );
}

#[test]
fn the_command_line_adds_lists_and_removes_and_shows_a_token_once() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = crate::GhostdSettings {
        data_dir: tmp.path().join("data"),
        db_path: tmp.path().join("data/ghost.db"),
        ..crate::GhostdSettings::default()
    };
    let (code, out, _) = cli(&["list"], &cfg);
    assert_eq!(code, 0);
    assert!(out.contains("No outside tools"));

    let (code, out, err) = cli(&["add", "claude-code"], &cfg);
    assert_eq!(code, 0, "{err}");
    let token = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("ghost.claude-code."))
        .expect("the token on a line of its own")
        .to_string();
    assert!(out.contains(&format!(
        "claude mcp add --transport http ghost http://127.0.0.1:47824/mcp --header \"Authorization: Bearer {token}\""
    )));
    assert!(
        out.contains("\"mcpServers\"") && out.contains("\"url\": \"http://127.0.0.1:47824/mcp\"")
    );
    assert!(!tmp.path().join("data/clients/claude-code.token").exists());

    // For a tool that only starts commands: the token in a private file, none in the config.
    let (code, out, _) = cli(&["add", "codex", "--command"], &cfg);
    assert_eq!(code, 0);
    assert!(out.contains("mcp serve --client codex"));
    assert!(tmp.path().join("data/clients/codex.token").exists());
    let config = out.split("mcp.json:").nth(1).unwrap();
    assert!(!config.contains("ghost.codex."), "{config}");

    let (code, out, _) = cli(&["list"], &cfg);
    assert_eq!(code, 0);
    assert!(out.contains("claude-code") && out.contains("codex") && out.contains("never used"));
    assert!(!out.contains(&token), "a list never shows a token");

    // The same name again, a bad name, and no name at all.
    assert_eq!(cli(&["add", "claude-code"], &cfg).0, 1);
    assert_eq!(cli(&["add", "Claude Code"], &cfg).0, 1);
    assert_eq!(cli(&["add", "Bad_Name"], &cfg).0, 1);
    assert_eq!(cli(&["add", "one", "two"], &cfg).0, 2);
    assert_eq!(cli(&["add"], &cfg).0, 2);
    assert_eq!(cli(&["frobnicate"], &cfg).0, 2);

    let (code, out, _) = cli(&["remove", "codex"], &cfg);
    assert_eq!(code, 0);
    assert!(out.contains("no longer works"));
    assert!(!tmp.path().join("data/clients/codex.token").exists());
    assert_eq!(cli(&["remove", "codex"], &cfg).0, 1);
    // What the daemon reads is the same store: the removed client is gone from it.
    let store = Store::open(&cfg.db_path).unwrap();
    assert!(store.get_client("codex").unwrap().is_none());
    let kept = store.get_client("claude-code").unwrap().unwrap();
    assert!(!kept.token_hash.contains(&token));
}

// --- agent_ask over the run with nobody there, and the schedules (ADR 0047) ------------------

mod asking {
    use std::time::Duration;

    use ghost_core::conversations::models::Role;
    use ghost_core::conversations::{
        Conversations, ConversationsOptions, SOURCE as CONVERSATIONS, conversations_source,
    };
    use ghost_core::gate::Request;
    use ghost_core::schedules::jobs::AgentState;
    use ghost_core::schedules::{Jobs, is_run_session};
    use ghost_llm::Options;
    use ghost_voice::conversation::Reach;
    use ghost_voice::headless::{HeadlessParts, HeadlessRunner, asked_mark};
    use ghost_voice::llm::{ModelPort, ScriptedModel, ScriptedReply};

    use super::*;

    /// The listener over the real pieces: the agents, the gate, Ghost's own app with the
    /// schedules, and `agent_ask` over the headless run with a scripted model.
    struct Asking {
        _tmp: tempfile::TempDir,
        store: Arc<Store>,
        convs: Arc<Conversations>,
        waiting: Arc<WaitingList>,
        clients: Arc<Clients>,
        jobs: Arc<Jobs>,
        runner: Arc<AskRunner>,
        host: OutsideHost,
        url: String,
        _attached: Attached,
        _apps: Arc<Apps>,
    }

    async fn asking(policy: &str, replies: Vec<ScriptedReply>, ttfb: Duration) -> Asking {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("ghost");
        std::fs::create_dir_all(&cfg).unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let convs = Arc::new(
            Conversations::new(store.clone(), &cfg, ConversationsOptions::default()).unwrap(),
        );
        convs.ensure_ghost().unwrap();
        let gate = Arc::new(
            WriteGate::new(store.clone())
                .with_autonomy(AutonomyPolicy::parse(policy, "<test>").unwrap()),
        );
        gate.register(CONVERSATIONS, conversations_source(&convs).operations);
        let hire = gate.operation(CONVERSATIONS, "create").unwrap();
        let prepared = (hire.prepare)(
            json!({"name": "Ledger", "job": "the books"})
                .as_object()
                .cloned()
                .unwrap(),
        )
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
        let (_bus, events) = broadcast::channel::<String>(64);
        let waiting =
            Arc::new(WaitingList::new(Items::new(store.clone())).with_changes(gate.clone()));
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
        let server = GhostServer::new(Parts {
            store: store.clone(),
            waiting: waiting.clone(),
            apps: Arc::downgrade(&apps),
            status: Arc::new(status),
        })
        .with_schedules(jobs.clone());
        let attached = ghost_app::attach(&apps, server.clone()).await.unwrap();
        gate.register(NAME, attached.app.operations());
        // What an agent reaches in a run: here, Ghost's own app.
        let reach: Reach = {
            let apps = apps.clone();
            Arc::new(move |_p| apps.sources())
        };
        let model = ScriptedModel::new(vec![replies], ttfb);
        let runner = Arc::new(
            AskRunner::new(
                Arc::new(HeadlessRunner::new(HeadlessParts {
                    conversations: convs.clone(),
                    gate: gate.clone(),
                    reach,
                    model: Arc::new(move || Ok(model.clone() as Arc<dyn ModelPort>)),
                    options: Options::default(),
                    skills: None,
                })),
                CancellationToken::new(),
            )
            // The scheduler's queue: a short wait, so a test of a busy agent is quick.
            .with_turns(jobs.turns().clone(), Duration::from_millis(300)),
        );
        let clients = Arc::new(Clients::new(store.clone(), tmp.path().join("clients")));
        let host = OutsideHost::new(
            "127.0.0.1:0",
            clients.clone(),
            Some(&server),
            Outside::new(gate.clone()).with_runner(runner.clone()),
        );
        host.start(&waiting, events).await;
        let url = format!("http://127.0.0.1:{}{PATH}", host.port());
        Asking {
            _tmp: tmp,
            store,
            convs,
            waiting,
            clients,
            jobs,
            runner,
            host,
            url,
            _attached: attached,
            _apps: apps,
        }
    }

    impl Asking {
        async fn client(&self, name: &str) -> Client {
            let token = self.clients.create(name, false).unwrap().token.to_string();
            connect(&self.url, &token, ClientLifecycleMode::Initialize)
                .await
                .unwrap()
        }

        fn transcript(&self, slug: &str) -> Vec<(Role, String, String)> {
            self.convs
                .turns(slug, 100)
                .unwrap()
                .into_iter()
                .map(|t| (t.role, t.session_id, t.text))
                .collect()
        }
    }

    #[tokio::test]
    async fn a_client_asks_an_agent_and_gets_its_written_answer() {
        let w = asking(
            "",
            vec![ScriptedModel::say("October came to 4,210.")],
            Duration::ZERO,
        )
        .await;
        let c = w.client("claude-code").await;
        let out = call(
            &c,
            "agent_ask",
            json!({"agent": "Ledger", "text": "the October total?"}),
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            json!({"agent": "ledger", "answer": "October came to 4,210."})
        );

        // The turn is in that agent's transcript, marked as from that tool, under a run's
        // session id; nothing of it is in Ghost's.
        let mark = asked_mark("Claude Code");
        let turns = w.transcript("ledger");
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_eq!(turns[0].0, Role::System);
        assert_eq!(turns[0].2, format!("{mark} the October total?"));
        assert_eq!(turns[1].0, Role::Assistant);
        assert_eq!(turns[1].2, format!("{mark} October came to 4,210."));
        assert!(
            turns
                .iter()
                .all(|t| is_run_session(&t.1) && t.1 == turns[0].1)
        );
        assert!(w.transcript("ghost").is_empty());

        // The audit has the ask as the client's change.
        let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
        let ask = rows.iter().find(|p| p.operation == "agent_ask").unwrap();
        assert_eq!(ask.status, ProposalStatus::Executed);
        assert_eq!(ask.session_id, "client:claude-code");
        let _ = c.cancel().await;
        w.host.stop().await;
    }

    /// ADR 0052: the same run for one agent asking another: marked as asked by that
    /// agent, under a session recall can tell from an outside tool's, and told it cannot
    /// hand the task on.
    #[tokio::test]
    async fn an_agent_asks_another_through_the_same_run() {
        let w = asking(
            "",
            vec![ScriptedModel::say("Paid on the 3rd.")],
            Duration::ZERO,
        )
        .await;
        let ledger = w.convs.get("ledger").unwrap().unwrap();
        let answer = w
            .runner
            .ask(&ledger, "Is October paid?", "agent:ghost")
            .await
            .unwrap();
        assert_eq!(answer, "Paid on the 3rd.");
        let turns = w.transcript("ledger");
        assert_eq!(
            turns[0].2,
            "[asked by Ghost, another agent] Is October paid?"
        );
        assert!(turns[0].1.starts_with("run-agentask-"), "{}", turns[0].1);
        // An outside tool's run says whose it is.
        let w = asking("", vec![ScriptedModel::say("Yes.")], Duration::ZERO).await;
        let ledger = w.convs.get("ledger").unwrap().unwrap();
        w.runner
            .ask(&ledger, "Paid?", "client:claude-code")
            .await
            .unwrap();
        let session = w.transcript("ledger")[0].1.clone();
        assert!(session.starts_with("run-ask-claude-code-"), "{session}");
        assert_eq!(session.len(), "run-ask-claude-code-".len() + 32);
    }

    /// A tool that goes away mid-`agent_ask`: the HTTP connection is closed with the run
    /// still waiting on the model. The run is stopped, what was asked is kept as not
    /// answered, and the audit records the ask as stopped, not done.
    #[tokio::test]
    async fn a_connection_closed_mid_ask_stops_the_run_and_it_is_recorded() {
        // The model would take ten minutes to answer.
        let w = asking(
            "",
            vec![ScriptedModel::say("Too late.")],
            Duration::from_secs(600),
        )
        .await;
        let token = w
            .clients
            .create("claude-code", false)
            .unwrap()
            .token
            .to_string();
        let http = reqwest::Client::new();
        let send = |body: Value, session: Option<String>| {
            let mut req = http
                .post(&w.url)
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("authorization", format!("Bearer {token}"))
                .body(body.to_string());
            if let Some(s) = session {
                req = req.header("mcp-session-id", s);
            }
            req.send()
        };
        let hello = send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}}}),
            None,
        )
        .await
        .unwrap();
        assert_eq!(hello.status(), 200);
        let session = hello
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        let _ = hello.bytes().await;
        let done = send(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            session.clone(),
        )
        .await
        .unwrap();
        assert!(done.status().is_success(), "{}", done.status());
        // The ask: its response is a stream that would carry the answer. Once the run has
        // started (its row in the gate), the tool goes away: the response is dropped and
        // the connection with it.
        let asked = send(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "agent_ask", "arguments": {"agent": "ledger", "text": "the total?"}}}),
            session,
        )
        .await
        .unwrap();
        assert_eq!(asked.status(), 200);
        let started = || {
            w.store
                .list_proposals(&ProposalQuery::page(10, 0))
                .unwrap()
                .0
                .into_iter()
                .find(|p| p.operation == "agent_ask")
        };
        for _ in 0..100 {
            if started().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(started().is_some(), "the ask reached the gate");
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(asked);
        drop(http);

        // Stopped, not run to its ten minutes: what was asked is kept as not answered,
        // and the ask's outcome in the audit is not a success.
        let began = std::time::Instant::now();
        let mut kept = Vec::new();
        for _ in 0..1000 {
            kept = w.transcript("ledger");
            if !kept.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(
            kept[0].2,
            format!("{} the total? (not answered)", asked_mark("Claude Code"))
        );
        // Within moments, not at the run's own ten-minute limit.
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "{:?}",
            began.elapsed()
        );
        let ask = started().unwrap();
        let mut events = Vec::new();
        for _ in 0..100 {
            events = w.store.events(&ask.id).unwrap();
            if events.iter().any(|e| e.event == "finished") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let finished = events
            .iter()
            .find(|e| e.event == "finished")
            .expect("the ask has an outcome");
        assert_eq!(
            finished.detail.as_ref().unwrap()["status"],
            "failed",
            "the ask is recorded as stopped: {events:?}"
        );
        assert!(
            events.iter().any(|e| e
                .detail
                .as_ref()
                .is_some_and(|d| d.to_string().contains("stopped before Ledger answered"))),
            "{events:?}"
        );
        // And the agent is free again for the next.
        assert!(!w.jobs.turns().busy("ledger"));
        w.host.stop().await;
    }

    /// One piece of work at a time per agent: an ask waits for a scheduled run of the same
    /// agent, goes before the next one, and is told the agent is busy after its wait.
    #[tokio::test]
    async fn an_ask_takes_turns_with_the_agents_scheduled_runs() {
        let w = asking(
            "",
            vec![ScriptedModel::say("Paid."), ScriptedModel::say("Paid.")],
            Duration::from_millis(100),
        )
        .await;
        let ledger = w.convs.get("ledger").unwrap().unwrap();
        let turns = w.jobs.turns().clone();
        // A scheduled run of Ledger is going: the ask waits, and past its wait is refused
        // with a clear error; nothing ran.
        assert!(turns.try_run("ledger"));
        let e = w
            .runner
            .ask(&ledger, "Is October paid?", "client:claude-code")
            .await
            .unwrap_err();
        assert!(e.contains("Ledger is busy with other work"), "{e}");
        assert!(w.transcript("ledger").is_empty());
        // The run ends while an ask waits: the ask goes, and no run starts until it is done.
        let asking = tokio::spawn({
            let (runner, ledger) = (w.runner.clone(), ledger.clone());
            async move { runner.ask(&ledger, "Is October paid?", "agent:ghost").await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        turns.release("ledger");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(turns.busy("ledger"));
        assert!(!turns.try_run("ledger"), "a run waits for the ask");
        assert_eq!(asking.await.unwrap().unwrap(), "Paid.");
        assert!(!turns.busy("ledger"));
        assert!(turns.try_run("ledger"));
        turns.release("ledger");
        // Another agent was never held up.
        assert!(turns.try_run("ghost"));
    }

    #[tokio::test]
    async fn a_change_that_would_ask_inside_the_turn_is_set_aside_on_the_waiting_list() {
        // The operator's permissions ask before an agent tells them something. The client
        // wrote the question, and nothing a client writes is the operator's yes: the
        // change the agent makes for it waits, and the turn carries on and says so.
        let w = asking(
            "[permissions]\nask = [\"ghost.notify\"]\n",
            vec![
                ScriptedModel::call(
                    "",
                    "ghost_notify",
                    json!({"title": "The October total is ready"})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
                ScriptedModel::say("I have asked the operator before telling them."),
            ],
            Duration::ZERO,
        )
        .await;
        let c = w.client("claude-code").await;
        let out = call(
            &c,
            "agent_ask",
            json!({"agent": "ledger", "text": "tell the operator the October total is ready"}),
        )
        .await
        .unwrap();
        assert_eq!(
            out["answer"],
            "I have asked the operator before telling them."
        );

        let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
        let notify = rows.iter().find(|p| p.operation == "notify").unwrap();
        assert_eq!(notify.status, ProposalStatus::Pending, "it has not run");
        assert!(is_run_session(&notify.session_id), "{}", notify.session_id);
        w.waiting.refresh(None).unwrap();
        let items = w.waiting.items.open(Some(Tell::Tell)).unwrap();
        assert!(
            items
                .iter()
                .any(|i| i.source == "gate" && i.key == format!("change:{}", notify.id)),
            "{items:?}"
        );
        // Nothing was told: the agent's item is not there.
        assert!(!items.iter().any(|i| i.source == "agent:ledger"));
        let _ = c.cancel().await;
        w.host.stop().await;
    }

    #[tokio::test]
    async fn an_agent_ghost_does_not_have_is_a_clear_error_and_nothing_runs() {
        let w = asking("", vec![ScriptedModel::say("Never said.")], Duration::ZERO).await;
        let c = w.client("claude-code").await;
        let e = call(&c, "agent_ask", json!({"agent": "nobody", "text": "hi"}))
            .await
            .unwrap_err();
        assert!(
            e.contains("no agent called nobody") && e.contains("ledger") && e.contains("ghost"),
            "{e}"
        );
        assert!(w.transcript("ledger").is_empty() && w.transcript("ghost").is_empty());
        let _ = c.cancel().await;
        w.host.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_turn_stops_when_the_request_it_was_made_for_does_and_is_bounded() {
        // The model takes a quarter of an hour. The request is cancelled: the turn stops,
        // and what was asked is still in the transcript, as not answered.
        let slow = Duration::from_secs(900);
        let w = asking("", vec![ScriptedModel::say("Too late.")], slow).await;
        let ledger = w.convs.get("ledger").unwrap().unwrap();
        let request = CancellationToken::new();
        let turn = {
            let (runner, ledger, request) = (w.runner.clone(), ledger.clone(), request.clone());
            tokio::spawn(ghost_core::apps::with_cancel(request, async move {
                runner.ask(&ledger, "the total?", "client:cursor").await
            }))
        };
        tokio::time::sleep(Duration::from_secs(5)).await;
        request.cancel();
        let e = turn.await.unwrap().unwrap_err();
        assert!(e.contains("stopped before Ledger answered"), "{e}");
        let turns = w.transcript("ledger");
        assert_eq!(turns.len(), 1, "{turns:?}");
        assert_eq!(
            turns[0].2,
            format!("{} the total? (not answered)", asked_mark("Cursor"))
        );

        // Left alone, it is bounded by the run's own clock: ten minutes.
        let e = w
            .runner
            .ask(&ledger, "the total?", "client:cursor")
            .await
            .unwrap_err();
        assert!(e.contains("did not finish within 10 minutes"), "{e}");
    }

    #[tokio::test]
    async fn a_client_sets_up_a_schedule_as_its_own_and_manages_only_its_own() {
        let w = asking("", Vec::new(), Duration::ZERO).await;
        let claude = w.client("claude-code").await;
        let cursor = w.client("cursor").await;
        let names: Vec<String> = claude
            .list_all_tools()
            .await
            .unwrap()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(names.iter().any(|n| n == "schedules_list"));
        assert!(names.iter().any(|n| n == "schedule_manage"));
        // Finding an app's tool is for an agent, which can call it.
        assert!(!names.iter().any(|n| n == "tools_search"), "{names:?}");

        // Through the gate, attributed to the client, run as the agent it names (Ghost
        // when it names none).
        let made = call(
            &claude,
            "schedule_manage",
            json!({"action": "create", "name": "build watch", "every": "30m",
                   "instruction": "Check the build and say if it is red."}),
        )
        .await
        .unwrap();
        assert_eq!(made["status"], "created");
        assert_eq!(made["agent"], "ghost");
        assert_eq!(made["made_by"], "client:claude-code");
        let job = w.jobs.find("build watch").unwrap().unwrap();
        assert_eq!(job.created_by, "client:claude-code");
        let books = call(
            &claude,
            "schedule_manage",
            json!({"action": "create", "name": "books", "every": "1d", "agent": "ledger",
                   "instruction": "Close the day."}),
        )
        .await
        .unwrap();
        assert_eq!(books["agent"], "ledger");
        let (rows, _) = w.store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
        assert!(
            rows.iter()
                .filter(|p| p.operation == "schedule_manage")
                .all(|p| p.session_id == "client:claude-code"
                    && p.status == ProposalStatus::Executed)
        );

        // Another client cannot pause it, read it, or see what it says.
        let e = call(
            &cursor,
            "schedule_manage",
            json!({"action": "pause", "id": "build watch"}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("an outside tool manages only its own"), "{e}");
        assert!(w.jobs.find("build watch").unwrap().unwrap().enabled);
        let e = call(&cursor, "schedules_list", json!({"id": "build watch"}))
            .await
            .unwrap_err();
        assert!(e.contains("that you set up"), "{e}");
        let seen = call(&cursor, "schedules_list", json!({})).await.unwrap();
        let rows = seen["schedules"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(|r| r["yours"] == false
                && r.get("name").is_none()
                && r.get("made_by").is_none())
        );
        // A schedule an agent or the operator made is not a client's to manage either.
        let theirs = w
            .jobs
            .create(
                &ghost_core::schedules::JobEdit {
                    name: Some("morning".into()),
                    agent: Some("ledger".into()),
                    instruction: Some("Look.".into()),
                    when: Some(ghost_core::schedules::Written {
                        every: Some("1h".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ghost_core::schedules::OPERATOR,
                ghost_core::time::utcnow(),
            )
            .unwrap();
        let e = call(
            &claude,
            "schedule_manage",
            json!({"action": "remove", "id": theirs.id}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("manages only its own"), "{e}");

        // Its own it reads whole, pauses and removes.
        let mine = call(&claude, "schedules_list", json!({"id": "build watch"}))
            .await
            .unwrap();
        assert_eq!(mine["instruction"], "Check the build and say if it is red.");
        let paused = call(
            &claude,
            "schedule_manage",
            json!({"action": "pause", "id": "build watch"}),
        )
        .await
        .unwrap();
        assert_eq!(paused["status"], "paused");
        let gone = call(
            &claude,
            "schedule_manage",
            json!({"action": "remove", "id": "build watch"}),
        )
        .await
        .unwrap();
        assert_eq!(gone["status"], "removed");
        assert!(w.jobs.find("build watch").unwrap().is_none());
        let _ = claude.cancel().await;
        let _ = cursor.cancel().await;
        w.host.stop().await;
    }
}

/// ADR 0052: a client recalls the turns of the runs it started, and nothing an agent said
/// to the operator or to another client; it reads the watches and the routes; and asking
/// for an agent's memory, or naming another agent's slug in `_agent`, gets it nothing.
#[tokio::test]
async fn a_client_recalls_only_its_own_runs() {
    use ghost_core::conversations::models::Role;
    let w = world("").await;
    let c = w
        .client("claude-code", ClientLifecycleMode::Initialize)
        .await;
    let ledger = w
        .store
        .get_conversation("ledger", false)
        .unwrap()
        .unwrap()
        .id;
    let hex = |c: char| c.to_string().repeat(32);
    for (session, text) in [
        (
            format!("run-ask-claude-code-{}", hex('a')),
            "the invoice went out",
        ),
        (
            format!("run-ask-cursor-{}", hex('b')),
            "the invoice for cursor",
        ),
        ("s1".to_string(), "the invoice is between us"),
    ] {
        w.store
            .add_turn(&ledger, &session, Role::System, text)
            .unwrap();
    }
    let found = call(&c, "recall", json!({"query": "invoice", "_agent": "ghost"}))
        .await
        .unwrap();
    let results = found["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{found}");
    assert_eq!(results[0]["kind"], "asked by an outside tool");
    let page = call(&c, "transcript_read", json!({"agent": "ledger"}))
        .await
        .unwrap();
    assert_eq!(page["turns"].as_array().unwrap().len(), 1, "{page}");
    assert!(call(&c, "watches_list", json!({})).await.is_ok());
    assert!(call(&c, "routes_read", json!({})).await.is_ok());
    assert!(
        call(&c, "memory_read", json!({"agent": "ledger"}))
            .await
            .is_err()
    );
}
