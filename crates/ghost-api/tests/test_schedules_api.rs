//! The schedules as the operator's desktop reaches them (ADR 0047): list, add, change,
//! pause, resume, remove, run now, and what a schedule's runs said. The runner is a fake
//! that answers at once; no model is reached.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::World;
use ghost_core::schedules::jobs::AgentState;
use ghost_core::schedules::{
    AgentRunner, Ended, Jobs, RunOutcome, RunRequest, Scheduler, SchedulerParts,
};
use ghost_core::waiting::{Items, WaitingList};
use serde_json::{Value, json};

struct Answers(Mutex<Vec<String>>);

#[async_trait]
impl AgentRunner for Answers {
    async fn run(&self, request: RunRequest) -> RunOutcome {
        self.0.lock().unwrap().push(request.instruction);
        RunOutcome {
            answer: "The build is red on main.".into(),
            model_calls: 1,
            ..RunOutcome::ended(Ended::Done)
        }
    }
}

struct Sched {
    world: World,
    scheduler: Arc<Scheduler>,
    asked: Arc<Answers>,
}

fn sched() -> Sched {
    let mut world = World::new();
    let jobs = Arc::new(
        Jobs::new(world.store.clone())
            .with_default_tz("Europe/London")
            .with_agents(Arc::new(|slug: &str| match slug {
                "ghost" | "ledger" => AgentState::Known,
                "old" => AgentState::Retired,
                _ => AgentState::Unknown,
            })),
    );
    let asked = Arc::new(Answers(Mutex::new(Vec::new())));
    let scheduler = Scheduler::new(SchedulerParts {
        jobs,
        runner: asked.clone(),
        waiting: Arc::new(WaitingList::new(Items::new(world.store.clone()))),
        reader: None,
        conversations: None,
    });
    world.schedules = Some(scheduler.clone());
    Sched {
        world,
        scheduler,
        asked,
    }
}

fn morning() -> Value {
    json!({
        "name": "morning check",
        "agent": "ledger",
        "instruction": "Check the build and say if it is red.",
        "cron": "0 9 * * 1-5",
    })
}

#[tokio::test]
async fn a_daemon_without_schedules_says_so() {
    let c = World::new().client();
    let r = c.get("/schedules").await;
    assert_eq!(r.status, 503);
    assert_eq!(r.detail(), "schedules are not running");
    assert_eq!(c.post("/schedules", Some(morning())).await.status, 503);
}

