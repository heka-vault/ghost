//! The realtime voice in a whole session (ADR 0063), against the scripted provider on a
//! local WebSocket (`ghost_voice::realtime::fake`), with no key and no network: small talk
//! the voice answers itself, a lookup the agent answers and the voice says exactly, a
//! change that asks and is confirmed by the operator's own yes, a blocked change and a
//! reading that claims otherwise, an interruption mid-answer, the provider dropping, the
//! day's cap, and a key refused at the start.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_core::fakes::{FakeWriter, changeset};
use ghost_core::models::{FakeIssue, SetLabels};
use ghost_core::source::{Operation, Prepared, ReadTool};
use ghost_core::{AutonomyPolicy, Change, Risk, Store, Target, ToolSource, WriteGate};
use ghost_infer::TurnVerdict;
use ghost_voice::acks::Acks;
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Frame};
use ghost_voice::llm::{ModelPort, ScriptedModel, ScriptedReply};
use ghost_voice::pipeline::{Greeting, Models, SessionHooks, SessionParams, build_session};
use ghost_voice::realtime::fake::{FakeRealtime, Reply};
use ghost_voice::realtime::openai::OpenAiRealtime;
use ghost_voice::realtime::stage::{RealtimeParams, START_SENTENCE};
use ghost_voice::realtime::{CAP_SENTENCE, FALLBACK_SENTENCE, Failure, SAY_EXACTLY};
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::tap::OnTurn;
use ghost_voice::transport::{Ear, Mouth, scripted_media};
use ghost_voice::tts::{SharedSynth, Synth};
use ghost_voice::turns::TurnModel;
use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

const KEY: &str = "sk-fake-realtime";

struct FakeParakeet(Arc<Mutex<Vec<String>>>);

impl Recognizer for FakeParakeet {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(self.0.lock().unwrap().pop().unwrap_or_default())
    }
}

/// Kokoro, the local voice: what it was asked to say.
struct FakeKokoro(Arc<Mutex<Vec<String>>>);

