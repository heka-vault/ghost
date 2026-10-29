//! Hold (ADR 0021): "hang on, Ghost" keeps the session while the operator talks to someone
//! else. While on hold the gate below drops everything the microphone hears, on the spot,
//! without logging it, storing it or showing it to the model; the only thing it looks for is
//! the operator addressing Ghost by name at the start of an utterance ("okay Ghost, we have a
//! solution"), which resumes the session with that utterance as the next turn.
//!
//! The rule is the desk's passive rule applied inside a session: local STT runs so the wake
//! phrase can be heard, nothing else leaves the machine. The session's context, the thread
//! and any question Ghost had asked all survive the hold.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use ghost_core::source::BoxFuture;
use ghost_infer::wake::{DEFAULT_WAKE_PHRASES, match_wake};
use regex::Regex;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};

/// What may come before the name and still count as addressing Ghost: "okay Ghost",
/// "alright, so, Ghost", "um, hey Ghost". Anything else before it ("we should fix that in
/// Ghost") is the room talking about Ghost, not to it.
pub const LEAD_INS: &[&str] = &[
    "okay", "ok", "alright", "all", "right", "hey", "so", "um", "uh", "oh", "and", "well", "now",
];

/// Addressing Ghost on hold is "Ghost, ..." first of all: the bare name, which the passive
/// wake phrases leave out on purpose (too easy to wake by), is right here, because Ghost is
/// waiting to be called and only the start of an utterance counts.
pub const BARE_NAME: &[&str] = &["ghost"];

pub type OnResume = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

static NOT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s]").unwrap());

