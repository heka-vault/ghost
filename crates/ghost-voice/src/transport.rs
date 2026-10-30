//! Where a session's audio comes from and goes to: the desk's sound devices
//! (`ghost_audio`), the phone's WebRTC tracks (the media crate fulfils [`MediaIn`] and
//! [`MediaOut`]), or the scripted devices of the replay harness. The two stages at the
//! ends of the pipeline, [`AudioInput`] and [`AudioOutput`], are the same whichever.
//!
//! The output stage is where a barge-in is felt: every audio frame carries its turn's
//! token, a cancelled one is dropped unplayed, and the write of the one in progress stops
//! within a frame and clears what the device still queued. It is also where Ghost's
//! speaking is known first: it raises the shared flag (`crate::speaking`) as it writes a
//! reply's first sample and lowers it when the last has played or the turn was cut, so
//! the stages that must not mistake Ghost's voice for the operator's learn it then and
//! not when the `BotStarted` frame has worked its way up behind a busy TTS stage. The
//! same first sample settles the turn's answer as heard (`crate::turns::TurnControl`):
//! live, the frame was still on its way 1.3 s after a one-word reply had played, and a
//! leak of that reply reopened the turn as if nothing of the answer had been heard.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_audio::{AudioSettings, Capture, CaptureConfig, MixerHandle, Playback, PlaybackConfig};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::frames::{Audio, Direction, Frame, Out, Stage};
use crate::speaking::Speaking;
use crate::turns::TurnControl;

pub const IN_RATE: u32 = 16_000;
pub const OUT_RATE: u32 = 24_000;
pub const FRAME_SECS: f64 = 0.02;

/// The microphone side: 20 ms of 16 kHz mono s16 at a time.
#[async_trait]
pub trait MediaIn: Send {
    /// The next frame; None when the source closed.
    async fn next_frame(&mut self) -> Option<Audio>;
    fn close(&mut self) {}
}

/// The speaker side: 24 kHz mono s16 in, at the pace the device takes it.
#[async_trait]
pub trait MediaOut: Send {
    /// Queue `pcm`; waits for room, never for playback.
    async fn write(&mut self, pcm: &[i16]) -> Result<(), String>;
    /// Barge-in: drop what is queued.
    async fn clear(&mut self);
    /// Resolves when what was queued has played.
    async fn drain(&mut self);
    /// The thinking sound under speech, on or off.
    fn set_thinking(&self, on: bool);
    /// A message to the far end's data channel (the phone); nothing at the desk.
    async fn send_message(&self, _message: Value) {}
    fn close(&mut self) {}
}

/// One session's transport.
pub struct Media {
    pub input: Box<dyn MediaIn>,
    pub output: Box<dyn MediaOut>,
    /// `desk`, `phone` or `scripted`: a label on the latency records.
    pub origin: &'static str,
}

// --- the stages -----------------------------------------------------------------------

/// The pipeline's source: the microphone's frames go in, and whatever the session
/// injects (a typed turn, a transfer, the end).
pub struct AudioInput {
    media: Option<Box<dyn MediaIn>>,
    token: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl AudioInput {
    pub fn new(media: Box<dyn MediaIn>, token: CancellationToken) -> Self {
        Self {
            media: Some(media),
            token,
            task: None,
        }
    }
}

#[async_trait]
impl Stage for AudioInput {
    fn name(&self) -> &'static str {
        "audio_in"
    }