impl Synth for FakeKokoro {
    fn voices(&self) -> Vec<String> {
        vec!["af_heart".into()]
    }
    fn synthesize(&mut self, text: &str, _v: &str, _s: f32) -> ghost_infer::Result<Vec<i16>> {
        self.0.lock().unwrap().push(text.to_string());
        Ok(vec![20000; 24000 / 10])
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

/// The agent's model, keeping the operator's turns of every context it was sent.
struct Agent {
    model: Arc<ScriptedModel>,
    asked: Arc<Mutex<Vec<Vec<String>>>>,
}

impl ModelPort for Agent {
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
        "agent".into()
    }
}

fn doc(name: &str) -> FakeIssue {
    FakeIssue {
        owner: "doc".into(),
        repo: name.into(),
        number: 1,
    }
}

/// An app with one read and two changes: `nuke` asks, `never` is blocked.
fn app(writer: Arc<FakeWriter>) -> ToolSource {
    let read = ReadTool {
        name: "github_search_issues".into(),
        description: "Search issues.".into(),
        properties: json!({}),
        required: Vec::new(),
        handler: Arc::new(|_| {
            Box::pin(async move {
                let mut m = Map::new();
                m.insert("issues".into(), json!([{"number": 3}, {"number": 7}]));
                Ok(m)
            })
        }),
        operation: String::new(),
    };
    let op = |name: &str| {
        let w = writer.clone();
        Operation {
            name: name.into(),
            description: format!("{name}."),
            properties: json!({"name": {"type": "string"}}),
            required: vec!["name".into()],
            prepare: Arc::new(move |a| {
                Box::pin(async move {
                    let name = a
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Ok(Prepared {
                        target: Target::FakeIssue(doc(&name)),
                        changes: changeset(vec![Change::SetLabels(SetLabels {
                            add: vec!["flagged".into()],
                            remove: vec![],
                        })]),
                        rationale: String::new(),
                        asked_by: Vec::new(),
                    })
                })
            }),
            writer: w,
            risk: Risk::High,
            allowed: Arc::new(|_| true),
            asked_by: Vec::new(),
            tool_name: None,
        }
    };
    ToolSource {
        name: "fake".into(),
        title: "Fake".into(),
        app: true,
        read_tools: vec![read],
        operations: vec![op("nuke"), op("never")],
        ..Default::default()
    }
}

struct World {
    session: ghost_voice::pipeline::GhostSession,
    fake: FakeRealtime,
    turns: Arc<Mutex<Vec<(String, String)>>>,
    local: Arc<Mutex<Vec<String>>>,
    asked: Arc<Mutex<Vec<Vec<String>>>>,
    records: Arc<Mutex<Vec<Value>>>,
    screen: Arc<Mutex<Vec<String>>>,
    fallbacks: Arc<Mutex<Vec<Failure>>>,
    writer: Arc<FakeWriter>,
    ear: Ear,
    _mouth: Mouth,
}

struct Setup {
    transcripts: Vec<&'static str>,
    agent: Vec<Vec<ScriptedReply>>,
    key: &'static str,
    /// The day's spend at which the cap is reached; None: no cap.
    cap: Option<f64>,
}

impl Setup {
    fn new(transcripts: &[&'static str], agent: Vec<Vec<ScriptedReply>>) -> Self {
        Self {
            transcripts: transcripts.to_vec(),
            agent,
            key: KEY,
            cap: None,
        }
    }
}

async fn world(setup: Setup) -> World {
    let fake = FakeRealtime::start(KEY).await;
    let settings = VoiceSettings::from_vars(|name| match name {
        "GHOST_TURN_GRACE_SECS" => Some("0.1".into()),
        "GHOST_THINKING_DELAY_SECS" => Some("0.1".into()),
        "GHOST_LATENCY" => Some("1".into()),
        _ => None,
    })
    .unwrap();
    let mut heard: Vec<String> = setup.transcripts.iter().map(|s| s.to_string()).collect();
    heard.reverse();
    let stt: SharedRecognizer = Arc::new(Mutex::new(FakeParakeet(Arc::new(Mutex::new(heard)))));
    let local = Arc::new(Mutex::new(Vec::new()));
    let tts: SharedSynth = Arc::new(Mutex::new(FakeKokoro(local.clone())));
    let models = Arc::new(Models {
        stt,
        tts,
        turn: Arc::new(Mutex::new(Complete)),
        silero: "/nonexistent/silero.onnx".into(),
        voices: vec!["af_heart".into()],
        acks: Acks::new(None),
        problem: None,
    });
    let writer = Arc::new(FakeWriter::default());
    for n in ["readme", "secret"] {
        writer.seed(&doc(n));
    }
    let source = app(writer.clone());
    let gate = Arc::new(
        WriteGate::new(Arc::new(Store::in_memory().unwrap())).with_autonomy(
            AutonomyPolicy::parse(
                "[permissions]\nblock = [\"fake.never\"]\nask = [\"fake.nuke\"]\n",
                "test",
            )
            .unwrap(),
        ),
    );
    gate.register("fake", source.operations.clone());
    let (media, mouth, ear) = scripted_media();
    let turns = Arc::new(Mutex::new(Vec::new()));
    let t = turns.clone();
    let on_turn: OnTurn = Arc::new(move |role, text| {
        let t = t.clone();
        Box::pin(async move {
            t.lock().unwrap().push((role.to_string(), text));
        })
    });
    let records = Arc::new(Mutex::new(Vec::new()));
    let r = records.clone();
    let screen = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let fallbacks = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(OpenAiRealtime::new(
        fake.url.clone(),
        Zeroizing::new(setup.key.to_string()),
        "gpt-realtime-2.1-mini",
        "marin",
        500,
        Some(ghost_voice::realtime::settings::OPENAI.models[0].prices),
    ));
    let spent = Arc::new(Mutex::new(0.0f64));
    let cap = setup.cap;
    let session = build_session(SessionParams {
        settings: Arc::new(settings),
        models,
        model: Arc::new(Agent {
            model: ScriptedModel::new(setup.agent, Duration::from_millis(30)),
            asked: asked.clone(),
        }),
        fast_model: None,
        sources: vec![source],
        gate,
        events: None,
        media,
        greeting: Greeting::Text("Yeah?".into()),
        first_utterance: None,
        first_typed: false,
        hooks: SessionHooks {
            on_turn: Some(on_turn),
            on_latency: Some(Arc::new(move |rec| r.lock().unwrap().push(rec.clone()))),
            on_level: Some(Arc::new(|_| {})),
            on_heard: Some({
                let screen = screen.clone();
                Arc::new(move |rec| {
                    if let ghost_voice::tap::TurnRecord::Closed(t) = rec {
                        screen.lock().unwrap().push(format!("heard:{t}"));
                    }
                    Box::pin(async {})
                })
            }),
            ..SessionHooks::default()
        },
        wake_phrases: None,
        wake_fuzzy: 0.0,
        conversations: None,
        start_in: "ghost".into(),
        latency_log: None,
        reach: None,
        context_note: None,
        realtime: Some(RealtimeParams {
            provider,
            on_usage: Some(Arc::new(move |_u, usd| {
                let mut s = spent.lock().unwrap();
                *s += usd;
                cap.is_some_and(|c| *s >= c)
            })),
            on_fallback: Some({
                let f = fallbacks.clone();
                Arc::new(move |failure: &Failure| f.lock().unwrap().push(failure.clone()))
            }),
            connect_timeout: Duration::from_secs(3),
        }),
    });
    World {
        session,
        fake,
        turns,
        local,
        asked,
        records,
        screen,
        fallbacks,
        writer,
        ear,
        _mouth: mouth,
    }
}

async fn until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The operator speaks: a VAD segment the local recogniser reads.
async fn say(w: &World, secs: f64) {
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    for _ in 0..(secs / 0.02) as usize {
        w.session
            .inject(vec![Frame::AudioIn(Audio::new(vec![500; 320], 16000))])
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.session
        .inject(vec![Frame::VadStopped { stop_secs: 0.2 }])
        .await;
}

/// Ghost has stopped speaking: nothing more has played for a while.
async fn quiet(w: &World) {
    let mut last = w.ear.seconds_played();
    let mut still = 0;
    while still < 4 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = w.ear.seconds_played();
        if (now - last).abs() < 1e-9 {
            still += 1;
        } else {
            still = 0;
            last = now;
        }
    }
    tokio::time::sleep(Duration::from_millis(900)).await; // past the echo window
}

/// What the local voice said, its clauses joined.
fn local_said(w: &World) -> String {
    w.local.lock().unwrap().join(" ")
}

fn made(w: &World, name: &str) -> bool {
    w.writer
        .issue(&doc(name))
        .labels
        .iter()
        .any(|l| l == "flagged")
}

fn said(w: &World, role: &str) -> Vec<String> {
    w.turns
        .lock()
        .unwrap()
        .iter()
        .filter(|(r, _)| r == role)
        .map(|(_, t)| t.clone())
        .collect()
}

fn realtime_records(w: &World, what: &str) -> Vec<Value> {
    w.records
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "realtime" && r["what"] == what)
        .cloned()
        .collect()
}

/// The session opened and the realtime voice said the greeting.
async fn opened(w: &World) {
    w.session.start(true).await;
    until(
        "the greeting said by the realtime voice",
        Duration::from_secs(5),
        || said(w, "assistant").contains(&"Yeah?".to_string()),
    )
    .await;
}

/// The operator says something the realtime voice hears too.
async fn turn(w: &World, words: &str, reply: Option<Reply>) {
    say(w, 0.3).await;
    let want = format!("heard:{words}");
    until(
        "the turn heard on this machine",
        Duration::from_secs(5),
        || w.screen.lock().unwrap().contains(&want),
    )
    .await;
    w.fake.heard(reply).await;
}

async fn end(w: World) {
    w.session.cancel().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), w.session.run()).await;
}

