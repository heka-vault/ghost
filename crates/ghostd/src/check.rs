//! `ghostd check`: what this build is, the vault, the model connection, the policy,
//! Ghost's folder (the watches and hooks, each hook's fixtures and password, what an
//! older layout left; ADR 0035), the apps as they connect, the skills and the
//! confirmation classifier. Prints what it finds; exits 1 on a problem. The same
//! describers feed the daemon's log at start and Ghost's own items on the waiting list
//! (`ghost_problems`, ADR 0029).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_core::apps::{APPS_FILE, AppSpec, Apps, load_apps};
use ghost_core::connection::ModelConnection;
use ghost_core::folder::Folder;
use ghost_core::hooks::{ItemsFactory, Scheme, Secrets, run_fixtures};
use ghost_core::jev::{JevClient, JevOptions};
use ghost_core::passwords::{Keyring, SharedEnviron};
use ghost_core::policy::AutonomyPolicy;
use ghost_core::routes::RoutePolicy;
use ghost_core::signin::{SignIns, auth_for_apps};
use ghost_core::skills::Skills;
use ghost_core::waiting::Items;
use ghost_core::watches::WaitingItems;

use crate::build_info;
use crate::config::GhostdSettings;
use crate::passwords::{KEYRING_WAITS, check_passwords, keyring_waits, open_passwords};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Each watch's and hook's declared urgency, by its items' source.
pub fn declared_urgency(folder: &Folder) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = folder
        .watches
        .values()
        .map(|w| (w.source(), w.urgency.clone()))
        .collect();
    out.extend(
        folder
            .hooks
            .values()
            .map(|h| (h.source(), h.urgency.clone())),
    );
    out
}

pub fn describe_routes(p: &RoutePolicy) -> String {
    let quiet = p.quiet_hours.map_or(String::new(), |(from, to)| {
        format!(", quiet {}-{}", from.format("%H:%M"), to.format("%H:%M"))
    });
    let enabled: Vec<&str> = p.enabled.iter().map(|r| r.as_str()).collect();
    format!(
        "routes: {}; escalate after {} min{quiet}",
        enabled.join(", "),
        p.escalate_after
    )
}

/// One line per watch for the log and `ghostd check`.
pub fn describe_watches(folder: &Folder) -> Vec<String> {
    folder
        .watches
        .values()
        .map(|w| {
            let mut steps = "check".to_string();
            if w.jev.is_some() {
                steps.push_str(", jev");
            }
            if w.model.is_some() {
                steps.push_str(", model");
            }
            format!(
                "watch {}: {}.{} every {}s, {steps}, {}",
                w.name,
                w.app,
                w.read,
                w.every,
                w.response.as_str()
            )
        })
        .collect()
}

fn scheme(s: Scheme) -> &'static str {
    match s {
        Scheme::Github => "github",
        Scheme::Hmac => "hmac",
    }
}

fn items_factory() -> ItemsFactory {
    Arc::new(|store| Arc::new(Items::new(store)) as Arc<dyn WaitingItems>)
}

/// `ghostd check` for Ghost's folder (ADR 0035): the watches and hooks, each hook's
/// fixtures and password, and what in the folder doesn't read or is no longer read (an
/// older layout's department.json, memory.md, docs/, departments/). Prints the lines;
/// returns the problems.
pub async fn check_folder(
    config_dir: &Path,
    secrets: &dyn Secrets,
    out: &mut dyn Write,
) -> Vec<String> {
    let folder = Folder::new(config_dir);
    let mut problems = Vec::new();
    let _ = writeln!(out, "folder: {}", folder.path.display());
    for problem in &folder.problems {
        let _ = writeln!(out, "  problem: {problem}");
        problems.push(problem.clone());
    }
    for line in describe_watches(&folder) {
        let _ = writeln!(out, "{line}");
    }
    let factory = items_factory();
    for h in folder.hooks.values() {
        let failures = run_fixtures(h, None, &factory).await;
        let _ = writeln!(
            out,
            "hook {}: {}, {} event(s), {}",
            h.name,
            scheme(h.verify),
            h.events.len(),
            if failures.is_empty() {
                "fixtures pass".to_string()
            } else {
                format!("{} fixture(s) fail", failures.len())
            }
        );
        problems.extend(failures.iter().map(|f| format!("hook {}: {f}", h.name)));
        let name = h.secret_name();
        if secrets.secret(&name).is_none_or(|s| s.is_empty()) {
            problems.push(format!("hook {}: password {name} is not set", h.name));
        }
    }
    problems
}

