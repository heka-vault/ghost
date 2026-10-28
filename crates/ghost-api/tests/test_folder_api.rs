//! What the desktop reads from Ghost's folder (ADR 0035): the rules, the outputs, the
//! watches and webhooks, with no department in any of it.

mod common;

use common::World;
use ghost_api::Controller;
use serde_json::{Value, json};

fn watch() -> Value {
    json!({
        "app": "github",
        "read": "list_workflow_runs",
        "args": {"owner": "heka-vault", "repo": "ghost"},
        "every": 60,
        "items": "workflow_runs",
        "fields": {"id": {"at": "id", "type": "int"}},
        "key": "{id}",
        "title": "CI failed: {id}",
    })
}

fn hook() -> Value {
    json!({
        "verify": "github",
        "secret": "${GITHUB_HOOK_SECRET}",
        "events": {
            "workflow_run": {
                "fields": {
                    "id": {"at": "workflow_run.id", "type": "int"},
                    "conclusion": {"at": "workflow_run.conclusion"},
                },
                "when": [{"field": "conclusion", "op": "==", "value": "failure"}],
                "key": "{id}",
                "title": "CI failed: {id}",
                "check": {
                    "app": "github",
                    "read": "get_workflow_run",
                    "args": {"run_id": "{id}"},
                    "fields": {"conclusion": {"at": "conclusion"}},
                },
            }
        },
    })
}

#[tokio::test]
async fn the_rules_are_read_and_added_through_the_gate() {
    let w = World::new();
    let c = w.client();
    let r = c.get("/rules").await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json,
        json!({"path": w.rules.path.display().to_string(), "text": ""})
    );
    let r = c
        .post("/rules", Some(json!({"text": "  Always use UK dates.  "})))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["status"], "success");
    assert_eq!(r["applied"], json!(["add the rule: Always use UK dates."]));
    assert_eq!(
        c.get("/rules").await["text"],
        "# Ghost\n\nAlways use UK dates.\n"
    );
    // through the gate: audited under the desktop's own session, like the spoken rules.add
    let audit = c.get("/audit?source=rules").await;
    let items = audit["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let entry = &items[0];
    assert_eq!(entry["operation"], "add");
    assert_eq!(entry["session_id"], "ui");
    assert!(entry.get("department").is_none());
    assert_eq!(
        c.post("/rules", Some(json!({"text": ""}))).await.status,
        422
    );
    assert_eq!(
        c.post("/rules", Some(json!({"text": "x".repeat(1001)})))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn a_rule_the_permissions_block_is_refused() {
    let w = World::new();
    let c = w.client();
    std::fs::write(&w.policy_path, "[permissions]\nblock = [\"rules\"]\n").unwrap();
    w.control.reload_policy().unwrap();
    let r = c.post("/rules", Some(json!({"text": "Be brief."}))).await;
    assert_eq!(r.status, 409);
    assert!(r.detail().contains("blocked"));
    assert_eq!(c.get("/rules").await["text"], "");
}

#[tokio::test]
async fn outputs_are_listed_from_one_folder() {
    let w = World::new();
    let c = w.client();
    assert_eq!(c.get("/outputs").await.json, json!([]));
    let out = &w.outputs.folder;
    std::fs::create_dir_all(out).unwrap();
    std::fs::write(out.join("week.html"), "<h1>Week</h1>").unwrap();
    std::fs::write(out.join(".hidden.md"), "x").unwrap();
    let rows = c.get("/outputs").await;
    let shape: Vec<(String, i64)> = rows
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_string(),
                f["bytes"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(shape, vec![("week.html".to_string(), 13)]);
    assert_eq!(rows[0]["path"], out.join("week.html").display().to_string());
    assert!(rows[0]["modified"].is_string());
}

#[tokio::test]
async fn watches_lists_the_watches_and_hooks_and_what_is_wrong() {
    // `Watch.app` is the app it reads (ADR 0034); the listing said `server` and failed.
    let mut w = World::new();
    let c = w.client();
    assert_eq!(
        c.get("/watches").await.json,
        json!({"watches": [], "hooks": [], "listening": [], "problems": []})
    );
    let config = w.config.clone();
    std::fs::write(
        config.join("watches.json"),
        json!({"ci": watch()}).to_string(),
    )
    .unwrap();
    std::fs::write(config.join("hooks.json"), json!({"ci": hook()}).to_string()).unwrap();
    w.hooks_listening = vec!["127.0.0.1:8766".into()];
    let c = w.client();
    let r = c.get("/watches").await; // re-read now
    assert_eq!(r.status, 200, "{}", r.text);
    let v = &r.json;
    let watches: Vec<(Value, Value, Value)> = v["watches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["name"].clone(), x["app"].clone(), x["read"].clone()))
        .collect();
    assert_eq!(
        watches,
        vec![(json!("ci"), json!("github"), json!("list_workflow_runs"))]
    );
    let hooks: Vec<(Value, Value, Value)> = v["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h["name"].clone(), h["path"].clone(), h["on"].clone()))
        .collect();
    assert_eq!(hooks, vec![(json!("ci"), json!("/hooks/ci"), json!(false))]);
    assert_eq!(v["listening"], json!(["127.0.0.1:8766"]));
    assert_eq!(v["problems"], json!([]));
    // a broken file, and what an older layout left, are problems to show
    std::fs::write(config.join("watches.json"), "{nope").unwrap();
    std::fs::write(
        config.join("department.json"),
        json!({"title": "Ghost"}).to_string(),
    )
    .unwrap();
    let v = c.get("/watches").await;
    assert_eq!(v["watches"], json!([]));
    let problems = v["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 2, "{problems:?}");
    let joined = problems
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("department.json") && joined.contains("watches.json"));
}

#[tokio::test]
async fn the_department_routes_are_gone() {
    let w = World::new();
    let c = w.client();
    assert_eq!(c.get("/departments").await.status, 404);
    assert_eq!(c.get("/departments/ghost").await.status, 404);
    assert_eq!(c.get("/departments/ghost/journal").await.status, 404);
    assert_eq!(
        c.post("/departments", Some(json!({"name": "Sales"})))
            .await
            .status,
        404
    );
    assert_eq!(
        c.post("/departments/ghost/rules", Some(json!({"text": "x"})))
            .await
            .status,
        404
    );
    let status = c.get("/status").await;
    assert!(status.json.get("department").is_none());
    assert!(
        c.get("/waiting").await["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("department").is_none())
    );
}
