//! Parakeet TDT 0.6B v3 (int8) as `onnx_asr` runs it for `ghost_voice.parakeet`: NeMo's
//! log-mel front end (the NumPy preprocessor `onnx_asr` picks on CPU), the encoder, the
//! decoder-joint, and the TDT greedy decode of `onnx_asr.models.nemo` and `onnx_asr.asr`.
//!
//! The front end follows the NumPy to the rounding: the int8 model flips a comma or a
//! capital on a feature difference of 1e-5, so the filterbank is the float32 table NeMo
//! exported (`parakeet_fbanks`), the mel energies accumulate in float32 with one FMA per
//! non-zero weight in bin order (what one K block of OpenBLAS's sgemm does, bit for bit),
//! and the mean and variance are sequential float32 sums over the frames (NumPy's
//! `sum(axis=1)` on a `(1, T, 128)` array). What is left is the log: NumPy's float32 `log`
//! (Intel SVML) differs from a correctly rounded one by an ulp on a few per cent of values.
//!
//! Measured on the target CPU: about 0.16 s per command, 1.7 GB RAM, 10 s to load
//! (docs/benchmarks/2026-10-16-speech-cpu.md). Four `intra_op` threads, one `inter_op`.
//!
//! One recognition runs at a time on the loaded model (`&mut self`): the pipeline's final
//! waits for an interim in flight, and an interim tick that finds the model busy is skipped
//! (`InterimSchedule`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use ort::{session::Session, value::Tensor};
use regex::Regex;
use serde::Deserialize;

use crate::{
    InferError, Result,
    mel::{Spectrum, hann_symmetric, slaney_filterbank},
    session::{self, SessionConfig},
};

#[path = "parakeet_fbanks.rs"]
mod fbanks;

/// An interim reads at most this much of the open segment: enough for the pre-roll and the
/// first words, which is all barge-in needs, and it keeps each pass near 0.2 s when the
/// segment runs long (Parakeet's time grows with the audio, about 30 ms a second).
pub const INTERIM_WINDOW_SECS: f32 = 4.0;

const SAMPLE_RATE: usize = 16_000;

/// The model files, as `onnx_asr` names them in the Hugging Face snapshot.
#[derive(Debug, Clone)]
pub struct ParakeetPaths {
    pub encoder: PathBuf,
    pub decoder_joint: PathBuf,
    pub vocab: PathBuf,
    pub config: Option<PathBuf>,
}

impl ParakeetPaths {
    /// `encoder-model[.<quantization>].onnx` and friends in `dir`.
    pub fn in_dir(dir: &Path, quantization: Option<&str>) -> Self {
        let suffix = quantization.map(|q| format!(".{q}")).unwrap_or_default();
        Self {
            encoder: dir.join(format!("encoder-model{suffix}.onnx")),
            decoder_joint: dir.join(format!("decoder_joint-model{suffix}.onnx")),
            vocab: dir.join("vocab.txt"),
            config: Some(dir.join("config.json")),
        }
    }

    /// The int8 snapshot `onnx_asr` downloaded into the Hugging Face cache, if any.
    pub fn huggingface_cache() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        let snapshots = PathBuf::from(home)
            .join(".cache/huggingface/hub/models--istupakov--parakeet-tdt-0.6b-v3-onnx/snapshots");
        let mut dirs: Vec<PathBuf> = fs::read_dir(&snapshots)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("encoder-model.int8.onnx").exists())
            .collect();
        dirs.sort();
        dirs.first().map(|d| Self::in_dir(d, Some("int8")))
    }
}

#[derive(Debug, Deserialize, Default)]
struct AsrConfig {
    #[serde(default)]
    features_size: Option<usize>,
    #[serde(default)]
    max_tokens_per_step: Option<usize>,
}

/// NeMo's filterbank table as a dense `[bins, filters]` row-major float32 matrix, the
/// numbers `onnx_asr` ships (`preprocessors/data/fbanks.npz`, `nemo128`).
pub fn nemo128_table() -> Vec<f32> {
    let mut out = vec![0.0f32; fbanks::BINS * fbanks::FILTERS];
    for &(bin, filter, bits) in fbanks::WEIGHTS {
        out[usize::from(bin) * fbanks::FILTERS + usize::from(filter)] = f32::from_bits(bits);
    }
    out
}