    async fn started(&mut self, out: &Out) {
        let Some(mut media) = self.media.take() else {
            return;
        };
        let out = out.clone();
        let token = self.token.clone();
        self.task = Some(tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    f = media.next_frame() => f,
                };
                let Some(frame) = frame else { break };
                out.push(Frame::AudioIn(frame)).await;
            }
            media.close();
        }));
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Up {
            return; // nothing is above the microphone
        }
        if matches!(frame, Frame::End | Frame::Cancel)
            && let Some(t) = self.task.take()
        {
            t.abort();
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// The pipeline's sink: speech to the device, the bot-speaking frames back up, the
/// thinking sound's switch, and the barge-in's clear.
pub struct AudioOutput {
    media: Box<dyn MediaOut>,
    speaking: bool,
    /// Shared with the stages that must know at once.
    speaking_now: Speaking,
    /// The turn's answer is heard the moment its first sample is written.
    turns: Option<Arc<TurnControl>>,
}

impl AudioOutput {
    pub fn new(media: Box<dyn MediaOut>) -> Self {
        Self {
            media,
            speaking: false,
            speaking_now: Speaking::new(),
            turns: None,
        }
    }

    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking_now = speaking;
        self
    }

    pub fn with_turns(mut self, turns: Arc<TurnControl>) -> Self {
        self.turns = Some(turns);
        self
    }

    /// Write in frames, stopping the moment the turn is cancelled.
    async fn play(&mut self, audio: &Audio, turn: &CancellationToken) -> bool {
        let step = (f64::from(audio.rate) * FRAME_SECS) as usize;
        for chunk in audio.samples.chunks(step.max(1)) {
            if turn.is_cancelled() {
                self.speaking_now.set(false);
                self.media.clear().await;
                return false;
            }
            let wrote = tokio::select! {
                biased;
                _ = turn.cancelled() => None,
                r = self.media.write(chunk) => Some(r),
            };
            match wrote {
                None => {
                    self.speaking_now.set(false);
                    self.media.clear().await;
                    return false;
                }
                Some(Err(e)) => {
                    warn!("audio out: {e}");
                    return false;
                }
                Some(Ok(())) => self.speaking_now.audio(),
            }
        }
        true
    }

    async fn speech(&mut self, audio: &Audio, turn: &CancellationToken, out: &Out) {
        if turn.is_cancelled() {
            return; // a stale frame: never reaches the speaker
        }
        self.speaking_now.set(true);
        if let Some(turns) = &self.turns {
            turns.heard();
        }
        if !self.speaking {
            self.speaking = true;
            out.push_up(Frame::BotStarted).await;
        }
        self.play(audio, turn).await;
    }

    /// An acknowledgment (`crate::acks`): played whole before anything after it, never over
    /// the reply, and without `BotStarted`/`BotStopped`, which belong to the reply. The
    /// shared flag is on while it plays, so the barge-in rule and the echo filter know.
    async fn acknowledge(&mut self, audio: &Audio, turn: &CancellationToken, out: &Out) {
        if turn.is_cancelled() || self.speaking {
            return;
        }
        self.speaking_now.set(true);
        out.push_up(Frame::AckPlayed).await;
        if self.play(audio, turn).await {
            tokio::select! {
                biased;
                _ = turn.cancelled() => self.media.clear().await,
                _ = self.media.drain() => {}
            }
        }
        self.speaking_now.set(false);
    }

    async fn stopped_speaking(&mut self, turn: Option<CancellationToken>, out: &Out) {
        if !self.speaking {
            return;
        }
        match turn {
            Some(turn) => {
                tokio::select! {
                    biased;
                    _ = turn.cancelled() => self.media.clear().await,
                    _ = self.media.drain() => {}
                }
            }
            None => self.media.drain().await,
        }
        self.speaking_now.set(false);
        self.speaking = false;
        out.push_up(Frame::BotStopped).await;
    }
}

