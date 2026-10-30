//! Build one conversation session: the stages in the Python order, one task each, over
//! bounded inboxes, with the session's token and the turn's.
//!
//! [`Models`] is loaded once by the daemon and shared across sessions (about 3 GB):
//! Parakeet, Kokoro and Smart Turn behind mutexes, each session's stage over them.
//! [`build_session`] assembles a [`GhostSession`]; [`GhostSession::start`] runs it,
//! [`GhostSession::open`] greets or runs the first turn, [`GhostSession::end`] queues the
//! end behind whatever is being said, [`GhostSession::cancel`] stops now.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_core::conversations::{Conversations, GHOST};
use ghost_core::source::Slot;
use ghost_core::{Events, ToolSource, WriteGate};
use ghost_infer::wake::DEFAULT_WAKE_PHRASES;
use ghost_infer::{Kokoro, Parakeet, Silero, SmartTurn, VadAnalyzer};
use ghost_llm::{Context, Options};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::acks::{AckStage, Acks, Outside};
use crate::config::VoiceSettings;
use crate::confirmation::{ConfirmationProcessor, last_assistant_text};
use crate::conversation::{ConversationRuntime, OnSwitch, Reach};
use crate::echo::EchoFilter;
use crate::elevenlabs::session_tts;
use crate::frames::{Clock, Direction, Frame, INBOX, Observer, Out, Pushed, Stage, assemble};
use crate::hold::{HoldGate, HoldState, OnResume};
use crate::intents::{QuickIntents, pick};
use crate::intercept::{InterceptorProcessor, SessionAction};
use crate::latency::LatencyObserver;
use crate::llm::{Compaction, LlmStage, ModelPort, SharedContext};
use crate::narration::NarrationCut;
use crate::prompt::{Persona, build_system_prompt};
use crate::speaking::Speaking;
use crate::stt::{ParakeetStt, SharedRecognizer};
use crate::tap::{AssistantTap, OnPhase, OnTurn, SessionPhase, UserTap, no_phase, no_turn};
use crate::thinking::{ThinkingSoundCut, ThinkingSoundProcessor};
use crate::tools::{ToolDispatcher, ToolsParams};
use crate::transport::{AudioInput, AudioOutput, Media};
use crate::tts::{SharedSynth, TtsSettings};
use crate::turns::{SmartTurnAnalyzer, SmartTurnParams, TurnControl, TurnModel, UserAggregator};

pub const TTS_SAMPLE_RATE: u32 = 24_000;
pub const GREETING_VARIED: &str = "<varied>";

/// An agent called directly (ADR 0014; the phone can start in one) answers as itself.
pub const THREAD_GREETINGS: &[&str] = &[
    "{name} here.",
    "{name}. Go ahead.",
    "{name}, what do you need?",
];
pub const GREETINGS: &[&str] = &[
    "Ghost. What do you need?",
    "Yeah?",
    "I'm here.",
    "What's up?",
    "Go ahead.",
    "Listening.",
    "Hey. What can I do?",
    "Ghost here.",
];
/// Spoken when the operator comes back from hold with only Ghost's name ("Ghost?").
pub const BACK_LINES: &[&str] = &["I'm here.", "Go ahead.", "Yes?"];

// --- the models ------------------------------------------------------------------------

/// Loaded once by the daemon and shared across sessions. Each session builds its own
/// stages over them; the mutexes serialize inference.
pub struct Models {
    pub stt: SharedRecognizer,
    pub tts: SharedSynth,
    pub turn: Arc<Mutex<dyn TurnModel>>,
    /// Silero is small: loaded per session from here.
    pub silero: PathBuf,
    pub voices: Vec<String>,
    /// The acknowledgments, rendered per voice from `tts` (`crate::acks`).
    pub acks: Arc<Acks>,
}

impl Models {
    pub fn load(s: &VoiceSettings) -> anyhow::Result<Self> {
        let paths = s.parakeet_paths().ok_or_else(|| {
            anyhow::anyhow!("Parakeet's files are not in the Hugging Face cache; set GHOST_STT_DIR")
        })?;
        info!(
            "Loading {} ({})",
            s.stt_model,
            s.stt_quantization.as_deref().unwrap_or("fp32")
        );
        let parakeet = Parakeet::load(&paths, s.stt_threads)?;
        let kokoro = Kokoro::load(&s.kokoro_paths())?;
        let voices: Vec<String> = kokoro.voices().into_iter().map(String::from).collect();
        let turn = SmartTurn::load(&s.smart_turn_model, s.smart_turn_threads)?;
        Ok(Self {
            stt: Arc::new(Mutex::new(parakeet)),
            tts: Arc::new(Mutex::new(kokoro)),
            turn: Arc::new(Mutex::new(turn)),
            silero: s.silero_model.clone(),
            voices,
            acks: Acks::new(Some(&s.cache_dir)),
        })
    }

