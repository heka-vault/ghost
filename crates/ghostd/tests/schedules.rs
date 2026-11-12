//! A schedule in a running daemon (ADR 0047): made over the control API, run beside an
//! open session, its answer on the waiting list, and the session none the wiser. Fake
//! models, a scripted LLM, silent media, a temp folder and a fake keyring: never the
//! operator's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::BoxStream;
use ghost_api::GhostClient;
use ghost_core::MemoryKeyring;
use ghost_core::source::BoxFuture;
use ghost_infer::TurnVerdict;
use ghost_llm::{Context, Event, LlmError, Message, Options, Tools};
use ghost_voice::VoiceSettings;
use ghost_voice::llm::{ModelPort, ScriptedModel};
use ghost_voice::pipeline::Models;
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::transport::{MediaIn, Silent, scripted_media};
use ghost_voice::tts::{SharedSynth, Synth};
use ghost_voice::turns::TurnModel;
use ghostd::config::GhostdSettings;
use ghostd::daemon::{Daemon, Ports};
use hyper::Method;
use serde_json::{Value, json};

struct NoSpeech;

impl Recognizer for NoSpeech {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        Ok(String::new())
    }
}

struct Quick;

impl Synth for Quick {
    fn voices(&self) -> Vec<String> {
        vec!["af_heart".into()]
    }
    fn synthesize(
        &mut self,
        _text: &str,
        _voice: &str,
        _speed: f32,
    ) -> ghost_infer::Result<Vec<i16>> {
        Ok(vec![20000; 2400])
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

/// One model for the session and for a run: a run is told apart by how its request
/// starts, and every context either was given is kept.
struct Split {
    session: Arc<ScriptedModel>,
    run: Arc<ScriptedModel>,
    runs_saw: Arc<Mutex<Vec<Context>>>,
}

fn is_run(context: &Context) -> bool {
    context
        .messages
        .iter()
        .any(|m| matches!(m, Message::User(t) if t.starts_with("[scheduled run:")))
}

impl ModelPort for Split {
    fn stream(
        &self,
        context: Context,
        tools: &Tools,
        options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        if is_run(&context) {
            self.runs_saw.lock().unwrap().push(context.clone());
            self.run.stream(context, tools, options)
        } else {
            self.session.stream(context, tools, options)
        }
    }

    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        self.session.complete(system, user, max_tokens)
    }

    fn model_name(&self) -> String {
        "scripted".into()
    }
}

struct World {
    daemon: Arc<Daemon>,
    client: GhostClient,
    runs_saw: Arc<Mutex<Vec<Context>>>,
}

fn world(tmp: &std::path::Path) -> World {
    let t = |p: &str| tmp.join(p);
    let cfg = GhostdSettings {
        data_dir: t("data"),
        db_path: t("data/ghost.db"),
        policy_path: t("config/policy.toml"),
        config_dir: t("config"),
        catalog_path: t("cache/catalog.json"),
        skills_dir: t("config/skills"),
        home_skills_dir: t("home-skills"),
        api_socket: Some(t("ghost.sock")),
        settings_path: t("config/settings.env"),
        env_path: t("config/env"),
        vault_env_path: t("no-heka"),
        infra_dir: t("infra"),
        wake_mode: "file".into(),
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
        "GHOST_TURN_GRACE_SECS" => Some("0.1".into()),
        _ => None,
    })
    .unwrap();
    std::fs::create_dir_all(tmp.join("rt")).unwrap();
    let runs_saw: Arc<Mutex<Vec<Context>>> = Arc::new(Mutex::new(Vec::new()));
    let mut ports = Ports::headless();
    ports.keyring = Arc::new(MemoryKeyring::new());
    ports.load_models = Arc::new(|s: &VoiceSettings| {
        let stt: SharedRecognizer = Arc::new(Mutex::new(NoSpeech));
        let tts: SharedSynth = Arc::new(Mutex::new(Quick));
        Ok(Models {
            stt,
            tts,
            turn: Arc::new(Mutex::new(Complete)),
            silero: s.silero_model.clone(),
            voices: vec!["af_heart".into()],
            acks: ghost_voice::acks::Acks::new(None),
        })
    });
    ports.model = {
        let runs_saw = runs_saw.clone();
        Arc::new(move |_: &VoiceSettings, _: &HashMap<String, String>| {
            let fast = Duration::from_millis(10);
            Ok(Arc::new(Split {
                session: ScriptedModel::new(
                    vec![vec![ScriptedModel::say("Hello from the session.")]],
                    fast,
                ),
                run: ScriptedModel::new(
                    vec![vec![ScriptedModel::say(
                        "All quiet on main: the build is green.",
                    )]],
                    fast,
                ),
                runs_saw: runs_saw.clone(),
            }) as Arc<dyn ModelPort>)
        })
    };
    ports.desk_media = Arc::new(|_req| {
        Box::pin(async {
            let (media, _mouth, _ear) = scripted_media();
            Ok(media)
        })
    });
    ports.passive_media = Some(Arc::new(|| Ok(Box::new(Silent) as Box<dyn MediaIn>)));
    let daemon = Arc::new(
        Daemon::new(cfg.clone(), voice, ports)
            .with_runtime_dir(tmp.join("rt"))
            .with_models_retry(Duration::from_millis(200)),
    );
    let client = GhostClient::with_timeout(cfg.api_socket.clone().unwrap(), Duration::from_secs(5));
    World {
        daemon,
        client,
        runs_saw,
    }
}

