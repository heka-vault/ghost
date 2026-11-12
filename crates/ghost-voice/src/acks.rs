//! A one- or two-word acknowledgment as Ghost goes to an app (#43).
//!
//! The operator asks for something; when the answer needs an app (`apps.json`: GitHub, T3,
//! Mobbin, the scripts app), the model's first tool call to it is the moment Ghost goes off
//! to work, and Ghost says "Sure." or "On it." right then, before the tools run, so the
//! operator knows it heard. An answer Ghost already has, or one from its own quick in-tree
//! tools (the agents, memory, rules, outputs, skills, the waiting list), just comes.
//!
//! The words are Ghost's, from two fixed lists, never the model's: a read gets one of
//! [`READ`], a change one of [`CHANGE`], picked at random and never the same twice in a
//! row. They are rendered once per voice into the cache (`~/.cache/ghost/acks-v1/`) and
//! played as they are, so nothing is synthesized on the turn's path. The model's own
//! narration ("let me check the Quotient projects first") is still dropped by
//! `narration`; this is what is said in its place.
//!
//! The acknowledgment is not a turn: not in the transcript, the context, or the last reply
//! the gate reads. It plays without `BotStarted`/`BotStopped` (`Frame::Ack`), so the
//! timeline's reply, the desktop's phase and the phone's speaking events stay the
//! answer's; the shared speaking flag covers it for the barge-in rule, and the echo filter
//! is told its words, since a short clip after silence is where the canceller leaks most
//! (#42). One per turn: a reply that calls tools in several rounds is acknowledged once,
//! and not at all once any of it has been spoken.
//!
//! The moment is the first sign a reply needs a tool ([`Frame::ToolStarting`], as the
//! model begins writing the call), not the call made whole a second or more later, and it
//! is any tool that sends the model round again: Ghost's own lookups (recall, the docs,
//! the tool search) cost a model call as an app's do (2026-11-12: four turns of the live
//! run went five seconds with only the thinking sound, three of them on Ghost's own tools).
//! When the model wrote a few words before its first call ("Checking four thirteen.",
//! `crate::narration::preamble`), those are said instead of a fixed line, rendered then;
//! a goodbye's or a hold's call is not acknowledged, its own words follow.
//!
//! With a fast model in front (ADR 0050) the moment comes sooner: when the fast model
//! hands the turn to the main one ([`Frame::HandOver`]), the acknowledgment is said then,
//! and covers the main model's first token as well as the tools. Nothing has been done
//! for the turn at that point, so the operator may still be going on after a pause
//! (ADR 0049): the acknowledgment waits until the turn's grace has run from the end of
//! their speech, and is not said if they went on first.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rand::Rng;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::debug;
use crate::frames::{Audio, Direction, Frame, Out, Stage};
use crate::tts::{OnSay, SharedSynth};
use crate::turns::TurnControl;

/// Ghost going to read something.
pub const READ: &[&str] = &["Sure.", "Let's see.", "One sec.", "Checking."];
/// Ghost going to change something.
pub const CHANGE: &[&str] = &["On it.", "Will do.", "Okay.", "Sure thing."];
/// Kokoro's rate, and the playback's.
pub const RATE: u32 = 24_000;
/// The cache folder; a new list or a new way of rendering is a new version.
const VERSION: &str = "acks-v2";
/// Kept around the words when a clip is trimmed: enough that it starts and ends softly.
const LEAD_SECS: f64 = 0.02;
const TAIL_SECS: f64 = 0.04;

/// Only the spoken part: Kokoro pads a short phrase with about 0.2 s of silence after it,
/// which the answer would wait behind when it is ready before the acknowledgment ends.
/// Silence is under 2% of the clip's peak (-34 dB).
pub fn trim(pcm: &[i16]) -> Vec<i16> {
    let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let floor = peak / 50;
    let Some(first) = pcm.iter().position(|s| s.unsigned_abs() > floor) else {
        return Vec::new();
    };
    let last = pcm
        .iter()
        .rposition(|s| s.unsigned_abs() > floor)
        .unwrap_or(first);
    let lead = (LEAD_SECS * f64::from(RATE)) as usize;
    let tail = (TAIL_SECS * f64::from(RATE)) as usize;
    pcm[first.saturating_sub(lead)..(last + 1 + tail).min(pcm.len())].to_vec()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AckKind {
    Read,
    Change,
}

impl AckKind {
    pub fn lines(self) -> &'static [&'static str] {
        match self {
            AckKind::Read => READ,
            AckKind::Change => CHANGE,
        }
    }
}