    /// A session's VAD over Silero, or None when the file isn't there (the harness's
    /// tests feed VAD frames themselves).
    pub fn vad(&self, s: &VoiceSettings) -> Option<VadAnalyzer> {
        match Silero::load(&self.silero) {
            Ok(m) => Some(VadAnalyzer::new(m, s.vad_params())),
            Err(e) => {
                warn!("Silero: {e}; no VAD in this session");
                None
            }
        }
    }
}

// --- the session's control ---------------------------------------------------------------

/// What the stages and the daemon do to a running session: inject frames at the source,
/// hang up, hold and resume.
pub struct SessionControl {
    pub session_id: String,
    source: mpsc::Sender<Pushed>,
    pub token: CancellationToken,
    pub turns: Arc<TurnControl>,
    pub hold: Arc<HoldState>,
    hold_timer: Mutex<Option<JoinHandle<()>>>,
    hold_timeout: Duration,
    on_phase: OnPhase,
    on_turn: OnTurn,
    on_hang_up: Option<SessionAction>,
    ended: AtomicBool,
}

impl SessionControl {
    pub async fn inject(&self, frames: Vec<Frame>) {
        for frame in frames {
            let _ = self
                .source
                .send(Pushed {
                    frame,
                    direction: Direction::Down,
                })
                .await;
        }
    }

    /// Queue the end behind whatever is queued; the session ends once that has played.
    pub async fn end(&self) {
        if !self.ended.swap(true, Ordering::SeqCst) {
            self.stop_hold_timer();
            self.inject(vec![Frame::End]).await;
        }
    }

    /// Stop now, whatever is queued.
    pub fn cancel(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.stop_hold_timer();
        self.token.cancel();
    }

    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// The model called hang_up: its goodbye in this response is spoken, then the session
    /// ends. The shortcut (`hang_up_now`) has queued its own goodbye already; a hold that
    /// ran out or the idle timeout hangs up with nothing said.
    pub async fn hang_up(&self, keep_current: bool) {
        info!("hang_up");
        self.stop_hold_timer();
        self.inject(vec![Frame::HangUp { keep_current }]).await;
        if let Some(h) = &self.on_hang_up {
            h().await;
        }
        self.end().await;
    }

    pub async fn hang_up_now(&self) {
        self.hang_up(false).await;
    }

    fn stop_hold_timer(&self) {
        if let Some(t) = self.hold_timer.lock().unwrap().take() {
            t.abort();
        }
    }

    /// Ghost stops listening until addressed by name; the context, the thread and a
    /// pending question keep. Hangs up by itself after `hold_timeout`.
    pub async fn hold(self: &Arc<Self>) {
        if self.hold.on_hold() {
            return;
        }
        info!("hold");
        self.hold.hold();
        self.inject(vec![Frame::ThinkingStop]).await; // no loop while waiting
        self.stop_hold_timer();
        let me = self.clone();
        let timeout = self.hold_timeout;
        *self.hold_timer.lock().unwrap() = Some(tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            info!(
                "hold: nothing for {:.0}s; hanging up quietly",
                timeout.as_secs_f64()
            );
            me.hang_up(false).await;
        }));
        (self.on_phase)(SessionPhase::Hold).await;
    }

    /// Back to listening; nothing said yet.
    pub async fn unhold(&self) {
        self.stop_hold_timer();
        if !self.hold.on_hold() {
            return;
        }
        self.hold.release();
        info!("hold: resumed");
        (self.on_phase)(SessionPhase::Listening).await;
    }

    /// Addressed by name again: back to listening, with the rest of what was said as the
    /// next turn, or a word that Ghost is here when the name was all.
    pub async fn resume(&self, remainder: String) {
        self.hold.hold(); // the gate cleared itself; unhold expects to do it
        self.unhold().await;
        if !remainder.trim().is_empty() {
            (self.on_turn)("user", remainder.clone()).await;
            self.inject(vec![
                Frame::ThinkingStart,
                Frame::user_text(remainder, true),
            ])
            .await;
        } else {
            self.inject(vec![Frame::Speak(pick(BACK_LINES))]).await;
        }
    }
}

