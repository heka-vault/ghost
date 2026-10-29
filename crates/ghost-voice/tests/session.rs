//! A whole session through every stage, with fakes for the models and the scripted
//! devices for the transport: a voice turn end to end, a barge-in that stops playback
//! within 100 ms, and a goodbye that ends the session.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_core::{AutonomyPolicy, Store, WriteGate};
use ghost_infer::TurnVerdict;
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Frame};
use ghost_voice::llm::{ScriptedModel, ScriptedReply};
use ghost_voice::pipeline::{Greeting, Models, SessionHooks, SessionParams, build_session};
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::tap::OnTurn;
use ghost_voice::transport::{Ear, Mouth, scripted_media};
use ghost_voice::tts::{SharedSynth, Synth};
use ghost_voice::turns::TurnModel;
use serde_json::Value;

struct FakeParakeet(Arc<Mutex<Vec<String>>>);

impl Recognizer for FakeParakeet {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        std::thread::sleep(Duration::from_millis(30)); // Parakeet's 0.16 s, scaled
        Ok(self.0.lock().unwrap().pop().unwrap_or_default())
    }
}

/// A clause takes `secs` of audio at 24 kHz.
struct FakeKokoro {
    secs: f64,
    spoken: Arc<Mutex<Vec<String>>>,
}