/// NeMo's preprocessor as `onnx_asr.preprocessors.numpy_preprocessor.NemoPreprocessorNumpy`
/// computes it: pre-emphasis 0.97, a 512-point FFT over a 400-sample Hann window every 160
/// samples, the mel filterbank, `log(mel + 2^-24)`, then per-feature normalization over
/// the valid frames.
struct NemoFeatures {
    n_mels: usize,
    window: Vec<f64>,
    /// Per FFT bin, the filters it feeds and their float32 weights.
    taps: Vec<Vec<(usize, f32)>>,
    spectrum: Spectrum,
    magnitude: Vec<f64>,
    energies: Vec<f32>,
}

impl NemoFeatures {
    const N_FFT: usize = 512;
    const WIN_LENGTH: usize = 400;
    const HOP: usize = 160;
    const PREEMPH: f32 = 0.97;
    /// `log_zero_guard_value`, 2^-24.
    const GUARD: f32 = 5.960_464_5e-8;

    fn new(n_mels: usize) -> Self {
        let pad = (Self::N_FFT - Self::WIN_LENGTH) / 2;
        let mut window = vec![0.0; Self::N_FFT];
        window[pad..pad + Self::WIN_LENGTH].copy_from_slice(&hann_symmetric(Self::WIN_LENGTH));
        let bins = Self::N_FFT / 2 + 1;
        let mut taps: Vec<Vec<(usize, f32)>> = vec![Vec::new(); bins];
        if n_mels == fbanks::FILTERS {
            for &(bin, filter, bits) in fbanks::WEIGHTS {
                taps[usize::from(bin)].push((usize::from(filter), f32::from_bits(bits)));
            }
        } else {
            // No exported table for this size: librosa.filters.mel(sr=16000, n_fft=512,
            // n_mels) in float64, stored as float32 as NeMo does.
            let dense = slaney_filterbank(bins, n_mels, 0.0, 8000.0, SAMPLE_RATE as u32);
            for (bin, row) in taps.iter_mut().enumerate() {
                for m in 0..n_mels {
                    let w = dense[bin * n_mels + m] as f32;
                    if w != 0.0 {
                        row.push((m, w));
                    }
                }
            }
        }
        Self {
            n_mels,
            window,
            taps,
            spectrum: Spectrum::new(Self::N_FFT),
            magnitude: vec![0.0; bins],
            energies: vec![0.0; n_mels],
        }
    }

    /// Features in `[n_mels, frames]` order with the frame count and the valid length.
    fn compute(&mut self, wav: &[f32]) -> Result<(Vec<f32>, usize, usize)> {
        let n = wav.len();
        let valid = n / Self::HOP;
        if valid < 2 {
            return Err(InferError::Audio(
                "too little audio for Parakeet (under 20 ms)".into(),
            ));
        }
        // Pre-emphasis in float32, then n_fft / 2 zeros on either side.
        let mut x = Vec::with_capacity(n + Self::N_FFT);
        x.resize(Self::N_FFT / 2, 0.0f32);
        x.push(wav[0]);
        for i in 1..n {
            x.push(wav[i] - Self::PREEMPH * wav[i - 1]);
        }
        x.resize(n + Self::N_FFT, 0.0);
        let frames = n / Self::HOP + 1;
        let n_mels = self.n_mels;
        let mut frame = vec![0.0f64; Self::N_FFT];
        let mut log_mel = vec![0.0f32; frames * n_mels];
        for t in 0..frames {
            let start = t * Self::HOP;
            for (i, f) in frame.iter_mut().enumerate() {
                *f = f64::from(x[start + i]) * self.window[i];
            }
            self.spectrum.magnitude(&frame, &mut self.magnitude);
            self.energies.fill(0.0);
            for (taps, &magnitude) in self.taps.iter().zip(&self.magnitude) {
                // |X| is cast to float32 before it is squared, as the NumPy does; then one
                // float32 FMA per weight, bins in order, as sgemm accumulates.
                let power = magnitude as f32;
                let power = power * power;
                for &(m, w) in taps {
                    self.energies[m] = power.mul_add(w, self.energies[m]);
                }
            }
            for (m, &e) in self.energies.iter().enumerate() {
                log_mel[t * n_mels + m] = (e + Self::GUARD).ln();
            }
        }
        // Sequential float32 sums over the valid frames, as NumPy reduces the frame axis.
        let mut sum = vec![0.0f32; n_mels];
        for t in 0..valid {
            for (m, s) in sum.iter_mut().enumerate() {
                *s += log_mel[t * n_mels + m];
            }
        }
        let mean: Vec<f32> = sum.iter().map(|s| s / valid as f32).collect();
        let mut squares = vec![0.0f32; n_mels];
        for t in 0..valid {
            for (m, s) in squares.iter_mut().enumerate() {
                let d = log_mel[t * n_mels + m] - mean[m];
                *s += d * d;
            }
        }
        let mut out = vec![0.0f32; n_mels * frames];
        for m in 0..n_mels {
            let var = squares[m] / (valid - 1) as f32;
            let denom = var.sqrt() + 1e-5;
            for t in 0..valid {
                out[m * frames + t] = (log_mel[t * n_mels + m] - mean[m]) / denom;
            }
        }
        Ok((out, frames, valid))
    }
}

