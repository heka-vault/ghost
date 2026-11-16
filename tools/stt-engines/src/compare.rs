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
fn rtf(numbers: &Value, results: &Results) -> (Option<f64>, Option<f64>) {
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
            let secs = t["read_secs"]
                .as_f64()
                .or_else(|| Some(t["feed_secs"].as_f64()? + t["finalize_secs"].as_f64()?))?;
            Some(secs / t["audio_secs"].as_f64()?)
        })
        .collect();
    (quantile(&xs, 0.5), quantile(&xs, 0.95))
}

fn f(x: Option<f64>, digits: usize) -> String {
    x.map_or_else(|| "-".into(), |v| format!("{v:.digits$}"))
}

pub fn compare(baseline: &Path, runs: &Path, bmode: &str) -> anyhow::Result<()> {
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
        let s = sum(&r, "file");
        let delta = paired(&base, bmode, &r, "file").map_or("-".into(), |(dl, lo, hi, v)| {
            format!(
                "{:+.1} pt ({:+.1} to {:+.1}), {v}",
                dl * 100.0,
                lo * 100.0,
                hi * 100.0
            )
        });
        let (p50, p95) = rtf(&n, &r);
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

/// Arrival times against the clock: when the first one and two words show, after they
/// were said, and the work left at the close; one line per streaming run.
pub fn arrivals(clock: &Path, runs: &[&Path]) -> anyhow::Result<()> {
    let clock: BTreeMap<String, Value> = serde_json::from_str(&std::fs::read_to_string(clock)?)?;
    println!(
        "| run | takes | first word shown after said, p50 / p95 s | two words, p50 / p95 s | first two right | feed s/s p50 / p95 | max frame ms p95 | work left at close p50 / p95 s | updates rewriting shown words | takes with a rewrite | finalize rewrote shown |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for run in runs {
        let n: Value = serde_json::from_str(&std::fs::read_to_string(run.join("numbers.json"))?)?;
        let mut l1 = Vec::new();
        let mut l2 = Vec::new();
        let (mut right, mut two) = (0, 0);
        let (mut feed, mut maxf, mut left) = (Vec::new(), Vec::new(), Vec::new());
        let (mut rw, mut changes, mut takes_rw, mut fin_rw, mut spoken) = (0u64, 0u64, 0, 0, 0);
        for t in n["takes"].as_array().into_iter().flatten() {
            let id = t["id"].as_str().unwrap_or("");
            let Some(c) = clock.get(id) else { continue }; // silent takes have no clock
            if t.get("spoken").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            spoken += 1;
            let a = &t["arrival"];
            if let (Some(at), Some(end)) = (a["first1"].as_f64(), c["end1"].as_f64()) {
                l1.push(at - end);
            }
            if let (Some(at), Some(end)) = (a["first2"].as_f64(), c["end2"].as_f64()) {
                l2.push(at - end);
            }
            if let Some(r) = a["first2_right"].as_bool() {
                two += 1;
                right += usize::from(r);
            }
            if let (Some(fs), Some(au)) = (t["feed_secs"].as_f64(), t["audio_secs"].as_f64()) {
                feed.push(fs / au);
            }
            if let Some(m) = t["max_frame_secs"].as_f64() {
                maxf.push(m * 1e3);
            }
            if let (Some(b), Some(fz)) = (t["backlog_secs"].as_f64(), t["finalize_secs"].as_f64()) {
                left.push(b + fz);
            }
            rw += t["rewrites"].as_u64().unwrap_or(0);
            changes += t["changes"].as_u64().unwrap_or(0);
            takes_rw += usize::from(t["rewrites"].as_u64().unwrap_or(0) > 0);
            fin_rw += usize::from(t["final_rewrote_shown"].as_bool().unwrap_or(false));
        }
        println!(
            "| {} | {} | {} / {} (n {}) | {} / {} (n {}) | {}/{} | {} / {} | {} | {} / {} | {}/{} | {}/{} | {}/{} |",
            run.file_name().unwrap().to_string_lossy(),
            spoken,
            f(quantile(&l1, 0.5), 2),
            f(quantile(&l1, 0.95), 2),
            l1.len(),
            f(quantile(&l2, 0.5), 2),
            f(quantile(&l2, 0.95), 2),
            l2.len(),
            right,
            two,
            f(quantile(&feed, 0.5), 3),
            f(quantile(&feed, 0.95), 3),
            f(quantile(&maxf, 0.95), 0),
            f(quantile(&left, 0.5), 3),
            f(quantile(&left, 0.95), 3),
            rw,
            changes,
            takes_rw,
            spoken,
            fin_rw,
            spoken,
        );
    }
    Ok(())
}
