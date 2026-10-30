//! Where a turn's time goes: one timeline per turn, timings only, never text.
//!
//! [`LatencyObserver`] is attached to every session as the pipeline's [`Observer`]. It
//! sees every frame on its first push (from the stage that made it), keeps the moments of
//! the turn on the pipeline clock, and takes a few durations straight from Ghost's own
//! stages through [`LatencyProbe`] (the turn hold, interceptors, the confirmation
//! classifier, the steps inside a tool). When the turn is over it logs one INFO line and
//! appends one JSON record to `latency.jsonl`, the same record `ghostd latency` reads.
//!
//! A voice turn, end to end:
//!
//! ```text
//! user stops talking (VAD stop minus its stop_secs)
//!   hear   VAD silence, STT, interceptors + confirmation, Smart Turn, our grace hold
//!   llm    each model call: start, first byte, first tool call, end, tokens, cache
//!   tools  each function call by name (read tools hit the app behind them)
//!   tts    last model call ends -> narration released -> first audio -> transport
//! Ghost's voice starts (BotStarted)
//! ```
//!
//! A reply the operator talked over and cut off carries `barge_in`: from when they began
//! (the VAD start minus its start_secs) to the interruption.
//!
//! What is never recorded: transcripts, model text, tool arguments or results, persona or
//! conversation names. The record holds numbers, counts, model ids, tool and operation
//! names (identifiers in Ghost's own code), classifier names and verdicts. Passive audio
//! never reaches a session, so it never reaches this module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use tracing::{error, info, warn};

use crate::frames::{Clock, Direction, Frame, Metric, Observer};

pub const RECORD_VERSION: u64 = 1;
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// A response that calls only these still speaks its text (`narration`).
const SPOKEN_WITH: &[&str] = &["hang_up", "hold"];

/// The stage names the observer tells services apart by.
pub const LLM: &str = "llm";
pub const STT: &str = "stt";
pub const TTS: &str = "tts";

// --- what Ghost's own stages report --------------------------------------------------

/// Durations Ghost's stages measure themselves. Every method is a no-op by default, so a
/// stage built without a session (tests, the wake listener) needs no checks.
pub trait LatencyProbe: Send + Sync {
    /// The turn model said done and the grace hold ran `secs` before the turn ended.
    fn held(&self, _secs: f64) {}
    /// The operator went on during the grace hold; the turn stayed open.
    fn kept_open(&self) {}
    /// An answer started during the grace hold: `started`, then `kept` (it was the
    /// answer), `withdrawn` (the operator went on), `missed` (the words changed) or
    /// `tool` (it reached a tool call and the confirmed turn ran again).
    fn speculation(&self, _outcome: &str) {}
    /// An interceptor (`name` is its code name) handled the turn in `secs`.
    fn intercepted(&self, _name: &str, _secs: f64) {}
    /// The interceptors looked at a transcript and let it through, taking `secs`.
    fn interceptors(&self, _secs: f64) {}
    /// A pending change met a transcript: which classifier decided, how long it took,
    /// and the whole call including any execution.
    fn confirmation(
        &self,
        _classifier: &str,
        _classify_secs: f64,
        _verdict: &str,
        _total_secs: f64,
    ) {
    }
    /// A step inside a function call (`prepare`, `submit`, `request`, `read`).
    fn tool_step(&self, _call_id: &str, _step: &str, _secs: f64) {}
    /// How a function call ended: `done`, `needs_confirmation`, `error`, `ok`.
    fn tool_status(&self, _call_id: &str, _status: &str) {}
    /// The operation a `change` call named, as `ability.operation`.
    fn tool_operation(&self, _call_id: &str, _operation: &str) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NullProbe;

impl LatencyProbe for NullProbe {}

pub fn null_probe() -> Arc<dyn LatencyProbe> {
    Arc::new(NullProbe)
}

// --- one turn ----------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct LlmCall {
    start: f64,
    ttfb: Option<f64>,
    first_text: Option<f64>,
    first_tool: Option<f64>,
    end: Option<f64>,
    model: Option<String>,
    tools: Vec<String>,
    thinking: bool,
    text: bool,
    usage: Option<IndexMap<&'static str, u64>>,
}

#[derive(Debug, Clone)]
struct ToolCallTiming {
    name: String,
    start: f64,
    end: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Turn {
    index: u64,
    kind: &'static str, // voice | text | opening | system
    opened: f64,
    anchor: Option<f64>,
    vad_stop: Option<f64>,
    stop_secs: f64,
    transcript: Option<f64>,
    stt_model: Option<f64>,
    segments: u64,
    released: Option<f64>,
    llm: Vec<LlmCall>,
    tools: IndexMap<String, ToolCallTiming>,
    tts_start: Option<f64>,
    tts_ttfb: Option<f64>,
    first_audio: Option<f64>,
    bot_started: Option<f64>,
    bot_stopped: Option<f64>,
    /// The acknowledgment's first sample at the speaker (`crate::acks`).
    ack: Option<f64>,
    spoken_direct: bool,
    over_at: Option<f64>,
    barge_in: Option<f64>,
    smart_turn: Vec<Value>,
    flags: BTreeSet<String>,
    // From the probe
    hold: f64,
    kept_open: u64,
    speculation: IndexMap<String, u64>,
    early_ttfb: Option<f64>,
    pre: f64,
    intercepted: Option<String>,
    confirm: Option<Value>,
    steps: IndexMap<String, IndexMap<String, f64>>,
    status: IndexMap<String, String>,
    operations: IndexMap<String, String>,
}

impl Turn {
    fn new(index: u64, kind: &'static str, t: f64) -> Self {
        Self {
            index,
            kind,
            opened: t,
            anchor: None,
            vad_stop: None,
            stop_secs: 0.0,
            transcript: None,
            stt_model: None,
            segments: 0,
            released: None,
            llm: Vec::new(),
            tools: IndexMap::new(),
            tts_start: None,
            tts_ttfb: None,
            first_audio: None,
            bot_started: None,
            bot_stopped: None,
            ack: None,
            spoken_direct: false,
            over_at: None,
            barge_in: None,
            smart_turn: Vec::new(),
            flags: BTreeSet::new(),
            hold: 0.0,
            kept_open: 0,
            speculation: IndexMap::new(),
            early_ttfb: None,
            pre: 0.0,
            intercepted: None,
            confirm: None,
            steps: IndexMap::new(),
            status: IndexMap::new(),
            operations: IndexMap::new(),
        }
    }

    fn open_llm(&mut self) -> Option<&mut LlmCall> {
        self.llm.iter_mut().rev().find(|c| c.end.is_none())
    }

    fn busy(&self) -> bool {
        self.llm.iter().any(|c| c.end.is_none()) || self.tools.values().any(|t| t.end.is_none())
    }

    fn happened(&self) -> bool {
        !self.llm.is_empty()
            || self.bot_started.is_some()
            || self.intercepted.is_some()
            || !self.tools.is_empty()
    }

