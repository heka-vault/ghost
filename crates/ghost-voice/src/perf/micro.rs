//! Component micro-benchmarks (`ghostd.perf.micro`): STT by utterance length, Kokoro
//! time to first audio by sentence length, Smart Turn inference, and the Jev
//! confirmation classifier when a key is set.
//!
//! Runs on the same CPU as everything else (and as a running ghostd, if there is one);
//! the numbers are what a session sees under that load, not a clean-room figure.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use ghost_core::ConfirmationContext;
use ghost_core::jev::{Jev, JevClient, JevOptions};
use ghost_infer::SmartTurn;
use ghost_infer::clauses::ClauseAggregator;
use serde_json::{Map, Value, json};

use super::audio::{IN_RATE, Voice};
use crate::config::VoiceSettings;
use crate::pipeline::Models;
use crate::stt::recognize;

pub const STT_LINES: [(&str, &str); 4] = [
    ("yes (1 word)", "Yes."),
    ("short (4 words)", "Close infra seventeen."),
    (
        "medium (9 words)",
        "What's going on with app one forty two today?",
    ),
    (
        "long (22 words)",
        "What's going on with app one forty two, and what have people said about it in the comments since last week, roughly?",
    ),
];
pub const TTS_LINES: [(&str, &str); 4] = [
    ("3 words", "Labeled it bug."),
    (
        "8 words",
        "Three issues are waiting for your sign-off today.",
    ),
    (
        "15 words",
        "Issue one forty two asks whether to use Redis or Postgres for the session cache.",
    ),
    (
        "30 words",
        "Issue one forty two asks whether to use Redis or Postgres for the session cache, and it blocks two others, so it is probably the one to decide first before the launch next week.",
    ),
];
pub const JEV_LINES: [&str; 4] = ["Yes, go ahead.", "Sure.", "No, wait.", "Okay, do it."];

