//! Kokoro's vocoder a window of frames at a time (ADR 0053), so a piece's first audio comes
//! after its encoder and one short window instead of after the whole render.
//!
//! Kokoro v1.0 is two halves in one graph: the encoder (BERT, the duration and prosody
//! predictors, the alignment: 20 to 100 ms for 2 to 32 words on the workstation) and the
//! decoder, the vocoder, which is 92 to 97 % of the time and grows with the audio (0.16 to
//! 0.21 s per second of speech). `tools/voice-trial/voice_trial/split_kokoro.py` cuts the
//! graph at that line without changing either half, and gives the decoder one more input,
//! `phase`: the harmonic source's running phase in cycles, added after its cumulative sum, so
//! a window that starts at frame `k` has the phase the whole render had there. Run one after
//! the other over every frame, the halves give the whole model's samples exactly.
//!
//! Over a window they do not: the decoder's AdaIN layers normalize each channel over the
//! frames they are given, so a window's audio is the vocoder's answer for those frames alone,
//! as a clause cut is today. Context on both sides of each window, a short crossfade in the
//! overlap and the carried phase keep the joins smooth; the level and timbre of a window can
//! still differ from the whole render's by a decibel or two
//! (docs/benchmarks/perf/2026-11-12-voice-speed.md, third section). That is why it is a
//! setting (`GHOST_TTS_STREAM=windowed`) and the whole render stays the default until the
//! operator has heard it.

use std::path::{Path, PathBuf};

use ort::{session::Session, value::Tensor};

use crate::{
    InferError, Result,
    session::{self, SessionConfig},
};

/// Samples per encoder frame at 24 kHz; the F0 and noise curves have two frames to one.
pub const HOP: usize = 600;
/// The sine generator's harmonics (the fundamental and eight more).
pub const HARMONICS: usize = 9;
const ASR_CHANNELS: usize = 512;
const RATE: f32 = 24_000.0;

/// The two halves' files.
#[derive(Debug, Clone, PartialEq)]
pub struct HalvesPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
}

impl HalvesPaths {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            encoder: dir.join("kokoro-encoder.onnx"),
            decoder: dir.join("kokoro-decoder.onnx"),
        }
    }

    pub fn exist(&self) -> bool {
        self.encoder.is_file() && self.decoder.is_file()
    }
}

/// How a piece is cut into windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlan {
    /// The first window's frames (25 ms each).
    pub first: usize,
    /// Each window after is this many times the one before.
    pub grow: f32,
    /// Frames of context decoded on each side of a window and thrown away.
    pub context: usize,
    /// Frames of the leading pad kept before the first word.
    pub lead_margin: usize,
    /// Frames over which a window fades in over the one before.
    pub fade: usize,
    /// A piece with no more frames than this after its lead is one window: a piece of 1.2 s or less
    /// renders quickly whole, and windows cost it the most in level (a two-word piece
    /// came out 3.5 LU quieter cut in three).
    pub whole_below: usize,
}

impl Default for WindowPlan {
    fn default() -> Self {
        Self {
            first: 8,
            grow: 2.0,
            context: 16,
            lead_margin: 2,
            fade: 2,
            whole_below: 48,
        }
    }
}

impl WindowPlan {
    /// The windows `[start, end)` over `start..frames`: the first `first` frames, each next
    /// one `grow` times longer, a short remainder joined to the window before it.
    pub fn windows(&self, start: usize, frames: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if start < frames && frames - start <= self.whole_below {
            return vec![(start, frames)];
        }
        let (mut s, mut size) = (start, self.first.max(1));
        while s < frames {
            let mut e = (s + size).min(frames);
            if frames - e < (size / 2).max(4) {
                e = frames;
            }
            out.push((s, e));
            s = e;
            size = ((size as f32) * self.grow.max(1.0)).round() as usize;
        }
        out
    }
}

/// What the encoder says about one batch of tokens.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// `[512, frames]`, row-major.
    pub asr: Vec<f32>,
    /// `2 * frames`.
    pub f0: Vec<f32>,
    pub n: Vec<f32>,
    /// Frames per token, the pads included.
    pub durations: Vec<i64>,
    pub frames: usize,
}

/// The two halves, loaded.
pub struct Halves {
    encoder: Session,
    decoder: Session,
}

