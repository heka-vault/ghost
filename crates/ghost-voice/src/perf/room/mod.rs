//! `ghost-perf room`: a session in a noisy room (`make perf-room`,
//! [docs/benchmarks/room-harness.md](../../../../../docs/benchmarks/room-harness.md)).
//!
//! A person at the desk (a Kokoro voice, close and dry) talks to Ghost through the whole
//! session pipeline (`build_session`: the real Silero, Parakeet, Smart Turn, the echo
//! filter, the turn rule, the idle clock, Kokoro for Ghost's voice), while the room goes on
//! around them: steady noise, a talker across the room, four of them at once, one who
//! mentions Ghost, music with a singer (`scene`). Ghost's voice plays through headphones or
//! through speakers with a canceller's residual coming back (`media`). The model is
//! scripted: it answers the person's lines with the scene's replies and anything else with
//! "Sorry, I didn't catch that.", so every turn Ghost takes is counted, and nothing is
//! spent.
//!
//! Each scene is played in real time (the stages keep wall-clock time), several at once.
//! What is kept is numbers: how many turns Ghost took that the person never said and how
//! many of those a model would have answered or acted on, the person's lines lost or
//! misheard, how long the session took to end once the person stopped, whether a stop and
//! an interruption still cut Ghost off, and how often the screen showed the person talking
//! for sound that came to nothing. `--show-text` prints what was heard to the terminal;
//! nothing writes it to disk.

pub mod media;
pub mod scene;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::BoxStream;
use ghost_core::source::BoxFuture;
use ghost_core::{AutonomyPolicy, Store, WriteGate};
use ghost_llm::{Context, Event, LlmError, Message, Options, Tools};
use serde::Serialize;
use serde_json::json;

use self::media::{Path as EchoPath, Room, RoomIn, RoomOut};
use self::scene::{Condition, RATE, Rng, active_db, at_the_desk};
use super::audio::Voice;
use super::hear::score::normalize;
use crate::activity::Activity;
use crate::config::VoiceSettings;
use crate::llm::ModelPort;
use crate::pipeline::{Greeting, Models, SessionHooks, SessionParams, build_session};
use crate::tap::SessionPhase;
use crate::transport::{Media, scripted_media};

/// The person's voice: a Kokoro voice of its own, close to the microphone.
pub const PERSON: &str = "am_michael";
/// How long a scene may go on after Ghost answered the person's last line before it is
/// called "did not end": the idle timeout (45 s) and half a minute more.
pub const END_CAP_SECS: f64 = 75.0;

/// When the person says a line.
#[derive(Debug, Clone, Copy)]
pub enum When {
    /// This long after Ghost stops speaking.
    AfterReply(f64),
    /// This long after Ghost starts speaking.
    IntoReply(f64),
}

#[derive(Debug, Clone)]
pub struct Say {
    pub text: &'static str,
    pub when: When,
    /// Quieter or louder than the person's usual level.
    pub gain_db: f64,
    /// What the model answers it with; None, nothing (a stop).
    pub reply: Option<&'static str>,
    /// Said to cut Ghost off: whether it did is measured.
    pub cuts: bool,
}

const fn say(text: &'static str, when: When, reply: &'static str) -> Say {
    Say {
        text,
        when,
        gain_db: 0.0,
        reply: Some(reply),
        cuts: false,
    }
}

#[derive(Debug, Clone)]
pub struct Script {
    pub name: &'static str,
    /// Words said with the wake, as the first turn (no audio of the person in the session).
    pub first: Option<(&'static str, &'static str)>,
    pub says: Vec<Say>,
}

/// The sessions every condition is played with.
pub fn scripts() -> Vec<Script> {
    vec![
        Script {
            // Wake, a question, a follow-up right after the answer, a quieter one a while
            // later, then nothing: the session should end on its own.
            name: "follow",
            first: None,
            says: vec![
                say(
                    "What's waiting for triage in the app repo?",
                    When::AfterReply(0.8),
                    "Three issues: the session cache choice, dark mode, and rotating the webhook secret.",
                ),
                say(
                    "Which of those is the oldest?",
                    When::AfterReply(1.0),
                    "The session cache one, opened three weeks ago.",
                ),
                Say {
                    gain_db: -5.0,
                    ..say(
                        "Okay, label it ready for agent.",
                        When::AfterReply(9.0),
                        "Done, it is labelled ready for agent.",
                    )
                },
            ],
        },
        Script {
            // An interruption with words, a one-word stop, and a question after it.
            name: "barge",
            first: None,
            says: vec![
                say(
                    "Read me the newest issue in the infra repo.",
                    When::AfterReply(0.8),
                    "The newest is issue seventeen, rotate the webhook secret. It was opened yesterday by the platform team, it is marked as a security fix, and the deadline is the end of the week.",
                ),
                Say {
                    cuts: true,
                    ..say(
                        "Wait, make it the app repo instead.",
                        When::IntoReply(1.8),
                        "The newest in the app repo is issue one fifty one, add dark mode. It was opened this morning, nobody has picked it up yet, and it has no labels, so it is waiting for triage like the others.",
                    )
                },
                Say {
                    text: "Stop.",
                    when: When::IntoReply(1.8),
                    gain_db: 0.0,
                    reply: None,
                    cuts: true,
                },
                say(
                    "What about issue one forty two?",
                    When::AfterReply(1.2),
                    "Issue one forty two is the session cache choice, Redis or Postgres.",
                ),
            ],
        },
        Script {
            // Woken with the request in the same breath, answered, and nothing more: the
            // session has never heard the person's level when the room starts talking.
            name: "first",
            first: Some((
                "what needs my sign-off",
                "Two things need your sign-off: the release notes and the laptop budget.",
            )),
            says: Vec::new(),
        },
    ]
}

