//! Run scripted conversations through `build_session` and collect their timelines
//! (`ghostd.perf.harness`).
//!
//! Each run gets a fresh temporary state directory: its own SQLite, agents, mock GitHub
//! and latency file, so runs are independent and the operator's state is never read or
//! written. The models (Parakeet, Kokoro, Smart Turn) are loaded once per process. The
//! model is scripted (`llm::ScriptedModel`) with the scenario's replies, so every tool
//! call reaches the real gate and the real mock app while the model's own time is a
//! setting (`--ttfb-ms`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_core::apps::{AppSpec, Apps, ProcessEnviron};
use ghost_core::conversations::{
    Conversations, ConversationsOptions, GHOST, Role, SOURCE as CONVERSATIONS, conversations_source,
};
use ghost_core::jev::{JevClassifier, JevClient, JevOptions};
use ghost_core::{
    AutonomyPolicy, ConfirmationClassifier, Events, RegexClassifier, Store, WriteGate,
};
use serde_json::{Value, json};
use tracing::{error, warn};

use super::audio::{PAUSE_MARK, Utterance, Voice};
use super::scenarios::{Line, Scenario};
use crate::config::VoiceSettings;
use crate::echo::words;
use crate::latency::Sink;
use crate::llm::ScriptedModel;
use crate::pipeline::{Greeting, Models, SessionHooks, SessionParams, build_session};
use crate::tap::OnTurn;
use crate::transport::scripted_media;

/// What the harness can turn off to measure a change, each one applied only while a run
/// builds and runs its session:
///   tts_clauses     Kokoro gets whole sentences again (GHOST_TTS_CLAUSES=false)
///   speculate       the model waits out the turn grace (GHOST_TURN_SPECULATE=false)
/// (`prompt_lines`, `system_cache` and `shared_client` are the model connection's; with
/// the scripted model they change nothing here and are accepted for the label's sake.)
pub const ABLATIONS: [&str; 5] = [
    "tts_clauses",
    "prompt_lines",
    "system_cache",
    "shared_client",
    "speculate",
];

/// A plausible record of earlier sessions, for --history: generic, like the seed issues.
pub const HISTORY: [(&str, &str); 8] = [
    ("user", "What's waiting for triage?"),
    (
        "call",
        "github_list_issues(owner=ghost-mock, repo=app, labels=[needs-triage])",
    ),
    (
        "assistant",
        "Three: the session cache choice, dark mode, and rotating the webhook.",
    ),
    ("user", "What's the cache one about?"),
    (
        "call",
        "github_issue_read(method=get, owner=ghost-mock, repo=app, issue_number=142)",
    ),
    (
        "assistant",
        "Redis versus Postgres for the session cache; it blocks two others.",
    ),
    ("user", "Leave it for now. Anything else urgent?"),
    (
        "assistant",
        "The webhook rotation is a security fix, so that one.",
    ),
];

/// The shipped policy runs everything (ADR 0034); the harness asks before a close so the
/// scenarios that measure the yes (ADR 0017) have one to measure.
pub const PERF_POLICY: &str = "[permissions]\nask = [\"github.issue_write(state=closed)\"]\n";

#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Earlier turns seeded into the ghost conversation's record.
    pub history: usize,
    pub jev: bool,
    /// `VoiceSettings` names (`GHOST_*` without the prefix, lowercase) to override.
    pub overrides: BTreeMap<String, String>,
    pub ablate: BTreeSet<String>,
    /// The scripted model's time to first token.
    pub ttfb: Duration,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            history: 0,
            jev: true,
            overrides: BTreeMap::new(),
            ablate: BTreeSet::new(),
            ttfb: Duration::ZERO,
        }
    }
}

impl RunConfig {
    pub fn label(&self) -> String {
        let mut parts = vec!["scripted model".to_string(), "local".to_string()];
        if !self.ablate.is_empty() {
            if ABLATIONS.iter().all(|a| self.ablate.contains(*a)) {
                parts.push("before".into());
            } else {
                parts.push(format!(
                    "-{}",
                    self.ablate.iter().cloned().collect::<Vec<_>>().join("-")
                ));
            }
        }
        if self.ttfb > Duration::ZERO {
            parts.push(format!("ttfb={}ms", self.ttfb.as_millis()));
        }
        if self.history > 0 {
            parts.push(format!("history={}", self.history));
        }
        if !self.jev {
            parts.push("regex".into());
        }
        parts.extend(self.overrides.iter().map(|(k, v)| format!("{k}={v}")));
        parts.join(" ")
    }
}