#[async_trait]
impl Stage for AudioOutput {
    fn name(&self) -> &'static str {
        "audio_out"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Up {
            return;
        }
        match &frame {
            Frame::TtsAudio { audio, turn } => {
                self.speech(audio, turn, out).await;
                return;
            }
            Frame::AudioOut { audio, turn } => {
                let was = self.speaking;
                self.speech(audio, turn, out).await;
                if !was {
                    self.stopped_speaking(Some(turn.clone()), out).await;
                }
                return;
            }
            Frame::TtsStopped => {
                self.stopped_speaking(None, out).await;
                return;
            }
            Frame::Ack { audio, turn, .. } => {
                self.acknowledge(audio, turn, out).await;
                return;
            }
            Frame::MixerEnable(on) => {
                self.media.set_thinking(*on);
                return;
            }
            Frame::Interruption => {
                self.speaking_now.set(false);
                self.media.clear().await;
                if self.speaking {
                    self.speaking = false;
                    out.push_up(Frame::BotStopped).await;
                }
                return;
            }
            Frame::End => {
                self.stopped_speaking(None, out).await;
                self.media.set_thinking(false);
            }
            Frame::Cancel => {
                self.speaking_now.set(false);
                self.media.clear().await;
                self.media.set_thinking(false);
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.speaking_now.set(false);
        self.media.set_thinking(false);
        self.media.close();
    }
}

// --- the desk -------------------------------------------------------------------------

/// The workstation's sound devices: `ghost.mic` in, the default sink out, the thinking
/// sound on the playback's mixer, the chime on open, music ducked for the session.
pub struct DeskIn {
    capture: Capture,
}

#[async_trait]
impl MediaIn for DeskIn {
    async fn next_frame(&mut self) -> Option<Audio> {
        let f = self.capture.next_frame().await?;
        Some(Audio::new(f.samples, f.rate))
    }
}

pub struct DeskOut {
    playback: Playback,
    mixer: MixerHandle,
    ducker: Option<ghost_audio::Ducker>,
}

#[async_trait]
impl MediaOut for DeskOut {
    async fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        self.playback.write(pcm).await.map_err(|e| e.to_string())
    }

    async fn clear(&mut self) {
        self.playback.clear().await;
    }

    async fn drain(&mut self) {
        if let Err(e) = self.playback.drain().await {
            warn!("audio out: {e}");
        }
    }

    fn set_thinking(&self, on: bool) {
        self.mixer.set_enabled(on);
    }

    fn close(&mut self) {
        if let Some(d) = &self.ducker
            && let Err(e) = d.restore()
        {
            warn!("duck: {e}");
        }
    }
}

/// Open the desk's devices for a session. `chime` plays the wake chime first and waits for
/// it, so the greeting follows it.
pub async fn desk_media(
    settings: &AudioSettings,
    thinking_sound: bool,
    chime: bool,
) -> Result<Media, ghost_audio::AudioError> {
    let capture = Capture::open(CaptureConfig {
        node: settings
            .input_device
            .clone()
            .or_else(|| Some("ghost.mic".into())),
        ..CaptureConfig::default()
    })?;
    let sound = if thinking_sound {
        match ghost_audio::sounds::thinking_sound(Path::new(&settings.cache_dir), OUT_RATE) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!("thinking sound: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut playback = Playback::open(PlaybackConfig {
        sink: settings.output_device.clone(),
        sound,
        sound_volume: settings.thinking_volume,
        ..PlaybackConfig::default()
    })?;
    let mixer = playback.mixer();
    let ducker = (!settings.duck_apps.is_empty()).then(|| {
        ghost_audio::Ducker::new(
            settings.duck_apps.iter().cloned(),
            settings.duck_level,
            ghost_audio::default_state_file(),
        )
    });
    if let Some(d) = &ducker {
        match d.duck() {
            Ok(ducked) if !ducked.is_empty() => info!("ducked {} stream(s)", ducked.len()),
            Ok(_) => {}
            Err(e) => warn!("duck: {e}"),
        }
    }
    if chime {
        playback.write(&ghost_audio::chime(true)).await?;
        playback.drain().await?;
    }
    Ok(Media {
        input: Box::new(DeskIn { capture }),
        output: Box::new(DeskOut {
            playback,
            mixer,
            ducker,
        }),
        origin: "desk",
    })
}

// --- the scripted devices ----------------------------------------------------------------

/// What the scripted input plays: a queue of utterances, silence in between.
#[derive(Default)]
pub struct MouthState {
    chunks: VecDeque<Option<Vec<i16>>>, // None marks the end of an utterance
    done: VecDeque<tokio::sync::oneshot::Sender<()>>,
    pub playing: bool,
}

#[derive(Clone, Default)]
pub struct Mouth(Arc<Mutex<MouthState>>);

