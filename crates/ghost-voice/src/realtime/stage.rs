//! The realtime voice in a session's pipeline (ADR 0063): four stages added to the local
//! pipeline, which stays whole underneath so a session can fall back to it at any moment.
//!
//! ```text
//! capture -> [uplink] -> speech-in (Parakeet, beside) -> hold -> echo -> taps -> interceptors
//!   -> [confirm watch] -> confirmation -> the user's turn -> [gate] -> model (Ghost's agent)
//!   -> narration cut -> [voice] -> thinking -> taps -> TTS (Kokoro, the fallback) -> playback
//! ```
//!
//! - [`Uplink`] sends the session's microphone audio to the provider: never on hold, and
//!   while Ghost speaks only once Ghost's own barge-in rule has said the operator is
//!   talking over it (with the second and a half before), so a leak of Ghost's voice past
//!   the canceller never reaches the provider as speech.
//! - [`ConfirmWatch`] marks a turn heard while a change waits for its yes: that turn goes
//!   to the agent, judged by the confirmation processor on Ghost's own transcript.
//! - [`Gate`] keeps the operator's spoken turns from the agent while the realtime voice
//!   answers them (they are kept for the next consult as the words of record), and lets
//!   through a consult, a typed turn, a resume, a confirmation turn, and everything once
//!   the session is local.
//! - [`Voice`] connects, plays the provider's audio, hands a consult to the agent, relays
//!   the agent's answers to be said exactly (checked as they play), and falls back.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_llm::Message;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::relay::{Outcome, RelayCheck, Verdict};
use super::{
    CONSULT, Connection, Event, Failure, HANG_UP, HOLD, OUT_RATE, RealtimeLink, RealtimeProvider,
    Usage, setup,
};
use crate::confirmation::SessionGate;
use crate::frames::{Audio, Direction, Frame, Out, Stage};
use crate::hold::HoldState;
use crate::intercept::SessionAction;
use crate::latency::LatencyProbe;
use crate::llm::Inject;
use crate::speaking::Speaking;
use crate::tap::OnTurn;

/// Said when the realtime voice did not answer as the session opened.
pub const START_SENTENCE: &str =
    "The realtime voice isn't answering, so I'm on this machine for now.";
/// What the realtime model is told a consult's answer is.
pub const ANSWERED: &str = "{\"status\": \"Ghost's agent answered; its answer is being said to the operator now, exactly.\"}";
pub const NOTHING: &str = "{\"status\": \"Ghost's agent had nothing to add.\"}";
/// A relay asked for and not begun in this long is said in the local voice instead.
pub const RELAY_START_SECS: f64 = 4.0;
/// How much of the operator's audio is kept back while Ghost speaks, in case it was them.
pub const HELD_BACK_SECS: f64 = 1.5;
/// The longest the end of a session waits for an answer the realtime voice is saying.
pub const END_WAIT_SECS: f64 = 8.0;

const CONNECTING: u8 = 0;
const LIVE: u8 = 1;
const LOCAL: u8 = 2;

/// The operator's words of record, between the turn's close and the agent.
#[derive(Default)]
pub struct Words {
    /// Spoken turns closed and on their way to the gate, oldest first.
    closed: VecDeque<String>,
    /// The operator's turns since the last consult, as Ghost heard them.
    since: Vec<String>,
    /// The last turn the realtime voice took and has not answered yet.
    unanswered: Option<String>,
    /// The next spoken turn answers a change waiting for its yes.
    confirm_next: bool,
    /// Turns for the agent the gate lets through (consults, a fallback's last words).
    forward: VecDeque<String>,
}

/// What the realtime stages share.
pub struct Shared {
    mode: AtomicU8,
    link: Mutex<Option<Arc<dyn RealtimeLink>>>,
    pub words: Mutex<Words>,
    barged: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            mode: AtomicU8::new(CONNECTING),
            link: Mutex::new(None),
            words: Mutex::new(Words::default()),
            barged: AtomicBool::new(false),
        }
    }
}

impl Shared {
    pub fn local(&self) -> bool {
        self.mode.load(Ordering::SeqCst) == LOCAL
    }

    pub fn live(&self) -> bool {
        self.mode.load(Ordering::SeqCst) == LIVE
    }

    fn link(&self) -> Option<Arc<dyn RealtimeLink>> {
        self.link.lock().unwrap().clone()
    }

    /// A spoken turn closed with these words (`crate::tap::TurnRecord::Closed`).
    pub fn closed(&self, text: &str) {
        self.words
            .lock()
            .unwrap()
            .closed
            .push_back(text.to_string());
    }

    /// The last closed turn was taken back to be heard whole.
    pub fn reopened(&self) {
        let mut w = self.words.lock().unwrap();
        w.closed.pop_back();
        w.since.pop();
    }
}

// --- the uplink ---------------------------------------------------------------------------

pub struct Uplink {
    shared: Arc<Shared>,
    hold: Arc<HoldState>,
    speaking: Speaking,
    back: VecDeque<i16>,
}