#[derive(Debug, Clone)]
pub struct LineResult {
    pub text: String,
    pub records: Vec<Value>,
    /// Word match of what STT heard against the script, 0..1.
    pub heard: Option<f64>,
    pub problems: Vec<String>,
    /// The last frame of the line -> Ghost loud at the speaker.
    pub audible: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub scenario: String,
    pub config: String,
    pub run: usize,
    pub lines: Vec<LineResult>,
    pub opening: Option<Value>,
    pub records: Vec<Value>,
    pub problems: Vec<String>,
    pub wall: f64,
    pub wake_to_voice: Option<f64>,
}

impl RunResult {
    pub fn to_json(&self) -> Value {
        json!({
            "scenario": self.scenario,
            "config": self.config,
            "run": self.run,
            "wall": (self.wall * 100.0).round() / 100.0,
            "wake_to_voice": self.wake_to_voice,
            "problems": self.problems,
            "heard": self.lines.iter().map(|l| l.heard).collect::<Vec<_>>(),
            "audible": self.lines.iter().map(|l| l.audible).collect::<Vec<_>>(),
            "records": self.records,
        })
    }
}

/// `none,all,tts_clauses,system_cache+prompt_lines` -> one set per variant.
pub fn parse_ablations(spec: &str) -> Result<Vec<BTreeSet<String>>, String> {
    let parts: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let parts = if parts.is_empty() {
        vec!["none"]
    } else {
        parts
    };
    let mut variants = Vec::new();
    for part in parts {
        if part == "none" {
            variants.push(BTreeSet::new());
        } else if part == "all" {
            variants.push(ABLATIONS.iter().map(|s| s.to_string()).collect());
        } else {
            let names: BTreeSet<String> = part.split('+').map(|n| n.trim().to_string()).collect();
            let unknown: Vec<&String> = names
                .iter()
                .filter(|n| !ABLATIONS.contains(&n.as_str()))
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "unknown ablation {unknown:?}; known: {ABLATIONS:?}"
                ));
            }
            variants.push(names);
        }
    }
    Ok(variants)
}

pub fn make_settings(base: &VoiceSettings, cfg: &RunConfig) -> Result<VoiceSettings, String> {
    let mut env: HashMap<String, String> = base.llm_env.clone();
    for (k, v) in &cfg.overrides {
        env.insert(format!("GHOST_{}", k.to_uppercase()), v.clone());
    }
    if cfg.ablate.contains("tts_clauses") {
        env.insert("GHOST_TTS_CLAUSES".into(), "false".into());
    }
    if cfg.ablate.contains("speculate") {
        env.insert("GHOST_TURN_SPECULATE".into(), "false".into());
    }
    // The base's own values, where the override didn't say.
    let fallback = |name: &str| std::env::var(name).ok();
    VoiceSettings::from_vars(move |name| env.get(name).cloned().or_else(|| fallback(name)))
        .map_err(|e| e.to_string())
        .map(|mut s| {
            s.input_device = None;
            s.output_device = None;
            s
        })
}

fn script_words(text: &str) -> Vec<String> {
    words(&text.replace(PAUSE_MARK.trim(), " "))
}

/// How much of the scripted line STT got, word by word (1.0 is every word).
pub fn heard_ratio(expected: &str, heard: &str) -> f64 {
    let (a, b) = (script_words(expected), words(heard));
    if a.is_empty() {
        return 1.0;
    }
    crate::echo::matching(&a, &b) as f64 / a.len().max(b.len()) as f64
}

/// Models and the utterance renderer, shared by every run in the process.
pub struct Bench {
    pub out: PathBuf,
    pub cache: PathBuf,
    pub show_text: bool,
    pub models: Arc<Models>,
    pub load_secs: f64,
    pub voice: Voice,
    pub infra_dir: PathBuf,
    pub base: VoiceSettings,
}

impl Bench {
    pub fn new(
        out: &Path,
        cache: &Path,
        infra_dir: &Path,
        show_text: bool,
    ) -> anyhow::Result<Self> {
        let base = VoiceSettings::from_env()?;
        crate::debug::enable(base.debug_audio); // the operator's own debugging session only
        let t = Instant::now();
        let models = Arc::new(Models::load(&base)?);
        let load_secs = t.elapsed().as_secs_f64();
        let voice = Voice::new(models.tts.clone(), &cache.join("utterances"));
        Ok(Self {
            out: out.to_path_buf(),
            cache: cache.to_path_buf(),
            show_text,
            models,
            load_secs,
            voice,
            infra_dir: infra_dir.to_path_buf(),
            base,
        })
    }

