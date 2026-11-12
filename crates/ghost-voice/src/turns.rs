//! The user's turn: when it starts, when it ends, and what the model is handed.
//!
//! Pipecat's default starts a user turn on any VAD activity, and starting a turn while the
//! bot is speaking is an interruption. On a workstation with open speakers, Kokoro's own
//! voice reaching the microphone is VAD activity, so with the default rule Ghost cut itself
//! off after a few words. Echo cancellation in PipeWire (`infra/pipewire`) is the real fix.
//! [`BargeInUserTurnStartStrategy`] is the backstop for when it leaks:
//!
//! - While Ghost is silent, VAD starts the turn at once, as before.
//! - While Ghost is speaking, only a transcript of at least `min_words` words starts (and so
//!   interrupts) the turn. Shorter fragments are discarded so they never leak into the next
//!   turn. `min_words=0` disables barge-in entirely. An interim transcript counts (`stt`
//!   re-reads the operator's open segment while Ghost speaks), so the operator needn't
//!   pause to cut Ghost off.
//!
//! The rule learns that Ghost is speaking two ways: the `BotStarted` and `BotStopped`
//! frames, and the output stage's shared flag (`crate::speaking`), true from the first
//! sample written. The frames reach this stage late, behind a TTS stage still rendering
//! the rest of a long reply, and the leak of Ghost's own first words past the echo
//! canceller falls in exactly that window; the flag is what closes it.
//!
//! [`PatientTurnStopStrategy`] is Smart Turn's end of turn, held for `grace_secs` before it
//! counts, with the speculative answer started at the verdict, and not taken when the
//! words stop mid-thought. How long the hold is depends on how sure the end is
//! ([`TurnTiming`], ADR 0049): a short one when Smart Turn is sure and the words end a
//! sentence, the usual one otherwise, the long silences for words that stop mid-thought.
//! [`UserAggregator`] is the stage that runs both over the frames,
//! keeps the turn's pieces, and hands the model the whole turn; when the operator goes on
//! before any of the answer was heard or done, or just as its first words began, it
//! reopens that turn ([`TurnControl`]).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_core::source::BoxFuture;
use ghost_infer::{SmartTurn, TurnVerdict};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::debug;
use crate::frames::{Direction, Frame, Metric, Out, Stage};
use crate::latency::{LatencyProbe, null_probe};
use crate::speaking::Speaking;
use crate::stt::{PRE_ROLL_SECS, SAMPLE_RATE, SharedRecognizer, TRAILING_SECS, recognize};

// --- the start -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessFrameResult {
    /// The strategy acted on the frame; the others need not see it.
    Stop,
    Continue,
}

/// What a start strategy triggered on the aggregator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartEvent {
    Started,
    Reset,
}

/// How long a short transcript heard over Ghost counts toward a barge-in. The echo
/// canceller passes the operator's voice under Ghost's audio in 150–250 ms pieces with
/// holes between (measured 2026-10-30), one word each, so "stop… stop… stop" and "Ghost"
/// + "hold on" arrive as separate finals that never make `min_words` alone.
const BURST_WINDOW: Duration = Duration::from_secs(2);

pub struct BargeInUserTurnStartStrategy {
    min_words: usize,
    bot_speaking: bool,
    speaking: Speaking,
    events: Vec<StartEvent>,
    /// Finals too short to interrupt on their own, heard over the reply being spoken.
    bursts: Vec<(Instant, usize)>,
}

impl BargeInUserTurnStartStrategy {
    pub fn new(min_words: usize) -> Self {
        Self {
            min_words,
            bot_speaking: false,
            speaking: Speaking::new(),
            events: Vec::new(),
            bursts: Vec::new(),
        }
    }

    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking = speaking;
        self
    }

    pub fn set_speaking(&mut self, speaking: Speaking) {
        self.speaking = speaking;
    }

    /// Ghost's voice is at the speaker: the frames said so, or the output stage's flag does.
    pub fn bot_speaking(&self) -> bool {
        self.bot_speaking || self.speaking.is_on()
    }

    /// What the strategy triggered since the last call, in order.
    pub fn take_events(&mut self) -> Vec<StartEvent> {
        std::mem::take(&mut self.events)
    }

    /// A turn opened by any strategy ends the bot's speech (interruption), so subsequent
    /// fragments belong to the operator.
    pub fn handle_user_turn_started(&mut self) {
        self.bot_speaking = false;
        self.bursts.clear();
    }

    pub fn process_frame(&mut self, frame: &Frame) -> ProcessFrameResult {
        match frame {
            Frame::BotStarted => {
                self.bot_speaking = true;
                self.bursts.clear(); // a new reply: what was heard over the last one is spent
            }
            Frame::BotStopped => {
                self.bot_speaking = false;
                self.bursts.clear();
            }
            Frame::VadStarted { .. } => {
                if !self.bot_speaking() {
                    debug::audio(|| "turn: started by VAD".into());
                    self.events.push(StartEvent::Started);
                    return ProcessFrameResult::Stop;
                }
                debug::audio(|| "turn: VAD while Ghost speaks, waiting for words".into());
            }
            Frame::Transcript(text) => return self.on_text(text, false),
            Frame::Interim(text) => return self.on_text(text, true),
            _ => {}
        }
        ProcessFrameResult::Continue
    }

    fn on_text(&mut self, text: &str, interim: bool) -> ProcessFrameResult {
        let words = text.split_whitespace().count();
        if !self.bot_speaking() {
            if words >= 1 {
                self.events.push(StartEvent::Started);
                return ProcessFrameResult::Stop;
            }
            return ProcessFrameResult::Continue;
        }
        if self.min_words > 0 {
            let now = Instant::now();
            self.bursts
                .retain(|(at, _)| now.duration_since(*at) <= BURST_WINDOW);
            let earlier: usize = self.bursts.iter().map(|(_, n)| n).sum();
            if words + earlier >= self.min_words {
                debug::audio(|| {
                    format!(
                        "turn: barge-in on {}{text:?}{}",
                        if interim { "interim " } else { "" },
                        if earlier > 0 {
                            format!(", with {earlier} word(s) heard over Ghost just before")
                        } else {
                            String::new()
                        }
                    )
                });
                self.bursts.clear();
                self.events.push(StartEvent::Started);
                return ProcessFrameResult::Stop;
            }
        }
        if interim {
            return ProcessFrameResult::Continue; // nothing was aggregated; the next may say more
        }
        // Too short to be a deliberate interruption on its own: most likely the bot's own
        // voice. Drop it rather than queue it, but count it for `BURST_WINDOW` in case it
        // is the operator coming through in pieces.
        debug::audio(|| format!("turn: dropped {text:?} while Ghost speaks"));
        self.bursts.push((Instant::now(), words));
        self.events.push(StartEvent::Reset);
        ProcessFrameResult::Continue
    }
}

// --- Smart Turn ------------------------------------------------------------------------

/// The model behind the analyzer: Smart Turn, or a fake in tests.
pub trait TurnModel: Send {
    fn predict(&mut self, audio: &[f32]) -> ghost_infer::Result<TurnVerdict>;
}

impl TurnModel for SmartTurn {
    fn predict(&mut self, audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
        SmartTurn::predict(self, audio)
    }
}

/// Pipecat's `SmartTurnParams`, with Ghost's two silences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmartTurnParams {
    /// When the model says incomplete but the words end a sentence, the turn still closes
    /// after this much silence.
    pub stop_secs: f64,
    /// When the words don't end a sentence (the model says incomplete and there is no full
    /// stop, or the words stop mid-thought whatever the model says), this much.
    pub unfinished_secs: f64,
    /// A full stop shortens the wait only when the model is this unsure at least. Parakeet
    /// ends nearly every segment with one ("Can you read me issue."), and the model at 0.04
    /// was right that the operator was going on; at 0.15 ("Yeah.") and 0.27 it was wrong.
    pub sentence_min_prob: f32,
    /// Added to the grace when the words end on a number ("four one five"): people pause
    /// after reading one out and often go on ("… from quotient").
    pub number_secs: f64,
    /// Audio before the speech started that is kept for the model.
    pub pre_speech_secs: f64,
    pub max_duration_secs: f64,
}

impl Default for SmartTurnParams {
    fn default() -> Self {
        Self {
            stop_secs: 1.6,
            unfinished_secs: 3.0,
            sentence_min_prob: 0.1,
            number_secs: 0.5,
            pre_speech_secs: 1.0,
            max_duration_secs: 8.0,
        }
    }
}

/// Smart Turn v3 whose loaded ONNX session is shared by every session's analyzer. An
/// analyzer buffers one audio stream's speech and silence, so a session that ended
/// mid-utterance would hand its half-turn to the next one, and two live sessions would
/// mix their audio: `fork` gives each pipeline its own analyzer over the one loaded model.
pub struct SmartTurnAnalyzer {
    model: Arc<Mutex<dyn TurnModel>>,
    params: SmartTurnParams,
    audio: Vec<i16>,
    speech_triggered: bool,
    sample_rate: u32,
}

impl SmartTurnAnalyzer {
    pub fn new(model: Arc<Mutex<dyn TurnModel>>, params: SmartTurnParams) -> Self {
        Self {
            model,
            params,
            audio: Vec::new(),
            speech_triggered: false,
            sample_rate: 16_000,
        }
    }

    pub fn fork(&self) -> Self {
        Self::new(self.model.clone(), self.params)
    }

    pub fn model(&self) -> &Arc<Mutex<dyn TurnModel>> {
        &self.model
    }

    pub fn params(&self) -> SmartTurnParams {
        self.params
    }

    pub fn speech_triggered(&self) -> bool {
        self.speech_triggered
    }

    pub fn buffered(&self) -> usize {
        self.audio.len()
    }

    /// Audio as the VAD saw it: speech starts the buffer; silence before it is kept only
    /// for the pre-speech window.
    pub fn append_audio(&mut self, samples: &[i16], is_speech: bool) {
        if is_speech {
            self.speech_triggered = true;
        }
        self.audio.extend_from_slice(samples);
        let max = (self.params.max_duration_secs * f64::from(self.sample_rate)) as usize;
        if !self.speech_triggered {
            let keep = (self.params.pre_speech_secs * f64::from(self.sample_rate)) as usize;
            if self.audio.len() > keep {
                let drop = self.audio.len() - keep;
                self.audio.drain(..drop);
            }
        } else if self.audio.len() > max {
            let drop = self.audio.len() - max;
            self.audio.drain(..drop);
        }
    }

    pub fn clear(&mut self) {
        self.audio.clear();
        self.speech_triggered = false;
    }

    /// The model's verdict on the turn so far, and how long it took; the buffer is cleared.
    /// Off the async runtime: the model runs on a blocking thread.
    pub async fn analyze_end_of_turn(&mut self) -> Option<(TurnVerdict, f64)> {
        if !self.speech_triggered || self.audio.is_empty() {
            self.clear();
            return None;
        }
        let audio = ghost_infer::audio::to_f32(&self.audio);
        self.clear();
        let model = self.model.clone();
        let began = Instant::now();
        let verdict = tokio::task::spawn_blocking(move || {
            let mut m = model.lock().unwrap_or_else(|e| e.into_inner());
            m.predict(&SmartTurn::last_eight_seconds(&audio))
        })
        .await
        .ok()?
        .ok()?;
        Some((verdict, began.elapsed().as_secs_f64() * 1000.0))
    }
}

// --- the stop --------------------------------------------------------------------------

/// What the stop strategy triggers on the aggregator.
#[async_trait]
pub trait TurnStopSink: Send + Sync {
    /// Smart Turn's own end of turn: the inference a confirmed turn runs.
    async fn turn_stopped(&self);
    /// Start answering now; the answer is held until the turn is confirmed.
    async fn inference_triggered(&self, speculation: Option<String>);
    /// The turn ended as speculated: the held answer is the answer.
    async fn turn_finalized(&self, confirms_speculation: bool);
    /// The held answer is thrown away.
    async fn speculation_cancelled(&self);
    /// The turn is about to close on the short grace: until `at`, when the grace itself
    /// would have run out, nothing may be done for it (ADR 0049).
    async fn settles(&self, _at: Instant) {}
}

#[derive(Default)]
struct StopInner {
    held: Option<JoinHandle<()>>,
    speculation: Option<String>,
    fallback: Option<JoinHandle<()>>,
    /// When the incomplete turn's silence began, so the words can shorten it.
    fallback_began: Option<Instant>,
    /// Smart Turn's probability that the turn was done when the fallback began.
    fallback_prob: f32,
    /// When the segment's silence began: the pause's length, for the record, when the
    /// operator goes on or the turn closes.
    silence_began: Option<Instant>,
    /// The segment's words reached the strategy since its verdict: a sure end is read off
    /// them, never off the words of the segment before.
    arrived: bool,
    /// The segment's words read at its first quiet (ADR 0054), until the stop's arrive.
    early: Option<String>,
    /// The stop's words for this segment arrived: early words after them are moot.
    final_seen: bool,
}

pub type TurnText = Arc<dyn Fn() -> String + Send + Sync>;

/// Last words nobody ends a request on, even with Parakeet's full stop after them:
/// fillers, conjunctions, articles.
const UNFINISHED: &[&str] = &[
    "um", "umm", "uh", "uhh", "uhm", "er", "erm", "ah", "hmm", "mm", //
    "and", "but", "or", "so", "because", "cause", "if", "then", "than", "that", "which", "who",
    "whose", "where", "when", "while", "whether", "although", "though", "unless", "until", //
    "the", "a", "an", "my", "your", "our", "their", "his", "her", "its", "some", "any", "every",
    "each",
];
/// A preposition still waiting for its object, unless a full stop says it was a particle
/// ("turn my music back on.", "pick it up.").
const UNFINISHED_PREPOSITIONS: &[&str] = &[
    "about", "to", "for", "of", "with", "from", "in", "on", "at", "by", "into", "onto", "like",
    "as", "over", "under", "after", "before", "between", "through", "without",
];
/// A subject or an auxiliary still waiting for its verb, unless a full stop says the short
/// answer was the whole of it ("Right, I did.", "Yes, I can.").
const UNFINISHED_UNLESS_STOPPED: &[&str] = &[
    "i", "i'm", "we", "we're", "is", "are", "was", "were", "am", "be", "been", "have", "has",
    "had", "do", "does", "did", "don't", "doesn't", "didn't", "can", "can't", "could", "would",
    "should", "will", "won't", "might", "must", "gonna", "wanna",
];

/// Words that take something after them, a number or a name, and got nothing: "read me
/// issue" wants its number. Whole with a determiner within the two words before them
/// ("close the issue", "what's the issue number"). Only words that nearly always take
/// one: "the app repo" and "open the file" are ends.
const WANTS_ARGUMENT: &[&str] = &[
    "issue", "number", "pr", "ticket", "line", "item", "step", "page", "version", "build",
    "commit", "called", "named", "titled",
];
const DETERMINERS: &[&str] = &[
    "the", "a", "an", "this", "that", "my", "your", "our", "their", "his", "her", "its", "each",
    "every", "any", "some", "no", "which", "one", "same", "first", "last", "next", "previous",
    "other", "another",
];

fn last_words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split_whitespace().rev().map(|w| {
        w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
            .replace('’', "'")
            .to_lowercase()
    })
}