#[tokio::test]
async fn small_talk_is_the_voices_own_and_never_reaches_the_agent() {
    let w = world(Setup::new(&["how are you"], vec![])).await;
    opened(&w).await;
    // The greeting was a relay: said exactly, with no tool allowed.
    let creates = w.fake.of("response.create");
    assert_eq!(
        creates[0]["response"]["instructions"],
        format!("{SAY_EXACTLY}Yeah?")
    );
    assert_eq!(creates[0]["response"]["tool_choice"], "none");
    turn(
        &w,
        "how are you",
        Some(Reply::Talk("Doing well, thanks.".into())),
    )
    .await;
    until(
        "the voice's answer in the transcript",
        Duration::from_secs(5),
        || said(&w, "assistant").contains(&"Doing well, thanks.".to_string()),
    )
    .await;
    assert!(
        w.asked.lock().unwrap().is_empty(),
        "the agent was not asked"
    );
    assert!(
        w.local.lock().unwrap().is_empty(),
        "the local voice said nothing"
    );
    // The words of record are the ones heard on this machine.
    assert!(said(&w, "user").contains(&"how are you".to_string()));
    assert!(
        w.fake.audio_bytes() > 0,
        "the operator's audio went to the provider"
    );
    assert!(w.ear.seconds_played() > 0.0);
    let talk = realtime_records(&w, "talk");
    assert!(!talk.is_empty(), "{:?}", w.records.lock().unwrap());
    assert!(
        w.records
            .lock()
            .unwrap()
            .iter()
            .all(|r| r["voice"] == "realtime")
    );
    end(w).await;
}