async fn get(client: &GhostClient, path: &str) -> Value {
    client
        .request(Method::GET, path, None)
        .await
        .map(|r| r.body)
        .unwrap_or(Value::Null)
}

async fn until<F: Fn(&Value) -> bool>(
    client: &GhostClient,
    path: &str,
    what: &str,
    ok: F,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let v = get(client, path).await;
        if ok(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "never: {what} (last: {v})");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn answered(turns: &Value, text: &str) -> bool {
    turns.as_array().is_some_and(|t| {
        t.iter()
            .any(|x| x["role"] == "assistant" && x["text"].as_str().unwrap_or("").contains(text))
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_schedule_runs_beside_an_open_session_and_the_session_is_not_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let w = world(tmp.path());
    let run = {
        let d = w.daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let client = &w.client;
    until(client, "/status", "passive with the models ready", |v| {
        v["phase"] == "passive" && v["models_ready"] == true
    })
    .await;
    // The schedules are served with nothing set up, and every agent has the two tools.
    let empty = get(client, "/schedules").await;
    assert_eq!(empty["schedules"], json!([]));
    assert_eq!(empty["problems"], json!([]));
    let ghost = w
        .daemon
        .conversations()
        .unwrap()
        .get("ghost")
        .unwrap()
        .unwrap();
    let reach = w.daemon.reach().unwrap()(&ghost);
    assert!(
        reach[0]
            .read_tools
            .iter()
            .any(|t| t.name == "ghost_schedules_list")
    );
    assert!(
        reach[0]
            .operations
            .iter()
            .any(|o| o.tool_name.as_deref() == Some("ghost_schedule_manage"))
    );

    // A session is open and the operator has said something in it.
    assert_eq!(client.say("hello there").await.unwrap()["ok"], true);
    let st = until(client, "/status", "a session", |v| v["phase"] != "passive").await;
    let session = st["session_id"].as_str().unwrap().to_string();
    until(client, "/session/turns", "the session's reply", |t| {
        answered(t, "Hello from the session.")
    })
    .await;
    let turns_before = get(client, "/session/turns").await;

    // The operator adds a schedule and runs it now, while the session is open.
    let made = client
        .request(
            Method::POST,
            "/schedules",
            Some(&json!({
                "name": "build check",
                "instruction": "Check the build and say if it is red.",
                "every": "1h",
            })),
        )
        .await
        .unwrap();
    assert_eq!(made.status, 200, "{}", made.body);
    let id = made.body["id"].as_str().unwrap().to_string();
    assert_eq!(made.body["agent"], "ghost");
    let started = client
        .request(Method::POST, &format!("/schedules/{id}/run"), None)
        .await
        .unwrap();
    assert_eq!(started.status, 200, "{}", started.body);
    let runs = until(
        client,
        &format!("/schedules/{id}/runs"),
        "the run's result",
        |v| v["runs"][0]["status"] == "ok",
    )
    .await;
    assert_eq!(
        runs["runs"][0]["answer"],
        "All quiet on main: the build is green."
    );
    assert_eq!(runs["runs"][0]["delivery"], "delivered");
    assert_eq!(runs["runs"][0]["model_calls"], 1);

    // Its answer is on the waiting list, under the agent, for the routes to deliver.
    let waiting = get(client, "/waiting").await;
    let item = waiting["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["source"] == "agent:ghost")
        .unwrap_or_else(|| panic!("no item: {waiting}"));
    assert_eq!(
        item["title"],
        "build check: All quiet on main: the build is green."
    );
    // The run was the agent's prompt for writing, with the session's words in its memory
    // of the conversation and none of the session's tools.
    let seen = w.runs_saw.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].system.contains("working on your own now"));
    assert!(!seen[0].system.contains("spoken aloud"));

    // The session is as it was: the same one, still open, with only its own turns, and
    // it still answers.
    let st = get(client, "/status").await;
    assert_eq!(st["session_id"], session.as_str());
    assert_ne!(st["phase"], "passive");
    assert_eq!(st["conversation"], "ghost");
    let turns_after = get(client, "/session/turns").await;
    assert_eq!(turns_after, turns_before);
    assert!(!turns_after.to_string().contains("build is green"));
    assert_eq!(client.say("still there?").await.unwrap()["ok"], true);
    until(
        client,
        "/session/turns",
        "a second reply in the session",
        |t| {
            t.as_array().is_some_and(|t| {
                t.iter().filter(|x| x["role"] == "assistant").count() >= 2
                    && t.iter().all(|x| x["session_id"] == session.as_str())
            })
        },
    )
    .await;

    // The run is in Ghost's transcript, marked, under its own session id; and after the
    // session ends the schedule is still there, with its next time.
    let convs = w.daemon.conversations().unwrap();
    let kept = convs.turns("ghost", 100).unwrap();
    let of_run: Vec<_> = kept
        .iter()
        .filter(|t| t.session_id.starts_with("run-"))
        .collect();
    assert!(
        of_run.iter().any(
            |t| t.text == "[scheduled run: build check] All quiet on main: the build is green."
        )
    );
    assert!(kept.iter().any(|t| t.session_id == session));
    client.sleep().await.unwrap();
    until(client, "/status", "passive again", |v| {
        v["phase"] == "passive"
    })
    .await;
    let after = get(client, &format!("/schedules/{id}")).await;
    assert_eq!(after["last_outcome"], "ok");
    assert_eq!(after["enabled"], true);
    assert!(after["next_run"].is_string());

    w.daemon.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the daemon stops")
        .unwrap()
        .unwrap();
}
