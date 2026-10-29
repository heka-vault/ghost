//! `ghostd latency`: where a turn's time went (ADR 0018), from the per-turn records the
//! voice pipeline appends to `latency.jsonl`: one JSON object per turn, timings, counts,
//! model ids and tool names only, never text. This is the reading side of
//! `ghost_voice.latency`: the records as the pipeline wrote them, the p50 and p95 per
//! stage, and one line per turn.

use std::io::Write;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Value, json};

/// The flat stages a summary reports, in the order the table shows them.
pub const STAGES: [&str; 15] = [
    "reply",
    "hear",
    "stt",
    "turn",
    "hold",
    "pre",
    "confirm",
    "llm",
    "llm_ttfb",
    "llm_calls",
    "tools",
    "tts",
    "tts_synth",
    "other",
    "barge_in",
];

pub const USAGE: &str =
    "usage: ghostd latency [--file PATH] [--last N] [--since N] [--origin desk|phone] [--json]";

/// Every record in `path` and its rotated generation, oldest first.
pub fn read_records(path: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    let rotated = path.with_file_name(format!(
        "{}.1",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    for p in [rotated, path.to_path_buf()] {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                out.push(v);
            }
        }
    }
    out
}

/// Nearest-rank-with-interpolation percentile, `q` in [0, 100].
pub fn percentile(values: &[f64], q: f64) -> Option<f64> {
    let mut xs: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    if xs.len() == 1 {
        return Some(xs[0]);
    }
    let k = (xs.len() - 1) as f64 * q / 100.0;
    let lo = k.floor() as usize;
    let hi = (lo + 1).min(xs.len() - 1);
    Some(xs[lo] + (xs[hi] - xs[lo]) * (k - lo as f64))
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn get<'a>(rec: &'a Value, key: &str) -> Option<&'a Value> {
    rec.get(key).filter(|v| !v.is_null())
}

/// Python's truth of a JSON value: null, false, 0, "" and empty containers are false.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn calls(rec: &Value) -> Vec<Value> {
    get(rec, "llm")
        .and_then(|l| l.get("calls"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The flat stages a summary reports for one record.
pub fn stage_values(rec: &Value) -> Vec<(&'static str, Option<f64>)> {
    let sub = |key: &str, field: &str| num(get(rec, key).and_then(|v| v.get(field)));
    let calls = calls(rec);
    vec![
        ("reply", num(get(rec, "reply"))),
        ("hear", sub("hear", "total")),
        ("stt", sub("hear", "stt")),
        ("turn", sub("hear", "turn")),
        ("hold", sub("hear", "hold")),
        ("pre", sub("hear", "pre")),
        ("confirm", sub("confirm", "total")),
        ("llm", sub("llm", "total")),
        ("llm_ttfb", calls.first().and_then(|c| num(c.get("ttfb")))),
        (
            "llm_calls",
            (!calls.is_empty()).then_some(calls.len() as f64),
        ),
        ("tools", sub("tools", "total")),
        ("tts", sub("tts", "total")),
        ("tts_synth", sub("tts", "synth")),
        ("other", num(get(rec, "other"))),
        ("barge_in", num(get(rec, "barge_in"))),
    ]
}

/// p50, p95 and count of one stage.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stat {
    pub n: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}

/// p50, p95 and count per stage, for the stages any record has.
pub fn summarize(records: &[Value]) -> IndexMap<&'static str, Stat> {
    let mut out = IndexMap::new();
    for stage in STAGES {
        let vals: Vec<f64> = records
            .iter()
            .filter_map(|r| {
                stage_values(r)
                    .into_iter()
                    .find(|(s, _)| *s == stage)
                    .and_then(|(_, v)| v)
            })
            .collect();
        if vals.is_empty() {
            continue;
        }
        out.insert(
            stage,
            Stat {
                n: vals.len(),
                p50: percentile(&vals, 50.0),
                p95: percentile(&vals, 95.0),
            },
        );
    }
    out
}

fn int(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_f64).map_or(0, |f| f as i64)
}

/// Share of input tokens read from the prompt cache across `calls`.
pub fn cache_share(calls: &[Value]) -> Option<f64> {
    let read: i64 = calls.iter().map(|c| int(c.get("cache_read"))).sum();
    let total: i64 = calls
        .iter()
        .map(|c| int(c.get("in")) + int(c.get("cache_read")) + int(c.get("cache_write")))
        .sum();
    (total != 0).then(|| read as f64 / total as f64)
}

fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

pub fn summary_table(records: &[Value], title: &str) -> Vec<String> {
    let s = summarize(records);
    let mut lines = vec![
        format!("{title} ({} turns)", records.len()),
        format!("  {:<10} {:>4} {:>7} {:>7}", "stage", "n", "p50", "p95"),
    ];
    for (stage, v) in &s {
        let cell = |x: Option<f64>| match x {
            Some(x) if *stage == "llm_calls" => format!("{x:7.0}"),
            Some(x) => format!("{x:7.2}"),
            None => "      -".to_string(),
        };
        lines.push(format!(
            "  {:<10} {:>4} {} {}",
            stage,
            v.n,
            cell(v.p50),
            cell(v.p95)
        ));
    }
    let all_calls: Vec<Value> = records.iter().flat_map(calls).collect();
    if let Some(share) = cache_share(&all_calls) {
        lines.push(format!(
            "  prompt cache: {} of input tokens read from cache",
            percent(share)
        ));
    }
    lines
}

/// A JSON scalar as Python's `str()` shows it.
fn text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
        Some(other) => other.to_string(),
    }
}

/// One log line per turn, timings only.
pub fn format_line(rec: &Value) -> String {
    let mut head = format!(
        "latency: turn {} {} {}",
        text(rec.get("turn")),
        text(rec.get("origin")),
        text(rec.get("kind"))
    );
    let reply = num(get(rec, "reply"));
    match reply {
        None => head.push_str(" no reply"),
        Some(r) => head.push_str(&format!(" reply {r:.2}s")),
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(hear) = get(rec, "hear").filter(|h| truthy(Some(h)))
        && let Some(total) = num(hear.get("total"))
    {
        let mut sub: Vec<String> = Vec::new();
        if let Some(stt) = num(hear.get("stt")) {
            sub.push(format!("stt {stt:.2}"));
        }
        if truthy(hear.get("pre")) {
            sub.push(format!("pre {:.2}", num(hear.get("pre")).unwrap_or(0.0)));
        }
        if truthy(hear.get("turn")) {
            sub.push(format!("turn {:.2}", num(hear.get("turn")).unwrap_or(0.0)));
        }
        if truthy(hear.get("hold")) {
            let kept = truthy(hear.get("speculation").and_then(|s| s.get("kept")));
            sub.push(format!(
                "hold {:.2}{}",
                num(hear.get("hold")).unwrap_or(0.0),
                if kept { ", answered during it" } else { "" }
            ));
        }
        parts.push(format!("hear {total:.2} ({})", sub.join(", ")));
    }
    if truthy(rec.get("intercepted")) {
        parts.push(format!("intercepted {}", text(rec.get("intercepted"))));
    }
    if let Some(c) = get(rec, "confirm").filter(|c| truthy(Some(c))) {
        parts.push(format!(
            "confirm {:.2} ({} {:.2})",
            num(c.get("total")).unwrap_or(0.0),
            text(c.get("classifier")),
            num(c.get("classify")).unwrap_or(0.0)
        ));
    }
    if let Some(llm) = get(rec, "llm").filter(|l| truthy(Some(l))) {
        let calls = calls(rec);
        let mut sub: Vec<String> = Vec::new();
        if let Some(first) = calls.first().and_then(|c| num(c.get("ttfb"))) {
            sub.push(format!("ttfb {first:.2}"));
        }
        sub.push(format!(
            "{} call{}",
            calls.len(),
            if calls.len() == 1 { "" } else { "s" }
        ));
        if let Some(share) = cache_share(&calls) {
            sub.push(format!("cache {}", percent(share)));
        }
        parts.push(format!(
            "llm {:.2} ({})",
            num(llm.get("total")).unwrap_or(0.0),
            sub.join(", ")
        ));
    }
    if let Some(tools) = get(rec, "tools").filter(|t| truthy(Some(t))) {
        let names: Vec<String> = tools
            .get("calls")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .map(|t| match num(t.get("secs")) {
                        Some(secs) => format!("{} {secs:.2}", text(t.get("name"))),
                        None => text(t.get("name")),
                    })
                    .collect()
            })
            .unwrap_or_default();
        parts.push(format!(
            "tools {:.2} ({})",
            num(tools.get("total")).unwrap_or(0.0),
            names.join(", ")
        ));
    }
    if let Some(tts) = get(rec, "tts").filter(|t| truthy(Some(t)))
        && let Some(total) = num(tts.get("total"))
    {
        parts.push(format!("tts {total:.2}"));
    }
    if let Some(other) = num(get(rec, "other"))
        && other.abs() >= 0.01
    {
        parts.push(format!("other {other:.2}"));
    }
    let mut line = head;
    if !parts.is_empty() {
        if reply.is_some() {
            line.push_str(" = ");
            line.push_str(&parts.join(" + "));
        } else {
            line.push_str(": ");
            line.push_str(&parts.join(", "));
        }
    }
    if let Some(cut) = num(get(rec, "barge_in")) {
        line.push_str(&format!(", cut off {cut:.2}s after the operator began"));
    }
    if let Some(flags) = get(rec, "flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        let names: Vec<String> = flags.iter().map(|f| text(Some(f))).collect();
        line.push_str(&format!(" [{}]", names.join(", ")));
    }
    line
}