impl Uplink {
    pub fn new(shared: Arc<Shared>, hold: Arc<HoldState>, speaking: Speaking) -> Self {
        Self {
            shared,
            hold,
            speaking,
            back: VecDeque::new(),
        }
    }

    fn send(&mut self, pcm: &[i16]) {
        let Some(link) = self.shared.link() else {
            return;
        };
        if !self.back.is_empty() {
            let back: Vec<i16> = self.back.drain(..).collect();
            link.audio(&back);
        }
        link.audio(pcm);
    }
}

#[async_trait]
impl Stage for Uplink {
    fn name(&self) -> &'static str {
        "realtime_uplink"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match (&frame, direction) {
            (Frame::AudioIn(a), Direction::Down) if self.shared.live() => {
                if self.hold.on_hold() {
                    // On hold nothing the room says leaves the machine (ADR 0021).
                    self.back.clear();
                } else if self.speaking.is_on() && !self.shared.barged.load(Ordering::SeqCst) {
                    self.back.extend(a.samples.iter().copied());
                    let keep = (HELD_BACK_SECS * f64::from(a.rate)) as usize;
                    while self.back.len() > keep {
                        self.back.pop_front();
                    }
                } else {
                    if !self.speaking.is_on() {
                        // Kept back while Ghost spoke and never a barge-in: not sent.
                        self.shared.barged.store(false, Ordering::SeqCst);
                        self.back.clear();
                    }
                    let pcm = a.samples.clone();
                    self.send(&pcm);
                }
            }
            (Frame::Interruption, Direction::Up) => {
                // Ghost's own rule heard the operator over its voice: what was kept back
                // is theirs, and so is what follows.
                self.shared.barged.store(true, Ordering::SeqCst);
                if self.shared.live() && !self.hold.on_hold() {
                    self.send(&[]);
                }
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }
}

// --- the confirm watch --------------------------------------------------------------------

pub struct ConfirmWatch {
    shared: Arc<Shared>,
    gate: Arc<dyn SessionGate>,
    session_id: String,
    speaking: Speaking,
}

impl ConfirmWatch {
    pub fn new(
        shared: Arc<Shared>,
        gate: Arc<dyn SessionGate>,
        session_id: &str,
        speaking: Speaking,
    ) -> Self {
        Self {
            shared,
            gate,
            session_id: session_id.to_string(),
            speaking,
        }
    }
}

#[async_trait]
impl Stage for ConfirmWatch {
    fn name(&self) -> &'static str {
        "realtime_confirm"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Down
            && let Frame::Transcript(_) = &frame
            && !self.speaking.is_on()
            && self.gate.pending(&self.session_id)
        {
            self.shared.words.lock().unwrap().confirm_next = true;
        }
        out.forward(frame, direction).await;
    }
}

// --- the gate -----------------------------------------------------------------------------

pub struct Gate {
    shared: Arc<Shared>,
}

impl Gate {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// Whether a run of the agent on this user message goes ahead.
    fn lets_through(&self, text: &str) -> bool {
        let mut w = self.shared.words.lock().unwrap();
        if w.forward.front().is_some_and(|f| f == text) {
            w.forward.pop_front();
            return true;
        }
        if w.closed.front().is_some_and(|c| c == text) {
            w.closed.pop_front();
            if self.shared.local() || std::mem::take(&mut w.confirm_next) {
                return true;
            }
            // The realtime voice answers it; the agent hears it with the next consult.
            w.since.push(text.to_string());
            w.unanswered = Some(text.to_string());
            return false;
        }
        // Not a spoken turn: typed, a resume after hold, a first utterance, a note.
        true
    }
}

#[async_trait]
impl Stage for Gate {
    fn name(&self) -> &'static str {
        "realtime_gate"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Down && !self.shared.local() {
            match &frame {
                Frame::ContextAppend {
                    messages,
                    run_llm: true,
                } if messages.len() == 1 => {
                    if let Message::User(text) = &messages[0]
                        && !self.lets_through(text)
                    {
                        return;
                    }
                }
                Frame::Speculate(_)
                | Frame::SpeculationConfirmed(_)
                | Frame::SpeculationCancelled
                | Frame::ReopenTurn(_)
                | Frame::EarlyTranscript(_) => return,
                _ => {}
            }
        }
        out.forward(frame, direction).await;
    }
}

// --- the voice ----------------------------------------------------------------------------

pub type OnUsage = Arc<dyn Fn(&Usage, f64) -> bool + Send + Sync>;
pub type OnFallback = Arc<dyn Fn(&Failure) + Send + Sync>;

/// What the daemon gives a realtime session.
#[derive(Clone)]
pub struct RealtimeParams {
    pub provider: Arc<dyn RealtimeProvider>,
    /// Each response's usage and its estimate in USD; `true` when the day's cap is reached.
    pub on_usage: Option<OnUsage>,
    /// The realtime voice could not be used or stopped: the waiting list's note.
    pub on_fallback: Option<OnFallback>,
    pub connect_timeout: Duration,
}

