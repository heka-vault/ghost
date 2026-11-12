//! A whole session through every stage, with fakes for the models and the scripted
//! devices for the transport: a voice turn end to end, a barge-in that stops playback
//! within 100 ms, a goodbye that ends the session, a turn that closes sooner when its end
//! is sure and is reopened when the operator goes on (ADR 0049), and a fast model that
//! speaks first and hands real work to the main one (ADR 0050).

use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// 0: renders a clause whole. More: in this many windows, each made in its share of
    /// `render` and handed on as it is made (ADR 0053).
    windows: usize,
    rendering: Rendering,
}

/// What the windowed fake did: windows handed on, renders finished, renders stopped.
#[derive(Clone, Default)]
struct Rendering(Arc<Mutex<(usize, usize, usize)>>);

impl Rendering {
    fn get(&self) -> (usize, usize, usize) {
        *self.0.lock().unwrap()
    }
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
    fn render(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
        emit: &mut dyn FnMut(Vec<i16>) -> bool,
    ) -> ghost_infer::Result<()> {
        if self.windows == 0 {
            let pcm = self.synthesize(text, voice, speed)?;
            emit(pcm);
            return Ok(());
        }
        self.spoken.lock().unwrap().push(text.to_string());
        let each = (24000.0 * self.secs) as usize / self.windows;
        for _ in 0..self.windows {
            std::thread::sleep(self.render / self.windows as u32);
            self.rendering.0.lock().unwrap().0 += 1;
            if !emit(vec![20000; each]) {
                self.rendering.0.lock().unwrap().2 += 1;
                return Ok(());
            }
        }
        self.rendering.0.lock().unwrap().1 += 1;
        Ok(())
    }
}

/// Smart Turn calling every end done, this sure of it.
struct Complete(f32);

impl TurnModel for Complete {
    fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
        Ok(TurnVerdict {
            probability: self.0,
            complete: true,
        })
    }
}

/// The scripted model, keeping the operator's turns of every context it is sent, and
/// the tools it was offered each time.
struct Asked {
    model: Arc<ScriptedModel>,
    asked: Arc<Mutex<Vec<Vec<String>>>>,
    offered: Arc<Mutex<Vec<Vec<String>>>>,
    /// Each warm-up: the tools, the system prompt and the thinking setting it carried.
    warmed: Arc<Mutex<Vec<Warmed>>>,
}

type Warmed = (Vec<String>, String, Option<String>);

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
        self.offered
            .lock()
            .unwrap()
            .push(tools.names().iter().map(|n| n.to_string()).collect());
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

    fn warm_cache(
        &self,
        context: ghost_llm::Context,
        tools: &ghost_llm::Tools,
        options: &ghost_llm::Options,
    ) -> ghost_core::source::BoxFuture<'static, Result<Option<ghost_llm::Usage>, ghost_llm::LlmError>>
    {
        self.warmed.lock().unwrap().push((
            tools.names().iter().map(|n| n.to_string()).collect(),
            context.system.clone(),
            options.thinking.clone(),
        ));
        Box::pin(async { Ok(Some(ghost_llm::Usage::default())) })
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
    /// The same for the fast model, when the session has one, and what each was offered.
    fast_asked: Arc<Mutex<Vec<Vec<String>>>>,
    offered: Arc<Mutex<Vec<Vec<String>>>>,
    fast_offered: Arc<Mutex<Vec<Vec<String>>>>,
    warmed: Arc<Mutex<Vec<Warmed>>>,
    fast_warmed: Arc<Mutex<Vec<Warmed>>>,
    /// The windowed fake's counts (ADR 0053).
    rendering: Rendering,
}

/// What a session's world is made of. `Setup::new` is the plain one.
struct Setup {
    transcripts: Vec<String>,
    replies: Vec<Vec<ScriptedReply>>,
    /// The fast model's script, when the session has one (ADR 0050).
    fast: Option<Vec<Vec<ScriptedReply>>>,
    /// A clause is this long at the speaker, and takes `render_ms` to make.
    clause_secs: f64,
    render_ms: u64,
    grace: f64,
    /// How sure Smart Turn is of every end.
    sure: f32,
    /// `GHOST_*` settings over the test's own.
    vars: Vec<(&'static str, String)>,
    sources: Vec<ToolSource>,
    /// The voice renders in this many windows a clause; 0, whole.
    windows: usize,
}

impl Setup {
    fn new(transcripts: &[&str], replies: Vec<Vec<ScriptedReply>>) -> Self {
        Self {
            transcripts: transcripts.iter().map(|s| s.to_string()).collect(),
            replies,
            fast: None,
            clause_secs: 0.3,
            render_ms: 40,
            grace: 0.1,
            sure: 0.99,
            vars: Vec::new(),
            sources: Vec::new(),
            windows: 0,
        }
    }

    fn set(mut self, name: &'static str, value: &str) -> Self {
        self.vars.push((name, value.to_string()));
        self
    }

    /// The short grace of ADR 0049, scaled: the grace `grace`, the short one `short`.
    fn graces(mut self, grace: f64, short: f64) -> Self {
        self.grace = grace;
        self.set("GHOST_TURN_CONFIDENT_GRACE_SECS", &short.to_string())
    }

