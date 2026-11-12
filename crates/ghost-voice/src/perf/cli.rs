//! `ghost-perf`: the latency harness and its micro-benchmarks (`ghostd.perf.cli`).
//!
//! ```text
//! ghost-perf run    [--scenario a,b] [--runs N] [--history N] [--set turn_grace_secs=0.4]
//!                   [--no-jev] [--ablate none,all,tts_clauses+speculate] [--ttfb-ms N]
//!                   [--tokens-per-sec N] [--profile sonnet,haiku,instant]
//!                   [--variant name:field=value+field=value]... [--out DIR] [--show-text]
//!                   [--llm-url URL --llm-model M [--llm-fast-model M] [--llm-key-env NAME]
//!                    [--llm-protocol anthropic|openai]]  (a real model; `fast=real`)
//! ghost-perf micro  [--only stt,tts,jev,turn] [--n N] [--out DIR]
//! ghost-perf llm    [--model a,b] [--endpoint current,proxy,openrouter+sysbp,<provider>]
//!                   [--openrouter-models a,b] [--thinking between_tools] [--n N] [--out DIR]
//! ghost-perf report DIR/records.json
//! ghost-perf hear   [--takes DIR|latest] [--channel aec|raw] [--voices am_michael,bf_emma]
//!                   [--blocks A,B] [--set vad_stop_secs=0.4] [--baseline DIR/results.json]
//!                   [--close-budget-ms 150] [--out DIR] [--show-text]
//! ghost-perf hear-report DIR/results.json [--baseline DIR/results.json]
//! ghost-perf turns  [--takes DIR|latest] [--channel aec|raw] [--voices am_michael]
//!                   [--blocks A,B] [--set turn_confident_grace_secs=0.5] [--out DIR]
//! ghost-perf turns-report DIR/turns.json [--set turn_confident_prob=0.95]
//! ```
//!
//! Output goes to `.scratch/perf/<timestamp>/` (report.md, records.json, latency.jsonl;
//! micro.txt and micro.json; llm.md and llm.json; `hear` to `<timestamp>-hear/`, results.json
//! and report.md, see `hear`). Keys are read from the environment and
//! never printed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::{Value, json};

use super::harness::{Bench, Profile, RealModel, RunConfig, parse_ablations};
use super::report::render;
use super::scenarios::{DEFAULT, by_name, scenarios};

pub const USAGE: &str = "usage: ghost-perf run|micro|llm|report|hear|hear-report|hear-check|turns|turns-report [options]";

fn csv(s: Option<&String>) -> Vec<String> {
    s.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// The repository root: the one with `infra/` beside `Cargo.toml`, found from this binary.
pub fn repo_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .find(|d| d.join("infra").is_dir() && d.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

struct Args {
    cmd: String,
    flags: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut it = argv.iter();
    let cmd = it.next().cloned().ok_or_else(|| USAGE.to_string())?;
    let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut positional = Vec::new();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            if name == "no-jev" || name == "show-text" {
                flags.entry(name.into()).or_default().push("true".into());
                continue;
            }
            let v = it
                .next()
                .cloned()
                .ok_or_else(|| format!("--{name} needs a value"))?;
            flags.entry(name.into()).or_default().push(v);
        } else {
            positional.push(a.clone());
        }
    }
    Ok(Args {
        cmd,
        flags,
        positional,
    })
}

impl Args {
    fn one(&self, name: &str) -> Option<&String> {
        self.flags.get(name).and_then(|v| v.last())
    }
    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }
}

