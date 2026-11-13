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
        mcp_listen: String::new(), // never the operator's port from a test
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
    world_with(tmp, wake_mode, |_| {})
}

/// The same, with the ports changed before the daemon is built.
fn world_with(tmp: &std::path::Path, wake_mode: &str, tweak: impl FnOnce(&mut Ports)) -> World {
    world_from(tmp, wake_mode, |_| {}, tweak)
}

/// The same, with the voice settings changed too.
fn world_from(
    tmp: &std::path::Path,
    wake_mode: &str,
    voice_tweak: impl FnOnce(&mut VoiceSettings),
    tweak: impl FnOnce(&mut Ports),
) -> World {
    let (cfg, mut voice) = settings(tmp, wake_mode);
    voice_tweak(&mut voice);
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
                acks: ghost_voice::acks::Acks::new(None),
                problem: None,
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
    tweak(&mut ports);
    let daemon = Arc::new(
        Daemon::new(cfg.clone(), voice, ports)
            .with_runtime_dir(tmp.join("rt"))
            .with_models_retry(Duration::from_millis(200)),
    );
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

    // ADR 0045: Ghost's own app is served before any session opens, with no vault, no
    // network and no apps.json. It is in the gate and in an agent's reach, and it is not
    // one of the operator's connected apps, nor ever an app that "isn't connected".
    let apps = w.daemon.apps().unwrap();
    assert!(apps.is_up("ghost"));
    assert!(
        w.daemon
            .gate()
            .unwrap()
            .operation("ghost", "notify")
            .is_ok()
    );
    assert_eq!(st["apps"], serde_json::json!([]));
    assert_eq!(st["waiting"], 0);
    let ghost = w.daemon.conversations().unwrap().get("ghost").unwrap();
    let reach = w.daemon.reach().unwrap()(&ghost.unwrap());
    assert_eq!(reach[0].name, "ghost");
    assert!(
        reach[0]
            .read_tools
            .iter()
            .any(|t| t.name == "ghost_docs_search")
    );
    let status = reach[0]
        .read_tools
        .iter()
        .find(|t| t.name == "ghost_status")
        .unwrap();
    let said = (status.handler)(serde_json::Map::new()).await.unwrap();
    assert_eq!(said["phase"], "passive");
    assert_eq!(said["session"]["open"], false);
    assert_eq!(said["problems"], serde_json::json!([]));
    assert!(said["model"]["provider"].is_string());

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

/// 2026-11-13: typing to Ghost with no session open started a full desk session with the
/// microphone live, and the room's music was taken for turns and answered. A typed turn is a
/// typed session: no capture is opened for it, the passive listener runs beside it (so "hey
/// Ghost" still works), the status says so, and it ends on its own after the reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_turn_opens_no_microphone_and_ends_on_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let mics: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let passive_opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = world_from(
        tmp.path(),
        "transcript",
        |v| v.typed_idle_secs = 1.0,
        |ports| {
            let inner = ports.desk_media.clone();
            let mics = mics.clone();
            ports.desk_media = Arc::new(move |req| {
                mics.lock().unwrap().push(req.mic);
                inner(req)
            });
            let opens = passive_opens.clone();
            ports.passive_media = Some(Arc::new(move || {
                opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(Silent) as Box<dyn MediaIn>)
            }));
        },
    );
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "passive with the models ready", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    let opened_before = passive_opens.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(client.say("what's on my list").await.unwrap()["ok"], true);
    let st = until(client, "a typed session", |v| v["session_id"].is_string()).await;
    assert_eq!(st["session_typed"], true, "{st}");
    assert_eq!(st["session_origin"], "desk");
    // The session's own input was silent: no capture stream for it.
    assert_eq!(*mics.lock().unwrap(), vec![false]);
    // The passive listener is opened again while the typed session is still on, so "hey
    // Ghost" works beside it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while passive_opens.load(std::sync::atomic::Ordering::SeqCst) <= opened_before {
        assert!(
            Instant::now() < deadline,
            "no wake listener beside the typed session"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        client.status().await.unwrap()["session_id"].is_string(),
        "still in the typed session"
    );
    // The reply, then the session ends by itself a short while after it.
    until(client, "passive again", |v| v["phase"] == "passive").await;
    assert_eq!(client.status().await.unwrap()["session_typed"], false);
    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// A load that fails as on a machine without the model files: the first `fails` tries.
fn flaky_models(ports: &mut Ports, fails: usize) -> Arc<std::sync::atomic::AtomicUsize> {
    let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let inner = ports.load_models.clone();
    let counted = tries.clone();
    ports.load_models = Arc::new(move |s: &VoiceSettings| {
        if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < fails {
            anyhow::bail!("Parakeet's files are not in the Hugging Face cache; set GHOST_STT_DIR");
        }
        inner(s)
    });
    tries
}