    fn build(self) -> World {
        world_setup(self)
    }
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
    world_setup(Setup {
        clause_secs,
        grace,
        render_ms,
        sources,
        ..Setup::new(transcripts, replies)
    })
}

fn world_setup(setup: Setup) -> World {
    let Setup {
        transcripts,
        replies,
        fast,
        clause_secs,
        render_ms,
        grace,
        sure,
        vars,
        sources,
        windows,
    } = setup;
    let settings = VoiceSettings::from_vars(|name| {
        if let Some((_, v)) = vars.iter().find(|(k, _)| *k == name) {
            return Some(v.clone());
        }
        match name {
            "GHOST_TURN_GRACE_SECS" => Some(grace.to_string()),
            "GHOST_THINKING_DELAY_SECS" => Some("0.1".into()),
            "GHOST_LATENCY" => Some("1".into()),
            _ => None,
        }
    })
    .unwrap();
    let mut heard = transcripts;
    heard.reverse();
    let stt: SharedRecognizer = Arc::new(Mutex::new(FakeParakeet(Arc::new(Mutex::new(heard)))));
    let spoken = Arc::new(Mutex::new(Vec::new()));
    let rendering = Rendering::default();
    let tts: SharedSynth = Arc::new(Mutex::new(FakeKokoro {
        secs: clause_secs,
        render: Duration::from_millis(render_ms),
        spoken: spoken.clone(),
        windows,
        rendering: rendering.clone(),
    }));
    let models = Arc::new(Models {
        stt,
        tts,
        turn: Arc::new(Mutex::new(Complete(sure))),
        silero: "/nonexistent/silero.onnx".into(),
        voices: vec!["af_heart".into()],
        acks: rendered_acks(),
        problem: None,
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
    let offered = Arc::new(Mutex::new(Vec::new()));
    let fast_asked = Arc::new(Mutex::new(Vec::new()));
    let fast_offered = Arc::new(Mutex::new(Vec::new()));
    let warmed = Arc::new(Mutex::new(Vec::new()));
    let fast_warmed = Arc::new(Mutex::new(Vec::new()));
    let fast_model = fast.map(|script| {
        Arc::new(Asked {
            model: ScriptedModel::paced(script, Duration::from_millis(20), None, "fast"),
            asked: fast_asked.clone(),
            offered: fast_offered.clone(),
            warmed: fast_warmed.clone(),
        }) as Arc<dyn ModelPort>
    });
    let session = build_session(SessionParams {
        settings: Arc::new(settings),
        models,
        model: Arc::new(Asked {
            model: ScriptedModel::new(replies, Duration::from_millis(50)),
            asked: asked.clone(),
            offered: offered.clone(),
            warmed: warmed.clone(),
        }),
        fast_model,
        sources,
        gate,
        events: None,
        media,
        greeting: Greeting::Text("Yeah?".into()),
        first_utterance: None,
        first_typed: false,
        hooks: SessionHooks {
            on_turn: Some(on_turn),
            on_phase: None,
            on_hang_up: None,
            on_conversation: None,
            on_latency: Some(Arc::new(move |rec| r.lock().unwrap().push(rec.clone()))),
            on_level: None,
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
        fast_asked,
        offered,
        fast_offered,
        warmed,
        fast_warmed,
        rendering,
    }
}

/// What Ghost said, as the transcript has it.
fn said_by_ghost(w: &World) -> Vec<String> {
    w.turns
        .lock()
        .unwrap()
        .iter()
        .filter(|(r, _)| r == "assistant")
        .map(|(_, t)| t.clone())
        .collect()
}

/// Every latency record so far.
fn all_records(w: &World) -> Vec<Value> {
    w.records.lock().unwrap().clone()
}

/// A connected app whose one read tool counts its calls.
fn counting_app(calls: Arc<AtomicUsize>) -> ToolSource {
    let mut app = github_app();
    app.read_tools[0].handler = Arc::new(move |_| {
        let calls = calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut m = Map::new();
            m.insert("issues".into(), json!([{"number": 3, "title": "Redis"}]));
            Ok(m)
        })
    });
    app
}

async fn finish(w: &World) {
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear, &w.spoken, &w.asked);
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
    // the model is told the turn was said over its reply, whose opening words the
    // canceller loses
    say(&w, 0.3).await; // the segment ends; its final is the second transcript
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let noted = w.asked.lock().unwrap().iter().any(|users| {
            users.last().is_some_and(|u| {
                u.starts_with("stop that now") && u.contains("Said over your reply")
            })
        });
        if noted {
            break;
        }
        assert!(Instant::now() < deadline, "{:?}", w.asked.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
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
        &["read me the redis issue"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new())
                .after(Duration::from_millis(400)), // past the 0.1 s grace
            ScriptedModel::say("The redis issue is open."),
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
        said.contains(&"assistant:The redis issue is open.".to_string()),
        "{said:?}"
    );
    w.session.end().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = (&w.mouth, &w.ear, &w.spoken, &w.asked);
}

// --- a confident turn closes sooner (ADR 0049) --------------------------------------------

#[tokio::test]
async fn a_sure_end_closes_after_the_short_grace_and_a_middling_one_after_the_grace() {
    // The grace is 0.6 s and the short one 0.1 s. Smart Turn sure and a full stop: the
    // short one. The same words with Smart Turn less sure, or no stop written: the grace.
    for (sure, heard, band, hold) in [
        (0.97, "What time is it?", "confident", 0.08..0.35),
        (0.7, "What time is it?", "grace", 0.55..0.9),
        (0.97, "what time is it", "grace", 0.55..0.9),
    ] {
        let mut setup = Setup::new(&[heard], vec![vec![ScriptedModel::say("Half past nine.")]])
            .graces(0.6, 0.1);
        setup.sure = sure;
        let w = setup.build();
        w.session.start(true).await;
        wait_for(&w.records, "opening", Duration::from_secs(5)).await;
        say(&w, 0.3).await;
        let rec = voice_records(&w, 1).await.remove(0);
        assert_eq!(
            rec["hear"]["close"],
            json!(band),
            "{heard:?} at {sure}: {rec}"
        );
        let held = rec["hear"]["hold"].as_f64().unwrap();
        assert!(hold.contains(&held), "{heard:?} at {sure}: held {held}s");
        assert_eq!(rec["hear"]["speculation"]["kept"], json!(1), "{rec}");
        finish(&w).await;
    }
}

#[tokio::test]
async fn a_sure_end_the_operator_goes_on_from_is_one_turn_and_nothing_is_done_for_half_of_it() {
    // "What's waiting for triage?" sounds done and reads done, so the turn closes after
    // the short grace and the answer starts on its way to an app. The operator goes on
    // ("in the app repo") before anything was heard or done: the model gets one turn, the
    // app is called once, for the whole of it, and what was made for the half is dropped.
    let calls = Arc::new(AtomicUsize::new(0));
    let mut setup = Setup::new(
        &["What's waiting for triage?", "in the app repo"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new())
                .after(Duration::from_millis(350)),
            ScriptedModel::say("Three are waiting in the app repo."),
        ]],
    )
    .graces(0.6, 0.1);
    setup.sources = vec![counting_app(calls.clone())];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    tokio::time::sleep(Duration::from_millis(250)).await; // closed after 0.1 s, handed over
    assert_eq!(
        w.asked.lock().unwrap().last().cloned(),
        Some(vec!["What's waiting for triage?".to_string()])
    );
    // The model had its call ready long ago; it waits until the grace would have run out.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "nothing done inside the grace"
    );
    say(&w, 0.3).await; // the operator goes on
    let recs = voice_records(&w, 1).await;
    assert_eq!(
        w.asked.lock().unwrap().last().cloned(),
        Some(vec![
            "What's waiting for triage? in the app repo".to_string()
        ]),
        "one turn, whole"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the app was called for the whole turn only"
    );
    assert_eq!(recs[0]["hear"]["reopened"], json!("unheard"), "{}", recs[0]);
    assert_eq!(
        said_by_ghost(&w),
        vec!["Yeah?", "Three are waiting in the app repo."]
    );
    let closes: Vec<Value> = all_records(&w)
        .iter()
        .map(|r| r["hear"]["close"].clone())
        .filter(|c| !c.is_null())
        .collect();
    assert_eq!(closes.first(), Some(&json!("confident")), "{closes:?}");
    finish(&w).await;
}

