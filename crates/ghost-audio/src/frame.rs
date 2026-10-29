//! Frame sizing and the frame the daemon reads. The pipeline works in 20 ms frames of
//! 16 kHz mono s16 in (what Parakeet, Silero and the wake detector consume) and 24 kHz
//! mono s16 out (Kokoro's rate), as the Python transport did.

use std::time::{Duration, Instant};

/// The microphone's rate: what every model in the pipeline takes.
pub const CAPTURE_RATE: u32 = 16_000;
/// The speaker's rate: Kokoro's output, and the thinking sound's file.
pub const PLAYBACK_RATE: u32 = 24_000;
/// One frame, in milliseconds.
pub const FRAME_MS: u32 = 20;

/// Samples in one 20 ms frame at `rate`: 320 at 16 kHz, 480 at 24 kHz. The Python's
/// `int(sample_rate / 100) * 2`.
pub const fn frame_samples(rate: u32) -> usize {
    (rate / 100 * 2) as usize
}

/// One 20 ms frame of mono s16 from the microphone.
#[derive(Debug, Clone)]
pub struct Frame {
    pub samples: Vec<i16>,
    pub rate: u32,
    /// When the PipeWire process callback delivered the frame's last sample to Ghost.
    pub captured_at: Instant,
}

impl Frame {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / self.rate as f64)
    }

    /// Root mean square of the frame on the s16 scale, for a level meter.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum / self.samples.len() as f64).sqrt() as f32
    }

    /// The largest magnitude in the frame.
    pub fn peak(&self) -> i16 {
        self.samples
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .map_or(0, |m| m.min(i16::MAX as u16) as i16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_ms_at_each_rate() {
        assert_eq!(frame_samples(16_000), 320);
        assert_eq!(frame_samples(24_000), 480);
        assert_eq!(frame_samples(48_000), 960);
        assert_eq!(frame_samples(44_100), 882); // the Python's int(44100 / 100) * 2
    }

    #[test]
    fn frame_duration_and_levels() {
        let f = Frame {
            samples: vec![0, 1000, -1000, 0],
            rate: 16_000,
            captured_at: Instant::now(),
        };
        assert_eq!(f.duration(), Duration::from_micros(250));
        assert_eq!(f.peak(), 1000);
        assert!((f.rms() - 707.1068).abs() < 0.01);
        let silent = Frame {
            samples: vec![],
            rate: 16_000,
            captured_at: Instant::now(),
        };
        assert_eq!(silent.rms(), 0.0);
        assert_eq!(silent.peak(), 0);
        let min = Frame {
            samples: vec![i16::MIN],
            rate: 16_000,
            captured_at: Instant::now(),
        };
        assert_eq!(min.peak(), i16::MAX);
    }
}
