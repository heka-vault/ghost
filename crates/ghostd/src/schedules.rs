//! The daemon's side of the schedules (ADR 0047): the store's jobs over the agents Ghost
//! has, the runner over the session's own pieces (`ghost_voice::headless`), the scheduler's
//! task, what is wrong with a schedule or a watch that acts, and `ghostd check`'s lines.
//!
//! Nothing here opens a microphone or a speaker, and nothing waits on a session: a run
//! happens beside one.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use ghost_core::conversations::{Conversations, GHOST};
use ghost_core::folder::{AGENTS_DIR, Folder};
use ghost_core::gate::WriteGate;
use ghost_core::schedules::jobs::{AgentState, Agents};
use ghost_core::schedules::{Jobs, Scheduler, SchedulerParts};
use ghost_core::skills::Skills;
use ghost_core::store::Store;
use ghost_core::waiting::WaitingList;
use ghost_core::watches::{AppReader, WatchRunner};
use ghost_llm::Options;
use ghost_voice::conversation::Reach;
use ghost_voice::headless::{HeadlessParts, HeadlessRunner, ModelFor};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::config::GhostdSettings;
use crate::daemon::Daemon;

/// A run writes for reading, not for a voice: it may say more than a spoken reply.
pub const RUN_MAX_TOKENS: u32 = 4096;

/// What Ghost knows of an agent by its slug, from the agents themselves.
pub fn agents_of(conversations: &Arc<Conversations>) -> Agents {
    let conversations = conversations.clone();
    Arc::new(move |slug: &str| match conversations.get(slug) {
        Ok(Some(p)) if p.retired() => AgentState::Retired,
        Ok(Some(_)) => AgentState::Known,
        _ => AgentState::Unknown,
    })
}

/// The schedules over the daemon's store.
pub fn jobs(store: &Arc<Store>, conversations: &Arc<Conversations>) -> Arc<Jobs> {
    Arc::new(Jobs::new(store.clone()).with_agents(agents_of(conversations)))
}

/// What is wrong with the schedules, and with a watch that acts through an agent Ghost
/// doesn't have, as the waiting list takes Ghost's own trouble: (key, sentence).
pub fn problems(jobs: &Jobs, folder: &Folder) -> Vec<(String, String)> {
    let mut out = jobs.problems();
    for w in folder.watches.values() {
        let Some(act) = &w.act else { continue };
        let said = match jobs.agent_state(&act.agent) {
            AgentState::Known => continue,
            AgentState::Retired => "is retired",
            AgentState::Unknown => "is not an agent Ghost has",
        };
        out.push((
            format!("watch:{}:act", w.name),
            format!(
                "The watch {} acts through {}, which {said}; it will not act",
                w.name, act.agent
            ),
        ));
    }
    out
}