/// What the voice stage needs of the session.
pub struct VoiceParts {
    pub shared: Arc<Shared>,
    pub params: RealtimeParams,
    pub gate: Arc<dyn SessionGate>,
    pub session_id: String,
    pub on_turn: OnTurn,
    pub inject: Inject,
    pub hang_up: SessionAction,
    pub hold: SessionAction,
    pub probe: Arc<dyn LatencyProbe>,
}

enum Input {
    Frame(Frame, Direction),
    Provider(Event),
    Connected(Result<Connection, Failure>, f64),
    Tick,
}

pub struct Voice {
    parts: Option<VoiceParts>,
    tx: Option<mpsc::UnboundedSender<Input>>,
}

impl Voice {
    pub fn new(parts: VoiceParts) -> Self {
        Self {
            parts: Some(parts),
            tx: None,
        }
    }
}

#[async_trait]
impl Stage for Voice {
    fn name(&self) -> &'static str {
        "realtime_voice"
    }

    async fn started(&mut self, out: &Out) {
        let Some(parts) = self.parts.take() else {
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        self.tx = Some(tx.clone());
        let provider = parts.params.provider.clone();
        let timeout = parts.params.connect_timeout;
        let connect_tx = tx.clone();
        tokio::spawn(async move {
            let began = Instant::now();
            let r = match tokio::time::timeout(timeout, provider.connect(&setup())).await {
                Ok(r) => r,
                Err(_) => Err(Failure::Unreachable("no answer in time".into())),
            };
            let _ = connect_tx.send(Input::Connected(r, began.elapsed().as_secs_f64()));
        });
        let tick_tx = tx.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_millis(100));
            loop {
                every.tick().await;
                if tick_tx.send(Input::Tick).is_err() {
                    break;
                }
            }
        });
        let actor = Actor::new(parts, out.clone(), tx);
        tokio::spawn(actor.run(rx));
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &self.tx {
            Some(tx) => {
                if let Err(mpsc::error::SendError(Input::Frame(frame, direction))) =
                    tx.send(Input::Frame(frame, direction))
                {
                    out.forward(frame, direction).await;
                }
            }
            None => out.forward(frame, direction).await,
        }
    }

    async fn stopped(&mut self) {
        self.tx = None;
    }
}

struct Job {
    text: String,
    tag: String,
    /// The operator's speech end the answer is timed from, and when the agent was asked.
    anchor: Option<Instant>,
    consulted: Option<Instant>,
    answered: Option<Instant>,
}

struct Resp {
    id: String,
    item: String,
    turn: CancellationToken,
    job: Option<Job>,
    check: Option<RelayCheck>,
    held: VecDeque<Vec<i16>>,
    released_secs: f64,
    pushed: bool,
    said: String,
    settled: bool,
    cut: bool,
    started: Instant,
    heard_at: Option<Instant>,
    outcome: Option<&'static str>,
}

struct Consult {
    call_id: String,
    at: Instant,
    anchor: Option<Instant>,
}

enum After {
    HangUp,
    Hold,
}

struct Actor {
    p: VoiceParts,
    out: Out,
    tx: mpsc::UnboundedSender<Input>,
    link: Option<Arc<dyn RealtimeLink>>,
    held: Vec<(Frame, Direction)>,
    reply: String,
    reply_calls: bool,
    consults: VecDeque<Consult>,
    queue: VecDeque<Job>,
    asked: Option<(Job, Instant)>,
    active: Option<Resp>,
    after: Vec<After>,
    relays: u64,
    auto: bool,
    speech_end: Option<Instant>,
    bot_since: Option<Instant>,
    /// When the audio handed to the speaker so far will have played.
    queued_until: Option<Instant>,
    /// A response the provider finished that is still playing: its turn and its item, so
    /// an interruption stops it and tells the provider what was heard.
    playing: Option<(CancellationToken, String)>,
    end: Option<(Frame, Instant)>,
    usage: Usage,
    usd: f64,
    was_live: bool,
}

fn secs_since(a: Option<Instant>, b: Option<Instant>) -> Value {
    match (a, b) {
        (Some(a), Some(b)) if b >= a => json!(((b - a).as_secs_f64() * 1000.0).round() / 1000.0),
        _ => Value::Null,
    }
}

impl Actor {
    fn new(p: VoiceParts, out: Out, tx: mpsc::UnboundedSender<Input>) -> Self {
        Self {
            p,
            out,
            tx,
            link: None,
            held: Vec::new(),
            reply: String::new(),
            reply_calls: false,
            consults: VecDeque::new(),
            queue: VecDeque::new(),
            asked: None,
            active: None,
            after: Vec::new(),
            relays: 0,
            auto: true,
            speech_end: None,
            bot_since: None,
            queued_until: None,
            playing: None,
            end: None,
            usage: Usage::default(),
            usd: 0.0,
            was_live: false,
        }
    }

    fn mode(&self) -> u8 {
        self.p.shared.mode.load(Ordering::SeqCst)
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Input>) {
        while let Some(input) = rx.recv().await {
            match input {
                Input::Frame(frame, direction) => {
                    if self.mode() == CONNECTING {
                        self.held.push((frame, direction));
                    } else {
                        self.frame(frame, direction).await;
                    }
                }
                Input::Provider(e) => self.event(e).await,
                Input::Connected(r, secs) => self.connected(r, secs).await,
                Input::Tick => self.tick().await,
            }
        }
        if let Some(l) = self.link.take() {
            l.close();
        }
    }