struct Args {
    file: Option<PathBuf>,
    last: usize,
    since: usize,
    origin: Option<String>,
    json: bool,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        file: None,
        last: 10,
        since: 0,
        origin: None,
        json: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut std::slice::Iter<'_, String>| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--file" => a.file = Some(PathBuf::from(value(&mut it)?)),
            "--last" => {
                a.last = value(&mut it)?
                    .parse()
                    .map_err(|_| "--last takes a number".to_string())?;
            }
            "--since" => {
                a.since = value(&mut it)?
                    .parse()
                    .map_err(|_| "--since takes a number".to_string())?;
            }
            "--origin" => {
                let o = value(&mut it)?;
                if o != "desk" && o != "phone" {
                    return Err("--origin is desk or phone".to_string());
                }
                a.origin = Some(o);
            }
            "--json" => a.json = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn origin_of(r: &Value) -> String {
    text(r.get("origin"))
}

/// `ghostd latency [--last N] [--file PATH] [--origin desk|phone] [--json]`: p50/p95
/// per stage from the per-turn records, overall and per origin, then the last N turns.
/// `default_path` is where the daemon writes them. The exit code.
pub fn latency_command(
    args: &[String],
    default_path: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let a = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(err, "{USAGE}\n{e}");
            return 2;
        }
    };
    let path = a.file.unwrap_or_else(|| default_path.to_path_buf());
    let mut records = read_records(&path);
    if let Some(origin) = &a.origin {
        records.retain(|r| r.get("origin").and_then(Value::as_str) == Some(origin));
    }
    if a.since > 0 && records.len() > a.since {
        records = records.split_off(records.len() - a.since);
    }
    if records.is_empty() {
        let _ = writeln!(out, "no latency records in {}", path.display());
        return 1;
    }
    let mut origins: Vec<String> = records.iter().map(origin_of).collect();
    origins.sort();
    origins.dedup();
    if a.json {
        let mut doc: IndexMap<String, IndexMap<&'static str, Stat>> = IndexMap::new();
        doc.insert("all".to_string(), summarize(&records));
        for o in &origins {
            let sub: Vec<Value> = records
                .iter()
                .filter(|r| origin_of(r) == *o)
                .cloned()
                .collect();
            doc.insert(o.clone(), summarize(&sub));
        }
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json!(doc)).unwrap_or_default()
        );
        return 0;
    }
    let _ = writeln!(
        out,
        "{}",
        summary_table(&records, &format!("all turns in {}", path.display())).join("\n")
    );
    if origins.len() > 1 {
        for origin in &origins {
            let sub: Vec<Value> = records
                .iter()
                .filter(|r| origin_of(r) == *origin)
                .cloned()
                .collect();
            let _ = writeln!(out, "\n{}", summary_table(&sub, origin).join("\n"));
        }
    }
    for kind in ["voice", "text", "opening"] {
        let sub: Vec<Value> = records
            .iter()
            .filter(|r| r.get("kind").and_then(Value::as_str) == Some(kind))
            .cloned()
            .collect();
        if !sub.is_empty() && sub.len() != records.len() {
            let _ = writeln!(
                out,
                "\n{}",
                summary_table(&sub, &format!("{kind} turns")).join("\n")
            );
        }
    }
    if a.last > 0 {
        let n = a.last.min(records.len());
        let _ = writeln!(out, "\nlast {n} turns:");
        for r in &records[records.len() - n..] {
            let ts = r.get("ts").and_then(Value::as_str).unwrap_or("");
            let clock: String = ts.chars().skip(11).take(8).collect();
            let line = format_line(r);
            let _ = writeln!(
                out,
                "  {clock} {}",
                line.strip_prefix("latency: ").unwrap_or(&line)
            );
        }
    }
    0
}

