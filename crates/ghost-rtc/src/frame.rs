//! The frames either side of the wire. The pipeline works in 20 ms frames of 16 kHz mono
//! s16 in (what Parakeet, Silero and the wake detector consume) and 24 kHz mono s16 out
//! (Kokoro's rate); Opus on RTP runs at 48 kHz. The same numbers as `ghost-audio`'s
//! `frame.rs` and the Python transport.

use std::time::{Duration, Instant};

/// What the pipeline hears: the inbound frames' rate.
pub const IN_RATE: u32 = 16_000;
/// What the pipeline says: the outbound frames' rate (Kokoro's).
pub const OUT_RATE: u32 = 24_000;
/// Opus's RTP clock; what is decoded from and encoded to.
pub const WIRE_RATE: u32 = 48_000;
/// One frame, in milliseconds: one Opus packet.
pub const FRAME_MS: u32 = 20;

/// Samples in one 20 ms frame at `rate`: 320 at 16 kHz, 480 at 24 kHz, 960 at 48 kHz.
pub const fn frame_samples(rate: u32) -> usize {
    (rate / 100 * 2) as usize
}

/// One frame of mono s16 PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    pub samples: Vec<i16>,
    pub rate: u32,
    /// Inbound: when the frame was decoded and resampled, ready for the pipeline.
    pub at: Instant,
}

impl PcmFrame {
    pub fn new(samples: Vec<i16>, rate: u32) -> Self {
        PcmFrame {
            samples,
            rate,
            at: Instant::now(),
        }
    }

    /// A 20 ms frame of silence at `rate`.
    pub fn silence(rate: u32) -> Self {
        PcmFrame::new(vec![0; frame_samples(rate)], rate)
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / self.rate as f64)
    }

    /// Root mean square on the s16 scale.
    pub fn rms(&self) -> f32 {
        rms(&self.samples)
    }

    /// The largest magnitude in the frame.
    pub fn peak(&self) -> i16 {
        peak(&self.samples)
    }
}

/// Root mean square of `samples` on the s16 scale.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// The largest magnitude in `samples`.
pub fn peak(samples: &[i16]) -> i16 {
    samples
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .map_or(0, |m| m.min(i16::MAX as u16) as i16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_ms_at_each_rate() {
        assert_eq!(frame_samples(IN_RATE), 320);
        assert_eq!(frame_samples(OUT_RATE), 480);
        assert_eq!(frame_samples(WIRE_RATE), 960);
    }

    #[test]
    fn frame_duration_and_levels() {
        let f = PcmFrame::new(vec![0, 1000, -1000, 0], IN_RATE);
        assert_eq!(f.duration(), Duration::from_micros(250));
        assert_eq!(f.peak(), 1000);
        assert!((f.rms() - 707.1068).abs() < 0.01);
        let s = PcmFrame::silence(OUT_RATE);
        assert_eq!(s.samples.len(), 480);
        assert_eq!(s.rms(), 0.0);
        assert_eq!(peak(&[i16::MIN]), i16::MAX);
    }
}
