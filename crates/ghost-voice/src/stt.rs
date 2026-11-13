//! Speech in: Silero's segments and Parakeet's transcripts (`ghost_voice.parakeet` as a
//! segmented STT service), with the interim re-reads while Ghost speaks.
//!
//! A segment is transcribed once the VAD says it ended, so an operator talking over Ghost
//! was heard, and could cut it off (`turns`), only after they paused. With `interim_secs`
//! above zero, while Ghost is speaking and the operator's segment is open, the segment so
//! far (the pre-roll included) is transcribed again every `interim_secs` and pushed as
//! [`Frame::Interim`]. Only the barge-in rule and the echo filter read those; the context,
//! the interceptors, the confirmation path and the transcript store take final
//! [`Frame::Transcript`]s only. The wake listener's stage has them off.
//!
//! One recognition runs at a time on the loaded model, across every session: a final waits
//! for an interim in flight, and an interim tick that finds the model busy is skipped.
//! Whether Ghost is speaking comes from the output stage's shared flag (`crate::speaking`)
//! as much as from the `BotStarted` frame, which reaches this stage last of all.
//!
//! The segment's read starts when the operator goes quiet, not when the VAD has heard
//! enough quiet to call the segment over (`with_early_read`): at the VAD's first quiet
//! frame the segment so far is read, with silence where the rest of the stop would be, so
//! the words are ready when the stop is confirmed. Speech again before the stop throws the
//! read away (`Metric::EarlyRead`, `discarded`); the next quiet starts another.
//!
//! The VAD runs here (Pipecat ran it in the user aggregator and sent its frames back up):
//! the frames it makes are the same, `VadStarted` and `VadStopped`, and every stage below
//! sees them in order with the audio. Transcripts are logged only under
//! `GHOST_DEBUG_AUDIO` (`debug`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ghost_infer::parakeet::{InterimSchedule, interim_audio};
use ghost_infer::{Parakeet, VadAnalyzer, VadParams, VadState};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::debug;
use crate::frames::{Direction, Frame, Metric, Out, Stage};
use crate::speaking::Speaking;

pub const SAMPLE_RATE: u32 = 16_000;
/// Audio kept before the VAD's start, so the first word isn't clipped: the VAD fires its
/// `start_secs` and the model's onset after speech began, and Parakeet reads a first word
/// better with the room's silence before it (a 0.4 s pre-roll lost "What's" from "What's
/// waiting for triage?" on the replay rig).
pub const PRE_ROLL_SECS: f64 = 1.0;
/// Silence after a segment, so the model sees its end.
pub const TRAILING_SECS: f64 = 0.2;

/// The model behind the stage: Parakeet, or a fake in tests.
pub trait Recognizer: Send {
    /// float32 16 kHz -> text.
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String>;
}

impl Recognizer for Parakeet {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        Parakeet::transcribe(self, audio)
    }
}

/// One loaded model, shared by every session's stage and the wake listener's check.
pub type SharedRecognizer = Arc<Mutex<dyn Recognizer>>;

/// Transcribe in the calling thread (off the async runtime), taking a turn on the shared
/// model. With `wait` off, None without running when the model is busy.
/// The noise floor laid under every recognition, in dBFS. Parakeet reads nothing from a
/// stretch that ends in about a second of exact digital silence: with Ghost's audio
/// through the reSpeaker, the array's own canceller leaves −99 dBFS between words, so a
/// segment's closing half second of silence plus `TRAILING_SECS` was enough (2026-10-30
/// 21:34: "what part of your source code is the speech to text" came through as three
/// words and three empty segments at −25 dBFS; the oracle read the same audio whole once
/// −70 dBFS of noise was added, and nothing with a second of zeros appended).
pub const DITHER_DBFS: f32 = -72.0;

/// `audio` with [`DITHER_DBFS`] of deterministic noise under it: the same samples give the
/// same transcript, and nothing audible changes (the old chain's noise floor sat higher).
pub fn dithered(audio: &[f32]) -> Vec<f32> {
    let amplitude = 10f32.powf(DITHER_DBFS / 20.0) * 3.0_f32.sqrt(); // uniform noise's rms
    let mut state: u32 = 0x9E37_79B9;
    audio
        .iter()
        .map(|&x| {
            // xorshift32: cheap, deterministic, good enough for a noise floor
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let unit = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            x + unit * amplitude
        })
        .collect()
}

pub fn recognize(model: &SharedRecognizer, audio: &[f32], wait: bool) -> Option<String> {
    let mut guard = if wait {
        model.lock().unwrap_or_else(|e| e.into_inner())
    } else {
        match model.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        }
    };
    let audio = dithered(audio);
    match guard.transcribe(&audio) {
        Ok(text) => Some(text.trim().to_string()),
        Err(e) => {
            warn!("Parakeet failed: {e}");
            Some(String::new())
        }
    }
}

/// The recognizer as the wake listener's check takes it: one reading, waiting its turn.
pub fn recognize_fn(model: &SharedRecognizer) -> crate::wakeword::Recognize {
    let model = model.clone();
    Arc::new(move |audio: Vec<f32>| recognize(&model, &audio, true).unwrap_or_default())
}

/// A stretch's level in dBFS.
pub fn level_dbfs(samples: &[i16]) -> f64 {
    let power = samples
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / samples.len().max(1) as f64;
    20.0 * (power.sqrt() / 32768.0).max(1e-9).log10()
}

