//! `ghostd run` end to end on a side socket, in both wake modes: the daemon starts over
//! a temp folder with fakes for Parakeet, Kokoro and Smart Turn and a scripted model,
//! answers `/status`, takes a file wake, greets through the scripted devices (Ghost's
//! voice reaches the speaker), answers a typed turn (`/session/say`) with the model's
//! words recorded in `/session/turns`, ends on `/sleep`, and winds down on the stop
//! token with its socket removed.
//!
//! Nothing here touches the operator's folders, keyring, microphone or speakers: the
//! devices are the replay harness's, the keyring is in memory, every path is a temp dir.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_api::GhostClient;
use ghost_core::MemoryKeyring;
use ghost_infer::TurnVerdict;
use ghost_voice::VoiceSettings;
use ghost_voice::llm::{ModelPort, ScriptedModel};
use ghost_voice::pipeline::Models;
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::transport::{Ear, MediaIn, Silent, scripted_media};
use ghost_voice::tts::{SharedSynth, Synth};
use ghost_voice::turns::TurnModel;
use ghostd::config::GhostdSettings;
use ghostd::daemon::{Daemon, Ports};
use serde_json::Value;

struct NoSpeech;

impl Recognizer for NoSpeech {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        Ok(String::new())
    }
}

/// A clause takes 0.3 s of audio at 24 kHz.
struct FakeKokoro(Arc<Mutex<Vec<String>>>);