impl Halves {
    pub fn load(paths: &HalvesPaths) -> Result<Self> {
        let config = SessionConfig {
            intra_threads: 0,
            inter_threads: 0,
            entries: Vec::new(),
        };
        let encoder = session::open(&paths.encoder, &config)?;
        let decoder = session::open(&paths.decoder, &config)?;
        if !decoder.inputs().iter().any(|i| i.name() == "phase") {
            return Err(InferError::Model(format!(
                "{} has no phase input; make it with tools/voice-trial's split_kokoro.py --phase",
                paths.decoder.display()
            )));
        }
        Ok(Self { encoder, decoder })
    }

    /// The encoder over `tokens` (without the pads; they are added here, as `Kokoro::infer`
    /// adds them).
    pub fn encode(&mut self, tokens: &[i64], style: &[f32], speed: f32) -> Result<Encoded> {
        let mut padded = Vec::with_capacity(tokens.len() + 2);
        padded.push(0);
        padded.extend_from_slice(tokens);
        padded.push(0);
        let n = padded.len();
        let outputs = self.encoder.run(ort::inputs![
            "tokens" => Tensor::from_array(([1usize, n], padded))?,
            "style" => Tensor::from_array(([1usize, 256], style.to_vec()))?,
            "speed" => Tensor::from_array(([1usize], vec![speed]))?,
        ])?;
        let (shape, asr) = session::output_f32(&outputs, "asr")?;
        let (_, f0) = session::output_f32(&outputs, "f0")?;
        let (_, nn) = session::output_f32(&outputs, "n")?;
        let durations = session::output_i64(&outputs, "durations")?;
        let frames = *shape.last().unwrap_or(&0) as usize;
        if asr.len() != ASR_CHANNELS * frames || f0.len() != 2 * frames || nn.len() != 2 * frames {
            return Err(InferError::Model(format!(
                "the encoder's outputs do not agree: asr {shape:?}, f0 {}, n {}",
                f0.len(),
                nn.len()
            )));
        }
        Ok(Encoded {
            asr,
            f0,
            n: nn,
            durations,
            frames,
        })
    }

    /// The decoder over frames `[start, end)` of `enc`, with the phase the whole render has
    /// at `start`.
    pub fn decode(
        &mut self,
        enc: &Encoded,
        start: usize,
        end: usize,
        style: &[f32],
    ) -> Result<Vec<f32>> {
        let len = end - start;
        let mut asr = Vec::with_capacity(ASR_CHANNELS * len);
        for c in 0..ASR_CHANNELS {
            let row = c * enc.frames;
            asr.extend_from_slice(&enc.asr[row + start..row + end]);
        }
        let f0 = enc.f0[2 * start..2 * end].to_vec();
        let n = enc.n[2 * start..2 * end].to_vec();
        let phase = phase_before(&enc.f0, 2 * start).to_vec();
        let outputs = self.decoder.run(ort::inputs![
            "asr" => Tensor::from_array(([1usize, ASR_CHANNELS, len], asr))?,
            "f0" => Tensor::from_array(([1usize, 2 * len], f0))?,
            "n" => Tensor::from_array(([1usize, 2 * len], n))?,
            "style" => Tensor::from_array(([1usize, 256], style.to_vec()))?,
            "phase" => Tensor::from_array(([1usize, 1, HARMONICS], phase))?,
        ])?;
        let (_, audio) = session::first_output_f32(&outputs)?;
        Ok(audio)
    }

