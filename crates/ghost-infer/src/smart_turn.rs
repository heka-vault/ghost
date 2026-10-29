//! Smart Turn v3.2 as Pipecat runs it (`pipecat.audio.turn.smart_turn.local_smart_turn_v3`):
//! the last 8 s of the operator's turn (zero-padded at the front), Whisper's log-mel
//! features as `_whisper_features.compute_whisper_log_mel_features` vendors them from
//! `transformers` (zero-mean unit-variance waveform, a 400-point FFT every 160 samples
//! over a periodic Hann window, the Slaney filterbank, `log10`, clamped 8 dB under the
//! peak, `(x + 4) / 4`), 800 frames of 80 bins, and the model's probability that the turn
//! is complete, which Pipecat compares with 0.5. Two intra-op threads in Ghost
//! (`smart_turn_threads`), one inter-op, sequential execution.
//!
//! The waveform normalization is NumPy's to the bit: `x.mean()` and `x.var()` on a
//! contiguous float32 array reduce with NumPy's pairwise sum (`pairwise_sum`), and the
//! int8 model moves its probability by 1e-2 on a feature difference of 6e-6, so the sum
//! is ported rather than approximated.

use std::path::Path;

use ort::{session::Session, value::Tensor};

use crate::{
    Result,
    mel::{Spectrum, hann_periodic, slaney_filterbank},
    session::{self, SessionConfig},
};

const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const SAMPLING_RATE: u32 = 16_000;
const MEL_FLOOR: f64 = 1e-10;
const NORM_VARIANCE_EPS: f32 = 1e-7;
/// 8 s at 16 kHz.
pub const N_SAMPLES: usize = 128_000;
/// Frames the model takes.
pub const N_FRAMES: usize = 800;

/// What the model said about a turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnVerdict {
    /// The model's probability that the turn is complete (its `logits` output is already
    /// a sigmoid).
    pub probability: f32,
    /// `probability > 0.5`.
    pub complete: bool,
}

/// The loaded model with its feature extractor.
pub struct SmartTurn {
    session: Session,
    spectrum: Spectrum,
    window: Vec<f64>,
    filters: Vec<f64>,
    power: Vec<f64>,
}

impl SmartTurn {
    /// Load with `threads` intra-op threads (Ghost: 2).
    pub fn load(path: &Path, threads: usize) -> Result<Self> {
        let session = session::open(path, &SessionConfig::threads(threads))?;
        let bins = N_FFT / 2 + 1;
        Ok(Self {
            session,
            spectrum: Spectrum::new(N_FFT),
            window: hann_periodic(N_FFT),
            filters: slaney_filterbank(
                bins,
                N_MELS,
                0.0,
                f64::from(SAMPLING_RATE) / 2.0,
                SAMPLING_RATE,
            ),
            power: vec![0.0; bins],
        })
    }

    /// The last 8 s of `audio`, zero-padded at the front when shorter
    /// (`truncate_audio_to_last_n_seconds`).
    pub fn last_eight_seconds(audio: &[f32]) -> Vec<f32> {
        if audio.len() >= N_SAMPLES {
            audio[audio.len() - N_SAMPLES..].to_vec()
        } else {
            let mut out = vec![0.0; N_SAMPLES - audio.len()];
            out.extend_from_slice(audio);
            out
        }
    }

    /// Log-mel features for `audio` (float32 16 kHz, any length), `[80, 800]` row-major.
    pub fn features(&mut self, audio: &[f32]) -> Vec<f32> {
        let mut x = Self::last_eight_seconds(audio);
        // Zero-mean unit-variance in float32, before the spectrogram, as NumPy computes
        // `(x - x.mean()) / np.sqrt(x.var() + eps)`.
        let n = x.len() as f32;
        let mean = pairwise_sum(&x) / n;
        for v in x.iter_mut() {
            *v -= mean;
        }
        let squares: Vec<f32> = x.iter().map(|&d| d * d).collect();
        let var = pairwise_sum(&squares) / n;
        let denom = (var + NORM_VARIANCE_EPS).sqrt();
        for v in x.iter_mut() {
            *v /= denom;
        }
        // Reflect-pad by n_fft / 2, then a frame every hop; the last frame is dropped.
        let pad = N_FFT / 2;
        let n = x.len();
        let mut padded = Vec::with_capacity(n + 2 * pad);
        for i in (1..=pad).rev() {
            padded.push(f64::from(x[i]));
        }
        padded.extend(x.iter().map(|&v| f64::from(v)));
        for i in 1..=pad {
            padded.push(f64::from(x[n - 1 - i]));
        }
        let frames = (padded.len() - N_FFT) / HOP + 1;
        let bins = self.power.len();
        let mut frame = vec![0.0f64; N_FFT];
        let mut mel = vec![0.0f64; N_MELS * N_FRAMES];
        let mut peak = f64::NEG_INFINITY;
        for t in 0..frames.min(N_FRAMES) {
            let start = t * HOP;
            for (i, f) in frame.iter_mut().enumerate() {
                *f = padded[start + i] * self.window[i];
            }
            self.spectrum.power(&frame, &mut self.power);
            for m in 0..N_MELS {
                let mut acc = 0.0;
                for b in 0..bins {
                    acc += self.filters[b * N_MELS + m] * self.power[b];
                }
                let v = acc.max(MEL_FLOOR).log10();
                peak = peak.max(v);
                mel[m * N_FRAMES + t] = v;
            }
        }
        let floor = peak - 8.0;
        mel.iter()
            .map(|&v| ((v.max(floor) + 4.0) / 4.0) as f32)
            .collect()
    }