#[tokio::test]
async fn a_lookup_is_the_agents_and_its_answer_is_said_exactly() {
    let w = world(Setup::new(
        &["anything critical in acme"],
        vec![vec![
            ScriptedModel::call("", "github_search_issues", Map::new()),
            ScriptedModel::say("Two are critical: 3 and 7."),
        ]],
    ))
    .await;
    opened(&w).await;
    turn(
        &w,
        "anything critical in acme",
        Some(Reply::Consult {
            ack: "One sec.".into(),
            request: "critical issues in acme".into(),
        }),
    )
    .await;
    until("the answer said", Duration::from_secs(8), || {
        said(&w, "assistant").contains(&"Two are critical: 3 and 7.".to_string())
    })
    .await;
    // The agent got the operator's own words and the voice's reading, marked as that.
    let asked = w.asked.lock().unwrap().clone();
    let first = &asked[0][0];
    assert!(first.starts_with("anything critical in acme"), "{first}");
    assert!(
        first.contains("passed this on as: \"critical issues in acme\""),
        "{first}"
    );
    // The consult was answered, and the answer was asked for word for word.
    let outputs = w.fake.of("conversation.item.create");
    assert_eq!(outputs[0]["item"]["type"], "function_call_output");
    let last = w.fake.of("response.create").last().cloned().unwrap();
    assert_eq!(
        last["response"]["instructions"],
        format!("{SAY_EXACTLY}Two are critical: 3 and 7.")
    );
    assert!(
        w.local.lock().unwrap().is_empty(),
        "{:?}",
        w.local.lock().unwrap()
    );
    until("its record", Duration::from_secs(3), || {
        !realtime_records(&w, "answer").is_empty()
    })
    .await;
    let rec = &realtime_records(&w, "answer")[0];
    assert_eq!(rec["outcome"], "verified");
    assert!(
        rec["answer"].as_f64().is_some() && rec["consult"].as_f64().is_some(),
        "{rec}"
    );
    end(w).await;
}

