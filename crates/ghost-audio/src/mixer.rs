//! The thinking sound under speech: Pipecat's `SoundfileMixer` arithmetic, which the
//! Python pipeline used with `volume=GHOST_THINKING_VOLUME` (0.12) and `loop=True`.
//! `mixed = clip(audio + sound * volume, -32768, 32767)` cast to s16 (truncation toward
//! zero, as numpy's `astype`), the sound restarting from its beginning when the chunk
//! would run past its end.
//!
//! Runs inside the playback callback: it never allocates, and the sound is an `Arc` set
//! when the stream opens.

use std::sync::Arc;

/// One sample of the mix, as the Python computed it in float64.
#[inline]
pub fn mix_sample(audio: i16, sound: i16, volume: f64) -> i16 {
    (audio as f64 + sound as f64 * volume).clamp(-32768.0, 32767.0) as i16
}

/// The sound and where in it the next chunk starts.
#[derive(Debug)]
pub struct Mixer {
    sound: Arc<Vec<i16>>,
    pos: usize,
}

impl Mixer {
    pub fn new(sound: Arc<Vec<i16>>) -> Self {
        Self { sound, pos: 0 }
    }

    /// Back to the start of the sound.
    pub fn reset(&mut self) {
        self.pos = 0;
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn sound_len(&self) -> usize {
        self.sound.len()
    }

    /// Mix one chunk of the sound into `audio` in place and advance.
    pub fn mix_into(&mut self, audio: &mut [i16], volume: f64) {
        let len = self.sound.len();
        if len == 0 || audio.is_empty() {
            return;
        }
        if self.pos + audio.len() > len {
            self.pos = 0; // the Python goes back to the beginning rather than wrap mid-chunk
        }
        for a in audio.iter_mut() {
            let s = self.sound[self.pos];
            *a = mix_sample(*a, s, volume);
            self.pos += 1;
            if self.pos >= len {
                self.pos = 0; // only reached when the sound is shorter than one chunk
            }
        }
    }

    /// The same over little-endian s16 bytes (PipeWire's buffer), in place.
    pub fn mix_into_le_bytes(&mut self, bytes: &mut [u8], volume: f64) {
        let len = self.sound.len();
        let n = bytes.len() / 2;
        if len == 0 || n == 0 {
            return;
        }
        if self.pos + n > len {
            self.pos = 0;
        }
        let (pairs, _) = bytes.as_chunks_mut::<2>();
        for dst in pairs.iter_mut() {
            let a = i16::from_le_bytes(*dst);
            let s = self.sound[self.pos];
            *dst = mix_sample(a, s, volume).to_le_bytes();
            self.pos += 1;
            if self.pos >= len {
                self.pos = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_arithmetic_matches_pipecat() {
        assert_eq!(mix_sample(0, 1000, 0.12), 120);
        assert_eq!(mix_sample(0, -1000, 0.12), -120);
        assert_eq!(mix_sample(100, 1000, 0.5), 600);
        // numpy's astype truncates toward zero: 0 + 999 * 0.12 = 119.88 -> 119
        assert_eq!(mix_sample(0, 999, 0.12), 119);
        assert_eq!(mix_sample(0, -999, 0.12), -119);
        // volume 0 leaves the speech alone
        assert_eq!(mix_sample(-5, 32767, 0.0), -5);
    }

    #[test]
    fn clips_at_the_s16_bounds() {
        assert_eq!(mix_sample(32000, 32767, 1.0), 32767);
        assert_eq!(mix_sample(-32000, -32767, 1.0), -32768);
        assert_eq!(mix_sample(i16::MAX, 1, 1.0), i16::MAX);
        assert_eq!(mix_sample(i16::MIN, -1, 1.0), i16::MIN);
    }

    #[test]
    fn mixes_a_chunk_and_advances() {
        let sound = Arc::new(vec![100, 200, 300, 400, 500, 600]);
        let mut m = Mixer::new(sound);
        let mut audio = [10, 10, 10, 10];
        m.mix_into(&mut audio, 1.0);
        assert_eq!(audio, [110, 210, 310, 410]);
        assert_eq!(m.position(), 4);
        // 4 + 4 > 6: the Python restarts the sound rather than wrap mid-chunk
        let mut audio = [0, 0, 0, 0];
        m.mix_into(&mut audio, 0.5);
        assert_eq!(audio, [50, 100, 150, 200]);
        assert_eq!(m.position(), 4);
        m.reset();
        assert_eq!(m.position(), 0);
    }

    #[test]
    fn a_sound_shorter_than_the_chunk_wraps_inside_it() {
        let mut m = Mixer::new(Arc::new(vec![1, 2]));
        let mut audio = [0; 5];
        m.mix_into(&mut audio, 1.0);
        assert_eq!(audio, [1, 2, 1, 2, 1]);
        assert_eq!(m.position(), 1);
    }

    #[test]
    fn empty_sound_or_chunk_is_a_no_op() {
        let mut m = Mixer::new(Arc::new(vec![]));
        let mut audio = [7, 7];
        m.mix_into(&mut audio, 1.0);
        assert_eq!(audio, [7, 7]);
        let mut m = Mixer::new(Arc::new(vec![1]));
        m.mix_into(&mut [], 1.0);
        assert_eq!(m.position(), 0);
    }

    #[test]
    fn bytes_path_equals_sample_path() {
        let sound: Arc<Vec<i16>> =
            Arc::new((0..480).map(|i| (i * 37 % 2000 - 1000) as i16).collect());
        let speech: Vec<i16> = (0..480).map(|i| (i * 91 % 30000 - 15000) as i16).collect();
        let mut a = Mixer::new(sound.clone());
        let mut b = Mixer::new(sound);
        let mut as_samples = speech.clone();
        a.mix_into(&mut as_samples, 0.12);
        let mut as_bytes: Vec<u8> = speech.iter().flat_map(|s| s.to_le_bytes()).collect();
        b.mix_into_le_bytes(&mut as_bytes, 0.12);
        let back: Vec<i16> = as_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        assert_eq!(as_samples, back);
        assert_eq!(a.position(), b.position());
        // an odd trailing byte is left alone
        let mut odd = vec![1u8, 0, 9];
        b.mix_into_le_bytes(&mut odd, 1.0);
        assert_eq!(odd[2], 9);
    }
}
