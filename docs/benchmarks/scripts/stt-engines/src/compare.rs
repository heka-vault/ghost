//! Every run's `results.json` against the baseline's `file` read, take by take, with the
//! hear harness's paired bootstrap (4,000 resamples, seed 60), and the timings beside.

use std::collections::BTreeMap;
use std::path::Path;

use ghost_voice::perf::hear::plan::Kind;
use ghost_voice::perf::hear::score::{Class, paired_delta};
use ghost_voice::perf::hear::{Read, Results, TakeResult, read_results};
use serde_json::Value;

use crate::common::quantile;

struct Sum {
    wer: f64,
    errors: usize,
    words: usize,
    class: [(usize, usize); 4],
    terms_said: usize,
    terms_heard: usize,
    terms_inserted: usize,
    empty: usize,
    spoken: usize,
    silent_with_words: usize,
    silent: usize,
}

fn read<'a>(t: &'a TakeResult, mode: &str) -> Option<&'a Read> {
    t.modes.get(mode)
}

fn sum(r: &Results, mode: &str) -> Sum {
    let mut s = Sum {
        wer: 0.0,
        errors: 0,
        words: 0,
        class: [(0, 0); 4],
        terms_said: 0,
        terms_heard: 0,
        terms_inserted: 0,
        empty: 0,
        spoken: 0,
        silent_with_words: 0,
        silent: 0,
    };
    for t in &r.takes {
        let Some(x) = read(t, mode) else { continue };
        if t.kind == Kind::Silence {
            s.silent += 1;
            s.silent_with_words += usize::from(x.words > 0);
            continue;
        }
        s.spoken += 1;
        s.errors += x.errors();
        s.words += t.ref_words;
        s.empty += usize::from(x.words == 0);
        for c in Class::ALL {
            let k = x.class(c);
            s.class[c as usize].0 += k[1] + k[2] + k[3];
            s.class[c as usize].1 += k[0];
        }
        s.terms_said += x.terms_said;
        s.terms_heard += x.terms_heard;
        s.terms_inserted += x.terms_inserted;
    }
    s.wer = s.errors as f64 / s.words.max(1) as f64;
    s
}

fn paired(
    base: &Results,
    bmode: &str,
    cand: &Results,
    cmode: &str,
) -> Option<(f64, f64, f64, &'static str)> {
    let by: BTreeMap<&str, &TakeResult> = base.takes.iter().map(|t| (t.id.as_str(), t)).collect();
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for t in cand.takes.iter().filter(|t| t.kind != Kind::Silence) {
        let (Some(bt), Some(cr)) = (by.get(t.id.as_str()), read(t, cmode)) else {
            continue;
        };
        let Some(br) = read(bt, bmode) else { continue };
        a.push((br.errors() as f64, bt.ref_words as f64));
        b.push((cr.errors() as f64, t.ref_words as f64));
    }
    if a.is_empty() {
        return None;
    }
    let d = paired_delta(&a, &b, 4000, 60);
    Some((d.delta, d.low, d.high, d.verdict().word()))
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// Model seconds per second of audio over the spoken takes, (p50, p95).
fn rtf(numbers: &Value, results: &Results, mode: &str) -> (Option<f64>, Option<f64>) {
    let (read_key, audio_key) = if mode == "turn" {
        ("turn_read_secs", "turn_audio_secs")
    } else {
        ("read_secs", "audio_secs")
    };
    let spoken: BTreeMap<&str, bool> = results
        .takes
        .iter()
        .map(|t| (t.id.as_str(), t.kind != Kind::Silence))
        .collect();
    let xs: Vec<f64> = numbers["takes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            spoken
                .get(t["id"].as_str().unwrap_or(""))
                .copied()
                .unwrap_or(false)
        })
        .filter_map(|t| {
            let secs = t[read_key]
                .as_f64()
                .or_else(|| Some(t["feed_secs"].as_f64()? + t["finalize_secs"].as_f64()?))?;
            Some(secs / t[audio_key].as_f64()?)
        })
        .collect();
    (quantile(&xs, 0.5), quantile(&xs, 0.95))
}

