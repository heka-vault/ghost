//! `ghost-perf realtime` (`make perf-turns REALTIME_KEY_ENV=VAR`): the realtime voice
//! against a real provider (ADR 0062), with spoken lines rendered by Kokoro and streamed at
//! the pace of speech, as the microphone would.
//!
//! For each line it measures the first sound (the end of the line's speech to the
//! provider's first audio), and for a lookup the time to the consult and, once a fixed
//! answer is handed back, the time to the relay's first audio and whether the reading
//! matched the answer word for word. The agent's own time is not here: the main harness
//! (`make perf`) measures it, and a lookup's full reply is about the consult, plus the
//! agent, plus the relay's start.
//!
//! The key is read from the environment variable named with `--key-env` and nowhere else:
//! not the vault, never printed, never written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::realtime::openai::OpenAiRealtime;
use crate::realtime::relay::{Outcome, RelayCheck};
use crate::realtime::settings::OPENAI;
use crate::realtime::{CONSULT, Event, RealtimeProvider, setup};

pub const USAGE: &str = "usage: ghost-perf realtime --key-env VAR [--model M] [--voice V] [--n N] [--url URL] [--out DIR]";

/// Lines the operator might say: small talk the voice answers itself, lookups it hands on.
pub const LINES: &[(&str, bool)] = &[
    ("Hey, how's it going today?", false),
    ("What issues are open in the ghost repo?", true),
    ("Thanks, that's helpful.", false),
    ("Is anything on my calendar tomorrow morning?", true),
];
/// The answer handed back for every lookup, as the agent would.
pub const ANSWER: &str = "Three issues are open: 412 about the wake word, 415 about the phone, and 420 about the release notes.";

pub struct Args {
    pub key_env: String,
    pub model: String,
    pub voice: String,
    pub n: usize,
    pub url: String,
    pub out: PathBuf,
}

pub fn parse(argv: &[String], out: &Path) -> Result<Args, String> {
    let mut a = Args {
        key_env: String::new(),
        model: OPENAI.default_model.into(),
        voice: OPENAI.default_voice.into(),
        n: 2,
        url: OPENAI.url.into(),
        out: out.to_path_buf(),
    };
    let mut it = argv.iter();
    while let Some(k) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{k} wants a value; {USAGE}"))
        };
        match k.as_str() {
            "--key-env" => a.key_env = v()?,
            "--model" => a.model = v()?,
            "--voice" => a.voice = v()?,
            "--n" => a.n = v()?.parse().map_err(|_| USAGE.to_string())?,
            "--url" => a.url = v()?,
            "--out" => a.out = PathBuf::from(v()?),
            other => return Err(format!("unknown {other:?}; {USAGE}")),
        }
    }
    if a.key_env.is_empty() {
        return Err(format!("name the variable that holds the key; {USAGE}"));
    }
    Ok(a)
}

fn secs(a: Instant, b: Option<Instant>) -> Value {
    b.map_or(Value::Null, |b| {
        json!((b.saturating_duration_since(a).as_secs_f64() * 1000.0).round() / 1000.0)
    })
}

