//! A whole session through every stage, with fakes for the models and the scripted
//! devices for the transport: a voice turn end to end, a barge-in that stops playback
//! within 100 ms, and a goodbye that ends the session.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_core::source::{ReadTool, ToolSource};
use ghost_core::{AutonomyPolicy, Store, WriteGate};
use ghost_infer::TurnVerdict;
use ghost_voice::acks::{Acks, CHANGE, Clip, READ, Voiced};
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Frame};
use ghost_voice::llm::{ModelPort, ScriptedModel, ScriptedReply};
use ghost_voice::pipeline::{Greeting, Models, SessionHooks, SessionParams, build_session};
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::tap::OnTurn;
use ghost_voice::transport::{Ear, Mouth, scripted_media};
use ghost_voice::tts::{SharedSynth, Synth};
use ghost_voice::turns::TurnModel;
use serde_json::{Map, Value, json};

struct FakeParakeet(Arc<Mutex<Vec<String>>>);

impl Recognizer for FakeParakeet {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        std::thread::sleep(Duration::from_millis(30)); // Parakeet's 0.16 s, scaled
        Ok(self.0.lock().unwrap().pop().unwrap_or_default())
    }
}

/// A clause takes `secs` of audio at 24 kHz and `render` to make (Kokoro's is about 0.5 s).
struct FakeKokoro {
    secs: f64,
    render: Duration,
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
        std::thread::sleep(self.render);
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

/// The scripted model, keeping the operator's turns of every context it is sent.
struct Asked {
    model: Arc<ScriptedModel>,
    asked: Arc<Mutex<Vec<Vec<String>>>>,
}

impl ModelPort for Asked {
    fn stream(
        &self,
        context: ghost_llm::Context,
        tools: &ghost_llm::Tools,
        options: &ghost_llm::Options,
    ) -> futures::stream::BoxStream<'static, Result<ghost_llm::Event, ghost_llm::LlmError>> {
        let users = context
            .messages
            .iter()
            .filter_map(|m| match m {
                ghost_llm::Message::User(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        self.asked.lock().unwrap().push(users);
        self.model.stream(context, tools, options)
    }

    fn complete(
        &self,
        system: String,
        user: String,
        max_tokens: u32,
    ) -> ghost_core::source::BoxFuture<'_, Result<String, ghost_llm::LlmError>> {
        self.model.complete(system, user, max_tokens)
    }

    fn model_name(&self) -> String {
        self.model.model_name()
    }
}

struct World {
    session: ghost_voice::pipeline::GhostSession,
    mouth: Mouth,
    ear: Ear,
    records: Arc<Mutex<Vec<Value>>>,
    turns: Arc<Mutex<Vec<(String, String)>>>,
    spoken: Arc<Mutex<Vec<String>>>,
    /// The operator's turns in each context the model was sent.
    asked: Arc<Mutex<Vec<Vec<String>>>>,
}

fn world(
    transcripts: &[&str],
    replies: Vec<Vec<ScriptedReply>>,
    clause_secs: f64,
    grace: f64,
    render_ms: u64,
) -> World {
    world_with(
        transcripts,
        replies,
        clause_secs,
        grace,
        render_ms,
        Vec::new(),
    )
}

/// The acknowledgments, rendered ahead as the daemon does at load: short and loud.
fn rendered_acks() -> Arc<Acks> {
    let clips = |lines: &[&'static str]| -> Vec<Clip> {
        lines
            .iter()
            .map(|text| Clip {
                text,
                pcm: Arc::new(vec![20000; 24000 / 5]),
            })
            .collect()
    };
    let acks = Acks::new(None);
    acks.insert(
        "af_heart",
        1.0,
        Voiced {
            read: clips(READ),
            change: clips(CHANGE),
        },
    );
    acks
}

/// A connected app with one read tool.
fn github_app() -> ToolSource {
    ToolSource {
        name: "github".into(),
        title: "GitHub".into(),
        app: true,
        read_tools: vec![ReadTool {
            name: "github_search_issues".into(),
            description: "Search issues.".into(),
            properties: json!({}),
            required: Vec::new(),
            handler: Arc::new(|_| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await; // the app, away
                    let mut m = Map::new();
                    m.insert("issues".into(), json!([{"number": 3, "title": "Redis"}]));
                    Ok(m)
                })
            }),
            operation: String::new(),
        }],
        ..Default::default()
    }
}