/// Words a model would act on: a turn of the room's that has one is counted as one that
/// would have reached a tool (a proxy: the scripted model calls none).
const ACTS: &[&str] = &[
    "send", "close", "label", "assign", "move", "check", "read", "deploy", "schedule", "open",
    "delete", "merge",
];

fn would_act(text: &str) -> bool {
    normalize(text).iter().any(|w| ACTS.contains(&w.as_str()))
}

/// How much of `line` is in `heard`: its words found, in order, over its length.
pub fn recall(line: &str, heard: &str) -> f64 {
    let (a, b) = (normalize(line), normalize(heard));
    if a.is_empty() {
        return 0.0;
    }
    crate::echo::matching(&a, &b) as f64 / a.len() as f64
}

/// Word errors of `heard` against `line`: (edits, words said).
pub fn word_errors(line: &str, heard: &str) -> (usize, usize) {
    let (a, b) = (normalize(line), normalize(heard));
    let edits = super::hear::score::align(&a, &b)
        .iter()
        .filter(|op| !matches!(op, super::hear::score::Op::Hit(_)))
        .count();
    (edits, a.len())
}

/// The scripted model of the room: the person's lines get their replies, anything else
/// "Sorry, I didn't catch that."
struct RoomModel {
    replies: Vec<(&'static str, &'static str)>,
}

/// A false turn this far or more under the person's segments is "well below" them: the
/// sound a level can tell from the person. Closer than this, only who is talking could.
pub const BELOW_DB: f64 = 10.0;

const SORRY: &str = "Sorry, I didn't catch that.";

impl ModelPort for RoomModel {
    fn stream(
        &self,
        context: Context,
        _tools: &Tools,
        _options: &Options,
    ) -> BoxStream<'static, Result<Event, LlmError>> {
        let last = context
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::User(t) if !t.starts_with("[system]") => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let reply = self
            .replies
            .iter()
            .map(|(line, reply)| (recall(line, &last), *reply))
            .filter(|(r, _)| *r >= 0.6)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(SORRY, |(_, r)| r);
        let words: Vec<String> = reply.split_inclusive(' ').map(String::from).collect();
        Box::pin(crate::llm::async_stream_words(
            words,
            Vec::new(),
            Duration::from_millis(600),
            Some(150.0),
            || {},
        ))
    }

    fn complete(
        &self,
        _system: String,
        _user: String,
        _max_tokens: u32,
    ) -> BoxFuture<'_, Result<String, LlmError>> {
        Box::pin(async { Ok("A summary.".to_string()) })
    }

    fn model_name(&self) -> String {
        "room-scripted".into()
    }
}

/// What happened, when (seconds from the session's start).
#[derive(Debug, Clone)]
enum Ev {
    Heard(String),
    Activity(Activity),
    Phase(SessionPhase),
    Said {
        line: usize,
        start: f64,
        end: f64,
    },
    /// A segment's read: its level (dBFS over the segment) and how many words.
    Read {
        dbfs: f64,
        words: usize,
    },
}

#[derive(Clone)]
struct Log {
    zero: Instant,
    events: Arc<Mutex<Vec<(f64, Ev)>>>,
}

impl Log {
    fn push(&self, ev: Ev) {
        let t = self.zero.elapsed().as_secs_f64();
        self.events.lock().unwrap().push((t, ev));
    }

    fn now(&self) -> f64 {
        self.zero.elapsed().as_secs_f64()
    }

    fn snapshot(&self) -> Vec<(f64, Ev)> {
        self.events.lock().unwrap().clone()
    }