fn epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Parakeet TDT as one session's speech-in stage.
pub struct ParakeetStt {
    model: SharedRecognizer,
    vad: Option<VadAnalyzer>,
    params: VadParams,
    schedule: Arc<Mutex<InterimSchedule>>,
    interim_secs: f64,
    pre_roll: VecDeque<i16>,
    segment: Vec<i16>,
    user_speaking: bool,
    /// As the frames said it.
    bot_speaking: bool,
    /// As the output stage says it.
    speaking: Speaking,
    interim_task: Option<JoinHandle<()>>,
    zero: Instant,
    /// The VAD's settings while Ghost speaks (`VoiceSettings::vad_params_over_ghost`):
    /// the canceller passes the operator in 150–350 ms pieces under Ghost's audio. Swapped
    /// in and out between segments only, never under one.
    over_ghost: Option<VadParams>,
    /// The VAD's settings while a close waits (`Speaking::is_closing`): the room's, but
    /// starting quicker, so the operator going on after a breath counts 100 ms sooner.
    closing: Option<VadParams>,
    mode: VadMode,
    pending_mode: Option<VadMode>,
    /// The VAD's clock (it resets Silero's state every few seconds of it): epoch seconds
    /// live, the audio's own time when a harness feeds a file faster than real time.
    vad_clock: Option<VadClock>,
    /// Read a segment from its first quiet frame (`with_early_read`).
    early_read: EarlyReadMode,
    /// The read started at the first quiet frame, while the VAD waits out its stop.
    early: Option<EarlyRead>,
    /// The VAD's state after the frame before.
    vad_state: VadState,
    /// Quiet heard since the VAD began to stop, and whether this quiet's read began.
    quiet_secs: f64,
    early_tried: bool,
    /// How much quiet starts the early read: the VAD dips between words for a frame or
    /// two, and a read begun there is thrown away.
    early_after_secs: f64,
    /// Each segment's read as numbers (the microphone's health, ADR 0064).
    on_read: Option<OnRead>,
}

/// A segment's read as numbers: its level in dBFS and how many words were read. Never the
/// words, never the audio. Not called for a segment heard while Ghost spoke (its echo may
/// leak loud and read as nothing without the microphone being at fault).
pub type OnRead = Arc<dyn Fn(f64, usize) + Send + Sync>;

/// Whether a segment is read from its first quiet frame (`ParakeetStt::with_early_read`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EarlyReadMode {
    /// At the VAD's stop, as before.
    #[default]
    Off,
    /// From the first quiet frame; its words are the segment's.
    On,
    /// From the first quiet frame, and read again at the stop as before: the words are
    /// the stop's, and whether the early read said the same is counted (`differs`). For
    /// the harness, which measures that the early words are the same.
    Verify,
    /// From the first quiet frame, its words sent on at once as `Frame::EarlyTranscript`
    /// for the turn's speculative answer only, and read again at the stop: the segment's
    /// words are still the stop's, and an answer started on early words that differ from
    /// them is started again (ADR 0054).
    Speculate,
}

/// A segment's read begun at its first quiet frame.
struct EarlyRead {
    task: JoinHandle<(Option<String>, f64)>,
    /// `Speculate`: stops the early words going out once they are moot (speech again,
    /// the stop's read under way).
    moot: Option<tokio_util::sync::CancellationToken>,
}

/// What became of an early read (`Metric::EarlyRead`): `used`, `discarded` (speech came
/// back first), `same` and `differs` (verified against the read at the stop).
pub const EARLY_USED: &str = "used";
pub const EARLY_DISCARDED: &str = "discarded";

/// Seconds, for the VAD (`ParakeetStt::with_vad_clock`).
pub type VadClock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Which of the VAD's settings are in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadMode {
    Room,
    OverGhost,
    Closing,
}

impl ParakeetStt {
    /// `vad` None: the VAD frames come from upstream (a test, or a transport with its own).
    /// `interim_secs` is for sessions (see the module doc); passive audio gets none.
    pub fn new(
        model: SharedRecognizer,
        vad: Option<VadAnalyzer>,
        params: VadParams,
        interim_secs: f64,
    ) -> Self {
        Self {
            model,
            vad,
            params,
            schedule: Arc::new(Mutex::new(InterimSchedule::new(interim_secs))),
            interim_secs,
            pre_roll: VecDeque::new(),
            segment: Vec::new(),
            user_speaking: false,
            bot_speaking: false,
            speaking: Speaking::new(),
            interim_task: None,
            zero: Instant::now(),
            over_ghost: None,
            closing: None,
            mode: VadMode::Room,
            pending_mode: None,
            vad_clock: None,
            early_read: EarlyReadMode::Off,
            early: None,
            vad_state: VadState::Quiet,
            quiet_secs: 0.0,
            early_tried: false,
            early_after_secs: 0.0,
            on_read: None,
        }
    }

    /// Report each segment's read (level, words) to `on_read`.
    pub fn with_on_read(mut self, on_read: Option<OnRead>) -> Self {
        self.on_read = on_read;
        self
    }

    fn report_read(&self, dbfs: f64, words: usize) {
        if self.mode == VadMode::OverGhost || self.speaking.is_on() {
            return;
        }
        if let Some(f) = &self.on_read {
            f(dbfs, words);
        }
    }

    /// Read each segment from its first quiet frame, so its words are ready when the VAD
    /// confirms the stop (see the module doc).
    pub fn with_early_read(mut self, mode: EarlyReadMode) -> Self {
        self.early_read = mode;
        self
    }

    /// Start the early read once this much quiet was heard, not at the first quiet frame.
    pub fn with_early_read_after(mut self, secs: f64) -> Self {
        self.early_after_secs = secs.max(0.0);
        self
    }

    /// The audio a segment is read on: what was heard, then silence to the length the
    /// read at the stop has (the rest of the VAD's stop, then `TRAILING_SECS`).
    fn read_audio(segment: &[i16], pad_secs: f64) -> Vec<f32> {
        let mut audio = segment.to_vec();
        audio.extend(std::iter::repeat_n(
            0i16,
            (pad_secs * f64::from(SAMPLE_RATE)) as usize,
        ));
        ghost_infer::audio::to_f32(&audio)
    }

    /// The operator went quiet (the VAD's first quiet frame after speech): read what was
    /// said, with silence for the stop the VAD has still to wait.
    pub(crate) fn silence_began(&mut self, out: &Out) {
        if self.early_read == EarlyReadMode::Off || !self.user_speaking || self.segment.is_empty() {
            return;
        }
        self.drop_early();
        self.early_tried = true;
        // The read at the stop has the whole stop's quiet; what is still to come of it is
        // silence here.
        let to_come = (f64::from(self.active_params().stop_secs) - self.quiet_secs).max(0.0);
        let pad = to_come + TRAILING_SECS;
        let audio = Self::read_audio(&self.segment, pad);
        let model = self.model.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        let speculate = self.early_read == EarlyReadMode::Speculate;
        let task = tokio::task::spawn_blocking(move || {
            let began = Instant::now();
            let text = recognize(&model, &audio, true);
            if speculate {
                let _ = tx.send(text.clone());
            }
            (text, began.elapsed().as_secs_f64())
        });
        let moot = speculate.then(|| {
            // The words go on the moment they are read, ahead of the VAD's stop when the
            // read is quicker than the quiet still to come.
            let moot = tokio_util::sync::CancellationToken::new();
            let (m, out) = (moot.clone(), out.clone());
            tokio::spawn(async move {
                let text = tokio::select! {
                    _ = m.cancelled() => return,
                    t = rx => t.ok().flatten().unwrap_or_default(),
                };
                if !text.is_empty() && !m.is_cancelled() {
                    debug::audio(|| format!("early words: {text:?}"));
                    out.push(Frame::EarlyTranscript(text)).await;
                }
            });
            moot
        });
        self.early = Some(EarlyRead { task, moot });
    }

