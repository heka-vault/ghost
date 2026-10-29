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
//! counts, with the speculative answer started at the verdict. [`UserAggregator`] is the
//! stage that runs both over the frames, keeps the turn's pieces, and hands the model the
//! whole turn.

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

pub struct BargeInUserTurnStartStrategy {
    min_words: usize,
    bot_speaking: bool,
    speaking: Speaking,
    events: Vec<StartEvent>,
}

impl BargeInUserTurnStartStrategy {
    pub fn new(min_words: usize) -> Self {
        Self {
            min_words,
            bot_speaking: false,
            speaking: Speaking::new(),
            events: Vec::new(),
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
    }

    pub fn process_frame(&mut self, frame: &Frame) -> ProcessFrameResult {
        match frame {
            Frame::BotStarted => self.bot_speaking = true,
            Frame::BotStopped => self.bot_speaking = false,
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
        if self.min_words > 0 && words >= self.min_words {
            debug::audio(|| {
                format!(
                    "turn: barge-in on {}{text:?}",
                    if interim { "interim " } else { "" }
                )
            });
            self.events.push(StartEvent::Started);
            return ProcessFrameResult::Stop;
        }
        if interim {
            return ProcessFrameResult::Continue; // nothing was aggregated; the next may say more
        }
        // Too short to be a deliberate interruption while the bot talks: most likely the
        // bot's own voice. Drop it rather than queue it.
        debug::audio(|| format!("turn: dropped {text:?} while Ghost speaks"));
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

/// Pipecat's `SmartTurnParams`, with Ghost's `stop_secs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmartTurnParams {
    /// When the model says incomplete, the turn still closes after this much silence.
    pub stop_secs: f64,
    /// Audio before the speech started that is kept for the model.
    pub pre_speech_secs: f64,
    pub max_duration_secs: f64,
}

impl Default for SmartTurnParams {
    fn default() -> Self {
        Self {
            stop_secs: 1.6,
            pre_speech_secs: 0.0,
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
}

pub type TurnText = Arc<dyn Fn() -> String + Send + Sync>;

/// Smart Turn's end of turn, held for `grace_secs` before it counts.
///
/// Smart Turn ends a turn as soon as the words sound finished, and people pause in the
/// middle of a thought ("are there any other open issues I need to … look at"). Its
/// verdict waits here a moment; if the operator starts speaking again it is dropped, the
/// turn stays open, and the next words join it. `grace_secs=0` is Smart Turn as is.
///
/// With `speculate` the model starts answering at the verdict rather than after the hold,
/// so the hold costs nothing when the operator is done. The answer is held inside the LLM
/// stage until the hold ends and the turn is confirmed, and thrown away if the operator
/// goes on. A speculative answer that reaches a tool call withdraws itself and the
/// confirmed turn runs the call, so nothing is written for a turn that hadn't ended.
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

    /// Smart Turn said the operator is done: hold it for the grace, and start answering.
    pub async fn trigger_user_turn_stopped(&mut self) {
        self.cancel_fallback();
        if self.grace <= 0.0 {
            self.sink.turn_stopped().await;
            return;
        }
        self.release_hold().await;
        let heard = self.heard();
        if self.speculate && !heard.trim().is_empty() {
            self.inner.lock().unwrap().speculation = Some(heard.clone());
            self.probe.speculation("started");
            self.sink.inference_triggered(Some(heard)).await;
        }
        let inner = self.inner.clone();
        let sink = self.sink.clone();
        let probe = self.probe.clone();
        let turn_text = self.turn_text.clone();
        let grace = Duration::from_secs_f64(self.grace);
        let task = tokio::spawn(async move {
            let began = Instant::now();
            tokio::time::sleep(grace).await;
            let speculation = {
                let mut i = inner.lock().unwrap();
                i.held = None;
                i.speculation.take()
            };
            probe.held(began.elapsed().as_secs_f64());
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
        });
        self.inner.lock().unwrap().held = Some(task);
    }

    pub async fn release_hold(&mut self) {
        let (held, speculation) = {
            let mut i = self.inner.lock().unwrap();
            (i.held.take(), i.speculation.take())
        };
        if let Some(h) = held {
            h.abort();
        }
        if speculation.is_some() {
            self.probe.speculation("withdrawn");
            self.sink.speculation_cancelled().await;
        }
    }

    fn cancel_fallback(&mut self) {
        if let Some(f) = self.inner.lock().unwrap().fallback.take() {
            f.abort();
        }
    }

    /// A frame the aggregator saw. Returns the turn verdict when one was made on this
    /// frame (a VAD stop), for the latency record.
    pub async fn process_frame(&mut self, frame: &Frame) -> Option<(TurnVerdict, f64)> {
        match frame {
            Frame::VadStarted { .. } => {
                self.speaking = true;
                self.cancel_fallback();
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
            }
            Frame::AudioIn(audio) => {
                if let Some(a) = &mut self.analyzer {
                    a.append_audio(&audio.samples, self.speaking);
                }
            }
            Frame::VadStopped { .. } => {
                self.speaking = false;
                let verdict = match &mut self.analyzer {
                    Some(a) => a.analyze_end_of_turn().await,
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
                    Some(_) => self.start_fallback(),
                    None => {}
                }
                return verdict;
            }
            _ => {}
        }
        None
    }

    /// Smart Turn said incomplete: the turn still closes after `stop_secs` of silence.
    fn start_fallback(&mut self) {
        self.cancel_fallback();
        let stop_secs = self.analyzer.as_ref().map_or(0.0, |a| a.params().stop_secs);
        let inner = self.inner.clone();
        let sink = self.sink.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(stop_secs)).await;
            inner.lock().unwrap().fallback = None;
            sink.turn_stopped().await;
        });
        self.inner.lock().unwrap().fallback = Some(task);
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

/// The current turn's token: cancelled on a barge-in, so the model's and the voice's work
/// for the reply stop and no frame of it reaches the speaker. `interrupt` makes the next.
#[derive(Debug)]
pub struct TurnControl {
    session: CancellationToken,
    turn: Mutex<CancellationToken>,
}

impl TurnControl {
    pub fn new(session: CancellationToken) -> Arc<Self> {
        let turn = session.child_token();
        Arc::new(Self {
            session,
            turn: Mutex::new(turn),
        })
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
        if !text.trim().is_empty() {
            out.push(Frame::user_text(text, true)).await;
        }
    }

    async fn inference_triggered(&self, speculation: Option<String>) {
        let Some(out) = self.out() else { return };
        if let Some(text) = speculation {
            out.push(Frame::Speculate(text)).await;
        }
    }

    async fn turn_finalized(&self, confirms_speculation: bool) {
        let Some(out) = self.out() else { return };
        self.take_text();
        out.push_up(Frame::UserStopped).await;
        out.push(Frame::UserStopped).await;
        if confirms_speculation {
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
        self.start.handle_user_turn_started();
        self.stop.handle_user_turn_started().await;
        {
            let mut s = self.state.lock().unwrap();
            s.open = true;
        }
        // A new turn interrupts whatever Ghost was doing for the last one, spoken or still
        // being made (Pipecat's interruption): the reply's token is cancelled first, so the
        // voice stops within the frame and the model's stream ends, then the stages are told.
        self.turns.interrupt();
        out.push(Frame::Interruption).await;
        out.push_up(Frame::Interruption).await;
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
                if !self.turn_open() {
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                if self.turn_open() {
                    self.state.lock().unwrap().pieces.push(text.clone());
                    self.stop.process_frame(&frame).await;
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
                if !self.turn_open() {
                    self.start.process_frame(&frame);
                    self.apply_start_events(out).await;
                }
                self.stop.process_frame(&frame).await;
                out.forward(frame, direction).await;
                return;
            }
            Frame::VadStopped { .. } => {
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
    use crate::frames::drain;

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

    // --- speculative answer during the grace -----------------------------------------

    struct Speculating {
        s: PatientTurnStopStrategy,
        events: Arc<Events>,
        heard: Arc<Mutex<Vec<String>>>,
    }

    /// A patient strategy that speculates, with the turn's text already heard and each
    /// trigger recorded instead of reaching an aggregator.
    fn speculating(grace: f64, text: &str) -> Speculating {
        let events = Arc::new(Events(Mutex::new(Vec::new())));
        let mut s = PatientTurnStopStrategy::new(None, grace, true, events.clone());
        let heard = Arc::new(Mutex::new(vec![text.to_string()]));
        let h = heard.clone();
        s.set_turn_text(Arc::new(move || h.lock().unwrap().join(" ")));
        Speculating { s, events, heard }
    }

    fn seen(events: &Events) -> Vec<String> {
        events.0.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn test_the_answer_starts_at_the_verdict_and_is_kept() {
        let mut t = speculating(0.05, "what needs my sign off");
        t.s.trigger_user_turn_stopped().await;
        assert_eq!(seen(&t.events), vec!["answer:what needs my sign off"]);
        sleep(0.1).await;
        // Confirmed without a second inference: the held answer is the answer.
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "confirmed"]
        );
    }

    #[tokio::test]
    async fn test_going_on_withdraws_the_answer_and_keeps_the_turn_open() {
        let mut t = speculating(0.05, "what needs my sign off");
        t.s.trigger_user_turn_stopped().await;
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
        let mut t = speculating(0.05, "what needs my sign off");
        t.s.trigger_user_turn_stopped().await;
        t.s.process_frame(&Frame::EagerCancel).await;
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "rerun"]
        );
    }

    #[tokio::test]
    async fn test_changed_words_discard_the_answer() {
        let mut t = speculating(0.05, "what needs my sign off");
        t.s.trigger_user_turn_stopped().await;
        t.heard.lock().unwrap().push("and the comments".into()); // a late transcript
        sleep(0.1).await;
        assert_eq!(
            seen(&t.events),
            vec!["answer:what needs my sign off", "withdrawn", "rerun"]
        );
    }

    #[tokio::test]
    async fn test_no_words_no_speculation() {
        let mut t = speculating(0.05, "  ");
        t.s.trigger_user_turn_stopped().await;
        sleep(0.1).await;
        assert_eq!(seen(&t.events), vec!["rerun"]);
    }

    #[tokio::test]
    async fn test_a_turn_that_ends_another_way_withdraws_the_answer() {
        let mut t = speculating(10.0, "what needs my sign off");
        t.s.trigger_user_turn_stopped().await;
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
        let mut t = speculating(10.0, "What's waiting for triage?");
        t.s.process_frame(&said("What's waiting for triage?")).await;
        t.s.trigger_user_turn_stopped().await;
        t.s.process_frame(&vad_start()).await; // the pause ends
        t.heard.lock().unwrap().push("In the app repo.".into());
        t.s.process_frame(&said("In the app repo.")).await; // what the analyzer keeps
        t.s.trigger_user_turn_stopped().await;
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
        let mut t = speculating(0.05, "all right good");
        t.s.trigger_user_turn_stopped().await;
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
}
