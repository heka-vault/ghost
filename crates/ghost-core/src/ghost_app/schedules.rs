//! Schedules on Ghost's own app (ADR 0047): `schedules_list`, a read, and
//! `schedule_manage`, a change through the gate, so an agent told "check the build every
//! morning at nine and tell me if it is red" can set that up, and "stop that" can stop it.
//!
//! As with `notify`, the calling agent is never an argument the model writes: the
//! dispatcher sets `_agent`. A schedule an agent makes is attributed to it and runs as it;
//! an agent manages its own, and Ghost (agent zero) manages all. The tools are on the
//! server only when the daemon gives it the schedules ([`GhostServer::with_schedules`]).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{GhostServer, reply};
use crate::conversations::GHOST;
use crate::schedules::when::parse_tz;
use crate::schedules::{Check, Job, JobEdit, Jobs, Run, Written, by_agent, clip};
use crate::time::{iso, utcnow};

/// What `schedules_list` shows of a run's answer.
pub const ANSWER_SHOWN: usize = 1200;
pub const RUNS_SHOWN: i64 = 5;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SchedulesList {
    /// A schedule's id or name, for its last runs and what they said; leave out for all.
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ScheduleManage {
    /// create, update, pause, resume, remove or run_now.
    pub action: String,
    /// The schedule's id or name; for every action but create.
    #[serde(default)]
    pub id: Option<String>,
    /// A few words to call it by.
    #[serde(default)]
    pub name: Option<String>,
    /// What the agent is to do each time, written to stand alone.
    #[serde(default)]
    pub instruction: Option<String>,
    /// Once: 2026-11-12T09:00, or a delay such as 20m.
    #[serde(default)]
    pub at: Option<String>,
    /// An interval: 30m, 2h, 1d.
    #[serde(default)]
    pub every: Option<String>,
    /// Five cron fields: 0 9 * * 1-5.
    #[serde(default)]
    pub cron: Option<String>,
    /// A time zone such as Europe/London; the machine's when left out.
    #[serde(default)]
    pub tz: Option<String>,
    /// Ghost only: the agent that does it. Any other agent schedules itself.
    #[serde(default)]
    pub agent: Option<String>,
    /// Only between these hours: 08:00-18:00. Empty to clear.
    #[serde(default)]
    pub active_hours: Option<String>,
    /// digest, notify (the default) or call: how loud the answer may get.
    #[serde(default)]
    pub urgency: Option<String>,
    /// Look first, with no model: a connected app whose read-only tool is called, and
    /// the agent runs only when what it returns has changed.
    #[serde(default)]
    pub check_app: Option<String>,
    #[serde(default)]
    pub check_read: Option<String>,
    #[serde(default)]
    pub check_args: Option<Map<String, Value>>,
    /// Skills to read first.
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

fn local(when: Option<DateTime<Utc>>, tz: &str) -> Value {
    match (when, parse_tz(tz)) {
        (Some(t), Ok(zone)) => json!(format!(
            "{} {tz}",
            t.with_timezone(&zone).format("%Y-%m-%d %H:%M")
        )),
        (Some(t), Err(_)) => json!(iso(&t)),
        (None, _) => Value::Null,
    }
}

fn job_view(job: &Job) -> Value {
    let mut out = json!({
        "id": job.id,
        "name": job.name,
        "agent": job.agent,
        "when": job.schedule.say(&job.options.tz),
        "state": if job.enabled { "on" } else { "paused" },
        "next_run": local(job.next_run_at.filter(|_| job.enabled), &job.options.tz),
        "last_run": local(job.last_run_at, &job.options.tz),
        "last_outcome": job.last_outcome,
        "made_by": job.created_by,
    });
    if let Some(hours) = &job.options.active_hours {
        out["active_hours"] = json!(format!("{}-{}", hours.start, hours.end));
    }
    if let Some(check) = &job.options.check {
        out["check"] = json!(format!("{}.{}", check.app, check.read));
    }
    out
}

fn run_view(run: &Run, tz: &str) -> Value {
    json!({
        "when": local(Some(run.scheduled_for), tz),
        "outcome": run.status.as_str(),
        "told": run.delivery,
        "answer": run.answer.as_deref().map(|a| clip(a, ANSWER_SHOWN)),
        "error": run.error,
    })
}

fn given(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl GhostServer {
    /// The same server with the schedules, and the two tools that go with them.
    pub fn with_schedules(mut self, jobs: Arc<Jobs>) -> Self {
        self.schedules = Some(jobs);
        self.tool_router.merge(Self::schedules_router());
        self
    }

    fn jobs(&self) -> Result<&Arc<Jobs>, String> {
        self.schedules
            .as_ref()
            .ok_or_else(|| "schedules are not available here".to_string())
    }

    fn do_schedules_list(&self, p: &SchedulesList) -> Result<Value, String> {
        let jobs = self.jobs()?;
        if let Some(id) = given(&p.id) {
            let job = jobs
                .find(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("there is no schedule {id}"))?;
            let runs = jobs.runs(&job.id, RUNS_SHOWN).map_err(|e| e.to_string())?;
            let mut out = job_view(&job);
            out["instruction"] = json!(job.instruction);
            out["runs"] = json!(
                runs.iter()
                    .map(|r| run_view(r, &job.options.tz))
                    .collect::<Vec<_>>()
            );
            return Ok(out);
        }
        let all = jobs.list().map_err(|e| e.to_string())?;
        Ok(json!({
            "schedules": all.iter().map(job_view).collect::<Vec<_>>(),
            "now": local(Some(utcnow()), jobs.default_tz()),
        }))
    }

    fn edit_from(p: &ScheduleManage) -> Result<JobEdit, String> {
        let when = (p.at.is_some() || p.every.is_some() || p.cron.is_some() || p.tz.is_some())
            .then(|| Written {
                at: p.at.clone(),
                every: p.every.clone(),
                cron: p.cron.clone(),
                tz: p.tz.clone(),
            });
        let active_hours = match p.active_hours.as_deref().map(str::trim) {
            None => None,
            Some("") => Some(None),
            Some(hours) => {
                let (start, end) = hours
                    .split_once('-')
                    .ok_or("active_hours is a start and an end: 08:00-18:00")?;
                Some(Some((start.trim().to_string(), end.trim().to_string())))
            }
        };
        let check = match (given(&p.check_app), given(&p.check_read)) {
            (Some(app), Some(read)) => Some(Some(Check {
                app: app.to_string(),
                read: read.to_string(),
                args: p.check_args.clone().unwrap_or_default(),
            })),
            (None, None) => None,
            _ => return Err("a check names both check_app and check_read".into()),
        };
        Ok(JobEdit {
            name: p.name.clone(),
            agent: None,
            instruction: p.instruction.clone(),
            when,
            active_hours,
            urgency: p.urgency.clone(),
            check,
            skills: p.skills.clone(),
            ..JobEdit::default()
        })
    }

    fn do_schedule_manage(&self, p: ScheduleManage) -> Result<Value, String> {
        let jobs = self.jobs()?;
        let caller = self.agent(p.caller.clone())?;
        let now = utcnow();
        let action = p.action.trim().to_lowercase();
        let mut edit = Self::edit_from(&p)?;
        // Who it runs as: Ghost may name any agent; anyone else schedules itself.
        let named = given(&p.agent);
        if caller != GHOST && named.is_some_and(|a| a != caller) {
            return Err(
                "you make schedules for yourself; Ghost makes them for other agents".into(),
            );
        }
        if action == "create" {
            edit.agent = Some(named.unwrap_or(&caller).to_string());
            let job = jobs
                .create(&edit, &by_agent(&caller), now)
                .map_err(|e| e.to_string())?;
            let mut out = job_view(&job);
            out["status"] = json!("created");
            return Ok(out);
        }
        let wanted = given(&p.id).ok_or("say which schedule: its id or its name")?;
        let job = jobs
            .find(wanted)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no schedule {wanted}; see ghost_schedules_list"))?;
        if !job.managed_by(&caller) {
            return Err(format!(
                "{} is {}'s schedule; an agent manages only its own",
                job.name, job.agent
            ));
        }
        let (job, status) = match action.as_str() {
            "update" => {
                edit.agent = named.map(String::from);
                (
                    jobs.update(&job.id, &edit, now)
                        .map_err(|e| e.to_string())?,
                    "updated",
                )
            }
            "pause" => (
                jobs.set_enabled(&job.id, false, now)
                    .map_err(|e| e.to_string())?,
                "paused",
            ),
            "resume" => (
                jobs.set_enabled(&job.id, true, now)
                    .map_err(|e| e.to_string())?,
                "resumed",
            ),
            "remove" => {
                jobs.remove(&job.id).map_err(|e| e.to_string())?;
                return Ok(json!({"id": job.id, "name": job.name, "status": "removed"}));
            }
            "run_now" => {
                let run = jobs.request_run(&job.id, now).map_err(|e| e.to_string())?;
                return Ok(json!({
                    "id": job.id,
                    "name": job.name,
                    "run": run.id,
                    "status": "started; its answer goes to the operator's waiting list",
                }));
            }
            other => {
                return Err(format!(
                    "action is create, update, pause, resume, remove or run_now, not {other}"
                ));
            }
        };
        let mut out = job_view(&job);
        out["status"] = json!(status);
        Ok(out)
    }
}

#[tool_router(router = schedules_router)]
impl GhostServer {
    #[tool(
        description = "The schedules: what each agent does on its own, when it runs next and how the last run ended. With an id or a name, that schedule's instruction and what its last runs said.",
        annotations(read_only_hint = true)
    )]
    fn schedules_list(&self, Parameters(p): Parameters<SchedulesList>) -> CallToolResult {
        reply(self.do_schedules_list(&p))
    }

    #[tool(
        description = "Set up, change or stop a schedule: an instruction an agent carries out on its own, with nobody there, whose answer goes to the operator's waiting list. When is exactly one of at (once: 2026-11-12T09:00, or a delay such as 20m), every (30m, 2h, 1d) or cron (five fields: 0 9 * * 1-5), in tz when it is not the machine's. The instruction is all a run is given: write it to stand alone, and say when there is nothing to tell, so the run answers [SILENT]."
    )]
    fn schedule_manage(&self, Parameters(p): Parameters<ScheduleManage>) -> CallToolResult {
        reply(self.do_schedule_manage(p))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::apps::{Apps, CALLER_ARG};
    use crate::gate::{Request, Submission, WriteGate};
    use crate::ghost_app::{Attached, Caller, NAME, Parts, attach};
    use crate::levels::Autonomy;
    use crate::models::OutcomeStatus;
    use crate::policy::AutonomyPolicy;
    use crate::schedules::jobs::AgentState;
    use crate::schedules::{OPERATOR, RunStatus};
    use crate::store::Store;
    use crate::waiting::{Items, WaitingList};

    struct World {
        apps: Arc<Apps>,
        jobs: Arc<Jobs>,
        server: GhostServer,
        gate: WriteGate,
        _attached: Attached,
    }

    async fn world(policy: &str) -> World {
        let store = Arc::new(Store::in_memory().unwrap());
        let apps = Arc::new(Apps::new(
            Vec::new(),
            Arc::new(HashMap::<String, String>::new()),
            None,
        ));
        let jobs = Arc::new(
            Jobs::new(store.clone())
                .with_default_tz("America/Detroit")
                .with_agents(Arc::new(|slug: &str| match slug {
                    "ghost" | "ledger" | "brand" => AgentState::Known,
                    _ => AgentState::Unknown,
                })),
        );
        let server = GhostServer::new(Parts {
            store: store.clone(),
            waiting: Arc::new(WaitingList::new(Items::new(store.clone()))),
            apps: Arc::downgrade(&apps),
            status: Arc::new(Map::new),
        })
        .with_schedules(jobs.clone());
        let attached = attach(&apps, server.clone()).await.unwrap();
        let gate =
            WriteGate::new(store).with_autonomy(AutonomyPolicy::parse(policy, "<test>").unwrap());
        gate.register(NAME, attached.app.operations());
        World {
            apps,
            jobs,
            server,
            gate,
            _attached: attached,
        }
    }

    /// The change as the dispatcher submits it: the calling agent in `_agent`.
    async fn manage(w: &World, agent: &str, args: Value) -> Submission {
        let op = w.gate.operation(NAME, "schedule_manage").unwrap();
        let mut args = args.as_object().cloned().unwrap();
        args.insert(CALLER_ARG.into(), json!(agent));
        let prepared = (op.prepare)(args).await.unwrap();
        let mut req = Request::new(
            "s",
            NAME,
            "schedule_manage",
            prepared.target,
            prepared.changes,
            "test",
        );
        req.conversation = Some(agent.to_string());
        req.asked_by = prepared.asked_by;
        w.gate.submit(req).await.unwrap()
    }

    async fn list(w: &World, args: Value) -> Map<String, Value> {
        let src = w
            .apps
            .sources()
            .into_iter()
            .find(|s| s.name == NAME)
            .unwrap();
        let t = src
            .read_tools
            .iter()
            .find(|t| t.name == "ghost_schedules_list")
            .expect("the read");
        (t.handler)(args.as_object().cloned().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_tools_are_a_read_and_a_change_and_only_with_the_schedules() {
        let w = world("").await;
        let src = w
            .apps
            .sources()
            .into_iter()
            .find(|s| s.name == NAME)
            .unwrap();
        assert!(
            src.read_tools
                .iter()
                .any(|t| t.name == "ghost_schedules_list")
        );
        let op = src
            .operations
            .iter()
            .find(|o| o.tool_name.as_deref() == Some("ghost_schedule_manage"))
            .expect("the change");
        // The caller is never an argument the model is shown; the three kinds are.
        assert!(op.properties.get(CALLER_ARG).is_none());
        for arg in ["action", "at", "every", "cron", "tz", "instruction"] {
            assert!(op.properties.get(arg).is_some(), "{arg}");
        }
        // The description says what forms are taken.
        for said in [
            "2026-11-12T09:00",
            "20m",
            "30m, 2h, 1d",
            "0 9 * * 1-5",
            "[SILENT]",
        ] {
            assert!(op.description.contains(said), "{said}");
        }
        // A server without the schedules has neither tool.
        let bare = GhostServer::new(Parts {
            store: Arc::new(Store::in_memory().unwrap()),
            waiting: Arc::new(WaitingList::new(Items::new(Arc::new(
                Store::in_memory().unwrap(),
            )))),
            apps: std::sync::Weak::new(),
            status: Arc::new(Map::new),
        });
        assert!(!bare.tools().iter().any(|t| t.name.contains("schedule")));
        assert_eq!(w.server.tools().len(), bare.tools().len() + 2);
    }

    #[tokio::test]
    async fn an_agent_sets_up_a_schedule_and_it_is_attributed_to_it() {
        let w = world("").await;
        let sub = manage(
            &w,
            "ledger",
            json!({
                "action": "create", "name": "morning build check",
                "instruction": "Check the build on main. If it is red say which job failed; if it is green answer [SILENT].",
                "cron": "0 9 * * *",
            }),
        )
        .await;
        // What no rule names runs quietly through the gate, and is in the audit.
        assert_eq!(sub.autonomy, Autonomy::Quiet);
        assert_eq!(sub.outcome.as_ref().unwrap().status, OutcomeStatus::Success);
        assert_eq!(sub.proposal.conversation.as_deref(), Some("ledger"));
        let jobs = w.jobs.list().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].agent, "ledger");
        assert_eq!(jobs[0].created_by, "agent:ledger");
        assert_eq!(jobs[0].schedule.say("UTC"), "0 9 * * * (America/Detroit)");

        let listed = list(&w, json!({})).await;
        let row = &listed["schedules"][0];
        assert_eq!(row["name"], "morning build check");
        assert_eq!(row["agent"], "ledger");
        assert_eq!(row["state"], "on");
        assert_eq!(row["made_by"], "agent:ledger");
        assert!(
            row["next_run"]
                .as_str()
                .unwrap()
                .ends_with("09:00 America/Detroit")
        );

        // "Stop that": by name, by the agent that made it.
        let sub = manage(
            &w,
            "ledger",
            json!({"action": "pause", "id": "Morning build check"}),
        )
        .await;
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        assert!(!w.jobs.list().unwrap()[0].enabled);
        let one = list(&w, json!({"id": jobs[0].id})).await;
        assert_eq!(one["state"], "paused");
        assert!(one["instruction"].as_str().unwrap().contains("[SILENT]"));
        assert_eq!(one["runs"], json!([]));
    }

    #[tokio::test]
    async fn an_agent_cannot_manage_another_agents_schedule_and_ghost_can() {
        let w = world("").await;
        let job = w
            .jobs
            .create(
                &JobEdit {
                    name: Some("books".into()),
                    agent: Some("ledger".into()),
                    instruction: Some("Reconcile the week.".into()),
                    when: Some(Written {
                        every: Some("1d".into()),
                        ..Written::default()
                    }),
                    ..JobEdit::default()
                },
                OPERATOR,
                utcnow(),
            )
            .unwrap();
        for action in ["pause", "remove", "run_now", "update"] {
            let sub = manage(
                &w,
                "brand",
                json!({"action": action, "id": job.id, "instruction": "Do my bidding."}),
            )
            .await;
            let outcome = sub.outcome.unwrap();
            assert_eq!(outcome.status, OutcomeStatus::Failed, "{action}");
            assert!(
                outcome
                    .error
                    .unwrap()
                    .contains("is ledger's schedule; an agent manages only its own"),
                "{action}"
            );
        }
        let still = w.jobs.get(&job.id).unwrap().unwrap();
        assert!(still.enabled);
        assert_eq!(still.instruction, "Reconcile the week.");
        assert!(w.jobs.queued().unwrap().is_empty());
        // Nor make one that runs as somebody else.
        let sub = manage(
            &w,
            "brand",
            json!({"action": "create", "agent": "ledger", "instruction": "x", "every": "1h"}),
        )
        .await;
        assert!(sub.outcome.unwrap().error.unwrap().contains("for yourself"));
        assert_eq!(w.jobs.list().unwrap().len(), 1);
        // The model's own word about who it is counts for nothing: with no `_agent` the
        // call is refused.
        let nobody = w
            .server
            .do_schedule_manage(ScheduleManage {
                action: "remove".into(),
                id: Some(job.id.clone()),
                ..ScheduleManage::default()
            })
            .unwrap_err();
        assert!(nobody.contains("which agent is calling"));
        // And a caller a listener authenticated is that agent, whatever the arguments say.
        let outside = w.server.for_caller(Caller::Named("brand".into()));
        let refused = outside
            .do_schedule_manage(ScheduleManage {
                action: "remove".into(),
                id: Some(job.id.clone()),
                caller: Some("ledger".into()),
                ..ScheduleManage::default()
            })
            .unwrap_err();
        assert!(refused.contains("manages only its own"));

        // The agent it runs as may; Ghost may manage any, and make one for any agent.
        let sub = manage(&w, "ledger", json!({"action": "run_now", "id": "books"})).await;
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        assert_eq!(w.jobs.queued().unwrap().len(), 1);
        assert_eq!(w.jobs.queued().unwrap()[0].status, RunStatus::Queued);
        let sub = manage(
            &w,
            "ghost",
            json!({"action": "update", "id": job.id, "every": "2h", "urgency": "digest"}),
        )
        .await;
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        assert_eq!(
            w.jobs.get(&job.id).unwrap().unwrap().options.urgency,
            "digest"
        );
        let sub = manage(
            &w,
            "ghost",
            json!({"action": "create", "agent": "brand", "instruction": "Post the weekly note.", "at": "2h"}),
        )
        .await;
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        let made = w.jobs.find("Post the weekly note.").unwrap().unwrap();
        assert_eq!(
            (made.agent.as_str(), made.created_by.as_str()),
            ("brand", "agent:ghost")
        );
        let sub = manage(&w, "ghost", json!({"action": "remove", "id": job.id})).await;
        assert_eq!(sub.outcome.unwrap().status, OutcomeStatus::Success);
        assert!(w.jobs.get(&job.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn what_does_not_read_is_said_and_the_operators_rules_apply() {
        let w = world("[permissions]\nask = [\"ghost.schedule_manage\"]\n").await;
        // Asked about, like any change the operator says to ask about: nothing is made.
        let sub = manage(
            &w,
            "ghost",
            json!({"action": "create", "instruction": "x", "every": "1h"}),
        )
        .await;
        assert_eq!(sub.autonomy, Autonomy::Confirm);
        assert!(sub.outcome.is_none());
        assert!(w.jobs.list().unwrap().is_empty());

        let said = |p: ScheduleManage| {
            w.server
                .do_schedule_manage(ScheduleManage {
                    caller: Some("ghost".into()),
                    ..p
                })
                .unwrap_err()
        };
        let create = || ScheduleManage {
            action: "create".into(),
            instruction: Some("Check the build.".into()),
            ..ScheduleManage::default()
        };
        assert!(said(create()).contains("at, every or cron"));
        assert!(
            said(ScheduleManage {
                cron: Some("at nine".into()),
                ..create()
            })
            .contains("not a cron expression")
        );
        assert!(
            said(ScheduleManage {
                every: Some("1h".into()),
                at: Some("20m".into()),
                ..create()
            })
            .contains("not two")
        );
        assert!(
            said(ScheduleManage {
                every: Some("1h".into()),
                agent: Some("intern".into()),
                ..create()
            })
            .contains("no agent intern")
        );
        assert!(
            said(ScheduleManage {
                every: Some("1h".into()),
                check_app: Some("github".into()),
                ..create()
            })
            .contains("check_app and check_read")
        );
        assert!(
            said(ScheduleManage {
                action: "explode".into(),
                id: Some("x".into()),
                ..ScheduleManage::default()
            })
            .contains("no schedule x")
        );
        assert!(
            said(ScheduleManage {
                action: "pause".into(),
                ..ScheduleManage::default()
            })
            .contains("which schedule")
        );
    }
}