#[cfg(test)]
mod tests {
    //! The latency harness's model-free parts and `ghostd latency`.

    use super::*;

    /// A voice turn's record as the pipeline writes it (`tests/test_perf.py`).
    pub(crate) fn voice_record(
        turn: i64,
        reply: f64,
        tools: &[&str],
        status: Option<&str>,
    ) -> Value {
        let calls: Vec<Value> = tools
            .iter()
            .map(|t| json!({"name": t, "secs": 0.01, "status": status}))
            .collect();
        json!({
            "turn": turn,
            "kind": "voice",
            "origin": "desk",
            "spoke": true,
            "reply": reply,
            "hear": {
                "total": 1.2, "vad": 0.2, "stt": 0.15, "pre": 0.0, "turn": 0.05,
                "hold": 0.8, "segments": 1, "kept_open": 0,
            },
            "llm": {
                "total": reply - 1.6,
                "calls": [{
                    "ttfb": 1.0, "dur": 1.1, "in": 100, "cache_read": 3000,
                    "cache_write": 0, "out": 40, "tools": tools,
                }],
            },
            "tools": if calls.is_empty() { Value::Null } else { json!({"total": 0.01, "calls": calls}) },
            "tts": {"total": 0.4, "synth": 0.35},
            "other": 0.0,
        })
    }

    #[test]
    fn latency_command_summarizes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latency.jsonl");
        let lines: Vec<String> = (0..5)
            .map(|i| {
                let mut r = voice_record(i, 2.0 + i as f64 / 10.0, &[], None);
                r["ts"] = json!("2026-10-19T17:00:00Z");
                r.to_string()
            })
            .collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let args: Vec<String> = ["--file", path.to_str().unwrap(), "--last", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            latency_command(&args, Path::new("/dev/null"), &mut out, &mut err),
            0
        );
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("reply") && out.contains("p50") && out.contains("last 2 turns"));
        assert!(out.contains("prompt cache: 97% of input tokens read from cache"));
        assert!(out.contains("17:00:00 turn 4 desk voice reply 2.40s = hear 1.20"));
        let missing = dir.path().join("missing.jsonl");
        let args = vec!["--file".to_string(), missing.to_string_lossy().into_owned()];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            latency_command(&args, Path::new("/dev/null"), &mut out, &mut err),
            1
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("no latency records in")
        );
    }

    #[test]
    fn a_line_reads_as_the_pipeline_logged_it() {
        let rec = voice_record(2, 3.0, &["list_awaiting_signoff"], Some("ok"));
        assert_eq!(
            format_line(&rec),
            "latency: turn 2 desk voice reply 3.00s = hear 1.20 (stt 0.15, turn 0.05, hold 0.80) \
             + llm 1.40 (ttfb 1.00, 1 call, cache 97%) + tools 0.01 (list_awaiting_signoff 0.01) + tts 0.40"
        );
        let opening = json!({"turn": 1, "kind": "opening", "origin": "phone", "spoke": true,
            "tts": {"total": 0.3}, "barge_in": 1.5, "flags": ["cut_off"]});
        assert_eq!(
            format_line(&opening),
            "latency: turn 1 phone opening no reply: tts 0.30, cut off 1.50s after the operator began [cut_off]"
        );
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 50.0), Some(2.5));
        assert_eq!(percentile(&[], 50.0), None);
        let table = summary_table(&[rec], "t");
        assert_eq!(table[0], "t (1 turns)");
        assert_eq!(table[1], "  stage         n     p50     p95");
        assert_eq!(table[2], "  reply         1    3.00    3.00");
        assert!(
            table
                .iter()
                .any(|l| l == "  llm_calls     1       1       1")
        );
    }
}
