//! The frames the pipeline carries and the contract a stage fulfils.
//!
//! Pipecat's frames were classes; here [`Frame`] is one closed enum, so every stage matches
//! exhaustively on what reaches it. A stage ([`Stage`]) takes a frame with its direction
//! and pushes what it makes through an [`Out`]: `push` for a frame it originates (the
//! latency observer sees those), `forward` for one passing through. The session's runner
//! (`pipeline`) gives each stage one inbox, a bounded `mpsc` of [`Pushed`] frames, and
//! an `Out` whose downstream end is the next stage's inbox and whose upstream end is the
//! previous one's; a stage that needs a timer keeps a clone of its `Out`.
//!
//! No frame carries passive audio: [`Frame::AudioIn`] exists inside a session, after the
//! wake stage, and the passive detector (`wakeword`) consumes audio without forwarding it.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ghost_core::source::Slot;
use ghost_llm::{Message, Tools};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A stage's inbox: 20 ms audio frames make this about five seconds of audio, far more
/// than a paced source ever queues; the bound is there so a stuck stage is felt, not hidden.
pub const INBOX: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Down,
    Up,
}

/// Mono s16: 16 kHz from the microphone, 24 kHz to the speaker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audio {
    pub samples: Vec<i16>,
    pub rate: u32,
}

impl Audio {
    pub fn new(samples: Vec<i16>, rate: u32) -> Self {
        Self { samples, rate }
    }

    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(self.rate)
    }
}

/// What a service measured (Pipecat's `MetricsFrame` data): a time to first byte, a
/// model call's token usage, Smart Turn's verdict. Numbers and model ids only.
#[derive(Clone, Debug, PartialEq)]
pub enum Metric {
    Ttfb {
        value: f64,
        model: Option<String>,
    },
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        model: Option<String>,
    },
    Turn {
        complete: bool,
        probability: f64,
        ms: f64,
    },
}

/// Everything that travels between stages. The names keep Pipecat's where the idea is the
/// same (`UserStarted` is `UserStartedSpeakingFrame`, `LlmEnd` is
/// `LLMFullResponseEndFrame`), so the Python's tests read across.
#[derive(Clone, Debug)]
pub enum Frame {
    // --- speech in
    /// 16 kHz from the microphone, inside a session only.
    AudioIn(Audio),
    VadStarted {
        start_secs: f32,
    },
    VadStopped {
        stop_secs: f32,
    },
    /// An interim transcript: the operator's open segment read again while Ghost speaks.
    /// Only the barge-in rule and the echo filter read these.
    Interim(String),
    /// A final transcript of one segment: the pipeline's own evidence of what was said.
    Transcript(String),
    /// The user's turn opened (a start strategy fired).
    UserStarted,
    /// The user's turn closed (after Smart Turn and the grace): the model is about to run.
    UserStopped,
    /// An interceptor took the turn: the aggregator forgets its earlier pieces.
    ResetAggregation,

    // --- the model
    ContextAppend {
        messages: Vec<Message>,
        run_llm: bool,
    },
    ContextUpdate {
        messages: Vec<Message>,
        run_llm: bool,
    },
    SystemPrompt(String),
    SetTools(Tools),
    RunLlm,
    /// Start answering `text` at Smart Turn's verdict, before the grace confirms the turn.
    Speculate(String),
    /// The grace ended with the same words: the held answer is the answer.
    SpeculationConfirmed,
    /// The operator went on, or the words changed: the held answer is thrown away.
    SpeculationCancelled,
    /// The model withdrew a speculative answer that reached a tool call; the confirmed
    /// turn runs its own inference.
    EagerCancel,
    LlmStart,
    /// The turn the reply after `LlmStart` was made for: the speech stages stamp its audio
    /// with this token, so a reply still queued when the operator cut in never plays.
    ReplyTurn(CancellationToken),
    LlmText(String),
    LlmThought,
    /// The names of the calls a reply makes, as the model finished them.
    FunctionCallsStarted(Vec<String>),
    FunctionCallInProgress {
        name: String,
        id: String,
    },
    FunctionCallResult {
        name: String,
        id: String,
    },
    LlmEnd,
    /// The reply hit the output cap (`ghost_llm`); the turn is flagged `cut_off`.
    ReplyCut,
    Metrics(Metric),