/// Every word an acknowledgment can say, lowercased: what the echo filter drops.
pub fn words() -> HashSet<String> {
    READ.iter()
        .chain(CHANGE)
        .flat_map(|l| crate::echo::words(l))
        .collect()
}

/// One clip: the words and their audio at [`RATE`].
#[derive(Clone)]
pub struct Clip {
    pub text: &'static str,
    pub pcm: Arc<Vec<i16>>,
}

/// The lists rendered in one voice at one speed.
#[derive(Clone, Default)]
pub struct Voiced {
    pub read: Vec<Clip>,
    pub change: Vec<Clip>,
}

impl Voiced {
    pub fn of(&self, kind: AckKind) -> &[Clip] {
        match kind {
            AckKind::Read => &self.read,
            AckKind::Change => &self.change,
        }
    }
}

fn key(voice: &str, speed: f64) -> String {
    let voice: String = voice
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{voice}@{speed:.2}")
}

fn slug(text: &str) -> String {
    crate::echo::words(text).join("-")
}

/// The rendered clips, by voice, shared by every session: rendered once from the loaded
/// Kokoro (on a thread of its own, a phrase at a time so a reply's speech can go between)
/// and kept on disk, so a voice is rendered once per machine.
pub struct Acks {
    dir: Option<PathBuf>,
    ready: Mutex<HashMap<String, Arc<Voiced>>>,
    warming: Mutex<HashSet<String>>,
}

impl Acks {
    /// `cache_dir` None: memory only (tests).
    pub fn new(cache_dir: Option<&Path>) -> Arc<Self> {
        Arc::new(Self {
            dir: cache_dir.map(|d| d.join(VERSION)),
            ready: Mutex::new(HashMap::new()),
            warming: Mutex::new(HashSet::new()),
        })
    }

    /// The clips for this voice, if they are rendered.
    pub fn get(&self, voice: &str, speed: f64) -> Option<Arc<Voiced>> {
        self.ready.lock().unwrap().get(&key(voice, speed)).cloned()
    }

    pub fn insert(&self, voice: &str, speed: f64, voiced: Voiced) {
        self.ready
            .lock()
            .unwrap()
            .insert(key(voice, speed), Arc::new(voiced));
    }

    /// Render this voice in the background unless it is ready or on its way.
    pub fn warm(self: &Arc<Self>, synth: SharedSynth, voice: &str, speed: f64) {
        let k = key(voice, speed);
        if self.ready.lock().unwrap().contains_key(&k)
            || !self.warming.lock().unwrap().insert(k.clone())
        {
            return;
        }
        let me = self.clone();
        let owned = voice.to_string();
        let done = k.clone();
        let spawned = std::thread::Builder::new()
            .name("ghost-acks".into())
            .spawn(move || {
                let voiced = me.render(&synth, &owned, speed);
                if voiced.read.is_empty() && voiced.change.is_empty() {
                    warn!("acks: nothing rendered in voice {owned}; Ghost goes to apps silently");
                } else {
                    me.ready
                        .lock()
                        .unwrap()
                        .insert(done.clone(), Arc::new(voiced));
                }
                me.warming.lock().unwrap().remove(&done);
            });
        if let Err(e) = spawned {
            warn!("acks: {e}");
            self.warming.lock().unwrap().remove(&k);
        }
    }