    async fn connected(&mut self, r: Result<Connection, Failure>, secs: f64) {
        match r {
            Ok(c) => {
                if self.mode() == LOCAL {
                    c.link.close();
                    return;
                }
                info!(
                    "realtime: {} {} connected in {secs:.2}s",
                    self.p.params.provider.name(),
                    self.p.params.provider.model()
                );
                *self.p.shared.link.lock().unwrap() = Some(c.link.clone());
                self.link = Some(c.link);
                self.p.shared.mode.store(LIVE, Ordering::SeqCst);
                self.was_live = true;
                self.record(json!({"what": "connect", "secs": (secs * 1000.0).round() / 1000.0}));
                let tx = self.tx.clone();
                let mut events = c.events;
                tokio::spawn(async move {
                    while let Some(e) = events.recv().await {
                        if tx.send(Input::Provider(e)).is_err() {
                            break;
                        }
                    }
                });
                for (f, d) in std::mem::take(&mut self.held) {
                    self.frame(f, d).await;
                }
            }
            Err(f) => self.fallback(f).await,
        }
    }

    // --- frames from the pipeline -----------------------------------------------------

    async fn frame(&mut self, frame: Frame, direction: Direction) {
        if self.mode() == LOCAL {
            self.out.forward(frame, direction).await;
            return;
        }
        if direction == Direction::Up {
            match &frame {
                Frame::BotStarted => {
                    self.bot_since = Some(Instant::now());
                    if let Some(r) = &mut self.active
                        && r.pushed
                    {
                        // the voice's first sample at the speaker
                        r.heard_at = self.bot_since;
                    }
                }
                Frame::BotStopped => {
                    self.bot_since = None;
                    self.queued_until = None;
                    self.playing = None;
                    self.sync_auto();
                    self.ask_next();
                }
                _ => {}
            }
            self.out.forward(frame, direction).await;
            return;
        }
        match frame {
            Frame::VadStopped { stop_secs } => {
                self.speech_end = Instant::now()
                    .checked_sub(Duration::from_secs_f64(f64::from(stop_secs).max(0.0)));
                self.out.forward(frame, direction).await;
            }
            Frame::LlmStart => {
                self.reply.clear();
                self.reply_calls = false;
            }
            Frame::LlmText(t) => self.reply.push_str(&t),
            Frame::FunctionCallsStarted(names) => {
                if !names
                    .iter()
                    .all(|n| crate::narration::SPOKEN_WITH.contains(&n.as_str()))
                {
                    self.reply_calls = true;
                }
            }
            Frame::LlmEnd => {
                if !self.reply_calls {
                    let text = std::mem::take(&mut self.reply);
                    self.answer(text);
                }
            }
            Frame::Speak(text) => {
                if !text.trim().is_empty() {
                    self.relay(text, None, None, None);
                }
            }
            Frame::UserStopped
            | Frame::ThinkingStart
            | Frame::ToolStarting { .. }
            | Frame::ModelWriting
            | Frame::FunctionCallInProgress { .. }
            | Frame::FunctionCallResult { .. } => {}
            Frame::Interruption => {
                self.interrupted();
                self.out.forward(frame, direction).await;
            }
            Frame::End => {
                if self.active.is_some() || self.asked.is_some() || !self.queue.is_empty() {
                    // An answer is being said: the end waits for it, a while at most.
                    self.end = Some((frame, Instant::now()));
                } else {
                    self.close();
                    self.out.forward(frame, direction).await;
                }
            }
            Frame::Cancel => {
                self.close();
                self.out.forward(frame, direction).await;
            }
            other => self.out.forward(other, direction).await,
        }
    }

    fn close(&mut self) {
        if let Some(l) = self.link.take() {
            l.close();
        }
        *self.p.shared.link.lock().unwrap() = None;
    }

    /// The agent's reply that ends its turn: said exactly by the realtime voice.
    fn answer(&mut self, text: String) {
        let consult = self.consults.pop_front();
        if text.trim().is_empty() {
            if let (Some(c), Some(l)) = (&consult, &self.link) {
                l.answer(&c.call_id, NOTHING);
            }
            return;
        }
        if let (Some(c), Some(l)) = (&consult, &self.link) {
            l.answer(&c.call_id, ANSWERED);
        }
        let (anchor, consulted) = match &consult {
            Some(c) => (c.anchor, Some(c.at)),
            None => (self.speech_end, None),
        };
        self.relay(text, anchor, consulted, Some(Instant::now()));
    }

    fn relay(
        &mut self,
        text: String,
        anchor: Option<Instant>,
        consulted: Option<Instant>,
        answered: Option<Instant>,
    ) {
        self.relays += 1;
        self.queue.push_back(Job {
            text,
            tag: format!("relay-{}", self.relays),
            anchor,
            consulted,
            answered,
        });
        self.ask_next();
    }