    // --- the thinking sound
    /// Queued with a programmatic first turn: the model is about to run.
    ThinkingStart,
    /// Nothing is coming: an interceptor took the turn, or the session went on hold.
    ThinkingStop,
    MixerEnable(bool),

    // --- speech out
    /// A line spoken directly: a greeting, an interceptor's reply.
    Speak(String),
    /// The agent's voice (ADR 0014).
    VoiceChange {
        voice: String,
        speed: f64,
    },
    /// Travels with a transfer: text after it is the new agent's.
    Marker(String),
    TtsStarted,
    /// Words the cloud voice aligned with returned audio.
    TtsText(String),
    /// 24 kHz speech, with the turn it belongs to: cancelled, it never reaches the speaker.
    TtsAudio {
        audio: Audio,
        turn: CancellationToken,
    },
    TtsStopped,
    /// 24 kHz audio that isn't speech (the transfer tone).
    AudioOut {
        audio: Audio,
        turn: CancellationToken,
    },
    /// Ghost's one- or two-word acknowledgment as it goes to an app (`crate::acks`): 24 kHz,
    /// rendered ahead. Played without `BotStarted`/`BotStopped`, so it is not the reply
    /// to the timeline, the phase or the phone; the shared speaking flag covers it.
    Ack {
        text: String,
        audio: Audio,
        turn: CancellationToken,
    },
    /// Up from the output stage: the acknowledgment's first sample is at the speaker.
    AckPlayed,
    BotStarted,
    BotStopped,

    // --- control
    /// The operator cut in: whatever Ghost was saying stops.
    Interruption,
    /// The session is ending; `keep_current` lets the goodbye in the current reply through.
    HangUp {
        keep_current: bool,
    },
    /// A phone's data channel said `client-ready`.
    ClientConnected,
    /// Finish what is queued, then stop.
    End,
    /// Stop now.
    Cancel,
    Error(String),
}