/// What `ghost_problems` needs of the apps: each as declared, and how it is now.
pub trait AppStates {
    fn specs(&self) -> Vec<AppSpec>;
    /// connected, connecting, needs_sign_in, failed or stopped.
    fn state(&self, name: &str) -> &'static str;
    fn problem(&self, name: &str) -> Option<String>;
}

impl AppStates for Apps {
    fn specs(&self) -> Vec<AppSpec> {
        Apps::specs(self)
    }

    fn state(&self, name: &str) -> &'static str {
        Apps::state(self, name)
    }

    fn problem(&self, name: &str) -> Option<String> {
        Apps::problem(self, name)
    }
}

/// Ghost's own trouble, for the waiting list (ADR 0029): a keyring to unlock or
/// passwords still in plain files (ADR 0036), an app that isn't connected or wants the
/// operator to sign in again, a file in Ghost's folder that doesn't read or is no longer
/// read (ADR 0035).
pub fn ghost_problems(
    apps: &dyn AppStates,
    folder_problems: &[String],
    passwords: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = passwords.to_vec();
    for spec in apps.specs() {
        let name = &spec.name;
        let state = apps.state(name);
        if state == "connected" {
            continue;
        }
        if state == "needs_sign_in" {
            out.push((
                format!("app:{name}"),
                format!("{} needs you to sign in again", spec.shown()),
            ));
        } else {
            let why = apps
                .problem(name)
                .unwrap_or_else(|| "not connected".to_string());
            out.push((
                format!("app:{name}"),
                format!("{} isn't connected: {why}", spec.shown()),
            ));
        }
    }
    if let Some(first) = folder_problems.first() {
        out.push((
            "folder".to_string(),
            format!("Ghost's folder has a problem: {first}"),
        ));
    }
    out
}

/// Each app as connected: its reads and changes, and the changes Ghost's own
/// permissions block or ask about (ADR 0034).
pub fn describe_apps(apps: &Apps, policy: &AutonomyPolicy) -> Vec<String> {
    let mut lines = Vec::new();
    for spec in apps.specs() {
        let name = &spec.name;
        let live = apps.app(name).filter(|a| a.connected());
        let Some(live) = live else {
            lines.push(format!(
                "  app {name} ({}): not connected: {}",
                spec.transport(),
                apps.problem(name).unwrap_or_default()
            ));
            continue;
        };
        let reads = live.reads().len();
        let changes = live.changes();
        let careful: Vec<String> = changes
            .iter()
            .filter_map(|t| match policy.resolve(name, &t.name, None) {
                ghost_core::levels::Autonomy::Deny => Some(format!("{}=block", t.name)),
                ghost_core::levels::Autonomy::Confirm => Some(format!("{}=ask", t.name)),
                _ => None,
            })
            .collect();
        let mut line = format!(
            "  app {name} ({}): {reads} reads, {} changes",
            spec.transport(),
            changes.len()
        );
        if !careful.is_empty() {
            line.push_str("; ");
            line.push_str(&careful.join(", "));
        }
        lines.push(line);
    }
    lines
}

pub fn describe_skills(skills: &Skills, connected: &BTreeSet<String>) -> String {
    let items: Vec<String> = skills
        .all()
        .iter()
        .map(|s| {
            let missing = s.missing(connected.iter());
            let where_ = if s.source == "ghost" {
                String::new()
            } else {
                format!(" ({})", s.source)
            };
            let mut note = s
                .problem
                .as_ref()
                .map_or(String::new(), |p| format!(" [problem: {p}]"));
            if !missing.is_empty() {
                note.push_str(&format!(" [needs {}]", missing.join(", ")));
            }
            format!("{}{where_}{note}", s.name)
        })
        .collect();
    format!(
        "skills: {} (in {})",
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        },
        skills.ghost_dir.display()
    )
}

