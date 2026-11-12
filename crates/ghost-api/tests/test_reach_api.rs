//! How much Ghost read and did (ADR 0060): the calls' counts and sizes totalled over a
//! stretch of time, the audit read by date, and a call's counts as an event.

mod common;

use chrono::{Duration, Utc};
use common::World;
use ghost_core::reach::ReachRow;

fn row(session: &str, agent: &str, app: &str, read: bool, items: u64, bytes: u64) -> ReachRow {
    ReachRow {
        at: Utc::now() - Duration::minutes(5),
        session_id: session.into(),
        agent: agent.into(),
        app: app.into(),
        tool: "list_issues".into(),
        read,
        ok: true,
        items: Some(items),
        bytes,
        ms: 120,
    }
}

fn q(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[tokio::test]
async fn reach_totals_a_day_by_app_agent_and_run() {
    let w = World::new();
    for (i, mut r) in [
        row("s1", "ghost", "github", true, 400, 3_000_000),
        row("s1", "ghost", "linear", true, 12, 700_000),
        row("run-r1", "ledger", "github", false, 1, 300),
    ]
    .into_iter()
    .enumerate()
    {
        r.at += Duration::seconds(i as i64);
        w.store.add_reach(&r).unwrap();
    }
    // Yesterday's is not today's.
    let mut old = row("s0", "ghost", "github", true, 9, 9);
    old.at = Utc::now() - Duration::days(2);
    w.store.add_reach(&old).unwrap();

    let c = w.client();
    let since = Utc::now() - Duration::hours(1);
    let got = c.get(&format!("/reach?since={}", q(since))).await;
    assert_eq!(got.status, 200, "{}", got.text);
    assert_eq!(got["total"]["calls"], 3);
    assert_eq!(got["total"]["reads"], 2);
    assert_eq!(got["total"]["items"], 413);
    assert_eq!(got["total"]["bytes"], 3_700_300);
    assert_eq!(
        got["total"]["apps"],
        serde_json::json!(["github", "linear"])
    );
    assert_eq!(got["by_app"][0]["key"], "github");
    // The latest calls themselves, newest first: counts and sizes, for the screen's margin.
    let recent = got["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0]["bytes"], 300);
    let run = got["by_session"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "run-r1")
        .unwrap()
        .clone();
    assert_eq!(run["agent"], "ledger");

    // One run alone.
    let one = c
        .get(&format!("/reach?since={}&sessions=run-r1", q(since)))
        .await;
    assert_eq!(one["total"]["calls"], 1);

    // Bounded: a backwards range, and more than 31 days, are refused.
    let until = since - Duration::hours(1);
    assert_eq!(
        c.get(&format!("/reach?since={}&until={}", q(since), q(until)))
            .await
            .status,
        422
    );
    assert_eq!(
        c.get(&format!("/reach?since={}", q(since - Duration::days(40))))
            .await
            .status,
        422
    );
}

#[tokio::test]
async fn the_audit_reads_a_stretch_of_time() {
    let w = World::new();
    common::pend(&w.gate, "s1").await;
    let c = w.client();
    let now = Utc::now();
    let today = c
        .get(&format!(
            "/audit?since={}&until={}",
            q(now - Duration::hours(1)),
            q(now + Duration::hours(1))
        ))
        .await;
    assert_eq!(today.status, 200, "{}", today.text);
    assert_eq!(today["total"], 1);
    let yesterday = c
        .get(&format!(
            "/audit?since={}&until={}",
            q(now - Duration::days(2)),
            q(now - Duration::days(1))
        ))
        .await;
    assert_eq!(yesterday["total"], 0);
}

#[tokio::test]
async fn a_calls_counts_are_an_event_with_no_contents() {
    let w = World::new();
    let mut rx = w.state.bus.subscribe();
    w.state
        .publish_reach(&row("run-r1", "ledger", "github", true, 37, 2_050_000));
    let text = rx.try_recv().unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "reach");
    assert_eq!(v["reach"]["session_id"], "run-r1");
    assert_eq!(v["reach"]["items"], 37);
    assert_eq!(v["reach"]["bytes"], 2_050_000);
    let keys: Vec<&String> = v["reach"].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 10, "app, tool, counts and times only: {keys:?}");
}
