//! How much Ghost read (ADR 0060): every call an agent makes to an app is kept as counts
//! and sizes, with the session or run and the agent, and never its contents. The size is
//! what the app sent, before the result was fitted to the model's budget.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ghost_core::apps::{App, AppSpec, Session, SessionError, source_of};
use ghost_core::reach::ReachRow;
use ghost_core::{Store, WriteGate};
use ghost_voice::intercept::no_action;
use ghost_voice::tools::{ToolDispatcher, ToolsParams};
use rmcp::model::{CallToolResult, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

/// An app whose `list` returns 500 issues, about 36 kB, and whose `fail` fails.
struct Issues;

#[async_trait]
impl Session for Issues {
    async fn list_tools(&self) -> Result<Vec<Tool>, SessionError> {
        let schema = json!({"type": "object", "properties": {}});
        let schema = Arc::new(schema.as_object().unwrap().clone());
        Ok(["list", "fail"]
            .iter()
            .map(|n| {
                Tool::new(n.to_string(), format!("{n}."), schema.clone())
                    .annotate(ToolAnnotations::new().read_only(true))
            })
            .collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        _arguments: Map<String, Value>,
    ) -> Result<CallToolResult, SessionError> {
        if name == "fail" {
            return Err(SessionError::Failed("Boom".into()));
        }
        let items: Vec<Value> = (0..500)
            .map(|i| json!({"number": i, "title": format!("Issue number {i} in the tracker"), "state": "open"}))
            .collect();
        Ok(CallToolResult::structured(
            json!({"total_count": 500, "items": items}),
        ))
    }

    async fn close(&self) {}
}

async fn kept(store: &Store, n: usize) -> Vec<ReachRow> {
    let since = chrono::Utc::now() - chrono::Duration::hours(1);
    let until = chrono::Utc::now() + chrono::Duration::hours(1);
    for _ in 0..100 {
        let rows = store.reach_between(since, until, &[]).unwrap();
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    store.reach_between(since, until, &[]).unwrap()
}

#[tokio::test]
async fn a_read_is_kept_as_counts_and_the_size_the_app_sent_never_its_contents() {
    let app = Arc::new(App::new(
        AppSpec {
            title: "Issues".into(),
            ..AppSpec::at("issues", "memory://")
        },
        Arc::new(std::collections::HashMap::<String, String>::new()),
        None,
    ));
    app.attach(Arc::new(Issues)).await.unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let gate = Arc::new(WriteGate::new(store.clone()));
    let heard: Arc<Mutex<Vec<ReachRow>>> = Arc::default();
    {
        let heard = heard.clone();
        gate.on_reach(Arc::new(move |r| heard.lock().unwrap().push(r.clone())));
    }
    let tools = ToolDispatcher::alone(
        ToolsParams {
            sources: vec![source_of(&app)],
            gate,
            session_id: "run-r1".into(),
            on_hang_up: no_action(),
            on_hold: None,
            turns: None,
            events: None,
            conversations: None,
            on_call: None,
            last_request: None,
            probe: None,
        },
        "ledger",
    );
    let got = tools.call("issues_list", Map::new(), "c1").await;
    // The model was given what fits its budget...
    let given = serde_json::to_string(&got.result).unwrap().len();
    assert!(given < 30_000, "{given}");
    let failed = tools.call("issues_fail", Map::new(), "c2").await;
    assert!(failed.error_text().is_some());
    // ...a session's own tool is not an app's call, and is not kept.
    tools.call("hang_up", Map::new(), "c3").await;

    let rows = kept(&store, 2).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    let read = &rows[0];
    assert_eq!(
        (
            read.session_id.as_str(),
            read.agent.as_str(),
            read.app.as_str(),
            read.tool.as_str()
        ),
        ("run-r1", "ledger", "issues", "list")
    );
    assert!(read.read && read.ok);
    assert_eq!(read.items, Some(500));
    // ...and the size kept is what the app sent: far more than the model was shown.
    assert!(
        read.bytes as usize > given + 5_000,
        "{} vs {given}",
        read.bytes
    );
    let fail = &rows[1];
    assert!(!fail.ok);
    assert_eq!((fail.items, fail.bytes), (None, 0));
    assert_eq!(heard.lock().unwrap().len(), 2);
    // Nothing of what came back is in the row: a row has no field for it.
    let shown = format!("{rows:?}");
    assert!(!shown.contains("Issue number"), "{shown}");
}