    fn ask_next(&mut self) {
        if self.active.is_some() || self.asked.is_some() {
            return;
        }
        let Some(link) = self.link.clone() else {
            return;
        };
        let Some(job) = self.queue.pop_front() else {
            return;
        };
        link.say(&job.text, &job.tag);
        self.asked = Some((job, Instant::now()));
    }

    fn played_ms(&self) -> u64 {
        self.bot_since
            .map_or(0, |b| (b.elapsed().as_secs_f64() * 1000.0) as u64)
    }

    fn interrupted(&mut self) {
        let played = self.played_ms();
        self.queued_until = None;
        if let Some((turn, item)) = self.playing.take() {
            turn.cancel();
            if let Some(l) = &self.link {
                l.truncate(&item, played);
            }
        }
        let Some(r) = &mut self.active else {
            self.queue.clear();
            return;
        };
        if r.cut {
            return;
        }
        r.cut = true;
        r.turn.cancel();
        r.held.clear();
        r.outcome = Some("interrupted");
        if let Some(l) = &self.link {
            if !r.settled {
                l.cancel();
            }
            l.truncate(&r.item, played);
        }
        let heard = match &r.check {
            Some(c) => c.head(c.verified()),
            None => r.said.trim().to_string(),
        };
        if !heard.is_empty() {
            let on = self.p.on_turn.clone();
            tokio::spawn(async move { on("assistant", heard).await });
        }
        // What was queued answered a turn the operator has talked over.
        self.queue.clear();
    }

    // --- events from the provider -----------------------------------------------------

    async fn event(&mut self, e: Event) {
        if self.mode() != LIVE {
            return;
        }
        match e {
            Event::SpeechStarted | Event::SpeechStopped => {}
            Event::Started { response, tag } => {
                let job = match &self.asked {
                    Some((j, _)) if tag.as_deref() == Some(j.tag.as_str()) => {
                        self.asked.take().map(|(j, _)| j)
                    }
                    _ => None,
                };
                let check = job.as_ref().map(|j| RelayCheck::new(&j.text));
                if job.is_none() {
                    // The realtime voice answers the operator itself.
                    self.p.shared.words.lock().unwrap().unanswered = None;
                }
                self.active = Some(Resp {
                    id: response,
                    item: String::new(),
                    turn: CancellationToken::new(),
                    job,
                    check,
                    held: VecDeque::new(),
                    released_secs: 0.0,
                    pushed: false,
                    said: String::new(),
                    settled: false,
                    cut: false,
                    started: Instant::now(),
                    heard_at: None,
                    outcome: None,
                });
            }
            Event::Audio {
                response,
                item,
                pcm,
            } => {
                let Some(r) = &mut self.active else { return };
                if r.id != response || r.cut {
                    return;
                }
                if r.item.is_empty() {
                    r.item = item;
                }
                if r.check.is_some() {
                    r.held.push_back(pcm);
                    self.release().await;
                } else {
                    let turn = r.turn.clone();
                    r.pushed = true;
                    self.push_audio(pcm, turn).await;
                }
            }
            Event::Said { response, delta } => {
                let Some(r) = &mut self.active else { return };
                if r.id != response || r.cut {
                    return;
                }
                r.said.push_str(&delta);
                if let Some(c) = &mut r.check {
                    if let Verdict::Diverged { at, heard } = c.feed(&delta) {
                        self.strayed(at, &heard).await;
                        return;
                    }
                    self.release().await;
                }
            }
            Event::SaidAll { response, text } => {
                let Some(r) = &mut self.active else { return };
                if r.id != response || r.cut {
                    return;
                }
                if let Some(c) = &mut r.check {
                    r.settled = true;
                    match c.finish(Some(&text)) {
                        Outcome::Verified => {
                            r.outcome = Some("verified");
                            self.release().await;
                        }
                        Outcome::Incomplete { at } => self.strayed(at, "").await,
                        Outcome::Diverged { at, heard } => self.strayed(at, &heard).await,
                    }
                } else if !text.trim().is_empty() {
                    r.said = text.clone();
                    let on = self.p.on_turn.clone();
                    on("assistant", text).await;
                }
            }
            Event::Call {
                call_id,
                name,
                args,
                ..
            } => {
                let Some(link) = self.link.clone() else {
                    return;
                };
                match name.as_str() {
                    CONSULT => {
                        let request = args["request"].as_str().unwrap_or("").to_string();
                        self.consult(call_id, request).await;
                    }
                    HANG_UP => {
                        link.answer(&call_id, "{\"status\": \"ending\"}");
                        self.after.push(After::HangUp);
                    }
                    HOLD => {
                        link.answer(&call_id, "{\"status\": \"holding\"}");
                        self.after.push(After::Hold);
                    }
                    other => {
                        warn!("realtime: the voice called {other:?}, which it does not have");
                        link.answer(&call_id, "{\"error\": \"no such tool\"}");
                    }
                }
                if self.active.is_none() {
                    self.run_after().await;
                }
            }
            Event::Done {
                response,
                status,
                usage,
            } => {
                if let Some(u) = usage {
                    self.spend(&u).await;
                    if self.mode() != LIVE {
                        return;
                    }
                }
                let Some(r) = &mut self.active else { return };
                if r.id != response {
                    return;
                }
                if let Some(c) = &mut r.check
                    && !r.settled
                    && !r.cut
                {
                    r.settled = true;
                    match c.finish(None) {
                        Outcome::Verified => r.outcome = Some("verified"),
                        Outcome::Incomplete { at } => {
                            self.strayed(at, "").await;
                        }
                        Outcome::Diverged { at, heard } => {
                            self.strayed(at, &heard).await;
                        }
                    }
                }
                if let super::Status::Failed(code) = &status {
                    warn!("realtime: a response failed ({code})");
                }
                self.finish().await;
            }
            Event::Rejected { code, .. } => {
                if code.contains("active_response")
                    && let Some((job, _)) = self.asked.take()
                {
                    // It is busy answering: asked again once it is done.
                    self.queue.push_front(job);
                } else {
                    info!("realtime: the provider declined a request ({code})");
                }
            }
            Event::Failed(f) => self.fallback(f).await,
        }
    }

