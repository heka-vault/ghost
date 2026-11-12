//! Questions in the desktop (ADR 0051): listed with who asked and the choices, answered
//! in the operator's words or with a choice (route `desktop`), declined; a change waiting
//! for a yes is the question Yes or No, and Yes is the same Confirm as ever. A temp store.

mod common;

use std::sync::Arc;

use common::{World, pend};
use ghost_core::questions::{Ask, Questions};
use ghost_core::waiting::{Items, WaitingList};
use serde_json::{Value, json};

fn ask(w: &World, text: &str, choices: &[&str]) -> String {
    let waiting = Arc::new(WaitingList::new(Items::new(w.store.clone())));
    let qs = Questions::new(w.store.clone(), waiting);
    let mut a = Ask::new("agent:ledger", "Ledger", text);
    a.agent = Some("ledger".into());
    a.choices = choices.iter().map(|c| c.to_string()).collect();
    qs.ask(a, None).unwrap().key
}

#[tokio::test]
async fn a_question_is_listed_and_answered_with_a_choice_from_the_desktop() {
    let w = World::new();
    let c = w.client();
    let key = ask(
        &w,
        "Should the October invoice go out today",
        &["Yes", "No"],
    );
    let listed = c.get("/questions").await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    let q = &listed["questions"][0];
    assert_eq!(q["key"], key.as_str());
    assert_eq!(q["kind"], "question");
    assert_eq!(q["asked_by"], "Ledger");
    assert_eq!(q["choices"], json!(["Yes", "No"]));
    assert_eq!(q["state"], "open");
    // The waiting list carries it as a question too.
    let waiting = c.get("/waiting").await;
    assert_eq!(waiting["items"][0]["question"]["key"], key.as_str());

    let out = c
        .post(
            "/questions/answer",
            Some(json!({"key": key, "text": "Yes"})),
        )
        .await;
    assert_eq!(out.status, 200, "{}", out.text);
    assert_eq!(out["question"]["state"], "answered");
    assert_eq!(out["question"]["answer"], "Yes");
    assert_eq!(out["question"]["choice"], "Yes");
    assert_eq!(out["question"]["answered_via"], "desktop");
    // Answering is the resolution: it has left the waiting list.
    assert_eq!(c.get("/waiting").await["items"], json!([]));
    // Settled, it is still listed for a day, and cannot be answered again.
    assert_eq!(
        c.get("/questions").await["questions"][0]["state"],
        "answered"
    );
    let again = c
        .post("/questions/answer", Some(json!({"key": key, "text": "No"})))
        .await;
    assert_eq!(again.status, 409, "{}", again.text);
}

#[tokio::test]
async fn a_question_answered_in_words_and_one_declined() {
    let w = World::new();
    let c = w.client();
    let free = ask(&w, "Which account should it go from?", &[]);
    let out = c
        .post(
            "/questions/answer",
            Some(json!({"key": free, "text": "the business one, please"})),
        )
        .await;
    assert_eq!(out["question"]["answer"], "the business one, please");
    assert_eq!(out["question"]["choice"], Value::Null);
    let no = ask(&w, "Ship it?", &["Yes", "No"]);
    let out = c.post("/questions/decline", Some(json!({"key": no}))).await;
    assert_eq!(out.status, 200, "{}", out.text);
    assert_eq!(out["question"]["state"], "declined");
    assert_eq!(out["question"]["answer"], Value::Null);
    assert_eq!(
        c.post("/questions/decline", Some(json!({"key": "question:nope"})))
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn a_change_waiting_for_a_yes_is_a_question_and_yes_is_the_confirm() {
    let w = World::new();
    let c = w.client();
    let sub = pend(&w.gate, "sess1").await;
    w.gate.set_aside("sess1", "hung up").unwrap();
    let listed = c.get("/questions").await;
    let q = &listed["questions"][0];
    assert_eq!(q["kind"], "change");
    assert_eq!(q["choices"], json!(["Yes", "No"]));
    assert_eq!(q["change_id"], sub.proposal.id.as_str());
    let key = q["key"].as_str().unwrap().to_string();
    // Neither yes nor no is no answer to a change.
    let maybe = c
        .post(
            "/questions/answer",
            Some(json!({"key": key, "text": "maybe"})),
        )
        .await;
    assert_eq!(maybe.status, 422);
    let out = c
        .post(
            "/questions/answer",
            Some(json!({"key": key, "text": "Yes"})),
        )
        .await;
    assert_eq!(out.status, 200, "{}", out.text);
    assert_eq!(out["outcome"]["status"], "success");
    let entry = c.get("/audit").await["items"][0].clone();
    assert_eq!(entry["status"], "executed");
    let said: Vec<Value> = entry["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["event"] == "user_turn")
        .cloned()
        .collect();
    assert_eq!(said[0]["detail"]["classifier"], "ui");
    // And No cancels another.
    let other = pend(&w.gate, "sess2").await;
    w.gate.set_aside("sess2", "hung up").unwrap();
    let out = c
        .post(
            "/questions/answer",
            Some(json!({"key": format!("change:{}", other.proposal.id), "text": "no"})),
        )
        .await;
    assert_eq!(out["ok"], true);
    assert!(
        c.get("/questions").await["questions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