    /// One batch's audio, window by window: `emit` gets each window's samples as they are
    /// made, the leading pad skipped (less `plan.lead_margin` frames) and the tail trimmed,
    /// `pause` seconds of silence after the last. `emit` returning false stops the render.
    /// Returns false when stopped.
    pub fn render(
        &mut self,
        tokens: &[i64],
        style: &[f32],
        speed: f32,
        pause: f32,
        plan: &WindowPlan,
        emit: &mut dyn FnMut(Vec<f32>) -> bool,
    ) -> Result<bool> {
        let enc = self.encode(tokens, style, speed)?;
        let frames = enc.frames;
        if frames == 0 {
            return Ok(true);
        }
        let lead = (enc.durations.first().copied().unwrap_or(0).max(0) as usize)
            .saturating_sub(plan.lead_margin)
            .min(frames.saturating_sub(1));
        let windows = plan.windows(lead, frames);
        let fade_n = plan.fade * HOP;
        let mut tail: Vec<f32> = Vec::new();
        let last = windows.len().saturating_sub(1);
        for (i, &(s, e)) in windows.iter().enumerate() {
            let ws = s.saturating_sub(plan.context);
            let we = (e + plan.context).min(frames);
            let audio = self.decode(&enc, ws, we, style)?;
            let from = ((s - ws) * HOP).min(audio.len());
            let to = ((e - ws) * HOP).min(audio.len());
            let mut body = audio[from..to].to_vec();
            let k = fade_n.min(tail.len()).min(body.len());
            for (j, v) in body.iter_mut().take(k).enumerate() {
                let w = j as f32 / k as f32;
                *v = tail[j] * (1.0 - w) + *v * w;
            }
            tail = audio[to..(to + fade_n).min(audio.len())].to_vec();
            if i == 0 {
                let (start, _) = crate::kokoro::trim_silence(&body);
                if start < body.len() {
                    body.drain(..start);
                }
            }
            if i == last {
                let (start, end) = crate::kokoro::trim_silence(&body);
                if end > start {
                    body.truncate(end);
                }
                if pause > 0.0 {
                    body.extend(std::iter::repeat_n(0.0, (pause * RATE) as usize));
                }
            }
            if !body.is_empty() && !emit(body) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The sine generator's phase in cycles at F0 frame `start`, for each harmonic: the sum,
/// over the frames before it, of each harmonic's cycles per sample, each frame's reduced mod
/// 1 as the model reduces it (`(f0 * h / 24000) % 1`, then the cumulative sum), and the total
/// mod 1. The model's own interpolation down to the frame rate samples each F0 frame at its
/// middle, where the upsampled curve is that frame's value, so the sum is exact.
pub fn phase_before(f0: &[f32], start: usize) -> [f32; HARMONICS] {
    let mut out = [0.0f32; HARMONICS];
    for (h, slot) in out.iter_mut().enumerate() {
        let harmonic = (h + 1) as f32;
        let mut total = 0.0f64;
        for &f in &f0[..start.min(f0.len())] {
            total += f64::from((f * harmonic / RATE).rem_euclid(1.0));
        }
        *slot = total.rem_euclid(1.0) as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_start_small_grow_and_cover_every_frame() {
        let plan = WindowPlan {
            whole_below: 0,
            ..WindowPlan::default()
        };
        let w = plan.windows(10, 132);
        assert_eq!(w.first(), Some(&(10, 18)));
        assert_eq!(w.last().map(|x| x.1), Some(132));
        for pair in w.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "{w:?}");
            assert!(pair[1].1 - pair[1].0 >= pair[0].1 - pair[0].0 || pair[1].1 == 132);
        }
        // A short piece is one window; a remainder too short to stand is joined on.
        assert_eq!(plan.windows(0, 6), vec![(0, 6)]);
        assert_eq!(plan.windows(0, 11), vec![(0, 11)]);
        assert!(plan.windows(5, 5).is_empty());
        // By default a piece of a second or less after its lead is one window.
        let plan = WindowPlan::default();
        assert_eq!(plan.windows(15, 63), vec![(15, 63)]);
        assert!(plan.windows(15, 64).len() > 1);
    }

    #[test]
    fn the_phase_is_each_harmonics_cycles_so_far_mod_one() {
        let f0 = vec![240.0f32; 10]; // 0.01 cycles a sample at the fundamental
        let p = phase_before(&f0, 0);
        assert_eq!(p, [0.0; HARMONICS]);
        let p = phase_before(&f0, 5);
        assert!((p[0] - 0.05).abs() < 1e-6, "{p:?}");
        assert!((p[1] - 0.10).abs() < 1e-6, "{p:?}");
        // Unvoiced frames add nothing.
        let mut f0 = vec![0.0f32; 4];
        f0.extend([12_000.0f32; 3]); // half a cycle a sample: 1.5 cycles, 0.5 mod 1
        let p = phase_before(&f0, 7);
        assert!((p[0] - 0.5).abs() < 1e-6, "{p:?}");
        // Past the end is the whole curve.
        assert_eq!(phase_before(&f0, 100), phase_before(&f0, 7));
    }

    #[test]
    fn the_paths_are_the_split_files() {
        let p = HalvesPaths::in_dir(Path::new("/m"));
        assert_eq!(p.encoder, PathBuf::from("/m/kokoro-encoder.onnx"));
        assert_eq!(p.decoder, PathBuf::from("/m/kokoro-decoder.onnx"));
        assert!(!p.exist());
    }
}