/// The transcript stops mid-thought: a trailing comma or an ellipsis, a last word from
/// [`UNFINISHED`], or one from [`WANTS_ARGUMENT`] with no determiner before it. A full stop
/// is read past: Parakeet writes one at nearly every segment's end ("Read me issue."), so
/// the words decide; a question or exclamation mark is an end unless the last word wants
/// an argument. Smart Turn hears the voice;
/// this reads the words, so "can you count, um" isn't an end however it sounded.
pub fn sounds_unfinished(text: &str) -> bool {
    let text = text.trim_end();
    if text.ends_with("...") || text.ends_with('…') || text.ends_with(',') {
        return true;
    }
    let asked = text.ends_with(['?', '!']);
    let stopped = text.ends_with('.');
    let mut words = last_words(text.trim_end_matches(['.', '?', '!']));
    let Some(last) = words.next() else {
        return false;
    };
    // A word that wants an argument wants it under a question mark too: Parakeet writes
    // "Can you pull up issue?" for the question form (2026-10-30 19:56, closed in 1.6 s
    // with the number still to come).
    if WANTS_ARGUMENT.contains(&last.as_str()) {
        return !words
            .take(2)
            .any(|before| DETERMINERS.contains(&before.as_str()));
    }
    if asked {
        return false;
    }
    if UNFINISHED.contains(&last.as_str()) {
        return true;
    }
    if !stopped
        && (UNFINISHED_UNLESS_STOPPED.contains(&last.as_str())
            || UNFINISHED_PREPOSITIONS.contains(&last.as_str()))
    {
        return true;
    }
    false
}

const NUMBER_WORDS: &[&str] = &[
    "zero",
    "oh",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
];

/// The words end on a number, spoken or in digits ("four one five", "issue 415."), and
/// not on a question ("Can you count to three?" is whole).
pub fn ends_with_number(text: &str) -> bool {
    let text = text.trim_end();
    if text.ends_with(['?', '!']) {
        return false;
    }
    let text = text.trim_end_matches(['.', ',']);
    last_words(text).next().is_some_and(|last| {
        !last.is_empty()
            && (last.chars().all(|c| c.is_ascii_digit() || c == '-')
                || NUMBER_WORDS.contains(&last.as_str()))
    })
}

/// The words end a sentence: a full stop, a question mark or an exclamation mark, which
/// Parakeet writes when it hears an end and leaves off when it doesn't ("Can you").
pub fn ends_a_sentence(text: &str) -> bool {
    text.trim_end().ends_with(['.', '?', '!'])
}

/// Which wait a segment's end got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// Smart Turn is sure and the words end a sentence: the short grace.
    Confident,
    /// Smart Turn says done: the grace.
    Grace,
    /// Done, and the words end on a number: the grace and a little more.
    Number,
    /// Done by the voice, but the words stop mid-thought: the long silence.
    Unfinished,
    /// Smart Turn says not done, but the words end a sentence: the shorter silence.
    Sentence,
    /// Smart Turn says not done: the long silence.
    Incomplete,
}

impl Close {
    pub fn as_str(self) -> &'static str {
        match self {
            Close::Confident => "confident",
            Close::Grace => "grace",
            Close::Number => "number",
            Close::Unfinished => "unfinished",
            Close::Sentence => "sentence",
            Close::Incomplete => "incomplete",
        }
    }
}

/// What the rule reads off a turn's words at a segment's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndWords {
    /// They end a sentence ([`ends_a_sentence`]).
    pub sentence: bool,
    /// They stop mid-thought ([`sounds_unfinished`]).
    pub unfinished: bool,
    /// They end on a number ([`ends_with_number`]).
    pub number: bool,
}

impl EndWords {
    pub fn of(words: &str) -> Self {
        Self {
            sentence: ends_a_sentence(words),
            unfinished: sounds_unfinished(words),
            number: ends_with_number(words),
        }
    }

    /// An end nobody goes on from mid-thought: a full stop, a question mark or an
    /// exclamation mark Parakeet wrote, no trailing word that wants more, and not a
    /// number, after which people pause and go on ("four one five ... from quotient").
    pub fn sure(self) -> bool {
        self.sentence && !self.unfinished && !self.number
    }
}

/// How long a turn's close waits after a segment ends, by how sure the end is (ADR 0049).
/// Every wait is counted from the VAD's stop. The strategy below runs these waits over the
/// frames; this is the rule itself, pure, so the harness can run recorded takes through it
/// (`perf::turns`) and a test can hold the strategy to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnTiming {
    /// The grace: Smart Turn says done.
    pub grace_secs: f64,
    /// The short grace, when Smart Turn is at least `confident_prob` sure and the words
    /// end a sentence. At or over `grace_secs` there is no short grace.
    pub confident_grace_secs: f64,
    pub confident_prob: f32,
    pub stop_secs: f64,
    pub unfinished_secs: f64,
    pub sentence_min_prob: f32,
    pub number_secs: f64,
}

impl TurnTiming {
    /// Smart Turn's probability alone allows the short grace.
    pub fn confident(&self, prob: f32) -> bool {
        self.grace_secs > 0.0
            && self.confident_grace_secs < self.grace_secs
            && prob >= self.confident_prob
    }

    /// The wait a segment's end gets, and its seconds from the VAD's stop.
    pub fn close(&self, verdict: TurnVerdict, words: &str) -> (Close, f64) {
        self.close_on(verdict, EndWords::of(words))
    }

    /// The same, on what was read off the words.
    pub fn close_on(&self, verdict: TurnVerdict, words: EndWords) -> (Close, f64) {
        if !verdict.complete {
            return if words.sentence
                && !words.unfinished
                && verdict.probability >= self.sentence_min_prob
            {
                (Close::Sentence, self.stop_secs)
            } else {
                (Close::Incomplete, self.unfinished_secs)
            };
        }
        if self.grace_secs <= 0.0 {
            return (Close::Grace, 0.0); // Smart Turn as is
        }
        if words.unfinished {
            return (Close::Unfinished, self.unfinished_secs.max(self.grace_secs));
        }
        if self.confident(verdict.probability) && words.sure() {
            return (Close::Confident, self.confident_grace_secs.max(0.0));
        }
        if words.number && self.number_secs > 0.0 {
            return (Close::Number, self.grace_secs + self.number_secs);
        }
        (Close::Grace, self.grace_secs)
    }
}

/// How often the hold looks for the segment's words once the short grace has run.
const WORDS_POLL: Duration = Duration::from_millis(10);

/// Smart Turn's end of turn, held for `grace_secs` before it counts.
///
/// With a short grace set (`with_confident`), a verdict Smart Turn is sure of whose words
/// end a sentence closes after that instead (ADR 0049): the words are read when they
/// arrive, the turn closes then or at the short grace, whichever is later, and anything
/// less sure waits the grace as before. The reopened turn (`TurnControl`) is what makes
/// that safe: going on before Ghost was heard takes the turn back whole.
///
/// Smart Turn ends a turn as soon as the words sound finished, and people pause in the
/// middle of a thought ("are there any other open issues I need to … look at"). Its
/// verdict waits here a moment; if the operator starts speaking again it is dropped, the
/// turn stays open, and the next words join it. `grace_secs=0` is Smart Turn as is.
///
/// When the hold ends on words that stop mid-thought ([`sounds_unfinished`]), the verdict
/// is taken for Smart Turn's "not done": the turn closes only after `unfinished_secs` of
/// silence. The words are read at the end of the hold, not at the verdict, because the
/// segment's transcript reaches this stage after its VAD stop.
///
/// Smart Turn's "not done" waits `unfinished_secs` too, unless the words that then arrive
/// end a sentence ([`ends_a_sentence`]): a full stop Parakeet wrote is a better sign than
/// the model's doubt, and the turn closes after `stop_secs` counted from the same silence.
///
/// With `speculate` the model starts answering while the close waits, so the wait costs
/// nothing when the operator is done. The answer starts when the segment's words arrive,
/// not at the verdict: the verdict comes first, and an answer started then is to the turn
/// without its last words, thrown away every time (the Rust daemon did that for a day). It
/// is held inside the LLM stage until the turn is confirmed, and thrown away if the
/// operator goes on; later words restart it. Words that stop mid-thought start none. A
/// speculative answer that reaches a tool call withdraws itself and the confirmed turn
/// runs the call, so nothing is written for a turn that hadn't ended.
///
/// The speculation answers `turn_text()`, the user aggregator's whole turn so far: the
/// analyzer's own latest transcript alone would answer a turn kept open across a pause
/// from its second half.
pub struct PatientTurnStopStrategy {
    analyzer: Option<SmartTurnAnalyzer>,
    grace: f64,
    /// The short grace and the probability it needs; at or over `grace`, none.
    confident_grace: f64,
    confident_prob: f32,
    speculate: bool,
    probe: Arc<dyn LatencyProbe>,
    inner: Arc<Mutex<StopInner>>,
    sink: Arc<dyn TurnStopSink>,
    turn_text: TurnText,
    text: Arc<Mutex<String>>,
    speaking: bool,
    shared: Speaking,
}