    /// The first phase `want` at or after `from`.
    async fn phase_after(&self, want: SessionPhase, from: f64, within: f64) -> Option<f64> {
        let deadline = self.now() + within;
        loop {
            if let Some(t) = self.events.lock().unwrap().iter().find_map(|(t, e)| {
                matches!(e, Ev::Phase(p) if *p == want && *t >= from).then_some(*t)
            }) {
                return Some(t);
            }
            if self.now() > deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// One scene: a script in a room.
#[derive(Debug, Clone)]
pub struct Scene {
    pub condition: Condition,
    /// The person's level over the room's, dB; None in a quiet room.
    pub snr: Option<f64>,
    pub path: EchoPath,
    pub script: Script,
    pub seed: u64,
}

impl Scene {
    pub fn label(&self) -> String {
        format!(
            "{} {} {} {}",
            self.condition.name(),
            self.snr.map_or("-".into(), |s| format!("{s:.0}dB")),
            self.path.name(),
            self.script.name
        )
    }
}

/// A scene's numbers. No words.
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct SceneResult {
    pub condition: String,
    pub snr: Option<f64>,
    pub path: String,
    pub script: String,
    /// Turns Ghost closed with words the person never said (each one a model call).
    pub false_turns: usize,
    /// ... of which with words a model would act on.
    pub false_acts: usize,
    /// ... of which from sound well below the person's level (`BELOW_DB` or more under the
    /// person's segments), and from sound about as loud as the person.
    pub false_below: usize,
    pub false_similar: usize,
    /// Turns opened (the screen showed the person talking) that closed with no words.
    pub empty_opens: usize,
    /// Ghost cut off with no line of the person's under way.
    pub false_cuts: usize,
    /// The person's lines (stops aside) with no turn that heard most of them.
    pub lost: usize,
    pub lines: usize,
    /// Word errors over the person's turns, and the words they said.
    pub word_edits: usize,
    pub words: usize,
    /// Lines meant to cut Ghost off, and how many did within 1.5 s of their end.
    pub cuts_wanted: usize,
    pub cuts_made: usize,
    /// Seconds from the end of Ghost's answer to the person's last line to the session's
    /// end; None when it had not ended `END_CAP_SECS` later.
    pub end_secs: Option<f64>,
    pub wall: f64,
    pub problems: Vec<String>,
}

/// Models, voices and beds, loaded once.
pub struct RoomBench {
    pub models: Arc<Models>,
    pub voice: Voice,
    pub settings: VoiceSettings,
    pub beds: HashMap<Condition, Arc<Vec<f32>>>,
    /// The person's active level as rendered at the desk, dBFS.
    pub person_db: f64,
    /// The person's lines as a segment's read measures them (`stt::level_dbfs`), dBFS.
    pub person_read_db: f64,
    pub show_text: bool,
    pub keep_audio: Option<PathBuf>,
}

/// The beds are long enough that no scene hears the same stretch twice.
const BED_SECS: f64 = 240.0;

impl RoomBench {
    pub fn new(
        settings: VoiceSettings,
        cache: &Path,
        conditions: &[Condition],
        show_text: bool,
        keep_audio: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        // Synthetic voices only: `--set debug_audio=1` logs the stages' decisions to the
        // terminal, for a scene that went wrong.
        crate::debug::enable(settings.debug_audio);
        let mut whole = settings.clone();
        whole.tts_stream = crate::config::TtsStream::Whole;
        let models = Arc::new(Models::load(&whole)?);
        let voice = Voice::new(models.tts.clone(), &cache.join("utterances"));
        let mut beds = HashMap::new();
        for c in conditions {
            let path = cache.join("room").join(format!("{}-v1.wav", c.name()));
            let bed = match super::audio::read_wav(&path) {
                Ok(pcm) if *c != Condition::Quiet => {
                    pcm.iter().map(|s| f32::from(*s) / 32768.0 / 0.25).collect()
                }
                _ => {
                    let bed = scene::bed(*c, &voice, BED_SECS, 0x6057)?;
                    if *c != Condition::Quiet {
                        let _ = std::fs::create_dir_all(path.parent().unwrap());
                        // Stored a quarter down so the peaks fit in 16 bits.
                        let pcm: Vec<i16> = bed
                            .iter()
                            .map(|x| ((x * 0.25).clamp(-1.0, 1.0) * 32767.0) as i16)
                            .collect();
                        super::audio::write_wav(&path, &pcm)?;
                    }
                    bed
                }
            };
            beds.insert(*c, Arc::new(bed));
        }
        let person = voice.with_voice(PERSON);
        let mut all: Vec<f32> = Vec::new();
        for s in scripts() {
            for line in s.says {
                let u = person.render(line.text)?;
                all.extend(at_the_desk(
                    &ghost_infer::audio::to_f32(&u.pcm),
                    &mut Rng::new(1),
                ));
            }
        }
        let person_db = active_db(&all);
        // A segment's level as the speech-in stage reports it: the line with the quiet the
        // VAD keeps around it.
        let mut reads = Vec::new();
        for s in scripts() {
            for line in s.says.iter().filter(|l| l.gain_db == 0.0) {
                let u = person.render(line.text)?;
                let mut pcm = vec![0i16; RATE / 5];
                pcm.extend(
                    at_the_desk(&ghost_infer::audio::to_f32(&u.pcm), &mut Rng::new(1))
                        .iter()
                        .map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16),
                );
                pcm.extend(vec![0i16; RATE / 5]);
                reads.push(crate::stt::level_dbfs(&pcm));
            }
        }
        let person_read_db = reads.iter().sum::<f64>() / reads.len().max(1) as f64;
        Ok(Self {
            models,
            voice: person,
            settings,
            beds,
            person_db,
            person_read_db,
            show_text,
            keep_audio,
        })
    }

    fn line_audio(&self, say: &Say, seed: u64) -> anyhow::Result<Vec<i16>> {
        let u = self.voice.render(say.text)?;
        let mut pcm = at_the_desk(&ghost_infer::audio::to_f32(&u.pcm), &mut Rng::new(seed));
        scene::scale(&mut pcm, say.gain_db);
        Ok(pcm
            .iter()
            .map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect())
    }

    pub async fn run(&self, scene: &Scene) -> SceneResult {
        let began = Instant::now();
        let mut result = SceneResult {
            condition: scene.condition.name().into(),
            snr: scene.snr,
            path: scene.path.name().into(),
            script: scene.script.name.into(),
            ..SceneResult::default()
        };
        let lines: Vec<Vec<i16>> = match scene
            .script
            .says
            .iter()
            .enumerate()
            .map(|(i, s)| self.line_audio(s, scene.seed + i as u64))
            .collect()
        {
            Ok(l) => l,
            Err(e) => {
                result.problems.push(format!("render: {e}"));
                return result;
            }
        };
        let bed = self.beds[&scene.condition].clone();
        // The bed is at -20 dB active; the person is `person_db`.
        let gain = match scene.snr {
            Some(snr) => 10f64.powf((self.person_db - snr + 20.0) / 20.0) as f32,
            None => 0.0,
        };
        let start_at = Rng::new(scene.seed).below(bed.len().max(1));
        // With the room gate on, the session knows the person's level from the segment that
        // woke it, as a laptop's transcript wake measures it: the wake phrase (and the
        // request said with it) over this scene's room, with the second before it.
        let mut settings = self.settings.clone();
        if settings.room_gate == Some(true) {
            let words = match scene.script.first {
                Some((first, _)) => format!("Hey Ghost, {first}?"),
                None => "Hey Ghost.".to_string(),
            };
            match self.voice.render(&words) {
                Ok(u) => {
                    let mut wake = vec![0.0f32; RATE];
                    wake.extend(at_the_desk(
                        &ghost_infer::audio::to_f32(&u.pcm),
                        &mut Rng::new(scene.seed),
                    ));
                    let back = start_at + bed.len() - wake.len() % bed.len().max(1);
                    let pcm: Vec<i16> = wake
                        .iter()
                        .enumerate()
                        .map(|(i, x)| {
                            let b = if bed.is_empty() {
                                0.0
                            } else {
                                bed[(back + i) % bed.len()] * gain
                            };
                            ((x + b).clamp(-1.0, 1.0) * 32767.0) as i16
                        })
                        .collect();
                    settings.room_person_db = crate::stt::gate_level(&pcm);
                }
                Err(e) => result.problems.push(format!("render the wake: {e}")),
            }
        }
        let room = Room::new(bed, start_at, gain, scene.path, self.keep_audio.is_some());
        let (scripted, mouth, _ear) = scripted_media();
        let media = Media {
            input: Box::new(RoomIn {
                inner: scripted.input,
                room: room.clone(),
            }),
            output: Box::new(RoomOut {
                inner: scripted.output,
                room: room.clone(),
            }),
            origin: "desk",
        };
        let log = Log {
            zero: Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let gate = Arc::new(
            WriteGate::new(Arc::new(Store::in_memory().expect("an in-memory store")))
                .with_autonomy(AutonomyPolicy::default()),
        );
        let mut replies: Vec<(&'static str, &'static str)> = scene
            .script
            .says
            .iter()
            .filter_map(|s| s.reply.map(|r| (s.text, r)))
            .collect();
        if let Some(first) = scene.script.first {
            replies.push(first);
        }
        let show = self.show_text;
        let hooks = SessionHooks {
            on_heard: Some({
                let log = log.clone();
                Arc::new(move |rec| {
                    if let crate::tap::TurnRecord::Closed(text) = rec {
                        if show {
                            println!("    {:.1}s heard: {text}", log.now());
                        }
                        log.push(Ev::Heard(text)); // in memory only
                    }
                    Box::pin(async {})
                })
            }),
            on_activity: Some({
                let log = log.clone();
                Arc::new(move |a| {
                    if show {
                        println!("    {:.1}s {a:?}", log.now());
                    }
                    log.push(Ev::Activity(a))
                })
            }),
            on_phase: Some({
                let log = log.clone();
                Arc::new(move |p| {
                    log.push(Ev::Phase(p));
                    Box::pin(async {})
                })
            }),
            on_read: Some({
                let log = log.clone();
                Arc::new(move |dbfs, words| log.push(Ev::Read { dbfs, words }))
            }),
            ..Default::default()
        };
        let session = build_session(SessionParams {
            settings: Arc::new(settings),
            models: self.models.clone(),
            model: Arc::new(RoomModel { replies }),
            fast_model: None,
            sources: Vec::new(),
            gate,
            events: None,
            media,
            greeting: Greeting::Varied,
            first_utterance: scene.script.first.map(|(t, _)| t.to_string()),
            first_typed: false,
            hooks,
            wake_phrases: None,
            wake_fuzzy: 0.0,
            conversations: None,
            start_in: "ghost".into(),
            latency_log: None,
            reach: None,
            context_note: None,
            realtime: None,
        });
        session.start(true).await;
        let session = Arc::new(session);
        let run_task = {
            let s = session.clone();
            tokio::spawn(async move { s.run().await })
        };
        // The greeting, or the answer to the words said with the wake.
        let opened = log
            .phase_after(SessionPhase::Speaking, 0.0, 20.0)
            .await
            .unwrap_or(0.0);
        let mut last_end = log
            .phase_after(SessionPhase::Listening, opened, 30.0)
            .await
            .unwrap_or_else(|| log.now());
        let mut after = last_end;
        for (i, (s, pcm)) in scene.script.says.iter().zip(&lines).enumerate() {
            if run_task.is_finished() {
                result.problems.push(format!("ended before line {}", i + 1));
                break;
            }
            match s.when {
                When::AfterReply(d) => {
                    let t = log
                        .phase_after(SessionPhase::Listening, after, 25.0)
                        .await
                        .unwrap_or_else(|| log.now());
                    let wait = (t + d - log.now()).max(0.0);
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
                When::IntoReply(d) => {
                    let t = log
                        .phase_after(SessionPhase::Speaking, after, 25.0)
                        .await
                        .unwrap_or_else(|| log.now());
                    let wait = (t + d - log.now()).max(0.0);
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
            }
            let start = log.now();
            if self.show_text {
                println!("    {start:.1}s says: {}", s.text);
            }
            let done = mouth.say(pcm);
            let _ = tokio::time::timeout(Duration::from_secs(20), done).await;
            let end = log.now();
            log.push(Ev::Said {
                line: i,
                start,
                end,
            });
            after = end;
        }
        // Ghost's answer to the last line: the first time it goes quiet after speaking.
        if !scene.script.says.is_empty() {
            if let Some(spoke) = log.phase_after(SessionPhase::Speaking, after, 20.0).await {
                last_end = log
                    .phase_after(SessionPhase::Listening, spoke, 40.0)
                    .await
                    .unwrap_or_else(|| log.now());
            } else {
                last_end = after;
            }
        }
        let deadline = last_end + END_CAP_SECS;
        while !run_task.is_finished() && log.now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let ended_at = run_task.is_finished().then(|| log.now());
        if !run_task.is_finished() {
            session.end().await;
            let _ = tokio::time::timeout(Duration::from_secs(10), async {
                while !run_task.is_finished() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            if !run_task.is_finished() {
                session.cancel().await;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
        result.end_secs = ended_at.map(|t| ((t - last_end) * 10.0).round() / 10.0);
        if let (Some(dir), Some(tape)) = (&self.keep_audio, room.tape()) {
            let name = scene.label().replace(' ', "_");
            let _ = super::audio::write_wav(&dir.join(format!("{name}.wav")), &tape);
        }
        score(
            &mut result,
            &scene.script,
            &log.snapshot(),
            self.person_read_db,
        );
        result.wall = (began.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        result
    }
}

/// The numbers of one scene, from its events.
fn score(r: &mut SceneResult, script: &Script, events: &[(f64, Ev)], person_db: f64) {
    let said: Vec<(usize, f64, f64)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Ev::Said { line, start, end } => Some((*line, *start, *end)),
            _ => None,
        })
        .collect();
    let mut matched = vec![false; script.says.len()];
    let mut heard_at: Vec<f64> = Vec::new();
    let mut last_heard = f64::MIN;
    for (t, e) in events {
        let Ev::Heard(text) = e else { continue };
        heard_at.push(*t);
        // The loudest segment with words read for this turn, against the person's.
        let loudest = events
            .iter()
            .filter(|(at, _)| *at > last_heard && *at <= *t)
            .filter_map(|(_, e)| match e {
                Ev::Read { dbfs, words } if *words > 0 => Some(*dbfs),
                _ => None,
            })
            .fold(f64::MIN, f64::max);
        last_heard = *t;
        // A turn is the person's when most of a line said in the last 20 s is in it.
        let best = said
            .iter()
            .filter(|(_, start, end)| *start <= *t && *t - *end <= 20.0)
            .map(|(i, _, _)| (*i, recall(script.says[*i].text, text)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, rec)) if rec >= 0.6 => {
                if !matched[i] {
                    matched[i] = true;
                    let (e, n) = word_errors(script.says[i].text, text);
                    r.word_edits += e;
                    r.words += n;
                }
            }
            _ => {
                r.false_turns += 1;
                if would_act(text) {
                    r.false_acts += 1;
                }
                // No read of its own (heard over Ghost's voice, which is not reported):
                // counted as about as loud, the case a level cannot settle.
                if loudest > f64::MIN && loudest <= person_db - BELOW_DB {
                    r.false_below += 1;
                } else {
                    r.false_similar += 1;
                }
            }
        }
    }
    for (i, s) in script.says.iter().enumerate() {
        if s.reply.is_none() {
            continue; // a stop is measured by the cut
        }
        r.lines += 1;
        if !matched[i] {
            r.lost += 1;
            r.words += normalize(s.text).len();
            r.word_edits += normalize(s.text).len();
        }
    }
    // Opened for nothing: a turn's start with no words before the next start. A start is
    // pushed both ways and the observer sees each: one within 0.2 s of another is the same.
    let mut starts: Vec<f64> = Vec::new();
    for (t, e) in events {
        if matches!(e, Ev::Activity(Activity::YouStarted))
            && starts.last().is_none_or(|l| *t - *l > 0.2)
        {
            starts.push(*t);
        }
    }
    for (k, s) in starts.iter().enumerate() {
        let next = starts.get(k + 1).copied().unwrap_or(f64::MAX);
        if !heard_at.iter().any(|h| *h >= *s && *h < next) {
            r.empty_opens += 1;
        }
    }
    let cuts: Vec<f64> = events
        .iter()
        .filter_map(|(t, e)| matches!(e, Ev::Activity(Activity::Cut)).then_some(*t))
        .collect();
    for c in &cuts {
        if !said
            .iter()
            .any(|(_, start, end)| *c >= *start && *c <= *end + 1.5)
        {
            r.false_cuts += 1;
        }
    }
    for (i, start, end) in &said {
        if script.says[*i].cuts {
            r.cuts_wanted += 1;
            if cuts.iter().any(|c| *c >= *start && *c <= *end + 1.5) {
                r.cuts_made += 1;
            }
        }
    }
}

// --- the run ---------------------------------------------------------------------------

pub struct RoomArgs {
    pub conditions: Vec<Condition>,
    pub snrs: Vec<f64>,
    pub paths: Vec<EchoPath>,
    pub scripts: Vec<String>,
    pub jobs: usize,
    pub overrides: BTreeMap<String, String>,
    pub label: String,
    pub out: PathBuf,
    pub cache: PathBuf,
    pub show_text: bool,
    pub keep_audio: bool,
    pub seed: u64,
    /// Skip the scenes `results.jsonl` in `out` already has.
    pub resume: bool,
}

const USAGE: &str = "usage: ghost-perf room [--conditions quiet,noise,talker,babble,name,music] \
[--snr 20,12,6,3] [--paths headphones,speakers] [--scripts follow,barge,first] [--jobs 6] \
[--set NAME=VALUE ...] [--label TEXT] [--seed N] [--out DIR] [--resume] [--show-text] [--keep-audio]";

fn parse(argv: &[String], scratch: &Path) -> Result<RoomArgs, String> {
    let mut a = RoomArgs {
        conditions: Condition::ALL.to_vec(),
        snrs: vec![20.0, 12.0, 6.0, 3.0],
        paths: vec![EchoPath::Headphones, EchoPath::Speakers],
        scripts: scripts().iter().map(|s| s.name.to_string()).collect(),
        jobs: 6,
        overrides: BTreeMap::new(),
        label: String::new(),
        out: PathBuf::new(),
        cache: scratch.to_path_buf(),
        show_text: false,
        keep_audio: false,
        seed: 1,
        resume: false,
    };
    let mut out = None;
    let mut it = argv.iter();
    let csv = |v: &str| -> Vec<String> {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--show-text" => a.show_text = true,
            "--keep-audio" => a.keep_audio = true,
            "--resume" => a.resume = true,
            "--conditions" => {
                a.conditions = csv(&value()?)
                    .iter()
                    .map(|c| Condition::named(c).ok_or(format!("unknown condition {c}")))
                    .collect::<Result<_, _>>()?
            }
            "--snr" => {
                a.snrs = csv(&value()?)
                    .iter()
                    .map(|s| s.parse().map_err(|_| format!("bad snr {s}")))
                    .collect::<Result<_, _>>()?
            }
            "--paths" => {
                a.paths = csv(&value()?)
                    .iter()
                    .map(|p| EchoPath::named(p).ok_or(format!("unknown path {p}")))
                    .collect::<Result<_, _>>()?
            }
            "--scripts" => a.scripts = csv(&value()?),
            "--jobs" => a.jobs = value()?.parse().map_err(|_| "bad --jobs")?,
            "--seed" => a.seed = value()?.parse().map_err(|_| "bad --seed")?,
            "--label" => a.label = value()?,
            "--out" => out = Some(PathBuf::from(value()?)),
            "--set" => {
                let v = value()?;
                let (k, val) = v.split_once('=').ok_or(format!("--set {v}: NAME=VALUE"))?;
                a.overrides
                    .insert(k.trim().to_string(), val.trim().to_string());
            }
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}; {USAGE}")),
        }
    }
    a.out = out.unwrap_or_else(|| {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        scratch.join(format!("{stamp}-room"))
    });
    Ok(a)
}

/// The session's settings: Ghost's defaults and the run's `--set`s, and nothing from this
/// computer's environment or the operator's settings.
pub fn settings(overrides: &BTreeMap<String, String>) -> Result<VoiceSettings, String> {
    let vars: HashMap<String, String> = overrides
        .iter()
        .map(|(k, v)| (format!("GHOST_{}", k.to_uppercase()), v.clone()))
        .collect();
    VoiceSettings::from_vars(|name| vars.get(name).cloned())
        .map_err(|e| e.to_string())
        .map(|mut s| {
            s.input_device = None;
            s.output_device = None;
            s
        })
}

pub fn scenes(a: &RoomArgs) -> Vec<Scene> {
    let all = scripts();
    let mut out = Vec::new();
    let mut seed = a.seed;
    for c in &a.conditions {
        let snrs: Vec<Option<f64>> = if *c == Condition::Quiet {
            vec![None]
        } else {
            a.snrs.iter().map(|s| Some(*s)).collect()
        };
        for snr in snrs {
            for path in &a.paths {
                for s in all.iter().filter(|s| a.scripts.iter().any(|n| n == s.name)) {
                    seed += 1;
                    out.push(Scene {
                        condition: *c,
                        snr,
                        path: *path,
                        script: s.clone(),
                        seed: seed * 7_919,
                    });
                }
            }
        }
    }
    out
}

pub fn cli(argv: &[String], scratch: &Path) -> i32 {
    let a = match parse(argv, scratch) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let settings = match settings(&a.overrides) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&a.out) {
        eprintln!("{}: {e}", a.out.display());
        return 2;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    match rt.block_on(run(&a, settings)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("room: {e}");
            1
        }
    }
}

async fn run(a: &RoomArgs, settings: VoiceSettings) -> anyhow::Result<()> {
    let t = Instant::now();
    let keep = a.keep_audio.then(|| a.out.join("audio"));
    if let Some(k) = &keep {
        std::fs::create_dir_all(k)?;
    }
    let bench = Arc::new(RoomBench::new(
        settings,
        &a.cache,
        &a.conditions,
        a.show_text,
        keep,
    )?);
    println!(
        "room: models and beds in {:.1}s; the person at {:.1} dBFS",
        t.elapsed().as_secs_f64(),
        bench.person_db
    );
    // Each scene's numbers are kept as it finishes, so a long run is never lost.
    let jsonl = a.out.join("results.jsonl");
    let done: Vec<SceneResult> = if a.resume {
        std::fs::read_to_string(&jsonl)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    } else {
        let _ = std::fs::remove_file(&jsonl);
        Vec::new()
    };
    let key = |c: &str, snr: Option<f64>, p: &str, s: &str| format!("{c}|{snr:?}|{p}|{s}");
    let have: std::collections::HashSet<String> = done
        .iter()
        .map(|r| key(&r.condition, r.snr, &r.path, &r.script))
        .collect();
    let scenes: Vec<Scene> = scenes(a)
        .into_iter()
        .filter(|s| {
            !have.contains(&key(
                s.condition.name(),
                s.snr,
                s.path.name(),
                s.script.name,
            ))
        })
        .collect();
    let writer = Arc::new(Mutex::new(()));
    let n = scenes.len();
    let jobs = Arc::new(tokio::sync::Semaphore::new(a.jobs.max(1)));
    let mut tasks = Vec::new();
    for (i, scene) in scenes.into_iter().enumerate() {
        let bench = bench.clone();
        let jobs = jobs.clone();
        let (writer, jsonl) = (writer.clone(), jsonl.clone());
        tasks.push(tokio::spawn(async move {
            let _permit = jobs.acquire_owned().await;
            let r = bench.run(&scene).await;
            {
                let _w = writer.lock().unwrap();
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&jsonl)
                    && let Ok(line) = serde_json::to_string(&r)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
            println!(
                "  [{}/{n}] {}: false {} (act {}), empty {}, cut {}/{} false-cut {}, lost {}/{}, end {}",
                i + 1,
                scene.label(),
                r.false_turns,
                r.false_acts,
                r.empty_opens,
                r.cuts_made,
                r.cuts_wanted,
                r.false_cuts,
                r.lost,
                r.lines,
                r.end_secs.map_or("never".into(), |s| format!("{s:.0}s")),
            );
            r
        }));
    }
    let mut results = done;
    for t in tasks {
        results.push(t.await?);
    }
    let data = json!({
        "label": a.label,
        "overrides": a.overrides,
        "person_db": bench.person_db,
        "person_read_db": bench.person_read_db,
        "results": results,
    });
    std::fs::write(
        a.out.join("results.json"),
        serde_json::to_string_pretty(&data)?,
    )?;
    let report = render(&a.label, &results);
    std::fs::write(a.out.join("report.md"), &report)?;
    print!("{report}");
    println!(
        "room: {} in {:.0}s",
        a.out.display(),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The table: one row per room, both paths and every script summed.
pub fn render(label: &str, results: &[SceneResult]) -> String {
    #[derive(Default)]
    struct Row {
        false_turns: usize,
        false_acts: usize,
        below: usize,
        similar: usize,
        empty: usize,
        false_cuts: usize,
        lost: usize,
        lines: usize,
        edits: usize,
        words: usize,
        cuts_made: usize,
        cuts_wanted: usize,
        ends: Vec<Option<f64>>,
    }
    let mut rows: BTreeMap<(usize, i64, String), Row> = BTreeMap::new();
    for r in results {
        let order = Condition::named(&r.condition).map_or(99, |c| c as usize);
        let key = (order, -(r.snr.unwrap_or(99.0) as i64), r.path.clone());
        let row = rows.entry(key).or_default();
        row.false_turns += r.false_turns;
        row.false_acts += r.false_acts;
        row.below += r.false_below;
        row.similar += r.false_similar;
        row.empty += r.empty_opens;
        row.false_cuts += r.false_cuts;
        row.lost += r.lost;
        row.lines += r.lines;
        row.edits += r.word_edits;
        row.words += r.words;
        row.cuts_made += r.cuts_made;
        row.cuts_wanted += r.cuts_wanted;
        if r.script != "barge" {
            row.ends.push(r.end_secs);
        }
    }
    let mut s = String::new();
    if !label.is_empty() {
        s.push_str(&format!("### {label}\n\n"));
    }
    s.push_str("| room | SNR | path | false turns | of them act | well below / as loud | opened for nothing | false cuts | lines lost | WER | stops and interruptions cut | session ended after |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    let mut total = Row::default();
    for ((order, snr, path), row) in &rows {
        let name = Condition::ALL.get(*order).map_or("?", |c| c.name());
        let ends: Vec<String> = row
            .ends
            .iter()
            .map(|e| e.map_or("never".into(), |s| format!("{s:.0}s")))
            .collect();
        s.push_str(&format!(
            "| {name} | {} | {path} | {} | {} | {}/{} | {} | {} | {}/{} | {:.1}% | {}/{} | {} |\n",
            if *snr == -99 {
                "-".into()
            } else {
                format!("{} dB", -snr)
            },
            row.false_turns,
            row.false_acts,
            row.below,
            row.similar,
            row.empty,
            row.false_cuts,
            row.lost,
            row.lines,
            100.0 * row.edits as f64 / row.words.max(1) as f64,
            row.cuts_made,
            row.cuts_wanted,
            ends.join(", "),
        ));
        total.false_turns += row.false_turns;
        total.false_acts += row.false_acts;
        total.below += row.below;
        total.similar += row.similar;
        total.empty += row.empty;
        total.false_cuts += row.false_cuts;
        total.lost += row.lost;
        total.lines += row.lines;
        total.edits += row.edits;
        total.words += row.words;
        total.cuts_made += row.cuts_made;
        total.cuts_wanted += row.cuts_wanted;
        total.ends.extend(row.ends.iter().copied());
    }
    let never = total.ends.iter().filter(|e| e.is_none()).count();
    s.push_str(&format!(
        "| **all** | | | **{}** | **{}** | **{}/{}** | **{}** | **{}** | **{}/{}** | **{:.1}%** | **{}/{}** | **{} of {} never ended** |\n",
        total.false_turns,
        total.false_acts,
        total.below,
        total.similar,
        total.empty,
        total.false_cuts,
        total.lost,
        total.lines,
        100.0 * total.edits as f64 / total.words.max(1) as f64,
        total.cuts_made,
        total.cuts_wanted,
        never,
        total.ends.len(),
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(list: Vec<(f64, Ev)>) -> Vec<(f64, Ev)> {
        list
    }

    #[test]
    fn a_turn_is_the_persons_when_it_holds_most_of_a_line_just_said() {
        let script = &scripts()[0];
        let ev = events(vec![
            (
                1.0,
                Ev::Said {
                    line: 0,
                    start: 1.0,
                    end: 3.0,
                },
            ),
            (
                3.5,
                Ev::Heard("What's waiting for triage in the app repo?".into()),
            ),
            (
                8.0,
                Ev::Read {
                    dbfs: -35.0,
                    words: 6,
                },
            ),
            (9.0, Ev::Heard("did you see the game last night".into())),
            (
                11.0,
                Ev::Read {
                    dbfs: -22.0,
                    words: 6,
                },
            ),
            (12.0, Ev::Heard("can you send me that spreadsheet".into())),
        ]);
        let mut r = SceneResult::default();
        score(&mut r, script, &ev, -20.0);
        assert_eq!(r.false_turns, 2);
        assert_eq!(r.false_acts, 1, "send");
        assert_eq!((r.false_below, r.false_similar), (1, 1));
        assert_eq!(r.lines, 3);
        assert_eq!(r.lost, 2, "two lines never said are lost");
    }

    #[test]
    fn a_start_with_no_words_after_it_is_an_empty_open() {
        let script = &scripts()[2];
        let ev = events(vec![
            (1.0, Ev::Activity(Activity::YouStarted)),
            (1.0, Ev::Activity(Activity::YouStarted)), // pushed up and down
            (2.0, Ev::Activity(Activity::YouStopped)),
            (3.0, Ev::Activity(Activity::YouStarted)),
            (4.0, Ev::Heard("hello there".into())),
        ]);
        let mut r = SceneResult::default();
        score(&mut r, script, &ev, -20.0);
        assert_eq!(r.empty_opens, 1);
        assert_eq!(r.false_turns, 1);
    }

    #[test]
    fn a_cut_counts_for_the_line_said_just_before_it() {
        let script = &scripts()[1];
        let ev = events(vec![
            (
                0.0,
                Ev::Said {
                    line: 2,
                    start: 5.0,
                    end: 5.5,
                },
            ),
            (6.2, Ev::Activity(Activity::Cut)),
            (20.0, Ev::Activity(Activity::Cut)),
        ]);
        let mut r = SceneResult::default();
        score(&mut r, script, &ev, -20.0);
        assert_eq!((r.cuts_made, r.cuts_wanted, r.false_cuts), (1, 1, 1));
    }

    #[test]
    fn the_settings_are_ghosts_defaults_and_the_runs_own() {
        let mut o = BTreeMap::new();
        o.insert("idle_timeout_secs".to_string(), "30".to_string());
        let s = settings(&o).unwrap();
        assert_eq!(s.idle_timeout_secs, 30.0);
        assert_eq!(s.vad_min_volume, 0.6);
    }
}
