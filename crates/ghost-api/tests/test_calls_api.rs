//! What a session did between its turns (ADR 0058): the stored activity read back by
//! session, and what one call returned, kept in memory for the live session's last
//! calls, redacted and cut. A temp store.

mod common;

use common::World;
use ghost_api::Phase;
use ghost_core::store::ActivityRow;
use serde_json::json;

#[tokio::test]
async fn a_sessions_stored_activity_is_read_back_by_name_only() {
    let w = World::new();
    let now = chrono::Utc::now();
    for (kind, id) in [("call", "c1"), ("result", "c1"), ("cut", "")] {
        w.store
            .add_activity(&ActivityRow {
                session_id: "s1".into(),
                kind: kind.into(),
                at: now,
                name: (!id.is_empty()).then(|| "github_issue_read".into()),
                call_id: (!id.is_empty()).then(|| id.into()),
                names: Vec::new(),
                secs: None,
            })
            .unwrap();
    }
    let c = w.client();
    let got = c.get("/activity?sessions=s1,s2").await;
    assert_eq!(got.status, 200, "{}", got.text);
    let kinds: Vec<&str> = got
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["call", "result", "cut"]);
    assert_eq!(got[0]["name"], "github_issue_read");
    assert_eq!(got[0]["id"], "c1");
    assert_eq!(got[0]["session_id"], "s1");
    let many: Vec<String> = (0..41).map(|i| format!("s{i}")).collect();
    assert_eq!(
        c.get(&format!("/activity?sessions={}", many.join(",")))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn a_calls_result_is_kept_for_the_live_session_cut_and_redacted() {
    let w = World::new();
    // Outside a session nothing is kept.
    w.state
        .keep_call("c0", "github_issue_read", true, &json!({"title": "x"}));
    let c = w.client();
    assert_eq!(c.get("/calls/c0").await.status, 404);

    w.state.set_phase(Phase::Listening, Some("s1"), None);
    // A password that reached a result anyway never reaches the screen.
    ghost_core::redact::also_redact(
        "test-calls",
        [("CALLS_TOKEN".to_string(), "sk-calls-0123456789".to_string())],
    );
    let long = "a".repeat(5_000);
    w.state.keep_call(
        "c1",
        "github_issue_read",
        true,
        &json!({"a_title": "Login loop", "b_token": "sk-calls-0123456789", "c_body": long}),
    );
    let got = c.get("/calls/c1").await;
    assert_eq!(got.status, 200, "{}", got.text);
    assert_eq!(got["name"], "github_issue_read");
    assert_eq!(got["read"], true);
    assert_eq!(got["session_id"], "s1");
    let preview = got["preview"].as_str().unwrap();
    assert_eq!(preview.chars().count(), 2_000);
    assert!(preview.contains("Login loop"));
    assert!(!preview.contains("sk-calls-0123456789"), "{preview}");
    assert!(got["dropped"].as_u64().unwrap() > 3_000);
}

#[tokio::test]
async fn only_the_last_calls_are_kept() {
    let w = World::new();
    w.state.set_phase(Phase::Listening, Some("s1"), None);
    for i in 0..=ghost_api::state::CALLS_KEPT {
        w.state
            .keep_call(&format!("c{i}"), "ghost_status", true, &json!({"ok": true}));
    }
    let c = w.client();
    assert_eq!(c.get("/calls/c0").await.status, 404, "the oldest went");
    assert_eq!(
        c.get(&format!("/calls/c{}", ghost_api::state::CALLS_KEPT))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn the_closed_turn_and_the_words_heard_are_events_of_a_session_only() {
    let w = World::new();
    let mut rx = w.state.bus.subscribe();
    // No session: nothing, so nothing passive can ever go out this way (ADR 0013).
    w.state.publish_heard(Some("hey ghost what time is it"));
    w.state.publish_saying(0, 0, 3);
    w.state.set_phase(Phase::Listening, Some("s1"), None);
    w.state
        .publish_heard(Some("  remind me tomorrow to review it "));
    w.state.publish_heard(None);
    w.state.publish_heard(Some("   ")); // nothing said: nothing sent
    w.state.publish_saying(2, 3, 9);
    let mut got = Vec::new();
    while let Ok(text) = rx.try_recv() {
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        if matches!(v["type"].as_str(), Some("heard" | "saying")) {
            got.push(v);
        }
    }
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[0]["heard"]["text"], "remind me tomorrow to review it");
    assert_eq!(got[0]["heard"]["reopened"], false);
    assert_eq!(got[0]["heard"]["session_id"], "s1");
    assert_eq!(got[1]["heard"]["reopened"], true);
    assert_eq!(got[1]["heard"]["text"], "");
    assert_eq!(
        got[2]["saying"],
        json!({"session_id": "s1", "reply": 2, "from": 3, "to": 9})
    );
}