// --- the runner ------------------------------------------------------------------------

/// The stages, their inboxes, and the tasks that run them.
pub struct Pipeline {
    stages: Vec<Box<dyn Stage>>,
    inboxes: Vec<mpsc::Receiver<Pushed>>,
    senders: Vec<mpsc::Sender<Pushed>>,
    end_tx: mpsc::Sender<Pushed>,
    end_rx: mpsc::Receiver<Pushed>,
    clock: Clock,
    observer: Option<Arc<dyn Observer>>,
}

impl Pipeline {
    pub fn new(stages: Vec<Box<dyn Stage>>, observer: Option<Arc<dyn Observer>>) -> Self {
        let mut inboxes = Vec::new();
        let mut senders = Vec::new();
        for _ in &stages {
            let (tx, rx) = mpsc::channel(INBOX);
            senders.push(tx);
            inboxes.push(rx);
        }
        let (end_tx, end_rx) = mpsc::channel(INBOX);
        Self {
            stages,
            inboxes,
            senders,
            end_tx,
            end_rx,
            clock: Clock::new(),
            observer,
        }
    }

    pub fn source(&self) -> mpsc::Sender<Pushed> {
        self.senders[0].clone()
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Run every stage until the end reaches the last one, or the token is cancelled.
    pub async fn run(self, token: CancellationToken) {
        let n = self.stages.len();
        let mut tasks = Vec::new();
        let (drop_tx, mut drop_rx) = mpsc::channel::<Pushed>(INBOX);
        let drain_up = tokio::spawn(async move { while drop_rx.recv().await.is_some() {} });
        let senders = self.senders;
        let mut inboxes = self.inboxes;
        for (i, mut stage) in self.stages.into_iter().enumerate() {
            let down = if i + 1 < n {
                senders[i + 1].clone()
            } else {
                self.end_tx.clone()
            };
            let up = if i > 0 {
                senders[i - 1].clone()
            } else {
                drop_tx.clone()
            };
            let out = Out::new(
                down,
                up,
                stage.name(),
                self.clock.clone(),
                self.observer.clone(),
            );
            let mut rx = std::mem::replace(&mut inboxes[i], mpsc::channel(1).1);
            let token = token.clone();
            tasks.push(tokio::spawn(async move {
                stage.started(&out).await;
                loop {
                    let next = tokio::select! {
                        biased;
                        _ = token.cancelled() => None,
                        p = rx.recv() => p,
                    };
                    let Some(p) = next else { break };
                    let ends = matches!(p.frame, Frame::End) && p.direction == Direction::Down;
                    stage.process(p.frame, p.direction, &out).await;
                    if ends {
                        break;
                    }
                }
                stage.stopped().await;
            }));
        }
        drop(senders);
        drop(drop_tx);
        let mut end_rx = self.end_rx;
        tokio::select! {
            _ = token.cancelled() => {}
            _ = async {
                while let Some(p) = end_rx.recv().await {
                    if matches!(p.frame, Frame::End) {
                        break;
                    }
                }
            } => {}
        }
        for t in tasks {
            let _ = tokio::time::timeout(Duration::from_secs(5), t).await;
        }
        drain_up.abort();
    }
}

// --- the session -------------------------------------------------------------------------

/// What the daemon gives a session besides the models and the transport.
#[derive(Default)]
pub struct SessionHooks {
    pub on_turn: Option<OnTurn>,
    pub on_phase: Option<OnPhase>,
    pub on_hang_up: Option<SessionAction>,
    pub on_conversation: Option<OnSwitch>,
    pub on_latency: Option<crate::latency::Sink>,
}

/// How a session opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Greeting {
    /// One of `GREETINGS` (or `THREAD_GREETINGS` for an agent).
    Varied,
    Text(String),
    /// No greeting: the model runs on the context (a context note, a phone that connects).
    None,
}

pub struct SessionParams {
    pub settings: Arc<VoiceSettings>,
    pub models: Arc<Models>,
    pub model: Arc<dyn ModelPort>,
    pub sources: Vec<ToolSource>,
    pub gate: Arc<WriteGate>,
    pub events: Option<Arc<Events>>,
    pub media: Media,
    pub greeting: Greeting,
    /// What the operator said right after the wake phrase: the first user turn, no greeting.
    pub first_utterance: Option<String>,
    pub hooks: SessionHooks,
    /// What brings a session back from hold (ADR 0021); the desk's wake phrases by default.
    pub wake_phrases: Option<Vec<String>>,
    pub wake_fuzzy: f64,
    pub conversations: Option<Arc<Conversations>>,
    pub start_in: String,
    pub latency_log: Option<PathBuf>,
    /// What an agent's session reaches: every connected app and Ghost's own sources (ADR 0035).
    pub reach: Option<Reach>,
    /// Into the model's context before the greeting, without running it: why Ghost
    /// started this session itself (ADR 0032).
    pub context_note: Option<String>,
}