    /// Both lists in this voice: each from the cache, or rendered and written there.
    /// Blocking. A phrase that fails is left out.
    pub fn render(&self, synth: &SharedSynth, voice: &str, speed: f64) -> Voiced {
        let one = |text: &'static str| -> Option<Clip> {
            let path = self.dir.as_ref().map(|d| {
                d.join(key(voice, speed))
                    .join(format!("{}.wav", slug(text)))
            });
            if let Some(p) = &path
                && p.exists()
            {
                match ghost_audio::sounds::load_wav(p, RATE) {
                    Ok(pcm) if !pcm.is_empty() => {
                        return Some(Clip {
                            text,
                            pcm: Arc::new(pcm),
                        });
                    }
                    Ok(_) => {}
                    Err(e) => warn!("acks: {e}; rendering it again"),
                }
            }
            let rendered = {
                let mut s = synth.lock().unwrap_or_else(|e| e.into_inner());
                s.synthesize(text, voice, speed as f32)
            };
            let pcm = match rendered.map(|pcm| trim(&pcm)) {
                Ok(pcm) if !pcm.is_empty() => pcm,
                Ok(_) => return None,
                Err(e) => {
                    warn!("acks: Kokoro could not say {text:?} in {voice}: {e}");
                    return None;
                }
            };
            if let Some(p) = &path
                && let Err(e) = ghost_audio::sounds::write_wav(p, RATE, &pcm)
            {
                warn!("acks: {e}");
            }
            Some(Clip {
                text,
                pcm: Arc::new(pcm),
            })
        };
        Voiced {
            read: READ.iter().filter_map(|t| one(t)).collect(),
            change: CHANGE.iter().filter_map(|t| one(t)).collect(),
        }
    }
}