    /// The verdict on `audio` (`_predict_endpoint`).
    pub fn predict(&mut self, audio: &[f32]) -> Result<TurnVerdict> {
        let features = self.features(audio);
        self.predict_features(&features)
    }

    /// The verdict on features already extracted (`[80, 800]`).
    pub fn predict_features(&mut self, features: &[f32]) -> Result<TurnVerdict> {
        let outputs = self.session.run(ort::inputs![
            "input_features" => Tensor::from_array(([1usize, N_MELS, N_FRAMES], features.to_vec()))?,
        ])?;
        let (_, out) = session::first_output_f32(&outputs)?;
        let probability = out.first().copied().unwrap_or(0.0);
        Ok(TurnVerdict {
            probability,
            complete: probability > 0.5,
        })
    }
}

/// NumPy's `pairwise_sum` for a contiguous float32 reduction (`numpy/_core/src/umath/
/// loops_utils.h.src`): under 8 elements a running sum, up to 128 eight interleaved
/// partial sums folded as `((r0 + r1) + (r2 + r3)) + ((r4 + r5) + (r6 + r7))` with the
/// remainder added one by one, and above that the two halves (the first cut to a multiple
/// of 8) summed recursively.
pub fn pairwise_sum(a: &[f32]) -> f32 {
    const BLOCK: usize = 128;
    let n = a.len();
    if n < 8 {
        let mut res = 0.0f32;
        for &v in a {
            res += v;
        }
        res
    } else if n <= BLOCK {
        let mut r = [0.0f32; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - (n % 8) {
            for (j, rj) in r.iter_mut().enumerate() {
                *rj += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sums NumPy 2.5 gives for `((i * 7919) % 1000) / 997 - 0.5` as float32, which
    /// a running float32 sum does not reach from 128 elements on.
    #[test]
    fn pairwise_sum_is_numpys() {
        let input = |n: usize| -> Vec<f32> {
            (0..n)
                .map(|i| (((i * 7919) % 1000) as f64 / 997.0 - 0.5) as f32)
                .collect()
        };
        for (n, bits) in [
            (7usize, 0x3f4f_db06u32),
            (20, 0x3f24_5540),
            (128, 0xbe34_c430),
            (129, 0xbd2e_9adc),
            (1000, 0x3f80_629d),
            (128_000, 0x4300_629d),
        ] {
            let got = pairwise_sum(&input(n));
            assert_eq!(
                got.to_bits(),
                bits,
                "n={n}: {got} vs {}",
                f32::from_bits(bits)
            );
        }
        assert_eq!(pairwise_sum(&vec![0.1f32; 1000]).to_bits(), 0x42c8_0002);
        assert_eq!(pairwise_sum(&[]), 0.0);
        let running: f32 = input(1000).iter().sum();
        assert_ne!(running.to_bits(), 0x3f80_629d);
    }

    #[test]
    fn the_last_eight_seconds_are_kept_and_padded_in_front() {
        let short = vec![1.0f32; 10];
        let out = SmartTurn::last_eight_seconds(&short);
        assert_eq!(out.len(), N_SAMPLES);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[N_SAMPLES - 1], 1.0);
        let long: Vec<f32> = (0..N_SAMPLES + 5).map(|i| i as f32).collect();
        let out = SmartTurn::last_eight_seconds(&long);
        assert_eq!(out[0], 5.0);
    }
}