    /// Ghost is saying this turn's reply.
    fn speaking(&self) -> bool {
        self.bot_started.is_some() && self.bot_stopped.is_none()
    }
}

// --- the observer --------------------------------------------------------------------

pub type Sink = Arc<dyn Fn(&Value) + Send + Sync>;

struct State {
    turn: Option<Turn>,
    count: u64,
    last_t: f64,
    opening: bool,
    pipeline_t0: Option<f64>,
    records: Vec<Value>,
}

/// Builds one timeline per turn from the frames and the probe; see the module doc.
///
/// `sink` gets each finished record (the harness keeps them); `log_path` is where the
/// JSONL goes (None: nowhere). Nothing here may raise into the pipeline.
pub struct LatencyObserver {
    pub session_id: String,
    pub origin: String,
    pub model: Option<String>,
    path: Option<PathBuf>,
    max_bytes: u64,
    sink: Option<Sink>,
    log_line: bool,
    built: Instant,
    state: Mutex<State>,
}

impl LatencyObserver {
    pub fn new(session_id: &str, origin: &str, model: Option<&str>) -> Self {
        Self {
            session_id: session_id.to_string(),
            origin: origin.to_string(),
            model: model.map(String::from),
            path: None,
            max_bytes: DEFAULT_MAX_BYTES,
            sink: None,
            log_line: true,
            built: Instant::now(),
            state: Mutex::new(State {
                turn: None,
                count: 0,
                last_t: 0.0,
                opening: true,
                pipeline_t0: None,
                records: Vec::new(),
            }),
        }
    }

    pub fn with_log(mut self, path: Option<PathBuf>, max_bytes: u64) -> Self {
        self.path = path;
        self.max_bytes = max_bytes;
        self
    }

    pub fn with_sink(mut self, sink: Sink) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn with_log_line(mut self, on: bool) -> Self {
        self.log_line = on;
        self
    }

    /// The pipeline's clock, once the session has one: its zero against when this
    /// observer was built is how long the session took to start (`build`).
    pub fn set_clock(&self, clock: &Clock) {
        let secs = clock
            .zero()
            .saturating_duration_since(self.built)
            .as_secs_f64();
        self.state.lock().unwrap().pipeline_t0 = Some(secs);
    }

    /// Straight to a voice turn, with no opening (a test, or a session that greeted
    /// elsewhere).
    pub fn skip_opening(&self) {
        let mut s = self.state.lock().unwrap();
        s.opening = false;
        s.turn = None;
    }