    /// Where `ghost-mock-github` is: on the PATH, or beside this binary.
    fn mock_github_command() -> String {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let sibling = dir.join("ghost-mock-github");
            if sibling.is_file() {
                return sibling.to_string_lossy().into_owned();
            }
        }
        "ghost-mock-github".into()
    }

    // --- one run -----------------------------------------------------------------

    pub async fn run(&self, scenario: &Scenario, cfg: &RunConfig, index: usize) -> RunResult {
        let began = Instant::now();
        let settings = match make_settings(&self.base, cfg) {
            Ok(s) => s,
            Err(e) => {
                return RunResult {
                    scenario: scenario.name.into(),
                    config: cfg.label(),
                    run: index,
                    lines: Vec::new(),
                    opening: None,
                    records: Vec::new(),
                    problems: vec![e],
                    wall: 0.0,
                    wake_to_voice: None,
                };
            }
        };
        let mut utterances: Vec<Utterance> = Vec::new();
        for line in &scenario.lines {
            match self.voice.render(&line.text) {
                Ok(u) => utterances.push(u),
                Err(e) => {
                    return RunResult {
                        scenario: scenario.name.into(),
                        config: cfg.label(),
                        run: index,
                        lines: Vec::new(),
                        opening: None,
                        records: Vec::new(),
                        problems: vec![format!("could not render a line: {e}")],
                        wall: 0.0,
                        wake_to_voice: None,
                    };
                }
            }
        }
        let tmp = tempfile::Builder::new()
            .prefix("ghost-perf-")
            .tempdir()
            .expect("a temp dir");
        let mut problems: Vec<String> = Vec::new();
        let records: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let heard: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(tokio::sync::Notify::new());

        let sink: Sink = {
            let records = records.clone();
            let notify = notify.clone();
            Arc::new(move |rec| {
                records.lock().unwrap().push(rec.clone());
                notify.notify_waiters();
            })
        };
        let on_turn: OnTurn = {
            let heard = heard.clone();
            let show = self.show_text;
            Arc::new(move |role, text| {
                let heard = heard.clone();
                Box::pin(async move {
                    if role == "user" {
                        heard.lock().unwrap().push(text.clone()); // in memory only, scored against the script
                    }
                    if show {
                        println!("    {role}: {text}");
                    }
                })
            })
        };

        let env = match self.env(tmp.path(), &settings, cfg).await {
            Ok(e) => e,
            Err(e) => {
                return RunResult {
                    scenario: scenario.name.into(),
                    config: cfg.label(),
                    run: index,
                    lines: Vec::new(),
                    opening: None,
                    records: Vec::new(),
                    problems: vec![format!("run failed: {e}")],
                    wall: began.elapsed().as_secs_f64(),
                    wake_to_voice: None,
                };
            }
        };
        let (media, mouth, ear) = scripted_media();
        let replies: Vec<_> = scenario
            .lines
            .iter()
            .flat_map(|l| l.replies.clone())
            .collect();
        let model = ScriptedModel::new(replies, cfg.ttfb);
        let session = build_session(SessionParams {
            settings: Arc::new(settings),
            models: self.models.clone(),
            model,
            sources: env.sources.clone(),
            gate: env.gate.clone(),
            events: Some(Arc::new(Events::default())),
            media,
            greeting: Greeting::Varied,
            first_utterance: None,
            hooks: SessionHooks {
                on_turn: Some(on_turn),
                on_phase: None,
                on_hang_up: None,
                on_conversation: None,
                on_latency: Some(sink),
            },
            wake_phrases: None,
            wake_fuzzy: 0.0,
            conversations: Some(env.conversations.clone()),
            start_in: GHOST.into(),
            latency_log: Some(self.out.join("latency.jsonl")),
            reach: None,
            context_note: None,
        });
        session.start(true).await;
        let session = Arc::new(session);
        let run_task = {
            let s = session.clone();
            tokio::spawn(async move { s.run().await })
        };
        let mut lines: Vec<LineResult> = Vec::new();
        let mut wake_to_voice = None;
        let opening = self
            .wait(
                &records,
                &notify,
                |r| r["kind"] == "opening" && r["spoke"] == true,
                Duration::from_secs(25),
                0,
            )
            .await;
        match &opening {
            None => problems.push("no greeting".into()),
            Some(o) => {
                wake_to_voice = Some(
                    ((o["build"].as_f64().unwrap_or(0.0) + o["reply"].as_f64().unwrap_or(0.0))
                        * 1000.0)
                        .round()
                        / 1000.0,
                );
            }
        }
        for (line, u) in scenario.lines.iter().zip(utterances.iter()) {
            tokio::time::sleep(Duration::from_millis(800)).await; // the operator heard the end and starts talking
            let (n0, h0) = (records.lock().unwrap().len(), heard.lock().unwrap().len());
            let done = mouth.say(&u.pcm);
            // A goodbye can end the session before the line's last frames are sent; a stuck
            // pipeline must not hang the batch.
            let sent = tokio::time::timeout(Duration::from_secs_f64(u.seconds() + 10.0), async {
                tokio::select! {
                    _ = done => {}
                    _ = async { while !run_task.is_finished() { tokio::time::sleep(Duration::from_millis(50)).await } } => {}
                }
            })
            .await;
            if sent.is_err() {
                problems.push(format!("line {}: the microphone stalled", lines.len() + 1));
            }
            let spoke_at = Instant::now();
            if line.ends {
                let deadline = Instant::now() + Duration::from_secs(30);
                while !run_task.is_finished() && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if !run_task.is_finished() {
                    problems.push(format!("line {}: session did not end", lines.len() + 1));
                }
            } else {
                let rec = self
                    .wait(
                        &records,
                        &notify,
                        |r| r["kind"] == "voice" && r["spoke"] == true,
                        Duration::from_secs(45),
                        n0,
                    )
                    .await;
                if rec.is_none() {
                    problems.push(format!("line {}: no spoken reply", lines.len() + 1));
                }
                tokio::time::sleep(Duration::from_millis(300)).await; // let usage and late records land
            }
            let said = heard.lock().unwrap()[h0..].join(" ");
            let line_records: Vec<Value> = records.lock().unwrap()[n0..].to_vec();
            let result = LineResult {
                text: line.text.clone(),
                heard: (!said.is_empty()).then(|| heard_ratio(&line.text, &said)),
                problems: check_line(line, &line_records, run_task.is_finished()),
                audible: ear
                    .loud_after(spoke_at)
                    .map(|d| (d.as_secs_f64() * 1000.0).round() / 1000.0),
                records: line_records,
            };
            problems.extend(
                result
                    .problems
                    .iter()
                    .map(|p| format!("line {}: {p}", lines.len() + 1)),
            );
            lines.push(result);
            if run_task.is_finished() {
                break;
            }
        }
        if let Some(check) = scenario.check
            && let Some(problem) = check(&mock_issues(tmp.path()))
        {
            problems.push(problem);
        }
        if !run_task.is_finished() {
            session.end().await;
            let ended = tokio::time::timeout(Duration::from_secs(20), async {
                while !run_task.is_finished() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            if ended.is_err() {
                session.cancel().await;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
        env.apps.close().await;
        drop(tmp);
        let all = records.lock().unwrap().clone();
        RunResult {
            scenario: scenario.name.into(),
            config: cfg.label(),
            run: index,
            opening: all.iter().find(|r| r["kind"] == "opening").cloned(),
            records: all,
            lines,
            problems,
            wall: began.elapsed().as_secs_f64(),
            wake_to_voice,
        }
    }

    async fn wait(
        &self,
        records: &Arc<Mutex<Vec<Value>>>,
        notify: &Arc<tokio::sync::Notify>,
        want: impl Fn(&Value) -> bool,
        within: Duration,
        start: usize,
    ) -> Option<Value> {
        let deadline = Instant::now() + within;
        loop {
            let found = {
                let recs = records.lock().unwrap();
                let from = start.min(recs.len());
                recs[from..].iter().find(|r| want(r)).cloned()
            };
            if found.is_some() {
                return found;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let _ =
                tokio::time::timeout(left.min(Duration::from_millis(100)), notify.notified()).await;
        }
    }

    // --- an isolated Ghost -----------------------------------------------------------

    async fn env(
        &self,
        tmp: &Path,
        settings: &VoiceSettings,
        cfg: &RunConfig,
    ) -> anyhow::Result<Env> {
        std::fs::create_dir_all(tmp.join("data"))?;
        let store = Arc::new(Store::open(tmp.join("ghost.db"))?);
        // The example layout (infra/example) connects the mock GitHub app (ADR 0034); its
        // state file goes under this run's directory so every run starts from the seed.
        let mut spec = AppSpec::runs("github", &Self::mock_github_command());
        spec.title = "GitHub".into();
        spec.args = vec![
            "--state".into(),
            tmp.join("data/mock-github.json")
                .to_string_lossy()
                .into_owned(),
        ];
        let apps = Arc::new(Apps::new([spec], Arc::new(ProcessEnviron), None));
        apps.connect().await;
        if !apps.apps().contains_key("github") {
            anyhow::bail!("mock GitHub server did not start: {:?}", apps.failed());
        }
        let sources = apps.sources();
        let classifier: Arc<dyn ConfirmationClassifier> = if cfg.jev {
            let env: HashMap<String, String> = std::env::vars().collect();
            match JevClient::from_env("auto", &env, JevOptions::default()) {
                Ok(Some(client)) => Arc::new(JevClassifier::new(Arc::new(client))),
                Ok(None) => Arc::new(RegexClassifier),
                Err(e) => {
                    warn!("jev: {e}; the regex classifier judges");
                    Arc::new(RegexClassifier)
                }
            }
        } else {
            Arc::new(RegexClassifier)
        };
        let gate = Arc::new(
            WriteGate::new(store.clone())
                .with_autonomy(
                    AutonomyPolicy::parse(PERF_POLICY, "perf")
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                )
                .with_classifier(classifier),
        );
        for a in &sources {
            gate.register(&a.name, a.operations.clone());
        }
        // Ghost's standing instructions name the repositories, as an operator's would: the
        // app's tools take owner and repo, and nothing else in the prompt says which.
        std::fs::create_dir_all(tmp.join("config"))?;
        let rules = self.infra_dir.join("example/AGENTS.md");
        if rules.is_file() {
            std::fs::copy(&rules, tmp.join("config/AGENTS.md"))?;
        }
        let conversations = Arc::new(Conversations::new(
            store.clone(),
            tmp.join("config"),
            ConversationsOptions {
                default_voice: settings.tts_voice.clone(),
                default_speed: settings.tts_speed,
                voices: self.models.voices.clone(),
                ..Default::default()
            },
        )?);
        gate.register(
            CONVERSATIONS,
            conversations_source(&conversations).operations,
        );
        for i in 0..cfg.history {
            let (role, text) = HISTORY[i % HISTORY.len()];
            if let Some(r) = Role::parse(role) {
                let _ = conversations.record(GHOST, "earlier", r, text);
            }
        }
        Ok(Env {
            sources,
            apps,
            gate,
            conversations,
        })
    }
}

struct Env {
    sources: Vec<ghost_core::ToolSource>,
    apps: Arc<Apps>,
    gate: Arc<WriteGate>,
    conversations: Arc<Conversations>,
}

fn mock_issues(tmp: &Path) -> Value {
    let path = tmp.join("data/mock-github.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return json!({});
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("issues").cloned())
        .unwrap_or_else(|| json!({}))
}

pub fn tools_used(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .flat_map(|r| r["tools"]["calls"].as_array().cloned().unwrap_or_default())
        .collect()
}

pub fn check_line(line: &Line, records: &[Value], ended: bool) -> Vec<String> {
    let mut out = Vec::new();
    let used = tools_used(records);
    let names: BTreeSet<String> = used
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();
    let missing: Vec<&str> = line
        .tools
        .iter()
        .copied()
        .filter(|t| !names.contains(*t))
        .collect();
    if !missing.is_empty() {
        let got: Vec<&String> = names.iter().collect();
        out.push(format!(
            "expected {missing:?}, got {}",
            if got.is_empty() {
                "no tools".to_string()
            } else {
                format!("{got:?}")
            }
        ));
    }
    if let Some(status) = line.status
        && !used.iter().any(|t| t["status"] == status)
    {
        let mut got: Vec<String> = used
            .iter()
            .filter(|t| t.get("op").is_some())
            .map(|t| t["status"].as_str().unwrap_or("None").to_string())
            .collect();
        got.sort();
        got.dedup();
        out.push(format!(
            "expected status {status}, got {}",
            if got.is_empty() {
                "none".to_string()
            } else {
                format!("{got:?}")
            }
        ));
    }
    if line.intercepted
        && !records
            .iter()
            .any(|r| r.get("intercepted").is_some_and(|v| !v.is_null()))
    {
        out.push("expected the quick intents to take it".into());
    }
    if line.ends && !ended {
        out.push("expected the session to end".into());
    }
    if line.ends && !records.iter().any(|r| r["spoke"] == true) {
        out.push("hung up without a spoken goodbye".into());
    }
    out
}

/// A problem is logged once with its scenario, so a batch keeps going.
pub fn report_problem(scenario: &str, e: &str) {
    error!("perf: {scenario}: {e}");
}