impl Frame {
    pub fn kind(&self) -> &'static str {
        match self {
            Frame::AudioIn(_) => "AudioIn",
            Frame::VadStarted { .. } => "VadStarted",
            Frame::VadStopped { .. } => "VadStopped",
            Frame::Interim(_) => "Interim",
            Frame::Transcript(_) => "Transcript",
            Frame::UserStarted => "UserStarted",
            Frame::UserStopped => "UserStopped",
            Frame::ResetAggregation => "ResetAggregation",
            Frame::ContextAppend { .. } => "ContextAppend",
            Frame::ContextUpdate { .. } => "ContextUpdate",
            Frame::SystemPrompt(_) => "SystemPrompt",
            Frame::SetTools(_) => "SetTools",
            Frame::RunLlm => "RunLlm",
            Frame::Speculate(_) => "Speculate",
            Frame::SpeculationConfirmed => "SpeculationConfirmed",
            Frame::SpeculationCancelled => "SpeculationCancelled",
            Frame::EagerCancel => "EagerCancel",
            Frame::LlmStart => "LlmStart",
            Frame::ReplyTurn(_) => "ReplyTurn",
            Frame::LlmText(_) => "LlmText",
            Frame::LlmThought => "LlmThought",
            Frame::FunctionCallsStarted(_) => "FunctionCallsStarted",
            Frame::FunctionCallInProgress { .. } => "FunctionCallInProgress",
            Frame::FunctionCallResult { .. } => "FunctionCallResult",
            Frame::LlmEnd => "LlmEnd",
            Frame::ReplyCut => "ReplyCut",
            Frame::Metrics(_) => "Metrics",
            Frame::ThinkingStart => "ThinkingStart",
            Frame::ThinkingStop => "ThinkingStop",
            Frame::MixerEnable(_) => "MixerEnable",
            Frame::Speak(_) => "Speak",
            Frame::VoiceChange { .. } => "VoiceChange",
            Frame::Marker(_) => "Marker",
            Frame::TtsStarted => "TtsStarted",
            Frame::TtsText(_) => "TtsText",
            Frame::TtsAudio { .. } => "TtsAudio",
            Frame::TtsStopped => "TtsStopped",
            Frame::AudioOut { .. } => "AudioOut",
            Frame::Ack { .. } => "Ack",
            Frame::AckPlayed => "AckPlayed",
            Frame::BotStarted => "BotStarted",
            Frame::BotStopped => "BotStopped",
            Frame::Interruption => "Interruption",
            Frame::HangUp { .. } => "HangUp",
            Frame::ClientConnected => "ClientConnected",
            Frame::End => "End",
            Frame::Cancel => "Cancel",
            Frame::Error(_) => "Error",
        }
    }

    /// A frame that must not wait behind audio: it jumps the inbox (Pipecat's system frames).
    pub fn is_system(&self) -> bool {
        matches!(
            self,
            Frame::Interruption
                | Frame::Cancel
                | Frame::BotStarted
                | Frame::BotStopped
                | Frame::VadStarted { .. }
                | Frame::VadStopped { .. }
                | Frame::UserStarted
                | Frame::Error(_)
        )
    }

    /// A user turn's text: a typed turn, a resume after a hold, the first utterance.
    pub fn user_text(text: impl Into<String>, run_llm: bool) -> Frame {
        Frame::ContextAppend {
            messages: vec![Message::user(text)],
            run_llm,
        }
    }
}

/// A frame on its way, with the direction it travels.
#[derive(Clone, Debug)]
pub struct Pushed {
    pub frame: Frame,
    pub direction: Direction,
}

/// Seconds on the pipeline's clock, from the moment the session's pipeline was built.
#[derive(Clone, Debug)]
pub struct Clock {
    zero: Instant,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Self {
            zero: Instant::now(),
        }
    }

    pub fn zero(&self) -> Instant {
        self.zero
    }

    pub fn now(&self) -> f64 {
        self.zero.elapsed().as_secs_f64()
    }

    pub fn at(&self, instant: Instant) -> f64 {
        instant.saturating_duration_since(self.zero).as_secs_f64()
    }
}

/// Sees every frame on its first push, from the stage that made it (Pipecat's observer
/// with `observe_every_push=False`). The latency timeline is one.
pub trait Observer: Send + Sync {
    fn on_frame(&self, frame: &Frame, source: &'static str, at: f64, direction: Direction);
}

/// Where a stage pushes. Cheap to clone: a timer task keeps one.
#[derive(Clone)]
pub struct Out {
    down: mpsc::Sender<Pushed>,
    up: mpsc::Sender<Pushed>,
    source: &'static str,
    clock: Clock,
    observer: Option<Arc<dyn Observer>>,
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out").field("source", &self.source).finish()
    }
}

impl Out {
    pub fn new(
        down: mpsc::Sender<Pushed>,
        up: mpsc::Sender<Pushed>,
        source: &'static str,
        clock: Clock,
        observer: Option<Arc<dyn Observer>>,
    ) -> Self {
        Self {
            down,
            up,
            source,
            clock,
            observer,
        }
    }

    /// An `Out` whose both ends land in one receiver, for a stage driven by a test.
    pub fn capture() -> (Self, mpsc::Receiver<Pushed>) {
        let (tx, rx) = mpsc::channel(4096);
        (Self::new(tx.clone(), tx, "test", Clock::new(), None), rx)
    }