/// `ghostd check`. `cfg` is the settings the process environment gave; the vault's
/// passwords are read on top of it, as the daemon reads them (ADR 0036). The exit code.
pub async fn check_command(
    cfg: &GhostdSettings,
    keyring: Arc<dyn Keyring>,
    out: &mut dyn Write,
) -> i32 {
    // The vault first (ADR 0036): Ghost's settings read its passwords from this
    // environment, so they are read after it.
    let environ: SharedEnviron = Arc::new(Mutex::new(HashMap::new()));
    let vault = open_passwords(cfg, keyring, Arc::clone(&environ));
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.extend(environ.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let cfg = GhostdSettings::from_env(&env).unwrap_or_else(|_| cfg.clone());

    let build = build_info::detect(None, None, None);
    let _ = writeln!(out, "version: {}", build_info::describe(VERSION, &build));
    let mut problems = check_passwords(&vault, &cfg, out);
    let locked = KEYRING_WAITS.contains(&vault.state());

    // The model connection (ADR 0037). The voice's own settings (the TTS engine, the
    // trained wake detectors) are the pipeline's and are checked where it loads them.
    let connection = ModelConnection::from_env(&env);
    let _ = writeln!(out, "llm: {}", connection.target(&env).describe());
    problems.extend(connection.problems(&env, locked));
    let _ = writeln!(out, "wake: {}", cfg.wake_mode);

    let autonomy = match AutonomyPolicy::load(&cfg.policy_path) {
        Ok(p) => {
            let _ = writeln!(out, "autonomy policy: {}", p.source);
            p
        }
        Err(e) => {
            problems.push(format!("policy: {e}"));
            AutonomyPolicy::default()
        }
    };
    problems.extend(check_folder(&cfg.config_dir, &env, out).await);

    let apps_file = load_apps(&cfg.config_dir.join(APPS_FILE));
    for problem in &apps_file.problems {
        let _ = writeln!(out, "  problem: {problem}");
        problems.push(problem.clone());
    }
    if !apps_file.apps.is_empty() {
        let signins = SignIns::new(cfg.data_dir.join("apps")).with_port(cfg.signin_port);
        let apps = Apps::new(
            apps_file.apps.clone(),
            Arc::new(env.clone()),
            Some(auth_for_apps(&signins)),
        );
        apps.connect().await;
        let lines = describe_apps(&apps, &autonomy);
        apps.close().await;
        let _ = writeln!(out, "apps:");
        for line in lines {
            let _ = writeln!(out, "{line}");
            if line.contains("not connected") {
                problems.push(line.trim().to_string());
            }
        }
    }

    let skills = Skills::new(&cfg.skills_dir, Some(&cfg.home_skills_dir));
    let connected: BTreeSet<String> = apps_file.apps.iter().map(|a| a.name.clone()).collect();
    let _ = writeln!(out, "{}", describe_skills(&skills, &connected));
    problems.extend(
        skills
            .all()
            .iter()
            .filter_map(|s| s.problem.as_ref().map(|p| format!("skill {}: {p}", s.name))),
    );

    let options = JevOptions {
        timeout: Duration::from_secs_f64(cfg.jev_deadline_secs.max(0.1)),
        ..JevOptions::default()
    };
    match JevClient::from_env(&cfg.jev, &env, options) {
        Ok(Some(jev)) => {
            let _ = writeln!(
                out,
                "confirmation: jev via {} (regex fallback)",
                jev.provider()
            );
        }
        Ok(None) => {
            let _ = writeln!(out, "confirmation: regex");
        }
        Err(e) => problems.push(e.to_string()),
    }

    let with_passwords: BTreeSet<String> = apps_file
        .apps
        .iter()
        .filter(|s| !s.secrets().is_empty())
        .map(|s| s.name.clone())
        .collect();
    let model_password: BTreeSet<String> = std::iter::once(connection.password.clone())
        .filter(|p| !p.is_empty())
        .collect();
    let (problems, waits) = keyring_waits(&vault, &problems, &with_passwords, &model_password);
    for w in waits {
        let _ = writeln!(
            out,
            "passwords: warning: waits for the keyring: {}",
            w.trim()
        );
    }
    if problems.is_empty() {
        let _ = writeln!(out, "config ok");
        0
    } else {
        let _ = writeln!(out, "{}", problems.join("\n"));
        1
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn check_names_what_an_older_layout_left_and_reads_none_of_it() {
        // ADR 0035, no migration: `ghostd check` names each file and folder that is no
        // longer read, says where a department.json's watches and hooks go, and fails.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let watch = json!({
            "app": "github",
            "read": "list_workflow_runs",
            "items": "runs",
            "fields": {"id": {"at": "id", "type": "int"}},
            "key": "{id}",
            "title": "Run {id}",
        });
        std::fs::write(
            p.join("department.json"),
            json!({"title": "Ghost", "watches": {}}).to_string(),
        )
        .unwrap();
        std::fs::write(p.join("memory.md"), "What every agent knew.\n").unwrap();
        std::fs::create_dir(p.join("docs")).unwrap();
        std::fs::create_dir_all(p.join("departments").join("marketing")).unwrap();
        std::fs::write(p.join("watches.json"), json!({"ci": watch}).to_string()).unwrap();
        let secrets: HashMap<String, String> = HashMap::new();
        let mut out = Vec::new();
        let problems = check_folder(p, &secrets, &mut out).await;
        let out = String::from_utf8(out).unwrap();
        assert_eq!(problems.len(), 4, "{problems:?}");
        let old = problems
            .iter()
            .find(|p| p.contains("department.json"))
            .unwrap();
        assert!(old.contains("not read any more"));
        assert!(old.contains("watches.json") && old.contains("hooks.json"));
        let starts = |name: &str| {
            let path = p.join(name).to_string_lossy().into_owned();
            problems.iter().any(|q| q.starts_with(&path))
        };
        assert!(starts("memory.md"));
        assert!(starts("docs"));
        assert!(starts("departments/marketing"));
        assert!(out.contains("watch ci: github.list_workflow_runs every 60s, check, tell"));
        assert_eq!(out.matches("problem:").count(), 4);
        // a folder in today's shape is fine
        std::fs::remove_file(p.join("department.json")).unwrap();
        std::fs::remove_file(p.join("memory.md")).unwrap();
        std::fs::remove_dir(p.join("docs")).unwrap();
        std::fs::remove_dir(p.join("departments").join("marketing")).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            check_folder(p, &secrets, &mut out).await,
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn a_hook_is_checked_with_its_fixtures_and_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(
            p.join("hooks.json"),
            json!({"jobs": crate::hooks_listener::tests::jobs()}).to_string(),
        )
        .unwrap();
        let secrets: HashMap<String, String> = HashMap::new();
        let mut out = Vec::new();
        let problems = check_folder(p, &secrets, &mut out).await;
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("hook jobs: hmac, 1 event(s), 1 fixture(s) fail"),
            "{out}"
        );
        assert!(
            problems
                .iter()
                .any(|q| q.starts_with("hook jobs: no fixtures in"))
        );
        assert!(problems.contains(&"hook jobs: password JOB_SECRET is not set".to_string()));
    }

    struct FakeApps {
        specs: Vec<AppSpec>,
        states: HashMap<&'static str, &'static str>,
        problems: HashMap<&'static str, &'static str>,
    }

    impl AppStates for FakeApps {
        fn specs(&self) -> Vec<AppSpec> {
            self.specs.clone()
        }

        fn state(&self, name: &str) -> &'static str {
            self.states.get(name).copied().unwrap_or("failed")
        }

        fn problem(&self, name: &str) -> Option<String> {
            self.problems.get(name).map(|p| p.to_string())
        }
    }

    #[test]
    fn ghosts_own_problems_are_items_for_the_waiting_list() {
        // ADR 0029, ADR 0034: an app that isn't connected, one that wants a new sign-in, a
        // folder that doesn't read.
        let mut github = AppSpec::runs("github", "x");
        github.title = "GitHub".into();
        let figma = AppSpec::runs("figma", "y");
        let mut linear = AppSpec::at("linear", "https://x");
        linear.title = "Linear".into();
        linear.sign_in = true;
        let apps = FakeApps {
            specs: vec![github, figma, linear],
            states: HashMap::from([
                ("github", "connected"),
                ("figma", "failed"),
                ("linear", "needs_sign_in"),
            ]),
            problems: HashMap::from([
                ("figma", "setting FIGMA_TOKEN is not set"),
                ("linear", "needs sign-in"),
            ]),
        };
        let out = ghost_problems(&apps, &["watches.json: bad json".to_string()], &[]);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["app:figma", "app:linear", "folder"]);
        assert_eq!(out[1].1, "Linear needs you to sign in again");
        assert!(out[0].1.contains("FIGMA_TOKEN"));
        assert_eq!(
            out[0].1,
            "Figma isn't connected: setting FIGMA_TOKEN is not set"
        );
        assert_eq!(
            out[2].1,
            "Ghost's folder has a problem: watches.json: bad json"
        );
        assert_eq!(
            ghost_problems(&apps, &[], &[]).last().unwrap().0,
            "app:linear"
        );
        let passwords = vec![("passwords:locked".to_string(), "Unlock".to_string())];
        assert_eq!(
            ghost_problems(&apps, &[], &passwords)[0].0,
            "passwords:locked"
        );
    }

    #[test]
    fn the_routes_and_the_urgencies_are_described_as_declared() {
        let p = RoutePolicy {
            escalate_after: 10,
            quiet_hours: Some((
                chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                chrono::NaiveTime::from_hms_opt(7, 30, 0).unwrap(),
            )),
            ..RoutePolicy::default()
        };
        assert_eq!(
            describe_routes(&p),
            "routes: digest, phone; escalate after 10 min, quiet 22:00-07:30"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("hooks.json"),
            json!({"jobs": crate::hooks_listener::tests::jobs()}).to_string(),
        )
        .unwrap();
        let folder = Folder::new(dir.path());
        assert_eq!(
            declared_urgency(&folder),
            BTreeMap::from([("hook:jobs".to_string(), "notify".to_string())])
        );
    }
}