    pub fn records(&self) -> Vec<Value> {
        self.state.lock().unwrap().records.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn on_frame_inner(&self, frame: &Frame, src: &str, t: f64) {
        let mut s = self.lock();
        s.last_t = s.last_t.max(t);
        if s.opening && s.turn.is_none() {
            s.turn = Some(self.new_turn(&mut s, "opening", t));
        }
        match frame {
            Frame::ClientConnected => {
                if let Some(turn) = &mut s.turn
                    && turn.kind == "opening"
                    && turn.bot_started.is_none()
                {
                    turn.anchor = Some(t); // the phone's opening is timed from the peer connecting
                }
            }
            Frame::VadStarted { start_secs } => {
                if let Some(turn) = &mut s.turn {
                    if turn.kind == "voice" && turn.released.is_none() {
                        turn.flags.insert("_segment_open".into());
                    }
                    if turn.speaking() && !turn.flags.contains("_over") {
                        // A segment over Ghost's voice; the latest one is what a barge-in
                        // cut in with. Timed from when the speech began, as a turn is from
                        // when it ended.
                        turn.flags.insert("_over".into());
                        turn.over_at = Some(t - f64::from(*start_secs));
                    }
                }
            }
            Frame::VadStopped { stop_secs } => {
                if let Some(turn) = &mut s.turn
                    && turn.speaking()
                {
                    turn.flags.remove("_over");
                }
                self.vad_stop(&mut s, t, f64::from(*stop_secs));
            }
            Frame::UserStarted => self.user_started(&mut s, t),
            Frame::Transcript(_) if src == STT => {
                // Only the open voice turn's own words; a transcript while Ghost speaks is
                // echo or a barge-in, and a barge-in opens its own turn.
                if let Some(turn) = &mut s.turn
                    && turn.kind == "voice"
                    && turn.released.is_none()
                {
                    turn.transcript = Some(t);
                }
            }
            Frame::UserStopped => {
                if let Some(turn) = &mut s.turn
                    && turn.kind == "voice"
                    && turn.released.is_none()
                {
                    turn.released = Some(t);
                }
            }
            Frame::ThinkingStart => {
                if s.turn
                    .as_ref()
                    .is_none_or(|turn| turn.bot_started.is_some())
                {
                    let turn = self.new_turn(&mut s, "text", t);
                    self.replace(&mut s, turn);
                }
            }
            Frame::ReplyCut => {
                if let Some(turn) = &mut s.turn {
                    turn.flags.insert("cut_off".into());
                }
            }
            Frame::LlmStart if src == LLM => {
                let needs_new = match &s.turn {
                    None => true,
                    Some(turn) => turn.bot_started.is_some() && !turn.busy(),
                };
                if needs_new {
                    let turn = self.new_turn(&mut s, "system", t);
                    self.replace(&mut s, turn);
                }
                let model = self.model.clone();
                if let Some(turn) = &mut s.turn {
                    let early = turn.early_ttfb.take();
                    turn.llm.push(LlmCall {
                        start: t,
                        model,
                        ttfb: early,
                        ..LlmCall::default()
                    });
                }
            }
            Frame::LlmThought if src == LLM => {
                if let Some(call) = s.turn.as_mut().and_then(Turn::open_llm) {
                    call.thinking = true;
                }
            }
            Frame::LlmText(_) if src == LLM => {
                if let Some(call) = s.turn.as_mut().and_then(Turn::open_llm) {
                    call.text = true;
                    if call.first_text.is_none() {
                        call.first_text = Some(t);
                    }
                }
            }
            Frame::FunctionCallsStarted(names) if src == LLM => {
                let mut narrated = false;
                if let Some(call) = s.turn.as_mut().and_then(Turn::open_llm)
                    && call.first_tool.is_none()
                {
                    call.first_tool = Some(t);
                    call.tools = names.clone();
                    narrated =
                        call.text && !names.iter().all(|n| SPOKEN_WITH.contains(&n.as_str()));
                }
                if narrated && let Some(turn) = &mut s.turn {
                    turn.flags.insert("narration_dropped".into());
                }
            }
            Frame::LlmEnd if src == LLM => {
                if let Some(call) = s.turn.as_mut().and_then(Turn::open_llm) {
                    call.end = Some(t);
                }
            }
            Frame::FunctionCallInProgress { name, id } => {
                if let Some(turn) = &mut s.turn
                    && !turn.tools.contains_key(id)
                {
                    turn.tools.insert(
                        id.clone(),
                        ToolCallTiming {
                            name: name.clone(),
                            start: t,
                            end: None,
                        },
                    );
                }
            }
            Frame::FunctionCallResult { id, .. } => {
                if let Some(call) = s.turn.as_mut().and_then(|turn| turn.tools.get_mut(id))
                    && call.end.is_none()
                {
                    call.end = Some(t);
                }
            }
            Frame::Metrics(m) => self.metrics(&mut s, m, src),
            Frame::Speak(_) => {
                if let Some(turn) = &mut s.turn {
                    turn.spoken_direct = true;
                }
            }
            Frame::TtsStarted if src == TTS => {
                if let Some(turn) = &mut s.turn
                    && turn.tts_start.is_none()
                    && turn.bot_started.is_none()
                {
                    turn.tts_start = Some(t);
                }
            }
            Frame::TtsAudio { .. } if src == TTS => {
                if let Some(turn) = &mut s.turn
                    && turn.first_audio.is_none()
                    && turn.bot_started.is_none()
                {
                    turn.first_audio = Some(t);
                }
            }
            Frame::BotStarted => {
                if let Some(turn) = &mut s.turn
                    && turn.bot_started.is_none()
                {
                    turn.bot_started = Some(t);
                }
            }
            Frame::AckPlayed => {
                if let Some(turn) = &mut s.turn
                    && turn.ack.is_none()
                {
                    turn.ack = Some(t);
                }
            }
            Frame::BotStopped => {
                let finish = match &mut s.turn {
                    Some(turn) if turn.bot_started.is_some() && turn.bot_stopped.is_none() => {
                        turn.bot_stopped = Some(t);
                        !turn.busy()
                    }
                    _ => false,
                };
                if finish {
                    self.finish(&mut s);
                }
            }
            // An End is first pushed at the pipeline's source, before the goodbye queued
            // ahead of it has played; the session's end calls flush() instead.
            _ => {}
        }
    }

    fn metrics(&self, s: &mut State, m: &Metric, src: &str) {
        let Some(turn) = &mut s.turn else { return };
        match m {
            Metric::Ttfb { value, model } => {
                if src == LLM {
                    let started = turn.speculation.get("started").copied().unwrap_or(0) > 0;
                    let call = match turn.llm.iter_mut().rev().find(|c| c.end.is_none()) {
                        Some(c) => Some(c),
                        None => turn.llm.last_mut(),
                    };
                    match call {
                        Some(call) if call.ttfb.is_none() => {
                            call.ttfb = Some(*value);
                            if model.is_some() {
                                call.model = model.clone();
                            }
                        }
                        None if started => {
                            // A speculative answer is held until the turn is confirmed, so
                            // its metrics arrive before the call that carries them.
                            turn.early_ttfb = Some(*value);
                        }
                        _ => {}
                    }
                } else if src == TTS {
                    if turn.tts_ttfb.is_none() && turn.bot_started.is_none() {
                        turn.tts_ttfb = Some(*value);
                    }
                } else if src == STT {
                    turn.stt_model = Some(*value);
                }
            }
            Metric::Usage {
                input,
                output,
                cache_read,
                cache_write,
                model,
            } => {
                // Reported just after the response's end frame: the latest call without
                // usage is the one it belongs to.
                if let Some(call) = turn.llm.iter_mut().rev().find(|c| c.usage.is_none()) {
                    let mut u = IndexMap::new();
                    u.insert("in", *input);
                    u.insert("out", *output);
                    u.insert("cache_read", *cache_read);
                    u.insert("cache_write", *cache_write);
                    call.usage = Some(u);
                    if model.is_some() {
                        call.model = model.clone();
                    }
                }
            }
            Metric::Turn {
                complete,
                probability,
                ms,
            } => {
                turn.smart_turn.push(json!({
                    "complete": complete,
                    "p": round(*probability, 3),
                    "ms": round(*ms, 1),
                }));
            }
        }
    }

    // --- voice bookkeeping -----------------------------------------------------------

    fn user_started(&self, s: &mut State, t: f64) {
        if let Some(turn) = &s.turn
            && turn.kind == "voice"
            && turn.released.is_none()
            && turn.bot_started.is_none()
        {
            return; // the same turn, seen again (both broadcast copies)
        }
        if let Some(turn) = &mut s.turn {
            if turn.bot_started.is_none() && turn.happened() {
                turn.flags.insert("interrupted".into());
            } else if turn.speaking() {
                turn.flags.insert("barged_in".into());
                if let Some(over) = turn.over_at {
                    turn.barge_in = Some(t - over);
                }
            }
        }
        let turn = self.new_turn(s, "voice", t);
        self.replace(s, turn);
    }

    fn vad_stop(&self, s: &mut State, t: f64, stop_secs: f64) {
        let Some(turn) = &mut s.turn else { return };
        if turn.kind != "voice" || turn.released.is_some() {
            return;
        }
        if turn.flags.contains("_segment_open") || turn.vad_stop.is_none() {
            turn.flags.remove("_segment_open");
            turn.segments += 1;
            turn.vad_stop = Some(t);
        } else if let Some(v) = turn.vad_stop {
            turn.vad_stop = Some(v.min(t)); // the other broadcast copy of the same stop
        }
        turn.stop_secs = stop_secs;
    }

    // --- turn lifecycle ----------------------------------------------------------------

    fn new_turn(&self, s: &mut State, kind: &'static str, t: f64) -> Turn {
        s.opening = false;
        s.count += 1;
        Turn::new(s.count, kind, t)
    }

    fn replace(&self, s: &mut State, turn: Turn) {
        self.finish(s);
        s.turn = Some(turn);
    }

    fn finish(&self, s: &mut State) {
        let Some(turn) = s.turn.take() else { return };
        if !turn.happened() {
            return;
        }
        let build_secs = if turn.kind == "opening" {
            s.pipeline_t0
        } else {
            None
        };
        let record = build_record(
            &turn,
            &self.session_id,
            &self.origin,
            self.model.as_deref(),
            build_secs,
        );
        s.records.push(record.clone());
        if self.log_line {
            info!("{}", format_line(&record));
        }
        if let Some(sink) = &self.sink {
            sink(&record);
        }
        if let Some(path) = &self.path {
            append_record(path, &record, self.max_bytes);
        }
    }

    /// Finish whatever turn is open (the session is over).
    pub fn flush(&self) {
        let mut s = self.lock();
        self.finish(&mut s);
    }
}

impl Observer for LatencyObserver {
    fn on_frame(&self, frame: &Frame, source: &'static str, at: f64, _direction: Direction) {
        // Measuring must never break a session.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.on_frame_inner(frame, source, at)
        }));
        if r.is_err() {
            error!("latency: observer error");
        }
    }
}

impl LatencyProbe for LatencyObserver {
    fn held(&self, secs: f64) {
        if let Some(turn) = &mut self.lock().turn {
            turn.hold += secs;
        }
    }

    fn kept_open(&self) {
        if let Some(turn) = &mut self.lock().turn {
            turn.kept_open += 1;
        }
    }

    fn speculation(&self, outcome: &str) {
        if let Some(turn) = &mut self.lock().turn {
            *turn.speculation.entry(outcome.to_string()).or_insert(0) += 1;
        }
    }

    fn intercepted(&self, name: &str, secs: f64) {
        if let Some(turn) = &mut self.lock().turn {
            turn.intercepted = Some(name.to_string());
            turn.pre += secs;
        }
    }

    fn interceptors(&self, secs: f64) {
        if let Some(turn) = &mut self.lock().turn {
            turn.pre += secs;
        }
    }

    fn confirmation(&self, classifier: &str, classify_secs: f64, verdict: &str, total_secs: f64) {
        if let Some(turn) = &mut self.lock().turn {
            turn.pre += total_secs;
            turn.confirm = Some(json!({
                "classifier": classifier,
                "classify": round(classify_secs, 4),
                "verdict": verdict,
                "total": round(total_secs, 4),
            }));
        }
    }

    fn tool_step(&self, call_id: &str, step: &str, secs: f64) {
        if let Some(turn) = &mut self.lock().turn {
            turn.steps
                .entry(call_id.to_string())
                .or_default()
                .insert(step.to_string(), round(secs, 4));
        }
    }

    fn tool_status(&self, call_id: &str, status: &str) {
        if let Some(turn) = &mut self.lock().turn {
            turn.status.insert(call_id.to_string(), status.to_string());
        }
    }