/// The scheduler and its task. `close` stops the task and the runs that are going.
pub struct Running {
    pub scheduler: Arc<Scheduler>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Running {
    pub async fn close(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// The runner: the daemon's agents, gate, reach and model, as a session gets them.
pub fn runner(
    d: &Arc<Daemon>,
    gate: &Arc<WriteGate>,
    conversations: &Arc<Conversations>,
    skills: &Arc<Skills>,
) -> HeadlessRunner {
    let reach: Reach = {
        let weak = Arc::downgrade(d);
        Arc::new(move |p| {
            weak.upgrade()
                .and_then(|d| d.reach())
                .map(|reach| reach(p))
                .unwrap_or_default()
        })
    };
    let model: ModelFor = {
        let weak = Arc::downgrade(d);
        Arc::new(move || {
            let d = weak.upgrade().ok_or("Ghost is stopping")?;
            // As a session gets it: the connection as it is now, its key from the vault.
            (d.ports.model)(&d.voice(), &d.env()).map_err(|e| e.to_string())
        })
    };
    let voice = d.voice();
    HeadlessRunner::new(HeadlessParts {
        conversations: conversations.clone(),
        gate: gate.clone(),
        reach,
        model,
        options: Options {
            max_tokens: voice.llm_max_tokens.max(RUN_MAX_TOKENS),
            thinking: (!voice.llm_thinking.is_empty()).then(|| voice.llm_thinking.clone()),
            cache_system: true,
            effort: None,
        },
        skills: Some(skills.clone()),
    })
}

/// Start the scheduler: what is due runs from now on, a watch that acts starts its runs
/// through it, and what a restart finds half done is told once.
#[allow(clippy::too_many_arguments)]
pub fn start(
    d: &Arc<Daemon>,
    jobs: Arc<Jobs>,
    gate: &Arc<WriteGate>,
    waiting: &Arc<WaitingList>,
    reader: Arc<dyn AppReader>,
    conversations: &Arc<Conversations>,
    skills: &Arc<Skills>,
    watches: &WatchRunner,
) -> Running {
    let scheduler = Scheduler::new(SchedulerParts {
        jobs,
        runner: Arc::new(runner(d, gate, conversations, skills)),
        waiting: waiting.clone(),
        reader: Some(reader),
        conversations: Some(conversations.clone()),
    });
    watches.set_actor(scheduler.clone());
    let stop = d.stop.child_token();
    let task = tokio::spawn(scheduler.clone().run(stop.clone()));
    let n = scheduler.jobs().list().map_or(0, |l| l.len());
    info!(
        "schedules: {n}, times read in {}",
        scheduler.jobs().default_tz()
    );
    Running {
        scheduler,
        stop,
        task,
    }
}

/// `ghostd check` for the schedules and for the watches that act: one line each, and what
/// is wrong (a schedule that does not read, an agent Ghost doesn't have). The database is
/// opened as it is and never written.
pub fn check_schedules(cfg: &GhostdSettings, out: &mut dyn Write) -> Vec<String> {
    let folder = Folder::new(&cfg.config_dir);
    let agents_dir = cfg.config_dir.join(AGENTS_DIR);
    let store = cfg
        .db_path
        .exists()
        .then(|| Store::open_read_only(&cfg.db_path).ok())
        .flatten()
        .map(Arc::new);
    let agents: Agents = {
        let store = store.clone();
        Arc::new(move |slug: &str| known_agent(slug, &agents_dir, store.as_deref()))
    };
    let Some(store) = store.filter(|s| s.has_table("schedules")) else {
        let _ = writeln!(out, "schedules: none");
        let none = Jobs::new(Arc::new(match Store::in_memory() {
            Ok(s) => s,
            Err(e) => return vec![format!("schedules: {e}")],
        }))
        .with_agents(agents);
        return problems(&none, &folder)
            .into_iter()
            .map(|(_, p)| p)
            .collect();
    };
    let jobs = Jobs::new(store).with_agents(agents);
    let listed = jobs.list().unwrap_or_default();
    if listed.is_empty() {
        let _ = writeln!(out, "schedules: none");
    }
    for job in &listed {
        let _ = writeln!(
            out,
            "schedule {}: {}, {}, {}",
            job.name,
            job.agent,
            job.schedule.say(&job.options.tz),
            match (job.enabled, job.next_run_at) {
                (false, _) => "paused".to_string(),
                (true, Some(next)) => format!("next {}", next.format("%Y-%m-%d %H:%M UTC")),
                (true, None) => "no next run".to_string(),
            }
        );
    }
    for w in folder.watches.values() {
        if let Some(act) = &w.act {
            let _ = writeln!(
                out,
                "watch {}: acts through {}, {} a hour at the most",
                w.name, act.agent, act.per_hour
            );
        }
    }
    problems(&jobs, &folder)
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

/// An agent as `ghostd check` can tell without the daemon: Ghost always, one with a folder
/// under `agents/`, or one the database has; retired when the database says so.
fn known_agent(slug: &str, agents_dir: &Path, store: Option<&Store>) -> AgentState {
    let row = store.and_then(|s| s.get_conversation(slug, false).ok().flatten());
    match row {
        Some(p) if p.retired() => AgentState::Retired,
        Some(_) => AgentState::Known,
        None if slug == GHOST || agents_dir.join(slug).is_dir() => AgentState::Known,
        None => AgentState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use ghost_core::schedules::{JobEdit, OPERATOR, Written};
    use serde_json::json;

    use super::*;

    fn settings(tmp: &Path) -> GhostdSettings {
        GhostdSettings {
            data_dir: tmp.join("data"),
            config_dir: tmp.join("config"),
            db_path: tmp.join("data/ghost.db"),
            policy_path: tmp.join("config/policy.toml"),
            // Every path that defaults under the operator's home, in the temp folder.
            catalog_path: tmp.join("cache/catalog.json"),
            skills_dir: tmp.join("config/skills"),
            home_skills_dir: tmp.join("home-skills"),
            api_socket: Some(tmp.join("ghost.sock")),
            settings_path: tmp.join("config/settings.env"),
            env_path: tmp.join("config/env"),
            vault_env_path: tmp.join("no-heka"),
            infra_dir: tmp.join("infra"),
            dictation_flag: tmp.join("rt/dictation"),
            ..GhostdSettings::default()
        }
    }

    fn every(agent: &str, name: &str) -> JobEdit {
        JobEdit {
            name: Some(name.into()),
            agent: Some(agent.into()),
            instruction: Some("Check the build.".into()),
            when: Some(Written {
                every: Some("1h".into()),
                ..Written::default()
            }),
            ..JobEdit::default()
        }
    }

    fn acting_watch(agent: &str) -> serde_json::Value {
        json!({
            "app": "github", "read": "list_workflow_runs", "items": "runs",
            "fields": {"id": {"at": "id", "type": "int"}},
            "key": "{id}", "title": "Run {id}", "response": "act",
            "act": {"agent": agent, "instruction": "Say what broke."},
        })
    }

    #[test]
    fn check_names_a_schedule_that_does_not_read_and_an_agent_ghost_does_not_have() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = settings(tmp.path());
        std::fs::create_dir_all(cfg.config_dir.join("agents/ledger")).unwrap();
        std::fs::create_dir_all(&cfg.data_dir).unwrap();
        // Nothing yet: no database, no watches.
        let mut out = Vec::new();
        assert!(check_schedules(&cfg, &mut out).is_empty());
        assert_eq!(String::from_utf8(out).unwrap(), "schedules: none\n");

        let (good, gone, broken) = {
            let store = Arc::new(Store::open(&cfg.db_path).unwrap());
            let jobs = Jobs::new(store).with_default_tz("UTC");
            let good = jobs
                .create(&every("ledger", "books"), OPERATOR, Utc::now())
                .unwrap();
            let gone = jobs
                .create(&every("intern", "filing"), OPERATOR, Utc::now())
                .unwrap();
            let broken = jobs
                .create(&every("ghost", "broken"), OPERATOR, Utc::now())
                .unwrap();
            jobs.corrupt_for_test(
                &broken.id,
                &json!({"kind": "cron", "expr": "whenever", "tz": "UTC"}),
            );
            (good, gone, broken)
        };
        std::fs::write(
            cfg.config_dir.join("watches.json"),
            json!({"ci": acting_watch("ledger"), "deploys": acting_watch("nobody")}).to_string(),
        )
        .unwrap();
        let before = std::fs::metadata(&cfg.db_path).unwrap().modified().unwrap();
        let mut out = Vec::new();
        let problems = check_schedules(&cfg, &mut out);
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("schedule books: ledger, every 1h, next "),
            "{out}"
        );
        assert!(out.contains("schedule filing: intern, every 1h, next "));
        assert!(!out.contains("schedule broken"));
        assert!(out.contains("watch ci: acts through ledger, 12 a hour at the most"));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("does not read") && p.contains("whenever"))
        );
        assert!(problems.iter().any(|p| p
            == "The schedule filing is for intern, which is not an agent Ghost has; it will not run"));
        assert!(problems.iter().any(|p| p
            == "The watch deploys acts through nobody, which is not an agent Ghost has; it will not act"));
        // The check read the database and left it as it was.
        assert_eq!(
            std::fs::metadata(&cfg.db_path).unwrap().modified().unwrap(),
            before
        );
        let _ = (good, gone, broken);
    }
}