pub struct GhostSession {
    pub session_id: String,
    pub control: Arc<SessionControl>,
    pub conversation: Option<Arc<ConversationRuntime>>,
    pub latency: Option<Arc<LatencyObserver>>,
    pub context: SharedContext,
    pipeline: Mutex<Option<Pipeline>>,
    task: Mutex<Option<JoinHandle<()>>>,
    opening: Mutex<Option<Vec<Frame>>>,
    media_out: Mutex<Option<Arc<dyn crate::rtvi::RtviSink>>>,
}

impl GhostSession {
    /// Spawn the stages; with `open`, greet or run the first turn now.
    pub async fn start(&self, open: bool) {
        let pipeline = self.pipeline.lock().unwrap().take();
        let Some(pipeline) = pipeline else { return };
        if let Some(l) = &self.latency {
            l.set_clock(pipeline.clock());
        }
        let token = self.control.token.clone();
        *self.task.lock().unwrap() = Some(tokio::spawn(pipeline.run(token)));
        if open {
            self.open().await;
        }
    }

    /// The greeting or the first turn, for a transport whose far end connects after the
    /// pipeline starts.
    pub async fn open(&self) {
        let frames = self.opening.lock().unwrap().take();
        if let Some(frames) = frames {
            self.control.inject(frames).await;
        }
    }

    /// Wait until the pipeline has ended.
    pub async fn run(&self) {
        let task = self.task.lock().unwrap().take();
        if let Some(t) = task {
            let _ = t.await;
        }
        self.control.stop_hold_timer();
        if let Some(l) = &self.latency {
            l.flush(); // the turn that was open when the session ended
        }
    }

    pub async fn end(&self) {
        self.control.end().await;
    }

    pub async fn cancel(&self) {
        self.control.cancel();
    }

    pub fn ended(&self) -> bool {
        self.control.ended()
    }

    pub async fn inject(&self, frames: Vec<Frame>) {
        self.control.inject(frames).await;
    }

    /// A typed user turn: goes through the LLM, the tools and the gate like speech. A
    /// typed turn during a hold ends the hold first (ADR 0021).
    pub async fn say_text(&self, text: &str) {
        self.control.unhold().await;
        if let Some(c) = &self.conversation {
            // a transfer in flight: let its context land first
            c.wait_settled(Duration::from_secs(8)).await;
            c.record("user", text);
        }
        self.control
            .inject(vec![Frame::ThinkingStart, Frame::user_text(text, true)])
            .await;
    }

    /// Tell the model something happened outside the conversation and have it respond.
    pub async fn system_note(&self, note: &str) {
        self.control
            .inject(vec![Frame::user_text(note, true)])
            .await;
    }

    /// Transfer the session to an agent by slug (the desktop's Agents screen).
    pub async fn switch(&self, slug: &str) -> bool {
        let Some(c) = &self.conversation else {
            return false;
        };
        let Ok(Some(target)) = c.conversations().get(slug) else {
            return false;
        };
        let frames = c.switch(&target, "", "").await;
        if frames.is_empty() {
            return false;
        }
        self.control.inject(frames).await;
        true
    }

    /// Send `data` to a connected client as an RTVI `server-message`.
    pub async fn send_server_message(&self, data: serde_json::Value) {
        let sink = self.media_out.lock().unwrap().clone();
        if let Some(s) = sink {
            s.send(crate::rtvi::server_message(data)).await;
        }
    }

    pub fn set_rtvi(&self, sink: Arc<dyn crate::rtvi::RtviSink>) {
        *self.media_out.lock().unwrap() = Some(sink);
    }
}

