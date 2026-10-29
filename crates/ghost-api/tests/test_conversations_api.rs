//! Agent routes (ADR 0014, ADR 0035): registry, edits through the gate, switch and compact
//! delegated to the daemon, voices and samples, retire and rehire.

mod common;

use common::{World, strings};
use ghost_api::Phase;
use ghost_core::conversations::Role;
use serde_json::{Value, json};

#[tokio::test]
async fn ghost_listed_first_and_status_carries_active() {
    let w = World::new();
    let c = w.client();
    let r = c.get("/conversations").await;
    assert_eq!(r.status, 200);
    assert_eq!(strings(&r.json, "slug"), vec!["ghost".to_string()]);
    let ghost = &r[0];
    assert_eq!(ghost["is_ghost"], true);
    assert_eq!(ghost["active"], false);
    assert_eq!(ghost["voice"], "af_heart");
    assert!(ghost.get("department").is_none() && ghost.get("is_default").is_none()); // ADR 0035
    assert_eq!(c.get("/status").await["conversation"], Value::Null);
}

#[tokio::test]
async fn create_get_update_and_delete() {
    let w = World::new();
    let c = w.client();
    let r = c
        .post(
            "/conversations",
            Some(json!({
                "name": "Accounting",
                "aliases": ["the accountants"],
                "voice": "am_adam",
                "job": "the books",
            })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let d = &r.json;
    assert_eq!(d["slug"], "accounting");
    assert_eq!(d["voice"], "am_adam");
    assert_eq!(d["turn_count"], 0);
    assert_eq!(d["own_voice"], true);
    assert_eq!(d["job"], "the books");
    assert!(d.get("department").is_none());
    assert!(
        d["dir"]
            .as_str()
            .unwrap()
            .ends_with("config/agents/accounting")
    );
    assert!(!d["id"].as_str().unwrap().is_empty());

    // a duplicate is a conflict, an unknown voice is a validation error
    assert_eq!(
        c.post("/conversations", Some(json!({"name": "accounting"})))
            .await
            .status,
        409
    );
    assert_eq!(
        c.post(
            "/conversations",
            Some(json!({"name": "Ops", "voice": "zz"}))
        )
        .await
        .status,
        422
    );

    let r = c
        .patch(
            "/conversations/accounting",
            json!({
                "name": "Finance",
                "job": "the money",
                "persona": "Dry and precise.",
                "memory": "We were reconciling September.",
                "notes": ["books close on the fifth", "  ", "Sam owns payroll"],
                "speed": 1.2,
            }),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let d = &r.json;
    assert_eq!(d["name"], "Finance");
    assert_eq!(d["slug"], "accounting");
    assert_eq!(d["job"], "the money");
    assert_eq!(d["persona"].as_str().unwrap().trim(), "Dry and precise.");
    assert_eq!(
        d["memory"].as_str().unwrap().trim(),
        "We were reconciling September."
    );
    assert_eq!(
        d["notes"],
        json!(["books close on the fifth", "Sam owns payroll"])
    );
    assert_eq!(d["speed"], 1.2);
    assert_eq!(d["voice"], "am_adam");
    assert_eq!(d["compactions"], 1); // the memory edit is a version
    assert_eq!(
        w.conversations.files.persona("accounting").unwrap().trim(),
        "Dry and precise."
    );
    // an empty voice means Ghost's
    let r = c
        .patch("/conversations/accounting", json!({"voice": ""}))
        .await;
    assert_eq!(r["own_voice"], false);
    assert_eq!(r["voice"], "af_heart");

    let r = c
        .post(
            "/conversations/accounting/notes",
            Some(json!({"text": "  call   Pat  "})),
        )
        .await;
    assert_eq!(r["notes"].as_array().unwrap().last().unwrap(), "call Pat");

    // every edit is an audited write under the ui session
    let audit = c.get("/audit?source=conversations").await;
    assert!(audit["total"].as_i64().unwrap() >= 6);
    let sessions: std::collections::BTreeSet<String> =
        strings(&audit["items"], "session_id").into_iter().collect();
    assert_eq!(sessions, ["ui".to_string()].into_iter().collect());

    let comps = c.get("/conversations/accounting/compactions").await;
    assert_eq!(comps.json.as_array().unwrap().len(), 1);
    assert_eq!(comps[0]["source"], "edit");

    assert_eq!(c.delete("/conversations/ghost").await.status, 409);
    assert_eq!(
        c.delete("/conversations/accounting").await.json,
        json!({"ok": true, "detail": null})
    );
    assert_eq!(c.get("/conversations/accounting").await.status, 404);
    assert_eq!(c.get("/conversations/nope").await.status, 404);
}

#[tokio::test]
async fn reset_and_turns() {
    let w = World::new();
    let c = w.client();
    let convs = &w.conversations;
    convs.record("ghost", "s0", Role::User, "hello").unwrap();
    convs.record("ghost", "s0", Role::Assistant, "Hi.").unwrap();
    let turns = c.get("/conversations/ghost/turns").await;
    assert_eq!(
        strings(&turns.json, "text"),
        vec!["hello".to_string(), "Hi.".to_string()]
    );
    assert_eq!(turns[0]["compacted"], false);
    assert_eq!(c.post("/conversations/ghost/reset", None).await["ok"], true);
    assert_eq!(c.get("/conversations/ghost/turns").await.json, json!([]));
}

#[tokio::test]
async fn switch_and_compact_need_a_session() {
    let w = World::new();
    let c = w.client();
    assert_eq!(
        c.post("/conversations/ghost/switch", None).await.status,
        409
    );
    assert_eq!(
        c.post("/conversations/ghost/compact", None).await.status,
        409
    );
    c.post("/conversations", Some(json!({"name": "Legal"})))
        .await;
    w.state.set_phase(Phase::Passive, None, None);
    assert_eq!(c.post("/wake", None).await["ok"], true);
    assert_eq!(
        c.post("/conversations/legal/switch", None).await["ok"],
        true
    );
    assert!(w.control.has_call(json!(["switch", "legal"])));
    assert_eq!(c.get("/status").await["conversation"], "legal");
    let listed = c.get("/conversations").await;
    let legal = listed
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["slug"] == "legal")
        .unwrap();
    assert_eq!(legal["active"], true);
    // the active agent cannot be reset or deleted from under the session
    assert_eq!(c.post("/conversations/legal/reset", None).await.status, 409);
    assert_eq!(c.delete("/conversations/legal").await.status, 409);
    assert_eq!(
        c.post("/conversations/ghost/compact", None).await.status,
        409
    );
    assert_eq!(
        c.post("/conversations/legal/compact", None).await["ok"],
        true
    );
    assert!(w.control.has_call(json!(["compact", "legal"])));
}

#[tokio::test]
async fn voices_and_sample() {
    let w = World::new();
    let c = w.client();
    let r = c.get("/voices").await;
    assert_eq!(
        r[0],
        json!({"id": "af_heart", "label": "American female: Heart"})
    );
    assert_eq!(
        strings(&r.json, "id"),
        vec![
            "af_heart".to_string(),
            "am_adam".to_string(),
            "bf_emma".to_string()
        ]
    );
    assert_eq!(
        c.post("/voices/bf_emma/sample", Some(json!({"speed": 0.9})))
            .await["ok"],
        true
    );
    assert!(w.control.has_call(json!(["sample", "bf_emma", 0.9, null])));
    assert_eq!(c.post("/voices/zz_nobody/sample", None).await.status, 404);
}

#[tokio::test]
async fn an_agent_without_its_own_voice_speaks_in_ghosts() {
    let w = World::new();
    let c = w.client();
    c.patch("/conversations/ghost", json!({"voice": "bf_emma"}))
        .await;
    let d = c
        .post("/conversations", Some(json!({"name": "Brand"})))
        .await;
    assert_eq!(d["voice"], "bf_emma");
    assert_eq!(d["own_voice"], false);
    // the department field is gone from a hire, and so is the move
    let r = c
        .post(
            "/conversations",
            Some(json!({"name": "Sales", "department": "marketing"})),
        )
        .await;
    assert_eq!(r.status, 200);
    assert!(r.json.get("department").is_none());
    let status = c
        .post(
            "/conversations/brand/move",
            Some(json!({"department": "x"})),
        )
        .await
        .status;
    assert!([404, 405].contains(&status));
}

#[tokio::test]
async fn retire_rehire_and_seed_from() {
    let w = World::new();
    let c = w.client();
    c.post("/conversations", Some(json!({"name": "Heka New"})))
        .await;
    c.patch("/conversations/heka-new", json!({"memory": "Shipped v1."}))
        .await;
    assert_eq!(
        c.post("/conversations/ghost/retire", None).await.status,
        409
    );
    let r = c.post("/conversations/heka-new/retire", None).await;
    assert_eq!(r.json, json!({"ok": true, "detail": null}));
    let d = c.get("/conversations/heka-new").await;
    assert_ne!(d["retired_at"], Value::Null);
    assert_eq!(d["memory"].as_str().unwrap().trim(), "Shipped v1."); // readable
    assert_eq!(
        c.post("/conversations/heka-new/retire", None).await["detail"],
        "already retired"
    );
    // listed, with retired_at, so the app can group it; filterable
    let rows = c.get("/conversations").await;
    assert_eq!(
        strings(&rows.json, "slug"),
        vec!["ghost".to_string(), "heka-new".to_string()]
    );
    assert_eq!(
        strings(&c.get("/conversations?retired=true").await.json, "slug"),
        vec!["heka-new".to_string()]
    );
    assert_eq!(
        strings(&c.get("/conversations?retired=false").await.json, "slug"),
        vec!["ghost".to_string()]
    );
    assert!(
        w.conversations
            .resolve("heka new", false)
            .unwrap()
            .is_none()
    ); // out of voice routing
    // the session cannot be switched to a retired agent
    w.state.set_phase(Phase::Passive, None, None);
    c.post("/wake", None).await;
    assert_eq!(
        c.post("/conversations/heka-new/switch", None).await.status,
        409
    );

    // a new agent seeded from the retired one's memory
    let r = c
        .post(
            "/conversations",
            Some(json!({"name": "Heka Workshop", "seed_from": "heka-new"})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["memory"].as_str().unwrap().trim(), "Shipped v1.");
    assert_eq!(r["compactions"], 1);
    let r = c
        .post(
            "/conversations",
            Some(json!({"name": "Nope", "seed_from": "ghost"})),
        )
        .await;
    assert_eq!(r.status, 422);
    assert!(r.detail().contains("private"));
    assert_eq!(
        c.post(
            "/conversations",
            Some(json!({"name": "Nope", "seed_from": "zz"}))
        )
        .await
        .status,
        422
    );
    // seed_from on update replaces the memory as an edit
    c.patch(
        "/conversations/heka-new",
        json!({"memory": "Shipped v1. Then v2."}),
    )
    .await;
    let r = c
        .patch(
            "/conversations/heka-workshop",
            json!({"seed_from": "heka-new"}),
        )
        .await;
    assert_eq!(r["memory"].as_str().unwrap().trim(), "Shipped v1. Then v2.");

    let r = c.post("/conversations/heka-new/rehire", None).await;
    assert_eq!(r["ok"], true);
    assert_eq!(
        c.get("/conversations/heka-new").await["retired_at"],
        Value::Null
    );
    assert_eq!(
        c.post("/conversations/heka-new/rehire", None).await["detail"],
        "not retired"
    );
    assert_eq!(
        w.conversations
            .resolve("heka new", false)
            .unwrap()
            .unwrap()
            .slug,
        "heka-new"
    );
    // retiring the agent the session is with is refused
    c.post("/conversations/heka-new/switch", None).await;
    assert_eq!(
        c.post("/conversations/heka-new/retire", None).await.status,
        409
    );

    let audit = c.get("/audit?source=conversations").await;
    let ops: std::collections::BTreeSet<String> =
        strings(&audit["items"], "operation").into_iter().collect();
    for op in ["retire", "rehire", "create"] {
        assert!(ops.contains(op), "{op}");
    }
}
