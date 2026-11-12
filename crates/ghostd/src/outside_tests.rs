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
use ghost_core::store::{ProposalQuery, Store};
use ghost_core::waiting::{Items, Response as Tell};
use rmcp::model::{CallToolRequestParams, ProtocolVersion, ReadResourceRequestParams};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Map;

use super::*;
use crate::mcp::{Target, bridge, mcp_cli};

type Client = RunningService<RoleClient, ()>;

struct World {
    _tmp: tempfile::TempDir,
    store: Arc<Store>,
    gate: Arc<WriteGate>,
    waiting: Arc<WaitingList>,
    clients: Arc<Clients>,
    host: OutsideHost,
    url: String,
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
            .with_on_change(Arc::new(move || {
                let _ = bus.send(r#"{"type":"waiting","count":0}"#.to_string());
            })),
    );
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
    for (slug, name, job) in [("ghost", "Ghost", ""), ("ledger", "Ledger", "The books")] {
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
const TOOLS: [&str; 13] = [
    "agent_ask",
    "agents_list",
    "apps_list",
    "audit_recent",
    "docs_read",
    "docs_search",
    "events_wait",
    "notify",
    "resolve",
    "say",
    "status",
    "waiting_ack",
    "waiting_snooze",
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
    // their tokens, transcripts or audio: by its name, and by the name of any argument it
    // takes.
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
        "transcript",
        "transcripts",
        "turn",
        "turns",
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
        "confirm",
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
    // A tool Ghost has for itself is not one a client can call.
    for tool in [
        "passwords_list",
        "policy_read",
        "clients_add",
        "change",
        "waiting_list",
    ] {
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

// --- the command line ---------------------------------------------------------------------------

fn cli(args: &[&str], cfg: &crate::GhostdSettings) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = mcp_cli(&args, cfg, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
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