/// Build one session. `first_utterance` is what the operator said right after the wake
/// phrase; when given it is the first user turn and there is no greeting. With
/// `conversations` the session opens in `start_in` (normally `ghost`), seeded from its
/// memory, in its voice, and can be transferred (ADR 0014).
///
/// Every turn's timeline (`latency`) is logged and, with `latency_log`, appended there;
/// `on_latency` gets each record too. The media's origin (desk, phone) is only a label
/// on those records.
pub fn build_session(p: SessionParams) -> GhostSession {
    let settings = p.settings;
    let session_id = uuid::Uuid::new_v4().simple().to_string();
    let origin = p.media.origin;
    let latency = settings.latency.then(|| {
        let mut obs = LatencyObserver::new(&session_id, origin, Some(&p.model.model_name()))
            .with_log(
                p.latency_log.clone(),
                (settings.latency_max_mb * 1024.0 * 1024.0) as u64,
            );
        if let Some(sink) = p.hooks.on_latency.clone() {
            obs = obs.with_sink(sink);
        }
        Arc::new(obs)
    });
    let probe: Arc<dyn crate::latency::LatencyProbe> = match &latency {
        Some(l) => l.clone(),
        None => crate::latency::null_probe(),
    };
    let observer: Option<Arc<dyn Observer>> = latency.clone().map(|l| l as Arc<dyn Observer>);
    let events = p.events.unwrap_or_default();

    let session_token = CancellationToken::new();
    let turns = TurnControl::new(session_token.clone());
    let mut sources = p.sources;
    let fragments: Vec<String> = sources
        .iter()
        .map(|a| a.prompt.clone())
        .filter(|f| !f.is_empty())
        .collect();
    let on_turn_outer = p.hooks.on_turn.unwrap_or_else(no_turn);
    let on_phase = p.hooks.on_phase.unwrap_or_else(no_phase);

    let runtime = p.conversations.map(|convs| {
        sources.push(ghost_core::conversations::conversations_source(&convs));
        ConversationRuntime::new(
            convs,
            p.gate.clone(),
            &session_id,
            fragments.clone(),
            TTS_SAMPLE_RATE,
            settings.compact_keep_messages,
            &p.start_in,
            p.hooks.on_conversation.clone(),
            p.reach.clone(),
        )
        .with_turns(turns.clone())
    });

    // The pipeline's inboxes exist before the stages, so the control can inject at the source.
    let mut stages: Vec<(Option<Slot>, Box<dyn Stage>)> = Vec::new();

    let (source_tx, source_rx) = mpsc::channel::<Pushed>(INBOX);

    // The operator's words since Ghost last spoke, and that reply: what a change was asked
    // for in, read by the gate as a possible yes (ghost_core.request).
    let heard: Arc<Mutex<(Vec<String>, String)>> =
        Arc::new(Mutex::new((Vec::new(), String::new())));
    // Whether Ghost's voice is at the speaker, by handle: the output stage sets it, and
    // the stages that must not take Ghost's voice for the operator's read it before the
    // BotStarted frame has made its way up behind a TTS stage still rendering the reply.
    let speaking = Speaking::new();
    let echo = Arc::new(Mutex::new(
        EchoFilter::new(settings.echo_window_secs).with_speaking(speaking.clone()),
    ));
    // Each clause as it is rendered, a clause ahead of the speaker: what a leak of
    // Ghost's own voice would read as.
    let on_say: crate::tts::OnSay = {
        let echo = echo.clone();
        Arc::new(move |text: &str| echo.lock().unwrap().saying(text))
    };
    let last_request: crate::tools::LastRequest = {
        let heard = heard.clone();
        Arc::new(move || {
            let h = heard.lock().unwrap();
            (h.0.join(" "), h.1.clone())
        })
    };
    let on_turn: OnTurn = {
        let heard = heard.clone();
        let runtime = runtime.clone();
        let outer = on_turn_outer.clone();
        let echo = echo.clone();
        Arc::new(move |role, text| {
            let heard = heard.clone();
            let runtime = runtime.clone();
            let outer = outer.clone();
            let echo = echo.clone();
            Box::pin(async move {
                {
                    let mut h = heard.lock().unwrap();
                    if role == "assistant" {
                        echo.lock().unwrap().spoke(&text);
                        h.1 = text.clone();
                        h.0.clear();
                    } else if role == "user" {
                        h.0.push(text.clone());
                    }
                }
                if let Some(rt) = &runtime
                    && (role == "user" || role == "assistant")
                {
                    rt.record(role, &text);
                }
                outer(role, text).await;
            })
        })
    };
    let control = Arc::new(SessionControl {
        session_id: session_id.clone(),
        source: source_tx,
        token: session_token.clone(),
        turns: turns.clone(),
        hold: Arc::new(HoldState::default()),
        hold_timer: Mutex::new(None),
        hold_timeout: Duration::from_secs_f64(settings.hold_timeout_secs.max(0.0)),
        on_phase: on_phase.clone(),
        on_turn: on_turn.clone(),
        on_hang_up: p.hooks.on_hang_up.clone(),
        ended: AtomicBool::new(false),
    });
    let hold_state = control.hold.clone();
    let held: Arc<dyn Fn() -> bool + Send + Sync> = {
        let h = hold_state.clone();
        Arc::new(move || h.on_hold())
    };

    let hang_up: SessionAction = {
        let c = control.clone();
        Arc::new(move || {
            let c = c.clone();
            Box::pin(async move { c.hang_up(true).await })
        })
    };
    let hang_up_now: SessionAction = {
        let c = control.clone();
        Arc::new(move || {
            let c = c.clone();
            Box::pin(async move { c.hang_up_now().await })
        })
    };
    let hold_action: SessionAction = {
        let c = control.clone();
        Arc::new(move || {
            let c = c.clone();
            Box::pin(async move { c.hold().await })
        })
    };
    let on_resume: OnResume = {
        let c = control.clone();
        Arc::new(move |remainder| {
            let c = c.clone();
            Box::pin(async move { c.resume(remainder).await })
        })
    };

    // The model's context, the tools, the dispatcher.
    let system = match &runtime {
        Some(rt) => rt.system_prompt(None),
        None => build_system_prompt(&fragments, &Persona::ghost()),
    };
    let mut context = Context::new(system);
    if let Some(rt) = &runtime {
        context.messages = rt.initial_messages(None);
    }
    let context: SharedContext = Arc::new(Mutex::new(context));
    let last_assistant: crate::confirmation::LastAssistant = {
        let ctx = context.clone();
        Arc::new(move || last_assistant_text(&ctx.lock().unwrap()))
    };
    let record: Option<crate::tools::OnCall> = runtime.clone().map(|rt| {
        let rt = rt.clone();
        Arc::new(
            move |name: &str, args: &serde_json::Map<String, serde_json::Value>| {
                rt.record_call(name, args)
            },
        ) as crate::tools::OnCall
    });
    let dispatcher = ToolDispatcher::new(ToolsParams {
        sources: sources.clone(),
        gate: p.gate.clone(),
        session_id: session_id.clone(),
        on_hang_up: hang_up.clone(),
        on_hold: Some(hold_action.clone()),
        events: Some(events.clone()),
        conversations: runtime.clone(),
        on_call: record,
        last_request: Some(last_request),
        probe: Some(probe.clone()),
    });
    let tools = dispatcher.schemas();
    let options = Options {
        max_tokens: settings.llm_max_tokens,
        thinking: (!settings.llm_thinking.is_empty()).then(|| settings.llm_thinking.clone()),
        cache_system: true,
    };
    let inject: crate::llm::Inject = {
        let c = control.clone();
        Arc::new(move |frames| {
            let c = c.clone();
            Box::pin(async move { c.inject(frames).await })
        })
    };

    // Interceptors: Ghost's fixed intents, then the sources'.
    let mut interceptors: Vec<Arc<dyn ghost_core::TurnInterceptor>> = Vec::new();
    if settings.quick_intents {
        interceptors.push(Arc::new(QuickIntents));
    }
    for a in &sources {
        interceptors.extend(a.interceptors.iter().cloned());
    }

    // The stages, in the Python's order.
    let Media { input, output, .. } = p.media;
    stages.push((
        Some(Slot::AudioIn),
        Box::new(AudioInput::new(input, session_token.clone())),
    ));
    let interim = if settings.barge_in_min_words > 0 {
        settings.barge_in_interim_secs
    } else {
        0.0
    };
    stages.push((
        None,
        Box::new(
            ParakeetStt::new(
                p.models.stt.clone(),
                p.models.vad(&settings),
                settings.vad_params(),
                interim,
            )
            .with_speaking(speaking.clone()),
        ),
    ));
    let hold_gate = HoldGate::new(
        on_resume,
        Some(
            p.wake_phrases
                .unwrap_or_else(|| DEFAULT_WAKE_PHRASES.iter().map(|s| s.to_string()).collect()),
        ),
        p.wake_fuzzy,
    )
    .with_state(hold_state.clone());
    stages.push((None, Box::new(hold_gate))); // on hold, nothing below hears the room (ADR 0021)
    stages.push((None, Box::new(SharedEcho(echo.clone()))));
    stages.push((
        None,
        Box::new(UserTap::new(
            on_turn.clone(),
            on_phase.clone(),
            Some(held.clone()),
        )),
    ));
    stages.push((
        None,
        Box::new(
            InterceptorProcessor::new(interceptors, p.gate.clone(), &session_id, hang_up_now)
                .with_hold(hold_action)
                .with_last_assistant(last_assistant.clone())
                .with_probe(probe.clone())
                .with_events(events.clone())
                .with_speaking(speaking.clone()),
        ),
    ));
    stages.push((
        Some(Slot::AfterStt),
        Box::new(
            ConfirmationProcessor::new(p.gate.clone(), &session_id, last_assistant)
                .with_events(events.clone())
                .with_probe(probe.clone())
                .with_speaking(speaking.clone()),
        ),
    ));
    let analyzer = SmartTurnAnalyzer::new(
        p.models.turn.clone(),
        SmartTurnParams {
            stop_secs: settings.turn_stop_secs,
            ..SmartTurnParams::default()
        },
    );
    let on_idle: crate::turns::OnIdle = {
        let c = control.clone();
        let secs = settings.idle_timeout_secs;
        Arc::new(move || {
            let c = c.clone();
            Box::pin(async move {
                info!("idle for {secs}s");
                c.hang_up(true).await;
            })
        })
    };
    stages.push((
        Some(Slot::BeforeLlm),
        Box::new(
            UserAggregator::new(
                settings.barge_in_min_words,
                Some(analyzer),
                settings.turn_grace_secs,
                settings.turn_speculate,
                turns.clone(),
            )
            .with_probe(probe.clone())
            .with_idle(settings.idle_timeout_secs, on_idle, held.clone())
            .with_speaking(speaking.clone()),
        ),
    ));
    let compaction = runtime.as_ref().map(|_| Compaction {
        trigger_tokens: settings.compact_trigger_tokens,
        keep_messages: settings.compact_keep_messages,
        summary_tokens: settings.compact_summary_tokens,
    });
    stages.push((
        Some(Slot::AfterLlm),
        Box::new(
            LlmStage::new(
                p.model.clone(),
                context.clone(),
                tools,
                options,
                dispatcher.clone(),
                turns.clone(),
                inject,
                runtime.clone(),
            )
            .with_compaction(compaction),
        ),
    ));
    let tts_settings = match &runtime {
        Some(rt) => match rt.conversations().voice_for(&rt.active()) {
            Ok((voice, speed)) => TtsSettings { voice, speed },
            Err(_) => TtsSettings {
                voice: settings.tts_voice.clone(),
                speed: settings.tts_speed,
            },
        },
        None => TtsSettings {
            voice: settings.tts_voice.clone(),
            speed: settings.tts_speed,
        },
    };
    stages.push((None, Box::new(NarrationCut::default())));
    // At the turn's first call to an app, a word or two before the tools run (#43). Kokoro
    // only: an ElevenLabs reply would answer in another voice than the acknowledgment.
    if settings.ack && settings.tts_engine != "elevenlabs" {
        let outside: Outside = {
            let d = dispatcher.clone();
            Arc::new(move |name: &str| d.outside(name))
        };
        let on_ack: crate::tts::OnSay = {
            let echo = echo.clone();
            Arc::new(move |text: &str| echo.lock().unwrap().acked(text))
        };
        stages.push((
            None,
            Box::new(
                AckStage::new(
                    p.models.acks.clone(),
                    p.models.tts.clone(),
                    outside,
                    &tts_settings.voice,
                    tts_settings.speed,
                    turns.clone(),
                )
                .with_on_ack(on_ack),
            ),
        ));
    }
    stages.push((
        None,
        Box::new(ThinkingSoundProcessor::new(
            settings.thinking_delay_secs,
            Some(held.clone()),
        )),
    ));
    let on_marker: Option<crate::tap::OnMarker> = runtime
        .clone()
        .map(|rt| Arc::new(move |slug: &str| rt.on_marker(slug)) as crate::tap::OnMarker);
    stages.push((
        Some(Slot::BeforeTts),
        Box::new(AssistantTap::new(on_turn.clone(), on_marker)),
    ));
    let line: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>> = runtime.clone().map(|rt| {
        Arc::new(move || Some(rt.active().slug)) as Arc<dyn Fn() -> Option<String> + Send + Sync>
    });
    let tts = session_tts(
        &settings,
        p.models.tts.clone(),
        tts_settings,
        settings.clause_min_words(),
        line,
        turns.clone(),
        Some(on_say),
    );
    stages.push((Some(Slot::AudioOut), tts));
    stages.push((None, Box::new(ThinkingSoundCut)));
    stages.push((
        None,
        Box::new(AudioOutput::new(output).with_speaking(speaking)),
    ));

    let stages = assemble(stages, Vec::new());
    let mut pipeline = Pipeline::new(stages, observer);
    // The source inbox is the control's: swap the pipeline's first inbox for it.
    pipeline.senders[0] = control.source.clone();
    pipeline.inboxes[0] = source_rx;

    // What opens the session.
    let mut opening: Vec<Frame> = Vec::new();
    if let Some(rt) = &runtime {
        opening.extend(rt.start_frames());
    }
    if let Some(note) = &p.context_note {
        opening.push(Frame::user_text(note, false));
    }
    if let Some(first) = &p.first_utterance {
        if let Some(rt) = &runtime {
            rt.record("user", first);
        }
        opening.push(Frame::ThinkingStart);
        opening.push(Frame::user_text(first, true));
    } else {
        match &p.greeting {
            Greeting::Varied => {
                let text = match &runtime {
                    Some(rt) if !rt.active().is_ghost() => {
                        pick(THREAD_GREETINGS).replace("{name}", &rt.active().name)
                    }
                    _ => pick(GREETINGS),
                };
                opening.push(Frame::Speak(text));
            }
            Greeting::Text(t) => opening.push(Frame::Speak(t.clone())),
            Greeting::None => opening.push(Frame::RunLlm),
        }
    }

    GhostSession {
        session_id,
        control,
        conversation: runtime,
        latency,
        context,
        pipeline: Mutex::new(Some(pipeline)),
        task: Mutex::new(None),
        opening: Mutex::new(Some(opening)),
        media_out: Mutex::new(None),
    }
}