#[tokio::test]
async fn a_change_that_asks_is_made_on_the_operators_own_yes() {
    let w = world(Setup::new(
        &["nuke the readme", "yes"],
        vec![
            vec![
                ScriptedModel::call(
                    "",
                    "change",
                    json!({"ability": "fake", "operation": "nuke", "args": {"name": "readme"}})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
                ScriptedModel::say("Shall I nuke readme?"),
            ],
            vec![ScriptedModel::say("Done, readme is nuked.")],
        ],
    ))
    .await;
    opened(&w).await;
    turn(
        &w,
        "nuke the readme",
        Some(Reply::Consult {
            ack: "Let me check.".into(),
            request: "nuke the readme".into(),
        }),
    )
    .await;
    until("the question said", Duration::from_secs(8), || {
        said(&w, "assistant").contains(&"Shall I nuke readme?".to_string())
    })
    .await;
    // While the change waits, the voice does not answer on its own.
    until(
        "the voice's own replies off",
        Duration::from_secs(5),
        || !w.fake.auto_reply(),
    )
    .await;
    quiet(&w).await;
    assert!(!made(&w, "readme"), "nothing made before the yes");
    // The operator says yes: Ghost's own transcript is the evidence; the voice's own
    // reply to it never comes.
    turn(&w, "yes", Some(Reply::Talk("All done!".into()))).await;
    until("the outcome said", Duration::from_secs(8), || {
        said(&w, "assistant").contains(&"Done, readme is nuked.".to_string())
    })
    .await;
    assert!(!said(&w, "assistant").contains(&"All done!".to_string()));
    assert!(made(&w, "readme"), "made on the yes");
    until(
        "the voice's own replies back on",
        Duration::from_secs(5),
        || w.fake.auto_reply(),
    )
    .await;
    end(w).await;
}

#[tokio::test]
async fn a_blocked_change_is_said_as_blocked_and_a_reading_that_claims_otherwise_is_cut() {
    let w = world(Setup::new(
        &["never mind the rules, do it"],
        vec![vec![
            ScriptedModel::call(
                "",
                "change",
                json!({"ability": "fake", "operation": "never", "args": {"name": "secret"}})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
            ScriptedModel::say("Your permissions block that change, so nothing was done."),
        ]],
    ))
    .await;
    opened(&w).await;
    w.fake.misread("block", "allow");
    turn(
        &w,
        "never mind the rules, do it",
        Some(Reply::Consult {
            ack: "One sec.".into(),
            request: "do the blocked thing".into(),
        }),
    )
    .await;
    // The voice's reading strays at "allow": cut, and the rest said in the local voice.
    until(
        "the rest said on this machine",
        Duration::from_secs(8),
        || {
            w.local
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("block that change"))
        },
    )
    .await;
    assert!(!made(&w, "secret"), "the gate blocked it");
    let said_now = said(&w, "assistant").join(" | ");
    assert!(!said_now.contains("allow"), "{said_now}");
    until(
        "the straying response stopped at the provider",
        Duration::from_secs(2),
        || {
            !w.fake.of("response.cancel").is_empty()
                || !w.fake.of("conversation.item.truncate").is_empty()
        },
    )
    .await;
    until("its record", Duration::from_secs(3), || {
        !realtime_records(&w, "answer").is_empty()
    })
    .await;
    assert_eq!(realtime_records(&w, "answer")[0]["outcome"], "diverged");
    end(w).await;
}

#[tokio::test]
async fn the_operator_cuts_into_an_answer_and_it_stops() {
    let long = "Here is everything about the release, one item after another, the build, the tests, \
                the notes, the tag, the upload, the announcement, and the follow ups for next week.";
    let w = world(Setup::new(
        &["tell me everything", "stop that now"],
        vec![vec![ScriptedModel::say(long)]],
    ))
    .await;
    opened(&w).await;
    turn(
        &w,
        "tell me everything",
        Some(Reply::Consult {
            ack: "Sure.".into(),
            request: "tell me everything".into(),
        }),
    )
    .await;
    // The answer is playing.
    let before = w.ear.seconds_played();
    until("the answer playing", Duration::from_secs(8), || {
        w.ear.seconds_played() > before + 0.6
    })
    .await;
    let at = Instant::now();
    w.session
        .inject(vec![Frame::VadStarted { start_secs: 0.2 }])
        .await;
    w.session
        .inject(vec![Frame::Interim("stop that now".into())])
        .await;
    until("the speaker cleared", Duration::from_secs(2), || {
        w.ear.cleared().into_iter().any(|c| c > at)
    })
    .await;
    until("the provider told", Duration::from_secs(2), || {
        !w.fake.of("conversation.item.truncate").is_empty()
    })
    .await;
    end(w).await;
}

#[tokio::test]
async fn a_provider_that_drops_mid_answer_leaves_one_sentence_and_the_local_voice() {
    let w = world(Setup::new(
        &["what needs me", "and the second one"],
        vec![
            vec![ScriptedModel::say(
                "Three things need you today, the oldest from Monday morning.",
            )],
            vec![ScriptedModel::say("The second is the invoice.")],
        ],
    ))
    .await;
    opened(&w).await;
    w.fake.drop_mid_relay();
    turn(
        &w,
        "what needs me",
        Some(Reply::Consult {
            ack: "One sec.".into(),
            request: "what needs me".into(),
        }),
    )
    .await;
    until(
        "the sentence said on this machine",
        Duration::from_secs(8),
        || {
            w.local
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("I lost the realtime voice"))
        },
    )
    .await;
    assert!(FALLBACK_SENTENCE.starts_with("I lost the realtime voice"));
    until(
        "the rest of the answer said on this machine",
        Duration::from_secs(5),
        || {
            w.local
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("Monday morning"))
        },
    )
    .await;
    assert!(matches!(
        w.fallbacks.lock().unwrap()[0],
        Failure::Dropped(_)
    ));
    // The next turn is the local pipeline's: the agent answers, Kokoro speaks.
    quiet(&w).await;
    say(&w, 0.3).await;
    until(
        "the next answer said locally",
        Duration::from_secs(8),
        || {
            w.local
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("invoice"))
        },
    )
    .await;
    end(w).await;
}