    /// The relay's audio, as far as the words verified allow.
    async fn release(&mut self) {
        let Some(r) = &mut self.active else { return };
        let Some(c) = &r.check else { return };
        let allowed = c.allowed_secs();
        let mut pieces = Vec::new();
        while r.released_secs < allowed
            && let Some(pcm) = r.held.pop_front()
        {
            r.released_secs += pcm.len() as f64 / f64::from(OUT_RATE);
            pieces.push(pcm);
        }
        if pieces.is_empty() {
            return;
        }
        r.pushed = true;
        let turn = r.turn.clone();
        for pcm in pieces {
            self.push_audio(pcm, turn.clone()).await;
        }
    }

    async fn push_audio(&mut self, pcm: Vec<i16>, turn: CancellationToken) {
        // When it plays: after what is queued ahead of it (`BotStarted` comes only for
        // the first of a run, and corrects this).
        let now = Instant::now();
        let start = self.queued_until.map_or(now, |q| q.max(now));
        self.queued_until =
            Some(start + Duration::from_secs_f64(pcm.len() as f64 / f64::from(OUT_RATE)));
        if let Some(r) = &mut self.active
            && r.heard_at.is_none()
        {
            r.heard_at = Some(start);
        }
        self.out
            .push(Frame::TtsAudio {
                audio: Audio::new(pcm, OUT_RATE),
                turn,
            })
            .await;
    }

    /// The reading left the answer at word `at`: cut it, and say the rest locally.
    async fn strayed(&mut self, at: usize, heard: &str) {
        let played = self.played_ms();
        let Some(r) = &mut self.active else { return };
        if r.cut {
            return;
        }
        let Some(c) = &r.check else { return };
        let rest = c.rest(at);
        let head = c.head(at);
        warn!(
            "realtime: the voice's reading left the agent's answer at word {at} of {}{}; the rest is said on this machine",
            c.total(),
            if heard.is_empty() {
                " (it stopped short)"
            } else {
                ""
            }
        );
        r.cut = true;
        r.outcome = Some(if heard.is_empty() {
            "incomplete"
        } else {
            "diverged"
        });
        r.held.clear();
        r.turn.cancel();
        if let Some(l) = &self.link {
            if !r.settled {
                l.cancel();
            }
            l.truncate(&r.item, played);
        }
        if !head.is_empty() {
            let on = self.p.on_turn.clone();
            on("assistant", head).await;
        }
        self.out.push(Frame::Interruption).await;
        if !rest.is_empty() {
            self.out.push(Frame::Speak(rest)).await;
        }
    }

    async fn consult(&mut self, call_id: String, request: String) {
        let since = {
            let mut w = self.p.shared.words.lock().unwrap();
            w.unanswered = None;
            std::mem::take(&mut w.since)
        };
        let text = consult_text(&since, &request);
        self.p
            .shared
            .words
            .lock()
            .unwrap()
            .forward
            .push_back(text.clone());
        self.consults.push_back(Consult {
            call_id,
            at: Instant::now(),
            anchor: self.speech_end,
        });
        let inject = self.p.inject.clone();
        tokio::spawn(async move { inject(vec![Frame::user_text(text, true)]).await });
    }