/// The echo filter shared with the taps: `spoke` is fed by the assistant's turns.
struct SharedEcho(Arc<Mutex<EchoFilter>>);

#[async_trait::async_trait]
impl Stage for SharedEcho {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        let drop = {
            let mut e = self.0.lock().unwrap();
            match &frame {
                Frame::BotStarted => {
                    e.bot_speaking(true);
                    false
                }
                Frame::BotStopped => {
                    e.bot_speaking(false);
                    false
                }
                Frame::Transcript(text) => e.is_echo(text, None),
                Frame::Interim(text) => e.is_echo(text, Some(crate::echo::INTERIM_MIN_WORDS)),
                _ => false,
            }
        };
        if drop {
            if let Frame::Transcript(t) | Frame::Interim(t) = &frame {
                crate::debug::audio(|| format!("echo: dropped {t:?}"));
            }
            return;
        }
        out.forward(frame, direction).await;
    }
}

/// The slug a session starts in when none is named.
pub fn default_start_in() -> String {
    GHOST.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_end_runs_after_run_however_it_ended() {
        // The hold timer's cancel hangs on run(): it must run when the pipeline returns on
        // its own (the desktop's End) and when it is cancelled, not only on hang_up.
        for cancel in [false, true] {
            let (tx, rx) = mpsc::channel::<Pushed>(8);
            let token = CancellationToken::new();
            let control = Arc::new(SessionControl {
                session_id: "s".into(),
                source: tx,
                token: token.clone(),
                turns: TurnControl::new(token.clone()),
                hold: Arc::new(HoldState::default()),
                hold_timer: Mutex::new(Some(tokio::spawn(async {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }))),
                hold_timeout: Duration::from_secs(1),
                on_phase: no_phase(),
                on_turn: no_turn(),
                on_hang_up: None,
                ended: AtomicBool::new(false),
            });
            let mut rx = rx;
            let session = GhostSession {
                session_id: "s".into(),
                control: control.clone(),
                conversation: None,
                latency: None,
                context: Arc::new(Mutex::new(Context::new(""))),
                pipeline: Mutex::new(None),
                task: Mutex::new(Some(tokio::spawn(async move {
                    while let Some(p) = rx.recv().await {
                        if matches!(p.frame, Frame::End) {
                            break;
                        }
                    }
                }))),
                opening: Mutex::new(None),
                media_out: Mutex::new(None),
            };
            if cancel {
                session.cancel().await;
                // the pipeline task ends when its sender is dropped
                drop(session.control.source.clone());
            }
            if !cancel {
                session.end().await;
            } else {
                control.inject(vec![Frame::End]).await;
            }
            session.run().await;
            assert!(
                control.hold_timer.lock().unwrap().is_none(),
                "cancel={cancel}"
            );
        }
    }
}