/// A turn closed on a sure end whose answer has just reached the speaker: the greeting
/// (3 s) played, the turn was said and closed, and the reply's first clause began.
async fn the_answer_just_began(transcripts: &[&str]) -> (World, Instant) {
    let mut setup = Setup::new(
        transcripts,
        vec![vec![ScriptedModel::say(
            "Three are waiting, and the oldest is from Monday.",
        )]],
    )
    .graces(0.6, 0.1)
    // A wide window, so a slow machine's lag in seeing Ghost begin doesn't decide the test.
    .set("GHOST_TURN_REOPEN_STARTED_SECS", "0.6");
    setup.clause_secs = 3.0;
    let w = setup.build();
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
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    (w, Instant::now())
}

async fn cleared_after(w: &World, at: Instant) -> Duration {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(c) = w.ear.cleared().into_iter().find(|c| *c > at) {
            return c.duration_since(at);
        }
        assert!(Instant::now() < deadline, "the speaker was never cleared");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn words_a_moment_after_ghost_began_are_the_same_turn_not_an_interruption() {
    // The turn closed on a sure end and Ghost's first clause began; the operator was
    // going on (their speech began as Ghost's voice did). The VAD alone stops nothing
    // (it could be Ghost's own voice), so the words decide, and inside the window they
    // stop the clause, count it as not heard, and reopen the turn, where later they would
    // be an interruption of an answer that stands.
    let (w, began) =
        the_answer_just_began(&["What's waiting for triage?", "in the app repo"]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        w.ear.cleared().into_iter().all(|c| c < began),
        "the VAD alone stopped Ghost"
    );
    let at = Instant::now();
    w.session
        .inject(vec![Frame::Interim("in the app repo".into())])
        .await;
    let took = cleared_after(&w, at).await;
    assert!(took < Duration::from_millis(100), "stopped after {took:?}");
    for _ in 0..10 {
        w.session
            .inject(vec![Frame::AudioIn(Audio::new(vec![500; 320], 16000))])
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.session
        .inject(vec![Frame::VadStopped { stop_secs: 0.2 }])
        .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let last = w.asked.lock().unwrap().last().cloned();
        if last
            == Some(vec![
                "What's waiting for triage? in the app repo".to_string(),
            ])
        {
            break; // one turn, and no note that it was said over Ghost's reply
        }
        assert!(Instant::now() < deadline, "{:?}", w.asked.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Counted: how often a turn is reopened after Ghost's voice had begun.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let counted = all_records(&w)
            .iter()
            .any(|r| r["hear"]["reopened"] == "started");
        if counted {
            break;
        }
        assert!(Instant::now() < deadline, "{:#?}", all_records(&w));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.session.cancel().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
    let _ = &w.mouth;
}

// --- the voice is ready when the turn closes (ADR 0049) -----------------------------------

#[tokio::test]
async fn the_first_clause_is_rendered_while_the_turn_waits_and_plays_at_its_close() {
    // A clause takes 0.3 s to render and the grace is 0.6 s. Rendered ahead, the clause
    // is ready when the turn closes; rendered then, the reply starts 0.3 s later.
    let mut tts = Vec::new();
    for ahead in ["true", "false"] {
        let mut setup = Setup::new(
            &["what needs my sign off"],
            vec![vec![ScriptedModel::say(
                "Three issues are waiting, the oldest from Monday.",
            )]],
        )
        .set("GHOST_TTS_PRERENDER", ahead);
        setup.grace = 0.6;
        setup.render_ms = 300;
        let w = setup.build();
        w.session.start(true).await;
        wait_for(&w.records, "opening", Duration::from_secs(5)).await;
        say(&w, 0.3).await;
        let rec = voice_records(&w, 1).await.remove(0);
        tts.push(rec["tts"]["total"].as_f64().unwrap());
        if ahead == "true" {
            assert_eq!(rec["prerender"], json!({"rendered": 1, "used": 1}), "{rec}");
        } else {
            assert!(rec.get("prerender").is_none(), "{rec}");
        }
        // Either way each clause was rendered once and spoken in order.
        let deadline = Instant::now() + Duration::from_secs(5);
        while w.spoken.lock().unwrap().len() < 3 {
            assert!(Instant::now() < deadline, "{:?}", w.spoken.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            *w.spoken.lock().unwrap(),
            vec![
                "Yeah?",
                "Three issues are waiting,",
                "the oldest from Monday."
            ]
        );
        finish(&w).await;
    }
    assert!(
        tts[0] < 0.15,
        "ready at the close: voice {}s after it",
        tts[0]
    );
    assert!(
        tts[1] > 0.28,
        "rendered at the close: voice {}s after it",
        tts[1]
    );
}

/// A reply whose first sentence has no comma: the usual rule renders it whole first.
const NO_COMMA: &str = "I found three issues waiting and two of them need your sign-off today.";

/// Ghost says `reply` with the first piece cut at `first_words` (0: the usual rule); what
/// the voice rendered, how soon it was heard after the close, and the silences inside it.
async fn speaks_with_first_clause(
    first_words: &str,
    render_ms: u64,
) -> (Vec<String>, Value, Vec<Duration>) {
    let mut setup = Setup::new(
        &["what needs my sign off"],
        vec![vec![ScriptedModel::say(NO_COMMA)]],
    )
    .set("GHOST_TTS_FIRST_CLAUSE_WORDS", first_words)
    .set("GHOST_TTS_PRERENDER", "false"); // the render at the close is what is timed
    setup.render_ms = render_ms;
    setup.clause_secs = 0.3;
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    let began = Instant::now();
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while w.spoken.lock().unwrap().len() < 2 || w.ear.seconds_played() < 0.6 {
        assert!(Instant::now() < deadline, "{:?}", w.spoken.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await; // the last piece plays out
    let spoken = w.spoken.lock().unwrap().clone();
    let gaps = w.ear.gaps_after(began);
    finish(&w).await;
    (spoken, rec, gaps)
}

#[tokio::test]
async fn a_replys_first_piece_is_cut_early_and_the_rest_follows_without_a_seam() {
    // Off, the first sentence is rendered whole before any of it plays.
    let (spoken, before, _) = speaks_with_first_clause("0", 200).await;
    assert_eq!(spoken, vec!["Yeah?".to_string(), NO_COMMA.to_string()]);
    // On, it is cut before "and": the first piece plays while the rest renders, and the
    // two meet with no silence put between them.
    let (spoken, after, gaps) = speaks_with_first_clause("4", 200).await;
    assert_eq!(
        spoken,
        vec![
            "Yeah?",
            "I found three issues waiting",
            "and two of them need your sign-off today."
        ]
    );
    let gap_ms: Vec<u128> = gaps.iter().map(Duration::as_millis).collect();
    assert!(
        gaps.iter().all(|g| *g <= Duration::from_millis(40)),
        "silence inside the reply: {gap_ms:?} ms"
    );
    let _ = (before, after);
}

#[tokio::test]
async fn a_rest_that_is_not_ready_in_time_shows_as_a_gap() {
    // The ear catches a seam: the rest takes 0.45 s to render while the first piece plays
    // for 0.3 s, so the speaker runs dry between them.
    let (_, _, gaps) = speaks_with_first_clause("4", 450).await;
    assert!(
        gaps.iter().any(|g| *g >= Duration::from_millis(100)),
        "no gap seen: {gaps:?}"
    );
}

#[tokio::test]
async fn a_reopened_turn_drops_the_clause_rendered_for_its_first_half() {
    let mut setup = Setup::new(
        &["What's waiting for triage?", "in the app repo"],
        vec![vec![ScriptedModel::say("Three are waiting.")]],
    )
    .graces(0.6, 0.1);
    setup.render_ms = 800; // the voice takes its time, so nothing of the half answer is heard
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    say(&w, 0.3).await;
    let recs = voice_records(&w, 1).await;
    assert_eq!(recs[0]["hear"]["reopened"], json!("unheard"), "{}", recs[0]);
    let dropped: u64 = all_records(&w)
        .iter()
        .map(|r| r["prerender"]["dropped"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(dropped, 1, "{:#?}", all_records(&w));
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Three are waiting."]);
    finish(&w).await;
}

// --- a fast model speaks first (ADR 0050) -------------------------------------------------

#[tokio::test]
async fn the_fast_model_answers_plain_speech_and_the_main_model_is_not_asked() {
    let mut setup = Setup::new(
        &["what's a good name for a gray cat"],
        vec![vec![ScriptedModel::say("The main model's answer.")]],
    );
    setup.fast = Some(vec![vec![ScriptedModel::say("Ash, or Pepper.")]]);
    setup.sources = vec![github_app()];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Ash, or Pepper."]);
    assert!(
        w.asked.lock().unwrap().is_empty(),
        "the main model was asked"
    );
    assert_eq!(rec["model"], json!("fast"), "{rec}");
    assert!(rec.get("handover").is_none(), "{rec}");
    assert!(rec.get("ack").is_none(), "nothing to cover: {rec}");
    // It was offered what the main model is, in the same order, and one tool more.
    let fast = w.fast_offered.lock().unwrap().last().cloned().unwrap();
    assert_eq!(fast.last().map(String::as_str), Some("think_harder"));
    assert!(
        fast.contains(&"github_search_issues".to_string()),
        "{fast:?}"
    );
    finish(&w).await;
    let _ = &w.offered;
}

#[tokio::test]
async fn a_tool_call_from_the_fast_model_hands_the_turn_to_the_main_model() {
    // The fast model writes a long preamble (past what the narration cut holds back) and
    // then reaches for an app. None of that is spoken or kept and its call is not run:
    // the main model takes the turn, an acknowledgment covers the wait, and the main
    // model's own call runs, once.
    let preamble = "Let me have a careful look at the tracker for you, because there is a \
        lot going on in it this week and I would not want to miss anything important that \
        was filed since we last spoke, so bear with me while I go through every open item \
        one at a time and sort them.";
    assert!(preamble.len() > 240);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut setup = Setup::new(
        &["any critical issues in quotient"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new())
                .after(Duration::from_millis(300)), // past the 0.1 s grace: the turn's own
            ScriptedModel::say("Two are critical."),
        ]],
    )
    .set("GHOST_TTS_PRERENDER", "false"); // so the voice renders only what it speaks
    // ADR 0050 as first written: any call hands the turn over.
    setup.vars.push(("GHOST_LLM_FAST_READS", "false".into()));
    setup.fast = Some(vec![vec![ScriptedModel::call(
        preamble,
        "github_search_issues",
        Map::new(),
    )]]);
    setup.sources = vec![counting_app(calls.clone())];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "only the main model's call ran"
    );
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Two are critical."]);
    let rendered = w.spoken.lock().unwrap().clone();
    assert!(
        !rendered.iter().any(|t| t.contains("careful look")),
        "the fast model's words reached the voice: {rendered:?}"
    );
    let ack = rec["ack"]
        .as_f64()
        .expect("an acknowledgment covers the hand-over");
    assert!(ack < rec["reply"].as_f64().unwrap(), "{rec}");
    // The main model saw the turn as the operator said it, with nothing of the fast
    // model's in between, and was not offered the hand-over tool.
    assert_eq!(
        w.asked.lock().unwrap().first().cloned(),
        Some(vec!["any critical issues in quotient".to_string()])
    );
    let main = w.offered.lock().unwrap().first().cloned().unwrap();
    assert!(!main.contains(&"think_harder".to_string()), "{main:?}");
    let fast = w.fast_offered.lock().unwrap().first().cloned().unwrap();
    assert_eq!(
        fast[..fast.len() - 1],
        main[..],
        "the same tools, in the same order"
    );
    assert_eq!(
        w.fast_asked.lock().unwrap().len(),
        1,
        "one turn, one fast call"
    );
    finish(&w).await;
}

#[tokio::test]
async fn think_harder_hands_the_turn_over_and_is_never_run() {
    let mut setup = Setup::new(
        &["how should we split the billing service"],
        vec![vec![ScriptedModel::say("By tenant first, then by region.")]],
    )
    .set("GHOST_TTS_PRERENDER", "false");
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "Hmm, that is a hard one.",
        "think_harder",
        Map::new(),
    )]]);
    setup.sources = vec![github_app()];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert!(
        rec["tools"].is_null(),
        "think_harder is not a call that runs: {rec}"
    );
    assert!(
        rec["ack"].is_number(),
        "an acknowledgment covers the gap: {rec}"
    );
    assert_eq!(
        said_by_ghost(&w),
        vec!["Yeah?", "By tenant first, then by region."]
    );
    let rendered = w.spoken.lock().unwrap().clone();
    assert!(
        !rendered.iter().any(|t| t.contains("hard one")),
        "{rendered:?}"
    );
    // The context holds the operator's turn and the main model's answer, nothing between.
    let context = w.session.context.lock().unwrap().messages.clone();
    let texts: Vec<String> = context
        .iter()
        .filter_map(|m| match m {
            ghost_llm::Message::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["By tenant first, then by region."]);
    finish(&w).await;
}

#[tokio::test]
async fn after_a_tool_result_the_main_model_answers_not_the_fast_one() {
    // The fast model is for a turn's first call. What follows a tool's result, and the
    // next turn's follow-up call, are the main model's.
    let mut setup = Setup::new(
        &["any critical issues in quotient", "thanks"],
        vec![
            vec![
                ScriptedModel::call("", "github_search_issues", Map::new())
                    .after(Duration::from_millis(300)), // past the 0.1 s grace
                ScriptedModel::say("Two are critical."),
            ],
            vec![ScriptedModel::say("The main model would say more.")],
        ],
    );
    // ADR 0050 as first written: any call hands the turn over.
    setup.vars.push(("GHOST_LLM_FAST_READS", "false".into()));
    setup.fast = Some(vec![
        vec![ScriptedModel::call("", "github_search_issues", Map::new())],
        vec![ScriptedModel::say("Any time.")],
    ]);
    setup.sources = vec![github_app()];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    voice_records(&w, 1).await;
    assert_eq!(w.fast_asked.lock().unwrap().len(), 1);
    assert_eq!(
        w.asked.lock().unwrap().len(),
        2,
        "the call, then the answer"
    );
    say(&w, 0.3).await;
    voice_records(&w, 2).await;
    assert_eq!(
        said_by_ghost(&w),
        vec!["Yeah?", "Two are critical.", "Any time."]
    );
    assert_eq!(w.fast_asked.lock().unwrap().len(), 2);
    assert_eq!(
        w.asked.lock().unwrap().len(),
        2,
        "the second turn was the fast model's"
    );
    finish(&w).await;
}

#[tokio::test]
async fn a_held_answer_handed_over_and_then_withdrawn_does_not_ask_the_fast_model_twice() {
    // Inside the grace: the fast model hands the held answer over, the main model reaches
    // its tool call before the turn has ended and withdraws (no tool runs for a turn that
    // hadn't ended). At the close the turn's own run goes straight to the main model.
    let calls = Arc::new(AtomicUsize::new(0));
    let mut setup = Setup::new(
        &["any critical issues in quotient"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new()),
            ScriptedModel::say("Two are critical."),
        ]],
    );
    setup.grace = 0.6;
    // ADR 0050 as first written: any call hands the turn over.
    setup.vars.push(("GHOST_LLM_FAST_READS", "false".into()));
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "github_search_issues",
        Map::new(),
    )]]);
    setup.sources = vec![counting_app(calls.clone())];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(w.fast_asked.lock().unwrap().len(), 1, "asked once");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert!(rec["ack"].is_number(), "{rec}");
    assert_eq!(rec["hear"]["speculation"]["tool"], json!(1), "{rec}");
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Two are critical."]);
    finish(&w).await;
}

