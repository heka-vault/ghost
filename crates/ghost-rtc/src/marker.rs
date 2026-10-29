//! The measurement tone (`README.md`, `scripts/rtc/`): a 1 kHz burst once a second, silence
//! between, so a one-way latency can be read from when each burst's first sample was
//! handed to the sender and when it came out of the receiver. The far end
//! (`scripts/rtc/phone.py`) generates and detects the same pattern. Nothing here is on the
//! daemon's path; it is here so the probe, the tests and the far end agree on one shape.

/// The pattern: a burst every `PERIOD_MS`, `BURST_MS` long, at `HZ`, at `AMPLITUDE` of
/// full scale, starting at sample 0 of a period.
pub const PERIOD_MS: u32 = 1000;
pub const BURST_MS: u32 = 200;
pub const HZ: f32 = 1000.0;
pub const AMPLITUDE: f32 = 0.5;
/// A sample above this is tone (the burst peaks at 16383); Opus pre-echo stays well under.
pub const THRESHOLD: i16 = 4000;
/// How long the detector wants quiet before it calls a loud sample an onset.
pub const QUIET_MS: u32 = 300;

/// Generates the pattern at `rate`, frame by frame.
pub struct MarkerTone {
    rate: u32,
    period: usize,
    burst: usize,
    pos: usize,
    /// Bursts generated so far.
    pub count: u64,
}

impl MarkerTone {
    pub fn new(rate: u32) -> Self {
        MarkerTone {
            rate,
            period: (rate * PERIOD_MS / 1000) as usize,
            burst: (rate * BURST_MS / 1000) as usize,
            pos: 0,
            count: 0,
        }
    }

    /// The next `n` samples, and the index within them of a burst's first sample if one
    /// starts in this frame (the marker the receiver times).
    pub fn frame(&mut self, n: usize) -> (Vec<i16>, Option<usize>) {
        let mut out = Vec::with_capacity(n);
        let mut onset = None;
        for i in 0..n {
            if self.pos == 0 {
                onset = Some(i);
                self.count += 1;
            }
            let sample = if self.pos < self.burst {
                let t = self.pos as f32 / self.rate as f32;
                (AMPLITUDE * (2.0 * std::f32::consts::PI * HZ * t).sin() * 32767.0) as i16
            } else {
                0
            };
            out.push(sample);
            self.pos = (self.pos + 1) % self.period;
        }
        (out, onset)
    }
}

/// Finds burst onsets in received audio: the first sample over [`THRESHOLD`] after at
/// least [`QUIET_MS`] of quiet.
pub struct OnsetDetector {
    quiet_needed: usize,
    quiet: usize,
    /// Onsets found so far.
    pub count: u64,
}

impl OnsetDetector {
    pub fn new(rate: u32) -> Self {
        let quiet_needed = (rate * QUIET_MS / 1000) as usize;
        OnsetDetector {
            quiet_needed,
            quiet: quiet_needed,
            count: 0,
        }
    }

    /// Feed a frame; the index of an onset inside it, if one.
    pub fn feed(&mut self, samples: &[i16]) -> Option<usize> {
        let mut onset = None;
        for (i, &s) in samples.iter().enumerate() {
            if s.unsigned_abs() > THRESHOLD as u16 {
                if self.quiet >= self.quiet_needed && onset.is_none() {
                    onset = Some(i);
                    self.count += 1;
                }
                self.quiet = 0;
            } else {
                self.quiet = self.quiet.saturating_add(1);
            }
        }
        onset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tone_marks_once_a_second_at_a_frame_boundary() {
        let mut tone = MarkerTone::new(48_000);
        let mut onsets = Vec::new();
        for frame in 0..150 {
            let (pcm, onset) = tone.frame(960);
            assert_eq!(pcm.len(), 960);
            if let Some(i) = onset {
                onsets.push((frame, i));
            }
        }
        assert_eq!(onsets, vec![(0, 0), (50, 0), (100, 0)]);
        assert_eq!(tone.count, 3);
        let (burst, _) = MarkerTone::new(16_000).frame(320);
        assert!(burst.iter().any(|&s| s > 15_000));
    }

    #[test]
    fn the_detector_finds_the_same_onsets_through_frames() {
        let mut tone = MarkerTone::new(16_000);
        let mut det = OnsetDetector::new(16_000);
        let mut found = Vec::new();
        for frame in 0..150 {
            let (pcm, _) = tone.frame(320);
            if let Some(i) = det.feed(&pcm) {
                found.push((frame, i));
            }
        }
        // sin(0) is 0: the first loud sample is one in at 16 kHz (two at 48 kHz).
        assert_eq!(found, vec![(0, 1), (50, 1), (100, 1)]);
        // Mid-frame onsets are placed to the sample, and a burst does not re-trigger.
        let mut det = OnsetDetector::new(16_000);
        let mut pcm = vec![0i16; 100];
        pcm.extend(std::iter::repeat_n(10_000i16, 220));
        assert_eq!(det.feed(&pcm), Some(100));
        assert_eq!(det.feed(&[10_000; 320]), None);
        assert_eq!(det.feed(&[0; 320]), None);
        // Quiet shorter than QUIET_MS is not a gap.
        let mut short = vec![0i16; 1000];
        short.extend(std::iter::repeat_n(10_000i16, 100));
        assert_eq!(det.feed(&short), None);
        let mut long = vec![0i16; 4800];
        long.extend(std::iter::repeat_n(10_000i16, 100));
        assert_eq!(det.feed(&long), Some(4800));
    }
}