    /// The response is over: its audio ends, its record is kept, what waited on it runs.
    async fn finish(&mut self) {
        let Some(r) = self.active.take() else { return };
        if r.pushed && !r.cut {
            // the rest of a verified relay
            if !r.held.is_empty() {
                let turn = r.turn.clone();
                for pcm in r.held.iter().cloned() {
                    self.push_audio(pcm, turn.clone()).await;
                }
            }
            self.out.push(Frame::TtsStopped).await;
            self.playing = Some((r.turn.clone(), r.item.clone()));
        }
        if let Some(j) = &r.job
            && r.outcome == Some("verified")
        {
            let on = self.p.on_turn.clone();
            on("assistant", j.text.clone()).await;
        }
        let what = match &r.job {
            None => "talk",
            Some(j) if j.consulted.is_some() => "answer",
            Some(_) => "line",
        };
        let mut rec = json!({
            "what": what,
            "first_audio": secs_since(Some(r.started), r.heard_at),
        });
        match &r.job {
            None => {
                rec["first_sound"] = secs_since(self.speech_end, r.heard_at);
            }
            Some(j) => {
                rec["outcome"] = json!(r.outcome.unwrap_or("unheard"));
                rec["words"] = json!(r.check.as_ref().map_or(0, RelayCheck::total));
                rec["consult"] = secs_since(j.consulted, j.answered);
                rec["answer"] = secs_since(j.anchor, r.heard_at);
            }
        }
        self.record(rec);
        self.sync_auto();
        self.run_after().await;
        self.ask_next();
        if self.active.is_none()
            && self.asked.is_none()
            && self.queue.is_empty()
            && let Some((end, _)) = self.end.take()
        {
            self.close();
            self.out.forward(end, Direction::Down).await;
        }
    }

    async fn run_after(&mut self) {
        for a in std::mem::take(&mut self.after) {
            match a {
                After::HangUp => {
                    let h = self.p.hang_up.clone();
                    tokio::spawn(async move { h().await });
                }
                After::Hold => {
                    let h = self.p.hold.clone();
                    tokio::spawn(async move { h().await });
                }
            }
        }
    }

    /// While a change waits for its yes, the realtime voice does not answer on its own:
    /// the operator's words go to the agent and the gate (ADR 0017).
    fn sync_auto(&mut self) {
        let want = !self.p.gate.pending(&self.p.session_id);
        if want != self.auto
            && let Some(l) = &self.link
        {
            l.auto_reply(want);
            self.auto = want;
        }
    }

    async fn spend(&mut self, u: &Usage) {
        self.usage.add(u);
        let usd = self.p.params.provider.prices().map_or(0.0, |p| u.usd(&p));
        self.usd += usd;
        let over = self.p.params.on_usage.as_ref().is_some_and(|f| f(u, usd));
        if over {
            self.fallback(Failure::Cap).await;
        }
    }

    async fn tick(&mut self) {
        if self.mode() != LIVE {
            return;
        }
        let stale = self
            .asked
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed().as_secs_f64() > RELAY_START_SECS);
        if stale && let Some((job, _)) = self.asked.take() {
            warn!("realtime: an answer was not begun in time; it is said on this machine");
            self.out.push(Frame::Speak(job.text)).await;
            self.ask_next();
        }
        if let Some((_, at)) = &self.end
            && at.elapsed().as_secs_f64() > END_WAIT_SECS
            && let Some((end, _)) = self.end.take()
        {
            self.close();
            self.out.forward(end, Direction::Down).await;
        }
    }

    /// The session goes on with the local voice: one sentence, what was not said yet in
    /// the local voice, the operator's last words to the agent, a note on the waiting list.
    async fn fallback(&mut self, f: Failure) {
        if self.mode() == LOCAL {
            return;
        }
        warn!("realtime: {f}; the session goes on with the voice on this machine");
        self.p.shared.mode.store(LOCAL, Ordering::SeqCst);
        self.close();
        let mut to_say: Vec<String> = Vec::new();
        if let Some(mut r) = self.active.take() {
            if !r.cut {
                r.turn.cancel();
                self.out.push(Frame::Interruption).await;
                if let Some(c) = &r.check {
                    let rest = c.rest(c.verified());
                    if !rest.is_empty() {
                        to_say.push(rest);
                    }
                }
            }
            r.cut = true;
        }
        if let Some((job, _)) = self.asked.take() {
            to_say.push(job.text);
        }
        to_say.extend(self.queue.drain(..).map(|j| j.text));
        let sentence = if self.was_live {
            f.sentence()
        } else {
            match f {
                Failure::Cap => f.sentence(),
                _ => START_SENTENCE,
            }
        };
        self.out.push(Frame::Speak(sentence.to_string())).await;
        for t in to_say {
            self.out.push(Frame::Speak(t)).await;
        }
        let unanswered = self.p.shared.words.lock().unwrap().unanswered.take();
        if let Some(u) = unanswered {
            self.p
                .shared
                .words
                .lock()
                .unwrap()
                .forward
                .push_back(u.clone());
            let inject = self.p.inject.clone();
            tokio::spawn(async move { inject(vec![Frame::user_text(u, true)]).await });
        }
        if let Some(on) = &self.p.params.on_fallback {
            on(&f);
        }
        self.record(json!({"what": "fallback", "reason": f.kind()}));
        for (frame, d) in std::mem::take(&mut self.held) {
            self.out.forward(frame, d).await;
        }
        if let Some((end, _)) = self.end.take() {
            self.out.forward(end, Direction::Down).await;
        }
    }

    fn record(&self, mut rec: Value) {
        if let Some(m) = rec.as_object_mut() {
            m.insert("provider".into(), json!(self.p.params.provider.name()));
            m.insert("model".into(), json!(self.p.params.provider.model()));
            m.insert(
                "usd".into(),
                json!((self.usd * 10_000.0).round() / 10_000.0),
            );
        }
        self.p.probe.realtime(rec);
    }
}