#[tokio::test]
async fn a_schedule_is_added_listed_and_read() {
    let s = sched();
    let c = s.world.client();
    let empty = c.get("/schedules").await;
    assert_eq!(empty.status, 200);
    assert_eq!(
        empty.json,
        json!({"schedules": [], "time_zone": "Europe/London", "problems": []})
    );

    let made = c.post("/schedules", Some(morning())).await;
    assert_eq!(made.status, 200, "{}", made.text);
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!(made["name"], "morning check");
    assert_eq!(made["agent"], "ledger");
    assert_eq!(made["kind"], "cron");
    assert_eq!(made["cron"], "0 9 * * 1-5");
    assert_eq!(made["every"], Value::Null);
    assert_eq!(made["time_zone"], "Europe/London");
    assert_eq!(made["summary"], "0 9 * * 1-5 (Europe/London)");
    assert_eq!(made["created_by"], "operator");
    assert_eq!(made["enabled"], true);
    assert_eq!(made["running"], false);
    assert!(made["next_run"].is_string());
    assert_eq!(made["last_outcome"], Value::Null);
    // The defaults: two hours of grace, ten minutes, thirty turns, failures told.
    assert_eq!(made["grace_minutes"], 120);
    assert_eq!(made["max_runtime_minutes"], 10);
    assert_eq!(made["max_turns"], 30);
    assert_eq!(made["urgency"], "notify");
    assert_eq!(made["notify_failure"], true);
    assert_eq!(made["delete_after_run"], false);

    let listed = c.get("/schedules").await;
    assert_eq!(listed["schedules"].as_array().unwrap().len(), 1);
    assert_eq!(listed["schedules"][0], made.json);
    assert_eq!(c.get(&format!("/schedules/{id}")).await.json, made.json);
    assert_eq!(c.get("/schedules/nope").await.status, 404);

    // The other two kinds, with their options.
    let every = c
        .post(
            "/schedules",
            Some(json!({
                "instruction": "Look at the inbox.", "every": "30m",
                "active_hours": {"start": "08:00", "end": "18:00"},
                "urgency": "digest", "notify_failure": false, "max_turns": 12,
                "check": {"app": "mail", "read": "list_unread", "args": {"folder": "inbox"}},
                "skills": ["triage"],
            })),
        )
        .await;
    assert_eq!(every.status, 200, "{}", every.text);
    assert_eq!(every["kind"], "every");
    assert_eq!(every["every"], "30m");
    assert_eq!(every["agent"], "ghost"); // Ghost when none is named
    assert_eq!(every["name"], "Look at the inbox.");
    assert_eq!(
        every["active_hours"],
        json!({"start": "08:00", "end": "18:00"})
    );
    assert_eq!(every["urgency"], "digest");
    assert_eq!(every["notify_failure"], false);
    assert_eq!(every["max_turns"], 12);
    assert_eq!(
        every["check"],
        json!({"app": "mail", "read": "list_unread", "args": {"folder": "inbox"}})
    );
    assert_eq!(every["skills"], json!(["triage"]));
    let once = c
        .post(
            "/schedules",
            Some(json!({"instruction": "Remind me about the dentist.", "at": "2h"})),
        )
        .await;
    assert_eq!(once["kind"], "at");
    assert!(once["at"].is_string());
    assert_eq!(once["delete_after_run"], true);
    assert_eq!(
        c.get("/schedules").await["schedules"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn what_does_not_hold_is_refused_and_nothing_is_stored() {
    let s = sched();
    let c = s.world.client();
    let refuse = |body: Value, why: &'static str| {
        let c = &c;
        async move {
            let r = c.post("/schedules", Some(body)).await;
            assert_eq!(r.status, 422, "{}", r.text);
            assert!(r.detail().contains(why), "{}", r.detail());
        }
    };
    refuse(json!({"instruction": "x"}), "at, every or cron").await;
    refuse(json!({"every": "1h"}), "instruction").await;
    refuse(
        json!({"instruction": "x", "cron": "at nine"}),
        "not a cron expression",
    )
    .await;
    refuse(
        json!({"instruction": "x", "every": "1h", "at": "20m"}),
        "not two",
    )
    .await;
    refuse(
        json!({"instruction": "x", "every": "5s"}),
        "a minute at the least",
    )
    .await;
    refuse(
        json!({"instruction": "x", "every": "1h", "agent": "intern"}),
        "no agent intern",
    )
    .await;
    refuse(
        json!({"instruction": "x", "every": "1h", "agent": "old"}),
        "retired",
    )
    .await;
    refuse(
        json!({"instruction": "x", "cron": "0 9 * * *", "time_zone": "Mars/Olympus"}),
        "not a time zone",
    )
    .await;
    // A value of the wrong type is where and why, as every route says it.
    let r = c
        .post(
            "/schedules",
            Some(json!({"instruction": "x", "every": "1h", "urgency": "loud"})),
        )
        .await;
    assert_eq!(r.status, 422);
    assert!(
        c.get("/schedules").await["schedules"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_schedule_is_changed_paused_resumed_and_removed() {
    let s = sched();
    let c = s.world.client();
    let made = c.post("/schedules", Some(morning())).await;
    let id = made["id"].as_str().unwrap();
    let path = format!("/schedules/{id}");

    // What is left out stays as it is.
    let edited = c
        .patch(
            &path,
            json!({"name": "build check", "every": "2h", "urgency": "call"}),
        )
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text);
    assert_eq!(edited["name"], "build check");
    assert_eq!(edited["kind"], "every");
    assert_eq!(edited["every"], "2h");
    assert_eq!(edited["urgency"], "call");
    assert_eq!(edited["instruction"], made["instruction"]);
    assert_eq!(edited["agent"], "ledger");
    let hours = c
        .patch(
            &path,
            json!({"active_hours": {"start": "09:00", "end": "17:00"}}),
        )
        .await;
    assert_eq!(
        hours["active_hours"],
        json!({"start": "09:00", "end": "17:00"})
    );
    let cleared = c.patch(&path, json!({"clear_active_hours": true})).await;
    assert_eq!(cleared["active_hours"], Value::Null);
    assert_eq!(c.patch(&path, json!({"cron": "nope"})).await.status, 422);
    assert_eq!(
        c.patch("/schedules/nope", json!({"name": "x"}))
            .await
            .status,
        404
    );

    let paused = c.post(&format!("{path}/pause"), None).await;
    assert_eq!(paused.status, 200);
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["next_run"], Value::Null);
    let resumed = c.post(&format!("{path}/resume"), None).await;
    assert_eq!(resumed["enabled"], true);
    assert!(resumed["next_run"].is_string());
    assert_eq!(c.post("/schedules/nope/pause", None).await.status, 404);

    let gone = c.delete(&path).await;
    assert_eq!(gone.json, json!({"ok": true, "detail": null}));
    assert_eq!(c.delete(&path).await.status, 404);
    assert_eq!(c.get(&path).await.status, 404);
}

#[tokio::test]
async fn run_now_starts_a_run_and_its_runs_say_what_it_answered() {
    let s = sched();
    let c = s.world.client();
    let made = c.post("/schedules", Some(morning())).await;
    let id = made["id"].as_str().unwrap();
    let path = format!("/schedules/{id}");
    assert_eq!(
        c.get(&format!("{path}/runs")).await.json,
        json!({"runs": []})
    );

    let run = c.post(&format!("{path}/run"), None).await;
    assert_eq!(run.status, 200, "{}", run.text);
    assert_eq!(run["schedule"], id);
    assert_eq!(run["source"], "run_now");
    assert_eq!(run["status"], "queued");
    assert_eq!(run["agent"], "ledger");
    // Asked again while it waits: the same run, not a second.
    let again = c.post(&format!("{path}/run"), None).await;
    assert_eq!(again["id"], run["id"]);
    assert_eq!(c.get(&path).await["running"], true);

    s.scheduler.drain().await; // the daemon's ticker, here by hand
    assert_eq!(
        *s.asked.0.lock().unwrap(),
        vec!["Check the build and say if it is red."]
    );
    let runs = c.get(&format!("{path}/runs?limit=5")).await;
    assert_eq!(runs.status, 200);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    let done = &runs["runs"][0];
    assert_eq!(done["id"], run["id"]);
    assert_eq!(done["status"], "ok");
    assert_eq!(done["answer"], "The build is red on main.");
    assert_eq!(done["delivery"], "delivered");
    assert_eq!(done["model_calls"], 1);
    assert_eq!(done["retries"], 0);
    assert!(done["finished_at"].is_string());
    let after = c.get(&path).await;
    assert_eq!(after["last_outcome"], "ok");
    assert_eq!(after["running"], false);
    assert!(after["last_run"].is_string());
    // Every schedule's runs, and a watch's, in one list.
    let all = c.get("/runs").await;
    assert_eq!(all["runs"][0]["id"], run["id"]);

    assert_eq!(c.post("/schedules/nope/run", None).await.status, 404);
    assert_eq!(c.get("/schedules/nope/runs").await.status, 404);
    assert_eq!(c.get(&format!("{path}/runs?limit=many")).await.status, 422);
}

#[tokio::test]
async fn a_waiting_run_is_stopped_and_an_ended_one_is_not() {
    let s = sched();
    let c = s.world.client();
    let made = c.post("/schedules", Some(morning())).await;
    let path = format!("/schedules/{}", made["id"].as_str().unwrap());
    let run = c.post(&format!("{path}/run"), None).await;
    let cancel = format!("/runs/{}/cancel", run["id"].as_str().unwrap());
    assert_eq!(c.post(&cancel, None).await.status, 200);
    s.scheduler.drain().await;
    assert!(s.asked.0.lock().unwrap().is_empty());
    assert_eq!(
        c.get(&format!("{path}/runs")).await["runs"][0]["status"],
        "cancelled"
    );
    assert_eq!(c.post(&cancel, None).await.status, 404);
    assert_eq!(c.post("/runs/nope/cancel", None).await.status, 404);
}

#[tokio::test]
async fn the_list_says_what_is_wrong_with_a_schedule() {
    let s = sched();
    let c = s.world.client();
    let made = c.post("/schedules", Some(morning())).await;
    s.scheduler.jobs().corrupt_for_test(
        made["id"].as_str().unwrap(),
        &json!({"kind": "cron", "expr": "whenever", "tz": "UTC"}),
    );
    let listed = c.get("/schedules").await;
    assert!(listed["schedules"].as_array().unwrap().is_empty());
    let problems = listed["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].as_str().unwrap().contains("does not read"));
}

#[tokio::test]
async fn a_watch_that_acts_says_through_which_agent() {
    let w = World::new();
    std::fs::write(
        w.config.join("watches.json"),
        json!({"ci": {
            "app": "github", "read": "list_workflow_runs", "items": "workflow_runs",
            "fields": {"id": {"at": "id", "type": "int"}},
            "key": "{id}", "title": "CI failed: {id}",
            "response": "act",
            "act": {"agent": "ledger", "instruction": "Say what broke."},
        }})
        .to_string(),
    )
    .unwrap();
    let r = w.client().get("/watches").await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["problems"], json!([]));
    assert_eq!(r["watches"][0]["acts_through"], "ledger");
    assert_eq!(r["watches"][0]["response"], "record");
}