pub fn main(argv: &[String]) -> i32 {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let root = repo_root().unwrap_or_else(|| PathBuf::from("."));
    let scratch = root.join(".scratch/perf");
    if args.cmd == "report" {
        let Some(path) = args.positional.first() else {
            eprintln!("usage: ghost-perf report DIR/records.json");
            return 2;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{path}: {e}");
                return 2;
            }
        };
        let data: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{path}: {e}");
                return 2;
            }
        };
        let runs = data["runs"].as_array().cloned().unwrap_or_default();
        let meta: IndexMap<String, Value> = data["meta"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        print!("{}", render(&runs, &meta));
        return 0;
    }
    if args.cmd == "hear-report" {
        return hear_report(&args);
    }
    if args.cmd == "turns-report" {
        return turns_report(&args);
    }
    if args.cmd == "hear-check" {
        let Some(dir) = args.positional.first() else {
            eprintln!("usage: ghost-perf hear-check SESSION_DIR [--show-text]");
            return 2;
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        return match rt.block_on(super::hear::run_check(
            Path::new(dir),
            args.has("show-text"),
        )) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        };
    }
    let out = args.one("out").map(PathBuf::from).unwrap_or_else(|| {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        scratch.join(match args.cmd.as_str() {
            "hear" => format!("{stamp}-hear"),
            "turns" => format!("{stamp}-turns"),
            _ => stamp,
        })
    });
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("{}: {e}", out.display());
        return 2;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    match args.cmd.as_str() {
        "run" => rt.block_on(run(&args, &out, &scratch, &root)),
        "micro" => {
            let only = csv(args.one("only"));
            let only = if only.is_empty() {
                vec!["stt".into(), "tts".into(), "jev".into(), "turn".into()]
            } else {
                only
            };
            let n = args.one("n").and_then(|n| n.parse().ok()).unwrap_or(5);
            match rt.block_on(super::micro::run_micro(&only, n, &out, &scratch)) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("micro: {e}");
                    1
                }
            }
        }
        "llm" => {
            let endpoints = match args.one("endpoint") {
                Some(e) => csv(Some(e)),
                None => vec!["current".into()],
            };
            let run = super::llm::LlmRun {
                models: csv(args.one("model")),
                endpoints,
                openrouter_models: csv(args.one("openrouter-models")),
                thinking: args
                    .one("thinking")
                    .cloned()
                    .unwrap_or_else(|| "between_tools".into()),
                n: args.one("n").and_then(|n| n.parse().ok()).unwrap_or(4),
                out: out.clone(),
                infra_dir: root.join("infra"),
            };
            match rt.block_on(super::llm::run_llm(&run)) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("llm: {e}");
                    1
                }
            }
        }
        "hear" => match hear_args(&args, &out, &scratch, &root) {
            Ok(a) => match rt.block_on(super::hear::run_hear(&a)) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("hear: {e}");
                    1
                }
            },
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        "turns" => match hear_args(&args, &out, &scratch, &root) {
            Ok(a) => match rt.block_on(super::turns::run_turns(&a)) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("turns: {e}");
                    1
                }
            },
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        other => {
            eprintln!("unknown command {other:?}; {USAGE}");
            2
        }
    }
}