impl Synth for FakeKokoro {
    fn voices(&self) -> Vec<String> {
        vec!["af_heart".into()]
    }
    fn synthesize(
        &mut self,
        text: &str,
        _voice: &str,
        _speed: f32,
    ) -> ghost_infer::Result<Vec<i16>> {
        self.spoken.lock().unwrap().push(text.to_string());
        std::thread::sleep(Duration::from_millis(40)); // Kokoro's render, scaled
        Ok(vec![20000; (24000.0 * self.secs) as usize])
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

struct World {
    session: ghost_voice::pipeline::GhostSession,
    mouth: Mouth,
    ear: Ear,
    records: Arc<Mutex<Vec<Value>>>,
    turns: Arc<Mutex<Vec<(String, String)>>>,
    spoken: Arc<Mutex<Vec<String>>>,
}

fn world(
    transcripts: &[&str],
    replies: Vec<Vec<ScriptedReply>>,
    clause_secs: f64,
    grace: f64,
) -> World {
    let settings = VoiceSettings::from_vars(|name| match name {
        "GHOST_TURN_GRACE_SECS" => Some(grace.to_string()),
        "GHOST_THINKING_DELAY_SECS" => Some("0.1".into()),
        "GHOST_LATENCY" => Some("1".into()),
        _ => None,
    })
    .unwrap();
    let mut heard: Vec<String> = transcripts.iter().map(|s| s.to_string()).collect();
    heard.reverse();
    let stt: SharedRecognizer = Arc::new(Mutex::new(FakeParakeet(Arc::new(Mutex::new(heard)))));
    let spoken = Arc::new(Mutex::new(Vec::new()));
    let tts: SharedSynth = Arc::new(Mutex::new(FakeKokoro {
        secs: clause_secs,
        spoken: spoken.clone(),
    }));
    let models = Arc::new(Models {
        stt,
        tts,
        turn: Arc::new(Mutex::new(Complete)),
        silero: "/nonexistent/silero.onnx".into(),
        voices: vec!["af_heart".into()],
    });
    let gate = Arc::new(
        WriteGate::new(Arc::new(Store::in_memory().unwrap()))
            .with_autonomy(AutonomyPolicy::default()),
    );
    let (media, mouth, ear) = scripted_media();
    let records = Arc::new(Mutex::new(Vec::new()));
    let r = records.clone();
    let turns = Arc::new(Mutex::new(Vec::new()));
    let t = turns.clone();
    let on_turn: OnTurn = Arc::new(move |role, text| {
        let t = t.clone();
        Box::pin(async move {
            t.lock().unwrap().push((role.to_string(), text));
        })
    });
    let session = build_session(SessionParams {
        settings: Arc::new(settings),
        models,
        model: ScriptedModel::new(replies, Duration::from_millis(50)),
        sources: Vec::new(),
        gate,
        events: None,
        media,
        greeting: Greeting::Text("Yeah?".into()),
        first_utterance: None,
        hooks: SessionHooks {
            on_turn: Some(on_turn),
            on_phase: None,
            on_hang_up: None,
            on_conversation: None,
            on_latency: Some(Arc::new(move |rec| r.lock().unwrap().push(rec.clone()))),
        },
        wake_phrases: None,
        wake_fuzzy: 0.0,
        conversations: None,
        start_in: "ghost".into(),
        latency_log: None,
        reach: None,
        context_note: None,
    });
    World {
        session,
        mouth,
        ear,
        records,
        turns,
        spoken,
    }
}

/// The operator says something: a VAD segment with `secs` of audio, as the STT stage
/// takes it from a transport with its own VAD.
async fn say(w: &World, secs: f64) {
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    let frames = (secs / 0.02) as usize;
    for _ in 0..frames {
        w.session
            .inject(vec![Frame::AudioIn(Audio::new(vec![500; 320], 16000))])
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.session
        .inject(vec![Frame::VadStopped { stop_secs: 0.2 }])
        .await;
}

async fn wait_for(records: &Arc<Mutex<Vec<Value>>>, kind: &str, within: Duration) -> Option<Value> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(r) = records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == kind && r["spoke"] == true)
        {
            return Some(r.clone());
        }
        if Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_voice_turn_runs_through_every_stage() {
    let w = world(
        &["What needs my sign off"],
        vec![vec![ScriptedModel::say(
            "Three issues are waiting, the oldest from Monday.",
        )]],
        0.3,
        0.1,
    );
    w.session.start(true).await;
    let opening = wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    assert!(opening.is_some(), "the greeting was spoken");
    say(&w, 0.4).await;
    let turn = wait_for(&w.records, "voice", Duration::from_secs(10)).await;
    if turn.is_none() {
        eprintln!("records: {:#?}", w.records.lock().unwrap());
        eprintln!("turns: {:?}", w.turns.lock().unwrap());
        eprintln!("spoken: {:?}", w.spoken.lock().unwrap());
    }
    let turn = turn.expect("a spoken reply");
    assert!(turn["reply"].as_f64().unwrap() > 0.0);
    assert!(turn["hear"]["total"].as_f64().is_some());
    assert_eq!(turn["llm"]["calls"].as_array().unwrap().len(), 1);
    assert!(turn["tts"]["total"].as_f64().is_some());
    let said: Vec<String> = w
        .turns
        .lock()
        .unwrap()
        .iter()
        .map(|(r, t)| format!("{r}:{t}"))
        .collect();
    assert!(
        said.contains(&"user:What needs my sign off".to_string()),
        "{said:?}"
    );
    assert!(
        said.contains(&"assistant:Three issues are waiting, the oldest from Monday.".to_string()),
        "{said:?}"
    );
    assert_eq!(
        *w.spoken.lock().unwrap(),
        vec![
            "Yeah?",
            "Three issues are waiting,",
            "the oldest from Monday."
        ]
    );
    w.session.end().await;
    tokio::time::timeout(Duration::from_secs(5), w.session.run())
        .await
        .expect("the session ends");
    let _ = (&w.mouth, &w.ear);
}

#[tokio::test]
async fn a_barge_in_stops_playback_within_100ms() {
    // A long reply (3 s a clause); two heard words while Ghost speaks cut it off.
    let w = world(
        &["tell me everything", "stop that"],
        vec![vec![ScriptedModel::say(
            "Here is everything, in great detail, for a long while.",
        )]],
        3.0,
        0.1,
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    // wait until Ghost is speaking the reply (the greeting took the first 3 s)
    let deadline = Instant::now() + Duration::from_secs(10);
    while w.ear.seconds_played() < 3.5 {
        assert!(Instant::now() < deadline, "Ghost never spoke the reply");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let at = Instant::now();
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    w.session
        .inject(vec![Frame::Interim("stop that".into())])
        .await;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(c) = w.ear.cleared().into_iter().find(|c| *c > at) {
            let took = c.duration_since(at);
            assert!(
                took < Duration::from_millis(100),
                "cleared {took:?} after the barge-in"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the barge-in never cleared the speaker"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    w.session.cancel().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = &w.mouth;
}

#[tokio::test]
async fn a_goodbye_ends_the_session_without_the_model() {
    let w = world(&["That's all for now, bye."], vec![vec![]], 0.2, 0.1);
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    tokio::time::timeout(Duration::from_secs(10), w.session.run())
        .await
        .expect("the goodbye ends the session");
    let recs = w.records.lock().unwrap().clone();
    let goodbye = recs
        .iter()
        .find(|r| r["intercepted"] == "quick_intents")
        .expect("intercepted");
    assert!(goodbye["llm"].is_null());
    assert!(w.spoken.lock().unwrap().len() >= 2); // the greeting and the goodbye
}