impl Mouth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue `pcm` (16 kHz); the receiver fires when its last frame has been sent.
    pub fn say(&self, pcm: &[i16]) -> tokio::sync::oneshot::Receiver<()> {
        let n = (f64::from(IN_RATE) * FRAME_SECS) as usize;
        let mut s = self.0.lock().unwrap();
        for chunk in pcm.chunks(n) {
            let mut c = chunk.to_vec();
            c.resize(n, 0);
            s.chunks.push_back(Some(c));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        s.chunks.push_back(None);
        s.done.push_back(tx);
        rx
    }

    /// The input stopped (the session ended): nothing queued will be heard.
    pub fn release(&self) {
        let mut s = self.0.lock().unwrap();
        s.chunks.clear();
        while let Some(d) = s.done.pop_front() {
            let _ = d.send(());
        }
        s.playing = false;
    }

    pub fn playing(&self) -> bool {
        self.0.lock().unwrap().playing
    }

    fn next_frame(&self, n: usize) -> Vec<i16> {
        let mut s = self.0.lock().unwrap();
        while let Some(chunk) = s.chunks.pop_front() {
            match chunk {
                Some(c) => {
                    s.playing = true;
                    return c;
                }
                None => {
                    if let Some(d) = s.done.pop_front() {
                        let _ = d.send(());
                    }
                    s.playing = false;
                }
            }
        }
        vec![0; n]
    }
}

/// Sends 20 ms frames at real-time pace, silence unless an utterance is playing.
pub struct ScriptedIn {
    mouth: Mouth,
    next_t: Option<Instant>,
}

#[async_trait]
impl MediaIn for ScriptedIn {
    async fn next_frame(&mut self) -> Option<Audio> {
        let n = (f64::from(IN_RATE) * FRAME_SECS) as usize;
        let frame = FRAME_SECS;
        let next = self.next_t.get_or_insert_with(Instant::now);
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep(*next - now).await;
        } else if now.duration_since(*next) > Duration::from_millis(500) {
            // fell far behind (a stall): don't burst to catch up
            warn!(
                "perf: scripted mic {:.2}s behind",
                now.duration_since(*next).as_secs_f64()
            );
            *next = now;
        }
        *next += Duration::from_secs_f64(frame);
        Some(Audio::new(self.mouth.next_frame(n), IN_RATE))
    }

    fn close(&mut self) {
        self.mouth.release();
    }
}

/// RMS, full scale 1.0: Ghost's voice, not the thinking loop or silence.
pub const LOUD: f64 = 0.05;

#[derive(Default)]
struct EarState {
    until: Option<Instant>,
    first_loud: Vec<Instant>,
    cleared: Vec<Instant>,
    loud: bool,
    seconds_played: f64,
}

/// Accepts audio at the pace a sound card would (a 40 ms device buffer), drops it, and
/// notes when it turns loud: the moment the operator would hear Ghost.
#[derive(Clone, Default)]
pub struct Ear(Arc<Mutex<EarState>>);

impl Ear {
    const LEAD: Duration = Duration::from_millis(40);

    /// How long after `t` Ghost turned loud at the speaker.
    pub fn loud_after(&self, t: Instant) -> Option<Duration> {
        self.0
            .lock()
            .unwrap()
            .first_loud
            .iter()
            .find(|x| **x > t)
            .map(|x| x.duration_since(t))
    }

    pub fn seconds_played(&self) -> f64 {
        self.0.lock().unwrap().seconds_played
    }

    /// When the queue was cleared (a barge-in), each time.
    pub fn cleared(&self) -> Vec<Instant> {
        self.0.lock().unwrap().cleared.clone()
    }
}

pub struct ScriptedOut {
    ear: Ear,
}