fn f(x: Option<f64>, digits: usize) -> String {
    x.map_or_else(|| "-".into(), |v| format!("{v:.digits$}"))
}

pub fn compare(baseline: &Path, runs: &Path, bmode: &str, cmode: &str) -> anyhow::Result<()> {
    let base = read_results(baseline)?;
    let b = sum(&base, bmode);
    println!(
        "baseline {bmode} read ({}): WER {} ({} errors / {} words)\n",
        baseline.display(),
        pct(b.wer),
        b.errors,
        b.words
    );
    println!(
        "| run | WER | Δ vs baseline (95% CI) | numbers wrong | term errors | terms heard | empty reads | silent w/ words | model s/s p50 | p95 | load s | load + first read s | RSS after load MB | RSS after reads MB | peak MB |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut dirs: Vec<_> = std::fs::read_dir(runs)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("results.json").is_file())
        .collect();
    dirs.sort();
    for d in dirs {
        let r = read_results(&d.join("results.json"))?;
        let n: Value = serde_json::from_str(&std::fs::read_to_string(d.join("numbers.json"))?)?;
        let s = sum(&r, cmode);
        if s.spoken == 0 {
            continue; // no such read in this run
        }
        let delta = paired(&base, bmode, &r, cmode).map_or("-".into(), |(dl, lo, hi, v)| {
            format!(
                "{:+.1} pt ({:+.1} to {:+.1}), {v}",
                dl * 100.0,
                lo * 100.0,
                hi * 100.0
            )
        });
        let (p50, p95) = rtf(&n, &r, cmode);
        println!(
            "| {} | {} | {} | {}/{} | {}/{} | {}/{} (+{} made up) | {}/{} | {}/{} | {} | {} | {} | {} | {} | {} | {} |",
            d.file_name().unwrap().to_string_lossy(),
            pct(s.wer),
            delta,
            s.class[Class::Number as usize].0,
            s.class[Class::Number as usize].1,
            s.class[Class::Term as usize].0,
            s.class[Class::Term as usize].1,
            s.terms_heard,
            s.terms_said,
            s.terms_inserted,
            s.empty,
            s.spoken,
            s.silent_with_words,
            s.silent,
            f(p50, 3),
            f(p95, 3),
            f(n["load_secs"].as_f64(), 2),
            f(
                n["load_secs"]
                    .as_f64()
                    .zip(n["warmup_read_secs"].as_f64())
                    .map(|(a, b)| a + b),
                2
            ),
            f(n["rss_mb_after_load"].as_f64(), 0),
            f(n["rss_mb_after_reads"].as_f64(), 0),
            f(n["hwm_mb"].as_f64(), 0),
        );
    }
    Ok(())
}

/// When a take's first one and two words were shown, minus when they were said (the clock).
/// A stream's take whose words came only at finalize arrives at finalize: the end of the
/// audio, the backlog, then `finalize`. A take whose read has no words never arrives.
fn lags(
    numbers: &Value,
    results: Option<&Results>,
    clock: &BTreeMap<String, Value>,
) -> BTreeMap<String, [Option<(f64, bool)>; 2]> {
    let words: BTreeMap<&str, usize> = results
        .map(|r| {
            r.takes
                .iter()
                .filter_map(|t| Some((t.id.as_str(), t.modes.get("file")?.words)))
                .collect()
        })
        .unwrap_or_default();
    let mut out = BTreeMap::new();
    for t in numbers["takes"].as_array().into_iter().flatten() {
        let id = t["id"].as_str().unwrap_or("");
        let Some(c) = clock.get(id) else { continue };
        if t.get("spoken").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let a = &t["arrival"];
        let at_finalize = || -> Option<f64> {
            Some(
                t["audio_secs"].as_f64()?
                    + t["backlog_secs"].as_f64()?
                    + t["finalize_secs"].as_f64()?,
            )
        };
        let mut lag = [None, None];
        for (k, (key, end)) in [("first1", "end1"), ("first2", "end2")].iter().enumerate() {
            let Some(end) = c[*end].as_f64() else {
                continue;
            };
            lag[k] = match a[*key].as_f64() {
                Some(at) => Some((at - end, false)),
                None if words.get(id).copied().unwrap_or(0) > k => {
                    at_finalize().map(|at| (at - end, true))
                }
                None => None,
            };
        }
        out.insert(id.to_string(), lag);
    }
    out
}

fn p50_95(xs: &[f64]) -> String {
    format!("{} / {}", f(quantile(xs, 0.5), 2), f(quantile(xs, 0.95), 2))
}

/// Each streaming run's arrival (finalize counted) beside Ghost's interim re-read, on the
/// takes both have, plus the rest of the stream's measures.
pub fn arrivals(clock: &Path, interim: &Path, runs: &[&Path]) -> anyhow::Result<()> {
    let clock: BTreeMap<String, Value> = serde_json::from_str(&std::fs::read_to_string(clock)?)?;
    let iv: Value = serde_json::from_str(&std::fs::read_to_string(interim.join("numbers.json"))?)?;
    let il = lags(&iv, None, &clock);
    println!(
        "| run | first word: all takes p50 / p95 (n, at finalize) | first word, common takes: stream / Ghost interim p50 (n) | two words: all takes p50 / p95 (n, at finalize) | two words, common takes: stream / Ghost interim p50 (n) | stream later than interim, two words (takes) | finalize changed shown words incl. last / before last |"
    );
    println!("|---|---|---|---|---|---|---|");
    for run in runs {
        let n: Value = serde_json::from_str(&std::fs::read_to_string(run.join("numbers.json"))?)?;
        let r = read_results(&run.join("results.json"))?;
        let sl = lags(&n, Some(&r), &clock);
        let mut cells = Vec::new();
        let mut later = 0;
        for k in 0..2 {
            let all: Vec<f64> = sl.values().filter_map(|l| l[k].map(|x| x.0)).collect();
            let fin = sl.values().filter(|l| l[k].is_some_and(|x| x.1)).count();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for (id, l) in &sl {
                if let (Some(x), Some(y)) = (l[k], il.get(id).and_then(|l| l[k])) {
                    a.push(x.0);
                    b.push(y.0);
                    if k == 1 && x.0 > y.0 {
                        later += 1;
                    }
                }
            }
            cells.push(format!(
                "{} (n {}, {} at finalize)",
                p50_95(&all),
                all.len(),
                fin
            ));
            cells.push(format!(
                "{} / {} (n {})",
                f(quantile(&a, 0.5), 2),
                f(quantile(&b, 0.5), 2),
                a.len()
            ));
        }
        let (mut strict, mut loose) = (0, 0);
        for t in n["takes"].as_array().into_iter().flatten() {
            strict += usize::from(t["final_rewrote_shown_strict"].as_bool().unwrap_or(false));
            loose += usize::from(t["final_rewrote_shown"].as_bool().unwrap_or(false));
        }
        println!(
            "| {} | {} | {} | {} | {} | {} | {} / {} |",
            run.file_name().unwrap().to_string_lossy(),
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            later,
            strict,
            loose
        );
    }
    let all1: Vec<f64> = il.values().filter_map(|l| l[0].map(|x| x.0)).collect();
    let all2: Vec<f64> = il.values().filter_map(|l| l[1].map(|x| x.0)).collect();
    println!(
        "\nGhost interim, all takes: first word {} (n {}), two words {} (n {})",
        p50_95(&all1),
        all1.len(),
        p50_95(&all2),
        all2.len()
    );
    Ok(())
}
