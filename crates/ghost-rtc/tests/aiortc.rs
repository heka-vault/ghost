//! Against a real aiortc peer, `scripts/rtc/phone.py`, run with the Python venv's
//! interpreter: ignored by default, since CI has no venv. The phone offers on the script's
//! stdout and takes the answer on its stdin; the media stack under test is whichever
//! feature is on.
//!
//! ```bash
//! cargo nextest run -p ghost-rtc --features str0m --run-ignored ignored-only
//! GHOST_VENV=/path/to/.venv cargo test -p ghost-rtc --features webrtc -- --ignored
//! ```
//!
//! `GHOST_VENV` defaults to the main checkout's `.venv`. Nothing from the call is kept but
//! numbers.
#![cfg(any(feature = "str0m", feature = "webrtc", feature = "libwebrtc"))]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use ghost_rtc::probe::{self, Audio, Report};
use ghost_rtc::{Fingerprint, Link, Media, MediaConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

fn python() -> PathBuf {
    let venv = std::env::var("GHOST_VENV")
        .unwrap_or_else(|_| "/home/detroitpro/projects/active/ghost/.venv".to_string());
    PathBuf::from(venv).join("bin/python")
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rtc/phone.py")
}

struct Call {
    desk: Report,
    phone: Value,
    /// `Media::fingerprint()` right after each answer.
    fingerprints: Vec<Fingerprint>,
}

async fn call(seconds: f64, restart_at: f64) -> Call {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ghost_rtc=debug")),
        )
        .with_test_writer()
        .try_init();
    let media: Arc<dyn Media> = Arc::from(ghost_rtc::new_media(MediaConfig::default()).unwrap());
    let mut child = Command::new(python())
        .arg("-I")
        .arg(script())
        .args([
            "--stdio",
            "--seconds",
            &seconds.to_string(),
            "--restart-at",
            &restart_at.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("the venv's python runs the phone script");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let (stop_tx, stop_rx) = watch::channel(false);
    let session = tokio::spawn(probe::run(media.clone(), Audio::Markers, stop_rx));
    let mut phone = None;
    let mut fingerprints = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("offer") => {
                let restart = v["restart_pc"].as_bool().unwrap_or(false);
                let sdp = media
                    .answer(v["sdp"].as_str().unwrap(), restart)
                    .await
                    .expect("the offer is answered");
                fingerprints.push(media.fingerprint().expect("an answer has a fingerprint"));
                let reply = json!({"sdp": sdp, "type": "answer", "pc_id": "test"});
                stdin
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            Some("report") => {
                phone = Some(v);
                break;
            }
            _ => {}
        }
    }
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the phone exits")
        .unwrap();
    assert!(status.success(), "phone.py reported errors");
    // Give the stack a moment to notice the peer left, then stop.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = stop_tx.send(true);
    let desk = session.await.unwrap();
    media.close().await;
    assert_eq!(*media.link().borrow(), Link::Gone);
    Call {
        desk,
        phone: phone.expect("the phone wrote its report"),
        fingerprints,
    }
}

fn times(v: &Value) -> Vec<f64> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// One-way latencies: each receive paired with the latest send before it within 0.9 s.
fn pair(sent: &[f64], received: &[f64]) -> Vec<f64> {
    received
        .iter()
        .filter_map(|r| {
            sent.iter()
                .rev()
                .find(|s| **s <= *r && r - **s < 0.9)
                .map(|s| r - s)
        })
        .collect()
}

fn p50(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn loopback_call_with_aiortc_carries_audio_both_ways_and_the_rtvi_subset() {
    let c = call(8.0, 0.0).await;
    let errors = c.phone["errors"].as_array().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    // The handshake: client-ready in, bot-ready and one server-message out, send-text
    // never answered, disconnect-bot in at the end.
    assert_eq!(c.desk.client_ready.len(), 1);
    assert_eq!(c.desk.bot_ready_sent, 1);
    assert_eq!(times(&c.phone["bot_ready"]).len(), 1);
    assert_eq!(c.phone["server_messages"], 1);
    assert_eq!(c.phone["other_messages"], 0);
    assert_eq!(c.desk.disconnect_bot.len(), 1);
    // The fingerprint the phone read in the answer is the one the stack reports.
    let answers = c.phone["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["sha256"], true);
    assert_eq!(answers[0]["fingerprint"], c.fingerprints[0].to_string());
    assert!(answers[0]["candidates"].as_u64().unwrap() >= 1);
    // Audio both ways: the bursts, about one a second, with their one-way latency.
    let up = pair(&times(&c.phone["sent"]), &c.desk.received);
    let down = pair(&c.desk.sent, &times(&c.phone["received"]));
    assert!(up.len() >= 5, "uplink bursts heard: {}", up.len());
    assert!(down.len() >= 5, "downlink bursts heard: {}", down.len());
    assert!(p50(up.clone()) < 0.15, "uplink p50 {:.3} s", p50(up));
    assert!(p50(down.clone()) < 0.3, "downlink p50 {:.3} s", p50(down));
    assert!(c.desk.stats.packets_received > 300, "{:?}", c.desk.stats);
    assert_eq!(c.desk.stats.packets_lost, 0);
    assert_eq!(c.desk.stats.frames_dropped, 0);
    assert!(c.desk.frames > 300);
    let states: Vec<&str> = c.desk.links.iter().map(|(_, s)| s.as_str()).collect();
    assert!(states.starts_with(&["Waiting", "Connected"]), "{states:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn restart_pc_mid_call_gives_a_fresh_peer_and_the_phone_pins_each_answer() {
    let c = call(9.0, 4.0).await;
    let errors = c.phone["errors"].as_array().unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    let answers = c.phone["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[1]["restart"], true);
    for (i, a) in answers.iter().enumerate() {
        assert_eq!(a["sha256"], true);
        assert_eq!(
            a["fingerprint"],
            c.fingerprints[i].to_string(),
            "answer {i}"
        );
    }
    assert_eq!(c.desk.client_ready.len(), 2);
    assert_eq!(times(&c.phone["bot_ready"]).len(), 2);
    let restart_at = answers[1]["at"].as_f64().unwrap();
    let after_up = c
        .desk
        .received
        .iter()
        .filter(|t| **t > restart_at + 0.5)
        .count();
    let after_down = times(&c.phone["received"])
        .iter()
        .filter(|t| **t > restart_at + 0.5)
        .count();
    assert!(after_up >= 2, "uplink bursts after the restart: {after_up}");
    assert!(
        after_down >= 2,
        "downlink bursts after the restart: {after_down}"
    );
    // The phone closes its old peer first: a stack that notices passes through Gone before
    // the restart's Waiting, one that does not goes straight to Waiting; both come back.
    let states: Vec<&str> = c.desk.links.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(
        states.iter().filter(|s| **s == "Connected").count(),
        2,
        "{states:?}"
    );
    let without_gone: Vec<&str> = states.iter().copied().filter(|s| *s != "Gone").collect();
    assert!(
        without_gone.starts_with(&["Waiting", "Connected", "Waiting", "Connected"]),
        "{states:?}"
    );
}