async fn waiting_text(client: &GhostClient) -> String {
    client
        .request(hyper::Method::GET, "/waiting", None)
        .await
        .map(|r| r.body.to_string())
        .unwrap_or_default()
}

/// ADR 0038: a model that won't load is waited for, not a crash. The API answers, the
/// waiting list says what is missing, the load is tried again, and the item resolves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_that_wont_load_is_waited_for_not_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let mut tries = None;
    let w = world_with(tmp.path(), "file", |ports| {
        tries = Some(flaky_models(ports, 2))
    });
    let tries = tries.unwrap();
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    let st = until(client, "the API up with the models missing", |v| {
        v["models_ready"] == false && v["waiting"].as_u64().unwrap_or(0) >= 1
    })
    .await;
    assert_ne!(st["phase"], "passive");
    assert!(!run.is_finished(), "the daemon runs on");
    let text = waiting_text(client).await;
    assert!(
        text.contains("models:missing"),
        "the waiting list names it: {text}"
    );
    // ADR 0066: files that are not here are said as that, with where to get them.
    assert!(
        text.contains("not loaded") || text.contains("not downloaded yet"),
        "and says so in words: {text}"
    );

    until(client, "passive once the models load", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    assert!(
        tries.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "the load was tried again"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while waiting_text(client).await.contains("models:missing") {
        assert!(
            Instant::now() < deadline,
            "the item resolves once the models load"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    w.daemon.stop.cancel();
    run.await.unwrap().unwrap();
}

/// A stop while the models are still missing winds the daemon down cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_while_the_models_are_missing_winds_down() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world_with(tmp.path(), "file", |ports| {
        flaky_models(ports, usize::MAX);
    });
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "the API up with the models missing", |v| {
        v["models_ready"] == false && v["waiting"].as_u64().unwrap_or(0) >= 1
    })
    .await;
    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("the daemon stops while waiting for its models")
        .unwrap()
        .unwrap();
    assert!(client.status().await.is_err(), "the socket is gone");
}

