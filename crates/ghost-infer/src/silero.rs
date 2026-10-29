//! Silero VAD as Pipecat runs it (`pipecat.audio.vad.silero`): the stateful model over
//! 512-sample chunks at 16 kHz with 64 samples of context carried between chunks, and the
//! analyzer's start/stop logic (`pipecat.audio.vad.vad_analyzer`): a chunk is speech when the
//! model's confidence and the smoothed BS.1770 volume both clear their thresholds, speaking
//! starts after `start_secs` of such chunks and stops after `stop_secs` without.
//! One intra-op thread, one inter-op thread.

use std::path::Path;

use ort::{session::Session, value::Tensor};

use crate::{
    InferError, Result, loudness,
    session::{self, SessionConfig},
};

pub const SAMPLE_RATE: u32 = 16_000;
/// Samples per model call at 16 kHz.
pub const CHUNK: usize = 512;
const CONTEXT: usize = 64;
const STATE: usize = 2 * 128;
/// How often Pipecat resets the model's state, in seconds of its clock.
pub const MODEL_RESET_STATES_SECS: f64 = 5.0;

/// The model with its recurrent state and the context window it keeps.
pub struct Silero {
    session: Session,
    state: Vec<f32>,
    context: Vec<f32>,
    primed: bool,
}

impl Silero {
    pub fn load(path: &Path) -> Result<Self> {
        let session = session::open(path, &SessionConfig::threads(1))?;
        Ok(Self {
            session,
            state: vec![0.0; STATE],
            context: vec![0.0; CONTEXT],
            primed: false,
        })
    }

    /// Zero the recurrent state and the context (`SileroOnnxModel.reset_states`).
    pub fn reset_states(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
        self.primed = false;
    }

    /// The probability of speech in one 512-sample chunk (float32 in `[-1, 1)`).
    pub fn probability(&mut self, chunk: &[f32]) -> Result<f32> {
        if chunk.len() != CHUNK {
            return Err(InferError::Audio(format!(
                "Provided number of samples is {} (Supported values: 512 for 16000)",
                chunk.len()
            )));
        }
        if !self.primed {
            self.context.fill(0.0);
            self.primed = true;
        }
        let mut input = Vec::with_capacity(CONTEXT + CHUNK);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(chunk);
        let outputs = self.session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, CONTEXT + CHUNK], input.clone()))?,
            "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone()))?,
            "sr" => Tensor::from_array(([0usize; 0], vec![i64::from(SAMPLE_RATE)]))?,
        ])?;
        let (_, out) = session::output_f32(&outputs, "output")?;
        let (_, state) = session::output_f32(&outputs, "stateN")?;
        drop(outputs);
        self.state = state;
        self.context.copy_from_slice(&input[CHUNK..]);
        out.first()
            .copied()
            .ok_or_else(|| InferError::Model("Silero returned no output".into()))
    }
}

/// Pipecat's `VADParams`, with Ghost's defaults (`ghost_voice.config.VoiceSettings`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadParams {
    /// Minimum confidence for voice.
    pub confidence: f32,
    /// How long voice must hold before speaking starts.
    pub start_secs: f32,
    /// How long silence must hold before speaking stops.
    pub stop_secs: f32,
    /// Minimum smoothed volume for voice.
    pub min_volume: f32,
}

impl Default for VadParams {
    fn default() -> Self {
        Self {
            confidence: 0.7,
            start_secs: 0.2,
            stop_secs: 0.2,
            min_volume: 0.6,
        }
    }
}

/// `VADState`, numbered as Pipecat's enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Quiet = 1,
    Starting = 2,
    Speaking = 3,
    Stopping = 4,
}

/// `AudioVolumeTracker`: the loudness of the last 400 ms, read once the window is full.
struct VolumeTracker {
    window: usize,
    buffer: Vec<i16>,
    volume: Option<f64>,
}

impl VolumeTracker {
    const WINDOW_SECS: f64 = 0.4;

    fn new(sample_rate: u32) -> Self {
        Self {
            window: (Self::WINDOW_SECS * f64::from(sample_rate)).ceil() as usize,
            buffer: Vec::new(),
            volume: Some(0.0),
        }
    }

    fn update(&mut self, audio: &[i16]) {
        self.buffer.extend_from_slice(audio);
        if self.buffer.len() > self.window {
            let drop = self.buffer.len() - self.window;
            self.buffer.drain(..drop);
        }
        if self.buffer.len() == self.window {
            self.volume = None;
        }
    }

    fn volume(&mut self) -> f64 {
        *self
            .volume
            .get_or_insert_with(|| loudness::volume(&self.buffer, SAMPLE_RATE))
    }
}

/// `SileroVADAnalyzer` with `VADAnalyzer._run_analyzer`.
pub struct VadAnalyzer {
    model: Silero,
    params: VadParams,
    start_frames: usize,
    stop_frames: usize,
    starting_count: usize,
    stopping_count: usize,
    state: VadState,
    pending: Vec<i16>,
    volume: VolumeTracker,
    prev_volume: f64,
    last_reset_time: f64,
}

impl VadAnalyzer {
    const SMOOTHING: f64 = 0.2;

    pub fn new(model: Silero, params: VadParams) -> Self {
        let mut analyzer = Self {
            model,
            params,
            start_frames: 0,
            stop_frames: 0,
            starting_count: 0,
            stopping_count: 0,
            state: VadState::Quiet,
            pending: Vec::new(),
            volume: VolumeTracker::new(SAMPLE_RATE),
            prev_volume: 0.0,
            last_reset_time: 0.0,
        };
        analyzer.set_params(params);
        analyzer
    }