pub fn stats(xs: &[f64]) -> Map<String, Value> {
    let mut xs: Vec<f64> = xs.to_vec();
    xs.sort_by(f64::total_cmp);
    let n = xs.len();
    let p50 = if n == 0 {
        0.0
    } else if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    };
    let p95 = xs
        .get(((0.95 * (n.saturating_sub(1)) as f64).round() as usize).min(n.saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    let mut m = Map::new();
    m.insert("p50".into(), json!(p50));
    m.insert("p95".into(), json!(p95));
    m.insert("min".into(), json!(xs.first().copied().unwrap_or(0.0)));
    m
}

fn num(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

pub async fn run_micro(only: &[String], n: usize, out: &Path, cache: &Path) -> anyhow::Result<i32> {
    let settings = VoiceSettings::from_env()?;
    let mut results = Map::new();
    results.insert("n".into(), json!(n));
    results.insert(
        "cpus".into(),
        json!(
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1)
        ),
    );
    let mut lines: Vec<String> = Vec::new();
    let wants = |k: &str| only.iter().any(|o| o == k);
    let models = if wants("stt") || wants("tts") || wants("turn") {
        Some(Arc::new(Models::load(&settings)?))
    } else {
        None
    };
    let voice = models
        .as_ref()
        .map(|m| Voice::new(m.tts.clone(), &cache.join("utterances")));

    if wants("stt")
        && let (Some(m), Some(v)) = (&models, &voice)
    {
        lines.push(format!(
            "STT (Parakeet v3 int8, {} threads): seconds per utterance",
            settings.stt_threads
        ));
        let mut stt = Map::new();
        for (label, text) in STT_LINES {
            let u = v.render(text)?;
            let mut pcm = ghost_infer::audio::to_f32(&u.pcm);
            pcm.extend(std::iter::repeat_n(
                0.0,
                (0.2 * f64::from(IN_RATE)) as usize,
            ));
            let mut times = Vec::new();
            for _ in 0..n {
                let t = Instant::now();
                let _ = recognize(&m.stt, &pcm, true);
                times.push(t.elapsed().as_secs_f64());
            }
            let mut s = stats(&times);
            s.insert("audio".into(), json!(u.seconds()));
            lines.push(format!(
                "  {label:<18} audio {:4.1}s  p50 {:.3}  p95 {:.3}",
                u.seconds(),
                num(&Value::Object(s.clone()), "p50"),
                num(&Value::Object(s.clone()), "p95")
            ));
            stt.insert(label.into(), Value::Object(s));
        }
        results.insert("stt".into(), Value::Object(stt));
    }

    if wants("tts")
        && let Some(m) = &models
    {
        lines.push(format!(
            "TTS (Kokoro, voice {}): time to first audio / whole, a clause at a time",
            settings.tts_voice
        ));
        let mut tts = Map::new();
        for (label, text) in TTS_LINES {
            let (mut first, mut whole, mut audio) = (Vec::new(), Vec::new(), 0.0);
            for _ in 0..n {
                let t = Instant::now();
                let mut got_first = None;
                let mut total = 0usize;
                let mut agg = ClauseAggregator::new(settings.clause_min_words());
                let mut pieces = agg.push(text);
                pieces.extend(agg.flush());
                for piece in pieces {
                    let pcm = m.tts.lock().unwrap_or_else(|e| e.into_inner()).synthesize(
                        &piece,
                        &settings.tts_voice,
                        settings.tts_speed as f32,
                    )?;
                    if got_first.is_none() {
                        got_first = Some(t.elapsed().as_secs_f64());
                    }
                    total += pcm.len();
                }
                audio = total as f64 / 24_000.0;
                first.push(got_first.unwrap_or(0.0));
                whole.push(t.elapsed().as_secs_f64());
            }
            let (fs, ws) = (stats(&first), stats(&whole));
            lines.push(format!(
                "  {label:<10} audio {audio:4.1}s  first p50 {:.3} p95 {:.3}  whole p50 {:.3}",
                num(&Value::Object(fs.clone()), "p50"),
                num(&Value::Object(fs.clone()), "p95"),
                num(&Value::Object(ws.clone()), "p50")
            ));
            tts.insert(
                label.into(),
                json!({"first": fs, "whole": ws, "audio": audio}),
            );
        }
        results.insert("tts".into(), Value::Object(tts));
        lines.push("TTS (ElevenLabs): not measured here (the Python harness's elevenlabs micro; ADR 0026 proposed)".into());
    }

    if wants("turn")
        && let (Some(_m), Some(v)) = (&models, &voice)
    {
        lines.push("Smart Turn v3: inference on the last 8 s of an utterance".into());
        let u = v.render(STT_LINES[2].1)?;
        let pcm = ghost_infer::audio::to_f32(&u.pcm);
        let mut turn = SmartTurn::load(&settings.smart_turn_model, settings.smart_turn_threads)?;
        let mut times = Vec::new();
        for _ in 0..n {
            let t = Instant::now();
            let _ = turn.predict(&SmartTurn::last_eight_seconds(&pcm))?;
            times.push(t.elapsed().as_secs_f64());
        }
        let s = stats(&times);
        lines.push(format!(
            "  p50 {:.3}  p95 {:.3}",
            num(&Value::Object(s.clone()), "p50"),
            num(&Value::Object(s.clone()), "p95")
        ));
        results.insert("turn".into(), Value::Object(s));
    }

    if wants("jev") {
        let env: HashMap<String, String> = std::env::vars().collect();
        match JevClient::from_env("auto", &env, JevOptions::default()) {
            Ok(Some(jev)) => {
                let ctx = ConfirmationContext {
                    pending_action:
                        "On ghost-mock/infra#17 \"Rotate the Discord webhook\": close the issue"
                            .into(),
                    question: "Okay to close infra 17?".into(),
                };
                let (mut times, mut errors) = (Vec::new(), 0);
                for i in 0..(n * JEV_LINES.len()) {
                    let t = Instant::now();
                    if jev
                        .classify_utterance(JEV_LINES[i % JEV_LINES.len()], &ctx)
                        .await
                        .is_err()
                    {
                        errors += 1;
                    }
                    times.push(t.elapsed().as_secs_f64());
                }
                let mut s = stats(&times);
                s.insert("errors".into(), json!(errors));
                s.insert("via".into(), json!(jev.provider()));
                let over = times.iter().filter(|x| **x > 0.5).count();
                lines.push(format!(
                    "Jev via {}: p50 {:.3}  p95 {:.3}  over the 0.5 s deadline {over}/{}  errors {errors}",
                    jev.provider(),
                    num(&Value::Object(s.clone()), "p50"),
                    num(&Value::Object(s.clone()), "p95"),
                    times.len()
                ));
                results.insert("jev".into(), Value::Object(s));
            }
            Ok(None) => lines.push("Jev: no key, skipped".into()),
            Err(e) => lines.push(format!("Jev: {e}")),
        }
    }

    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("micro.json"),
        serde_json::to_string_pretty(&Value::Object(results))?,
    )?;
    std::fs::write(out.join("micro.txt"), lines.join("\n") + "\n")?;
    println!("{}", lines.join("\n"));
    println!("\nmicro: {}", out.join("micro.txt").display());
    Ok(0)
}