    fn tool_operation(&self, call_id: &str, operation: &str) {
        if let Some(turn) = &mut self.lock().turn {
            turn.operations
                .insert(call_id.to_string(), operation.to_string());
        }
    }
}

// --- the record ----------------------------------------------------------------------

/// Python's `round(x, nd)`: half to even on the decimal.
pub fn round(x: f64, nd: i32) -> f64 {
    let scale = 10f64.powi(nd);
    (x * scale).round_ties_even() / scale
}

fn r(x: Option<f64>) -> Value {
    match x {
        Some(v) => json!(round(v, 3)),
        None => Value::Null,
    }
}

fn union(intervals: &mut [(f64, f64)]) -> f64 {
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (mut total, mut end) = (0.0, f64::NEG_INFINITY);
    for (s, e) in intervals.iter() {
        if *e <= end {
            continue;
        }
        total += e - s.max(end);
        end = *e;
    }
    total
}

fn now_stamp() -> String {
    let now = chrono::Utc::now();
    format!(
        "{}.{:02}Z",
        now.format("%Y-%m-%dT%H:%M:%S"),
        now.timestamp_subsec_millis() / 10
    )
}

/// The JSON record of one finished turn. Times are seconds; `at` fields are from the
/// turn's anchor (the moment the operator stopped talking, for a voice turn).
pub fn build_record(
    turn: &Turn,
    session_id: &str,
    origin: &str,
    model: Option<&str>,
    build_secs: Option<f64>,
) -> Value {
    let voice = turn.kind == "voice";
    let anchor = match (voice, turn.vad_stop, turn.anchor) {
        (true, Some(vs), _) => vs - turn.stop_secs,
        (_, _, Some(a)) => a,
        _ => turn.opened,
    };
    let end = turn.bot_started;
    // Model calls and tools on the way to Ghost's voice (all of them if it never spoke).
    let horizon = end.unwrap_or(f64::INFINITY);
    let calls: Vec<&LlmCall> = turn.llm.iter().filter(|c| c.start < horizon).collect();
    let tools: Vec<&ToolCallTiming> = turn.tools.values().filter(|t| t.start < horizon).collect();
    let rel = |x: Option<f64>| -> Value {
        match x {
            Some(v) => json!(round(v - anchor, 3)),
            None => Value::Null,
        }
    };

    let mut rec = Map::new();
    rec.insert("v".into(), json!(RECORD_VERSION));
    rec.insert("ts".into(), json!(now_stamp()));
    rec.insert("session".into(), json!(session_id));
    rec.insert("origin".into(), json!(origin));
    rec.insert("turn".into(), json!(turn.index));
    rec.insert("kind".into(), json!(turn.kind));
    rec.insert(
        "model".into(),
        json!(
            turn.llm
                .iter()
                .find_map(|c| c.model.clone())
                .or_else(|| model.map(String::from))
        ),
    );
    rec.insert("reply".into(), r(end.map(|e| e - anchor)));
    rec.insert("spoke".into(), json!(end.is_some()));
    if let Some(a) = turn.ack {
        rec.insert("ack".into(), r(Some(a - anchor)));
    }
    if let Some(b) = build_secs {
        rec.insert("build".into(), r(Some(b)));
    }

    let mut hear_total = None;
    if voice {
        let mut released = turn.released;
        if released.is_none()
            && turn.intercepted.is_some()
            && let Some(tr) = turn.transcript
        {
            // The interceptor took the transcript, so the turn never ended the usual
            // way: Ghost heard it once the interceptor had decided.
            released = Some(tr + turn.pre);
        }
        hear_total = released.map(|rel| rel - anchor);
        let stt = match (turn.transcript, turn.vad_stop) {
            (Some(tr), Some(vs)) if tr >= vs => Some(tr - vs),
            _ => None,
        };
        let turn_wait = match (released, turn.transcript) {
            (Some(rel), Some(tr)) => {
                Some((rel - tr.max(turn.vad_stop.unwrap_or(0.0)) - turn.hold).max(0.0))
            }
            _ => None,
        };
        let mut hear = Map::new();
        hear.insert("total".into(), r(hear_total));
        hear.insert("vad".into(), r(Some(turn.stop_secs)));
        hear.insert("stt".into(), r(stt));
        hear.insert("stt_model".into(), r(turn.stt_model));
        hear.insert("pre".into(), r(Some(turn.pre)));
        hear.insert("turn".into(), r(turn_wait));
        hear.insert("hold".into(), r(Some(turn.hold)));
        hear.insert("segments".into(), json!(turn.segments));
        hear.insert("kept_open".into(), json!(turn.kept_open));
        if !turn.speculation.is_empty() {
            hear.insert("speculation".into(), json!(turn.speculation));
        }
        if !turn.smart_turn.is_empty() {
            hear.insert("smart_turn".into(), json!(turn.smart_turn));
        }
        rec.insert("hear".into(), Value::Object(hear));
    }
    if let Some(c) = &turn.confirm {
        rec.insert("confirm".into(), c.clone());
    }
    if let Some(i) = &turn.intercepted {
        rec.insert("intercepted".into(), json!(i));
    }

    let mut llm_calls = Vec::new();
    for c in &turn.llm {
        let mut entry = Map::new();
        entry.insert("at".into(), rel(Some(c.start)));
        entry.insert("ttfb".into(), r(c.ttfb));
        entry.insert("dur".into(), r(c.end.map(|e| e - c.start)));
        entry.insert("first_tool".into(), r(c.first_tool.map(|f| f - c.start)));
        entry.insert("first_text".into(), r(c.first_text.map(|f| f - c.start)));
        entry.insert("tools".into(), json!(c.tools));
        entry.insert("text".into(), json!(c.text));
        entry.insert("thinking".into(), json!(c.thinking));
        if let Some(u) = &c.usage {
            for (k, v) in u {
                entry.insert((*k).to_string(), json!(v));
            }
        }
        llm_calls.push(Value::Object(entry));
    }
    let clip = |s: f64, e: Option<f64>| -> (f64, f64) { (s, e.unwrap_or(horizon).min(horizon)) };
    let mut llm_spans: Vec<(f64, f64)> = calls
        .iter()
        .filter(|c| c.end.is_some() || end.is_some())
        .map(|c| clip(c.start, c.end))
        .collect();
    let llm_total = union(&mut llm_spans.clone());
    rec.insert(
        "llm".into(),
        if llm_calls.is_empty() {
            Value::Null
        } else {
            json!({"total": round(llm_total, 3), "calls": llm_calls})
        },
    );

    let mut tool_calls = Vec::new();
    for (cid, tc) in &turn.tools {
        let mut entry = Map::new();
        entry.insert("name".into(), json!(tc.name));
        entry.insert("at".into(), rel(Some(tc.start)));
        entry.insert("secs".into(), r(tc.end.map(|e| e - tc.start)));
        if let Some(op) = turn.operations.get(cid) {
            entry.insert("op".into(), json!(op));
        }
        if let Some(st) = turn.status.get(cid) {
            entry.insert("status".into(), json!(st));
        }
        if let Some(steps) = turn.steps.get(cid) {
            entry.insert("steps".into(), json!(steps));
        }
        tool_calls.push(Value::Object(entry));
    }
    // Tool time on the path is what tools add beyond the model calls they overlap.
    let tool_spans: Vec<(f64, f64)> = tools
        .iter()
        .filter(|tc| tc.end.is_some() || end.is_some())
        .map(|tc| clip(tc.start, tc.end))
        .collect();
    llm_spans.extend(tool_spans);
    let tools_total = union(&mut llm_spans) - llm_total;
    rec.insert(
        "tools".into(),
        if tool_calls.is_empty() {
            Value::Null
        } else {
            json!({"total": round(tools_total, 3), "calls": tool_calls})
        },
    );

    let mut tts_total = None;
    if let Some(end) = end {
        let last_llm_end = calls
            .iter()
            .filter_map(|c| c.end)
            .fold(None, |m: Option<f64>, e| Some(m.map_or(e, |m| m.max(e))));
        let start_tts = if let Some(l) = last_llm_end {
            l
        } else if voice && hear_total.is_some() {
            anchor + hear_total.unwrap_or(0.0)
        } else {
            turn.anchor.unwrap_or(turn.opened)
        };
        if start_tts <= end {
            tts_total = Some(end - start_tts);
        }
        let mut tts = Map::new();
        tts.insert("total".into(), r(tts_total));
        tts.insert("wait".into(), r(turn.tts_start.map(|ts| ts - start_tts)));
        tts.insert(
            "synth".into(),
            r(match (turn.first_audio, turn.tts_start) {
                (Some(fa), Some(ts)) => Some(fa - ts),
                _ => None,
            }),
        );
        tts.insert("ttfb".into(), r(turn.tts_ttfb));
        tts.insert("out".into(), r(turn.first_audio.map(|fa| end - fa)));
        tts.insert("direct".into(), json!(turn.spoken_direct));
        rec.insert("tts".into(), Value::Object(tts));
        if let Some(bs) = turn.bot_stopped {
            rec.insert("speech".into(), r(Some(bs - end)));
        }
        let parts = hear_total.unwrap_or(0.0) + llm_total + tools_total + tts_total.unwrap_or(0.0);
        rec.insert("other".into(), r(Some((end - anchor) - parts)));
    }
    if let Some(b) = turn.barge_in {
        rec.insert("barge_in".into(), r(Some(b)));
    }
    let flags: Vec<&String> = turn.flags.iter().filter(|f| !f.starts_with('_')).collect();
    if !flags.is_empty() {
        rec.insert("flags".into(), json!(flags));
    }
    Value::Object(rec)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn int(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_u64).unwrap_or(0)
}