/// What the agent is given for a consult: the operator's own words, as Ghost heard them,
/// and the realtime voice's reading of them, marked as that.
pub fn consult_text(since: &[String], request: &str) -> String {
    let request = request.trim();
    let words = since.join(" ");
    if words.trim().is_empty() {
        format!(
            "[The realtime voice passed on a request Ghost did not hear itself: \"{request}\". \
             Answer it as the operator's request, in a few spoken sentences.]"
        )
    } else {
        format!(
            "{words}\n\n[The realtime voice passed this on as: \"{request}\". The words above are \
             the operator's own, as Ghost heard them.]"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consult_carries_the_operators_words_and_marks_the_voices_reading() {
        let t = consult_text(
            &["what's open on ghost".into()],
            "list open issues in ghost",
        );
        assert!(t.starts_with("what's open on ghost"));
        assert!(t.contains("passed this on as: \"list open issues in ghost\""));
        let t = consult_text(&[], "list open issues");
        assert!(t.contains("did not hear itself"));
    }

    #[derive(Default)]
    struct Heard(Mutex<usize>);

    impl RealtimeLink for Heard {
        fn audio(&self, pcm: &[i16]) {
            *self.0.lock().unwrap() += pcm.len();
        }
        fn cancel(&self) {}
        fn truncate(&self, _item: &str, _played_ms: u64) {}
        fn answer(&self, _call_id: &str, _output: &str) {}
        fn say(&self, _text: &str, _tag: &str) {}
        fn auto_reply(&self, _on: bool) {}
        fn ask(&self, _text: &str) {}
        fn close(&self) {}
    }

    #[tokio::test]
    async fn the_uplink_sends_nothing_on_hold_and_holds_back_what_it_hears_over_ghost() {
        let shared = Arc::new(Shared::default());
        let link = Arc::new(Heard::default());
        *shared.link.lock().unwrap() = Some(link.clone());
        let hold = Arc::new(HoldState::default());
        let speaking = Speaking::new();
        let mut up = Uplink::new(shared.clone(), hold.clone(), speaking.clone());
        let (out, _rx) = Out::capture();
        let frame = || Frame::AudioIn(Audio::new(vec![100; 320], 16_000));
        let sent = || *link.0.lock().unwrap();
        // Connecting: nothing leaves.
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 0);
        shared.mode.store(LIVE, Ordering::SeqCst);
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320);
        // On hold, nothing the room says leaves the machine.
        hold.hold();
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320);
        hold.release();
        // Over Ghost's voice it is held back, and sent only once Ghost's rule says it is
        // the operator talking over it.
        speaking.set(true);
        up.process(frame(), Direction::Down, &out).await;
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320);
        up.process(Frame::Interruption, Direction::Up, &out).await;
        assert_eq!(sent(), 320 * 3);
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320 * 4);
        // Held back and never a barge-in: dropped when Ghost stops.
        speaking.set(false);
        up.process(frame(), Direction::Down, &out).await;
        speaking.set(true);
        shared.barged.store(false, Ordering::SeqCst);
        up.process(frame(), Direction::Down, &out).await;
        speaking.set(false);
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320 * 6);
        // Local: nothing leaves.
        shared.mode.store(LOCAL, Ordering::SeqCst);
        up.process(frame(), Direction::Down, &out).await;
        assert_eq!(sent(), 320 * 6);
    }

    #[tokio::test]
    async fn the_gate_keeps_a_spoken_turn_from_the_agent_and_lets_the_rest_through() {
        let shared = Arc::new(Shared::default());
        shared.mode.store(LIVE, Ordering::SeqCst);
        let mut gate = Gate::new(shared.clone());
        let (out, mut rx) = Out::capture();
        // A spoken turn: the realtime voice answers it; the agent hears it later.
        shared.closed("how are you");
        gate.process(Frame::user_text("how are you", true), Direction::Down, &out)
            .await;
        // A typed turn and a note go through.
        gate.process(Frame::typed_text("what's open"), Direction::Down, &out)
            .await;
        gate.process(
            Frame::user_text("[system] note", false),
            Direction::Down,
            &out,
        )
        .await;
        let got = crate::frames::drain(&mut rx).await;
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(shared.words.lock().unwrap().since, vec!["how are you"]);
        // A turn that answers a change waiting for its yes goes to the agent.
        shared.words.lock().unwrap().confirm_next = true;
        shared.closed("yes");
        gate.process(Frame::user_text("yes", true), Direction::Down, &out)
            .await;
        assert_eq!(crate::frames::drain(&mut rx).await.len(), 1);
        // Once local, every turn goes to the agent.
        shared.mode.store(LOCAL, Ordering::SeqCst);
        shared.closed("and now");
        gate.process(Frame::user_text("and now", true), Direction::Down, &out)
            .await;
        assert_eq!(crate::frames::drain(&mut rx).await.len(), 1);
    }
}
