//! ADR 0077 end to end: `ghostd run` over a temp folder, a dictation through the control
//! socket with a fake microphone and a recognizer that reads a marked sentence, and then a
//! search for the mark everywhere Ghost could have put it: every file under the temp folder
//! (the store, the audit, the latency log, debug mode's file, the wake takes, a diagnostic
//! report saved with every box ticked), every line logged at any level with debug mode on,
//! every event on the bus (which the phone's stream and the desktop read), and every answer
//! the control API gave. The only place it may be is the clipboard.
//!
//! Its own test binary: the log capture is the process's global subscriber.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_api::GhostClient;
use ghost_core::MemoryKeyring;
use ghost_infer::TurnVerdict;
use ghost_voice::VoiceSettings;
use ghost_voice::frames::Audio;
use ghost_voice::llm::{ModelPort, ScriptedModel};
use ghost_voice::pipeline::Models;
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::transport::{MediaIn, Silent, scripted_media};
use ghost_voice::tts::{NoSynth, SharedSynth};
use ghost_voice::turns::TurnModel;
use ghostd::config::GhostdSettings;
use ghostd::daemon::{Daemon, LogMirror, Ports};
use ghostd::dictation::{Clipboard, Note, Notifier};
use serde_json::json;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use zeroize::Zeroizing;

/// The mark, in the words Parakeet "reads", and in the forms it could take on the way.
const SAID: &str = "Um, the quokka marigold password is seventeen.";
const MARKS: [&str; 3] = ["quokka", "marigold", "QUOKKA"];

struct ReadsTheMark;

impl Recognizer for ReadsTheMark {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        // Dictation's audio is loud; anything else Ghost reads (nothing here) is silence.
        let loud = audio.iter().any(|x| x.abs() > 0.01);
        Ok(if loud { SAID } else { "" }.into())
    }
}

struct Complete;

impl TurnModel for Complete {
    fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
        Ok(TurnVerdict {
            probability: 0.99,
            complete: true,
        })
    }
}

struct Loud;

#[async_trait::async_trait]
impl MediaIn for Loud {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some(Audio::new(vec![3000; 320], 16_000))
    }
}

#[derive(Default)]
struct Board(Mutex<Vec<String>>);