impl PatientTurnStopStrategy {
    pub fn new(
        analyzer: Option<SmartTurnAnalyzer>,
        grace_secs: f64,
        speculate: bool,
        sink: Arc<dyn TurnStopSink>,
    ) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let t = text.clone();
        Self {
            analyzer,
            grace: grace_secs,
            confident_grace: grace_secs,
            confident_prob: 1.0,
            speculate,
            probe: null_probe(),
            inner: Arc::new(Mutex::new(StopInner::default())),
            sink,
            turn_text: Arc::new(move || t.lock().unwrap().clone()),
            text,
            speaking: false,
            shared: Speaking::new(),
        }
    }

    pub fn with_probe(mut self, probe: Arc<dyn LatencyProbe>) -> Self {
        self.probe = probe;
        self
    }

    /// The short grace (ADR 0049): a verdict at least `prob` sure whose words end a
    /// sentence closes after `grace_secs`. At or over the grace, the grace stands alone.
    pub fn with_confident(mut self, grace_secs: f64, prob: f32) -> Self {
        self.confident_grace = grace_secs;
        self.confident_prob = prob;
        self
    }

    /// The waits this strategy runs, as the rule states them.
    pub fn timing(&self) -> TurnTiming {
        TurnTiming {
            grace_secs: self.grace,
            confident_grace_secs: self.confident_grace,
            confident_prob: self.confident_prob,
            stop_secs: self.stop_secs(),
            unfinished_secs: self.unfinished_secs(),
            sentence_min_prob: self.sentence_min_prob(),
            number_secs: self.number_secs(),
        }
    }

    /// What a speculative answer answers: the aggregator's whole turn.
    pub fn set_turn_text(&mut self, turn_text: TurnText) {
        self.turn_text = turn_text;
    }

    fn heard(&self) -> String {
        (self.turn_text)()
    }

    pub fn speculation(&self) -> Option<String> {
        self.inner.lock().unwrap().speculation.clone()
    }

    pub fn holding(&self) -> bool {
        self.inner.lock().unwrap().held.is_some()
    }

    /// Smart Turn said the operator is done: hold it for the grace. The answer starts when
    /// the segment's words arrive, a moment after this.
    pub async fn trigger_user_turn_stopped(&mut self) {
        self.trigger_stopped_with(0.0).await;
    }

    /// The same, with Smart Turn's probability that the turn is done: sure enough, and the
    /// hold may be the short one.
    pub async fn trigger_stopped_with(&mut self, prob: f32) {
        self.cancel_fallback();
        if self.grace <= 0.0 {
            self.probe.closed(Close::Grace.as_str());
            self.sink.turn_stopped().await;
            return;
        }
        self.release_hold().await;
        let short = self
            .timing()
            .confident(prob)
            .then(|| Duration::from_secs_f64(self.confident_grace.max(0.0)));
        self.inner.lock().unwrap().arrived = false;
        let inner = self.inner.clone();
        let sink = self.sink.clone();
        let probe = self.probe.clone();
        let turn_text = self.turn_text.clone();
        let grace = Duration::from_secs_f64(self.grace);
        // An unfinished end waits as long as Smart Turn's "not done" does, counted from the
        // same silence.
        let rest = Duration::from_secs_f64((self.unfinished_secs() - self.grace).max(0.0));
        let number = Duration::from_secs_f64(self.number_secs());
        let shared = self.shared.clone();
        shared.set_closing(true);
        let task = tokio::spawn(async move {
            let began = Instant::now();
            let mut band = Close::Grace;
            if let Some(short) = short {
                // Sure of the end by the voice: the words decide, when they are here. They
                // follow the verdict by the read's time, so the hold looks for them from
                // the short grace on and falls back to the grace without them.
                tokio::time::sleep(short).await;
                while began.elapsed() < grace && !inner.lock().unwrap().arrived {
                    tokio::time::sleep(WORDS_POLL).await;
                }
                let words = turn_text();
                if inner.lock().unwrap().arrived && EndWords::of(&words).sure() {
                    debug::audio(|| {
                        format!(
                            "turn: {words:?} ends a sentence and Smart Turn is sure ({prob:.2}); closing after {:.2}s",
                            began.elapsed().as_secs_f64()
                        )
                    });
                    band = Close::Confident;
                    // Closed early to speak early. What the turn does, a tool call, waits
                    // as long as it always did: the operator may yet go on.
                    sink.settles(began + grace).await;
                }
            }
            if band != Close::Confident {
                tokio::time::sleep(grace.saturating_sub(began.elapsed())).await;
                let words = turn_text();
                if sounds_unfinished(&words) {
                    debug::audio(|| format!("turn: {words:?} sounds unfinished, waiting"));
                    if inner.lock().unwrap().speculation.take().is_some() {
                        probe.speculation("unfinished");
                        sink.speculation_cancelled().await;
                    }
                    band = Close::Unfinished;
                    tokio::time::sleep(rest).await;
                } else if ends_with_number(&words) && !number.is_zero() {
                    // The answer started on the words stays; the close just waits a little.
                    debug::audio(|| {
                        format!(
                            "turn: {words:?} ends on a number; waiting {:.1}s more",
                            number.as_secs_f64()
                        )
                    });
                    band = Close::Number;
                    tokio::time::sleep(number).await;
                }
            }
            inner.lock().unwrap().held = None;
            shared.set_closing(false);
            probe.held(began.elapsed().as_secs_f64());
            probe.closed(band.as_str());
            Self::close(&inner, &sink, &probe, &turn_text).await;
        });
        self.inner.lock().unwrap().held = Some(task);
        self.speculate_on_early().await;
    }

    fn number_secs(&self) -> f64 {
        self.analyzer
            .as_ref()
            .map_or(0.0, |a| a.params().number_secs)
    }

    /// The output stage's shared flags: `closing` is raised while a close waits, so the
    /// VAD can re-arm quicker for the operator going on.
    pub fn set_speaking(&mut self, shared: Speaking) {
        self.shared = shared;
    }

    /// The wait is over: the answer started on these words is the answer, or the turn runs.
    async fn close(
        inner: &Mutex<StopInner>,
        sink: &Arc<dyn TurnStopSink>,
        probe: &Arc<dyn LatencyProbe>,
        turn_text: &TurnText,
    ) {
        let (speculation, pause) = {
            let mut i = inner.lock().unwrap();
            (i.speculation.take(), i.silence_began.take())
        };
        if let Some(began) = pause {
            probe.pause(began.elapsed().as_secs_f64(), false);
        }
        if let Some(s) = speculation {
            if s == turn_text() {
                probe.speculation("kept");
                sink.turn_finalized(true).await;
                return;
            }
            // the words changed (late, or an interceptor took them)
            probe.speculation("missed");
            sink.speculation_cancelled().await;
        }
        sink.turn_stopped().await;
    }

    /// Start answering `heard` while the close waits, unless that answer is under way.
    async fn speculate_on(&mut self, heard: String) {
        if !self.speculate || heard.trim().is_empty() {
            return;
        }
        if self.inner.lock().unwrap().speculation.as_deref() == Some(heard.as_str()) {
            return;
        }
        self.withdraw().await;
        debug::audio(|| format!("turn: answering {heard:?} while the close waits"));
        self.probe.speculation("started");
        // The start goes out before the close can know of it: a close that found the
        // answer while its start was still on the way confirmed an answer the model's
        // stage did not have yet, and the turn got none (2026-11-11, on words that arrived
        // as the hold ran out).
        self.sink.inference_triggered(Some(heard.clone())).await;
        let waiting = {
            let mut i = self.inner.lock().unwrap();
            let waiting = i.held.is_some() || i.fallback.is_some();
            if waiting {
                i.speculation = Some(heard);
            }
            waiting
        };
        if !waiting {
            // The close came meanwhile and the turn's own run is under way.
            self.probe.speculation("missed");
            self.sink.speculation_cancelled().await;
        }
    }

    /// The segment's words read at its first quiet (ADR 0054): the answer may start on
    /// them while the close waits, as it would on the stop's words, which still decide.
    /// The stop's words arriving the same keep it; different, they start it again
    /// (`words_arrived`). Nothing else is done with them.
    pub async fn early_words(&mut self, text: String) {
        // They come in the VAD's stop, before its `VadStopped`: speech that resumes in it
        // stops them at the read (`ParakeetStt::speech_resumed`), a new segment clears them.
        if !self.speculate || text.trim().is_empty() {
            return;
        }
        {
            let mut i = self.inner.lock().unwrap();
            if i.final_seen {
                return; // the stop's words came first: these are moot
            }
            i.early = Some(text.clone());
        }
        if self.holding() && !sounds_unfinished(&text) {
            self.probe.speculation("early");
            self.speculate_on(text).await;
        }
    }

    /// The hold began after early words arrived: the answer starts on them now.
    async fn speculate_on_early(&mut self) {
        let early = {
            let i = self.inner.lock().unwrap();
            if i.final_seen { None } else { i.early.clone() }
        };
        if let Some(text) = early
            && !sounds_unfinished(&text)
        {
            self.probe.speculation("early");
            self.speculate_on(text).await;
        }
    }

    /// The answer under way, if any, is thrown away.
    async fn withdraw(&mut self) {
        if self.inner.lock().unwrap().speculation.take().is_some() {
            self.probe.speculation("withdrawn");
            self.sink.speculation_cancelled().await;
        }
    }

    pub async fn release_hold(&mut self) {
        if let Some(h) = self.inner.lock().unwrap().held.take() {
            h.abort();
        }
        self.shared.set_closing(false);
        self.withdraw().await;
    }

    fn in_fallback(&self) -> bool {
        self.inner.lock().unwrap().fallback.is_some()
    }

    fn cancel_fallback(&mut self) {
        let mut i = self.inner.lock().unwrap();
        if let Some(f) = i.fallback.take() {
            f.abort();
            self.shared.set_closing(false);
        }
        i.fallback_began = None;
    }

    /// The segment's words arrived while the close waits: the answer starts on them, or
    /// starts again when they changed; words that stop mid-thought start none. For a turn
    /// Smart Turn called incomplete, words that end a sentence shorten the silence.
    async fn words_arrived(&mut self, heard: String) {
        if self.inner.lock().unwrap().speculation.as_deref() != Some(heard.as_str()) {
            self.withdraw().await; // an answer to other words
        }
        if self.holding() {
            if !sounds_unfinished(&heard) {
                self.speculate_on(heard).await;
            }
        } else if self.in_fallback() && ends_a_sentence(&heard) && !sounds_unfinished(&heard) {
            // Punctuation alone: "Can you read issue?" ends with a question mark and wants
            // its number (20:59, saved only by Smart Turn's 0.01 under the floor).
            let (began, prob) = {
                let i = self.inner.lock().unwrap();
                (i.fallback_began, i.fallback_prob)
            };
            if prob < self.sentence_min_prob() {
                debug::audio(|| {
                    format!(
                        "turn: {heard:?} ends a sentence, but Smart Turn is sure it's not done ({prob:.2}); waiting"
                    )
                });
            } else if let Some(began) = began {
                let stop = self.stop_secs();
                debug::audio(|| {
                    format!(
                        "turn: {heard:?} ends a sentence; closing {:.1}s after the silence began",
                        stop
                    )
                });
                self.run_fallback(stop, began, Close::Sentence);
            }
            self.speculate_on(heard).await;
        }
    }

    /// A frame the aggregator saw. Returns the turn verdict when one was made on this
    /// frame (a VAD stop), for the latency record.
    pub async fn process_frame(&mut self, frame: &Frame) -> Option<(TurnVerdict, f64)> {
        match frame {
            Frame::VadStarted { .. } => {
                self.speaking = true;
                {
                    let mut i = self.inner.lock().unwrap();
                    i.early = None;
                    i.final_seen = false;
                }
                let waiting = self.holding() || self.in_fallback();
                self.cancel_fallback();
                if waiting {
                    // The pause's length, for the record: what this operator's breaths are.
                    let pause = self.inner.lock().unwrap().silence_began.take();
                    if let Some(began) = pause {
                        let secs = began.elapsed().as_secs_f64();
                        self.probe.pause(secs, true);
                        debug::audio(|| format!("turn: went on after a {secs:.2}s pause"));
                    }
                }
                if self.holding() {
                    debug::audio(|| "turn: kept open, the operator went on".into());
                    self.probe.kept_open();
                    self.release_hold().await;
                }
            }
            Frame::EagerCancel => {
                // The LLM stage withdrew it: the answer reached a tool call. The confirmed
                // turn has to run its own inference, which makes the call.
                let had = self.inner.lock().unwrap().speculation.take().is_some();
                if had {
                    self.probe.speculation("tool");
                }
            }
            Frame::Transcript(text) => {
                *self.text.lock().unwrap() = text.clone();
                let early = {
                    let mut i = self.inner.lock().unwrap();
                    i.final_seen = true;
                    i.early.take()
                };
                if let Some(early) = early {
                    // How often the early words were the stop's (ADR 0054); where they
                    // differ, the answer started on them is started again below.
                    self.probe.speculation(if early == *text {
                        "early_same"
                    } else {
                        "early_differs"
                    });
                }
                if self.holding() || self.in_fallback() {
                    let heard = self.heard();
                    self.words_arrived(heard).await;
                    // Only now: a hold that closes on the words finds their answer started.
                    self.inner.lock().unwrap().arrived = true;
                }
            }
            Frame::AudioIn(audio) => {
                if let Some(a) = &mut self.analyzer {
                    a.append_audio(&audio.samples, self.speaking);
                }
            }
            Frame::VadStopped { .. } => {
                self.speaking = false;
                self.inner.lock().unwrap().silence_began = Some(Instant::now());
                let verdict = match &mut self.analyzer {
                    Some(a) => {
                        let verdict = a.analyze_end_of_turn().await;
                        if let Some((v, ms)) = &verdict {
                            debug::audio(|| {
                                format!(
                                    "turn: Smart Turn says {} ({:.2}, {ms:.0} ms)",
                                    if v.complete { "done" } else { "not done" },
                                    v.probability
                                )
                            });
                        }
                        verdict
                    }
                    None => Some((
                        TurnVerdict {
                            probability: 1.0,
                            complete: true,
                        },
                        0.0,
                    )),
                };
                match verdict {
                    Some((v, _)) if v.complete => self.trigger_stopped_with(v.probability).await,
                    Some((v, _)) => self.start_fallback(v.probability),
                    None => {}
                }
                return verdict;
            }
            _ => {}
        }
        None
    }

    /// The silence that closes a turn Smart Turn calls incomplete whose words end a sentence.
    fn stop_secs(&self) -> f64 {
        self.analyzer.as_ref().map_or(0.0, |a| a.params().stop_secs)
    }

    /// The silence that closes a turn whose words don't end a sentence.
    fn unfinished_secs(&self) -> f64 {
        self.analyzer
            .as_ref()
            .map_or(0.0, |a| a.params().unfinished_secs)
    }

    /// Smart Turn said incomplete, with `prob` that it was complete: the turn still closes
    /// after a silence, `unfinished_secs` unless the words that follow end a sentence and
    /// the model wasn't sure (`words_arrived`).
    fn start_fallback(&mut self, prob: f32) {
        self.cancel_fallback();
        self.inner.lock().unwrap().fallback_prob = prob;
        self.shared.set_closing(true);
        self.run_fallback(self.unfinished_secs(), Instant::now(), Close::Incomplete);
    }

    fn sentence_min_prob(&self) -> f32 {
        self.analyzer
            .as_ref()
            .map_or(0.0, |a| a.params().sentence_min_prob)
    }

    /// Close the turn `secs` after `began`, the answer under way being the answer.
    fn run_fallback(&mut self, secs: f64, began: Instant, band: Close) {
        if let Some(f) = self.inner.lock().unwrap().fallback.take() {
            f.abort();
        }
        let wait = Duration::from_secs_f64(secs).saturating_sub(began.elapsed());
        let inner = self.inner.clone();
        let sink = self.sink.clone();
        let probe = self.probe.clone();
        let turn_text = self.turn_text.clone();
        let shared = self.shared.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            {
                let mut i = inner.lock().unwrap();
                i.fallback = None;
                i.fallback_began = None;
            }
            shared.set_closing(false);
            probe.closed(band.as_str());
            Self::close(&inner, &sink, &probe, &turn_text).await;
        });
        let mut i = self.inner.lock().unwrap();
        i.fallback = Some(task);
        i.fallback_began = Some(began);
    }

    pub async fn handle_user_turn_started(&mut self) {
        self.release_hold().await;
        self.cancel_fallback();
    }

    pub async fn handle_user_turn_stopped(&mut self) {
        self.release_hold().await;
        self.cancel_fallback();
        *self.text.lock().unwrap() = String::new();
        if let Some(a) = &mut self.analyzer {
            a.clear();
        }
    }

    /// The session is ending; nothing is left to withdraw it from.
    pub async fn cleanup(&mut self) {
        self.inner.lock().unwrap().speculation = None;
        self.release_hold().await;
        self.cancel_fallback();
    }
}

impl Drop for PatientTurnStopStrategy {
    fn drop(&mut self) {
        let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = i.held.take() {
            h.abort();
        }
        if let Some(f) = i.fallback.take() {
            f.abort();
        }
    }
}

// --- the aggregator --------------------------------------------------------------------

/// Where the answer to the last turn the aggregator handed over is, for taking the turn
/// back when the operator goes on before Ghost answered.
#[derive(Debug, Default)]
enum Answer {
    /// Nothing to take back: Ghost was heard, or acted, or no turn is waiting.
    #[default]
    Settled,
    /// The turn's words went to the model and nothing of the answer has been heard.
    Unheard(String),
    /// Ghost's voice began answering the turn at this moment, and nothing was done for
    /// it: for a short while the operator going on is still the same turn (ADR 0049).
    Started(String, Instant),
    /// The operator went on first: the turn is theirs again, and no call of its answer runs.
    Reopened,
}

/// The current turn's token: cancelled on a barge-in, so the model's and the voice's work
/// for the reply stop and no frame of it reaches the speaker. `interrupt` makes the next.
///
/// It also keeps where the last turn's answer is, shared by the aggregator and the model's
/// stage. Smart Turn takes a pause mid-sentence for an end often enough ("can you count,
/// um"); when the operator goes on before Ghost has said or done anything, the turn they
/// were in is reopened rather than answered in two halves.
#[derive(Debug)]
pub struct TurnControl {
    session: CancellationToken,
    turn: Mutex<CancellationToken>,
    answer: Mutex<Answer>,
    /// The current turn cut Ghost's reply off. A hold asked for in it, by phrase or by the
    /// model's tool, only stops Ghost: the operator is about to speak (ADR 0021).
    barged_in: std::sync::atomic::AtomicBool,
    /// Tool calls in flight (the model's stage counts them): Ghost is working, not idle.
    calling: std::sync::atomic::AtomicUsize,
    /// Cancelled when the calls in flight should stop without the turn being cut: the
    /// operator hung up (ADR 0046). Replaced by a fresh one each time.
    calls: Mutex<CancellationToken>,
    /// Until when the last turn's tool calls wait (`settle_at`).
    settles: Mutex<Option<Instant>>,
}

impl TurnControl {
    pub fn new(session: CancellationToken) -> Arc<Self> {
        let turn = session.child_token();
        Arc::new(Self {
            session,
            turn: Mutex::new(turn),
            answer: Mutex::new(Answer::Settled),
            barged_in: std::sync::atomic::AtomicBool::new(false),
            calling: std::sync::atomic::AtomicUsize::new(0),
            calls: Mutex::new(CancellationToken::new()),
            settles: Mutex::new(None),
        })
    }

    /// The turn closed on the short grace (ADR 0049): no tool call of its answer runs
    /// before `at`, when the grace would have closed it, so going on in a pause still
    /// finds nothing done.
    pub fn settle_at(&self, at: Instant) {
        *self.settles.lock().unwrap() = Some(at);
    }

    /// When the last turn's calls may run, if that is still to come.
    pub fn settles(&self) -> Option<Instant> {
        (*self.settles.lock().unwrap()).filter(|at| *at > Instant::now())
    }