impl Synth for FakeKokoro {
    fn voices(&self) -> Vec<String> {
        vec!["af_heart".into(), "am_adam".into()]
    }
    fn synthesize(
        &mut self,
        text: &str,
        _voice: &str,
        _speed: f32,
    ) -> ghost_infer::Result<Vec<i16>> {
        self.0.lock().unwrap().push(text.to_string());
        Ok(vec![20000; 24_000 * 3 / 10])
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

fn settings(tmp: &std::path::Path, wake_mode: &str) -> (GhostdSettings, VoiceSettings) {
    let t = |p: &str| tmp.join(p);
    let cfg = GhostdSettings {
        data_dir: t("data"),
        db_path: t("data/ghost.db"),
        policy_path: t("config/policy.toml"),
        config_dir: t("config"),
        catalog_path: t("cache/catalog.json"),
        skills_dir: t("config/skills"),
        home_skills_dir: t("home-skills"),
        api_socket: Some(t("rust.sock")),
        settings_path: t("config/settings.env"),
        env_path: t("config/env"),
        vault_env_path: t("no-heka"),
        infra_dir: t("infra"),
        wake_mode: wake_mode.into(),
        chime: false,
        dictation_flag: t("rt/dictation"),
        ..GhostdSettings::default()
    };
    let cache = t("cache").to_string_lossy().into_owned();
    let voice = VoiceSettings::from_vars(|name| match name {
        "GHOST_CACHE_DIR" => Some(cache.clone()),
        "GHOST_THINKING_SOUND" => Some("false".into()),
        "GHOST_SILERO_MODEL" => Some(t("nonexistent/silero.onnx").to_string_lossy().into_owned()),
        "GHOST_SMART_TURN_MODEL" => Some(t("nonexistent/turn.onnx").to_string_lossy().into_owned()),
        "GHOST_LATENCY" => Some("1".into()),
        "GHOST_TURN_GRACE_SECS" => Some("0.1".into()),
        _ => None,
    })
    .unwrap();
    (cfg, voice)
}

struct World {
    daemon: Arc<Daemon>,
    client: GhostClient,
    ear: Arc<Mutex<Option<Ear>>>,
    spoken: Arc<Mutex<Vec<String>>>,
}

fn world(tmp: &std::path::Path, wake_mode: &str) -> World {
    let (cfg, voice) = settings(tmp, wake_mode);
    std::fs::create_dir_all(tmp.join("rt")).unwrap();
    let spoken: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let ear: Arc<Mutex<Option<Ear>>> = Arc::new(Mutex::new(None));
    let mut ports = Ports::headless();
    ports.keyring = Arc::new(MemoryKeyring::new());
    ports.load_models = {
        let spoken = spoken.clone();
        Arc::new(move |s: &VoiceSettings| {
            let stt: SharedRecognizer = Arc::new(Mutex::new(NoSpeech));
            let tts: SharedSynth = Arc::new(Mutex::new(FakeKokoro(spoken.clone())));
            Ok(Models {
                stt,
                tts,
                turn: Arc::new(Mutex::new(Complete)),
                silero: s.silero_model.clone(),
                voices: vec!["af_heart".into(), "am_adam".into()],
            })
        })
    };
    ports.model = Arc::new(|_: &VoiceSettings, _: &HashMap<String, String>| {
        let lines = vec![
            vec![ScriptedModel::say("Hello from the fake model.")],
            vec![ScriptedModel::say("Bye then.")],
        ];
        Ok(ScriptedModel::new(lines, Duration::from_millis(20)) as Arc<dyn ModelPort>)
    });
    ports.desk_media = {
        let ear = ear.clone();
        Arc::new(move |_req| {
            let ear = ear.clone();
            Box::pin(async move {
                let (media, _mouth, e) = scripted_media();
                *ear.lock().unwrap() = Some(e);
                Ok(media)
            })
        })
    };
    ports.passive_media = Some(Arc::new(|| Ok(Box::new(Silent) as Box<dyn MediaIn>)));
    let daemon = Arc::new(Daemon::new(cfg.clone(), voice, ports).with_runtime_dir(tmp.join("rt")));
    let client = GhostClient::with_timeout(cfg.api_socket.clone().unwrap(), Duration::from_secs(5));
    World {
        daemon,
        client,
        ear,
        spoken,
    }
}

async fn until<F: Fn(&Value) -> bool>(client: &GhostClient, what: &str, ok: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(v) = client.status().await
            && ok(&v)
        {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "the daemon never reached: {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn turns(client: &GhostClient) -> Vec<Value> {
    client
        .request(hyper::Method::GET, "/session/turns", None)
        .await
        .map(|r| r.body.as_array().cloned().unwrap_or_default())
        .unwrap_or_default()
}

async fn a_file_wake_and_a_typed_turn(wake_mode: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path(), wake_mode);
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    let st = until(client, "passive with the models ready", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    assert_eq!(st["wake_mode"], wake_mode);
    assert_eq!(st["session_id"], Value::Null);
    assert!(!run.is_finished());

    // the file wake: a session opens in Ghost and greets
    std::fs::write(tmp.path().join("rt/wake"), "").unwrap();
    let st = until(client, "a session", |v| v["phase"] != "passive").await;
    let session_id = st["session_id"].as_str().unwrap().to_string();
    assert_eq!(st["conversation"], "ghost");
    assert_eq!(st["session_origin"], "desk");
    let heard_at = Instant::now();
    loop {
        let played = w
            .ear
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0.0, Ear::seconds_played);
        if played > 0.0 {
            break;
        }
        assert!(
            heard_at.elapsed() < Duration::from_secs(20),
            "Ghost never spoke"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !w.spoken.lock().unwrap().is_empty(),
        "the greeting went through the voice"
    );

    // a typed turn: through the model, recorded as the session's turns
    let ok = client.say("what's up").await.unwrap();
    assert_eq!(ok["ok"], true, "{ok}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let t = turns(client).await;
        let answered = t.iter().any(|x| {
            x["role"] == "assistant" && x["text"].as_str().unwrap_or("").contains("fake model")
        });
        if answered {
            assert!(
                t.iter()
                    .any(|x| x["role"] == "user" && x["text"] == "what's up" && x["via"] == "text")
            );
            assert!(t.iter().all(|x| x["session_id"] == session_id));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the model's reply never arrived: {t:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let refused = client.wake(None).await.unwrap(); // no wake inside a session
    assert_eq!(refused["ok"], false, "{refused}");

    // sleep: back to passive, the session over
    let ok = client.sleep().await.unwrap();
    assert_eq!(ok["ok"], true);
    let st = until(client, "passive again", |v| v["phase"] == "passive").await;
    assert_eq!(st["session_id"], Value::Null);
    assert!(turns(client).await.is_empty());

    // the stop: the daemon winds down and its socket goes
    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the daemon stops")
        .unwrap()
        .unwrap();
    assert!(!tmp.path().join("rust.sock").exists());
    assert!(tmp.path().join("data/latency.jsonl").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ghostd_run_answers_a_file_wake_and_a_typed_turn_in_file_mode() {
    a_file_wake_and_a_typed_turn("file").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ghostd_run_answers_a_file_wake_and_a_typed_turn_in_transcript_mode() {
    a_file_wake_and_a_typed_turn("transcript").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_line_without_a_session_wakes_with_those_words_and_a_call_names_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path(), "file");
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "passive with the models ready", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    let ok = client.say("hello there").await.unwrap();
    assert_eq!(ok["ok"], true);
    let st = until(client, "a session", |v| v["phase"] != "passive").await;
    assert_eq!(st["conversation"], "ghost");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let t = turns(client).await;
        if t.iter().any(|x| x["role"] == "assistant") {
            let first = &t[0];
            assert_eq!(first["role"], "user");
            assert_eq!(first["text"], "hello there");
            assert_eq!(first["via"], "text"); // the first turn, typed
            break;
        }
        assert!(Instant::now() < deadline, "no reply: {t:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    client.sleep().await.unwrap();
    until(client, "passive again", |v| v["phase"] == "passive").await;
    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