/// The model's own words before its first call, rendered for the acknowledgment; None
/// when they cannot be (the fixed line is said instead).
async fn render(
    synth: SharedSynth,
    words: String,
    voice: String,
    speed: f64,
) -> Option<(String, Vec<i16>)> {
    tokio::task::spawn_blocking(move || {
        let rendered = {
            let mut s = synth.lock().unwrap_or_else(|e| e.into_inner());
            s.synthesize(&words, &voice, speed as f32)
        };
        match rendered.map(|pcm| trim(&pcm)) {
            Ok(pcm) if !pcm.is_empty() => Some((words, pcm)),
            Ok(_) => None,
            Err(e) => {
                warn!("ack: Kokoro could not say the model's words: {e}");
                None
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// Which clip, at random, never the one said last.
#[derive(Debug, Default)]
pub struct Picker {
    last: Option<&'static str>,
}

impl Picker {
    pub fn pick<'a>(&mut self, clips: &'a [Clip]) -> Option<&'a Clip> {
        let choices: Vec<&Clip> = clips
            .iter()
            .filter(|c| clips.len() < 2 || Some(c.text) != self.last)
            .collect();
        if choices.is_empty() {
            return None;
        }
        let clip = choices[rand::rng().random_range(0..choices.len())];
        self.last = Some(clip.text);
        Some(clip)
    }
}

/// Whether a tool the model called goes to an app, and as a read or a change.
pub type Outside = Arc<dyn Fn(&str) -> Option<AckKind> + Send + Sync>;

/// After the narration cut: says the acknowledgment at the turn's first call to an app,
/// when nothing of the reply has been spoken yet.
pub struct AckStage {
    acks: Arc<Acks>,
    synth: SharedSynth,
    outside: Outside,
    voice: String,
    speed: f64,
    turns: Arc<TurnControl>,
    reply_turn: Option<CancellationToken>,
    on_ack: Option<OnSay>,
    /// This turn has been acknowledged.
    acked: bool,
    /// Something of this turn's reply went to the voice.
    spoke: bool,
    picker: Picker,
    /// How long after the operator's speech ended a hand-over is acknowledged at the
    /// soonest (`with_settle`), and when that speech ended.
    settle: std::time::Duration,
    segment_ended: Option<std::time::Instant>,
}

impl AckStage {
    pub fn new(
        acks: Arc<Acks>,
        synth: SharedSynth,
        outside: Outside,
        voice: &str,
        speed: f64,
        turns: Arc<TurnControl>,
    ) -> Self {
        acks.warm(synth.clone(), voice, speed);
        Self {
            acks,
            synth,
            outside,
            voice: voice.to_string(),
            speed,
            turns,
            reply_turn: None,
            on_ack: None,
            acked: false,
            spoke: false,
            picker: Picker::default(),
            settle: std::time::Duration::ZERO,
            segment_ended: None,
        }
    }

    /// A hand-over is acknowledged no sooner than `secs` after the operator's speech
    /// ended (the turn's grace): before that they may only be pausing.
    pub fn with_settle(mut self, secs: f64) -> Self {
        self.settle = std::time::Duration::from_secs_f64(secs.max(0.0));
        self
    }

    /// Told the words as the acknowledgment goes out: the echo filter (`EchoFilter::acked`).
    pub fn with_on_ack(mut self, on_ack: OnSay) -> Self {
        self.on_ack = Some(on_ack);
        self
    }

    fn new_turn(&mut self) {
        self.acked = false;
        self.spoke = false;
    }

    /// What a call sends Ghost to: an app's read or change, Ghost's own `change`, or any
    /// other of Ghost's own tools, which is a read. None for a goodbye or a hold.
    fn kind_of(&self, names: &[String]) -> Option<AckKind> {
        let quick = names
            .iter()
            .all(|n| crate::narration::SPOKEN_WITH.contains(&n.as_str()));
        if names.is_empty() || quick {
            return None;
        }
        names
            .iter()
            .filter(|n| !crate::narration::SPOKEN_WITH.contains(&n.as_str()))
            .map(|n| {
                (self.outside)(n).unwrap_or(if n == "change" {
                    AckKind::Change
                } else {
                    AckKind::Read
                })
            })
            .max()
    }

    async fn acknowledge(&mut self, names: &[String], out: &Out) {
        let kind = self.kind_of(names);
        let turn = self
            .reply_turn
            .clone()
            .unwrap_or_else(|| self.turns.current());
        self.say(kind, None, names, turn, std::time::Duration::ZERO, out)
            .await;
    }

    /// No later than the turn's grace from the end of the operator's speech: before that
    /// they may only be pausing (ADR 0049).
    fn settle_wait(&self) -> std::time::Duration {
        self.segment_ended.map_or(std::time::Duration::ZERO, |at| {
            self.settle.saturating_sub(at.elapsed())
        })
    }

    /// A reply began a call: said now, with the model's own words when it wrote a few.
    async fn starting(&mut self, name: &str, said: Option<String>, out: &Out) {
        let names = vec![name.to_string()];
        let kind = self.kind_of(&names);
        let turn = self
            .reply_turn
            .clone()
            .unwrap_or_else(|| self.turns.current());
        let wait = self.settle_wait();
        self.say(kind, said, &names, turn, wait, out).await;
    }

    /// The fast model handed the turn over: the main model's first token is a second or
    /// more away whatever it then does, so Ghost says it heard, unless what was asked
    /// for is a goodbye or a hold, whose own words follow.
    async fn handed_over(&mut self, names: &[String], out: &Out) {
        let quick = !names.is_empty()
            && names
                .iter()
                .all(|n| crate::narration::SPOKEN_WITH.contains(&n.as_str()));
        if quick {
            return;
        }
        let kind = self.kind_of(names).unwrap_or(AckKind::Read);
        let wait = self.settle_wait();
        self.say(Some(kind), None, names, self.turns.current(), wait, out)
            .await;
    }

    /// Say `words` (the model's own, rendered now) or else one of `kind`'s lines for
    /// `turn`, after `wait`, unless the turn is cut first.
    async fn say(
        &mut self,
        kind: Option<AckKind>,
        words: Option<String>,
        names: &[String],
        turn: CancellationToken,
        wait: std::time::Duration,
        out: &Out,
    ) {
        if self.acked || self.spoke {
            return;
        }
        let Some(kind) = kind else {
            return;
        };
        self.acked = true; // once per turn, said or not
        if turn.is_cancelled() {
            return;
        }
        let clip = match self.acks.get(&self.voice, self.speed) {
            Some(voiced) => self.picker.pick(voiced.of(kind)).cloned(),
            None => {
                self.acks.warm(self.synth.clone(), &self.voice, self.speed);
                None
            }
        };
        if words.is_none() && clip.is_none() {
            debug::audio(|| format!("ack: none yet in {}", self.voice));
            return;
        }
        if words.is_none()
            && wait.is_zero()
            && let Some(clip) = &clip
        {
            // A fixed line, due now: in order, before anything after it.
            debug::audio(|| format!("ack: {:?} before {}", clip.text, names.join(", ")));
            if let Some(on_ack) = &self.on_ack {
                on_ack(clip.text);
            }
            out.push(Frame::Ack {
                text: clip.text.to_string(),
                audio: Audio::new((*clip.pcm).clone(), RATE),
                turn,
            })
            .await;
            return;
        }
        // The stage's inbox is not held while the words render or the grace runs.
        let (out, on_ack) = (out.clone(), self.on_ack.clone());
        let (synth, voice, speed) = (self.synth.clone(), self.voice.clone(), self.speed);
        let names = names.join(", ");
        let work = async move {
            let rendered = match words {
                Some(words) => render(synth, words, voice, speed).await,
                None => None,
            };
            let (text, pcm) = match (rendered, clip) {
                (Some((text, pcm)), _) => (text, pcm),
                (None, Some(clip)) => (clip.text.to_string(), (*clip.pcm).clone()),
                (None, None) => return,
            };
            tokio::select! {
                biased;
                () = turn.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            debug::audio(|| format!("ack: {text:?} before {names}"));
            if let Some(on_ack) = &on_ack {
                on_ack(&text);
            }
            out.push(Frame::Ack {
                text,
                audio: Audio::new(pcm, RATE),
                turn,
            })
            .await;
        };
        tokio::spawn(work);
    }
}

#[async_trait]
impl Stage for AckStage {
    fn name(&self) -> &'static str {
        "ack"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Down {
            match &frame {
                // A new turn: the operator's (its end, or a barge-in) or one the session
                // queued (the wake's own request, a typed turn).
                Frame::UserStarted
                | Frame::UserStopped
                | Frame::ThinkingStart
                | Frame::Interruption => self.new_turn(),
                Frame::ReplyTurn(t) => self.reply_turn = Some(t.clone()),
                Frame::VadStopped { .. } => self.segment_ended = Some(std::time::Instant::now()),
                Frame::LlmText(_) | Frame::Speak(_) => self.spoke = true,
                Frame::VoiceChange { voice, speed } => {
                    self.voice = voice.clone();
                    self.speed = *speed;
                    self.acks.warm(self.synth.clone(), voice, *speed);
                }
                Frame::FunctionCallsStarted(names) => {
                    let names = names.clone();
                    self.acknowledge(&names, out).await;
                }
                Frame::HandOver { names, .. } => {
                    let names = names.clone();
                    self.handed_over(&names, out).await;
                }
                Frame::ToolStarting { name, said } => {
                    let (name, said) = (name.clone(), said.clone());
                    self.starting(&name, said, out).await;
                }
                _ => {}
            }
        }
        out.forward(frame, direction).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::drain;
    use crate::tts::Synth;

    struct Fake;

    impl Synth for Fake {
        fn voices(&self) -> Vec<String> {
            vec!["af_heart".into()]
        }
        fn synthesize(&mut self, text: &str, _: &str, _: f32) -> ghost_infer::Result<Vec<i16>> {
            Ok(vec![1000; 100 * text.len()])
        }
    }

    fn synth() -> SharedSynth {
        Arc::new(Mutex::new(Fake))
    }

    #[test]
    fn test_every_line_is_one_or_two_words() {
        for line in READ.iter().chain(CHANGE) {
            let n = crate::echo::words(line).len();
            assert!((1..=2).contains(&n), "{line}");
        }
        assert!(words().contains("sure") && words().contains("let's"));
    }

    #[test]
    fn test_a_clip_is_trimmed_to_its_words() {
        let rate = RATE as usize;
        let mut pcm = vec![0i16; rate / 10]; // 0.1 s of silence
        pcm.extend(vec![8000i16; rate / 4]); // 0.25 s of words
        pcm.extend(vec![30i16; rate / 5]); // 0.2 s of Kokoro's quiet tail
        let t = trim(&pcm);
        let secs = t.len() as f64 / f64::from(RATE);
        assert!(
            (secs - (0.25 + LEAD_SECS + TAIL_SECS)).abs() < 0.002,
            "{secs}"
        );
        assert!(trim(&[0; 100]).is_empty());
    }

    #[test]
    fn test_never_the_same_twice_in_a_row() {
        let acks = Acks::new(None);
        let v = acks.render(&synth(), "af_heart", 1.0);
        assert_eq!(v.read.len(), READ.len());
        assert_eq!(v.change.len(), CHANGE.len());
        let mut p = Picker::default();
        let mut last = "";
        for _ in 0..200 {
            let c = p.pick(&v.read).unwrap();
            assert_ne!(c.text, last);
            last = c.text;
        }
        let one = vec![v.read[0].clone()];
        assert_eq!(p.pick(&one).unwrap().text, v.read[0].text); // a list of one repeats
    }

    #[test]
    fn test_rendered_once_then_read_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let acks = Acks::new(Some(dir.path()));
        let first = acks.render(&synth(), "af_heart", 1.0);
        let file = dir
            .path()
            .join(VERSION)
            .join("af_heart@1.00")
            .join("one-sec.wav");
        assert!(file.exists(), "{file:?}");
        struct Mute;
        impl Synth for Mute {
            fn voices(&self) -> Vec<String> {
                Vec::new()
            }
            fn synthesize(&mut self, _: &str, _: &str, _: f32) -> ghost_infer::Result<Vec<i16>> {
                panic!("rendered again")
            }
        }
        let again = Acks::new(Some(dir.path())).render(
            &(Arc::new(Mutex::new(Mute)) as SharedSynth),
            "af_heart",
            1.0,
        );
        assert_eq!(again.read[1].pcm, first.read[1].pcm);
    }

    #[tokio::test]
    async fn test_warm_renders_in_the_background() {
        let acks = Acks::new(None);
        acks.warm(synth(), "af_heart", 1.0);
        for _ in 0..200 {
            if acks.get("af_heart", 1.0).is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("never rendered");
    }

    // --- the stage ----------------------------------------------------------------

    fn stage(said: Arc<Mutex<Vec<String>>>) -> AckStage {
        stage_with(said, TurnControl::new(CancellationToken::new()))
    }

    fn stage_with(said: Arc<Mutex<Vec<String>>>, turns: Arc<TurnControl>) -> AckStage {
        let acks = Acks::new(None);
        acks.insert("af_heart", 1.0, acks.render(&synth(), "af_heart", 1.0));
        let outside: Outside = Arc::new(|name: &str| match name {
            n if n.starts_with("github_issue_write") => Some(AckKind::Change),
            n if n.starts_with("github_") => Some(AckKind::Read),
            _ => None,
        });
        AckStage::new(acks, synth(), outside, "af_heart", 1.0, turns)
            .with_on_ack(Arc::new(move |t: &str| said.lock().unwrap().push(t.into())))
    }

    async fn run(s: &mut AckStage, frames: Vec<Frame>) -> Vec<String> {
        let (out, mut rx) = Out::capture();
        for f in frames {
            s.process(f, Direction::Down, &out).await;
        }
        drain(&mut rx)
            .await
            .into_iter()
            .filter_map(|f| match f {
                Frame::Ack { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    fn calls(names: &[&str]) -> Frame {
        Frame::FunctionCallsStarted(names.iter().map(|n| n.to_string()).collect())
    }

    fn handed(names: &[&str]) -> Frame {
        Frame::HandOver {
            names: names.iter().map(|n| n.to_string()).collect(),
            secs: 0.6,
        }
    }

    #[tokio::test]
    async fn test_a_hand_over_is_acknowledged_once_and_not_again_at_the_call() {
        // ADR 0050: the fast model asked for the main one. The gap is covered whatever
        // it asked for, a tool of an app's or `think_harder`; the main model's own call,
        // when it comes, is not acknowledged a second time.
        for names in [vec!["github_issue_read"], vec!["think_harder"], vec![]] {
            let said = Arc::new(Mutex::new(Vec::new()));
            let mut s = stage(said.clone());
            let acks = run(
                &mut s,
                vec![
                    Frame::UserStopped,
                    handed(&names),
                    calls(&["github_issue_read"]),
                ],
            )
            .await;
            assert_eq!(acks.len(), 1, "{names:?}");
            assert!(READ.contains(&acks[0].as_str()), "{acks:?}");
        }
        // A change is acknowledged as one; a goodbye's own words are what is said.
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut s = stage(said.clone());
        let acks = run(
            &mut s,
            vec![Frame::UserStopped, handed(&["github_issue_write"])],
        )
        .await;
        assert!(CHANGE.contains(&acks[0].as_str()), "{acks:?}");
        let none = run(&mut s, vec![Frame::UserStopped, handed(&["hang_up"])]).await;
        assert!(none.is_empty(), "{none:?}");
    }

    #[tokio::test]
    async fn test_a_hand_over_waits_out_the_grace_and_is_not_said_if_the_operator_went_on() {
        // The turn closed after the short grace (ADR 0049) and nothing was done for it:
        // the operator may be pausing. The acknowledgment waits until the grace has run
        // from the end of their speech; going on first takes the turn, and the word.
        for goes_on in [false, true] {
            let said = Arc::new(Mutex::new(Vec::new()));
            let turns = TurnControl::new(CancellationToken::new());
            let mut s = stage_with(said.clone(), turns.clone()).with_settle(0.3);
            let (out, mut rx) = Out::capture();
            for f in [
                Frame::VadStopped { stop_secs: 0.2 },
                Frame::UserStopped,
                handed(&["github_issue_read"]),
            ] {
                s.process(f, Direction::Down, &out).await;
            }
            let acks = |frames: Vec<Frame>| {
                frames
                    .iter()
                    .filter(|f| matches!(f, Frame::Ack { .. }))
                    .count()
            };
            assert_eq!(acks(drain(&mut rx).await), 0, "not yet");
            if goes_on {
                turns.interrupt();
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            assert_eq!(acks(drain(&mut rx).await), usize::from(!goes_on));
            assert_eq!(said.lock().unwrap().len(), usize::from(!goes_on));
        }
    }

    #[tokio::test]
    async fn test_a_turn_that_goes_to_an_app_is_acknowledged_once() {
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut s = stage(said.clone());
        let acks = run(
            &mut s,
            vec![
                Frame::UserStopped,
                Frame::LlmStart,
                calls(&["github_quotient_search_issues"]),
                Frame::LlmEnd,
                Frame::LlmStart,
                calls(&["github_quotient_issue_read"]), // a second round: no second ack
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(acks.len(), 1);
        assert!(READ.contains(&acks[0].as_str()), "{acks:?}");
        assert_eq!(*said.lock().unwrap(), acks); // the echo filter was told
        // the next turn is acknowledged again, with other words
        let next = run(
            &mut s,
            vec![Frame::UserStopped, calls(&["github_quotient_issue_read"])],
        )
        .await;
        assert_eq!(next.len(), 1);
        assert_ne!(next[0], acks[0]);
    }

    #[tokio::test]
    async fn test_a_change_gets_the_change_words() {
        let mut s = stage(Arc::default());
        let acks = run(
            &mut s,
            vec![
                Frame::ThinkingStart, // the wake's own request
                calls(&["github_quotient_search_issues", "github_issue_write"]),
            ],
        )
        .await;
        assert!(CHANGE.contains(&acks[0].as_str()), "{acks:?}");
    }

    #[tokio::test]
    async fn test_no_ack_for_what_ghost_has_a_goodbye_or_a_reply_already_speaking() {
        let mut s = stage(Arc::default());
        let none = run(
            &mut s,
            vec![
                // an answer from what Ghost knows
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::LlmText("The first one was the Redis timeout.".into()),
                Frame::LlmEnd,
                // hanging up, holding
                Frame::UserStopped,
                calls(&["hang_up"]),
                Frame::UserStopped,
                starting("hold", None),
                // a reply already being spoken when it reaches for an app
                Frame::UserStopped,
                Frame::LlmText("Here are the three. ".into()),
                calls(&["github_quotient_issue_read"]),
            ],
        )
        .await;
        assert!(none.is_empty(), "{none:?}");
    }

    #[tokio::test]
    async fn ghosts_own_lookups_are_acknowledged_too() {
        // 2026-11-12: recall, the tool search and the docs each sent the model round again,
        // a second or two of silence each, and none was acknowledged.
        let mut s = stage(Arc::default());
        let read = run(&mut s, vec![Frame::UserStopped, calls(&["ghost_recall"])]).await;
        assert!(READ.contains(&read[0].as_str()), "{read:?}");
        let change = run(&mut s, vec![Frame::UserStopped, calls(&["change"])]).await;
        assert!(CHANGE.contains(&change[0].as_str()), "{change:?}");
    }

    fn starting(name: &str, said: Option<&str>) -> Frame {
        Frame::ToolStarting {
            name: name.into(),
            said: said.map(String::from),
        }
    }

    async fn acks_within(
        rx: &mut tokio::sync::mpsc::Receiver<crate::frames::Pushed>,
        ms: u64,
    ) -> Vec<String> {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        drain(rx)
            .await
            .into_iter()
            .filter_map(|f| match f {
                Frame::Ack { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_call_is_acknowledged_as_it_starts_with_the_models_own_words_when_it_wrote_some() {
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut s = stage(said.clone());
        let (out, mut rx) = Out::capture();
        for f in [
            Frame::UserStopped,
            starting("github_issue_read", Some("Checking four thirteen.")),
            // the call made whole, a second later: not acknowledged again
            calls(&["github_issue_read"]),
        ] {
            s.process(f, Direction::Down, &out).await;
        }
        assert_eq!(
            acks_within(&mut rx, 100).await,
            vec!["Checking four thirteen."]
        );
        assert_eq!(*said.lock().unwrap(), vec!["Checking four thirteen."]);
        // No words: one of the fixed lines, at once.
        let none = run(
            &mut s,
            vec![Frame::UserStopped, starting("ghost_tools_search", None)],
        )
        .await;
        assert!(READ.contains(&none[0].as_str()), "{none:?}");
    }

    #[tokio::test]
    async fn a_call_start_waits_out_the_grace_and_is_not_said_if_the_operator_went_on() {
        for goes_on in [false, true] {
            let turns = TurnControl::new(CancellationToken::new());
            let mut s = stage_with(Arc::default(), turns.clone()).with_settle(0.3);
            let (out, mut rx) = Out::capture();
            for f in [
                Frame::VadStopped { stop_secs: 0.2 },
                Frame::UserStopped,
                starting("github_issue_read", Some("Checking.")),
            ] {
                s.process(f, Direction::Down, &out).await;
            }
            assert!(acks_within(&mut rx, 50).await.is_empty(), "not yet");
            if goes_on {
                turns.interrupt();
            }
            let later = acks_within(&mut rx, 400).await;
            assert_eq!(later.len(), usize::from(!goes_on), "{later:?}");
        }
    }

    #[tokio::test]
    async fn test_a_cut_off_turn_is_not_acknowledged() {
        let mut s = stage(Arc::default());
        let token = CancellationToken::new();
        token.cancel();
        let none = run(
            &mut s,
            vec![
                Frame::UserStopped,
                Frame::ReplyTurn(token),
                calls(&["github_quotient_search_issues"]),
            ],
        )
        .await;
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn test_nothing_said_before_the_voice_is_rendered() {
        let acks = Acks::new(None);
        let outside: Outside = Arc::new(|_| Some(AckKind::Read));
        let mut s = AckStage::new(
            acks.clone(),
            Arc::new(Mutex::new(crate::tts::NoSynth)),
            outside,
            "am_adam",
            1.0,
            TurnControl::new(CancellationToken::new()),
        );
        let none = run(&mut s, vec![Frame::UserStopped, calls(&["github_x"])]).await;
        assert!(none.is_empty());
    }
}