    /// The model's stage is waiting on `n` more (or fewer) tool calls.
    pub fn calls_began(&self, n: usize) {
        self.calling
            .fetch_add(n, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn calls_ended(&self, n: usize) {
        self.calling
            .fetch_sub(n, std::sync::atomic::Ordering::SeqCst);
    }

    /// A tool call is in flight: a long one is not the operator gone quiet.
    pub fn calling(&self) -> bool {
        self.calling.load(std::sync::atomic::Ordering::SeqCst) > 0
    }

    /// Cancelled when the calls in flight should stop (`stop_calls`).
    pub fn calls_token(&self) -> CancellationToken {
        self.calls.lock().unwrap().clone()
    }

    /// Stop the tool calls in flight: the operator hung up, so nobody will hear what they
    /// return. Each app is told its request was cancelled.
    pub fn stop_calls(&self) {
        let mut calls = self.calls.lock().unwrap();
        calls.cancel();
        *calls = CancellationToken::new();
    }

    /// A turn began; `barged_in` when Ghost was speaking and it cut the reply off.
    pub fn began(&self, barged_in: bool) {
        self.barged_in
            .store(barged_in, std::sync::atomic::Ordering::SeqCst);
        *self.settles.lock().unwrap() = None;
    }

    pub fn barged_in(&self) -> bool {
        self.barged_in.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The aggregator handed the model a turn: until it is heard or acts, it can be reopened.
    pub fn answering(&self, text: &str) {
        *self.answer.lock().unwrap() = Answer::Unheard(text.to_string());
    }

    /// Ghost's voice reached the operator: the turn stands. The output stage says so at
    /// the reply's first sample, by handle; the aggregator says so again on `BotStarted`,
    /// which climbs the pipeline behind the TTS stage's rendering and can arrive after
    /// the reply has played.
    pub fn heard(&self) {
        let mut a = self.answer.lock().unwrap();
        let text = match &*a {
            Answer::Unheard(text) => text.clone(),
            _ => return,
        };
        *a = Answer::Started(text, Instant::now());
    }

    /// The operator's words cut in on an answer that had only begun: their speech began
    /// (`onset`) within `window` of Ghost's first sample, and nothing was done for the
    /// turn. They were going on, not answering Ghost: the turn is taken for not heard,
    /// and its words come back. Words decide this, as they decide any barge-in; the VAD
    /// alone never stops Ghost, whose own first words reach it too.
    pub fn reopen_started(&self, onset: Instant, window: Duration) -> Option<String> {
        let mut a = self.answer.lock().unwrap();
        match &*a {
            Answer::Started(text, at) if onset <= *at + window => {
                let text = text.clone();
                *a = Answer::Reopened;
                Some(text)
            }
            _ => None,
        }
    }

    /// The answer is about to call a tool. False when the operator already took the turn
    /// back: its calls don't run. Otherwise the turn stands from here.
    pub fn acting(&self) -> bool {
        let mut a = self.answer.lock().unwrap();
        if matches!(*a, Answer::Reopened) {
            return false;
        }
        *a = Answer::Settled;
        true
    }

    /// The last turn went to the model and nothing of its answer has been heard or done.
    pub fn unheard(&self) -> bool {
        matches!(&*self.answer.lock().unwrap(), Answer::Unheard(_))
    }

    /// The operator went on: the turn's words when nothing of its answer was heard or done.
    pub fn reopen(&self) -> Option<String> {
        let mut a = self.answer.lock().unwrap();
        match std::mem::take(&mut *a) {
            Answer::Unheard(text) => {
                *a = Answer::Reopened;
                Some(text)
            }
            other => {
                *a = other;
                None
            }
        }
    }

    /// The model's stage has taken the reopened turn out of the context.
    pub fn settle_reopened(&self) {
        let mut a = self.answer.lock().unwrap();
        if matches!(*a, Answer::Reopened) {
            *a = Answer::Settled;
        }
    }

    pub fn session(&self) -> CancellationToken {
        self.session.clone()
    }

    pub fn current(&self) -> CancellationToken {
        self.turn.lock().unwrap().clone()
    }

    /// Cancel the turn in progress and start a fresh one.
    pub fn interrupt(&self) -> CancellationToken {
        let mut t = self.turn.lock().unwrap();
        t.cancel();
        *t = self.session.child_token();
        t.clone()
    }
}

#[derive(Default)]
struct Aggregation {
    pieces: Vec<String>,
    open: bool,
}

/// The stop strategy's sink in the pipeline: the aggregator's own `Out`.
struct AggregatorSink {
    out: Mutex<Option<Out>>,
    state: Arc<Mutex<Aggregation>>,
    turns: Arc<TurnControl>,
}

impl AggregatorSink {
    fn out(&self) -> Option<Out> {
        self.out.lock().unwrap().clone()
    }

    fn take_text(&self) -> String {
        let mut s = self.state.lock().unwrap();
        s.open = false;
        let text = s.pieces.join(" ");
        s.pieces.clear();
        text
    }
}

#[async_trait]
impl TurnStopSink for AggregatorSink {
    async fn turn_stopped(&self) {
        let Some(out) = self.out() else { return };
        let text = self.take_text();
        out.push_up(Frame::UserStopped).await;
        out.push(Frame::UserStopped).await;
        if text.trim().is_empty() {
            // Speech the STT made nothing of (a 0.7 s answer, 2026-10-30): no model run,
            // so no thinking sound for it, which `UserStopped` would otherwise start and
            // nothing would stop.
            debug::audio(|| "turn: closed with no words; nothing to answer".into());
            out.push(Frame::ThinkingStop).await;
            return;
        }
        self.turns.answering(&text);
        out.push(Frame::user_text(text, true)).await;
    }

    async fn inference_triggered(&self, speculation: Option<String>) {
        let Some(out) = self.out() else { return };
        if let Some(text) = speculation {
            out.push(Frame::Speculate(text)).await;
        }
    }

    async fn turn_finalized(&self, confirms_speculation: bool) {
        let Some(out) = self.out() else { return };
        let text = self.take_text();
        out.push_up(Frame::UserStopped).await;
        out.push(Frame::UserStopped).await;
        if confirms_speculation {
            self.turns.answering(&text);
            out.push(Frame::SpeculationConfirmed(text)).await;
        }
    }

    async fn speculation_cancelled(&self) {
        let Some(out) = self.out() else { return };
        out.push(Frame::SpeculationCancelled).await;
    }

    async fn settles(&self, at: Instant) {
        self.turns.settle_at(at);
    }
}

// --- the turn, read whole --------------------------------------------------------------

/// A turn longer than this is handed over in pieces, as before: the tape stops growing.
pub const TAPE_MAX_SECS: f64 = 30.0;

/// The turn's audio, for one read of the whole turn (ADR 0044). Parakeet reads a sentence
/// cut at a pause as two sentences and loses words at the cut (#52 item 1: on the
/// operator's session of 2026-11-06 the pieces scored 8.4%, the whole turn read once
/// 4.2%). So when a second or later segment's transcript arrives, the aggregator reads the
/// tape instead, from the first segment's pre-roll to the last segment's end, pauses
/// included, and that one text replaces the pieces. A single-segment turn is read once
/// already. While no turn is open the tape is a ring of `PRE_ROLL_SECS`, as the speech
/// stage keeps; it is emptied at the turn's end.
#[derive(Default)]
struct TurnTape {
    pre: VecDeque<i16>,
    audio: Vec<i16>,
    /// Samples up to the last VAD stop: a read ends there, not at the close.
    end: usize,
    open: bool,
    segments: usize,
    /// Pieces that stood before the tape's first segment (a reopened turn's words, or a
    /// segment whose audio went by before the turn opened): they stay in front.
    base: usize,
    full: bool,
    /// The turn closed and the tape runs on a while: reopened, the turn is still read
    /// whole, pause and all (`resume`).
    parked: bool,
}

/// Speech that began under an acknowledgment and is still going this long after it ended
/// is not its echo: the VAD over Ghost's voice closes a segment 0.6 s after the sound
/// stops (`GHOST_VAD_OVER_GHOST_STOP_SECS`).
const ECHO_TAIL: Duration = Duration::from_millis(750);

/// How long a closed turn's tape runs on, waiting for the turn to be reopened.
const PARK_MAX_SECS: f64 = 10.0;

impl TurnTape {
    fn push(&mut self, samples: &[i16]) {
        if self.open {
            if !self.full {
                self.audio.extend_from_slice(samples);
                if self.audio.len() > (TAPE_MAX_SECS * f64::from(SAMPLE_RATE)) as usize {
                    self.full = true;
                }
            }
            return;
        }
        self.pre.extend(samples.iter().copied());
        let keep = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
        while self.pre.len() > keep {
            self.pre.pop_front();
        }
        if self.parked {
            self.audio.extend_from_slice(samples);
            let rate = f64::from(SAMPLE_RATE);
            if self.audio.len() - self.end > (PARK_MAX_SECS * rate) as usize
                || self.audio.len() > (TAPE_MAX_SECS * rate) as usize
            {
                self.parked = false; // nobody went on: the next turn is its own
                self.audio.clear();
            }
        }
    }

    /// The turn opened: the ring becomes the tape's start.
    fn begin(&mut self) {
        self.audio = self.pre.drain(..).collect();
        self.end = 0;
        self.open = true;
        self.parked = false;
        self.segments = 0;
        self.base = 0;
        self.full = false;
    }

    /// The turn closed: the ring runs again for the next turn's pre-roll, and the tape
    /// runs on in case this one is reopened. (Left open, the next turn began with no
    /// pre-roll at all, and its whole read lost what was said before the VAD's start.)
    fn park(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.parked = !self.full && self.segments > 0;
        if !self.parked {
            self.audio.clear();
        }
    }

    /// The turn was reopened before anything of its answer was heard: the tape goes on
    /// where it was, so the turn is read whole across the pause. False, and a fresh tape,
    /// when there is none to go on with.
    fn resume(&mut self) -> bool {
        if self.parked {
            self.parked = false;
            self.open = true;
            return true;
        }
        self.begin();
        false
    }

    /// A segment started with `pieces` already in the turn.
    fn started(&mut self, pieces: usize) {
        if self.open {
            self.segments += 1;
            if self.segments == 1 {
                self.base = pieces;
            }
        }
    }

    fn stopped(&mut self) {
        if self.open {
            self.end = self.audio.len();
        }
    }

    fn clear(&mut self) {
        self.audio.clear();
        self.end = 0;
        self.open = false;
        self.parked = false;
        self.segments = 0;
        self.full = false;
    }

    /// The whole turn so far, with the trailing silence a segment gets, when reading it
    /// would say more than the pieces: a second or later segment, under the cap.
    fn whole(&self) -> Option<Vec<i16>> {
        if !self.open || self.segments < 2 || self.full || self.end == 0 {
            return None;
        }
        let mut audio = self.audio[..self.end].to_vec();
        audio.extend(std::iter::repeat_n(
            0i16,
            (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize,
        ));
        Some(audio)
    }
}

/// One read of the tape, off the runtime; None when the model heard nothing in it.
async fn read_whole(model: &SharedRecognizer, audio: Vec<i16>) -> Option<String> {
    let secs = audio.len() as f64 / f64::from(SAMPLE_RATE);
    let pcm = ghost_infer::audio::to_f32(&audio);
    let model = model.clone();
    let began = Instant::now();
    let text = tokio::task::spawn_blocking(move || recognize(&model, &pcm, true))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let took = began.elapsed().as_secs_f64();
    if text.is_empty() {
        debug::audio(|| format!("turn: read whole ({secs:.1}s) in {took:.2}s: nothing"));
        None
    } else {
        debug::audio(|| format!("turn: read whole ({secs:.1}s) in {took:.2}s: {text:?}"));
        Some(text)
    }
}

pub type OnIdle = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// Pipecat's user aggregator with Ghost's strategies: the VAD's segments, the barge-in
/// rule, the patient stop, the turn's pieces, and the model run.
pub struct UserAggregator {
    start: BargeInUserTurnStartStrategy,
    stop: PatientTurnStopStrategy,
    sink: Arc<AggregatorSink>,
    state: Arc<Mutex<Aggregation>>,
    turns: Arc<TurnControl>,
    bot_speaking: bool,
    speaking: Speaking,
    idle_timeout: Option<Duration>,
    on_idle: Option<OnIdle>,
    idle: Option<JoinHandle<()>>,
    held: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The VAD's state, and the last stop's `stop_secs`: a turn a final transcript opens
    /// after the VAD stopped (a barge-in on a final) is judged on the spot.
    vad_on: bool,
    vad_stop_secs: f32,
    /// The turn's audio, and the model that reads it whole (`with_reader`).
    tape: TurnTape,
    reader: Option<SharedRecognizer>,
    /// How long after Ghost's first sample the operator's words still reopen the turn
    /// (`with_reopen_started`); zero: once any of the answer is at the speaker it stands.
    reopen_started: Duration,
    /// When the segment that began over Ghost's voice began, by the VAD's own start time.
    over_onset: Option<Instant>,
    probe: Arc<dyn LatencyProbe>,
}

impl UserAggregator {
    /// `analyzer` None: a VAD stop alone ends the turn (tests).
    pub fn new(
        min_words: usize,
        analyzer: Option<SmartTurnAnalyzer>,
        grace_secs: f64,
        speculate: bool,
        turns: Arc<TurnControl>,
    ) -> Self {
        let state = Arc::new(Mutex::new(Aggregation::default()));
        let sink = Arc::new(AggregatorSink {
            out: Mutex::new(None),
            state: state.clone(),
            turns: turns.clone(),
        });
        let mut stop = PatientTurnStopStrategy::new(analyzer, grace_secs, speculate, sink.clone());
        let s = state.clone();
        stop.set_turn_text(Arc::new(move || s.lock().unwrap().pieces.join(" ")));
        Self {
            start: BargeInUserTurnStartStrategy::new(min_words),
            stop,
            sink,
            state,
            turns,
            bot_speaking: false,
            speaking: Speaking::new(),
            idle_timeout: None,
            on_idle: None,
            idle: None,
            held: Arc::new(|| false),
            vad_on: false,
            vad_stop_secs: 0.2,
            tape: TurnTape::default(),
            reader: None,
            reopen_started: Duration::ZERO,
            over_onset: None,
            probe: null_probe(),
        }
    }

    pub fn with_probe(mut self, probe: Arc<dyn LatencyProbe>) -> Self {
        self.probe = probe.clone();
        self.stop = self.stop.with_probe(probe);
        self
    }

    /// The short grace for an end Smart Turn is sure of (`GHOST_TURN_CONFIDENT_*`, ADR 0049).
    pub fn with_confident_close(mut self, grace_secs: f64, prob: f32) -> Self {
        self.stop = self.stop.with_confident(grace_secs, prob);
        self
    }

    /// Words from the operator whose speech began within `secs` of Ghost's first sample
    /// reopen the turn (`GHOST_TURN_REOPEN_STARTED_SECS`); 0 keeps a turn whose answer
    /// began, and those words are a barge-in as before.
    pub fn with_reopen_started(mut self, secs: f64) -> Self {
        self.reopen_started = Duration::from_secs_f64(secs.max(0.0));
        self
    }

    /// The model that reads the whole turn once a second segment ends (`TurnTape`,
    /// `GHOST_TURN_REREAD`); None hands the model the pieces as they came.
    pub fn with_reader(mut self, model: Option<SharedRecognizer>) -> Self {
        self.reader = model;
        self
    }

    /// The output stage's flag (`crate::speaking`): the barge-in rule reads it, and the
    /// idle timeout does not fire while Ghost is still talking.
    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.start.set_speaking(speaking.clone());
        self.stop.set_speaking(speaking.clone());
        self.speaking = speaking;
        self
    }

    pub fn with_idle(
        mut self,
        timeout_secs: f64,
        on_idle: OnIdle,
        held: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        if timeout_secs > 0.0 {
            self.idle_timeout = Some(Duration::from_secs_f64(timeout_secs));
            self.on_idle = Some(on_idle);
        }
        self.held = held;
        self
    }

    /// What the turn holds so far.
    pub fn aggregation(&self) -> String {
        self.state.lock().unwrap().pieces.join(" ")
    }

    pub fn turn_open(&self) -> bool {
        self.state.lock().unwrap().open
    }

    fn arm_idle(&mut self) {
        let (Some(timeout), Some(on_idle)) = (self.idle_timeout, self.on_idle.clone()) else {
            return;
        };
        if let Some(t) = self.idle.take() {
            t.abort();
        }
        let held = self.held.clone();
        let speaking = self.speaking.clone();
        let turns = self.turns.clone();
        self.idle = Some(tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // Still talking (the BotStarted that disarms this is behind the TTS stage):
            // the BotStopped that follows arms it again. Still waiting on an app (a call
            // may take minutes, ADR 0046): the reply that follows speaks, and arms it too.
            if !held() && !speaking.is_on() && !turns.calling() {
                on_idle().await;
            }
        }));
    }

    fn disarm_idle(&mut self) {
        if let Some(t) = self.idle.take() {
            t.abort();
        }
    }

    async fn turn_started(&mut self, out: &Out) {
        self.disarm_idle();
        // Going on before any of the answer was heard or done is the same turn: its words
        // come back, and the model's stage takes them and the unheard answer out of the
        // context. Talking over Ghost is a barge-in, and the turn it answered stands.
        // Over the first moment of Ghost's voice it is still the same turn: the operator
        // had not heard an answer, they were going on (ADR 0049).
        let speaking = self.start.bot_speaking();
        let over = self.over_onset.take();
        // Unheard and yet Ghost is speaking: an acknowledgment is all that was said (a
        // fast model handed the turn over, ADR 0050), and words over it go on with the
        // turn like words in silence.
        let unheard = self.turns.reopen();
        let begun = match (&unheard, over) {
            (None, Some(onset)) if speaking && !self.reopen_started.is_zero() => {
                self.turns.reopen_started(onset, self.reopen_started)
            }
            _ => None,
        };
        let after_start = begun.is_some();
        let reopened = unheard.or(begun);
        let barged_in = speaking && reopened.is_none();
        self.turns.began(barged_in);
        self.start.handle_user_turn_started();
        self.stop.handle_user_turn_started().await;
        {
            let mut s = self.state.lock().unwrap();
            s.open = true;
            if let Some(text) = &reopened {
                s.pieces = vec![text.clone()];
            }
        }
        // A turn reopened unheard is read whole across its pause; one Ghost spoke into
        // (its answer's first words, or an acknowledgment) has Ghost's voice on the tape,
        // and starts a fresh one.
        if reopened.is_some() && !speaking {
            self.tape.resume();
        } else {
            self.tape.begin();
        }
        // A new turn interrupts whatever Ghost was doing for the last one, spoken or still
        // being made (Pipecat's interruption): the reply's token is cancelled first, so the
        // voice stops within the frame and the model's stream ends, then the stages are told.
        self.turns.interrupt();
        out.push(Frame::Interruption).await;
        out.push_up(Frame::Interruption).await;
        let was_reopened = reopened.is_some();
        if let Some(text) = reopened {
            debug::audio(|| {
                if after_start {
                    format!("turn: reopened, the answer had only begun: {text:?}")
                } else {
                    format!("turn: reopened, nothing of the answer heard: {text:?}")
                }
            });
            out.push(Frame::ReopenTurn(text)).await;
        }
        self.bot_speaking = false;
        out.push_up(Frame::UserStarted).await;
        out.push(Frame::UserStarted).await;
        if was_reopened {
            self.probe.reopened(after_start); // on the turn that goes on
        }
    }

    /// Speech began while Ghost said an acknowledgment, with the turn's answer still
    /// unheard (a hand-over, ADR 0050), and it is still going well after the
    /// acknowledgment ended: the operator going on. The VAD's start was not trusted while
    /// Ghost's voice played; an echo would have ended with it.
    fn went_on_under_an_acknowledgment(&self) -> bool {
        self.vad_on
            && self.over_onset.is_some()
            && !self.turn_open()
            && !self.start.bot_speaking()
            && self.turns.unheard()
            && self.speaking.silent_for().is_some_and(|d| d >= ECHO_TAIL)
    }

    async fn apply_start_events(&mut self, out: &Out) {
        for ev in self.start.take_events() {
            match ev {
                StartEvent::Started => self.turn_started(out).await,
                StartEvent::Reset => self.state.lock().unwrap().pieces.clear(),
            }
        }
    }
}

#[async_trait]
impl Stage for UserAggregator {
    fn name(&self) -> &'static str {
        "user_aggregator"
    }