/// The loaded model.
pub struct Parakeet {
    encoder: Session,
    decoder_joint: Session,
    features: NemoFeatures,
    vocab: Vec<String>,
    blank: usize,
    vocab_size: usize,
    max_tokens_per_step: usize,
    state_shape: (usize, usize),
    space_pattern: Regex,
}

/// A transcript with the token ids and their text, for the parity tests and for debugging.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub ids: Vec<usize>,
    pub tokens: Vec<String>,
}

impl Parakeet {
    /// Load the three files with `threads` intra-op threads (Ghost uses 4).
    pub fn load(paths: &ParakeetPaths, threads: usize) -> Result<Self> {
        let config: AsrConfig = match &paths.config {
            Some(path) if path.exists() => serde_json::from_str(&fs::read_to_string(path)?)
                .map_err(|e| InferError::Model(format!("{}: {e}", path.display())))?,
            _ => AsrConfig::default(),
        };
        let n_mels = config.features_size.unwrap_or(80);
        let mut vocab: Vec<(usize, String)> = Vec::new();
        for line in fs::read_to_string(&paths.vocab)?.lines() {
            let line = line.trim_end_matches('\n');
            let (token, id) = line.rsplit_once(' ').ok_or_else(|| {
                InferError::Model(format!("{}: bad line {line:?}", paths.vocab.display()))
            })?;
            let id: usize = id.parse().map_err(|_| {
                InferError::Model(format!("{}: bad id in {line:?}", paths.vocab.display()))
            })?;
            vocab.push((id, token.replace('\u{2581}', " ")));
        }
        vocab.sort_by_key(|(id, _)| *id);
        let vocab: Vec<String> = vocab.into_iter().map(|(_, t)| t).collect();
        let blank = vocab
            .iter()
            .position(|t| t == "<blk>")
            .ok_or_else(|| InferError::Model("vocab.txt has no <blk>".into()))?;
        let config_threads = SessionConfig::threads(threads);
        let encoder = session::open(&paths.encoder, &config_threads)?;
        let decoder_joint = session::open(&paths.decoder_joint, &config_threads)?;
        let state_shape = {
            let input = decoder_joint
                .inputs()
                .iter()
                .find(|i| i.name() == "input_states_1")
                .ok_or_else(|| InferError::Model("decoder_joint has no input_states_1".into()))?;
            let dims = match input.dtype() {
                ort::value::ValueType::Tensor { shape, .. } => shape.to_vec(),
                _ => return Err(InferError::Model("input_states_1 is not a tensor".into())),
            };
            if dims.len() != 3 || dims[0] <= 0 || dims[2] <= 0 {
                return Err(InferError::Model(format!(
                    "input_states_1 has shape {dims:?}"
                )));
            }
            (dims[0] as usize, dims[2] as usize)
        };
        Ok(Self {
            encoder,
            decoder_joint,
            features: NemoFeatures::new(n_mels),
            vocab_size: vocab.len(),
            vocab,
            blank,
            max_tokens_per_step: config.max_tokens_per_step.unwrap_or(10),
            state_shape,
            space_pattern: Regex::new(r"\A\s|\s\B|(\s)\b").expect("a fixed pattern"),
        })
    }

    /// The encoder's input for `samples_16k`: features in `[n_mels, frames]` order, the
    /// frame count and the valid length (the parity tests compare them with the Python's).
    pub fn features(&mut self, samples_16k: &[f32]) -> Result<(Vec<f32>, usize, usize)> {
        self.features.compute(samples_16k)
    }

    /// Transcribe float32 16 kHz PCM, stripped, as `ParakeetSTTService.recognize` returns it.
    pub fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String> {
        Ok(self.transcribe_tokens(samples_16k)?.text)
    }