    /// Speech again before the stop: the early read was of half a segment.
    pub(crate) async fn speech_resumed(&mut self, out: &Out) {
        self.quiet_secs = 0.0;
        self.early_tried = false;
        if let Some(e) = self.early.take() {
            if let Some(m) = &e.moot {
                m.cancel();
            }
            // The read runs on to its end on its thread; its CPU is what this costs. The
            // stage does not wait for it: the operator is talking.
            let out = out.clone();
            tokio::spawn(async move {
                let secs = e.task.await.map(|(_, s)| s).unwrap_or_default();
                debug::audio(|| {
                    format!("early read discarded: speech again ({secs:.2}s of model)")
                });
                out.push(Frame::Metrics(Metric::EarlyRead {
                    what: EARLY_DISCARDED,
                    secs,
                }))
                .await;
            });
        }
    }

    fn drop_early(&mut self) {
        if let Some(e) = self.early.take() {
            if let Some(m) = &e.moot {
                m.cancel();
            }
            e.task.abort();
        }
    }

    /// The VAD reads `clock` instead of the wall clock: the `hear` harness feeds a take
    /// faster than real time, and Silero's state resets every 5 s of this clock, as it
    /// would live every 5 s of audio.
    pub fn with_vad_clock(mut self, clock: VadClock) -> Self {
        self.vad_clock = Some(clock);
        self
    }

    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking = speaking;
        self
    }

    pub fn with_params_over_ghost(mut self, params: VadParams) -> Self {
        self.over_ghost = Some(params);
        self
    }

    /// While a close waits, the VAD starts after `secs` of speech instead of the room's.
    pub fn with_closing_start_secs(mut self, secs: f32) -> Self {
        self.closing = Some(VadParams {
            start_secs: secs,
            ..self.params
        });
        self
    }

    /// The interim schedule and the VAD's settings follow Ghost's voice as the output
    /// stage reports it, not only the frames, which reach this stage late behind a busy
    /// TTS stage.
    fn sync_speaking(&mut self) {
        let on = self.bot_speaking || self.speaking.is_on();
        {
            let mut schedule = self.schedule.lock().unwrap();
            if on {
                schedule.bot_started();
            } else {
                schedule.bot_stopped();
            }
        }
        let mode = if on {
            VadMode::OverGhost
        } else if self.speaking.is_closing() {
            VadMode::Closing
        } else {
            VadMode::Room
        };
        self.switch_vad(mode);
    }

    /// The VAD's settings for now: the room's, the ones for over Ghost's voice, or the
    /// ones for while a close waits (each only when it was given).
    pub fn active_params(&self) -> VadParams {
        match self.mode {
            VadMode::OverGhost => self.over_ghost.unwrap_or(self.params),
            VadMode::Closing => self.closing.unwrap_or(self.params),
            VadMode::Room => self.params,
        }
    }

    pub fn vad_mode(&self) -> VadMode {
        self.mode
    }

    fn switch_vad(&mut self, mode: VadMode) {
        if mode == self.mode {
            self.pending_mode = None;
            return;
        }
        if self.user_speaking {
            // `set_params` resets the VAD; a segment under way finishes on its settings.
            self.pending_mode = Some(mode);
            return;
        }
        self.apply_vad(mode);
    }

    fn apply_vad(&mut self, mode: VadMode) {
        let before = self.active_params();
        self.mode = mode;
        self.pending_mode = None;
        let params = self.active_params();
        if params != before
            && let Some(vad) = &mut self.vad
        {
            vad.set_params(params);
        }
    }

    /// A segment ended: a switch that waited for it happens now.
    fn segment_ended(&mut self) {
        self.user_speaking = false;
        if let Some(mode) = self.pending_mode.take() {
            self.apply_vad(mode);
        }
    }

    pub fn model(&self) -> &SharedRecognizer {
        &self.model
    }

    pub fn interim_secs(&self) -> f64 {
        self.interim_secs
    }

    fn now(&self) -> f64 {
        self.zero.elapsed().as_secs_f64()
    }

    fn busy(&self) -> bool {
        self.interim_task.as_ref().is_some_and(|t| !t.is_finished())
            || matches!(
                self.model.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            )
    }

    async fn vad_started(&mut self, out: &Out) {
        self.schedule.lock().unwrap().vad_started();
        self.user_speaking = true;
        self.segment = self.pre_roll.drain(..).collect(); // only audio since the last segment seeds the next
        out.push(Frame::VadStarted {
            start_secs: self.active_params().start_secs,
        })
        .await;
    }

    async fn vad_stopped(&mut self, out: &Out) {
        self.quiet_secs = 0.0;
        self.early_tried = false;
        self.schedule.lock().unwrap().vad_stopped();
        let stop_secs = self.active_params().stop_secs;
        self.segment_ended();
        out.push(Frame::VadStopped { stop_secs }).await;
        self.final_(out).await;
    }

    /// The segment's transcript: waits for an interim in flight, then takes the model.
    /// With an early read under way, its words, waited for.
    async fn final_(&mut self, out: &Out) {
        let mut segment = std::mem::take(&mut self.segment);
        let early = self.early.take();
        if segment.is_empty() {
            if let Some(e) = early {
                e.task.abort();
            }
            return;
        }
        let early = match early {
            // Its words have gone on, or will not now: the stop's read starts at once
            // (behind the early one on the model's lock), and the early read's words are
            // counted against it by the turn (ADR 0054).
            // The early words may still be on their way; the turn ignores any that come
            // after the stop's.
            Some(_) if self.early_read == EarlyReadMode::Speculate => None,
            Some(e) => {
                let began = Instant::now();
                let (text, secs) = e.task.await.unwrap_or((None, 0.0));
                Some((
                    text.unwrap_or_default(),
                    began.elapsed().as_secs_f64(),
                    secs,
                ))
            }
            None => None,
        };
        if let Some((text, waited, secs)) = &early
            && self.early_read == EarlyReadMode::On
        {
            out.push(Frame::Metrics(Metric::EarlyRead {
                what: EARLY_USED,
                secs: *secs,
            }))
            .await;
            // What the read at the stop would have cost the turn: only the wait for it.
            out.push(Frame::Metrics(Metric::Ttfb {
                value: *waited,
                model: None,
            }))
            .await;
            let secs_audio = segment.len() as f64 / f64::from(SAMPLE_RATE);
            let dbfs = level_dbfs(&segment);
            self.report_read(dbfs, text.split_whitespace().count());
            if text.is_empty() {
                debug::audio(|| {
                    format!(
                        "heard nothing in a {secs_audio:.1}s segment at {dbfs:.0} dBFS (read early)"
                    )
                });
                out.push(Frame::HeardNothing {
                    secs: secs_audio,
                    dbfs,
                })
                .await;
            } else {
                debug::audio(|| format!("heard: {text:?} (read early)"));
                out.push(Frame::Transcript(text.clone())).await;
            }
            return;
        }
        let secs = segment.len() as f64 / f64::from(SAMPLE_RATE);
        let level = level_dbfs(&segment);
        segment.extend(std::iter::repeat_n(
            0i16,
            (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize,
        ));
        let audio = ghost_infer::audio::to_f32(&segment);
        let model = self.model.clone();
        let began = Instant::now();
        let text = tokio::task::spawn_blocking(move || recognize(&model, &audio, true))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        out.push(Frame::Metrics(Metric::Ttfb {
            value: began.elapsed().as_secs_f64(),
            model: None,
        }))
        .await;
        if let Some((early_text, _, secs)) = early {
            // Verifying: the stop's words are the segment's; the early read's are counted.
            if early_text != text {
                debug::audio(|| format!("early read differs: {early_text:?} at the stop {text:?}"));
            }
            out.push(Frame::Metrics(Metric::EarlyRead {
                what: if early_text == text {
                    "same"
                } else {
                    "differs"
                },
                secs,
            }))
            .await;
        }
        self.report_read(level, text.split_whitespace().count());
        if text.is_empty() {
            // 2026-10-30 19:28: a 2 s segment Smart Turn judged done (0.98) left no
            // line at all, and the turn closed on the words before it.
            debug::audio(|| format!("heard nothing in a {secs:.1}s segment at {level:.0} dBFS"));
            out.push(Frame::HeardNothing { secs, dbfs: level }).await;
        } else {
            debug::audio(|| format!("heard: {text:?}"));
            out.push(Frame::Transcript(text)).await;
        }
    }

    fn maybe_interim(&mut self, out: &Out) {
        if self.interim_secs <= 0.0 {
            return;
        }
        let busy = self.busy();
        let due = self.schedule.lock().unwrap().tick(self.now(), busy);
        let Some(segment_id) = due else { return };
        let window = (f64::from(SAMPLE_RATE)
            * f64::from(ghost_infer::parakeet::INTERIM_WINDOW_SECS)) as usize;
        let start = self.segment.len().saturating_sub(window);
        let trailing = vec![0i16; (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize];
        let audio = interim_audio(&self.segment[start..], &trailing);
        let model = self.model.clone();
        let schedule = self.schedule.clone();
        let out = out.clone();
        self.interim_task = Some(tokio::spawn(async move {
            let text = tokio::task::spawn_blocking(move || recognize(&model, &audio, false))
                .await
                .ok()
                .flatten();
            let Some(text) = text.filter(|t| !t.is_empty()) else {
                return;
            };
            // The segment ended (its final is queued behind this) or Ghost went quiet: stale.
            if !schedule.lock().unwrap().is_current(segment_id) {
                return;
            }
            debug::audio(|| format!("interim: {text:?}"));
            out.push(Frame::Interim(text)).await;
        }));
    }

    fn cancel_interim(&mut self) {
        if let Some(t) = self.interim_task.take() {
            t.abort();
        }
    }
}

impl Drop for ParakeetStt {
    fn drop(&mut self) {
        self.cancel_interim();
        self.drop_early();
    }
}

#[async_trait]
impl Stage for ParakeetStt {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::AudioIn(audio) => {
                if let Some(vad) = &mut self.vad {
                    // Pipecat's rule: only Speaking and Quiet are states the pipeline sees;
                    // a dip through Stopping and back is the same segment.
                    let now = self.vad_clock.as_ref().map_or_else(epoch, |c| c());
                    let after = vad.analyze(&audio.samples, now).unwrap_or(vad.state());
                    let before = std::mem::replace(&mut self.vad_state, after);
                    if !self.user_speaking && after == VadState::Speaking {
                        self.vad_started(out).await;
                    }
                    if self.user_speaking {
                        self.segment.extend_from_slice(&audio.samples);
                        match after {
                            VadState::Stopping => {
                                if before != VadState::Stopping {
                                    self.quiet_secs = 0.0;
                                    self.early_tried = false;
                                }
                                self.quiet_secs += audio.seconds();
                                if !self.early_tried && self.quiet_secs >= self.early_after_secs {
                                    self.silence_began(out);
                                }
                            }
                            VadState::Speaking if before == VadState::Stopping => {
                                self.speech_resumed(out).await;
                            }
                            _ => {}
                        }
                    } else {
                        self.pre_roll.extend(audio.samples.iter().copied());
                        let keep = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
                        while self.pre_roll.len() > keep {
                            self.pre_roll.pop_front();
                        }
                    }
                    if self.user_speaking && after == VadState::Quiet {
                        out.forward(frame.clone(), direction).await;
                        self.vad_stopped(out).await;
                        return;
                    }
                } else if self.user_speaking {
                    self.segment.extend_from_slice(&audio.samples);
                } else {
                    self.pre_roll.extend(audio.samples.iter().copied());
                    let keep = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
                    while self.pre_roll.len() > keep {
                        self.pre_roll.pop_front();
                    }
                }
                self.sync_speaking();
                self.maybe_interim(out);
            }
            // VAD frames from upstream (a transport with its own VAD, or a test).
            Frame::VadStarted { .. } if self.vad.is_none() => {
                self.schedule.lock().unwrap().vad_started();
                self.user_speaking = true;
                self.segment = self.pre_roll.drain(..).collect(); // only audio since the last segment seeds the next
                out.forward(frame, direction).await;
                return;
            }
            Frame::VadStopped { .. } if self.vad.is_none() => {
                self.schedule.lock().unwrap().vad_stopped();
                self.segment_ended();
                out.forward(frame, direction).await;
                self.final_(out).await;
                return;
            }
            Frame::BotStarted => {
                self.bot_speaking = true;
                self.schedule.lock().unwrap().bot_started();
            }
            // an interruption silences Ghost before BotStopped
            Frame::BotStopped | Frame::Interruption => {
                self.bot_speaking = false;
                self.sync_speaking();
            }
            Frame::End | Frame::Cancel => {
                self.cancel_interim();
                self.drop_early();
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.cancel_interim();
        self.drop_early();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_the_dither_is_a_quiet_deterministic_floor() {
        let silence = vec![0.0f32; 16000];
        let a = super::dithered(&silence);
        let b = super::dithered(&silence);
        assert_eq!(a, b);
        let rms = (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
        let db = 20.0 * rms.log10();
        assert!(
            (super::DITHER_DBFS - 2.0..=super::DITHER_DBFS + 2.0).contains(&db),
            "{db}"
        );
        let speech = vec![0.1f32; 100];
        let d = super::dithered(&speech);
        assert!(d.iter().all(|x| (x - 0.1).abs() < 0.001));
    }

    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::confirmation::{ConfirmationProcessor, SessionGate};
    use crate::echo::EchoFilter;
    use crate::frames::{Audio, drain};
    use crate::hold::{HoldGate, OnResume};
    use crate::intercept::{InterceptorProcessor, no_action};
    use crate::tap::{UserTap, no_phase};
    use ghost_core::gate::Classification;
    use ghost_core::source::{Handled, TurnInterceptor, UserTurn};
    use ghost_core::{Outcome, Verdict};
    use tokio::sync::mpsc::Receiver;

    const RATE: u32 = 16000;

    #[tokio::test]
    async fn test_the_vad_settings_follow_ghosts_voice_between_segments() {
        // 2026-10-30: under a reading the operator came through in 150–350 ms pieces the
        // room's VAD settings never started on.
        let room = VadParams::default();
        let over = VadParams {
            start_secs: 0.1,
            min_volume: 0.3,
            confidence: 0.5,
            ..room
        };
        let model: SharedRecognizer = Arc::new(Mutex::new(FakeModel {
            text: "stop".into(),
            delay: Duration::ZERO,
            calls: Arc::new(Mutex::new((0, 0, 0))),
        }));
        let speaking = Speaking::new();
        let mut stt = ParakeetStt::new(model, None, room, 0.0)
            .with_params_over_ghost(over)
            .with_closing_start_secs(0.1)
            .with_speaking(speaking.clone());
        let (out, _rx) = Out::capture();
        let audio = || Frame::AudioIn(Audio::new(vec![0; 320], RATE));
        stt.process(audio(), Direction::Down, &out).await;
        assert_eq!(stt.active_params(), room);
        speaking.set(true);
        stt.process(audio(), Direction::Down, &out).await;
        assert_eq!(
            stt.active_params(),
            over,
            "Ghost speaks: the quicker settings"
        );
        // a segment under way keeps its settings until it ends
        stt.process(Frame::VadStarted { start_secs: 0.1 }, Direction::Down, &out)
            .await;
        speaking.set(false);
        stt.process(audio(), Direction::Down, &out).await;
        assert_eq!(stt.active_params(), over, "not under a segment");
        stt.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        assert_eq!(
            stt.active_params(),
            room,
            "the segment ended: the room's again"
        );
        // while a close waits, the VAD re-arms quicker for the operator going on
        speaking.set_closing(true);
        stt.process(audio(), Direction::Down, &out).await;
        assert_eq!(stt.vad_mode(), VadMode::Closing);
        assert_eq!(stt.active_params().start_secs, 0.1);
        assert_eq!(stt.active_params().min_volume, room.min_volume);
        speaking.set_closing(false);
        stt.process(audio(), Direction::Down, &out).await;
        assert_eq!(stt.vad_mode(), VadMode::Room);
    }

    /// `recognize` with a delay, counting calls that overlap.
    struct FakeModel {
        text: String,
        delay: Duration,
        calls: Arc<Mutex<(usize, usize, usize)>>, // calls, active, most at once
    }

    impl Recognizer for FakeModel {
        fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
            {
                let mut c = self.calls.lock().unwrap();
                c.0 += 1;
                c.1 += 1;
                c.2 = c.2.max(c.1);
            }
            std::thread::sleep(self.delay);
            self.calls.lock().unwrap().1 -= 1;
            Ok(self.text.clone())
        }
    }

    struct Model {
        shared: SharedRecognizer,
        calls: Arc<Mutex<(usize, usize, usize)>>,
    }

    fn model(text: &str, delay: f64) -> Model {
        let calls = Arc::new(Mutex::new((0, 0, 0)));
        let shared: SharedRecognizer = Arc::new(Mutex::new(FakeModel {
            text: text.into(),
            delay: Duration::from_secs_f64(delay),
            calls: calls.clone(),
        }));
        Model { shared, calls }
    }

    impl Model {
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().0
        }
        fn most_at_once(&self) -> usize {
            self.calls.lock().unwrap().2
        }
        fn fork(&self, interim_secs: f64) -> ParakeetStt {
            ParakeetStt::new(
                self.shared.clone(),
                None,
                VadParams::default(),
                interim_secs,
            )
        }
    }

    enum Step {
        F(Frame),
        Sleep(f64),
    }

    fn audio(secs: f64) -> Vec<Step> {
        let mut out = Vec::new();
        for _ in 0..((secs / 0.02).round() as usize) {
            out.push(Step::F(Frame::AudioIn(Audio::new(vec![0; 320], RATE))));
            out.push(Step::Sleep(0.02));
        }
        out
    }

    fn vad_start() -> Step {
        Step::F(Frame::VadStarted { start_secs: 0.2 })
    }

    fn vad_stop() -> Step {
        Step::F(Frame::VadStopped { stop_secs: 0.2 })
    }

    fn bot_started() -> Step {
        Step::F(Frame::BotStarted)
    }

    fn bot_stopped() -> Step {
        Step::F(Frame::BotStopped)
    }

    fn seq(parts: Vec<Vec<Step>>) -> Vec<Step> {
        parts.into_iter().flatten().collect()
    }

    async fn run(mut stt: ParakeetStt, steps: Vec<Step>) -> Vec<Frame> {
        let (out, mut rx) = Out::capture();
        for s in steps {
            match s {
                Step::F(f) => stt.process(f, Direction::Down, &out).await,
                Step::Sleep(secs) => tokio::time::sleep(Duration::from_secs_f64(secs)).await,
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        drain(&mut rx).await
    }

    fn interims(down: &[Frame]) -> Vec<String> {
        down.iter()
            .filter_map(|f| match f {
                Frame::Interim(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn finals(down: &[Frame]) -> Vec<String> {
        down.iter()
            .filter_map(|f| match f {
                Frame::Transcript(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    // --- when they are made -------------------------------------------------------

    #[tokio::test]
    async fn test_interims_while_ghost_speaks_and_the_operator_is_mid_sentence() {
        let m = model("stop that", 0.0);
        let down = run(
            m.fork(0.05),
            seq(vec![
                vec![bot_started(), vad_start()],
                audio(0.3),
                vec![vad_stop()],
            ]),
        )
        .await;
        assert!(interims(&down).len() >= 3, "{:?}", interims(&down));
        assert!(interims(&down).iter().all(|t| t == "stop that"));
        assert_eq!(finals(&down), vec!["stop that"]); // the final still comes, once, at the pause
    }

    #[tokio::test]
    async fn test_interims_follow_the_speaking_flag_before_bot_started_arrives() {
        // The first clause is at the speaker and the BotStarted frame is still behind the
        // TTS stage: the operator's open segment is re-read all the same.
        let m = model("stop that", 0.0);
        let speaking = Speaking::new();
        speaking.set(true);
        let down = run(
            m.fork(0.05).with_speaking(speaking),
            seq(vec![vec![vad_start()], audio(0.3), vec![vad_stop()]]),
        )
        .await;
        assert!(interims(&down).len() >= 3, "{:?}", interims(&down));
    }

    #[tokio::test]
    async fn test_no_interims_outside_ghost_speaking_over_an_open_segment() {
        let cases: Vec<(&str, Vec<Step>)> = vec![
            // Ghost is quiet: an ordinary turn, which waits for its final
            (
                "ghost-quiet",
                seq(vec![vec![vad_start()], audio(0.3), vec![vad_stop()]]),
            ),
            // Ghost speaks, the operator doesn't
            ("operator-quiet", seq(vec![vec![bot_started()], audio(0.3)])),
            // the operator's segment ended: nothing more until the next one opens
            (
                "segment-closed",
                seq(vec![
                    vec![bot_started(), vad_start(), vad_stop()],
                    audio(0.3),
                ]),
            ),
            // Ghost stopped (or was interrupted) while the operator goes on
            (
                "ghost-stopped",
                seq(vec![
                    vec![bot_started(), vad_start(), bot_stopped()],
                    audio(0.3),
                ]),
            ),
        ];
        for (name, steps) in cases {
            let m = model("stop that", 0.0);
            let down = run(m.fork(0.05), steps).await;
            assert!(interims(&down).is_empty(), "{name}");
            assert_eq!(m.calls(), finals(&down).len(), "{name}"); // and no pass was spent on one
        }
    }

    #[tokio::test]
    async fn test_off_at_zero_and_for_the_wake_listener() {
        // ghostd builds the passive listener with the default stage, which never makes an
        // interim, whatever the frames say.
        for interim_secs in [0.0, 0.0] {
            let m = model("stop that", 0.0);
            let down = run(
                m.fork(interim_secs),
                seq(vec![vec![bot_started(), vad_start()], audio(0.3)]),
            )
            .await;
            assert!(interims(&down).is_empty());
            assert_eq!(m.calls(), 0);
        }
    }

    // --- the read from the first quiet frame -------------------------------------------

    /// Reads what it is given: the words are the loud samples' count and the audio's
    /// length, so two reads say the same only of the same audio as Parakeet hears it.
    struct Counting {
        calls: Arc<Mutex<Vec<usize>>>,
        delay: Duration,
    }

    impl Recognizer for Counting {
        fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
            self.calls.lock().unwrap().push(audio.len());
            std::thread::sleep(self.delay);
            let loud = audio.iter().filter(|x| x.abs() > 0.01).count();
            Ok(format!("{loud} loud of {}", audio.len()))
        }
    }

    fn counting(delay_ms: u64) -> (SharedRecognizer, Arc<Mutex<Vec<usize>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Mutex::new(Counting {
                calls: calls.clone(),
                delay: Duration::from_millis(delay_ms),
            })),
            calls,
        )
    }

    fn speech(secs: f64) -> Vec<Frame> {
        (0..((secs / 0.02).round() as usize))
            .map(|_| Frame::AudioIn(Audio::new(vec![3000; 320], RATE)))
            .collect()
    }

    fn quiet(secs: f64) -> Vec<Frame> {
        (0..((secs / 0.02).round() as usize))
            .map(|_| Frame::AudioIn(Audio::new(vec![0; 320], RATE)))
            .collect()
    }

    fn early_reads(down: &[Frame]) -> Vec<&'static str> {
        down.iter()
            .filter_map(|f| match f {
                Frame::Metrics(Metric::EarlyRead { what, .. }) => Some(*what),
                _ => None,
            })
            .collect()
    }

    /// One segment as the VAD would mark it: speech, the first quiet frame (where the
    /// early read starts), then the stop's quiet; `resume` puts speech back in between.
    async fn segment(mode: EarlyReadMode, resume: bool) -> (Vec<Frame>, Vec<usize>) {
        let (model, calls) = counting(30);
        let mut stt =
            ParakeetStt::new(model, None, VadParams::default(), 0.0).with_early_read(mode);
        let (out, mut rx) = Out::capture();
        stt.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        for f in speech(0.6) {
            stt.process(f, Direction::Down, &out).await;
        }
        stt.silence_began(&out);
        if resume {
            for f in quiet(0.1) {
                stt.process(f, Direction::Down, &out).await;
            }
            stt.speech_resumed(&out).await;
            for f in speech(0.4) {
                stt.process(f, Direction::Down, &out).await;
            }
            stt.silence_began(&out);
        }
        // The rest of the VAD's stop: 0.2 s of quiet, then the stop.
        for f in quiet(0.2) {
            stt.process(f, Direction::Down, &out).await;
        }
        stt.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let down = drain(&mut rx).await;
        let calls = calls.lock().unwrap().clone();
        (down, calls)
    }

    #[tokio::test]
    async fn test_speculate_sends_the_early_words_on_and_the_stops_words_are_the_segments() {
        // ADR 0054: the early read's words go on as soon as they are read; the segment's
        // transcript is the stop's read, word for word the read `Off` makes.
        let (off, _) = segment(EarlyReadMode::Off, false).await;
        let (spec, calls) = segment(EarlyReadMode::Speculate, false).await;
        let finals = |down: &[Frame]| -> Vec<String> {
            down.iter()
                .filter_map(|f| match f {
                    Frame::Transcript(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(finals(&spec), finals(&off), "the stop's words, as today");
        assert_eq!(calls.len(), 2, "the early read and the stop's");
        let early: Vec<usize> = spec
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f, Frame::EarlyTranscript(_)))
            .map(|(i, _)| i)
            .collect();
        let last = spec
            .iter()
            .position(|f| matches!(f, Frame::Transcript(_)))
            .unwrap();
        assert_eq!(early.len(), 1, "{spec:?}");
        assert!(early[0] < last, "the early words come before the stop's");
        // Speech again before the stop: the half segment's early words never go on.
        let (resumed, _) = segment(EarlyReadMode::Speculate, true).await;
        let early: Vec<&Frame> = resumed
            .iter()
            .filter(|f| matches!(f, Frame::EarlyTranscript(_)))
            .collect();
        assert_eq!(early.len(), 1, "only the whole segment's: {resumed:?}");
        assert!(early_reads(&resumed).contains(&EARLY_DISCARDED));
    }

    #[tokio::test]
    async fn test_a_segment_read_from_its_first_quiet_frame_says_what_the_stop_would() {
        let (off, off_calls) = segment(EarlyReadMode::Off, false).await;
        let (on, on_calls) = segment(EarlyReadMode::On, false).await;
        // One read each, of audio of the same length, and the same words.
        assert_eq!(off_calls.len(), 1);
        assert_eq!(on_calls, off_calls, "the same length of audio");
        assert_eq!(finals(&on), finals(&off));
        assert_eq!(finals(&on).len(), 1);
        assert_eq!(early_reads(&on), vec![EARLY_USED]);
        assert!(early_reads(&off).is_empty());
        // Verifying reads twice, keeps the stop's words, and says they were the same.
        let (verify, calls) = segment(EarlyReadMode::Verify, false).await;
        assert_eq!(calls.len(), 2);
        assert_eq!(finals(&verify), finals(&off));
        assert_eq!(early_reads(&verify), vec!["same"]);
    }

    /// One segment through a stage that reads `text`, with `speaking` on or off: what
    /// `on_read` was told.
    async fn reads(text: &str, ghost_speaking: bool) -> Vec<(f64, usize)> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let speaking = Speaking::new();
        speaking.set(ghost_speaking);
        let mut stt = ParakeetStt::new(model(text, 0.0).shared, None, VadParams::default(), 0.0)
            .with_speaking(speaking)
            .with_on_read(Some(Arc::new(move |db, words| {
                s.lock().unwrap().push((db, words))
            })));
        let (out, _rx) = Out::capture();
        stt.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        for f in speech(0.5) {
            stt.process(f, Direction::Down, &out).await;
        }
        stt.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn test_each_read_is_reported_as_its_level_and_word_count() {
        // A loud segment read as nothing inside a session is what the passive check
        // watches for (ADR 0064): the daemon's health hears it as numbers only.
        let empty = reads("", false).await;
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].1, 0);
        assert!(empty[0].0 > -35.0, "{}", empty[0].0);
        assert_eq!(reads("what is on today", false).await[0].1, 4);
        // Ghost talking: its echo is not the microphone's fault
        assert!(reads("", true).await.is_empty());
    }

    #[tokio::test]
    async fn test_speech_again_before_the_stop_discards_the_early_read() {
        let (off, _) = segment(EarlyReadMode::Off, true).await;
        let (on, calls) = segment(EarlyReadMode::On, true).await;
        // The first quiet's read was of half the segment and is thrown away; the second
        // quiet's read is the whole segment's, and says what the stop's read says.
        assert_eq!(early_reads(&on), vec![EARLY_DISCARDED, EARLY_USED]);
        assert_eq!(calls.len(), 2);
        assert_eq!(finals(&on), finals(&off));
        assert_eq!(finals(&on).len(), 1);
    }

    #[tokio::test]
    async fn test_the_early_read_is_not_waited_for_twice_at_the_stop() {
        // With the read 0.15 s long and started 0.2 s before the stop, the words come at
        // the stop: the stage reports the time it waited, not the read's.
        let (model, _) = counting(150);
        let mut stt = ParakeetStt::new(model, None, VadParams::default(), 0.0)
            .with_early_read(EarlyReadMode::On);
        let (out, mut rx) = Out::capture();
        stt.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        for f in speech(0.5) {
            stt.process(f, Direction::Down, &out).await;
        }
        stt.silence_began(&out);
        for f in quiet(0.2) {
            stt.process(f, Direction::Down, &out).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stt.process(Frame::VadStopped { stop_secs: 0.2 }, Direction::Down, &out)
            .await;
        let down = drain(&mut rx).await;
        let waited: Vec<f64> = down
            .iter()
            .filter_map(|f| match f {
                Frame::Metrics(Metric::Ttfb { value, .. }) => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(waited.len(), 1);
        assert!(waited[0] < 0.05, "waited {}s at the stop", waited[0]);
        assert_eq!(finals(&down).len(), 1);
    }

    // --- one recognition at a time ---------------------------------------------------

    #[tokio::test]
    async fn test_recognitions_never_overlap_and_busy_ticks_are_skipped() {
        let m = model("stop that", 0.1); // slower than the interval
        let down = run(
            m.fork(0.03),
            seq(vec![
                vec![bot_started(), vad_start()],
                audio(0.5),
                vec![vad_stop(), Step::Sleep(0.2)],
            ]),
        )
        .await;
        assert_eq!(m.most_at_once(), 1);
        let n = interims(&down).len();
        assert!((2..=6).contains(&n), "{n}"); // not one per 0.03 s tick
        assert_eq!(finals(&down), vec!["stop that"]);
    }

    #[tokio::test]
    async fn test_an_interim_in_flight_at_the_pause_is_dropped_and_the_final_waits() {
        for goes_on in [false, true] {
            let m = model("stop that", 0.2);
            // the first interim starts 0.1 s in and is still running when the segment
            // ends; the operator may already be in the next one when it returns, with the
            // old one's words
            let then = if goes_on {
                seq(vec![vec![vad_start()], audio(0.1)])
            } else {
                Vec::new()
            };
            let down = run(
                m.fork(0.1),
                seq(vec![
                    vec![bot_started(), vad_start()],
                    audio(0.14),
                    vec![vad_stop()],
                    then,
                    vec![Step::Sleep(0.5)],
                ]),
            )
            .await;
            assert!(interims(&down).is_empty(), "goes_on={goes_on}"); // stale: the final is the word
            assert_eq!(finals(&down), vec!["stop that"]);
            assert_eq!(m.calls(), 2, "goes_on={goes_on}");
            assert_eq!(m.most_at_once(), 1);
        }
    }

    #[test]
    fn test_forks_share_one_model_and_one_turn_at_it() {
        let m = model("stop that", 0.0);
        let (session, wake) = (m.fork(0.3), m.fork(0.0));
        assert!(Arc::ptr_eq(session.model(), wake.model()));
        let pcm = vec![0.0f32; 1600];
        {
            let _busy = wake.model().lock().unwrap(); // another pipeline's final is running
            assert_eq!(recognize(session.model(), &pcm, false), None);
        }
        assert_eq!(m.calls(), 0);
        assert_eq!(
            recognize(session.model(), &pcm, false).as_deref(),
            Some("stop that")
        );
    }

    // --- only barge-in reads them -------------------------------------------------------

    fn interim(text: &str) -> Frame {
        Frame::Interim(text.into())
    }

    struct Catcher(Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl TurnInterceptor for Catcher {
        fn name(&self) -> &str {
            "always"
        }
        async fn on_user_turn(&self, turn: &UserTurn) -> Option<Handled> {
            self.0.lock().unwrap().push(turn.text.clone());
            None
        }
    }

    struct Gate(Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl SessionGate for Gate {
        fn pending(&self, _s: &str) -> bool {
            true // a change waits for a yes
        }
        fn cancel(&self, _s: &str, _r: &str) {}
        fn set_aside(&self, _s: &str, _r: &str) {}
        async fn on_user_turn(&self, _s: &str, text: &str, _q: &str) -> (Verdict, Option<Outcome>) {
            self.0.lock().unwrap().push(text.to_string());
            (Verdict::Unclear, None)
        }
        fn last_classification(&self) -> Option<Classification> {
            None
        }
    }

    async fn interims_of(rx: &mut Receiver<crate::frames::Pushed>) -> Vec<String> {
        interims(&drain(rx).await)
    }

    #[tokio::test]
    async fn test_interims_reach_no_transcript_store_interceptor_or_confirmation() {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let t = turns.clone();
        let on_turn: crate::tap::OnTurn = Arc::new(move |role, text| {
            let t = t.clone();
            Box::pin(async move {
                t.lock().unwrap().push((role, text));
            })
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let judged = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Gate(judged.clone()));
        let mut tap = UserTap::new(on_turn, no_phase(), None);
        let mut intercept = InterceptorProcessor::new(
            vec![Arc::new(Catcher(seen.clone()))],
            gate.clone(),
            "s",
            no_action(),
        );
        let mut confirm = ConfirmationProcessor::new(gate, "s", Arc::new(String::new));
        let stages: Vec<&mut dyn Stage> = vec![&mut tap, &mut intercept, &mut confirm];
        for stage in stages {
            let (out, mut rx) = Out::capture();
            // Ghost is quiet: the moment a final would be taken as a turn or a yes
            stage
                .process(interim("yes close it"), Direction::Down, &out)
                .await;
            assert_eq!(interims_of(&mut rx).await, vec!["yes close it"]); // passed on, for the turn strategy below
        }
        assert!(turns.lock().unwrap().is_empty());
        assert!(seen.lock().unwrap().is_empty());
        assert!(judged.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_hold_drops_interims() {
        let on_resume: OnResume = Arc::new(|_rest| {
            Box::pin(async {
                panic!("an interim never resumes a hold");
            })
        });
        let mut hold = HoldGate::new(on_resume, None, 0.0);
        let (out, mut rx) = Out::capture();
        hold.hold();
        hold.process(interim("okay ghost we're back"), Direction::Down, &out)
            .await;
        assert!(drain(&mut rx).await.is_empty());
    }

    #[tokio::test]
    async fn test_an_interim_echo_of_two_of_ghosts_words_is_dropped() {
        let mut echo = EchoFilter::new(3.0);
        let (out, mut rx) = Out::capture();
        echo.spoke("I found three open issues in the app repository.");
        echo.process(Frame::BotStarted, Direction::Down, &out).await;
        for f in [
            interim("three issues"),
            interim("wait stop"),
            interim("stop the"),
        ] {
            echo.process(f, Direction::Down, &out).await;
        }
        echo.process(
            Frame::Transcript("three issues".into()),
            Direction::Down,
            &out,
        )
        .await;
        let texts = |frames: &[Frame]| -> Vec<String> {
            frames
                .iter()
                .filter_map(|f| match f {
                    Frame::Interim(t) | Frame::Transcript(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        // While Ghost speaks a two-word final can only be a barge-in, and these are
        // Ghost's words: dropped like the interim.
        assert_eq!(texts(&drain(&mut rx).await), vec!["wait stop", "stop the"]);
        // Once Ghost has stopped the same two words are a short answer, kept.
        echo.process(Frame::BotStopped, Direction::Down, &out).await;
        echo.process(
            Frame::Transcript("three issues".into()),
            Direction::Down,
            &out,
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), vec!["three issues"]);
    }
}