/// The words of `text`, as the wake matcher counts them.
pub fn words(text: &str) -> Vec<String> {
    NOT_WORD
        .replace_all(&text.to_lowercase(), " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The rest of `text` when it starts by addressing Ghost, else None. "" when the name was
/// all that was said ("Ghost?").
///
/// Exact first, on the wake phrases and the bare name. Then, with `fuzzy` > 0, the
/// phonetic near-miss the desk's wake detector also accepts ("he goes" for "hey ghost"),
/// on the multi-word phrases only and only as a short call (the near-miss and at most two
/// words after it): a near-miss of the bare name ("guys") or "so he goes to the store"
/// would otherwise resume Ghost from across the room.
pub fn addressed(text: &str, phrases: &[impl AsRef<str>], fuzzy: f64) -> Option<String> {
    let ws = words(text);
    if ws.is_empty() {
        return None;
    }
    let all: Vec<String> = BARE_NAME
        .iter()
        .map(|s| s.to_string())
        .chain(phrases.iter().map(|p| p.as_ref().to_string()))
        .collect();
    let mut m = match_wake(text, &all, 0.0);
    if m.is_none() && fuzzy > 0.0 {
        let multi: Vec<String> = phrases
            .iter()
            .map(|p| p.as_ref().to_string())
            .filter(|p| p.split_whitespace().count() > 1)
            .collect();
        m = match_wake(text, &multi, fuzzy);
        if let Some(found) = &m
            && words(&found.remainder).len() > 2
        {
            m = None;
        }
    }
    let m = m?;
    // Where the phrase sits: everything before it must be a lead-in ("okay", "so").
    let tail = words(&m.remainder).len();
    let phrase_len = m.phrase.split_whitespace().count();
    let start = ws.len() as i64 - tail as i64 - phrase_len as i64;
    if start < 0
        || !ws[..start as usize]
            .iter()
            .all(|w| LEAD_INS.contains(&w.as_str()))
    {
        return None;
    }
    Some(m.remainder)
}

/// The session's hold flag, shared by the gate and whoever puts the session on hold.
#[derive(Debug, Default)]
pub struct HoldState(AtomicBool);

impl HoldState {
    pub fn on_hold(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn hold(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn release(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Right after STT. Off hold it passes everything. On hold it drops what the mic hears,
/// except an utterance addressed to Ghost, which ends the hold through
/// `on_resume(remainder)` (the frame itself is dropped; the resume injects the words).
///
/// Audio is not dropped: the VAD below it segments speech for the turn, and starving it
/// of audio silenced STT on hold, and nothing could bring Ghost back (2026-10-20).
pub struct HoldGate {
    on_resume: OnResume,
    phrases: Vec<String>,
    fuzzy: f64,
    state: Arc<HoldState>,
}

impl HoldGate {
    pub fn new(on_resume: OnResume, phrases: Option<Vec<String>>, fuzzy: f64) -> Self {
        Self {
            on_resume,
            phrases: phrases
                .unwrap_or_else(|| DEFAULT_WAKE_PHRASES.iter().map(|s| s.to_string()).collect()),
            fuzzy,
            state: Arc::new(HoldState::default()),
        }
    }

    pub fn state(&self) -> Arc<HoldState> {
        self.state.clone()
    }

    /// Share the session's hold flag, so whoever puts the session on hold flips the gate.
    pub fn with_state(mut self, state: Arc<HoldState>) -> Self {
        self.state = state;
        self
    }

    pub fn on_hold(&self) -> bool {
        self.state.on_hold()
    }

    pub fn hold(&self) {
        self.state.hold();
    }

    pub fn release(&self) {
        self.state.release();
    }
}

#[async_trait]
impl Stage for HoldGate {
    fn name(&self) -> &'static str {
        "hold"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if !self.state.on_hold() || direction != Direction::Down {
            out.forward(frame, direction).await;
            return;
        }
        match &frame {
            Frame::Transcript(text) => match addressed(text, &self.phrases, self.fuzzy) {
                None => {
                    debug::audio(|| format!("hold: dropped {text:?}"));
                }
                Some(remainder) => {
                    self.state.release();
                    (self.on_resume)(remainder).await;
                }
            },
            Frame::Interim(_)
            | Frame::UserStarted
            | Frame::UserStopped
            | Frame::VadStarted { .. }
            | Frame::VadStopped { .. } => {}
            _ => out.forward(frame, direction).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::frames::{Audio, drain};

    #[test]
    fn test_addressed_at_the_start_only() {
        let phrases = ["hey ghost", "okay ghost"];
        for (text, expect) in [
            ("Ghost, we have a solution.", Some("we have a solution.")),
            ("okay ghost, close seventeen", Some("close seventeen")),
            (
                "Alright, so, Ghost: what did we decide?",
                Some("what did we decide?"),
            ),
            ("hey ghost", Some("")),
            ("Ghost?", Some("")),
            // the room talking about Ghost, not to it
            ("we should fix that in Ghost", None),
            ("I think Ghost could do that part", None),
            ("let's move on", None),
            ("", None),
        ] {
            assert_eq!(
                addressed(text, &phrases, 0.0).as_deref(),
                expect,
                "{text:?}"
            );
        }
    }

    #[test]
    fn test_addressed_fuzzy_like_the_wake_detector() {
        let phrases = ["hey ghost", "okay ghost"];
        for (text, expect) in [
            ("he goes", Some("")), // Parakeet's "hey ghost"
            ("he goes, you there", Some("you there")),
            ("hey guys", Some("")),
            ("guys, let's move on", None), // a near-miss of the bare name never counts
            ("so he goes to the store", None), // a near-miss only as a short call
        ] {
            assert_eq!(
                addressed(text, &phrases, 0.8).as_deref(),
                expect,
                "{text:?}"
            );
        }
    }

    fn make() -> (HoldGate, Arc<Mutex<Vec<String>>>) {
        let resumed = Arc::new(Mutex::new(Vec::new()));
        let r = resumed.clone();
        let on_resume: OnResume = Arc::new(move |remainder| {
            let r = r.clone();
            Box::pin(async move {
                r.lock().unwrap().push(remainder);
            })
        });
        (
            HoldGate::new(on_resume, Some(vec!["ghost".into()]), 0.0),
            resumed,
        )
    }

    #[tokio::test]
    async fn test_off_hold_everything_passes() {
        let (mut gate, resumed) = make();
        let (out, mut rx) = Out::capture();
        gate.process(Frame::Transcript("close it".into()), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert!(matches!(pushed.last(), Some(Frame::Transcript(_))));
        assert!(resumed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_on_hold_the_room_is_dropped_and_the_name_resumes() {
        let (mut gate, resumed) = make();
        let (out, mut rx) = Out::capture();
        gate.hold();
        gate.process(Frame::UserStarted, Direction::Down, &out)
            .await;
        gate.process(
            Frame::Transcript("what about the ghost repo".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert!(drain(&mut rx).await.is_empty());
        assert!(resumed.lock().unwrap().is_empty());
        // audio still flows: the VAD below needs it to segment speech for STT
        gate.process(
            Frame::AudioIn(Audio::new(vec![0], 16000)),
            Direction::Down,
            &out,
        )
        .await;
        assert!(matches!(
            drain(&mut rx).await.last(),
            Some(Frame::AudioIn(_))
        ));
        // Ghost's own frames going down still pass (a "take your time" on its way to TTS)
        gate.process(
            Frame::Speak("Take your time.".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert!(matches!(drain(&mut rx).await.last(), Some(Frame::Speak(_))));
        gate.process(
            Frame::Transcript("okay Ghost, we picked option two".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert_eq!(
            *resumed.lock().unwrap(),
            vec!["we picked option two".to_string()]
        );
        assert!(!gate.on_hold());
        // the resuming frame itself is not forwarded: the resume injects the words
        assert!(drain(&mut rx).await.is_empty());
    }
}
