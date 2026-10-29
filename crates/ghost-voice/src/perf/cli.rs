//! `ghost-perf`: the latency harness and its micro-benchmarks (`ghostd.perf.cli`).
//!
//! ```text
//! ghost-perf run    [--scenario a,b] [--runs N] [--history N] [--set turn_grace_secs=0.4]
//!                   [--no-jev] [--ablate none,all,tts_clauses+speculate] [--ttfb-ms N]
//!                   [--out DIR] [--show-text]
//! ghost-perf micro  [--only stt,tts,jev,turn] [--n N] [--out DIR]
//! ghost-perf llm    [--model a,b] [--endpoint current,proxy,openrouter+sysbp,<provider>]
//!                   [--openrouter-models a,b] [--thinking between_tools] [--n N] [--out DIR]
//! ghost-perf report DIR/records.json
//! ```
//!
//! Output goes to `.scratch/perf/<timestamp>/` (report.md, records.json, latency.jsonl;
//! micro.txt and micro.json; llm.md and llm.json). Keys are read from the environment and
//! never printed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::{Value, json};

use super::harness::{Bench, RunConfig, parse_ablations};
use super::report::render;
use super::scenarios::{DEFAULT, by_name, scenarios};

pub const USAGE: &str = "usage: ghost-perf run|micro|llm|report [options]";

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
    let out = args
        .one("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()));
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
        other => {
            eprintln!("unknown command {other:?}; {USAGE}");
            2
        }
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
    let configs: Vec<RunConfig> = ablations
        .into_iter()
        .map(|ablate| RunConfig {
            history,
            jev: !args.has("no-jev"),
            overrides: overrides.clone(),
            ablate,
            ttfb,
        })
        .collect();
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
    meta.insert("endpoints".into(), json!("the scripted model (no network)"));
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
