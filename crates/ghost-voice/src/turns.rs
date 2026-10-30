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
//! words stop mid-thought. [`UserAggregator`] is the stage that runs both over the frames,
//! keeps the turn's pieces, and hands the model the whole turn; when the operator goes on
//! before any of the answer was heard or done, it reopens that turn ([`TurnControl`]).

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
/// the words decide; a question or exclamation mark is an end. Smart Turn hears the voice;
/// this reads the words, so "can you count, um" isn't an end however it sounded.
pub fn sounds_unfinished(text: &str) -> bool {
    let text = text.trim_end();
    if text.ends_with("...") || text.ends_with('…') || text.ends_with(',') {
        return true;
    }
    if text.ends_with(['?', '!']) {
        return false;
    }
    let stopped = text.ends_with('.');
    let mut words = last_words(text.trim_end_matches('.'));
    let Some(last) = words.next() else {
        return false;
    };
    if UNFINISHED.contains(&last.as_str()) {
        return true;
    }
    if !stopped
        && (UNFINISHED_UNLESS_STOPPED.contains(&last.as_str())
            || UNFINISHED_PREPOSITIONS.contains(&last.as_str()))
    {
        return true;
    }
    if WANTS_ARGUMENT.contains(&last.as_str()) {
        return !words
            .take(2)
            .any(|before| DETERMINERS.contains(&before.as_str()));
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

/// Smart Turn's end of turn, held for `grace_secs` before it counts.
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
        self.cancel_fallback();
        if self.grace <= 0.0 {
            self.sink.turn_stopped().await;
            return;
        }
        self.release_hold().await;
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
            tokio::time::sleep(grace).await;
            let words = turn_text();
            if sounds_unfinished(&words) {
                debug::audio(|| format!("turn: {words:?} sounds unfinished, waiting"));
                if inner.lock().unwrap().speculation.take().is_some() {
                    probe.speculation("unfinished");
                    sink.speculation_cancelled().await;
                }
                tokio::time::sleep(rest).await;
            } else if ends_with_number(&words) && !number.is_zero() {
                // The answer started on the words stays; the close just waits a little.
                debug::audio(|| {
                    format!(
                        "turn: {words:?} ends on a number; waiting {:.1}s more",
                        number.as_secs_f64()
                    )
                });
                tokio::time::sleep(number).await;
            }
            inner.lock().unwrap().held = None;
            shared.set_closing(false);
            probe.held(began.elapsed().as_secs_f64());
            Self::close(&inner, &sink, &probe, &turn_text).await;
        });
        self.inner.lock().unwrap().held = Some(task);
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
        self.inner.lock().unwrap().speculation = Some(heard.clone());
        self.probe.speculation("started");
        self.sink.inference_triggered(Some(heard)).await;
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
        } else if self.in_fallback() && ends_a_sentence(&heard) {
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
                self.run_fallback(stop, began);
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
                if self.holding() || self.in_fallback() {
                    let heard = self.heard();
                    self.words_arrived(heard).await;
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
                    Some((v, _)) if v.complete => self.trigger_user_turn_stopped().await,
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
        self.run_fallback(self.unfinished_secs(), Instant::now());
    }

    fn sentence_min_prob(&self) -> f32 {
        self.analyzer
            .as_ref()
            .map_or(0.0, |a| a.params().sentence_min_prob)
    }

    /// Close the turn `secs` after `began`, the answer under way being the answer.
    fn run_fallback(&mut self, secs: f64, began: Instant) {
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
}

impl TurnControl {
    pub fn new(session: CancellationToken) -> Arc<Self> {
        let turn = session.child_token();
        Arc::new(Self {
            session,
            turn: Mutex::new(turn),
            answer: Mutex::new(Answer::Settled),
            barged_in: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// A turn began; `barged_in` when Ghost was speaking and it cut the reply off.
    pub fn began(&self, barged_in: bool) {
        self.barged_in
            .store(barged_in, std::sync::atomic::Ordering::SeqCst);
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
        if matches!(*a, Answer::Unheard(_)) {
            *a = Answer::Settled;
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
            out.push(Frame::SpeculationConfirmed).await;
        }
    }

    async fn speculation_cancelled(&self) {
        let Some(out) = self.out() else { return };
        out.push(Frame::SpeculationCancelled).await;
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
        }
    }

    pub fn with_probe(mut self, probe: Arc<dyn LatencyProbe>) -> Self {
        self.stop = self.stop.with_probe(probe);
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
        self.idle = Some(tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // Still talking (the BotStarted that disarms this is behind the TTS stage):
            // the BotStopped that follows arms it again.
            if !held() && !speaking.is_on() {
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
        let barged_in = self.start.bot_speaking();
        self.turns.began(barged_in);
        let reopened = if barged_in { None } else { self.turns.reopen() };
        self.start.handle_user_turn_started();
        self.stop.handle_user_turn_started().await;
        {
            let mut s = self.state.lock().unwrap();
            s.open = true;
            if let Some(text) = &reopened {
                s.pieces = vec![text.clone()];
            }
        }
        // A new turn interrupts whatever Ghost was doing for the last one, spoken or still
        // being made (Pipecat's interruption): the reply's token is cancelled first, so the
        // voice stops within the frame and the model's stream ends, then the stages are told.
        self.turns.interrupt();
        out.push(Frame::Interruption).await;
        out.push_up(Frame::Interruption).await;
        if let Some(text) = reopened {
            debug::audio(|| format!("turn: reopened, nothing of the answer heard: {text:?}"));
            out.push(Frame::ReopenTurn(text)).await;
        }
        self.bot_speaking = false;
        out.push_up(Frame::UserStarted).await;
        out.push(Frame::UserStarted).await;
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
                    self.state.lock().unwrap().pieces.push(text.clone());
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
                        if let Some((verdict, ms)) = self.stop.process_frame(&stop).await {
                            out.push(Frame::Metrics(Metric::Turn {
                                complete: verdict.complete,
                                probability: f64::from(verdict.probability),
                                ms,
                            }))
                            .await;
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
            Frame::VadStarted { .. } => {
                self.vad_on = true;
                if !self.turn_open() {
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                self.stop.process_frame(&frame).await;
                out.forward(frame, direction).await;
                return;
            }
            Frame::VadStopped { stop_secs } => {
                self.vad_on = false;
                self.vad_stop_secs = *stop_secs;
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
            Frame::AudioIn(_) => {
                self.stop.process_frame(&frame).await;
                return; // audio ends here: the model and the voice take text
            }
            Frame::EagerCancel => {
                self.stop.process_frame(&frame).await;
                return;
            }
            Frame::UserStopped if direction == Direction::Down => {
                self.stop.handle_user_turn_stopped().await;
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