    /// Set the parameters and recalculate the frame counts. The VAD state resets, the
    /// volume state does not: the rolling window follows the audio stream.
    pub fn set_params(&mut self, params: VadParams) {
        self.params = params;
        let frames_per_sec = CHUNK as f64 / f64::from(SAMPLE_RATE);
        self.start_frames =
            (f64::from(params.start_secs) / frames_per_sec).round_ties_even() as usize;
        self.stop_frames =
            (f64::from(params.stop_secs) / frames_per_sec).round_ties_even() as usize;
        self.starting_count = 0;
        self.stopping_count = 0;
        self.state = VadState::Quiet;
    }

    pub fn params(&self) -> VadParams {
        self.params
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    /// `SileroVADAnalyzer.voice_confidence`: the model on one chunk, then a state reset
    /// every `MODEL_RESET_STATES_SECS` of `now` (Pipecat reads `time.time()`, so pass epoch
    /// seconds live; the first call resets).
    pub fn voice_confidence(&mut self, chunk: &[i16], now: f64) -> Result<f32> {
        let audio = crate::audio::to_f32(chunk);
        let confidence = self.model.probability(&audio)?;
        if now - self.last_reset_time >= MODEL_RESET_STATES_SECS {
            self.model.reset_states();
            self.last_reset_time = now;
        }
        Ok(confidence)
    }

    /// The smoothed volume after `audio` (`_get_smoothed_volume`).
    fn smoothed_volume(&mut self, audio: &[i16]) -> f64 {
        self.volume.update(audio);
        let volume = self.volume.volume();
        self.prev_volume + Self::SMOOTHING * (volume - self.prev_volume)
    }

    /// One chunk through the state machine (`_run_analyzer`'s loop body): the model's
    /// confidence and the smoothed volume it compared.
    fn step(&mut self, chunk: &[i16], now: f64) -> Result<(f32, f64)> {
        let confidence = self.voice_confidence(chunk, now)?;
        let volume = self.smoothed_volume(chunk);
        self.prev_volume = volume;
        let speaking =
            confidence >= self.params.confidence && volume >= f64::from(self.params.min_volume);
        if speaking {
            match self.state {
                VadState::Quiet => {
                    self.state = VadState::Starting;
                    self.starting_count = 1;
                }
                VadState::Starting => self.starting_count += 1,
                VadState::Stopping => {
                    self.state = VadState::Speaking;
                    self.stopping_count = 0;
                }
                VadState::Speaking => {}
            }
        } else {
            match self.state {
                VadState::Starting => {
                    self.state = VadState::Quiet;
                    self.starting_count = 0;
                }
                VadState::Speaking => {
                    self.state = VadState::Stopping;
                    self.stopping_count = 1;
                }
                VadState::Stopping => self.stopping_count += 1,
                VadState::Quiet => {}
            }
        }
        Ok((confidence, volume))
    }

    /// After the chunks of one call: a start or stop that has held long enough.
    fn settle(&mut self) {
        if self.state == VadState::Starting && self.starting_count >= self.start_frames {
            self.state = VadState::Speaking;
            self.starting_count = 0;
        }
        if self.state == VadState::Stopping && self.stopping_count >= self.stop_frames {
            self.state = VadState::Quiet;
            self.stopping_count = 0;
        }
    }

    /// Feed audio (any length; whole 512-sample chunks are analyzed, the rest waits) and
    /// get the state after it. `now` is the clock `voice_confidence` resets by.
    pub fn analyze(&mut self, audio: &[i16], now: f64) -> Result<VadState> {
        self.pending.extend_from_slice(audio);
        if self.pending.len() < CHUNK {
            return Ok(self.state);
        }
        let mut at = 0;
        while self.pending.len() - at >= CHUNK {
            let chunk: Vec<i16> = self.pending[at..at + CHUNK].to_vec();
            at += CHUNK;
            self.step(&chunk, now)?;
        }
        self.pending.drain(..at);
        self.settle();
        Ok(self.state)
    }

    /// Exactly one chunk (`analyze` on 512 samples), with what the decision rested on:
    /// the confidence, the smoothed volume, and the state after it.
    pub fn analyze_chunk(&mut self, chunk: &[i16], now: f64) -> Result<(f32, f64, VadState)> {
        if chunk.len() != CHUNK || !self.pending.is_empty() {
            return Err(InferError::Audio(format!(
                "analyze_chunk takes one chunk of {CHUNK} samples with nothing pending"
            )));
        }
        let (confidence, volume) = self.step(chunk, now)?;
        self.settle();
        Ok((confidence, volume, self.state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_counts_round_like_python() {
        let frames_per_sec = CHUNK as f64 / f64::from(SAMPLE_RATE);
        assert_eq!((0.2 / frames_per_sec).round_ties_even() as usize, 6);
        assert_eq!((0.5 / frames_per_sec).round_ties_even() as usize, 16);
        assert_eq!((0.4 / frames_per_sec).round_ties_even() as usize, 12); // 12.5 -> 12, as round() does
    }

    #[test]
    fn volume_tracker_waits_for_a_full_window() {
        let mut t = VolumeTracker::new(SAMPLE_RATE);
        t.update(&[1000; 512]);
        assert_eq!(t.volume(), 0.0);
        for _ in 0..13 {
            t.update(&[1000; 512]);
        }
        assert_eq!(t.buffer.len(), 6400);
        let step = t.volume(); // a constant: only the filter's transient has energy
        assert!(step > 0.0 && step < 0.6, "{step}");
        let tone: Vec<i16> = (0..6400)
            .map(|i| (10000.0 * (i as f64 * 0.4).sin()) as i16)
            .collect();
        t.update(&tone);
        assert!(t.volume() > step);
    }
}