#[tokio::test]
async fn a_quick_models_call_waits_until_the_grace_would_have_run_out() {
    // The turn closes after the short grace so that Ghost can speak sooner. What it
    // does is another matter: a model with its tool call ready in 50 ms must not run it
    // while the operator may only be pausing. The call waits out the grace (0.6 s here),
    // as it did when the grace was the only wait.
    let calls = Arc::new(AtomicUsize::new(0));
    let mut setup = Setup::new(
        &["What's waiting for triage?"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new()),
            ScriptedModel::say("Three are waiting."),
        ]],
    )
    .graces(0.6, 0.1);
    setup.sources = vec![counting_app(calls.clone())];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    tokio::time::sleep(Duration::from_millis(400)).await; // closed at 0.1 s, the call long ready
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "nothing is done inside the grace"
    );
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(rec["hear"]["close"], json!("confident"), "{rec}");
    let ran = rec["tools"]["calls"][0]["at"].as_f64().unwrap();
    assert!(
        ran >= 0.75,
        "the call ran {ran}s after the speech ended: {rec}"
    );
    finish(&w).await;
}

// --- the fast model reads for itself ------------------------------------------------------

/// A connected app with one read (counted, its result `result_chars` long) and one change.
fn app_with_a_change(reads: Arc<AtomicUsize>, result_chars: usize) -> ToolSource {
    use ghost_core::source::Operation;
    let mut app = counting_app(reads.clone());
    app.read_tools[0].handler = Arc::new(move |_| {
        let reads = reads.clone();
        Box::pin(async move {
            reads.fetch_add(1, Ordering::SeqCst);
            let mut m = Map::new();
            m.insert("issues".into(), json!("x".repeat(result_chars)));
            Ok(m)
        })
    });
    app.operations = vec![Operation {
        tool_name: Some("github_close_issue".into()),
        ..Operation::new(
            "close_issue",
            "Close an issue.",
            Arc::new(|_| Box::pin(async { anyhow::bail!("not in this test") })),
            Arc::new(ghost_core::fakes::FakeWriter::default()),
        )
    }];
    app
}