#[tokio::test]
async fn the_days_cap_hands_the_session_to_the_local_voice() {
    let mut setup = Setup::new(&[], vec![]);
    setup.cap = Some(0.0);
    let w = world(setup).await;
    w.session.start(true).await;
    until("the cap's sentence", Duration::from_secs(8), || {
        local_said(&w).contains(CAP_SENTENCE)
    })
    .await;
    assert_eq!(w.fallbacks.lock().unwrap()[0], Failure::Cap);
    end(w).await;
}

#[tokio::test]
async fn a_key_refused_at_the_start_is_said_and_the_session_is_local() {
    let mut setup = Setup::new(
        &["what needs me"],
        vec![vec![ScriptedModel::say("Nothing today.")]],
    );
    setup.key = "sk-wrong";
    let w = world(setup).await;
    w.session.start(true).await;
    until(
        "the start sentence and the greeting, locally",
        Duration::from_secs(8),
        || {
            let l = local_said(&w);
            l.contains(START_SENTENCE) && l.contains("Yeah?")
        },
    )
    .await;
    assert!(matches!(
        w.fallbacks.lock().unwrap()[0],
        Failure::Refused(_)
    ));
    assert_eq!(w.fake.wrong_keys(), 1);
    say(&w, 0.3).await;
    until("the agent answers locally", Duration::from_secs(8), || {
        w.local
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Nothing today"))
    })
    .await;
    end(w).await;
}