    async fn started(&mut self, out: &Out) {
        *self.sink.out.lock().unwrap() = Some(out.clone());
        self.arm_idle();
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::BotStarted => {
                self.bot_speaking = true;
                self.turns.heard();
                self.start.process_frame(&frame);
                self.disarm_idle();
            }
            Frame::BotStopped => {
                self.bot_speaking = false;
                self.start.process_frame(&frame);
                self.arm_idle();
            }
            Frame::ResetAggregation => {
                self.state.lock().unwrap().pieces.clear();
                return;
            }
            Frame::Transcript(text) => {
                // The start strategies run while no turn is open; the stop strategy while one is.
                let was_open = self.turn_open();
                if !was_open {
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                if self.turn_open() {
                    let whole = match (&self.reader, self.tape.whole()) {
                        (Some(model), Some(audio)) => read_whole(model, audio).await,
                        _ => None,
                    };
                    {
                        let mut s = self.state.lock().unwrap();
                        match whole {
                            Some(whole) => {
                                s.pieces.truncate(self.tape.base);
                                s.pieces.push(whole);
                            }
                            None => s.pieces.push(text.clone()),
                        }
                    }
                    self.stop.process_frame(&frame).await;
                    let words = text.split_whitespace().count();
                    if !was_open && !self.vad_on && (self.turns.barged_in() || words >= 3) {
                        // A turn opened by a final whose segment has already ended: its
                        // VadStopped went by while no turn was open, so the stop strategy
                        // never judged it, and no end is coming. Judge it now, on the audio
                        // it buffered. A barge-in on a final ("Go with that." cut the
                        // reading and hung open 45 s), or an utterance that began over the
                        // greeting and ended after it ("Read me issue four one five from
                        // quotient", 16 s of silence). A short tail after Ghost stopped
                        // ("What's up?", "One five.") is left to the next segment: most
                        // likely Ghost's own last words past the canceller.
                        debug::audio(|| {
                            "turn: a barge-in on a final: its end has passed, judging it now".into()
                        });
                        let stop = Frame::VadStopped {
                            stop_secs: self.vad_stop_secs,
                        };
                        match self.stop.process_frame(&stop).await {
                            Some((verdict, ms)) => {
                                out.push(Frame::Metrics(Metric::Turn {
                                    complete: verdict.complete,
                                    probability: f64::from(verdict.probability),
                                    ms,
                                }))
                                .await;
                            }
                            // Its segment was judged already, with the turn that has
                            // since closed (the read took longer than the grace: 1.5 s on
                            // a busy machine, 2026-11-11, and the turn hung until the idle
                            // timer). No audio is left to judge: the grace, then the close.
                            None => self.stop.trigger_user_turn_stopped().await,
                        }
                    }
                }
                out.forward(frame, direction).await;
                return;
            }
            Frame::Interim(_) => {
                if !self.turn_open() {
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                return; // only the barge-in rule reads an interim
            }
            Frame::EarlyTranscript(text) => {
                // A turn's first segment only: a later one is read whole at its close
                // (ADR 0044), and those words are not these.
                let first = self.state.lock().unwrap().pieces.is_empty();
                if self.turn_open() && first {
                    self.stop.early_words(text.clone()).await;
                }
                return; // never the turn's words, never stored
            }
            Frame::VadStarted { start_secs } => {
                self.vad_on = true;
                if !self.turn_open() {
                    if self.start.bot_speaking() {
                        // When this speech began: what a barge-in on its words is timed by.
                        let now = Instant::now();
                        self.over_onset = Some(
                            now.checked_sub(Duration::from_secs_f32(start_secs.max(0.0)))
                                .unwrap_or(now),
                        );
                    }
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                if self.turn_open() {
                    let pieces = self.state.lock().unwrap().pieces.len();
                    self.tape.started(pieces);
                }
                self.stop.process_frame(&frame).await;
                out.forward(frame, direction).await;
                return;
            }
            Frame::VadStopped { stop_secs } => {
                self.vad_on = false;
                self.vad_stop_secs = *stop_secs;
                self.tape.stopped();
                if self.turn_open()
                    && let Some((verdict, ms)) = self.stop.process_frame(&frame).await
                {
                    out.push(Frame::Metrics(Metric::Turn {
                        complete: verdict.complete,
                        probability: f64::from(verdict.probability),
                        ms,
                    }))
                    .await;
                }
                out.forward(frame, direction).await;
                return;
            }
            Frame::AudioIn(audio) => {
                if self.tape.open && !self.turn_open() {
                    self.tape.park(); // the turn closed since the last frame
                }
                self.tape.push(&audio.samples);
                self.stop.process_frame(&frame).await;
                if self.went_on_under_an_acknowledgment() {
                    debug::audio(|| {
                        "turn: speech that began under the acknowledgment goes on".into()
                    });
                    self.turn_started(out).await;
                    let pieces = self.state.lock().unwrap().pieces.len();
                    self.tape.started(pieces);
                }
                return; // audio ends here: the model and the voice take text
            }
            Frame::EagerCancel => {
                self.stop.process_frame(&frame).await;
                return;
            }
            Frame::UserStopped if direction == Direction::Down => {
                self.stop.handle_user_turn_stopped().await;
                self.tape.clear();
                self.arm_idle();
            }
            Frame::End | Frame::Cancel => {
                self.disarm_idle();
                self.stop.cleanup().await;
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.disarm_idle();
        self.stop.cleanup().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{Audio, drain};
    use tokio::sync::mpsc as mpsc_rx;

    // --- the start ---------------------------------------------------------------

    fn strategy(min_words: usize) -> BargeInUserTurnStartStrategy {
        BargeInUserTurnStartStrategy::new(min_words)
    }

    fn vad_start() -> Frame {
        Frame::VadStarted { start_secs: 0.2 }
    }

    fn said(text: &str) -> Frame {
        Frame::Transcript(text.into())
    }

    #[test]
    fn test_vad_starts_turn_while_bot_silent() {
        let mut s = strategy(2);
        assert_eq!(s.process_frame(&vad_start()), ProcessFrameResult::Stop);
        assert_eq!(s.take_events(), vec![StartEvent::Started]);
    }

    #[test]
    fn test_vad_ignored_while_bot_speaking() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(s.process_frame(&vad_start()), ProcessFrameResult::Continue);
        assert!(s.take_events().is_empty());
    }

    #[test]
    fn test_vad_ignored_while_the_speaker_says_ghost_talks_before_any_frame() {
        // The first clause is at the speaker; the BotStarted frame is still behind the
        // TTS stage. Ghost's own voice is VAD activity and no more; words still cut in.
        let speaking = Speaking::new();
        let mut s = strategy(2).with_speaking(speaking.clone());
        speaking.set(true);
        assert_eq!(s.process_frame(&vad_start()), ProcessFrameResult::Continue);
        assert!(s.take_events().is_empty());
        assert_eq!(
            s.process_frame(&said("stop that")),
            ProcessFrameResult::Stop
        );
        assert_eq!(s.take_events(), vec![StartEvent::Started]);
        speaking.set(false);
        assert_eq!(s.process_frame(&vad_start()), ProcessFrameResult::Stop);
    }

    #[test]
    fn test_short_fragment_while_bot_speaking_is_dropped() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(s.process_frame(&said("the")), ProcessFrameResult::Continue);
        assert_eq!(s.take_events(), vec![StartEvent::Reset]);
    }

    #[test]
    fn test_enough_words_while_bot_speaking_interrupts() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(
            s.process_frame(&said("stop that")),
            ProcessFrameResult::Stop
        );
        assert_eq!(s.take_events(), vec![StartEvent::Started]);
    }

    #[test]
    fn test_short_bursts_over_ghost_add_up_to_a_barge_in() {
        // 2026-10-30: the canceller chops the operator into one-word pieces under a
        // reading; "stop" three times over had to cut the reply.
        let mut s = strategy(3);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(s.process_frame(&said("stop")), ProcessFrameResult::Continue);
        assert_eq!(s.take_events(), vec![StartEvent::Reset]);
        assert_eq!(s.process_frame(&said("stop")), ProcessFrameResult::Continue);
        assert_eq!(s.process_frame(&said("stop")), ProcessFrameResult::Stop);
        assert_eq!(
            s.take_events(),
            vec![StartEvent::Reset, StartEvent::Started]
        );
        // What was heard over one reply does not count toward the next.
        let mut s = strategy(3);
        s.process_frame(&Frame::BotStarted);
        s.process_frame(&said("stop"));
        s.process_frame(&said("stop"));
        s.process_frame(&Frame::BotStopped);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(s.process_frame(&said("stop")), ProcessFrameResult::Continue);
        assert!(!s.take_events().contains(&StartEvent::Started));
    }

    #[test]
    fn test_two_word_interim_while_bot_speaking_interrupts() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(
            s.process_frame(&Frame::Interim("wait".into())),
            ProcessFrameResult::Continue
        );
        assert!(s.take_events().is_empty()); // nothing was aggregated to drop
        assert_eq!(
            s.process_frame(&Frame::Interim("wait stop".into())),
            ProcessFrameResult::Stop
        );
        assert_eq!(s.take_events(), vec![StartEvent::Started]);
    }

    #[test]
    fn test_zero_min_words_disables_barge_in() {
        let mut s = strategy(0);
        s.process_frame(&Frame::BotStarted);
        assert_eq!(
            s.process_frame(&said("please stop talking now")),
            ProcessFrameResult::Continue
        );
        assert_eq!(s.take_events(), vec![StartEvent::Reset]);
    }

    #[test]
    fn test_single_word_starts_turn_once_bot_stopped() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        s.process_frame(&Frame::BotStopped);
        assert_eq!(s.process_frame(&said("yes")), ProcessFrameResult::Stop);
        assert_eq!(s.take_events(), vec![StartEvent::Started]);
    }

    #[test]
    fn test_turn_start_clears_bot_speaking() {
        let mut s = strategy(2);
        s.process_frame(&Frame::BotStarted);
        s.handle_user_turn_started();
        assert!(!s.bot_speaking());
    }

    // --- the stop ------------------------------------------------------------------

    /// Records each trigger instead of reaching an aggregator.
    struct Events(Mutex<Vec<String>>);

    #[async_trait]
    impl TurnStopSink for Events {
        async fn turn_stopped(&self) {
            self.0.lock().unwrap().push("rerun".into());
        }
        async fn inference_triggered(&self, speculation: Option<String>) {
            self.0.lock().unwrap().push(match speculation {
                Some(t) => format!("answer:{t}"),
                None => "answer".into(),
            });
        }
        async fn turn_finalized(&self, confirms: bool) {
            self.0
                .lock()
                .unwrap()
                .push(if confirms { "confirmed" } else { "finalized" }.into());
        }
        async fn speculation_cancelled(&self) {
            self.0.lock().unwrap().push("withdrawn".into());
        }
    }

    fn patient(grace: f64) -> (PatientTurnStopStrategy, Arc<Events>) {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        (
            PatientTurnStopStrategy::new(None, grace, false, events.clone()),
            events,
        )
    }