impl Clipboard for Board {
    fn set(&self, text: Zeroizing<String>) -> Result<(), ghost_api::dictation::DictationReason> {
        self.0.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

#[derive(Default)]
struct Shown(Mutex<Vec<String>>);

impl Notifier for Shown {
    fn show(&self, note: Note) {
        let line = match note {
            Note::Recording => "Dictating".to_string(),
            Note::Ended(r) => r.sentence(),
        };
        self.0.lock().unwrap().push(line);
    }
}

/// Every log line, at every level.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn has_mark(bytes: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(bytes).to_lowercase();
    MARKS
        .iter()
        .find(|m| text.contains(&m.to_lowercase()))
        .copied()
}

/// Every file under `dir`, with the mark if it holds it.
fn files_with_mark(dir: &Path, found: &mut Vec<String>, seen: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_with_mark(&p, found, seen);
        } else if let Ok(bytes) = std::fs::read(&p) {
            *seen += 1;
            if let Some(m) = has_mark(&bytes) {
                found.push(format!("{} holds {m}", p.display()));
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictated_words_reach_only_the_clipboard() {
    let lines = Lines::default();
    let mirror = LogMirror::new();
    let capture = ghostd::debug_audio::Capture::new();
    {
        let lines = lines.clone();
        tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::TRACE)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || lines.clone()),
            )
            .with(mirror.clone())
            .with(capture.clone())
            .init();
    }
    let tmp = tempfile::tempdir().unwrap();
    let t = |p: &str| tmp.path().join(p);
    let cfg = GhostdSettings {
        data_dir: t("data"),
        db_path: t("data/ghost.db"),
        policy_path: t("config/policy.toml"),
        config_dir: t("config"),
        catalog_path: t("cache/catalog.json"),
        skills_dir: t("config/skills"),
        home_skills_dir: t("home-skills"),
        api_socket: Some(t("dictation.sock")),
        settings_path: t("config/settings.env"),
        env_path: t("config/env"),
        vault_env_path: t("no-heka"),
        infra_dir: t("infra"),
        wake_mode: "file".into(),
        chime: false,
        mcp_listen: String::new(),
        nerd_dictation_flag: t("rt/nerd-dictation"),
        ..GhostdSettings::default()
    };
    let cache = t("cache").to_string_lossy().into_owned();
    let voice = VoiceSettings::from_vars(|name| match name {
        "GHOST_CACHE_DIR" => Some(cache.clone()),
        "GHOST_THINKING_SOUND" => Some("false".into()),
        "GHOST_LATENCY" => Some("1".into()),
        "GHOST_SILERO_MODEL" => Some(t("nonexistent/silero.onnx").to_string_lossy().into_owned()),
        "GHOST_SMART_TURN_MODEL" => Some(t("nonexistent/turn.onnx").to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    std::fs::create_dir_all(t("rt")).unwrap();
    let board = Arc::new(Board::default());
    let shown = Arc::new(Shown::default());
    let mut ports = Ports::headless();
    ports.keyring = Arc::new(MemoryKeyring::new());
    ports.load_models = Arc::new(|s: &VoiceSettings| {
        let stt: SharedRecognizer = Arc::new(Mutex::new(ReadsTheMark));
        let tts: SharedSynth = Arc::new(Mutex::new(NoSynth));
        Ok(Models {
            stt,
            tts,
            turn: Arc::new(Mutex::new(Complete)),
            silero: s.silero_model.clone(),
            voices: vec!["af_heart".into()],
            acks: ghost_voice::acks::Acks::new(None),
            problem: None,
        })
    });
    ports.model = Arc::new(|_: &VoiceSettings, _: &HashMap<String, String>| {
        Ok(ScriptedModel::new(vec![], Duration::from_millis(5)) as Arc<dyn ModelPort>)
    });
    ports.desk_media = Arc::new(|_req| {
        Box::pin(async move {
            let (media, _mouth, _ear) = scripted_media();
            Ok(media)
        })
    });
    ports.passive_media = Some(Arc::new(|| Ok(Box::new(Silent) as Box<dyn MediaIn>)));
    ports.dictation_media = Some(Arc::new(|| Ok(Box::new(Loud) as Box<dyn MediaIn>)));
    ports.clipboard = board.clone();
    ports.dictation_notes = shown.clone();
    let daemon = Arc::new(
        Daemon::new(cfg.clone(), voice, ports)
            .with_runtime_dir(t("rt"))
            .with_models_retry(Duration::from_millis(200))
            .with_log_mirror(mirror.clone())
            .with_debug_capture(capture),
    );
    let client =
        GhostClient::with_timeout(cfg.api_socket.clone().unwrap(), Duration::from_secs(10));
    let run = {
        let d = daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(v) = client.status().await
            && v["phase"] == "passive"
            && v["models_ready"] == true
        {
            break;
        }
        assert!(Instant::now() < deadline, "the daemon never got ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Every event from here on, as the desktop and the phone's stream read the bus.
    let mut bus = daemon.state.bus.tap();
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let collecting = {
        let events = events.clone();
        tokio::spawn(async move {
            while let Ok(e) = bus.recv().await {
                events.lock().unwrap().push(e);
            }
        })
    };
    // Debug mode on: what Ghost hears is logged now, and kept in its file.
    let on = client.debug_audio(Some(true)).await.unwrap();
    assert_eq!(on["on"], true);
    // The passive loop must be waiting before dictation can take the microphone.
    let mut answers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let started = loop {
        let v = client.dictation(Some("start")).await.unwrap();
        if v["state"] == "recording" {
            break v;
        }
        assert!(Instant::now() < deadline, "dictation never started: {v}");
        answers.push(v);
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    answers.push(started);
    tokio::time::sleep(Duration::from_millis(400)).await;
    answers.push(client.dictation(None).await.unwrap());
    let stopped = client.dictation(Some("stop")).await.unwrap();
    assert_eq!(stopped["result"]["outcome"], "on_clipboard", "{stopped}");
    answers.push(stopped.clone());
    answers.push(client.dictation(None).await.unwrap());
    answers.push(client.status().await.unwrap());
    // It went where it should, cleaned up.
    assert_eq!(
        *board.0.lock().unwrap(),
        ["The quokka marigold password is seventeen."]
    );
    assert_eq!(
        stopped["result"]["chars"],
        "The quokka marigold password is seventeen.".len()
    );
    // A diagnostic report with every box ticked, saved into the temp folder.
    std::fs::create_dir_all(t("reports")).unwrap();
    let report = client
        .diagnose(&json!({
            "transcripts": true,
            "debug_log": true,
            "wake_recordings": true,
            "folder": t("reports").to_string_lossy(),
        }))
        .await
        .unwrap();
    answers.push(report);
    // Debug mode's file is still there while it is on: searched below, then turned off.
    let mut found = Vec::new();
    let mut seen = 0;
    files_with_mark(tmp.path(), &mut found, &mut seen);
    assert!(seen > 3, "the walk read the daemon's files ({seen})");
    assert!(
        t("reports").read_dir().unwrap().count() >= 1,
        "a report was saved"
    );
    client.debug_audio(Some(false)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    daemon.stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(20), run).await;
    collecting.abort();
    files_with_mark(tmp.path(), &mut found, &mut seen);

    for a in &answers {
        if let Some(m) = has_mark(a.to_string().as_bytes()) {
            found.push(format!("an API answer holds {m}: {a}"));
        }
    }
    for e in events.lock().unwrap().iter() {
        if let Some(m) = has_mark(e.as_bytes()) {
            found.push(format!("an event holds {m}: {e}"));
        }
    }
    let logged = lines.0.lock().unwrap().clone();
    assert!(
        String::from_utf8_lossy(&logged).contains("dictation: OnClipboard"),
        "the log capture saw the dictation's numbers"
    );
    if let Some(m) = has_mark(&logged) {
        found.push(format!("a log line holds {m}"));
    }
    for line in mirror.tail() {
        if let Some(m) = has_mark(line.as_bytes()) {
            found.push(format!("the log mirror holds {m}"));
        }
    }
    for s in shown.0.lock().unwrap().iter() {
        if let Some(m) = has_mark(s.as_bytes()) {
            found.push(format!("a notification holds {m}"));
        }
    }
    assert!(found.is_empty(), "dictated words escaped: {found:#?}");
}