/// One tool call from an outside tool: what it answered, or what it refused with.
async fn outside_call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    let params = rmcp::model::CallToolRequestParams::new(tool.to_string())
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await.map_err(|e| e.to_string())?;
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    if result.is_error == Some(true) {
        return Err(text);
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// ADR 0048, end to end: the operator connects a tool in Settings, the tool reads Ghost,
/// tells the operator something, has Ghost say a sentence at the desk, and is refused the
/// moment it is removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connected_tool_uses_the_running_ghost_and_is_refused_once_removed() {
    use rmcp::ServiceExt;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use serde_json::json;

    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path(), "file");
    // The operator let Ghost speak first at the desk; the listener on a port of its own.
    std::fs::create_dir_all(tmp.path().join("config")).unwrap();
    std::fs::write(
        tmp.path().join("config/policy.toml"),
        "[permissions]\nask = [\"ghost.waiting_snooze\"]\n\n[routes]\nenabled = [\"digest\", \"desk\"]\n",
    )
    .unwrap();
    w.daemon.cfg.lock().unwrap().mcp_listen = "127.0.0.1:0".into();
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "passive with the models ready", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;

    // Settings: connect a tool. Its token comes back this once.
    let made = client
        .request(
            hyper::Method::POST,
            "/clients",
            Some(&json!({"name": "claude-code"})),
        )
        .await
        .unwrap();
    assert_eq!(made.status, 200, "{}", made.body);
    let token = made.body["token"].as_str().unwrap().to_string();
    assert_eq!(made.body["client"]["title"], "Claude Code");
    assert_eq!(made.body["clients"]["listening"], json!(["127.0.0.1:0"]));
    assert_eq!(made.body["clients"]["problem"], Value::Null);
    let listed = client
        .request(hyper::Method::GET, "/clients", None)
        .await
        .unwrap();
    assert!(!listed.body.to_string().contains(&token), "never again");
    assert_eq!(listed.body["clients"][0]["last_used_at"], Value::Null);

    let port = w.daemon.late.outside.get().unwrap().port();
    let tool = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
                .auth_header(token.clone()),
        ))
        .await
        .unwrap();
    let status = outside_call(&tool, "status", json!({})).await.unwrap();
    assert_eq!(status["phase"], "passive");
    assert_eq!(status["session"]["open"], false);
    let agents = outside_call(&tool, "agents_list", json!({})).await.unwrap();
    assert_eq!(agents["agents"][0]["agent"], "ghost");
    // The daemon listening to its own events is not the desktop being open.
    assert_eq!(w.daemon.state.bus.subscribers(), 0);

    // It tells the operator something: on the waiting list under its own name, and in
    // the audit as its change.
    let told = outside_call(&tool, "notify", json!({"title": "the build finished"}))
        .await
        .unwrap();
    let key = told["key"].as_str().unwrap().to_string();
    assert!(
        waiting_text(client)
            .await
            .contains("From Claude Code: the build finished")
    );
    let audit = client
        .request(hyper::Method::GET, "/audit", None)
        .await
        .unwrap();
    let row = &audit.body["items"][0];
    assert_eq!(row["session_id"], "client:claude-code");
    assert_eq!(
        (&row["source"], &row["operation"]),
        (&json!("ghost"), &json!("notify"))
    );
    assert!(!audit.body.to_string().contains(&token));
    let listed = client
        .request(hyper::Method::GET, "/clients", None)
        .await
        .unwrap();
    assert!(listed.body["clients"][0]["last_used_at"].is_string());

    // The operator acknowledges it; the tool, waiting, hears so.
    let before = outside_call(&tool, "events_wait", json!({"timeout_s": 0}))
        .await
        .unwrap();
    let ack = client
        .request(
            hyper::Method::POST,
            "/waiting/ack",
            Some(&json!({"key": key})),
        )
        .await
        .unwrap();
    assert!(ack.ok(), "{}", ack.body);
    let heard = outside_call(
        &tool,
        "events_wait",
        json!({"since": before["cursor"], "timeout_s": 10}),
    )
    .await
    .unwrap();
    assert!(
        heard["events"].as_array().unwrap().iter().any(|e| {
            e["change"] == "acknowledged" && e["key"] == key.as_str() && e["yours"] == true
        }),
        "{heard}"
    );

    // A change the operator's permissions ask about has not run: it waits for their yes,
    // and the tool hears what came of it once they click Confirm.
    let asked = outside_call(
        &tool,
        "waiting_snooze",
        json!({"keys": [key], "minutes": 5}),
    )
    .await
    .unwrap();
    assert_eq!(asked["status"], "waiting for the operator", "{asked}");
    let id = asked["id"].as_str().unwrap().to_string();
    assert!(waiting_text(client).await.contains(&format!("change:{id}")));
    let cursor = heard["cursor"].clone();
    let yes = client
        .request(
            hyper::Method::POST,
            &format!("/proposals/{id}/confirm"),
            None,
        )
        .await
        .unwrap();
    assert!(yes.ok(), "{}", yes.body);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cursor = cursor;
    loop {
        let got = outside_call(
            &tool,
            "events_wait",
            json!({"since": cursor, "timeout_s": 5}),
        )
        .await
        .unwrap();
        cursor = got["cursor"].clone();
        if got["events"].as_array().unwrap().iter().any(|e| {
            e["type"] == "change"
                && e["id"] == id.as_str()
                && e["outcome"] == "success"
                && e["yours"] == true
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "no outcome: {got}");
    }

    // A sentence at the desk: Ghost says who it is from, through its own voice, and
    // waits for a reply as desk voice does (ADR 0032).
    let said = outside_call(&tool, "say", json!({"text": "the tests are green"}))
        .await
        .unwrap();
    assert_eq!(said["status"], "saying it at the desk", "{said}");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !w
        .spoken
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.contains("From Claude Code: the tests are green"))
    {
        assert!(
            Instant::now() < deadline,
            "never said: {:?}",
            w.spoken.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // While Ghost is at the desk, a second sentence waits its turn on the list.
    let later = outside_call(&tool, "say", json!({"text": "and the deploy is out"}))
        .await
        .unwrap();
    assert_eq!(later["status"], "on the waiting list", "{later}");
    client.sleep().await.unwrap();
    until(client, "passive again", |v| v["phase"] == "passive").await;

    // Removed in Settings: refused at its next call.
    let gone = client
        .request(hyper::Method::DELETE, "/clients/claude-code", None)
        .await
        .unwrap();
    assert_eq!(gone.body["clients"], json!([]));
    assert!(outside_call(&tool, "status", json!({})).await.is_err());
    let _ = tool.cancel().await;

    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the daemon stops")
        .unwrap()
        .unwrap();
}

// --- the microphone path (ADR 0064) ----------------------------------------------------------

const ARRAY: &str = "alsa_input.usb-Example_XVF3800_Array-00.iec958-stereo";

/// The owner's kind of desk: an array that cancels on chip is mic and speakers, with the
/// host canceller over it.
fn array_facts() -> ghostd::microphone::SoundFacts {
    use ghostd::microphone::{Node, SoundFacts};
    let node =
        |name: &str, class: &str, device: Option<&str>, channels: u16, extra: &[(&str, &str)]| {
            let mut props: std::collections::BTreeMap<String, String> = extra
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            props.insert("node.name".into(), name.into());
            if let Some(d) = device {
                props.insert("device.id".into(), d.into());
            }
            Node {
                name: name.into(),
                class: class.into(),
                description: String::new(),
                channels,
                props,
                running: false,
            }
        };
    let out = "alsa_output.usb-Example_XVF3800_Array-00.analog-stereo";
    SoundFacts {
        nodes: vec![
            node(ARRAY, "Audio/Source", Some("66"), 2, &[]),
            node(out, "Audio/Sink", Some("66"), 2, &[]),
            node("ghost.mic", "Audio/Source", None, 1, &[]),
            node(
                "ghost.mic.capture",
                "Stream/Input/Audio",
                None,
                1,
                &[("node.target", ARRAY)],
            ),
        ],
        default_source: Some(ARRAY.into()),
        default_sink: Some(out.into()),
    }
}

/// A passive input that says nothing and logs when it is closed.
struct LoggedIn(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl MediaIn for LoggedIn {
    async fn next_frame(&mut self) -> Option<ghost_voice::frames::Audio> {
        std::future::pending().await
    }
}

impl Drop for LoggedIn {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("wake closed".into());
    }
}

async fn until_log(log: &Arc<Mutex<Vec<String>>>, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !log.lock().unwrap().last().is_some_and(|l| l == what) {
        assert!(
            Instant::now() < deadline,
            "never: {what}; log {:?}",
            log.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_and_the_session_open_their_own_inputs_one_at_a_time_and_not_listening_opens_neither()
 {
    let tmp = tempfile::tempdir().unwrap();
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let wake_input = format!("{ARRAY}#0");
    let w = world_from(
        tmp.path(),
        "transcript",
        |v| {
            v.input_device = Some("ghost.mic".into());
            v.wake_input_device = Some(wake_input.clone());
        },
        |p| {
            p.sound_facts = Arc::new(|| Some(array_facts()));
            p.passive_media = Some({
                let log = log.clone();
                Arc::new(move || {
                    log.lock().unwrap().push("wake open".into());
                    Ok(Box::new(LoggedIn(log.clone())) as Box<dyn MediaIn>)
                })
            });
            p.desk_media = {
                let log = log.clone();
                Arc::new(move |req| {
                    log.lock().unwrap().push(format!(
                        "session open {} mic={}",
                        req.audio.input_device.clone().unwrap_or_default(),
                        req.mic
                    ));
                    Box::pin(async move { Ok(scripted_media().0) })
                })
            };
        },
    );
    // The plan: the wake on the array's processed channel, sessions on ghost.mic as set.
    let plan = w.daemon.mic_plan();
    assert_eq!(plan.wake.to_string(), wake_input);
    assert!(plan.session.is_canceller());
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "passive", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    until_log(&log, "wake open").await;

    // A wake: the passive input is closed before the session's opens.
    client.wake(None).await.unwrap();
    until_log(&log, "session open ghost.mic mic=true").await;
    {
        let l = log.lock().unwrap().clone();
        let n = l.len();
        assert_eq!(
            &l[n - 3..n - 1],
            &["wake open".to_string(), "wake closed".to_string()],
            "{l:?}"
        );
    }
    until(client, "the session", |v| {
        v["phase"] != "passive" && v["session_id"] != Value::Null
    })
    .await;
    // The greeting may still be playing; sleep until it takes.
    let deadline = Instant::now() + Duration::from_secs(20);
    while client.status().await.unwrap()["phase"] != "passive" {
        let _ = client.sleep().await;
        assert!(Instant::now() < deadline, "the session never ended");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    until_log(&log, "wake open").await;

    // Not listening (ADR 0057): the wake's input closes and nothing opens either input;
    // a typed turn's session has no microphone.
    client.set_listening(false, None).await.unwrap();
    until_log(&log, "wake closed").await;
    client.say("hello").await.unwrap();
    until_log(&log, "session open ghost.mic mic=false").await;
    assert!(
        !log.lock()
            .unwrap()
            .iter()
            .rev()
            .take(2)
            .any(|l| l == "wake open"),
        "{:?}",
        log.lock().unwrap()
    );
    w.daemon.stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(10), run).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_setting_for_a_microphone_that_is_gone_falls_back_to_one_that_is_there() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world_from(
        tmp.path(),
        "file",
        |v| v.input_device = Some("alsa_input.unplugged".into()),
        |p| p.sound_facts = Arc::new(|| Some(array_facts())),
    );
    let plan = w.daemon.mic_plan();
    // Gone: the machine's own choice, the array's processed channel, not silence.
    assert_eq!(plan.session.to_string(), format!("{ARRAY}#0"));
    assert!(
        plan.fell_back
            .as_deref()
            .unwrap()
            .contains("alsa_input.unplugged")
    );
    let session = w.daemon.audio_settings(false).input_device.unwrap();
    assert_eq!(session, format!("{ARRAY}#0"));
}