/// A turns report again from its results, under other waits (`--set`), running nothing.
fn turns_report(args: &Args) -> i32 {
    let Some(path) = args.positional.first() else {
        eprintln!("usage: ghost-perf turns-report DIR/turns.json [--set turn_confident_prob=0.95]");
        return 2;
    };
    let results = match super::turns::read_results(Path::new(path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let settings = overrides(args).and_then(|overrides| {
        let base = crate::config::VoiceSettings::from_env().map_err(|e| e.to_string())?;
        super::harness::make_settings(
            &base,
            &RunConfig {
                overrides,
                ..RunConfig::default()
            },
        )
    });
    match settings {
        Ok(s) => {
            print!("{}", super::turns::render(&results, &s.turn_timing()));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn overrides(args: &Args) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for kv in args.flags.get("set").cloned().unwrap_or_default() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("--set wants field=value, got {kv:?}"))?;
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

/// A named set of settings for one run: its name, the overrides, the fast model's profile,
/// and whether the real fast model is in front (`fast=real`).
type Variant = (Option<String>, BTreeMap<String, String>, Option<Profile>, bool);

/// `--fast haiku`, or `none`.
fn fast_profile(name: &str) -> Result<Option<Profile>, String> {
    if name == "none" || name.is_empty() {
        return Ok(None);
    }
    Profile::named(name).map(Some).ok_or_else(|| {
        format!(
            "unknown fast model profile {name:?}; known: {}",
            Profile::NAMES.join(", ")
        )
    })
}

/// Of sessions, the one whose manifest changed last.
fn newest(dirs: &[Option<PathBuf>]) -> Option<PathBuf> {
    dirs.iter()
        .flatten()
        .max_by_key(|d| {
            std::fs::metadata(d.join(super::hear::plan::MANIFEST))
                .and_then(|m| m.modified())
                .ok()
        })
        .cloned()
}

fn hear_args(
    args: &Args,
    out: &Path,
    scratch: &Path,
    root: &Path,
) -> Result<super::hear::HearArgs, String> {
    use super::hear::plan::{Channel, latest_session};
    // The Hearing test's sessions (ADR 0042), and the terminal recorder's before it.
    let data = std::env::var_os("GHOST_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/ghost")))
        .unwrap_or_else(|| PathBuf::from("."));
    let app = super::hear::store::Store::under(&data).sessions_dir();
    let older = root.join(".scratch/hear-takes");
    let takes = match args.one("takes").map(String::as_str) {
        None | Some("") | Some("kokoro") => None,
        Some("latest") => Some(
            newest(&[latest_session(&app), latest_session(&older)]).ok_or_else(|| {
                format!(
                    "no recorded session under {}; record one in Ghost's Hearing test",
                    app.display()
                )
            })?,
        ),
        Some(dir) => Some(PathBuf::from(dir)),
    };
    let voices = match csv(args.one("voices")) {
        v if v.is_empty() => vec!["am_michael".to_string()],
        v => v,
    };
    let budget_ms: f64 = args
        .one("close-budget-ms")
        .map(|v| {
            v.parse()
                .map_err(|_| format!("--close-budget-ms wants a number, got {v:?}"))
        })
        .transpose()?
        .unwrap_or(150.0);
    Ok(super::hear::HearArgs {
        takes,
        channel: Channel::parse(args.one("channel").map_or("aec", String::as_str))?,
        voices,
        blocks: csv(args.one("blocks"))
            .into_iter()
            .map(|b| b.to_uppercase())
            .collect(),
        overrides: overrides(args)?,
        baseline: args.one("baseline").map(PathBuf::from),
        out: out.to_path_buf(),
        cache: scratch.to_path_buf(),
        root: root.to_path_buf(),
        show_text: args.has("show-text"),
        close_budget_secs: budget_ms / 1000.0,
    })
}

/// A hear report again from its results, against another run's if given.
fn hear_report(args: &Args) -> i32 {
    let Some(path) = args.positional.first() else {
        eprintln!("usage: ghost-perf hear-report DIR/results.json [--baseline DIR/results.json]");
        return 2;
    };
    let load = |p: &str| super::hear::read_results(Path::new(p));
    let results = match load(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let baseline = match args.one("baseline").map(|b| load(b)).transpose() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let budget = args
        .one("close-budget-ms")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(150.0)
        / 1000.0;
    let (text, verdict) = super::hear::report::render(&results, baseline.as_ref(), budget);
    print!("{text}");
    match verdict {
        Some(false) => 1,
        _ => 0,
    }
}

async fn run(args: &Args, out: &Path, scratch: &Path, root: &Path) -> i32 {
    let names = {
        let n = csv(args.one("scenario"));
        if n.is_empty() {
            DEFAULT.iter().map(|s| s.to_string()).collect()
        } else {
            n
        }
    };
    let unknown: Vec<&String> = names.iter().filter(|n| by_name(n).is_none()).collect();
    if !unknown.is_empty() {
        eprintln!(
            "unknown scenario(s) {unknown:?}; known: {}",
            scenarios()
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return 2;
    }
    let runs_each: usize = args.one("runs").and_then(|r| r.parse().ok()).unwrap_or(3);
    let mut overrides = BTreeMap::new();
    for kv in args.flags.get("set").cloned().unwrap_or_default() {
        match kv.split_once('=') {
            Some((k, v)) => {
                overrides.insert(k.to_string(), v.to_string());
            }
            None => {
                eprintln!("--set wants field=value, got {kv:?}");
                return 2;
            }
        }
    }
    let ablations = match parse_ablations(args.one("ablate").map(String::as_str).unwrap_or("none"))
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let ttfb = Duration::from_millis(
        args.one("ttfb-ms")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    );
    let history = args
        .one("history")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let rate: Option<f64> = args.one("tokens-per-sec").and_then(|v| v.parse().ok());
    // The model's assumed time, one config per profile; none is `--ttfb-ms` as given.
    let mut profiles: Vec<Option<Profile>> = Vec::new();
    for name in csv(args.one("profile")) {
        match Profile::named(&name) {
            Some(p) => profiles.push(Some(p)),
            None => {
                eprintln!(
                    "unknown profile {name:?}; known: {}",
                    Profile::NAMES.join(", ")
                );
                return 2;
            }
        }
    }
    if profiles.is_empty() {
        profiles.push(None);
    }
    // A fast model in front (ADR 0050), scripted from the same lines: `--fast haiku`.
    let fast = match args
        .one("fast")
        .filter(|v| v.as_str() != "real")
        .map(|v| fast_profile(v))
        .transpose()
    {
        Ok(f) => f.flatten(),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // A real model instead of the script: `--llm-url http://host:8317 --llm-model m
    // [--llm-fast-model m] [--llm-key-env NAME] [--llm-protocol anthropic|openai]`. Only the
    // variable named is read for a key, and it is never printed.
    let real = args.one("llm-url").map(|url| RealModel {
        url: url.trim_end_matches('/').to_string(),
        model: args
            .one("llm-model")
            .cloned()
            .unwrap_or_else(|| ghost_core::connection::DEFAULT_CLAUDE.to_string()),
        fast_model: args.one("llm-fast-model").cloned(),
        key_env: args.one("llm-key-env").cloned(),
        protocol: args
            .one("llm-protocol")
            .cloned()
            .unwrap_or_else(|| "anthropic".into()),
    });
    if real.is_some() && args.has("profile") {
        eprintln!("--profile is the scripted model's time; a real model (--llm-url) has its own");
        return 2;
    }
    let fast_real_default = real.as_ref().is_some_and(|r| r.fast_model.is_some())
        && args.one("fast").map(String::as_str) == Some("real");
    let fast = if fast_real_default { None } else { fast };
    // Named sets of overrides, compared in one run: `before:turn_grace_secs=1.2+fast=haiku`.
    let mut variants: Vec<Variant> = Vec::new();
    for spec in args.flags.get("variant").cloned().unwrap_or_default() {
        let (name, sets) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        let mut over = overrides.clone();
        let mut fast = fast.clone();
        let mut fast_real = fast_real_default;
        for kv in sets.split('+').map(str::trim).filter(|s| !s.is_empty()) {
            match kv.split_once('=') {
                // The real fast model, given by --llm-fast-model.
                Some(("fast", "real")) => {
                    if real.as_ref().and_then(|r| r.fast_model.as_ref()).is_none() {
                        eprintln!("fast=real needs --llm-url and --llm-fast-model");
                        return 2;
                    }
                    fast = None;
                    fast_real = true;
                }
                // The fast model is the connection's, not a voice setting.
                Some(("fast", v)) => match fast_profile(v) {
                    Ok(p) => fast = p,
                    Err(e) => {
                        eprintln!("{e}");
                        return 2;
                    }
                },
                Some((k, v)) => {
                    over.insert(k.to_string(), v.to_string());
                }
                None => {
                    eprintln!("--variant wants name:field=value+field=value, got {spec:?}");
                    return 2;
                }
            }
        }
        variants.push((Some(name.to_string()), over, fast, fast_real));
    }
    if variants.is_empty() {
        variants.push((None, overrides.clone(), fast.clone(), fast_real_default));
    }
    let mut configs: Vec<RunConfig> = Vec::new();
    for ablate in ablations {
        for (variant, over, fast, fast_real) in &variants {
            for profile in &profiles {
                configs.push(RunConfig {
                    history,
                    jev: !args.has("no-jev"),
                    overrides: over.clone(),
                    ablate: ablate.clone(),
                    ttfb: profile.as_ref().map_or(ttfb, |p| p.ttfb),
                    call_ttfb: profile.as_ref().and_then(|p| p.call_ttfb),
                    tokens_per_sec: profile.as_ref().map_or(rate, |p| p.tokens_per_sec),
                    profile: profile.as_ref().map(|p| p.name.clone()),
                    variant: variant.clone(),
                    fast: fast.clone(),
                    real: real.clone(),
                    fast_real: *fast_real,
                });
            }
        }
    }
    let total = configs.len() * names.len() * runs_each;
    println!(
        "perf: {total} runs ({} configs x {} scenarios x {runs_each})",
        configs.len(),
        names.len()
    );
    println!("perf: output in {}", out.display());
    let t = Instant::now();
    let bench = match Bench::new(out, scratch, &root.join("infra"), args.has("show-text")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("perf: the models did not load: {e}");
            return 1;
        }
    };
    println!("perf: models loaded in {:.1}s", bench.load_secs);
    let mut runs: Vec<Value> = Vec::new();
    let mut meta: IndexMap<String, Value> = IndexMap::new();
    meta.insert(
        "date".into(),
        json!(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
    );
    meta.insert(
        "host".into(),
        json!(format!(
            "the workstation ({} CPUs)",
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1)
        )),
    );
    meta.insert(
        "endpoints".into(),
        json!(match &real {
            Some(r) => format!(
                "a real model: {} at {} ({}), fast model {}",
                r.model,
                r.url,
                r.protocol,
                r.fast_model.as_deref().unwrap_or("none")
            ),
            None => "the scripted model (no network)".to_string(),
        }),
    );
    let named: Vec<String> = profiles
        .iter()
        .flatten()
        .map(|p| {
            format!(
                "{} (first token {} ms, {})",
                p.name,
                p.ttfb.as_millis(),
                p.tokens_per_sec
                    .map_or("the reply at once".to_string(), |r| format!(
                        "{r:.0} tokens/s"
                    ))
            )
        })
        .collect();
    if !named.is_empty() {
        meta.insert(
            "model time (assumed, not measured)".into(),
            json!(named.join("; ")),
        );
    }
    let described: Vec<String> = variants
        .iter()
        .filter_map(|(name, over, fast, fast_real)| {
            name.as_ref().map(|n| {
                let mut sets: Vec<String> = over.iter().map(|(k, v)| format!("{k}={v}")).collect();
                if let Some(f) = fast {
                    sets.push(format!("a fast model in front ({})", f.name));
                }
                if *fast_real && let Some(m) = real.as_ref().and_then(|r| r.fast_model.as_ref()) {
                    sets.push(format!("the real fast model in front ({m})"));
                }
                format!(
                    "{n}: {}",
                    if sets.is_empty() {
                        "the defaults".to_string()
                    } else {
                        sets.join(", ")
                    }
                )
            })
        })
        .collect();
    if !described.is_empty() {
        meta.insert("variants".into(), json!(described.join("; ")));
    }
    meta.insert("scenarios".into(), json!(names.join(", ")));
    meta.insert("runs per scenario".into(), json!(runs_each));
    meta.insert(
        "settings overrides".into(),
        json!(if overrides.is_empty() {
            "none".to_string()
        } else {
            overrides
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        }),
    );
    meta.insert("history seeded".into(), json!(history));
    meta.insert(
        "confirmation".into(),
        json!(if args.has("no-jev") {
            "regex only"
        } else {
            "Jev when a key is set, else regex"
        }),
    );
    meta.insert("daemon".into(), json!("ghost-voice (Rust)"));
    // Interleaved: every config runs a scenario before any runs it again.
    for i in 1..=runs_each {
        for name in &names {
            let scenario = by_name(name).expect("checked above");
            for cfg in &configs {
                let res = bench.run(&scenario, cfg, i).await;
                let replies: Vec<String> = res
                    .records
                    .iter()
                    .filter(|r| r["kind"] == "voice")
                    .filter_map(|r| r["reply"].as_f64())
                    .map(|x| format!("{x:.2}s"))
                    .collect();
                let flag = if res.problems.is_empty() {
                    String::new()
                } else {
                    format!("  !! {}", res.problems.join("; "))
                };
                println!(
                    "  [{}] {name} #{i}: replies {} (wall {:.0}s){flag}",
                    cfg.label(),
                    if replies.is_empty() {
                        "-".to_string()
                    } else {
                        replies.join(", ")
                    },
                    res.wall
                );
                runs.push(res.to_json());
                let _ = std::fs::write(
                    out.join("records.json"),
                    serde_json::to_string(&json!({"meta": meta, "runs": runs})).unwrap_or_default(),
                );
            }
        }
    }
    meta.insert(
        "wall".into(),
        json!(format!("{:.0}s", t.elapsed().as_secs_f64())),
    );
    let _ = std::fs::write(
        out.join("records.json"),
        serde_json::to_string_pretty(&json!({"meta": meta, "runs": runs})).unwrap_or_default(),
    );
    let report = render(&runs, &meta);
    let _ = std::fs::write(out.join("report.md"), &report);
    println!("\nperf: report {}", out.join("report.md").display());
    0
}