#[async_trait]
impl MediaOut for ScriptedOut {
    async fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        let secs = pcm.len() as f64 / f64::from(OUT_RATE);
        let now = Instant::now();
        let wait = {
            let mut s = self.ear.0.lock().unwrap();
            if !pcm.is_empty() {
                let rms = (pcm
                    .iter()
                    .map(|x| {
                        let v = f64::from(*x) / 32768.0;
                        v * v
                    })
                    .sum::<f64>()
                    / pcm.len() as f64)
                    .sqrt();
                if rms >= LOUD && !s.loud {
                    // It plays once what is already queued in the device has played.
                    let at = s.until.map_or(now, |u| u.max(now));
                    s.first_loud.push(at);
                }
                s.loud = rms >= LOUD || (s.loud && rms >= LOUD / 3.0);
            }
            let until = s.until.map_or(now, |u| u.max(now)) + Duration::from_secs_f64(secs);
            s.until = Some(until);
            s.seconds_played += secs;
            until.saturating_duration_since(now + Ear::LEAD)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        Ok(())
    }

    async fn clear(&mut self) {
        let mut s = self.ear.0.lock().unwrap();
        s.until = None;
        s.loud = false;
        s.cleared.push(Instant::now());
    }

    async fn drain(&mut self) {
        let until = self.ear.0.lock().unwrap().until;
        if let Some(u) = until {
            let now = Instant::now();
            if u > now {
                tokio::time::sleep(u - now).await;
            }
        }
    }

    fn set_thinking(&self, _on: bool) {}
}

/// The replay harness's devices: a `Mouth` to speak into the session and an `Ear` that
/// notes when Ghost's voice would be heard.
pub fn scripted_media() -> (Media, Mouth, Ear) {
    let mouth = Mouth::new();
    let ear = Ear::default();
    let media = Media {
        input: Box::new(ScriptedIn {
            mouth: mouth.clone(),
            next_t: None,
        }),
        output: Box::new(ScriptedOut { ear: ear.clone() }),
        origin: "desk",
    };
    (media, mouth, ear)
}

/// A session with no devices at all: nothing comes in, what goes out is dropped at once.
pub struct Silent;

#[async_trait]
impl MediaIn for Silent {
    async fn next_frame(&mut self) -> Option<Audio> {
        std::future::pending().await
    }
}

#[async_trait]
impl MediaOut for Silent {
    async fn write(&mut self, _pcm: &[i16]) -> Result<(), String> {
        Ok(())
    }
    async fn clear(&mut self) {}
    async fn drain(&mut self) {}
    fn set_thinking(&self, _on: bool) {}
}