fn fast_reads(setup: Setup, max_chars: usize) -> Setup {
    setup
        .set("GHOST_LLM_FAST_READS", "true")
        .set("GHOST_LLM_FAST_READ_MAX_CHARS", &max_chars.to_string())
}

#[tokio::test]
async fn the_fast_model_reads_and_answers_from_a_short_result_without_the_main_model() {
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = fast_reads(
        Setup::new(
            &["any critical issues in quotient"],
            vec![vec![ScriptedModel::say("The main model was asked.")]],
        ),
        2000,
    );
    setup.fast = Some(vec![vec![
        ScriptedModel::call("", "github_search_issues", Map::new()),
        ScriptedModel::say("One is critical: the Redis one."),
    ]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 200)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "the fast model's read ran once"
    );
    assert!(
        w.asked.lock().unwrap().is_empty(),
        "the main model was asked"
    );
    assert_eq!(
        said_by_ghost(&w),
        vec!["Yeah?", "One is critical: the Redis one."]
    );
    assert!(rec.get("handover").is_none(), "{rec}");
    assert!(rec["ack"].is_number(), "the lookup is acknowledged: {rec}");
    assert_eq!(
        w.fast_asked.lock().unwrap().len(),
        2,
        "the read, then the answer"
    );
    finish(&w).await;
}