    pub fn source(&self) -> &'static str {
        self.source
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    fn observe(&self, frame: &Frame, direction: Direction) {
        if let Some(obs) = &self.observer {
            obs.on_frame(frame, self.source, self.clock.now(), direction);
        }
    }

    async fn send(&self, frame: Frame, direction: Direction) {
        let tx = match direction {
            Direction::Down => &self.down,
            Direction::Up => &self.up,
        };
        // The receiver is gone when the session ended: nothing to do with the frame.
        let _ = tx.send(Pushed { frame, direction }).await;
    }

    /// A frame this stage made, downstream.
    pub async fn push(&self, frame: Frame) {
        self.observe(&frame, Direction::Down);
        self.send(frame, Direction::Down).await;
    }

    /// A frame this stage made, upstream.
    pub async fn push_up(&self, frame: Frame) {
        self.observe(&frame, Direction::Up);
        self.send(frame, Direction::Up).await;
    }

    /// A frame passing through, the way it was going.
    pub async fn forward(&self, frame: Frame, direction: Direction) {
        self.send(frame, direction).await;
    }
}

/// One stage of the pipeline. `process` takes every frame that reaches it, in both
/// directions, and pushes what it makes through `out`; a stage that passes a frame on
/// calls `out.forward`. `started` runs once the pipeline is up, `stopped` when it is down.
#[async_trait]
pub trait Stage: Send {
    fn name(&self) -> &'static str;
    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out);
    async fn started(&mut self, _out: &Out) {}
    async fn stopped(&mut self) {}
}

/// Splice processors into the fixed pipeline. `core` is the ordered list of Ghost's own
/// processors, each tagged with the slot that comes right after it.
pub fn assemble<T>(core: Vec<(Option<Slot>, T)>, mut slots: Vec<(Slot, Vec<T>)>) -> Vec<T> {
    let mut out = Vec::new();
    for (slot, proc_) in core {
        out.push(proc_);
        if let Some(slot) = slot {
            for (s, extra) in slots.iter_mut() {
                if *s == slot {
                    out.append(extra);
                }
            }
        }
    }
    out
}

/// What a test driving one stage reads back: the frames pushed, in order.
pub async fn drain(rx: &mut mpsc::Receiver<Pushed>) -> Vec<Frame> {
    let mut out = Vec::new();
    while let Ok(p) = rx.try_recv() {
        out.push(p.frame);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assemble_splices_slots_in_order() {
        let core = vec![
            (Some(Slot::AudioIn), "in"),
            (None, "stt"),
            (Some(Slot::AfterStt), "confirm"),
            (None, "out"),
        ];
        let out = assemble(
            core,
            vec![
                (Slot::AfterStt, vec!["a", "b"]),
                (Slot::AudioIn, vec!["ns"]),
            ],
        );
        assert_eq!(out, vec!["in", "ns", "stt", "confirm", "a", "b", "out"]);
    }

    #[tokio::test]
    async fn a_pushed_frame_is_observed_once_and_a_forwarded_one_never() {
        struct Count(std::sync::Mutex<Vec<&'static str>>);
        impl Observer for Count {
            fn on_frame(&self, frame: &Frame, source: &'static str, _at: f64, _d: Direction) {
                assert_eq!(source, "test");
                self.0.lock().unwrap().push(frame.kind());
            }
        }
        let count = Arc::new(Count(Default::default()));
        let (tx, mut rx) = mpsc::channel(8);
        let out = Out::new(tx.clone(), tx, "test", Clock::new(), Some(count.clone()));
        out.push(Frame::LlmStart).await;
        out.forward(Frame::LlmEnd, Direction::Down).await;
        out.push_up(Frame::BotStarted).await;
        assert_eq!(*count.0.lock().unwrap(), vec!["LlmStart", "BotStarted"]);
        let got = drain(&mut rx).await;
        assert_eq!(got.len(), 3);
    }
}