    /// The transcript with its tokens.
    pub fn transcribe_tokens(&mut self, samples_16k: &[f32]) -> Result<Transcript> {
        let (features, frames, valid) = self.features.compute(samples_16k)?;
        let n_mels = self.features.n_mels;
        let outputs = self.encoder.run(ort::inputs![
            "audio_signal" => Tensor::from_array(([1usize, n_mels, frames], features))?,
            "length" => Tensor::from_array(([1usize], vec![valid as i64]))?,
        ])?;
        let (shape, encoded) = session::output_f32(&outputs, "outputs")?;
        let encoded_len = session::output_i64(&outputs, "encoded_lengths")?[0].max(0) as usize;
        drop(outputs);
        if shape.len() != 3 {
            return Err(InferError::Model(format!(
                "encoder outputs have shape {shape:?}"
            )));
        }
        let (dim, t_max) = (shape[1] as usize, shape[2] as usize);
        let encoded_len = encoded_len.min(t_max);
        let ids = self.decode(&encoded, dim, t_max, encoded_len)?;
        let tokens: Vec<String> = ids.iter().map(|&i| self.vocab[i].clone()).collect();
        let joined: String = tokens.concat();
        let text = self
            .space_pattern
            .replace_all(
                &joined,
                |caps: &regex::Captures<'_>| {
                    if caps.get(1).is_some() { " " } else { "" }
                },
            )
            .trim()
            .to_string();
        Ok(Transcript { text, ids, tokens })
    }

    /// The TDT greedy decode of `_AsrWithTransducerDecoding._decoding` with
    /// `NemoConformerTdt._decode`: the joint's first `vocab_size` logits pick the token, the
    /// rest pick how many frames to skip.
    fn decode(
        &mut self,
        encoded: &[f32],
        dim: usize,
        t_max: usize,
        encoded_len: usize,
    ) -> Result<Vec<usize>> {
        let (layers, hidden) = self.state_shape;
        let mut state1 = vec![0.0f32; layers * hidden];
        let mut state2 = vec![0.0f32; layers * hidden];
        let mut tokens: Vec<usize> = Vec::new();
        let mut t = 0usize;
        let mut emitted = 0usize;
        let mut column = vec![0.0f32; dim];
        while t < encoded_len {
            for d in 0..dim {
                column[d] = encoded[d * t_max + t];
            }
            let last = tokens.last().copied().unwrap_or(self.blank) as i32;
            let outputs = self.decoder_joint.run(ort::inputs![
                "encoder_outputs" => Tensor::from_array(([1usize, dim, 1], column.clone()))?,
                "targets" => Tensor::from_array(([1usize, 1], vec![last]))?,
                "target_length" => Tensor::from_array(([1usize], vec![1i32]))?,
                "input_states_1" => Tensor::from_array(([layers, 1usize, hidden], state1.clone()))?,
                "input_states_2" => Tensor::from_array(([layers, 1usize, hidden], state2.clone()))?,
            ])?;
            let (_, joint) = session::output_f32(&outputs, "outputs")?;
            let (_, new1) = session::output_f32(&outputs, "output_states_1")?;
            let (_, new2) = session::output_f32(&outputs, "output_states_2")?;
            drop(outputs);
            if joint.len() < self.vocab_size {
                return Err(InferError::Model(format!(
                    "the joint returned {} logits for a vocabulary of {}",
                    joint.len(),
                    self.vocab_size
                )));
            }
            let token = argmax(&joint[..self.vocab_size]);
            let step = argmax(&joint[self.vocab_size..]);
            if token != self.blank {
                state1 = new1;
                state2 = new2;
                tokens.push(token);
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank || emitted == self.max_tokens_per_step {
                t += 1;
                emitted = 0;
            }
        }
        Ok(tokens)
    }
}

/// `np.argmax`: the first of the largest.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}

/// The last `INTERIM_WINDOW_SECS` of an open segment plus the pipeline's trailing silence,
/// as float32 for an interim read.
pub fn interim_audio(segment: &[i16], trailing_silence: &[i16]) -> Vec<f32> {
    let window = (SAMPLE_RATE as f32 * INTERIM_WINDOW_SECS) as usize;
    let start = segment.len().saturating_sub(window);
    let mut out = crate::audio::to_f32(&segment[start..]);
    out.extend(crate::audio::to_f32(trailing_silence));
    out
}

/// When the open segment is read again while Ghost speaks (`ParakeetSTTService`'s interim
/// transcripts). With `interim_secs` > 0, while Ghost is speaking and the operator's segment
/// is open, the segment so far is transcribed again every `interim_secs`, the first one an
/// interval in. Only the barge-in rule and the echo filter read those. A segment counter
/// bumped at each VAD start and stop tells a late interim it is stale.
#[derive(Debug, Clone)]
pub struct InterimSchedule {
    interim_secs: f64,
    bot_speaking: bool,
    user_speaking: bool,
    segment: u64,
    due: Option<f64>,
}