#[tokio::test]
async fn after_a_long_result_the_main_model_answers() {
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = fast_reads(
        Setup::new(
            &["any critical issues in quotient"],
            vec![vec![ScriptedModel::say("Of the many, two are critical.")]],
        ),
        2000,
    );
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "github_search_issues",
        Map::new(),
    )]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 5000)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    voice_records(&w, 1).await;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(w.fast_asked.lock().unwrap().len(), 1, "only the read");
    assert_eq!(w.asked.lock().unwrap().len(), 1, "the main model answered");
    assert_eq!(
        said_by_ghost(&w),
        vec!["Yeah?", "Of the many, two are critical."]
    );
    finish(&w).await;
}

#[tokio::test]
async fn a_change_from_the_fast_model_still_hands_the_turn_over() {
    // Reads are the fast model's; a change is always the main model's to make.
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = fast_reads(
        Setup::new(
            &["close the redis one"],
            vec![vec![
                ScriptedModel::call("", "github_close_issue", Map::new())
                    .after(Duration::from_millis(300)),
                ScriptedModel::say("I couldn't close it."),
            ]],
        ),
        2000,
    );
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "github_close_issue",
        Map::new(),
    )]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 200)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert_eq!(w.fast_asked.lock().unwrap().len(), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "I couldn't close it."]);
    finish(&w).await;
}

