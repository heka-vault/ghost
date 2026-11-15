//! The room's devices: the scripted microphone with the room and Ghost's own voice mixed
//! in, and the scripted speaker that sends what it plays back into the room.
//!
//! The microphone hears, every 20 ms, the person (the `Mouth`, paced as a microphone is),
//! the room's bed from where the scene starts it, and, through speakers, Ghost's own voice
//! as it plays. The echo path stands for a working canceller: Ghost's voice comes back
//! duller, a few milliseconds late and `residual_db` down, except in the first half second
//! after it starts from silence, when the canceller is still converging and lets more by
//! (`onset_db`; measured on the desk's array, 2026-10-30). With headphones nothing comes
//! back.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::scene::{RATE, lowpass};
use crate::frames::Audio;
use crate::perf::audio::resample;
use crate::transport::{IN_RATE, MediaIn, MediaOut, OUT_RATE};

/// How Ghost's voice reaches the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Path {
    /// Headphones: it doesn't.
    Headphones,
    /// Speakers, through a canceller that leaks at the onset.
    Speakers,
}

impl Path {
    pub fn name(self) -> &'static str {
        match self {
            Self::Headphones => "headphones",
            Self::Speakers => "speakers",
        }
    }

    pub fn named(s: &str) -> Option<Self> {
        [Self::Headphones, Self::Speakers]
            .into_iter()
            .find(|p| p.name() == s)
    }
}

/// What the canceller leaves of Ghost's voice, below what was played.
pub const RESIDUAL_DB: f32 = -28.0;
/// ... and in its first half second after silence.
pub const ONSET_DB: f32 = -14.0;
const ONSET_SECS: f32 = 0.5;
/// Ghost's voice is this far from the microphone, in samples (about 20 ms).
const ECHO_DELAY: usize = 320;

struct State {
    bed: Arc<Vec<f32>>,
    at: usize,
    gain: f32,
    path: Path,
    /// Ghost's voice as the microphone will hear it, 16 kHz, in order.
    echo: VecDeque<f32>,
    /// Samples of Ghost's voice since it last started from silence.
    since_onset: usize,
    quiet_writes: usize,
    /// Every frame the microphone sent, when kept (`--keep-audio`).
    tape: Option<Vec<i16>>,
}

/// The room shared by the two devices.
#[derive(Clone)]
pub struct Room(Arc<Mutex<State>>);

impl Room {
    /// `bed` at its own level times `gain`, from sample `at`.
    pub fn new(bed: Arc<Vec<f32>>, at: usize, gain: f32, path: Path, keep: bool) -> Self {
        Self(Arc::new(Mutex::new(State {
            bed,
            at,
            gain,
            path,
            echo: std::iter::repeat_n(0.0, ECHO_DELAY).collect(),
            since_onset: 0,
            quiet_writes: usize::MAX,
            tape: keep.then(Vec::new),
        })))
    }

    pub fn tape(&self) -> Option<Vec<i16>> {
        self.0.lock().unwrap().tape.clone()
    }

    fn mix(&self, person: &[i16]) -> Vec<i16> {
        let mut s = self.0.lock().unwrap();
        let mut out = Vec::with_capacity(person.len());
        for p in person {
            let bed = if s.bed.is_empty() {
                0.0
            } else {
                let i = s.at % s.bed.len();
                s.bed[i] * s.gain
            };
            s.at += 1;
            let echo = s.echo.pop_front().unwrap_or(0.0);
            let v = f32::from(*p) / 32768.0 + bed + echo;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        if let Some(t) = s.tape.as_mut() {
            t.extend_from_slice(&out);
        }
        out
    }

    fn played(&self, pcm: &[i16]) {
        let mut s = self.0.lock().unwrap();
        if s.path == Path::Headphones || pcm.is_empty() {
            return;
        }
        let voice = ghost_infer::audio::to_f32(pcm);
        let rms = (voice.iter().map(|x| x * x).sum::<f32>() / voice.len() as f32).sqrt();
        if rms < 0.003 {
            s.quiet_writes = s.quiet_writes.saturating_add(1);
        } else {
            if s.quiet_writes > 10 {
                s.since_onset = 0; // started again after a silence: the canceller reconverges
            }
            s.quiet_writes = 0;
        }
        let at16 = lowpass(&resample(&voice, OUT_RATE, IN_RATE), 4_000.0);
        let onset = (ONSET_SECS * RATE as f32) as usize;
        for x in at16 {
            let db = if s.since_onset < onset {
                ONSET_DB
            } else {
                RESIDUAL_DB
            };
            s.since_onset += 1;
            s.echo.push_back(x * 10f32.powf(db / 20.0));
        }
    }

    fn cleared(&self) {
        let mut s = self.0.lock().unwrap();
        s.echo.clear();
        s.echo.extend(std::iter::repeat_n(0.0, ECHO_DELAY));
    }
}

/// The microphone: the scripted one, with the room mixed in.
pub struct RoomIn {
    pub inner: Box<dyn MediaIn>,
    pub room: Room,
}

#[async_trait]
impl MediaIn for RoomIn {
    async fn next_frame(&mut self) -> Option<Audio> {
        let frame = self.inner.next_frame().await?;
        Some(Audio::new(self.room.mix(&frame.samples), frame.rate))
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

/// The speaker: the scripted one, and what it plays goes back into the room.
pub struct RoomOut {
    pub inner: Box<dyn MediaOut>,
    pub room: Room,
}

#[async_trait]
impl MediaOut for RoomOut {
    async fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.room.played(pcm);
        self.inner.write(pcm).await
    }

    async fn clear(&mut self) {
        self.room.cleared();
        self.inner.clear().await;
    }

    async fn drain(&mut self) {
        self.inner.drain().await;
    }

    fn set_thinking(&self, on: bool) {
        self.inner.set_thinking(on);
    }

    fn lead(&self) -> Duration {
        self.inner.lead()
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_microphone_hears_the_person_the_room_and_the_echo() {
        let bed = Arc::new(vec![0.01f32; 1000]);
        let room = Room::new(bed, 0, 1.0, Path::Speakers, true);
        room.played(&vec![16384i16; 2400]); // 0.1 s of Ghost
        let first = room.mix(&[0i16; 320]);
        // the echo is 20 ms late: the first frame is the bed alone
        assert!(
            first.iter().all(|s| (*s - 327).abs() <= 1),
            "{:?}",
            &first[..4]
        );
        let second = room.mix(&[0i16; 320]);
        assert!(
            second[200] > 400,
            "the onset leak comes through: {}",
            second[200]
        );
        assert_eq!(room.tape().unwrap().len(), 640);
    }

    #[test]
    fn headphones_send_nothing_back() {
        let room = Room::new(Arc::new(Vec::new()), 0, 1.0, Path::Headphones, false);
        room.played(&vec![16384i16; 2400]);
        room.mix(&[0i16; 320]);
        assert!(room.mix(&[0i16; 320]).iter().all(|s| *s == 0));
    }
}