/// Share of input tokens read from the prompt cache across `calls`.
pub fn cache_share(calls: &[Value]) -> Option<f64> {
    let read: u64 = calls.iter().map(|c| int(c.get("cache_read"))).sum();
    let total: u64 = calls
        .iter()
        .map(|c| int(c.get("in")) + int(c.get("cache_read")) + int(c.get("cache_write")))
        .sum();
    (total > 0).then(|| read as f64 / total as f64)
}

fn calls_of(rec: &Value) -> Vec<Value> {
    rec.get("llm")
        .and_then(|l| l.get("calls"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One log line per turn, timings only.
pub fn format_line(rec: &Value) -> String {
    let mut head = format!(
        "latency: turn {} {} {}",
        rec["turn"],
        rec["origin"].as_str().unwrap_or(""),
        rec["kind"].as_str().unwrap_or("")
    );
    let reply = num(rec.get("reply"));
    match reply {
        None => head.push_str(" no reply"),
        Some(r) => head.push_str(&format!(" reply {r:.2}s")),
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(hear) = rec.get("hear")
        && let Some(total) = num(hear.get("total"))
    {
        let mut sub: Vec<String> = Vec::new();
        if let Some(stt) = num(hear.get("stt")) {
            sub.push(format!("stt {stt:.2}"));
        }
        if let Some(pre) = num(hear.get("pre")).filter(|v| *v != 0.0) {
            sub.push(format!("pre {pre:.2}"));
        }
        if let Some(t) = num(hear.get("turn")).filter(|v| *v != 0.0) {
            sub.push(format!("turn {t:.2}"));
        }
        if let Some(hold) = num(hear.get("hold")).filter(|v| *v != 0.0) {
            let kept = hear
                .get("speculation")
                .and_then(|s| s.get("kept"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0;
            sub.push(format!(
                "hold {hold:.2}{}",
                if kept { ", answered during it" } else { "" }
            ));
        }
        parts.push(format!("hear {total:.2} ({})", sub.join(", ")));
    }
    if let Some(i) = rec.get("intercepted").and_then(Value::as_str) {
        parts.push(format!("intercepted {i}"));
    }
    if let Some(c) = rec.get("confirm").filter(|c| c.is_object()) {
        parts.push(format!(
            "confirm {:.2} ({} {:.2})",
            num(c.get("total")).unwrap_or(0.0),
            c["classifier"].as_str().unwrap_or(""),
            num(c.get("classify")).unwrap_or(0.0)
        ));
    }
    if let Some(llm) = rec.get("llm").filter(|l| l.is_object()) {
        let calls = calls_of(rec);
        let mut sub: Vec<String> = Vec::new();
        if let Some(first) = calls.first().and_then(|c| num(c.get("ttfb"))) {
            sub.push(format!("ttfb {first:.2}"));
        }
        sub.push(format!(
            "{} call{}",
            calls.len(),
            if calls.len() != 1 { "s" } else { "" }
        ));
        if let Some(share) = cache_share(&calls) {
            sub.push(format!("cache {}", percent(share)));
        }
        parts.push(format!(
            "llm {:.2} ({})",
            num(llm.get("total")).unwrap_or(0.0),
            sub.join(", ")
        ));
    }
    if let Some(tools) = rec.get("tools").filter(|t| t.is_object()) {
        let names: Vec<String> = tools["calls"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|t| match num(t.get("secs")) {
                        Some(s) => format!("{} {s:.2}", t["name"].as_str().unwrap_or("")),
                        None => t["name"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        parts.push(format!(
            "tools {:.2} ({})",
            num(tools.get("total")).unwrap_or(0.0),
            names.join(", ")
        ));
    }
    if let Some(tts) = rec.get("tts")
        && let Some(total) = num(tts.get("total"))
    {
        parts.push(format!("tts {total:.2}"));
    }
    if let Some(other) = num(rec.get("other"))
        && other.abs() >= 0.01
    {
        parts.push(format!("other {other:.2}"));
    }
    let mut line = head;
    if !parts.is_empty() && reply.is_some() {
        line.push_str(" = ");
        line.push_str(&parts.join(" + "));
    } else if reply.is_none() && !parts.is_empty() {
        line.push_str(": ");
        line.push_str(&parts.join(", "));
    }
    if let Some(b) = num(rec.get("barge_in")) {
        line.push_str(&format!(", cut off {b:.2}s after the operator began"));
    }
    if let Some(flags) = rec.get("flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        let f: Vec<&str> = flags.iter().filter_map(Value::as_str).collect();
        line.push_str(&format!(" [{}]", f.join(", ")));
    }
    line
}

/// Python's `f"{x:.0%}"`: half to even.
fn percent(share: f64) -> String {
    format!("{}%", (share * 100.0).round_ties_even() as i64)
}

/// Append one line; past `max_bytes` the file moves to `.1` (one generation kept).
pub fn append_record(path: &Path, record: &Value, max_bytes: u64) {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Ok(meta) = std::fs::metadata(path)
            && meta.len() >= max_bytes
        {
            std::fs::rename(path, rotated(path))?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        writeln!(f, "{}", serde_json::to_string(record).unwrap_or_default())?;
        Ok(())
    };
    if let Err(e) = write() {
        warn!("latency: could not write {}: {e}", path.display());
    }
}

fn rotated(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.1"))
}

/// Every record in `path` and its rotated generation, oldest first.
pub fn read_records(path: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for p in [rotated(path), path.to_path_buf()] {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                out.push(v);
            }
        }
    }
    out
}

// --- summaries -------------------------------------------------------------------------

/// Nearest-rank-with-interpolation percentile, `q` in [0, 100].
pub fn percentile(values: &[f64], q: f64) -> Option<f64> {
    let mut xs: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    if xs.len() == 1 {
        return Some(xs[0]);
    }
    let k = (xs.len() - 1) as f64 * q / 100.0;
    let lo = k.floor() as usize;
    let hi = (lo + 1).min(xs.len() - 1);
    Some(xs[lo] + (xs[hi] - xs[lo]) * (k - lo as f64))
}

pub const STAGES: [&str; 16] = [
    "reply",
    "ack",
    "hear",
    "stt",
    "turn",
    "hold",
    "pre",
    "confirm",
    "llm",
    "llm_ttfb",
    "llm_calls",
    "tools",
    "tts",
    "tts_synth",
    "other",
    "barge_in",
];

/// The flat stages a summary reports for one record.
pub fn stage_values(rec: &Value) -> IndexMap<&'static str, Option<f64>> {
    let get = |a: &str, b: &str| rec.get(a).and_then(|v| v.get(b)).and_then(Value::as_f64);
    let calls = calls_of(rec);
    let mut out = IndexMap::new();
    out.insert("reply", num(rec.get("reply")));
    out.insert("ack", num(rec.get("ack")));
    out.insert("hear", get("hear", "total"));
    out.insert("stt", get("hear", "stt"));
    out.insert("turn", get("hear", "turn"));
    out.insert("hold", get("hear", "hold"));
    out.insert("pre", get("hear", "pre"));
    out.insert("confirm", get("confirm", "total"));
    out.insert("llm", get("llm", "total"));
    out.insert("llm_ttfb", calls.first().and_then(|c| num(c.get("ttfb"))));
    out.insert(
        "llm_calls",
        (!calls.is_empty()).then_some(calls.len() as f64),
    );
    out.insert("tools", get("tools", "total"));
    out.insert("tts", get("tts", "total"));
    out.insert("tts_synth", get("tts", "synth"));
    out.insert("other", num(rec.get("other")));
    out.insert("barge_in", num(rec.get("barge_in")));
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub n: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}

/// p50, p95 and count per stage.
pub fn summarize(records: &[Value]) -> IndexMap<&'static str, Stat> {
    let mut out = IndexMap::new();
    for stage in STAGES {
        let vals: Vec<f64> = records
            .iter()
            .filter_map(|r| stage_values(r).get(stage).copied().flatten())
            .collect();
        if vals.is_empty() {
            continue;
        }
        out.insert(
            stage,
            Stat {
                n: vals.len(),
                p50: percentile(&vals, 50.0),
                p95: percentile(&vals, 95.0),
            },
        );
    }
    out
}

pub fn summary_table(records: &[Value], title: &str) -> Vec<String> {
    let s = summarize(records);
    let mut lines = vec![
        format!("{title} ({} turns)", records.len()),
        format!("  {:<10} {:>4} {:>7} {:>7}", "stage", "n", "p50", "p95"),
    ];
    for (stage, v) in &s {
        let fmt = |x: Option<f64>| match x {
            Some(x) if *stage == "llm_calls" => format!("{x:7.0}"),
            Some(x) => format!("{x:7.2}"),
            None => "      -".to_string(),
        };
        lines.push(format!(
            "  {stage:<10} {:>4} {} {}",
            v.n,
            fmt(v.p50),
            fmt(v.p95)
        ));
    }
    let calls: Vec<Value> = records.iter().flat_map(calls_of).collect();
    if let Some(share) = cache_share(&calls) {
        lines.push(format!(
            "  prompt cache: {} of input tokens read from cache",
            percent(share)
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::frames::Audio;
    use tokio_util::sync::CancellationToken;

    const SECRET: &str = "zebra quartz"; // stands in for anything the operator or the model said
    const OTHER: &str = "other";

    struct Feed(Arc<LatencyObserver>);

    impl Feed {
        fn at(&self, t: f64, frame: Frame, src: &'static str) {
            self.0.on_frame(&frame, src, t, Direction::Down);
        }
        fn up(&self, t: f64, frame: Frame, src: &'static str) {
            self.0.on_frame(&frame, src, t, Direction::Up);
        }
    }

    fn observer(tmp: &Path) -> (Arc<LatencyObserver>, Feed, Arc<Mutex<Vec<Value>>>) {
        let records = Arc::new(Mutex::new(Vec::new()));
        let r = records.clone();
        let obs = Arc::new(
            LatencyObserver::new("s1", "phone", Some("claude-test"))
                .with_log(Some(tmp.join("latency.jsonl")), DEFAULT_MAX_BYTES)
                .with_sink(Arc::new(move |rec| r.lock().unwrap().push(rec.clone()))),
        );
        (obs.clone(), Feed(obs), records)
    }

    fn said() -> Frame {
        Frame::Transcript(SECRET.into())
    }

    fn usage(inp: u64, out: u64, read: u64, write: u64) -> Frame {
        Frame::Metrics(Metric::Usage {
            input: inp,
            output: out,
            cache_read: read,
            cache_write: write,
            model: Some("claude-test".into()),
        })
    }

    fn ttfb(value: f64) -> Frame {
        Frame::Metrics(Metric::Ttfb {
            value,
            model: Some("claude-test".into()),
        })
    }

    fn tts_audio() -> Frame {
        Frame::TtsAudio {
            audio: Audio::new(vec![0; 240], 24000),
            turn: CancellationToken::new(),
        }
    }

    fn in_progress(name: &str, id: &str) -> Frame {
        Frame::FunctionCallInProgress {
            name: name.into(),
            id: id.into(),
        }
    }

    fn result(name: &str, id: &str) -> Frame {
        Frame::FunctionCallResult {
            name: name.into(),
            id: id.into(),
        }
    }

    /// The operator asks, the model calls get_issue (narrating first), then answers.
    fn a_tool_turn(feed: &Feed, obs: &LatencyObserver) {
        feed.at(0.0, Frame::Speak("Yeah?".into()), OTHER); // the greeting
        feed.at(0.1, Frame::TtsStarted, TTS);
        feed.at(0.3, tts_audio(), TTS);
        feed.at(0.35, Frame::BotStarted, OTHER);
        feed.at(0.9, Frame::BotStopped, OTHER);

        feed.at(1.0, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(1.0, Frame::UserStarted, OTHER);
        feed.at(1.01, Frame::UserStarted, OTHER); // the other broadcast copy
        feed.at(2.0, Frame::VadStopped { stop_secs: 0.2 }, OTHER); // operator silent at 1.8
        feed.at(2.01, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(2.15, ttfb(0.12), STT);
        feed.at(2.2, said(), STT);
        obs.interceptors(0.01);
        feed.at(
            2.25,
            Frame::Metrics(Metric::Turn {
                complete: true,
                probability: 0.9,
                ms: 40.0,
            }),
            OTHER,
        );
        obs.held(0.8);
        feed.at(3.05, Frame::UserStopped, OTHER);

        feed.at(3.1, Frame::LlmStart, LLM);
        feed.at(3.7, ttfb(0.6), LLM);
        feed.at(3.72, Frame::LlmText(format!("Let me look, {SECRET}.")), LLM);
        feed.at(
            3.9,
            Frame::FunctionCallsStarted(vec!["get_issue".into()]),
            LLM,
        );
        feed.at(3.91, in_progress("get_issue", "c1"), LLM);
        feed.at(3.92, Frame::LlmEnd, LLM);
        feed.at(3.93, usage(100, 40, 3000, 0), LLM);
        obs.tool_step("c1", "read", 0.19);
        obs.tool_status("c1", "ok");
        feed.at(4.11, result("get_issue", "c1"), LLM);

        feed.at(4.2, Frame::LlmStart, LLM);
        feed.at(4.7, ttfb(0.5), LLM);
        feed.at(4.75, Frame::LlmText(format!("It is {SECRET}.")), LLM);
        feed.at(4.9, Frame::LlmEnd, LLM);
        feed.at(4.91, usage(50, 20, 3100, 200), LLM);
        feed.at(4.95, Frame::TtsStarted, TTS);
        feed.at(5.15, tts_audio(), TTS);
        feed.at(5.2, Frame::BotStarted, OTHER);
        feed.up(5.21, Frame::BotStarted, OTHER);
        feed.at(7.0, Frame::BotStopped, OTHER);
    }

    fn f(v: &Value, key: &str) -> f64 {
        v[key].as_f64().unwrap_or_else(|| panic!("{key} in {v}"))
    }

    #[test]
    fn test_a_tool_turn_adds_up() {
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        a_tool_turn(&feed, &obs);
        let recs = records.lock().unwrap().clone();
        let (opening, turn) = (&recs[0], &recs[1]);
        assert_eq!(opening["kind"], "opening");
        assert_eq!(f(opening, "reply"), 0.35);

        assert_eq!(turn["kind"], "voice");
        assert_eq!(turn["origin"], "phone");
        assert_eq!(f(turn, "reply"), 3.4); // 5.2 - 1.8, from when the operator stopped talking
        let hear = &turn["hear"];
        assert_eq!(f(hear, "total"), 1.25); // 3.05 - 1.8
        assert_eq!(f(hear, "vad"), 0.2);
        assert_eq!(f(hear, "stt"), 0.2);
        assert_eq!(f(hear, "stt_model"), 0.12);
        assert_eq!(f(hear, "hold"), 0.8);
        assert_eq!(f(hear, "pre"), 0.01);
        assert_eq!(f(hear, "turn"), 0.05); // 3.05 - 2.2 - 0.8
        assert_eq!(hear["segments"], 1);
        assert_eq!(
            hear["smart_turn"],
            json!([{"complete": true, "p": 0.9, "ms": 40.0}])
        );

        let llm = &turn["llm"];
        let calls = llm["calls"].as_array().unwrap();
        assert_eq!(f(&calls[0], "ttfb"), 0.6);
        assert_eq!(f(&calls[1], "ttfb"), 0.5);
        assert_eq!(calls[0]["tools"], json!(["get_issue"]));
        assert_eq!(f(&calls[0], "first_tool"), 0.8);
        assert_eq!(calls[0]["cache_read"], 3000);
        assert_eq!(calls[1]["cache_write"], 200);
        assert_eq!(f(llm, "total"), 1.52); // 0.82 + 0.7
        assert_eq!(f(&turn["tools"], "total"), 0.19); // 4.11 - 3.92, past the end of the first call
        assert_eq!(turn["tools"]["calls"][0]["steps"], json!({"read": 0.19}));
        let tts = &turn["tts"];
        assert_eq!(f(tts, "total"), 0.3);
        assert_eq!(f(tts, "synth"), 0.2);
        assert_eq!(f(tts, "out"), 0.05);
        assert_eq!(f(turn, "speech"), 1.8);
        let parts =
            f(hear, "total") + f(llm, "total") + f(&turn["tools"], "total") + f(tts, "total");
        assert!((f(turn, "reply") - parts - f(turn, "other")).abs() < 0.002);
        assert_eq!(turn["flags"], json!(["narration_dropped"]));
    }

    #[test]
    fn test_no_words_reach_the_record_the_file_or_the_log() {
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Lines {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Lines {
            type Writer = Lines;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }
        let lines = Lines::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .finish();
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        tracing::subscriber::with_default(subscriber, || {
            a_tool_turn(&feed, &obs);
        });
        let on_disk = std::fs::read_to_string(tmp.path().join("latency.jsonl")).unwrap();
        assert_eq!(on_disk.lines().count(), 2);
        let logged = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        let recs = serde_json::to_string(&*records.lock().unwrap()).unwrap();
        for blob in [recs, on_disk, logged.clone()] {
            for word in SECRET.split_whitespace() {
                assert!(!blob.contains(word), "{blob}");
            }
            assert!(!blob.contains("Yeah?"));
        }
        assert!(
            logged.contains("latency: turn 2 phone voice reply 3.40s = hear 1.25"),
            "{logged}"
        );
    }

    #[test]
    fn test_format_line_names_the_parts() {
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        a_tool_turn(&feed, &obs);
        let line = format_line(&records.lock().unwrap()[1]);
        assert!(
            line.starts_with("latency: turn 2 phone voice reply 3.40s = hear 1.25 (stt 0.20"),
            "{line}"
        );
        assert!(line.contains("hold 0.80"));
        assert!(
            line.contains("llm 1.52 (ttfb 0.60, 2 calls, cache 95%)"),
            "{line}"
        );
        assert!(line.contains("tools 0.19 (get_issue 0.20)")); // 0.01 of it overlaps the model call
        assert!(line.contains("other 0.14")); // hops: 0.05 to the model, 0.09 from the result back
        assert!(line.contains("tts 0.30"));
        assert!(line.ends_with("[narration_dropped]"));
    }

    #[test]
    fn test_an_intercepted_goodbye_is_timed_without_the_model() {
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::UserStarted, OTHER);
        feed.at(1.0, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(1.15, said(), STT);
        obs.intercepted("quick_intents", 0.002);
        feed.at(1.16, Frame::Speak("Talk soon!".into()), OTHER);
        feed.at(1.2, Frame::TtsStarted, TTS);
        feed.at(1.4, tts_audio(), TTS);
        feed.at(1.42, Frame::BotStarted, OTHER);
        feed.at(1.43, Frame::End, OTHER); // first seen at the source, before the goodbye ends
        assert!(records.lock().unwrap().is_empty());
        obs.flush(); // the session is over
        let recs = records.lock().unwrap().clone();
        assert_eq!(recs.len(), 1);
        let turn = &recs[0];
        assert_eq!(turn["intercepted"], "quick_intents");
        assert_eq!(f(turn, "reply"), 0.62);
        assert!(turn["llm"].is_null());
        assert_eq!(f(&turn["hear"], "total"), 0.352); // transcript at 1.15 plus the interceptor
        assert_eq!(turn["tts"]["direct"], true);
    }

    #[test]
    fn test_a_pause_mid_sentence_counts_segments_and_times_from_the_last() {
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(0.0, Frame::UserStarted, OTHER);
        feed.at(1.0, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(1.2, Frame::Transcript("first half".into()), STT);
        obs.kept_open();
        feed.at(1.5, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(2.5, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(2.7, Frame::Transcript("second half".into()), STT);
        obs.held(0.8);
        feed.at(3.6, Frame::UserStopped, OTHER);
        feed.at(3.6, Frame::LlmStart, LLM);
        feed.at(4.0, Frame::LlmEnd, LLM);
        feed.at(4.3, Frame::BotStarted, OTHER);
        obs.flush();
        let recs = records.lock().unwrap().clone();
        let turn = &recs[0];
        assert_eq!(turn["hear"]["segments"], 2);
        assert_eq!(turn["hear"]["kept_open"], 1);
        assert_eq!(f(turn, "reply"), 2.0); // from 2.3, the end of the second segment
        assert_eq!(f(&turn["hear"], "stt"), 0.2);
    }

    #[test]
    fn test_a_typed_turn_is_timed_from_its_start() {
        let tmp = tempfile::tempdir().unwrap();
        let (_obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::Speak("Yeah?".into()), OTHER);
        feed.at(0.2, Frame::BotStarted, OTHER);
        feed.at(0.6, Frame::BotStopped, OTHER);
        feed.at(10.0, Frame::ThinkingStart, OTHER);
        feed.at(10.01, Frame::LlmStart, LLM);
        feed.at(10.5, Frame::LlmEnd, LLM);
        feed.at(10.8, Frame::BotStarted, OTHER);
        feed.at(11.5, Frame::BotStopped, OTHER);
        let recs = records.lock().unwrap().clone();
        let (opening, turn) = (&recs[0], &recs[1]);
        assert_eq!(opening["kind"], "opening");
        assert_eq!(f(opening, "reply"), 0.2);
        assert_eq!(turn["kind"], "text");
        assert_eq!(f(turn, "reply"), 0.8);
        assert!(turn.get("hear").is_none());
    }

    #[test]
    fn test_a_new_turn_before_ghost_answered_marks_the_old_one_interrupted() {
        let tmp = tempfile::tempdir().unwrap();
        let (_obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::UserStarted, OTHER);
        feed.at(1.0, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(1.5, Frame::UserStopped, OTHER);
        feed.at(1.6, Frame::LlmStart, LLM);
        feed.at(2.0, Frame::UserStarted, OTHER); // the operator talks over the wait
        let recs = records.lock().unwrap().clone();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["flags"], json!(["interrupted"]));
        assert!(recs[0]["reply"].is_null());
    }

    #[test]
    fn test_a_barge_in_is_timed_from_when_the_operator_began() {
        let tmp = tempfile::tempdir().unwrap();
        let (_obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::Speak("Yeah?".into()), OTHER);
        feed.at(0.2, Frame::BotStarted, OTHER);
        // a blip of echo first: a segment that interrupted nothing
        feed.at(1.0, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(1.0, Frame::VadStarted { start_secs: 0.2 }, OTHER); // the other broadcast copy
        feed.at(1.3, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        feed.at(1.3, Frame::VadStopped { stop_secs: 0.2 }, OTHER);
        // the operator, from 2.0 (the VAD needs its start_secs to be sure)
        feed.at(2.2, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(2.21, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(2.9, Frame::UserStarted, OTHER); // two words heard: Ghost is cut off
        feed.at(2.91, Frame::UserStarted, OTHER);
        feed.at(2.95, Frame::BotStopped, OTHER);
        let recs = records.lock().unwrap().clone();
        assert_eq!(recs.len(), 1);
        let opening = &recs[0];
        assert_eq!(opening["flags"], json!(["barged_in"]));
        assert_eq!(f(opening, "barge_in"), 0.9);
        assert!(format_line(opening).contains("cut off 0.90s after the operator began"));
        assert_eq!(summarize(&recs)["barge_in"].p50, Some(0.9));
    }

    #[test]
    fn test_speech_while_ghost_is_quiet_is_no_barge_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (_obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::Speak("Yeah?".into()), OTHER);
        feed.at(0.2, Frame::BotStarted, OTHER);
        feed.at(0.6, Frame::BotStopped, OTHER);
        feed.at(1.2, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(1.2, Frame::UserStarted, OTHER);
        let recs = records.lock().unwrap().clone();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].get("barge_in").is_none() && recs[0].get("flags").is_none());
    }

    #[test]
    fn test_rotation_keeps_one_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("latency.jsonl");
        for i in 0..30 {
            append_record(&path, &json!({"turn": i, "pad": "x".repeat(100)}), 1000);
        }
        assert!(path.with_file_name("latency.jsonl.1").exists());
        assert!(std::fs::metadata(&path).unwrap().len() < 1200);
        let turns: Vec<i64> = read_records(&path)
            .iter()
            .map(|r| r["turn"].as_i64().unwrap())
            .collect();
        let mut sorted = turns.clone();
        sorted.sort_unstable();
        assert_eq!(turns, sorted);
        assert_eq!(*turns.last().unwrap(), 29);
        assert!(turns.len() < 30);
    }

    #[test]
    fn test_percentiles_and_summary() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 50.0), Some(2.5));
        assert_eq!(percentile(&[5.0], 95.0), Some(5.0));
        assert_eq!(percentile(&[], 50.0), None);
        let recs: Vec<Value> = [1.0, 2.0, 3.0]
            .iter()
            .map(|r| json!({"reply": r, "hear": {"total": r / 2.0}}))
            .collect();
        let s = summarize(&recs);
        assert_eq!(s["reply"].p50, Some(2.0));
        assert_eq!(s["hear"].n, 3);
        assert_eq!(
            cache_share(&[json!({"in": 10, "cache_read": 90, "cache_write": 0})]),
            Some(0.9)
        );
        assert_eq!(cache_share(&[]), None);
    }

    #[test]
    fn test_an_answer_given_during_the_grace_is_timed_from_the_operator() {
        // A speculative answer is held in the LLM stage until the turn is confirmed: its
        // first byte is reported before the call is seen, and the reply still counts from
        // the end of speech.
        let tmp = tempfile::tempdir().unwrap();
        let (obs, feed, records) = observer(tmp.path());
        feed.at(0.0, Frame::BotStopped, OTHER);
        obs.skip_opening(); // straight to a voice turn
        feed.at(1.0, Frame::VadStarted { start_secs: 0.2 }, OTHER);
        feed.at(1.0, Frame::UserStarted, OTHER);
        feed.at(2.0, Frame::VadStopped { stop_secs: 0.2 }, OTHER); // operator silent at 1.8
        feed.at(2.15, said(), STT);
        obs.speculation("started");
        feed.at(2.9, ttfb(0.7), LLM); // the held answer's first byte
        obs.held(0.8);
        obs.speculation("kept");
        feed.at(3.0, Frame::UserStopped, OTHER);
        feed.at(3.0, Frame::LlmStart, LLM); // released by the confirmation
        feed.at(3.0, Frame::LlmText(format!("It is {SECRET}.")), LLM);
        feed.at(3.01, Frame::LlmEnd, LLM);
        feed.at(3.05, Frame::TtsStarted, TTS);
        feed.at(3.3, tts_audio(), TTS);
        feed.at(3.35, Frame::BotStarted, OTHER);
        feed.at(5.0, Frame::BotStopped, OTHER);

        let recs = records.lock().unwrap().clone();
        let rec = recs.last().unwrap();
        assert_eq!(rec["kind"], "voice");
        assert_eq!(rec["hear"]["speculation"], json!({"started": 1, "kept": 1}));
        assert_eq!(f(&rec["llm"]["calls"][0], "ttfb"), 0.7);
        assert_eq!(f(rec, "reply"), 1.55); // 1.8 -> 3.35
        assert!(format_line(rec).contains("answered during it"));
    }
}