#[tokio::test]
async fn think_harder_and_a_read_after_its_rounds_hand_over_with_reads_on() {
    // think_harder is a hand-over whatever else is on.
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = fast_reads(
        Setup::new(
            &["how should we split the billing service"],
            vec![vec![ScriptedModel::say("By tenant first.")]],
        ),
        2000,
    )
    .set("GHOST_LLM_FAST_READ_ROUNDS", "1");
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "think_harder",
        Map::new(),
    )]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 200)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "By tenant first."]);
    finish(&w).await;

    // One round of reads: a second read is the main model's to make.
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = fast_reads(
        Setup::new(
            &["any critical issues in quotient"],
            vec![vec![
                ScriptedModel::call("", "github_search_issues", Map::new()),
                ScriptedModel::say("Two are critical."),
            ]],
        ),
        2000,
    )
    .set("GHOST_LLM_FAST_READ_ROUNDS", "1");
    setup.fast = Some(vec![vec![
        ScriptedModel::call("", "github_search_issues", Map::new()),
        ScriptedModel::call("", "github_search_issues", Map::new()),
    ]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 200)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert_eq!(reads.load(Ordering::SeqCst), 2, "one read each");
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Two are critical."]);
    finish(&w).await;
}

#[tokio::test]
async fn with_reads_off_any_call_hands_over_as_before() {
    let reads = Arc::new(AtomicUsize::new(0));
    let mut setup = Setup::new(
        &["any critical issues in quotient"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new())
                .after(Duration::from_millis(300)),
            ScriptedModel::say("Two are critical."),
        ]],
    );
    // ADR 0050 as first written: any call hands the turn over.
    setup.vars.push(("GHOST_LLM_FAST_READS", "false".into()));
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "github_search_issues",
        Map::new(),
    )]]);
    setup.sources = vec![app_with_a_change(reads.clone(), 200)];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    assert_eq!(rec["handover"]["n"], json!(1), "{rec}");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "the main model's read only"
    );
    finish(&w).await;
}