impl InterimSchedule {
    pub fn new(interim_secs: f64) -> Self {
        Self {
            interim_secs,
            bot_speaking: false,
            user_speaking: false,
            segment: 0,
            due: None,
        }
    }

    pub fn bot_started(&mut self) {
        self.bot_speaking = true;
    }

    /// `BotStoppedSpeakingFrame` or an interruption, which silences Ghost before it.
    pub fn bot_stopped(&mut self) {
        self.bot_speaking = false;
    }

    pub fn vad_started(&mut self) {
        self.user_speaking = true;
        self.segment += 1;
        self.due = None;
    }

    pub fn vad_stopped(&mut self) {
        self.user_speaking = false;
        self.segment += 1;
        self.due = None;
    }

    pub fn segment(&self) -> u64 {
        self.segment
    }

    /// On each audio frame, with the monotonic clock: the segment to read now, or `None`.
    /// `busy` says a recognition is in flight, and the tick is skipped.
    pub fn tick(&mut self, now: f64, busy: bool) -> Option<u64> {
        if self.interim_secs <= 0.0 || !(self.bot_speaking && self.user_speaking) {
            self.due = None;
            return None;
        }
        match self.due {
            None => {
                self.due = Some(now + self.interim_secs); // the first one an interval in
                None
            }
            Some(due) if now < due => None,
            Some(_) => {
                self.due = Some(now + self.interim_secs);
                if busy { None } else { Some(self.segment) }
            }
        }
    }

    /// Whether an interim read of `segment` is still worth pushing: the segment is the
    /// open one and both are still speaking. (Empty text is dropped by the caller.)
    pub fn is_current(&self, segment: u64) -> bool {
        segment == self.segment && self.bot_speaking && self.user_speaking
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_takes_the_first_of_equals() {
        assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
        assert_eq!(argmax(&[0.0]), 0);
    }

    #[test]
    fn the_space_pattern_joins_tokens_like_onnx_asr() {
        let re = Regex::new(r"\A\s|\s\B|(\s)\b").unwrap();
        let join = |s: &str| {
            re.replace_all(
                s,
                |caps: &regex::Captures<'_>| {
                    if caps.get(1).is_some() { " " } else { "" }
                },
            )
            .to_string()
        };
        // Tokens concatenate with the spaces their "▁" became; the pattern drops the
        // leading one and any before punctuation, and keeps one before a word.
        assert_eq!(join(" Hey guys, what's up?"), "Hey guys, what's up?");
        assert_eq!(join(" Done ."), "Done.");
        assert_eq!(join(" a  b"), "a b");
        assert_eq!(join(""), "");
    }

    #[test]
    fn features_shape_follows_the_hop() {
        let mut f = NemoFeatures::new(128);
        let wav: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (out, frames, valid) = f.compute(&wav).unwrap();
        assert_eq!(frames, 101);
        assert_eq!(valid, 100);
        assert_eq!(out.len(), 128 * 101);
        // The frame past the valid length is zero, the rest is normalized.
        assert!((0..128).all(|m| out[m * frames + 100] == 0.0));
        let mean: f32 = (0..100).map(|t| out[t]).sum::<f32>() / 100.0;
        assert!(mean.abs() < 1e-3, "{mean}");
    }

    #[test]
    fn interim_schedule_runs_an_interval_in_and_skips_when_busy() {
        let mut s = InterimSchedule::new(0.3);
        assert_eq!(s.tick(0.0, false), None); // nobody speaking
        s.bot_started();
        s.vad_started();
        assert_eq!(s.tick(1.0, false), None); // the first one an interval in
        assert_eq!(s.tick(1.2, false), None);
        assert_eq!(s.tick(1.3, true), None); // busy: skipped, rescheduled
        assert_eq!(s.tick(1.5, false), None);
        assert_eq!(s.tick(1.6, false), Some(1));
        assert!(s.is_current(1));
        s.vad_stopped();
        assert!(!s.is_current(1));
        assert_eq!(s.tick(2.0, false), None);
        s.vad_started();
        s.bot_stopped();
        assert_eq!(s.tick(3.0, false), None);
        assert!(!s.is_current(3));
    }

    #[test]
    fn interim_audio_takes_the_last_four_seconds() {
        let segment = vec![1i16; 16_000 * 6];
        let out = interim_audio(&segment, &[0; 160]);
        assert_eq!(out.len(), 16_000 * 4 + 160);
        assert_eq!(out[0], 1.0 / 32768.0);
        assert_eq!(*out.last().unwrap(), 0.0);
    }
}