pub fn silent_media() -> Media {
    Media {
        input: Box::new(Silent),
        output: Box::new(Silent),
        origin: "desk",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::drain;

    #[tokio::test]
    async fn the_mouth_feeds_frames_at_pace_and_silence_between() {
        let (mut media, mouth, _ear) = scripted_media();
        let done = mouth.say(&vec![1000; 640]); // two frames
        let began = Instant::now();
        let a = media.input.next_frame().await.unwrap();
        let b = media.input.next_frame().await.unwrap();
        let c = media.input.next_frame().await.unwrap();
        assert_eq!(a.samples.len(), 320);
        assert!(a.samples.iter().all(|s| *s == 1000) && b.samples.iter().all(|s| *s == 1000));
        assert!(c.samples.iter().all(|s| *s == 0));
        assert!(done.await.is_ok());
        assert!(began.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn a_cancelled_turns_audio_never_reaches_the_speaker_and_a_barge_in_clears_within_a_frame()
     {
        let (media, _mouth, ear) = scripted_media();
        let mut out_stage = AudioOutput::new(media.output);
        let (out, mut rx) = Out::capture();
        let stale = CancellationToken::new();
        stale.cancel();
        out_stage
            .process(
                Frame::TtsAudio {
                    audio: Audio::new(vec![20000; 2400], OUT_RATE),
                    turn: stale,
                },
                Direction::Down,
                &out,
            )
            .await;
        assert!(drain(&mut rx).await.is_empty());
        assert_eq!(ear.seconds_played(), 0.0);

        // a live turn, cancelled mid-write: the write stops within a frame
        let live = CancellationToken::new();
        let cancel = live.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            cancel.cancel();
        });
        let began = Instant::now();
        out_stage
            .process(
                Frame::TtsAudio {
                    audio: Audio::new(vec![20000; OUT_RATE as usize * 2], OUT_RATE), // 2 s
                    turn: live,
                },
                Direction::Down,
                &out,
            )
            .await;
        let took = began.elapsed();
        assert!(took < Duration::from_millis(200), "{took:?}");
        let pushed = drain(&mut rx).await;
        assert!(pushed.iter().any(|f| matches!(f, Frame::BotStarted)));
    }

    #[tokio::test]
    async fn a_replys_first_sample_settles_its_turn_as_heard() {
        // Live, 2026-10-30: "Okay." played whole, its BotStarted was still climbing the
        // pipeline 1.3 s later, and the leak of it reopened the turn it answered.
        let (media, _mouth, _ear) = scripted_media();
        let turns = TurnControl::new(CancellationToken::new());
        let mut out_stage = AudioOutput::new(media.output).with_turns(turns.clone());
        let (out, _rx) = Out::capture();
        turns.answering("all right");
        assert!(
            turns.reopen().is_some(),
            "nothing played yet: the turn is the operator's"
        );
        turns.answering("all right");
        out_stage
            .process(
                Frame::TtsAudio {
                    audio: Audio::new(vec![20000; OUT_RATE as usize / 10], OUT_RATE),
                    turn: CancellationToken::new(),
                },
                Direction::Down,
                &out,
            )
            .await;
        assert!(
            turns.reopen().is_none(),
            "the answer reached the speaker: the turn stands"
        );
    }

    #[tokio::test]
    async fn an_acknowledgment_plays_without_the_replys_frames() {
        let (media, _mouth, ear) = scripted_media();
        let speaking = Speaking::new();
        let mut out_stage = AudioOutput::new(media.output).with_speaking(speaking.clone());
        let (out, mut rx) = Out::capture();
        let turn = CancellationToken::new();
        out_stage
            .process(
                Frame::Ack {
                    text: "Sure.".into(),
                    audio: Audio::new(vec![20000; OUT_RATE as usize / 5], OUT_RATE),
                    turn: turn.clone(),
                },
                Direction::Down,
                &out,
            )
            .await;
        assert!((ear.seconds_played() - 0.2).abs() < 1e-6);
        assert!(!speaking.is_on(), "off once it has played");
        let up = drain(&mut rx).await;
        assert!(up.iter().any(|f| matches!(f, Frame::AckPlayed)));
        assert!(
            !up.iter()
                .any(|f| matches!(f, Frame::BotStarted | Frame::BotStopped)),
            "the reply's frames are the reply's"
        );
        // a cut-off turn's acknowledgment is never heard
        turn.cancel();
        out_stage
            .process(
                Frame::Ack {
                    text: "Okay.".into(),
                    audio: Audio::new(vec![20000; OUT_RATE as usize / 5], OUT_RATE),
                    turn,
                },
                Direction::Down,
                &out,
            )
            .await;
        assert!((ear.seconds_played() - 0.2).abs() < 1e-6);
    }

    #[tokio::test]
    async fn the_speaking_flag_follows_the_speaker_not_the_frames() {
        let (media, _mouth, _ear) = scripted_media();
        let speaking = Speaking::new();
        let mut out_stage = AudioOutput::new(media.output).with_speaking(speaking.clone());
        let (out, _rx) = Out::capture();
        let turn = CancellationToken::new();
        let clause = |secs: f64| Frame::TtsAudio {
            audio: Audio::new(vec![20000; (f64::from(OUT_RATE) * secs) as usize], OUT_RATE),
            turn: turn.clone(),
        };
        assert!(!speaking.is_on());
        out_stage.process(clause(0.1), Direction::Down, &out).await;
        assert!(speaking.is_on(), "on from the first sample written");
        out_stage
            .process(Frame::TtsStopped, Direction::Down, &out)
            .await; // drains what was queued
        assert!(!speaking.is_on(), "off once the last sample played");
        out_stage.process(clause(0.1), Direction::Down, &out).await;
        assert!(speaking.is_on());
        out_stage
            .process(Frame::Interruption, Direction::Down, &out)
            .await;
        assert!(!speaking.is_on(), "off when the operator cut in");
    }
}