// --- a voice that starts as it is made (ADR 0053) -----------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_windowed_voice_plays_its_first_window_before_the_piece_is_rendered() {
    // A 3 s clause made in six windows of 0.15 s each: the speaker has the first while the
    // other five are still being made.
    let w = Setup {
        clause_secs: 3.0,
        render_ms: 900,
        windows: 6,
        ..Setup::new(&["hello"], vec![])
    }
    .build();
    w.session.start(true).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while w.ear.seconds_played() <= 0.0 {
        assert!(Instant::now() < deadline, "the greeting never played");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (windows, finished, _) = w.rendering.get();
    assert_eq!(
        finished, 0,
        "the whole piece was rendered before any of it played"
    );
    assert!(
        windows < 6,
        "{windows} windows were made before the first sound"
    );
    // The rest arrives in time: the clause plays out whole, with no window lost.
    let deadline = Instant::now() + Duration::from_secs(6);
    while w.ear.seconds_played() < 2.99 {
        assert!(
            Instant::now() < deadline,
            "played {}",
            w.ear.seconds_played()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(w.rendering.get().1, 1);
    w.session.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_barge_in_stops_a_windowed_voice_within_100ms_and_stops_its_render() {
    // As `a_barge_in_stops_playback_within_100ms`, with the reply made a window at a time:
    // the speaker is cleared as fast, and the windows not yet made are never made.
    let w = Setup {
        clause_secs: 3.0,
        render_ms: 1200,
        windows: 6,
        ..Setup::new(
            &["tell me everything", "stop that now"],
            vec![vec![ScriptedModel::say(
                "Here is everything, in great detail, for a long while.",
            )]],
        )
    }
    .build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
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
    // The reply's render stops at its next window instead of making the rest.
    let deadline = Instant::now() + Duration::from_secs(2);
    while w.rendering.get().2 == 0 {
        assert!(
            Instant::now() < deadline,
            "the render went on after the barge-in: {:?}",
            w.rendering.get()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    w.session.cancel().await;
}

#[tokio::test]
async fn a_lookup_the_fast_model_makes_itself_is_covered_by_its_own_words() {
    // 2026-11-12, live: the fast model went straight to a read of its own, nothing was
    // acknowledged (no hand-over, and its words before the call were dropped), and the
    // operator heard the thinking sound for five seconds. Its words before the call are
    // said as the call starts, as an acknowledgment is: before the answer, not in the
    // transcript.
    let mut setup = Setup::new(
        &["any critical issues in quotient"],
        vec![vec![ScriptedModel::say("The main model was not needed.")]],
    )
    .set("GHOST_TTS_PRERENDER", "false");
    setup.fast = Some(vec![vec![
        ScriptedModel::call("Checking quotient.", "github_search_issues", Map::new()),
        ScriptedModel::say("Two are critical."),
    ]]);
    setup.sources = vec![github_app()];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    say(&w, 0.3).await;
    let rec = voice_records(&w, 1).await.remove(0);
    let ack = rec["ack"].as_f64().expect("the lookup was acknowledged");
    assert!(ack < rec["reply"].as_f64().unwrap(), "{rec}");
    let rendered = w.spoken.lock().unwrap().clone();
    assert!(
        rendered.iter().any(|t| t.contains("Checking quotient")),
        "{rendered:?}"
    );
    assert_eq!(said_by_ghost(&w), vec!["Yeah?", "Two are critical."]);
    assert!(
        w.asked.lock().unwrap().is_empty(),
        "the main model was asked"
    );
    finish(&w).await;
}

#[tokio::test]
async fn each_models_prompt_cache_is_warmed_as_the_session_opens_with_its_first_calls_prefix() {
    // 2026-11-12, live: every session's first turn found both models' prompts cold
    // (10,000 to 13,000 tokens written, first byte 1.9 to 2.7 s). The cache is written as
    // the session opens, with what the first call will send, so the first turn reads it.
    let mut setup = Setup::new(
        &["any critical issues in quotient"],
        vec![vec![ScriptedModel::say("Two are critical.")]],
    );
    setup.fast = Some(vec![vec![ScriptedModel::call(
        "",
        "think_harder",
        Map::new(),
    )]]);
    setup.sources = vec![github_app()];
    let w = setup.build();
    w.session.start(true).await;
    wait_for(&w.records, "opening", Duration::from_secs(5)).await;
    let main = w.warmed.lock().unwrap().clone();
    let fast = w.fast_warmed.lock().unwrap().clone();
    assert_eq!(main.len(), 1, "the main model, once: {main:?}");
    assert_eq!(fast.len(), 1, "the fast model, once: {fast:?}");
    assert!(
        main[0].2.is_some(),
        "with the main model's thinking setting"
    );
    assert_eq!(
        fast[0].2.as_deref(),
        Some("disabled"),
        "the fast model's own setting"
    );
    say(&w, 0.3).await;
    voice_records(&w, 1).await;
    // What each was warmed with is what its first call was offered, in the same order.
    assert_eq!(Some(&main[0].0), w.offered.lock().unwrap().first());
    assert_eq!(Some(&fast[0].0), w.fast_offered.lock().unwrap().first());
    assert_eq!(main[0].1, fast[0].1, "one system prompt");
    finish(&w).await;
}
