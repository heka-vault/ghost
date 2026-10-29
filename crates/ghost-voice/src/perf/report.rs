//! Markdown report from harness runs (`ghostd.perf.report`): where each turn's time went,
//! per scenario and stage, the model calls and the prompt cache, the tools, and the
//! biggest contributors. Numbers only; the harness never writes what was said.

use indexmap::IndexMap;
use serde_json::Value;

use crate::latency::{cache_share, percentile, stage_values};

pub const STAGE_ORDER: [&str; 10] = [
    "hear", "stt", "pre", "turn", "hold", "llm", "llm_ttfb", "tools", "tts", "other",
];

fn f(x: Option<f64>, nd: usize) -> String {
    match x {
        None => "-".into(),
        Some(x) => format!("{x:.nd$}"),
    }
}

fn pct(x: Option<f64>) -> String {
    match x {
        None => "-".into(),
        Some(x) => format!("{}%", (x * 100.0).round_ties_even() as i64),
    }
}

fn p(vals: &[Option<f64>], q: f64) -> Option<f64> {
    let xs: Vec<f64> = vals.iter().filter_map(|v| *v).collect();
    percentile(&xs, q)
}

fn mean(vals: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    let xs: Vec<f64> = vals.into_iter().flatten().collect();
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

fn records(run: &Value) -> Vec<Value> {
    run["records"].as_array().cloned().unwrap_or_default()
}

pub fn voice_turns(runs: &[Value]) -> Vec<Value> {
    runs.iter()
        .flat_map(records)
        .filter(|r| r["kind"] == "voice" && r["spoke"] == true)
        .collect()
}

pub fn calls_of(recs: &[Value]) -> Vec<Value> {
    recs.iter()
        .flat_map(|r| r["llm"]["calls"].as_array().cloned().unwrap_or_default())
        .collect()
}

fn stage(rec: &Value, k: &str) -> Option<f64> {
    stage_values(rec).get(k).copied().flatten()
}

pub fn render(runs: &[Value], meta: &IndexMap<String, Value>) -> String {
    let mut by_config: IndexMap<String, Vec<Value>> = IndexMap::new();
    for run in runs {
        by_config
            .entry(run["config"].as_str().unwrap_or("").to_string())
            .or_default()
            .push(run.clone());
    }
    let mut out: Vec<String> = vec![
        format!(
            "# Ghost latency harness, {}",
            meta.get("date").and_then(Value::as_str).unwrap_or("")
        ),
        String::new(),
    ];
    for (k, v) in meta {
        if k != "date" {
            let shown = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.push(format!("- {k}: {shown}"));
        }
    }
    out.push(String::new());
    if by_config.len() > 1 {
        out.extend(comparison(&by_config));
    }
    for (config, cruns) in &by_config {
        out.extend(config_section(config, cruns));
    }
    out.join("\n") + "\n"
}

/// Tool-calling model calls that wrote text first, out of all tool-calling calls
/// (hang_up and hold excepted: their text is spoken).
pub fn narration(calls: &[Value]) -> (usize, usize) {
    let tool_calls: Vec<&Value> = calls
        .iter()
        .filter(|c| {
            let tools = c["tools"].as_array().cloned().unwrap_or_default();
            !tools.is_empty()
                && tools != vec![Value::String("hang_up".into())]
                && tools != vec![Value::String("hold".into())]
        })
        .collect();
    let said = tool_calls.iter().filter(|c| c["text"] == true).count();
    (said, tool_calls.len())
}

/// (cache read, cache write) of each session's first model call.
pub fn first_call_cache(runs: &[Value]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for run in runs {
        for r in records(run) {
            let calls = r["llm"]["calls"].as_array().cloned().unwrap_or_default();
            if let Some(c) = calls.first()
                && c.get("in").is_some_and(|v| !v.is_null())
            {
                out.push((
                    c["cache_read"].as_u64().unwrap_or(0),
                    c["cache_write"].as_u64().unwrap_or(0),
                ));
                break;
            }
        }
    }
    out
}

fn comparison(by_config: &IndexMap<String, Vec<Value>>) -> Vec<String> {
    let mut out = vec![
        "## Configurations compared (voice turns)".to_string(),
        String::new(),
        "| config | turns | reply p50 | reply p95 | llm p50 | llm ttfb p50 | ttfb p95 \
         | tts synth p50 | tts p95 | calls/turn | narrated | cache | 1st call read \
         | wake to voice p50 | problems |"
            .to_string(),
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|".to_string(),
    ];
    for (config, cruns) in by_config {
        let turns = voice_turns(cruns);
        let calls = calls_of(&turns);
        let ttfbs: Vec<Option<f64>> = calls.iter().map(|c| c["ttfb"].as_f64()).collect();
        let problems = cruns
            .iter()
            .filter(|r| !r["problems"].as_array().is_none_or(Vec::is_empty))
            .count();
        let (said, tool_calls) = narration(&calls);
        let first = first_call_cache(cruns);
        let reads = first.iter().filter(|(r, _)| *r > 0).count();
        let st = |k: &str| -> Vec<Option<f64>> { turns.iter().map(|t| stage(t, k)).collect() };
        let wake: Vec<Option<f64>> = cruns.iter().map(|r| r["wake_to_voice"].as_f64()).collect();
        out.push(format!(
            "| {config} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {said}/{tool_calls} | {} | {reads}/{} | {} | {problems}/{} |",
            turns.len(),
            f(p(&st("reply"), 50.0), 2),
            f(p(&st("reply"), 95.0), 2),
            f(p(&st("llm"), 50.0), 2),
            f(p(&ttfbs, 50.0), 2),
            f(p(&ttfbs, 95.0), 2),
            f(p(&st("tts_synth"), 50.0), 2),
            f(p(&st("tts_synth"), 95.0), 2),
            f(
                (!turns.is_empty()).then(|| calls.len() as f64 / turns.len() as f64),
                1
            ),
            pct(cache_share(&calls)),
            first.len(),
            f(p(&wake, 50.0), 2),
            cruns.len(),
        ));
    }
    out.push(String::new());
    out.extend(scenario_matrix(by_config));
    out.push(String::new());
    out
}

/// Per scenario, each config's reply p50/p95, model time p50 and TTS synth p50.
fn scenario_matrix(by_config: &IndexMap<String, Vec<Value>>) -> Vec<String> {
    let configs: Vec<&String> = by_config.keys().collect();
    let mut out = vec![
        "### Per scenario: reply p50 / p95, model p50, TTS synth p50 (seconds)".to_string(),
        String::new(),
        format!(
            "| scenario | {} |",
            configs
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        format!("|---|{}", "---|".repeat(configs.len())),
    ];
    let mut scenarios: Vec<String> = Vec::new();
    for cruns in by_config.values() {
        for r in cruns {
            let name = r["scenario"].as_str().unwrap_or("").to_string();
            if !scenarios.contains(&name) {
                scenarios.push(name);
            }
        }
    }
    for name in scenarios {
        let mut cells = Vec::new();
        for config in &configs {
            let runs: Vec<Value> = by_config[*config]
                .iter()
                .filter(|r| r["scenario"] == name.as_str())
                .cloned()
                .collect();
            let turns = voice_turns(&runs);
            if turns.is_empty() {
                cells.push("-".to_string());
                continue;
            }
            let st = |k: &str| -> Vec<Option<f64>> { turns.iter().map(|t| stage(t, k)).collect() };
            cells.push(format!(
                "{} / {}, llm {}, tts {}",
                f(p(&st("reply"), 50.0), 2),
                f(p(&st("reply"), 95.0), 2),
                f(p(&st("llm"), 50.0), 2),
                f(p(&st("tts_synth"), 50.0), 2)
            ));
        }
        out.push(format!("| {name} | {} |", cells.join(" | ")));
    }
    out
}

fn config_section(config: &str, runs: &[Value]) -> Vec<String> {
    let mut out = vec![format!("## {config}"), String::new()];
    let turns = voice_turns(runs);
    out.extend(contributors(&turns));
    out.extend(per_scenario(runs));
    out.extend(stages(&turns));
    out.extend(llm(runs));
    out.extend(cache_sequence(runs));
    out.extend(tools(&turns));
    out.extend(confirm_and_stt(runs));
    out.extend(opening(runs));
    let problems: Vec<String> = runs
        .iter()
        .flat_map(|r| {
            r["problems"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |p| {
                    format!(
                        "- {} run {}: {}",
                        r["scenario"].as_str().unwrap_or(""),
                        r["run"],
                        p.as_str().unwrap_or("")
                    )
                })
        })
        .collect();
    if !problems.is_empty() {
        out.push("### Problems (model drift or failures)".into());
        out.push(String::new());
        out.extend(problems);
        out.push(String::new());
    }
    out
}

fn contributors(turns: &[Value]) -> Vec<String> {
    if turns.is_empty() {
        return vec!["No voice turns.".into(), String::new()];
    }
    let reply = mean(turns.iter().map(|t| stage(t, "reply"))).unwrap_or(0.0);
    let parts: Vec<(&str, Option<f64>)> = vec![
        (
            "turn hold (grace)",
            mean(turns.iter().map(|t| stage(t, "hold"))),
        ),
        (
            "LLM (all calls on the path)",
            mean(turns.iter().map(|t| stage(t, "llm"))),
        ),
        (
            "Smart Turn wait",
            mean(turns.iter().map(|t| stage(t, "turn"))),
        ),
        (
            "TTS to first audio + output",
            mean(turns.iter().map(|t| stage(t, "tts"))),
        ),
        (
            "VAD stop silence",
            mean(turns.iter().map(|t| t["hear"]["vad"].as_f64())),
        ),
        ("STT", mean(turns.iter().map(|t| stage(t, "stt")))),
        (
            "tools beyond the model",
            mean(turns.iter().map(|t| stage(t, "tools"))),
        ),
        (
            "pre-LLM processors (interceptors, Jev)",
            mean(turns.iter().map(|t| stage(t, "pre"))),
        ),
        (
            "pipeline hops (other)",
            mean(turns.iter().map(|t| stage(t, "other"))),
        ),
    ];
    let mut ranked: Vec<(f64, &str)> = parts.iter().map(|(k, v)| (v.unwrap_or(0.0), *k)).collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![
        format!(
            "### Where a voice turn's time goes ({} turns, mean reply {reply:.2}s)",
            turns.len()
        ),
        String::new(),
        "| contributor | mean s | share |".into(),
        "|---|---|---|".into(),
    ];
    for (v, k) in ranked {
        if reply > 0.0 {
            out.push(format!(
                "| {k} | {v:.2} | {}% |",
                (v / reply * 100.0).round_ties_even() as i64
            ));
        } else {
            out.push(format!("| {k} | {v:.2} | - |"));
        }
    }
    out.push(String::new());
    out
}

fn by_scenario(runs: &[Value]) -> IndexMap<String, Vec<Value>> {
    let mut by: IndexMap<String, Vec<Value>> = IndexMap::new();
    for run in runs {
        by.entry(run["scenario"].as_str().unwrap_or("").to_string())
            .or_default()
            .push(run.clone());
    }
    by
}

fn per_scenario(runs: &[Value]) -> Vec<String> {
    let mut out = vec![
        "### End to end per scenario (operator stops talking -> Ghost's voice starts)".into(),
        String::new(),
        "reply: from the VAD's end of speech to BotStartedSpeaking, as the live log reports it. \
         audible: from the line's last audio frame to Ghost's voice loud at the scripted \
         speaker or the WebRTC peer (device or network buffering included; the thinking \
         sound can count when it is loud)."
            .into(),
        String::new(),
        "| scenario | runs ok | turns | reply p50 | reply p95 | audible p50 | hear p50 | \
         llm p50 | llm ttfb p50 | tools p50 | tts p50 | other p50 |"
            .into(),
        "|---|---|---|---|---|---|---|---|---|---|---|---|".into(),
    ];
    for (name, sruns) in by_scenario(runs) {
        let turns = voice_turns(&sruns);
        let ok = sruns
            .iter()
            .filter(|r| r["problems"].as_array().is_none_or(Vec::is_empty))
            .count();
        let audible: Vec<Option<f64>> = sruns
            .iter()
            .flat_map(|r| {
                r["audible"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| a.as_f64())
            })
            .collect();
        let col = |k: &str, q: f64| -> String {
            let vals: Vec<Option<f64>> = turns.iter().map(|t| stage(t, k)).collect();
            f(p(&vals, q), 2)
        };
        out.push(format!(
            "| {name} | {ok}/{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            sruns.len(),
            turns.len(),
            col("reply", 50.0),
            col("reply", 95.0),
            f(p(&audible, 50.0), 2),
            col("hear", 50.0),
            col("llm", 50.0),
            col("llm_ttfb", 50.0),
            col("tools", 50.0),
            col("tts", 50.0),
            col("other", 50.0),
        ));
    }
    out.push(String::new());
    out
}

fn stages(turns: &[Value]) -> Vec<String> {
    let mut out = vec![
        "### Stages across voice turns".into(),
        String::new(),
        "| stage | n | p50 | p95 | max |".into(),
        "|---|---|---|---|---|".into(),
    ];
    let mut keys = vec!["reply"];
    keys.extend(STAGE_ORDER);
    keys.push("tts_synth");
    for k in keys {
        let vals: Vec<f64> = turns.iter().filter_map(|t| stage(t, k)).collect();
        if vals.is_empty() {
            continue;
        }
        let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        out.push(format!(
            "| {k} | {} | {} | {} | {max:.2} |",
            vals.len(),
            f(percentile(&vals, 50.0), 2),
            f(percentile(&vals, 95.0), 2)
        ));
    }
    let segs = turns
        .iter()
        .filter(|t| t["hear"]["segments"].as_u64().unwrap_or(0) > 1)
        .count();
    let kept: u64 = turns
        .iter()
        .map(|t| t["hear"]["kept_open"].as_u64().unwrap_or(0))
        .sum();
    let smart: Vec<Value> = turns
        .iter()
        .flat_map(|t| {
            t["hear"]["smart_turn"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    let incomplete = smart.iter().filter(|e| e["complete"] != true).count();
    out.push(String::new());
    out.push(format!(
        "Turns split by a VAD pause: {segs}/{}; kept open by the hold: {kept}; Smart Turn said \
         incomplete {incomplete}/{} times (each costs up to its silence timeout).",
        turns.len(),
        smart.len()
    ));
    out.push(String::new());
    out
}

fn llm(runs: &[Value]) -> Vec<String> {
    let mut out = vec![
        "### Model calls".into(),
        String::new(),
        "| scenario | calls | calls/turn | ttfb p50 | ttfb p95 | dur p50 | dur p95 | \
         first tool p50 | in (uncached) | cache read | cache write | out | cache hit |"
            .into(),
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|".into(),
    ];
    for (name, sruns) in by_scenario(runs) {
        let recs: Vec<Value> = sruns.iter().flat_map(records).collect();
        let calls = calls_of(&recs);
        if calls.is_empty() {
            continue;
        }
        let turns = recs
            .iter()
            .filter(|r| !r["llm"]["calls"].as_array().is_none_or(Vec::is_empty))
            .count();
        let pq = |key: &str, q: f64| -> String {
            let vals: Vec<Option<f64>> = calls.iter().map(|c| c[key].as_f64()).collect();
            f(p(&vals, q), 2)
        };
        let m = |key: &str| -> String {
            match mean(calls.iter().map(|c| c[key].as_f64())) {
                None => "-".into(),
                Some(v) => format!("{v:.0}"),
            }
        };
        out.push(format!(
            "| {name} | {} | {:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            calls.len(),
            calls.len() as f64 / turns.max(1) as f64,
            pq("ttfb", 50.0),
            pq("ttfb", 95.0),
            pq("dur", 50.0),
            pq("dur", 95.0),
            pq("first_tool", 50.0),
            m("in"),
            m("cache_read"),
            m("cache_write"),
            m("out"),
            pct(cache_share(&calls)),
        ));
    }
    out.push(String::new());
    out
}

/// Token use call by call in the first run of each multi-call scenario: whether each call
/// reads what the previous one wrote.
fn cache_sequence(runs: &[Value]) -> Vec<String> {
    let mut out = vec![
        "### Prompt cache, call by call (first run of each scenario)".to_string(),
        String::new(),
    ];
    let mut shown: Vec<String> = Vec::new();
    for run in runs {
        let scenario = run["scenario"].as_str().unwrap_or("").to_string();
        if shown.contains(&scenario) {
            continue;
        }
        let mut rows = Vec::new();
        for r in records(run) {
            for (i, c) in r["llm"]["calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                if c.get("in").is_none_or(Value::is_null) {
                    continue;
                }
                rows.push(format!(
                    "{} {}.{}: in {}, read {}, write {}, ttfb {}",
                    r["kind"].as_str().unwrap_or(""),
                    r["turn"],
                    i + 1,
                    c["in"],
                    c["cache_read"],
                    c["cache_write"],
                    f(c["ttfb"].as_f64(), 2)
                ));
            }
        }
        if rows.len() < 2 {
            continue;
        }
        shown.push(scenario.clone());
        out.push(format!("- **{scenario}**: {}", rows.join("; ")));
    }
    if out.len() == 2 {
        return Vec::new();
    }
    out.push(String::new());
    out
}

fn tools(turns: &[Value]) -> Vec<String> {
    let calls: Vec<Value> = turns
        .iter()
        .flat_map(|r| r["tools"]["calls"].as_array().cloned().unwrap_or_default())
        .collect();
    if calls.is_empty() {
        return Vec::new();
    }
    let mut out = vec![
        "### Tools (function call start -> result; read tools call the app behind them)".into(),
        String::new(),
        "| tool | n | p50 | p95 | steps p50 |".into(),
        "|---|---|---|---|---|".into(),
    ];
    let mut by: IndexMap<String, Vec<Value>> = IndexMap::new();
    for t in calls {
        let name = t["op"]
            .as_str()
            .or_else(|| t["name"].as_str())
            .unwrap_or("")
            .to_string();
        by.entry(name).or_default().push(t);
    }
    by.sort_keys();
    for (name, ts) in by {
        let mut steps: IndexMap<String, Vec<f64>> = IndexMap::new();
        for t in &ts {
            if let Some(obj) = t["steps"].as_object() {
                for (k, v) in obj {
                    if let Some(x) = v.as_f64() {
                        steps.entry(k.clone()).or_default().push(x);
                    }
                }
            }
        }
        let step_txt: Vec<String> = steps
            .iter()
            .map(|(k, v)| format!("{k} {}", f(percentile(v, 50.0), 3)))
            .collect();
        let secs: Vec<f64> = ts.iter().filter_map(|t| t["secs"].as_f64()).collect();
        out.push(format!(
            "| {name} | {} | {} | {} | {} |",
            ts.len(),
            f(percentile(&secs, 50.0), 3),
            f(percentile(&secs, 95.0), 3),
            if step_txt.is_empty() {
                "-".to_string()
            } else {
                step_txt.join(", ")
            }
        ));
    }
    out.push(String::new());
    out
}

fn confirm_and_stt(runs: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    let confirms: Vec<Value> = runs
        .iter()
        .flat_map(records)
        .filter_map(|r| r.get("confirm").filter(|c| c.is_object()).cloned())
        .collect();
    if !confirms.is_empty() {
        let mut by: IndexMap<String, Vec<f64>> = IndexMap::new();
        for c in &confirms {
            by.entry(c["classifier"].as_str().unwrap_or("").to_string())
                .or_default()
                .push(c["classify"].as_f64().unwrap_or(0.0));
        }
        let parts: Vec<String> = by
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k} x{} classify p50 {}s",
                    v.len(),
                    f(percentile(v, 50.0), 3)
                )
            })
            .collect();
        let total: Vec<f64> = confirms
            .iter()
            .filter_map(|c| c["total"].as_f64())
            .collect();
        out.push("### Confirmation".into());
        out.push(String::new());
        out.push(format!(
            "{} spoken answers: {}; whole step incl. the write p50 {}s.",
            confirms.len(),
            parts.join("; "),
            f(percentile(&total, 50.0), 3)
        ));
        out.push(String::new());
    }
    let heard: Vec<f64> = runs
        .iter()
        .flat_map(|r| r["heard"].as_array().cloned().unwrap_or_default())
        .filter_map(|h| h.as_f64())
        .collect();
    if !heard.is_empty() {
        let exact = heard.iter().filter(|h| **h >= 0.999).count();
        out.push("### STT accuracy on the scripted lines".into());
        out.push(String::new());
        out.push(format!(
            "{exact}/{} lines word-perfect; mean word match {}%.",
            heard.len(),
            (heard.iter().sum::<f64>() / heard.len() as f64 * 100.0).round_ties_even() as i64
        ));
        out.push(String::new());
    }
    out
}

fn opening(runs: &[Value]) -> Vec<String> {
    let openings: Vec<Value> = runs
        .iter()
        .flat_map(records)
        .filter(|r| r["kind"] == "opening")
        .collect();
    if openings.is_empty() {
        return Vec::new();
    }
    let builds: Vec<f64> = openings
        .iter()
        .filter_map(|r| r["build"].as_f64())
        .collect();
    let replies: Vec<f64> = openings
        .iter()
        .filter_map(|r| r["reply"].as_f64())
        .collect();
    let wake: Vec<f64> = runs
        .iter()
        .filter_map(|r| r["wake_to_voice"].as_f64())
        .collect();
    vec![
        "### Opening (session build -> greeting voice)".into(),
        String::new(),
        format!(
            "build p50 {}s, pipeline start -> greeting voice p50 {}s (p95 {}), together p50 {}s \
             (the desk also plays the chime and ducks music before this).",
            f(percentile(&builds, 50.0), 2),
            f(percentile(&replies, 50.0), 2),
            f(percentile(&replies, 95.0), 2),
            f(percentile(&wake, 50.0), 2)
        ),
        String::new(),
    ]
}