/// One line, spoken into the session, and what came back.
async fn one(c: &mut crate::realtime::Connection, pcm: &[i16], text: &str, lookup: bool) -> Value {
    // Speech at its own pace, then quiet, as a microphone sends it.
    let started = Instant::now();
    for (i, chunk) in pcm.chunks(320).enumerate() {
        c.link.audio(chunk);
        let due = started + Duration::from_millis(20 * (i as u64 + 1));
        tokio::time::sleep_until(due.into()).await;
    }
    let spoke = Instant::now();
    let silence = vec![0i16; 320];
    let mut first_audio = None;
    let mut consult = None;
    let mut relay_asked = None;
    let mut relay_audio = None;
    let mut relay = RelayCheck::new(ANSWER);
    let mut relay_id: Option<String> = None;
    let mut outcome = Value::Null;
    let mut quiet = tokio::time::interval(Duration::from_millis(20));
    let deadline = spoke + Duration::from_secs(25);
    loop {
        tokio::select! {
            _ = quiet.tick() => {
                c.link.audio(&silence);
                if Instant::now() > deadline { break; }
            }
            e = c.events.recv() => {
                let Some(e) = e else { break };
                match e {
                    Event::Started { response, tag } => {
                        if tag.is_some() { relay_id = Some(response); }
                    }
                    Event::Audio { response, .. } => {
                        let now = Some(Instant::now());
                        if first_audio.is_none() { first_audio = now; }
                        if relay_id.as_deref() == Some(response.as_str()) && relay_audio.is_none() { relay_audio = now; }
                    }
                    Event::Said { response, delta } if relay_id.as_deref() == Some(response.as_str()) => {
                        relay.feed(&delta);
                    }
                    Event::SaidAll { response, text } if relay_id.as_deref() == Some(response.as_str()) => {
                        outcome = json!(match relay.finish(Some(&text)) {
                            Outcome::Verified => "verified",
                            Outcome::Incomplete { .. } => "incomplete",
                            Outcome::Diverged { .. } => "diverged",
                        });
                    }
                    Event::Call { call_id, name, .. } if name == CONSULT => {
                        consult = Some(Instant::now());
                        c.link.answer(&call_id, crate::realtime::stage::ANSWERED);
                    }
                    Event::Done { response, .. } => {
                        if relay_id.as_deref() == Some(response.as_str()) {
                            break;
                        }
                        if consult.is_some() && relay_asked.is_none() {
                            relay_asked = Some(Instant::now());
                            c.link.say(ANSWER, "perf-relay");
                        } else if consult.is_none() {
                            break; // small talk, or a lookup it answered itself
                        }
                    }
                    Event::Failed(f) => {
                        return json!({"line": text, "error": f.to_string()});
                    }
                    _ => {}
                }
            }
        }
    }
    json!({
        "line": text,
        "lookup": lookup,
        "first_sound": secs(spoke, first_audio),
        "consult": secs(spoke, consult),
        "relay_start": relay_asked.map_or(Value::Null, |a| secs(a, relay_audio)),
        "relay": outcome,
    })
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

pub async fn run(a: &Args, synth: crate::tts::SharedSynth, cache: &Path) -> anyhow::Result<i32> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let Some(key) = env.get(&a.key_env).filter(|k| !k.trim().is_empty()) else {
        anyhow::bail!("{} is not set", a.key_env);
    };
    let prices = OPENAI
        .models
        .iter()
        .find(|m| m.id == a.model)
        .map(|m| m.prices);
    let p = OpenAiRealtime::new(
        a.url.clone(),
        Zeroizing::new(key.trim().to_string()),
        &a.model,
        &a.voice,
        500,
        prices,
    );
    let voice = super::audio::Voice::new(synth, &cache.join("utterances"));
    let rendered: Vec<(String, bool, Vec<i16>)> = LINES
        .iter()
        .map(|(t, l)| voice.render(t).map(|u| (t.to_string(), *l, u.pcm)))
        .collect::<anyhow::Result<_>>()?;
    let mut rows = Vec::new();
    for round in 0..a.n {
        let began = Instant::now();
        let mut c = p
            .connect(&setup())
            .await
            .map_err(|f| anyhow::anyhow!("{f}"))?;
        let connect = began.elapsed().as_secs_f64();
        println!("round {}: connected in {connect:.2}s", round + 1);
        for (text, lookup, pcm) in &rendered {
            let row = one(&mut c, pcm, text, *lookup).await;
            println!("  {}", row);
            rows.push(row);
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        c.link.close();
    }
    let col = |k: &str, lookup: Option<bool>| -> Vec<f64> {
        rows.iter()
            .filter(|r| lookup.is_none_or(|l| r["lookup"] == l))
            .filter_map(|r| r[k].as_f64())
            .collect()
    };
    let summary = json!({
        "provider": p.name(), "model": a.model, "voice": a.voice, "rounds": a.n,
        "first_sound_p50": median(col("first_sound", None)),
        "first_sound_small_talk_p50": median(col("first_sound", Some(false))),
        "consult_p50": median(col("consult", Some(true))),
        "relay_start_p50": median(col("relay_start", Some(true))),
        "relays_verified": rows.iter().filter(|r| r["relay"] == "verified").count(),
        "relays": rows.iter().filter(|r| !r["relay"].is_null()).count(),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    std::fs::create_dir_all(&a.out)?;
    std::fs::write(
        a.out.join("realtime.json"),
        serde_json::to_string_pretty(&json!({"summary": summary, "rows": rows}))?,
    )?;
    println!("kept in {}", a.out.join("realtime.json").display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::fake::{FakeRealtime, Reply};

    #[tokio::test]
    async fn a_line_spoken_into_the_fake_is_timed_and_a_lookups_relay_checked() {
        let f = FakeRealtime::start("sk-perf").await;
        f.answer_speech(vec![
            Reply::Talk("Going well, thanks.".into()),
            Reply::Consult {
                ack: "One sec.".into(),
                request: "open issues".into(),
            },
        ]);
        let p = OpenAiRealtime::new(
            f.url.clone(),
            Zeroizing::new("sk-perf".into()),
            "gpt-realtime-2.1-mini",
            "marin",
            500,
            None,
        );
        let mut c = p.connect(&setup()).await.unwrap();
        let speech = vec![8000i16; 16_000 / 2];
        let talk = one(&mut c, &speech, "hi", false).await;
        assert!(talk["first_sound"].as_f64().is_some(), "{talk}");
        assert!(talk["consult"].is_null());
        let lookup = one(&mut c, &speech, "what's open", true).await;
        assert!(lookup["consult"].as_f64().is_some(), "{lookup}");
        assert!(lookup["relay_start"].as_f64().is_some(), "{lookup}");
        assert_eq!(lookup["relay"], "verified", "{lookup}");
    }

    #[test]
    fn the_key_is_named_never_given() {
        let out = Path::new("/tmp");
        assert!(parse(&[], out).is_err());
        let a = parse(
            &[
                "--key-env".into(),
                "OPENAI_REALTIME_TEST_KEY".into(),
                "--n".into(),
                "3".into(),
            ],
            out,
        )
        .unwrap();
        assert_eq!(a.key_env, "OPENAI_REALTIME_TEST_KEY");
        assert_eq!(a.n, 3);
        assert_eq!(a.model, "gpt-realtime-2.1-mini");
    }
}