fn world_with(
    transcripts: &[&str],
    replies: Vec<Vec<ScriptedReply>>,
    clause_secs: f64,
    grace: f64,
    render_ms: u64,
    sources: Vec<ToolSource>,
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
        render: Duration::from_millis(render_ms),
        spoken: spoken.clone(),
    }));
    let models = Arc::new(Models {
        stt,
        tts,
        turn: Arc::new(Mutex::new(Complete)),
        silero: "/nonexistent/silero.onnx".into(),
        voices: vec!["af_heart".into()],
        acks: rendered_acks(),
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
    let asked = Arc::new(Mutex::new(Vec::new()));
    let session = build_session(SessionParams {
        settings: Arc::new(settings),
        models,
        model: Arc::new(Asked {
            model: ScriptedModel::new(replies, Duration::from_millis(50)),
            asked: asked.clone(),
        }),
        sources,
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
        asked,
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
        40,
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
    // A long reply (3 s a clause); three heard words while Ghost speaks (GHOST_BARGE_IN_MIN_WORDS) cut it off.
    let w = world(
        &["tell me everything", "stop that now"],
        vec![vec![ScriptedModel::say(
            "Here is everything, in great detail, for a long while.",
        )]],
        3.0,
        0.1,
        40,
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
        .inject(vec![Frame::Interim("stop that now".into())])
        .await;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(c) = w.ear.cleared().into_iter().find(|c| *c > at) {
            let took = c.duration_since(at);
            eprintln!("barge-in: playback cleared {took:?} after the operator's words");
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

/// Ghost's reply just reaching the speaker, as the echo tests want it: the greeting (3 s)
/// has played, the reply's first clause has started, and the TTS stage is still rendering
/// the second and third (0.5 s each, Kokoro's pace), with the `BotStarted` frame queued
/// behind that work. The first half second of a reply is where the echo canceller leaks.
async fn speaking_a_long_reply(transcripts: &[&str]) -> World {
    let w = world(
        transcripts,
        vec![vec![ScriptedModel::say(
            "Here is everything, in great detail, for a long while.",
        )]],
        3.0,
        0.1,
        500,
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while w.ear.seconds_played() < 2.99 {
        assert!(Instant::now() < deadline, "the greeting never finished");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    say(&w, 0.3).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while w.ear.seconds_played() < 3.01 {
        assert!(Instant::now() < deadline, "Ghost never spoke the reply");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    w
}

/// The reply plays on for `secs` after `at`: nothing cleared the speaker and more audio
/// reached it.
async fn plays_on(w: &World, at: Instant, secs: f64) {
    let before = w.ear.seconds_played();
    tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    let cleared: Vec<_> = w.ear.cleared().into_iter().filter(|c| *c > at).collect();
    assert!(cleared.is_empty(), "the speaker was cleared {cleared:?}");
    assert!(
        w.ear.seconds_played() > before,
        "Ghost stopped speaking: {before} s played before, {} after",
        w.ear.seconds_played()
    );
}

#[tokio::test]
async fn a_vad_start_alone_while_ghost_speaks_does_not_cut_it_off() {
    // Ghost's own voice past the echo canceller is VAD activity and nothing more. The
    // output stage says Ghost is speaking through the shared flag the moment it writes the
    // first sample; the BotStarted frame reaches the barge-in rule only once the TTS stage
    // has rendered the reply, which for a long reply is seconds later.
    let w = speaking_a_long_reply(&["tell me everything"]).await;
    let at = Instant::now();
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    plays_on(&w, at, 1.5).await;
    w.session.cancel().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = &w.mouth;
}

#[tokio::test]
async fn the_replys_own_words_heard_while_it_plays_do_not_cut_it_off() {
    // What leaks past the canceller reads as the words Ghost is saying right now. The
    // echo filter knows them a clause ahead of the speaker and drops the interim.
    let w = speaking_a_long_reply(&["tell me everything"]).await;
    let at = Instant::now();
    w.session
        .inject(vec![
            Frame::VadStarted { start_secs: 0.2 },
            Frame::Interim("here is everything".into()),
        ])
        .await;
    plays_on(&w, at, 1.5).await;
    w.session.cancel().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = &w.mouth;
}

/// The voice turns' latency records, once `n` of them have been spoken.
async fn voice_records(w: &World, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let recs: Vec<Value> = w
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "voice" && r["spoke"] == true)
            .cloned()
            .collect();
        if recs.len() >= n {
            return recs;
        }
        assert!(
            Instant::now() < deadline,
            "{} voice turn(s) spoken, wanted {n}: {:#?}",
            recs.len(),
            w.records.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_request_that_goes_to_an_app_is_acknowledged_before_the_answer() {
    let w = world_with(
        &["any critical issues in quotient", "read out number three"],
        vec![
            vec![
                ScriptedModel::call("Let me check.", "github_search_issues", Map::new()),
                ScriptedModel::say("Two are critical."),
            ],
            vec![
                ScriptedModel::call("", "github_search_issues", Map::new()),
                ScriptedModel::say("Number three is the Redis timeout."),
            ],
        ],
        0.2,
        0.1,
        40,
        vec![github_app()],
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let first = voice_records(&w, 1).await;
    say(&w, 0.3).await;
    let recs = voice_records(&w, 2).await;
    let _ = first;
    for rec in &recs {
        let ack = rec["ack"].as_f64().expect("acknowledged");
        let reply = rec["reply"].as_f64().unwrap();
        assert!(
            ack < reply,
            "the acknowledgment {ack} s came after the answer {reply} s"
        );
    }
    // Not a turn: no acknowledgment in the transcript, and the model's narration dropped.
    let said: Vec<String> = w
        .turns
        .lock()
        .unwrap()
        .iter()
        .map(|(r, t)| format!("{r}:{t}"))
        .collect();
    for line in READ.iter().chain(CHANGE) {
        assert!(
            !said.iter().any(|s| s.ends_with(line)),
            "{line} reached the transcript: {said:?}"
        );
    }
    assert!(
        said.contains(&"assistant:Two are critical.".to_string()),
        "{said:?}"
    );
    assert!(!said.iter().any(|s| s.contains("Let me check")), "{said:?}");
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear, &w.spoken);
}

#[tokio::test]
async fn an_answer_ghost_already_has_is_not_acknowledged() {
    let w = world_with(
        &["what was the first one"],
        vec![vec![ScriptedModel::say("The Redis timeout.")]],
        0.2,
        0.1,
        40,
        vec![github_app()],
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let recs = voice_records(&w, 1).await;
    assert!(recs[0].get("ack").is_none(), "{:#?}", recs[0]);
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear);
}

#[tokio::test]
async fn a_goodbye_ends_the_session_without_the_model() {
    let w = world(&["That's all for now, bye."], vec![vec![]], 0.2, 0.1, 40);
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

#[tokio::test]
async fn going_on_before_ghost_answers_is_one_turn() {
    // Smart Turn takes the pause in "can you count ... to three" for an end, and the
    // operator goes on while the answer is still being voiced: the model gets one turn,
    // not two halves.
    let w = world(
        &["Can you count", "to three"],
        vec![vec![ScriptedModel::say("One, two, three.")]],
        0.3,
        0.1,
        800, // the voice takes its time, so nothing of the first answer is heard
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    tokio::time::sleep(Duration::from_millis(400)).await; // the turn is handed over
    assert_eq!(
        w.asked.lock().unwrap().last().cloned(),
        Some(vec!["Can you count".to_string()])
    );
    say(&w, 0.3).await;
    let turn = wait_for(&w.records, "voice", Duration::from_secs(10)).await;
    assert!(turn.is_some(), "records: {:#?}", w.records.lock().unwrap());
    assert_eq!(
        w.asked.lock().unwrap().last().cloned(),
        Some(vec!["Can you count to three".to_string()])
    );
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear);
}

#[tokio::test]
async fn a_call_the_held_answer_reaches_after_the_close_is_still_made() {
    // Live, 2026-10-30: a 1.5 s first token against a 1.2 s grace. The close confirmed
    // the held answer while it was still streaming; when it then reached its tool call it
    // withdrew itself as if the turn hadn't ended, the confirmed stream ended with nothing,
    // and Ghost sat silent after every request that needed an app.
    let w = world_with(
        &["read me issue four one three"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new())
                .after(Duration::from_millis(400)), // past the 0.1 s grace
            ScriptedModel::say("Issue four one three is open."),
        ]],
        0.2,
        0.1,
        40,
        vec![github_app()],
    );
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let recs = voice_records(&w, 1).await;
    let rec = &recs[0];
    assert_eq!(
        rec["hear"]["speculation"]["kept"],
        json!(1),
        "the close confirmed the held answer: {rec}"
    );
    assert!(rec["ack"].is_number(), "the call was acknowledged: {rec}");
    let said: Vec<String> = w
        .turns
        .lock()
        .unwrap()
        .iter()
        .map(|(r, t)| format!("{r}:{t}"))
        .collect();
    assert!(
        said.contains(&"assistant:Issue four one three is open.".to_string()),
        "{said:?}"
    );
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear, &w.spoken, &w.asked);
}