    fn stops(events: &Events) -> Vec<String> {
        events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "rerun")
            .map(|_| "stopped".to_string())
            .collect()
    }

    async fn sleep(secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    #[tokio::test]
    async fn test_end_of_turn_waits_out_the_grace() {
        let (mut s, events) = patient(0.05);
        s.trigger_user_turn_stopped().await;
        assert!(stops(&events).is_empty());
        sleep(0.1).await;
        assert_eq!(stops(&events), vec!["stopped"]);
    }

    #[tokio::test]
    async fn test_speaking_again_keeps_the_turn_open() {
        let (mut s, events) = patient(0.05);
        s.trigger_user_turn_stopped().await;
        s.release_hold().await; // what VadStarted does
        sleep(0.1).await;
        assert!(stops(&events).is_empty());
    }

    // --- speculative answer while the close waits --------------------------------

    struct Speculating {
        s: PatientTurnStopStrategy,
        events: Arc<Events>,
        heard: Arc<Mutex<Vec<String>>>,
    }

    impl Speculating {
        /// A segment's words reach the strategy, as the aggregator holds them: after the
        /// VAD stop's verdict, never before it.
        async fn hears(&mut self, text: &str) {
            self.heard.lock().unwrap().push(text.to_string());
            self.s.process_frame(&said(text)).await;
        }
    }

    /// A patient strategy that speculates, nothing heard yet, with each trigger recorded
    /// instead of reaching an aggregator.
    fn speculating(grace: f64) -> Speculating {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        let s = PatientTurnStopStrategy::new(None, grace, true, events.clone());
        wired(s, events)
    }

    fn wired(mut s: PatientTurnStopStrategy, events: Arc<Events>) -> Speculating {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        s.set_turn_text(Arc::new(move || h.lock().unwrap().join(" ")));
        Speculating { s, events, heard }
    }

    fn seen(events: &Events) -> Vec<String> {
        events.0.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn test_the_answer_starts_when_the_words_arrive_and_is_kept() {
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        assert!(seen(&t.events).is_empty()); // the words aren't here yet
        t.hears("what needs my sign off").await;
        assert_eq!(seen(&t.events), vec!["answer:what needs my sign off"]);
        sleep(0.1).await;
        // Confirmed without a second inference: the held answer is the answer.
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "confirmed"]
        );
    }

    #[tokio::test]
    async fn test_the_verdict_alone_starts_no_answer() {
        // The verdict comes before the segment's words; an answer started on what was
        // heard before them is to the turn without its last words, and never the answer.
        let mut t = speculating(0.05);
        t.hears("what needs").await; // an earlier segment, no close waiting
        t.s.trigger_user_turn_stopped().await;
        sleep(0.1).await;
        assert_eq!(seen(&t.events), vec!["rerun"]);
    }

    #[tokio::test]
    async fn test_later_words_start_the_answer_again() {
        let mut t = speculating(0.1);
        t.s.trigger_user_turn_stopped().await;
        t.hears("what needs my sign off").await;
        t.hears("and the comments").await; // the words changed
        sleep(0.15).await;
        assert_eq!(
            seen(&t.events),
            vec![
                "answer:what needs my sign off",
                "withdrawn",
                "answer:what needs my sign off and the comments",
                "confirmed",
            ]
        );
    }

    #[tokio::test]
    async fn test_going_on_withdraws_the_answer_and_keeps_the_turn_open() {
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        t.hears("what needs my sign off").await;
        t.s.process_frame(&vad_start()).await;
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "withdrawn"]
        );
    }

    #[tokio::test]
    async fn test_an_answer_that_reached_a_tool_runs_again_once_confirmed() {
        // The LLM stage withdraws a speculative answer that wants a tool, so no tool runs
        // for a turn that hadn't ended; the confirmed turn makes the call.
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        t.hears("what needs my sign off").await;
        t.s.process_frame(&Frame::EagerCancel).await;
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "rerun"]
        );
    }

    #[tokio::test]
    async fn test_changed_words_discard_the_answer() {
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        t.hears("what needs my sign off").await;
        t.heard.lock().unwrap().push("and the comments".into()); // changed under it, no frame
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "withdrawn", "rerun"]
        );
    }

    // --- early words (ADR 0054) -----------------------------------------------------

    #[tokio::test]
    async fn test_early_words_start_the_answer_and_the_same_stop_words_keep_it() {
        // The early words come before the verdict: the answer starts as the hold does, and
        // the stop's words, the same, keep it. What the turn closes on is the stop's.
        let mut t = speculating(0.1);
        t.s.process_frame(&vad_start()).await;
        t.s.early_words("what needs my sign off".into()).await;
        assert!(seen(&t.events).is_empty(), "nothing before the hold");
        t.s.trigger_user_turn_stopped().await;
        assert_eq!(seen(&t.events), vec!["answer:what needs my sign off"]);
        t.hears("what needs my sign off").await;
        sleep(0.15).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "confirmed"],
            "started once, kept"
        );
    }

    #[tokio::test]
    async fn test_stop_words_that_differ_start_the_answer_again_on_themselves() {
        let mut t = speculating(0.1);
        t.s.process_frame(&vad_start()).await;
        t.s.trigger_user_turn_stopped().await;
        t.s.early_words("the latest book".into()).await; // arrives while the hold waits
        t.hears("the latest bug").await;
        sleep(0.15).await;
        assert_eq!(
            seen(&t.events),
            vec![
                "answer:the latest book",
                "withdrawn",
                "answer:the latest bug",
                "confirmed"
            ]
        );
        assert_eq!(t.s.speculation(), None);
    }

    #[tokio::test]
    async fn test_early_words_after_the_stop_words_or_speech_again_start_nothing() {
        let mut t = speculating(0.1);
        t.s.process_frame(&vad_start()).await;
        t.s.trigger_user_turn_stopped().await;
        t.hears("close it").await;
        t.s.early_words("close in".into()).await; // late: the stop's words are here
        sleep(0.15).await;
        assert_eq!(seen(&t.events), vec!["answer:close it", "confirmed"]);
        // Speech again: early words of the half segment are moot.
        let mut t = speculating(0.1);
        t.s.process_frame(&vad_start()).await;
        t.s.early_words("remind me tomorrow".into()).await;
        t.s.process_frame(&vad_start()).await;
        t.s.trigger_user_turn_stopped().await;
        assert!(seen(&t.events).is_empty(), "{:?}", seen(&t.events));
        // Words that stop mid-thought start no answer, early or not.
        let mut t = speculating(0.1);
        t.s.process_frame(&vad_start()).await;
        t.s.early_words("remind me to".into()).await;
        t.s.trigger_user_turn_stopped().await;
        assert!(seen(&t.events).is_empty(), "{:?}", seen(&t.events));
    }

    #[tokio::test]
    async fn test_no_words_no_speculation() {
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        t.hears("  ").await;
        sleep(0.1).await;
        assert_eq!(seen(&t.events), vec!["rerun"]);
    }

    #[tokio::test]
    async fn test_a_turn_that_ends_another_way_withdraws_the_answer() {
        let mut t = speculating(10.0);
        t.s.trigger_user_turn_stopped().await;
        t.hears("what needs my sign off").await;
        t.s.handle_user_turn_started().await; // a barge-in or a typed turn
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "withdrawn"]
        );
    }

    #[tokio::test]
    async fn test_a_turn_kept_open_is_answered_whole() {
        // The analyzer keeps only the latest transcript; the answer must be to the whole
        // turn the aggregator holds, or "what's waiting for triage ... in the app repo"
        // is answered as "in the app repo".
        let mut t = speculating(10.0);
        t.s.trigger_user_turn_stopped().await;
        t.hears("What's waiting for triage?").await;
        t.s.process_frame(&vad_start()).await; // the pause ends
        t.s.trigger_user_turn_stopped().await;
        t.hears("In the app repo.").await; // what the analyzer keeps
        assert_eq!(
            seen(&t.events),
            vec![
                "answer:What's waiting for triage?",
                "withdrawn",
                "answer:What's waiting for triage? In the app repo.",
            ]
        );
    }

    #[tokio::test]
    async fn test_words_an_interceptor_took_back_discard_the_answer() {
        let mut t = speculating(0.05);
        t.s.trigger_user_turn_stopped().await;
        t.hears("all right good").await;
        t.heard.lock().unwrap().clear(); // a quick intent handled the turn and reset the aggregator
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:all right good", "withdrawn", "rerun"]
        );
    }

    // --- how sure the end is (ADR 0049) ------------------------------------------

    fn timing() -> TurnTiming {
        TurnTiming {
            grace_secs: 1.2,
            confident_grace_secs: 0.35,
            confident_prob: 0.9,
            stop_secs: 1.6,
            unfinished_secs: 3.0,
            sentence_min_prob: 0.1,
            number_secs: 0.5,
        }
    }

    fn done(probability: f32) -> TurnVerdict {
        TurnVerdict {
            probability,
            complete: true,
        }
    }

    fn not_done(probability: f32) -> TurnVerdict {
        TurnVerdict {
            probability,
            complete: false,
        }
    }

    #[test]
    fn test_the_wait_for_each_kind_of_end() {
        let t = timing();
        // Sure by the voice and a sentence by the words: the short grace.
        for words in ["What time is it?", "Close it.", "Yes, go ahead."] {
            assert_eq!(
                t.close(done(0.97), words),
                (Close::Confident, 0.35),
                "{words}"
            );
        }
        assert_eq!(t.close(done(0.9), "Close it."), (Close::Confident, 0.35));
        // Middling by the voice: the grace, as before.
        assert_eq!(t.close(done(0.7), "What time is it?"), (Close::Grace, 1.2));
        assert_eq!(t.close(done(0.89), "Close it."), (Close::Grace, 1.2));
        // Sure by the voice, but Parakeet wrote no stop: the grace.
        assert_eq!(
            t.close(done(0.99), "what needs my sign off"),
            (Close::Grace, 1.2)
        );
        // A number: people pause after one and go on.
        assert_eq!(
            t.close(done(0.99), "Read me issue four one five."),
            (Close::Number, 1.7)
        );
        // Words that stop mid-thought wait as long as they did, however sure the voice.
        for words in [
            "Can you count, um",
            "Tell me about",
            "Read me issue.",
            "and then,",
            "Can you pull up issue?",
        ] {
            assert_eq!(
                t.close(done(0.99), words),
                (Close::Unfinished, 3.0),
                "{words}"
            );
        }
        // Smart Turn's "not done" waits as it did.
        assert_eq!(t.close(not_done(0.01), "Can you"), (Close::Incomplete, 3.0));
        assert_eq!(
            t.close(not_done(0.3), "I will talk to you later."),
            (Close::Sentence, 1.6)
        );
        assert_eq!(
            t.close(not_done(0.04), "Can you read me the issue."),
            (Close::Incomplete, 3.0)
        );
    }

    #[test]
    fn test_one_grace_for_every_turn_is_a_setting_away() {
        for old in [
            TurnTiming {
                confident_grace_secs: 1.2,
                ..timing()
            },
            TurnTiming {
                confident_prob: 1.1,
                ..timing()
            },
        ] {
            assert_eq!(
                old.close(done(0.99), "What time is it?"),
                (Close::Grace, 1.2)
            );
            assert_eq!(
                old.close(done(0.99), "Read me issue four one five."),
                (Close::Number, 1.7)
            );
        }
        // No grace at all is Smart Turn as is, as it was.
        let none = TurnTiming {
            grace_secs: 0.0,
            ..timing()
        };
        assert_eq!(
            none.close(done(0.99), "What time is it?"),
            (Close::Grace, 0.0)
        );
    }

    /// Smart Turn with one answer: done at or over a half.
    struct Sure(f32);

    impl TurnModel for Sure {
        fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
            Ok(TurnVerdict {
                probability: self.0,
                complete: self.0 >= 0.5,
            })
        }
    }

    /// The strategy over a Smart Turn that is `prob` sure, the waits a quarter of the
    /// shipped ones: grace 0.3 s, short grace 0.1 s at 0.9, a number +0.15 s, a doubted
    /// sentence 0.45 s, unfinished 0.75 s.
    fn banded(prob: f32) -> Speculating {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        let model: Arc<Mutex<dyn TurnModel>> = Arc::new(Mutex::new(Sure(prob)));
        let analyzer = SmartTurnAnalyzer::new(
            model,
            SmartTurnParams {
                stop_secs: 0.45,
                unfinished_secs: 0.75,
                number_secs: 0.15,
                ..SmartTurnParams::default()
            },
        );
        let s = PatientTurnStopStrategy::new(Some(analyzer), 0.3, true, events.clone())
            .with_confident(0.1, 0.9);
        wired(s, events)
    }

    /// A segment as the aggregator hands it over: the VAD's start, its audio, the stop
    /// with Smart Turn's verdict, then the words.
    async fn one_segment(t: &mut Speculating, text: &str) -> Instant {
        t.s.process_frame(&vad_start()).await;
        t.s.process_frame(&room(0.2)).await;
        t.s.process_frame(&vad_stop()).await;
        let stopped = Instant::now();
        t.hears(text).await;
        stopped
    }

    /// Seconds from `stopped` to the turn's close, by watching for it.
    async fn closed_after(t: &Speculating, stopped: Instant) -> f64 {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if seen(&t.events)
                .iter()
                .any(|e| e == "confirmed" || e == "rerun")
            {
                return stopped.elapsed().as_secs_f64();
            }
            assert!(
                Instant::now() < deadline,
                "never closed: {:?}",
                seen(&t.events)
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn test_the_strategy_waits_what_the_rule_says_for_each_end() {
        // The strategy runs the rule over the frames; each end closes when the rule says,
        // give or take the scheduler.
        for (prob, words, band) in [
            (0.97, "What time is it?", Close::Confident),
            (0.7, "What time is it?", Close::Grace),
            (0.99, "what needs my sign off", Close::Grace),
            (0.99, "Read me issue four one five.", Close::Number),
            (0.99, "Can you count, um", Close::Unfinished),
            (0.3, "I will talk to you later.", Close::Sentence),
            (0.01, "Can you", Close::Incomplete),
        ] {
            let mut t = banded(prob);
            let (expected, wait) = t.s.timing().close(
                TurnVerdict {
                    probability: prob,
                    complete: prob >= 0.5,
                },
                words,
            );
            assert_eq!(expected, band, "{words}");
            let stopped = one_segment(&mut t, words).await;
            let took = closed_after(&t, stopped).await;
            assert!(
                took >= wait - 0.01 && took < wait + 0.12,
                "{words:?} ({prob}): closed after {took:.2}s, the rule says {wait:.2}s"
            );
        }
    }

    #[tokio::test]
    async fn test_a_sure_end_keeps_the_answer_started_on_its_words() {
        let mut t = banded(0.97);
        let stopped = one_segment(&mut t, "What time is it?").await;
        closed_after(&t, stopped).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:What time is it?", "confirmed"],
            "closed on the held answer, with no second inference"
        );
    }

    #[tokio::test]
    async fn test_a_sure_end_whose_words_come_late_closes_when_they_come() {
        // A long turn's read outlasts the short grace: the close waits for the words
        // rather than taking the verdict alone.
        let mut t = banded(0.97);
        t.s.process_frame(&vad_start()).await;
        t.s.process_frame(&room(0.2)).await;
        t.s.process_frame(&vad_stop()).await;
        let stopped = Instant::now();
        sleep(0.18).await;
        assert!(seen(&t.events).is_empty(), "no words, no close");
        t.hears("What time is it?").await;
        let took = closed_after(&t, stopped).await;
        assert!((0.18..0.28).contains(&took), "closed after {took:.2}s");
    }

    /// A sink whose start of an answer takes a while to go out, as a frame does through a
    /// busy pipeline.
    struct SlowStart(Events);

    #[async_trait]
    impl TurnStopSink for SlowStart {
        async fn turn_stopped(&self) {
            self.0.turn_stopped().await;
        }
        async fn inference_triggered(&self, speculation: Option<String>) {
            tokio::time::sleep(Duration::from_millis(60)).await;
            self.0.inference_triggered(speculation).await;
        }
        async fn turn_finalized(&self, confirms: bool) {
            self.0.turn_finalized(confirms).await;
        }
        async fn speculation_cancelled(&self) {
            self.0.speculation_cancelled().await;
        }
    }

    #[tokio::test]
    async fn test_a_close_never_overtakes_the_start_of_the_answer_it_confirms() {
        // The words arrive after the short grace has run, so the hold closes the moment
        // it sees them. The answer's start must be out first, or the model's stage is
        // told to speak an answer it has not been asked for and the turn gets nothing.
        let slow = Arc::new(SlowStart(Events(Mutex::new(Vec::new()))));
        let model: Arc<Mutex<dyn TurnModel>> = Arc::new(Mutex::new(Sure(0.97)));
        let analyzer = SmartTurnAnalyzer::new(model, SmartTurnParams::default());
        let mut s = PatientTurnStopStrategy::new(Some(analyzer), 0.5, true, slow.clone())
            .with_confident(0.05, 0.9);
        let heard = Arc::new(Mutex::new(String::new()));
        let h = heard.clone();
        s.set_turn_text(Arc::new(move || h.lock().unwrap().clone()));
        s.process_frame(&vad_start()).await;
        s.process_frame(&room(0.2)).await;
        s.process_frame(&vad_stop()).await;
        sleep(0.12).await; // the short grace is over; the hold waits for the words
        *heard.lock().unwrap() = "What time is it?".into();
        s.process_frame(&said("What time is it?")).await;
        sleep(0.2).await;
        assert_eq!(
            seen(&slow.0),
            vec!["answer:What time is it?", "confirmed"],
            "the start, then the confirmation"
        );
    }

    #[tokio::test]
    async fn test_an_answer_started_as_the_hold_ran_out_is_not_left_behind() {
        // The hold ends while the answer's start is still going out: the turn runs on
        // its own, and the answer started for it is called off, not left for nobody.
        let slow = Arc::new(SlowStart(Events(Mutex::new(Vec::new()))));
        let mut s = PatientTurnStopStrategy::new(None, 0.05, true, slow.clone());
        let heard = Arc::new(Mutex::new(String::new()));
        let h = heard.clone();
        s.set_turn_text(Arc::new(move || h.lock().unwrap().clone()));
        s.trigger_user_turn_stopped().await;
        sleep(0.02).await;
        *heard.lock().unwrap() = "what needs my sign off".into();
        s.process_frame(&said("what needs my sign off")).await; // 60 ms to start: the hold ends in 30
        sleep(0.1).await;
        assert_eq!(
            seen(&slow.0),
            vec!["rerun", "answer:what needs my sign off", "withdrawn"]
        );
        assert_eq!(s.speculation(), None);
    }

    #[tokio::test]
    async fn test_a_sure_end_is_never_read_off_the_segment_before() {
        // "What's waiting for triage?" then a pause, then more: at the second segment's
        // verdict the turn's words still end a sentence, the first segment's. The short
        // grace must not close on them.
        let mut t = banded(0.97);
        t.s.trigger_stopped_with(0.97).await;
        t.hears("What's waiting for triage?").await;
        t.s.process_frame(&vad_start()).await; // the operator goes on inside the short grace
        t.s.process_frame(&room(0.2)).await;
        t.s.process_frame(&vad_stop()).await; // sure again, its words not read yet
        sleep(0.2).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:What's waiting for triage?", "withdrawn"],
            "held open: the second segment's words are not here"
        );
    }

    // --- the shared model ----------------------------------------------------------

    struct FakeModel;

    impl TurnModel for FakeModel {
        fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
            Ok(TurnVerdict {
                probability: 0.99,
                complete: true,
            })
        }
    }

    /// Smart Turn hearing no end, as on "can you" and on some goodbyes.
    struct NotDone;

    impl TurnModel for NotDone {
        fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
            Ok(TurnVerdict {
                probability: 0.01,
                complete: false,
            })
        }
    }

    /// Smart Turn leaning to "not done" without conviction, as on "Yeah." (0.15 live).
    struct Unsure;

    impl TurnModel for Unsure {
        fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
            Ok(TurnVerdict {
                probability: 0.15,
                complete: false,
            })
        }
    }

    #[test]
    fn test_smart_turn_fork_shares_the_model_but_not_the_audio() {
        let model: Arc<Mutex<dyn TurnModel>> = Arc::new(Mutex::new(FakeModel));
        let mut turn = SmartTurnAnalyzer::new(model.clone(), SmartTurnParams::default());
        turn.append_audio(&[1; 320], true); // half a turn
        assert!(turn.speech_triggered());
        let fork = turn.fork();
        assert!(Arc::ptr_eq(fork.model(), turn.model()));
        assert_eq!(fork.buffered(), 0);
        assert!(!fork.speech_triggered());
    }

    // --- the aggregator ------------------------------------------------------------

    fn aggregator(min_words: usize) -> UserAggregator {
        UserAggregator::new(
            min_words,
            None,
            0.0,
            false,
            TurnControl::new(CancellationToken::new()),
        )
    }

    // --- the turn, read whole ------------------------------------------------------

    /// A model that says `text` to any audio and keeps each read's length.
    struct WholeReader {
        text: &'static str,
        reads: Arc<Mutex<Vec<usize>>>,
    }

    impl crate::stt::Recognizer for WholeReader {
        fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
            self.reads.lock().unwrap().push(audio.len());
            Ok(self.text.to_string())
        }
    }

    fn room(secs: f64) -> Frame {
        Frame::AudioIn(Audio::new(vec![7i16; (secs * 16_000.0) as usize], 16_000))
    }

    fn vad_stop() -> Frame {
        Frame::VadStopped { stop_secs: 0.2 }
    }

    #[tokio::test]
    async fn test_a_second_segment_replaces_the_pieces_with_the_whole_turn_read_once() {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let model: SharedRecognizer = Arc::new(Mutex::new(WholeReader {
            text: "read me issue four oh three",
            reads: reads.clone(),
        }));
        // No Smart Turn: a VAD stop is the verdict, and the close waits half a second.
        let mut agg = UserAggregator::new(
            0,
            None,
            0.5,
            false,
            TurnControl::new(CancellationToken::new()),
        )
        .with_reader(Some(model));
        let (out, _rx) = Out::capture();
        agg.started(&out).await;
        agg.process(room(2.0), Direction::Down, &out).await; // the room before: one second is kept
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(room(1.0), Direction::Down, &out).await;
        agg.process(vad_stop(), Direction::Down, &out).await;
        agg.process(
            Frame::Transcript("Read me issue.".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert_eq!(agg.aggregation(), "Read me issue.");
        assert!(
            reads.lock().unwrap().is_empty(),
            "one segment was read once already"
        );
        // the operator goes on inside the grace: the same turn
        agg.process(room(0.3), Direction::Down, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(room(0.8), Direction::Down, &out).await;
        agg.process(vad_stop(), Direction::Down, &out).await;
        agg.process(
            Frame::Transcript("Four oh three.".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert_eq!(agg.aggregation(), "read me issue four oh three");
        let reads = reads.lock().unwrap().clone();
        assert_eq!(reads.len(), 1);
        // the pre-roll, both segments, the pause between them, the trailing silence
        let expected = ((1.0 + 1.0 + 0.3 + 0.8 + 0.2) * 16_000.0) as usize;
        assert_eq!(reads[0], expected);
    }

    #[tokio::test]
    async fn test_a_reopened_turn_is_read_whole_and_the_next_turn_has_its_pre_roll() {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let model: SharedRecognizer = Arc::new(Mutex::new(WholeReader {
            text: "remind me tomorrow morning to review the notes",
            reads: reads.clone(),
        }));
        let turns = TurnControl::new(CancellationToken::new());
        let mut agg =
            UserAggregator::new(0, None, 0.05, false, turns.clone()).with_reader(Some(model));
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(room(2.0), Direction::Down, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(room(1.0), Direction::Down, &out).await;
        agg.process(vad_stop(), Direction::Down, &out).await;
        agg.process(
            Frame::Transcript("Remind me tomorrow morning.".into()),
            Direction::Down,
            &out,
        )
        .await;
        sleep(0.1).await; // the close: the turn is handed over
        assert!(!agg.turn_open());
        // The operator goes on before any of the answer is heard: the same turn, and
        // its tape runs on across the pause.
        agg.process(room(0.4), Direction::Down, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(room(0.8), Direction::Down, &out).await;
        agg.process(vad_stop(), Direction::Down, &out).await;
        agg.process(
            Frame::Transcript("To review the notes.".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert_eq!(
            agg.aggregation(),
            "remind me tomorrow morning to review the notes"
        );
        let pushed = drain(&mut rx).await;
        assert!(
            pushed
                .iter()
                .any(|f| matches!(f, Frame::ReopenTurn(t) if t == "Remind me tomorrow morning.")),
            "{pushed:?}"
        );
        // the pre-roll, both segments, the pause between them, the trailing silence
        let whole = ((1.0 + 1.0 + 0.4 + 0.8 + 0.2) * 16_000.0) as usize;
        assert_eq!(*reads.lock().unwrap(), vec![whole]);

        // The answer is heard; the next turn is its own, and starts with the second
        // before it (left open, the tape gave the next turn no pre-roll at all).
        sleep(0.1).await;
        turns.heard();
        agg.process(room(2.0), Direction::Down, &out).await;
        for secs in [0.5, 0.6] {
            agg.process(vad_start(), Direction::Down, &out).await;
            agg.process(room(secs), Direction::Down, &out).await;
            agg.process(vad_stop(), Direction::Down, &out).await;
            agg.process(Frame::Transcript("And.".into()), Direction::Down, &out)
                .await;
            agg.process(room(0.01), Direction::Down, &out).await;
        }
        let next = ((1.0 + 0.5 + 0.01 + 0.6 + 0.2) * 16_000.0) as usize;
        assert_eq!(reads.lock().unwrap().last(), Some(&next));
    }

    // --- going on as Ghost's voice begins (ADR 0049) ---------------------------------

    /// A turn handed over and closed, Ghost not yet heard: the aggregator, its control,
    /// the output stage's flag, and what it pushes.
    async fn handed_over(
        window: f64,
    ) -> (
        UserAggregator,
        Arc<TurnControl>,
        Speaking,
        Out,
        mpsc_rx::Receiver<crate::frames::Pushed>,
    ) {
        let turns = TurnControl::new(CancellationToken::new());
        let speaking = Speaking::new();
        let mut agg = UserAggregator::new(2, None, 0.0, false, turns.clone())
            .with_speaking(speaking.clone())
            .with_reopen_started(window);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(said("What's waiting for triage?"), Direction::Down, &out)
            .await;
        agg.process(vad_stop(), Direction::Down, &out).await;
        let pushed = drain(&mut rx).await;
        assert!(
            pushed
                .iter()
                .any(|f| matches!(f, Frame::ContextAppend { .. })),
            "{pushed:?}"
        );
        (agg, turns, speaking, out, rx)
    }

    /// Ghost's first sample is at the speaker, as the output stage says it.
    fn ghost_begins(turns: &TurnControl, speaking: &Speaking) {
        speaking.set(true);
        turns.heard();
    }

    fn reopened_turn(pushed: &[Frame]) -> Option<String> {
        pushed.iter().find_map(|f| match f {
            Frame::ReopenTurn(t) => Some(t.clone()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn test_a_vad_start_as_ghosts_voice_begins_stops_nothing_without_words() {
        // Ghost's own first words past the canceller start the VAD too. The VAD alone
        // never stops Ghost, in the window or out of it; the words that follow decide.
        for after in [0.05, 0.3] {
            let (mut agg, turns, speaking, out, mut rx) = handed_over(0.6).await;
            ghost_begins(&turns, &speaking);
            sleep(after).await;
            agg.process(vad_start(), Direction::Down, &out).await;
            let pushed = drain(&mut rx).await;
            assert!(reopened_turn(&pushed).is_none(), "{pushed:?}");
            assert!(
                !pushed.iter().any(|f| matches!(f, Frame::Interruption)),
                "{pushed:?}"
            );
            assert!(!agg.turn_open());
        }
    }

    #[tokio::test]
    async fn test_words_soon_after_ghost_began_reopen_the_turn_and_later_ones_cut_in() {
        for (vad_after, reopens) in [(0.45, true), (1.1, false)] {
            let (mut agg, turns, speaking, out, mut rx) = handed_over(0.6).await;
            ghost_begins(&turns, &speaking);
            sleep(vad_after).await; // their speech began 0.2 s before this
            agg.process(vad_start(), Direction::Down, &out).await;
            assert!(!agg.turn_open(), "the VAD alone starts nothing over Ghost");
            agg.process(interim("in the app repo"), Direction::Down, &out)
                .await;
            let pushed = drain(&mut rx).await;
            assert!(agg.turn_open());
            assert_eq!(
                reopened_turn(&pushed).is_some(),
                reopens,
                "the VAD's start {vad_after}s after Ghost began: {pushed:?}"
            );
            assert_eq!(turns.barged_in(), !reopens);
            if reopens {
                assert!(pushed.iter().any(|f| matches!(f, Frame::Interruption)));
                assert_eq!(agg.aggregation(), "What's waiting for triage?");
                assert!(!turns.acting(), "no call of the half answer runs");
            }
        }
    }

    #[tokio::test]
    async fn test_words_over_an_acknowledgment_go_on_with_the_turn() {
        // The fast model handed the turn over and Ghost said "Sure." while the operator
        // was only pausing. Nothing of the answer was heard: their words over the
        // acknowledgment are the rest of the turn, however long after it began, and
        // whatever the window is.
        for window in [0.25, 0.0] {
            let (mut agg, turns, speaking, out, mut rx) = handed_over(window).await;
            speaking.set(true); // the acknowledgment plays; the answer is still unheard
            sleep(0.4).await;
            agg.process(vad_start(), Direction::Down, &out).await;
            assert!(!agg.turn_open(), "the VAD alone starts nothing over Ghost");
            agg.process(interim("in the app repo"), Direction::Down, &out)
                .await;
            let pushed = drain(&mut rx).await;
            assert_eq!(
                reopened_turn(&pushed).as_deref(),
                Some("What's waiting for triage?"),
                "{pushed:?}"
            );
            assert!(!turns.barged_in());
        }
    }

    #[tokio::test]
    async fn test_speech_still_going_after_an_acknowledgment_ended_takes_the_turn_back() {
        // No words yet (the interim reads stop with Ghost's voice), but the speech that
        // began under "Sure." is still going three quarters of a second after it ended.
        // An echo of the acknowledgment would have stopped with it.
        for keeps_talking in [true, false] {
            let (mut agg, turns, speaking, out, mut rx) = handed_over(0.25).await;
            speaking.set(true);
            speaking.audio(); // the acknowledgment's last chunk
            agg.process(vad_start(), Direction::Down, &out).await;
            speaking.set(false); // it is over
            sleep(0.4).await;
            agg.process(room(0.02), Direction::Down, &out).await;
            assert!(!agg.turn_open(), "too soon to tell from its echo");
            if !keeps_talking {
                agg.process(vad_stop(), Direction::Down, &out).await;
            }
            sleep(0.45).await;
            agg.process(room(0.02), Direction::Down, &out).await;
            let pushed = drain(&mut rx).await;
            assert_eq!(
                reopened_turn(&pushed).is_some(),
                keeps_talking,
                "{pushed:?}"
            );
            assert_eq!(agg.turn_open(), keeps_talking);
            assert!(!turns.barged_in());
        }
    }

    #[tokio::test]
    async fn test_with_no_window_a_turn_whose_answer_began_stands() {
        // GHOST_TURN_REOPEN_STARTED_SECS=0: as before ADR 0049.
        let (mut agg, turns, speaking, out, mut rx) = handed_over(0.0).await;
        ghost_begins(&turns, &speaking);
        sleep(0.1).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(interim("in the app repo"), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert!(reopened_turn(&pushed).is_none(), "{pushed:?}");
        assert!(turns.barged_in());
    }

    #[test]
    fn test_a_turn_whose_answer_began_is_reopened_only_for_a_moment_and_never_after_it_acted() {
        let window = Duration::from_millis(600);
        let turns = TurnControl::new(CancellationToken::new());
        turns.answering("half a");
        turns.heard();
        let began = Instant::now();
        assert_eq!(turns.reopen(), None, "heard: the plain reopen is over");
        assert_eq!(
            turns.reopen_started(began + Duration::from_secs(2), window),
            None
        );
        assert_eq!(
            turns.reopen_started(began + Duration::from_millis(300), window),
            Some("half a".to_string())
        );
        assert!(!turns.acting(), "the reopened turn's calls don't run");
        // An answer that acted stands, voice or no voice.
        let turns = TurnControl::new(CancellationToken::new());
        turns.answering("close it");
        assert!(turns.acting());
        turns.heard();
        assert_eq!(turns.reopen_started(Instant::now(), window), None);
    }

    #[tokio::test]
    async fn test_without_a_reader_the_pieces_stand() {
        let mut agg = UserAggregator::new(
            0,
            None,
            0.5,
            false,
            TurnControl::new(CancellationToken::new()),
        );
        let (out, _rx) = Out::capture();
        agg.started(&out).await;
        for text in ["Read me issue.", "Four oh three."] {
            agg.process(vad_start(), Direction::Down, &out).await;
            agg.process(room(0.5), Direction::Down, &out).await;
            agg.process(vad_stop(), Direction::Down, &out).await;
            agg.process(Frame::Transcript(text.into()), Direction::Down, &out)
                .await;
        }
        assert_eq!(agg.aggregation(), "Read me issue. Four oh three.");
    }

    #[test]
    fn test_the_tape_keeps_earlier_words_in_front_and_stops_at_the_cap() {
        let mut tape = TurnTape::default();
        tape.push(&[1; 16_000 * 3]);
        tape.begin();
        assert_eq!(tape.audio.len(), 16_000, "a pre-roll of one second");
        tape.started(1); // a reopened turn's words stand in front
        tape.push(&[1; 8_000]);
        tape.stopped();
        assert!(
            tape.whole().is_none(),
            "the first segment was read once already"
        );
        tape.started(2);
        tape.push(&[1; 8_000]);
        tape.stopped();
        assert_eq!(tape.base, 1);
        assert_eq!(tape.whole().map(|a| a.len()), Some(16_000 + 16_000 + 3_200));
        tape.push(&[1; (TAPE_MAX_SECS as usize + 1) * 16_000]);
        tape.started(2);
        tape.stopped();
        assert!(tape.whole().is_none(), "over the cap the pieces stand");
        tape.clear();
        assert!(!tape.open);
    }

    fn interim(text: &str) -> Frame {
        Frame::Interim(text.into())
    }

    #[tokio::test]
    async fn test_a_two_word_interim_while_ghost_speaks_cuts_it_off() {
        for (heard, cuts_in) in [(vec!["wait"], false), (vec!["wait", "wait stop"], true)] {
            let mut agg = aggregator(2);
            let (out, mut rx) = Out::capture();
            agg.started(&out).await;
            agg.process(Frame::BotStarted, Direction::Up, &out).await;
            agg.process(vad_start(), Direction::Down, &out).await;
            for text in &heard {
                agg.process(interim(text), Direction::Down, &out).await;
            }
            let pushed = drain(&mut rx).await;
            let started = pushed.iter().any(|f| matches!(f, Frame::UserStarted));
            assert_eq!(started, cuts_in, "{heard:?}");
            assert!(!pushed.iter().any(|f| matches!(f, Frame::Interim(_))));
            assert!(
                !pushed
                    .iter()
                    .any(|f| matches!(f, Frame::ContextAppend { .. }))
            ); // no final yet
        }
    }

    #[tokio::test]
    async fn test_a_turn_with_no_words_stops_the_thinking_sound_it_started() {
        // 2026-10-30: a 0.7 s answer Parakeet wrote nothing for closed the turn, the
        // thinking sound came on, and nothing answered for 20 s.
        let mut agg = aggregator(3);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        let stopped = pushed
            .iter()
            .position(|f| matches!(f, Frame::UserStopped))
            .expect("the turn closed");
        let quiet = pushed
            .iter()
            .position(|f| matches!(f, Frame::ThinkingStop))
            .expect("the thinking sound is told to stop");
        assert!(quiet > stopped, "{pushed:?}");
        assert!(
            !pushed
                .iter()
                .any(|f| matches!(f, Frame::ContextAppend { .. })),
            "no model run for nothing: {pushed:?}"
        );
    }

    #[tokio::test]
    async fn test_a_barge_in_on_a_final_closes_without_another_vad_stop() {
        // 2026-10-30: the VAD had stopped before the final "Go with that." opened the
        // turn; nothing judged its end, and the turn hung until the idle timer hung up.
        let mut agg = aggregator(3);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(Frame::BotStarted, Direction::Up, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await; // no turn open yet: the stop strategy is not consulted
        agg.process(said("hang on a sec now"), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert!(pushed.iter().any(|f| matches!(f, Frame::UserStarted)));
        assert!(
            pushed.iter().any(|f| matches!(f, Frame::UserStopped)),
            "the turn closed on its own: {pushed:?}"
        );

        // A short final after Ghost stopped, with no VAD of its own (a leak of its last
        // words), still waits for the next segment's end.
        let mut agg = aggregator(3);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(Frame::BotStarted, Direction::Up, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        agg.process(Frame::BotStopped, Direction::Up, &out).await;
        agg.process(said("what's up"), Direction::Down, &out).await;
        let pushed = drain(&mut rx).await;
        assert!(pushed.iter().any(|f| matches!(f, Frame::UserStarted)));
        assert!(!pushed.iter().any(|f| matches!(f, Frame::UserStopped)));

        // An utterance that began over the greeting and ended after it (2026-10-30 18:12:
        // "Read me issue four one five from quotient", 16 s of silence) closes on its own.
        let mut agg = aggregator(3);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(Frame::BotStarted, Direction::Up, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(Frame::BotStopped, Direction::Up, &out).await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        agg.process(
            said("read me issue four one five from quotient"),
            Direction::Down,
            &out,
        )
        .await;
        let pushed = drain(&mut rx).await;
        assert!(
            pushed.iter().any(|f| matches!(f, Frame::UserStopped)),
            "{pushed:?}"
        );
    }

    #[tokio::test]
    async fn test_words_that_arrive_after_their_turn_closed_are_answered_with_it() {
        // The read of the last segment took longer than the grace. The turn closed
        // without its words; they arrive with no turn open and no VAD stop to come. The
        // turn is reopened whole and must close again, or Ghost sits silent.
        struct Done;
        impl TurnModel for Done {
            fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
                Ok(TurnVerdict {
                    probability: 0.99,
                    complete: true,
                })
            }
        }
        let model: Arc<Mutex<dyn TurnModel>> = Arc::new(Mutex::new(Done));
        let analyzer = SmartTurnAnalyzer::new(model, SmartTurnParams::default());
        let mut agg = UserAggregator::new(
            2,
            Some(analyzer),
            0.05,
            false,
            TurnControl::new(CancellationToken::new()),
        );
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        for text in ["What's going on with the app", ""] {
            agg.process(vad_start(), Direction::Down, &out).await;
            agg.process(room(0.3), Direction::Down, &out).await;
            agg.process(vad_stop(), Direction::Down, &out).await;
            if !text.is_empty() {
                agg.process(said(text), Direction::Down, &out).await;
            }
        }
        sleep(0.1).await; // the grace runs out before the second segment is read
        assert!(!agg.turn_open());
        agg.process(
            said("and what have people said about it?"),
            Direction::Down,
            &out,
        )
        .await;
        sleep(0.15).await;
        let pushed = drain(&mut rx).await;
        let turns: Vec<String> = pushed
            .iter()
            .filter_map(|f| match f {
                Frame::ContextAppend { messages, .. } => match messages.first() {
                    Some(ghost_llm::Message::User(t)) => Some(t.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            turns.last().map(String::as_str),
            Some("What's going on with the app and what have people said about it?"),
            "{pushed:?}"
        );
        assert!(!agg.turn_open(), "closed again");
    }

    #[tokio::test]
    async fn test_only_the_final_becomes_context() {
        let mut agg = aggregator(2);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        agg.process(Frame::BotStarted, Direction::Up, &out).await;
        agg.process(vad_start(), Direction::Down, &out).await;
        agg.process(interim("wait stop"), Direction::Down, &out)
            .await;
        agg.process(interim("wait stop that's"), Direction::Down, &out)
            .await;
        agg.process(Frame::BotStopped, Direction::Up, &out).await;
        agg.process(
            said("wait stop that's the wrong one"),
            Direction::Down,
            &out,
        )
        .await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        let context: Vec<String> = pushed
            .iter()
            .filter_map(|f| match f {
                Frame::ContextAppend {
                    messages,
                    run_llm: true,
                } => Some(
                    messages
                        .iter()
                        .map(|m| match m {
                            ghost_llm::Message::User(text) => text.clone(),
                            _ => String::new(),
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(context, vec!["wait stop that's the wrong one".to_string()]);
    }

    // --- words that stop mid-thought ------------------------------------------------

    #[test]
    fn test_words_that_stop_mid_thought_sound_unfinished() {
        for (text, unfinished) in [
            ("Can you count um", true),
            ("Can you tell me about", true),
            ("I think that", true),
            ("You don't", true),
            ("So,", true),
            ("Let me think...", true),
            ("and then I", true),
            ("Can you count", false),
            ("Thank you.", false),
            ("Who is it for?", false),
            ("Right, I did.", false),
            ("Yes, I can.", false),
            ("", false),
            // 2026-10-30: a full stop is Parakeet's at nearly every segment's end, and a
            // word that wants a number or a name got none.
            ("Read me issue.", true),
            ("Number", true),
            ("Issue number.", true),
            ("Tell me about", true),
            ("Turn my music back on.", false),
            ("Can you tell me about", true),
            ("Close the issue.", false),
            ("What's the number?", false),
            ("What's the issue number.", false),
            ("In the app repo.", false),
            ("Open the file.", false),
        ] {
            assert_eq!(sounds_unfinished(text), unfinished, "{text:?}");
        }
    }

    #[test]
    fn test_words_that_end_on_a_number() {
        for (text, number) in [
            ("four one five", true),
            ("issue 415.", true),
            ("Read me issue four one five,", true),
            ("four one five from", false),
            ("one two three?", false),
            ("", false),
            ("about forty", true),
        ] {
            assert_eq!(ends_with_number(text), number, "{text:?}");
        }
    }

    /// A patient strategy over `model`, with the silences of an incomplete turn.
    fn with_model(
        model: impl TurnModel + 'static,
        grace: f64,
        stop: f64,
        unfinished: f64,
    ) -> Speculating {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        let model: Arc<Mutex<dyn TurnModel>> = Arc::new(Mutex::new(model));
        let analyzer = SmartTurnAnalyzer::new(
            model,
            SmartTurnParams {
                stop_secs: stop,
                unfinished_secs: unfinished,
                ..SmartTurnParams::default()
            },
        );
        let s = PatientTurnStopStrategy::new(Some(analyzer), grace, true, events.clone());
        wired(s, events)
    }

    /// One segment of speech reaches the strategy, with the model's verdict at its end.
    async fn segment_ends(s: &mut PatientTurnStopStrategy) {
        s.process_frame(&vad_start()).await;
        s.process_frame(&Frame::AudioIn(Audio::new(vec![500; 320], 16000)))
            .await;
        s.process_frame(&Frame::VadStopped { stop_secs: 0.2 }).await;
    }

    #[tokio::test]
    async fn test_an_unfinished_end_waits_as_long_as_words_that_dont_end_a_sentence() {
        let mut t = with_model(FakeModel, 0.05, 0.1, 0.3);
        segment_ends(&mut t.s).await;
        t.hears("Can you count um").await;
        sleep(0.15).await; // the grace is over, the unfinished silence isn't
        assert!(seen(&t.events).is_empty()); // and no answer was started on "um"
        sleep(0.25).await;
        assert_eq!(seen(&t.events), vec!["rerun"]);
    }

    #[tokio::test]
    async fn test_going_on_after_an_unfinished_end_keeps_the_turn_open() {
        let mut t = with_model(FakeModel, 0.05, 0.1, 0.3);
        segment_ends(&mut t.s).await;
        t.hears("Can you tell me about").await;
        sleep(0.1).await;
        t.s.process_frame(&vad_start()).await;
        sleep(0.3).await;
        assert!(stops(&t.events).is_empty());
    }

    #[tokio::test]
    async fn test_a_finished_end_is_not_held_past_the_grace() {
        let mut t = with_model(FakeModel, 0.05, 5.0, 5.0);
        segment_ends(&mut t.s).await;
        t.hears("Can you count to three?").await;
        sleep(0.15).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:Can you count to three?", "confirmed"]
        );
    }

    // --- Smart Turn says not done ---------------------------------------------------

    #[test]
    fn test_words_that_end_a_sentence() {
        for (text, ends) in [
            ("Thank you.", true),
            ("Who is it for?", true),
            ("Stop!", true),
            ("I will talk to you later. ", true),
            ("Can you", false),
            ("Do you have", false),
            ("Thank you", false),
            ("", false),
        ] {
            assert_eq!(ends_a_sentence(text), ends, "{text:?}");
        }
        // a question mark on a word that wants an argument is not an end
        for (text, unfinished) in [
            ("Can you pull up issue?", true),
            ("Can you read issue?", true),
            ("Which issue?", false),
            ("Can you read issue? Four oh three.", false),
        ] {
            assert_eq!(sounds_unfinished(text), unfinished, "{text:?}");
        }
    }

    #[tokio::test]
    async fn test_not_done_on_words_that_dont_end_a_sentence_waits_the_longer_silence() {
        let mut t = with_model(NotDone, 0.05, 0.1, 0.4);
        segment_ends(&mut t.s).await;
        t.hears("Can you").await;
        sleep(0.25).await; // past stop_secs
        assert!(seen(&t.events).is_empty()); // still open, and no answer to "Can you"
        sleep(0.3).await;
        assert_eq!(seen(&t.events), vec!["rerun"]);
    }

    #[tokio::test]
    async fn test_not_done_on_a_full_stop_closes_after_stop_secs_with_the_answer_started() {
        // "I will talk to you later." judged incomplete used to wait the whole fallback
        // and only then start the model.
        let mut t = with_model(Unsure, 0.05, 0.1, 1.0);
        segment_ends(&mut t.s).await;
        t.hears("I will talk to you later.").await;
        assert_eq!(seen(&t.events), vec!["answer:I will talk to you later."]);
        sleep(0.2).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:I will talk to you later.", "confirmed"]
        );
    }

    #[tokio::test]
    async fn test_words_that_end_on_a_number_wait_a_little_longer() {
        // "four one five [pause] from quotient": people pause after reading a number out.
        let mut t = with_model(FakeModel, 0.05, 0.1, 0.4); // number_secs stays 0.5
        segment_ends(&mut t.s).await;
        t.hears("Read me issue four one five").await;
        sleep(0.3).await; // past the grace, inside the number's extra half second
        assert_eq!(
            seen(&t.events),
            vec!["answer:Read me issue four one five"],
            "the answer started, the close waits"
        );
        sleep(0.4).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:Read me issue four one five", "confirmed"]
        );
    }

    #[tokio::test]
    async fn test_a_full_stop_does_not_override_a_sure_not_done() {
        // 2026-10-30: "Can you read me issue." (Parakeet's full stop, Smart Turn 0.04) closed
        // after the short silence while the operator was saying the number.
        let mut t = with_model(NotDone, 0.05, 0.1, 0.4);
        segment_ends(&mut t.s).await;
        t.hears("Can you read me issue.").await;
        let closed = |t: &Speculating| {
            seen(&t.events)
                .iter()
                .any(|e| e == "rerun" || e == "confirmed")
        };
        sleep(0.25).await; // past stop_secs: still open (the answer may have started)
        assert!(!closed(&t), "{:?}", seen(&t.events));
        sleep(0.3).await; // the longer silence
        assert!(closed(&t), "{:?}", seen(&t.events));
    }

    #[tokio::test]
    async fn test_going_on_during_the_longer_silence_keeps_the_turn_open() {
        let mut t = with_model(NotDone, 0.05, 0.1, 0.4);
        segment_ends(&mut t.s).await;
        t.hears("Can you").await;
        sleep(0.1).await;
        t.s.process_frame(&vad_start()).await;
        sleep(0.5).await;
        assert!(seen(&t.events).is_empty());
    }

    // --- going on before Ghost answered ---------------------------------------------

    fn user_turns(pushed: &[Frame]) -> Vec<String> {
        pushed
            .iter()
            .filter_map(|f| match f {
                Frame::ContextAppend {
                    messages,
                    run_llm: true,
                } => Some(
                    messages
                        .iter()
                        .map(|m| match m {
                            ghost_llm::Message::User(text) => text.clone(),
                            _ => String::new(),
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    }

    fn reopened(pushed: &[Frame]) -> Vec<String> {
        pushed
            .iter()
            .filter_map(|f| match f {
                Frame::ReopenTurn(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// One VAD segment with its transcript, as the aggregator sees it.
    async fn segment(agg: &mut UserAggregator, out: &Out, text: &str) {
        agg.process(vad_start(), Direction::Down, out).await;
        agg.process(said(text), Direction::Down, out).await;
        agg.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, out)
            .await;
    }

    #[tokio::test]
    async fn test_going_on_before_any_answer_is_heard_reopens_the_turn() {
        let mut agg = aggregator(2);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        segment(&mut agg, &out, "Can you count").await;
        assert_eq!(user_turns(&drain(&mut rx).await), vec!["Can you count"]);
        segment(&mut agg, &out, "to three").await; // the model is still on it
        let pushed = drain(&mut rx).await;
        assert_eq!(reopened(&pushed), vec!["Can you count"]);
        assert_eq!(user_turns(&pushed), vec!["Can you count to three"]);
    }

    #[tokio::test]
    async fn test_a_turn_whose_answer_was_heard_stands() {
        let mut agg = aggregator(2);
        let (out, mut rx) = Out::capture();
        agg.started(&out).await;
        segment(&mut agg, &out, "Can you count").await;
        agg.process(Frame::BotStarted, Direction::Up, &out).await;
        agg.process(Frame::BotStopped, Direction::Up, &out).await;
        drain(&mut rx).await;
        segment(&mut agg, &out, "to three").await;
        let pushed = drain(&mut rx).await;
        assert!(reopened(&pushed).is_empty());
        assert_eq!(user_turns(&pushed), vec!["to three"]);
    }

    #[test]
    fn test_a_reopened_turn_runs_no_call_and_an_acted_one_is_not_reopened() {
        let turns = TurnControl::new(CancellationToken::new());
        assert!(turns.reopen().is_none()); // nothing handed over yet
        turns.answering("close infra 17");
        assert_eq!(turns.reopen().as_deref(), Some("close infra 17"));
        assert!(!turns.acting()); // the half turn's call doesn't run
        turns.settle_reopened();
        assert!(turns.acting()); // a typed turn's or a note's does
        turns.answering("close infra 17");
        assert!(turns.acting());
        assert!(turns.reopen().is_none()); // the call ran: the turn stands
        turns.answering("what's waiting");
        turns.heard();
        assert!(turns.reopen().is_none());
    }
}
