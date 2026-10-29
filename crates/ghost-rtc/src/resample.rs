//! The resamplers either side of Opus, through `rubato`'s FFT resampler: 48 kHz down to
//! the pipeline's 16 kHz on the way in, 24 kHz (Kokoro) up to 48 kHz on the way out. Both
//! ratios are exact, so a fixed-input-size synchronous resampler fits: the stack pushes
//! whatever it has and pulls whole 20 ms frames.

use std::collections::VecDeque;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as _};

use crate::MediaError;
use crate::frame::{IN_RATE, OUT_RATE, WIRE_RATE, frame_samples};

/// A mono s16 resampler with its own input and output queues.
pub struct Resampler {
    inner: Fft<f32>,
    in_rate: u32,
    out_rate: u32,
    pending: Vec<f32>,
    out: VecDeque<i16>,
    scratch_out: Vec<f32>,
}

impl Resampler {
    /// Chunks of 20 ms of input; the first output carries the filter's delay as silence.
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self, MediaError> {
        let chunk = frame_samples(in_rate);
        let inner = Fft::<f32>::new(
            in_rate as usize,
            out_rate as usize,
            chunk,
            1,
            FixedSync::Input,
        )
        .map_err(|e| MediaError::Resample(e.to_string()))?;
        let scratch_out = vec![0.0; inner.output_frames_max()];
        Ok(Resampler {
            inner,
            in_rate,
            out_rate,
            pending: Vec::with_capacity(chunk * 2),
            out: VecDeque::with_capacity(frame_samples(out_rate) * 4),
            scratch_out,
        })
    }

    /// 48 kHz, the wire, down to the pipeline's 16 kHz.
    pub fn downsampler() -> Result<Self, MediaError> {
        Resampler::new(WIRE_RATE, IN_RATE)
    }

    /// Kokoro's 24 kHz up to the wire's 48 kHz.
    pub fn upsampler() -> Result<Self, MediaError> {
        Resampler::new(OUT_RATE, WIRE_RATE)
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// The filter's delay, in output samples.
    pub fn delay_samples(&self) -> usize {
        self.inner.output_delay()
    }

    /// Push any number of samples; whole chunks are resampled at once.
    pub fn push(&mut self, samples: &[i16]) {
        self.pending
            .extend(samples.iter().map(|&s| s as f32 / 32768.0));
        let chunk = self.inner.input_frames_next();
        while self.pending.len() >= chunk {
            let input =
                InterleavedSlice::new(&self.pending[..chunk], 1, chunk).expect("a mono chunk");
            let want = self.inner.output_frames_next();
            let mut output = InterleavedSlice::new_mut(&mut self.scratch_out[..want], 1, want)
                .expect("a mono output chunk");
            let (consumed, produced) = self
                .inner
                .process_into_buffer(&input, &mut output, None)
                .expect("a sized chunk resamples");
            debug_assert_eq!(consumed, chunk);
            self.out.extend(
                self.scratch_out[..produced]
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16),
            );
            self.pending.drain(..consumed);
        }
    }

    /// Output samples ready to pull.
    pub fn available(&self) -> usize {
        self.out.len()
    }

    /// `n` output samples, once that many are ready.
    pub fn pull(&mut self, n: usize) -> Option<Vec<i16>> {
        if self.out.len() < n {
            return None;
        }
        Some(self.out.drain(..n).collect())
    }

    /// One 20 ms output frame, once ready.
    pub fn pull_frame(&mut self) -> Option<Vec<i16>> {
        self.pull(frame_samples(self.out_rate))
    }

    /// Forget what is queued (barge-in, or a fresh peer).
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
        self.out.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::rms;

    fn tone(rate: u32, hz: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (0.5 * (2.0 * std::f32::consts::PI * hz * t).sin() * 32767.0) as i16
            })
            .collect()
    }

    /// The dominant frequency of `pcm` by the zero-crossing count.
    fn frequency(pcm: &[i16], rate: u32) -> f32 {
        let crossings = pcm.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        crossings as f32 / 2.0 * rate as f32 / pcm.len() as f32
    }

    fn run(r: &mut Resampler, pcm: &[i16], frame: usize) -> Vec<i16> {
        let mut out = Vec::new();
        for chunk in pcm.chunks(frame) {
            r.push(chunk);
            while let Some(f) = r.pull_frame() {
                out.extend(f);
            }
        }
        out
    }

    #[test]
    fn down_48_to_16_keeps_the_tone_and_the_count() {
        let mut r = Resampler::downsampler().unwrap();
        let seconds = 2;
        let pcm = tone(48_000, 1000.0, 48_000 * seconds);
        let out = run(&mut r, &pcm, 960);
        let delay = r.delay_samples();
        assert!(delay < 400, "delay {delay} samples at 16 kHz");
        // Whole frames: everything but what the filter still holds (a call yields 319 to
        // 321 samples; the queue evens it out).
        let total = out.len() + r.available() + r.pending.len() / 3;
        assert!(
            (total as i64 - (16_000 * seconds) as i64).abs() <= 16,
            "{total}"
        );
        assert_eq!(out.len() % 320, 0);
        let steady = &out[16_000..];
        assert!((frequency(steady, 16_000) - 1000.0).abs() < 5.0);
        let db = 20.0 * (rms(steady) / rms(&pcm[48_000..])).log10();
        assert!(db.abs() < 0.5, "level changed by {db} dB");
    }

    #[test]
    fn up_24_to_48_keeps_the_tone_and_the_count() {
        let mut r = Resampler::upsampler().unwrap();
        let pcm = tone(24_000, 1000.0, 24_000);
        let out = run(&mut r, &pcm, 480);
        assert_eq!(out.len() % 960, 0);
        assert!(out.len() >= 48_000 - 960 * 2);
        let steady = &out[24_000..];
        assert!((frequency(steady, 48_000) - 1000.0).abs() < 5.0);
        let db = 20.0 * (rms(steady) / rms(&pcm[12_000..])).log10();
        assert!(db.abs() < 0.5, "level changed by {db} dB");
    }

    #[test]
    fn silence_in_silence_out_and_partial_pushes_queue() {
        let mut r = Resampler::downsampler().unwrap();
        r.push(&[0; 500]);
        assert_eq!(r.available(), 0);
        assert!(r.pull_frame().is_none());
        r.push(&[0; 460]);
        // The first chunk comes out short by the filter's delay; the next catches up.
        assert!(
            r.available() >= 200 && r.available() <= 340,
            "{}",
            r.available()
        );
        r.push(&[0; 960]);
        let f = r.pull_frame().unwrap();
        assert_eq!(f.len(), 320);
        assert!(f.iter().all(|&s| s == 0));
        r.reset();
        assert_eq!(r.available(), 0);
    }

    #[test]
    fn a_marker_onset_lands_where_the_delay_says() {
        // 200 ms silence then a tone: the onset in the output sits delay samples later.
        let mut r = Resampler::downsampler().unwrap();
        let mut pcm = vec![0i16; 9600];
        pcm.extend(tone(48_000, 1000.0, 9600));
        let out = run(&mut r, &pcm, 960);
        let onset = out.iter().position(|&s| s.abs() > 3000).unwrap();
        let expected = 3200 + r.delay_samples();
        assert!(
            (onset as i64 - expected as i64).abs() <= 4,
            "onset {onset}, expected {expected}"
        );
    }
}
