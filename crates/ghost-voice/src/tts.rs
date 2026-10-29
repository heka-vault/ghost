//! Text to speech with Kokoro, a clause at a time (`ghost_voice.kokoro` and
//! `ghost_voice.clauses`).
//!
//! Each session's TTS is a fresh stage over the one loaded model: the model is shared
//! behind a mutex, the stage's settings (the agent's voice and speed, ADR 0014), its
//! clause aggregator and its place in the reply are its own, so a session after a
//! cancelled one still speaks and two sessions never mix their text.
//!
//! Kokoro renders a whole chunk before any of it plays, about 0.23 s per second of speech,
//! so the reply is handed over at the end of a sentence and, with `clause_min_words`, at
//! a clause boundary (a comma, colon, semicolon or dash) once that many words are pending
//! (`ghost_infer::clauses`). What reaches the model is speech, not Markdown (`spoken`).
//! Every audio frame carries the turn's token: cancelled by a barge-in, it never reaches
//! the speaker.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use ghost_infer::Kokoro;
use ghost_infer::clauses::ClauseAggregator;
use tracing::warn;

use crate::frames::{Audio, Direction, Frame, Metric, Out, Stage};
use crate::spoken::spoken;
use crate::turns::TurnControl;

pub const SAMPLE_RATE: u32 = 24_000;
pub const LANG: &str = "en-us";

/// The model behind the stage: Kokoro, or a fake in tests.
pub trait Synth: Send {
    fn voices(&self) -> Vec<String>;
    /// 24 kHz mono s16 for `text` in `voice` at `speed`.
    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> ghost_infer::Result<Vec<i16>>;
}

impl Synth for Kokoro {
    fn voices(&self) -> Vec<String> {
        Kokoro::voices(self).into_iter().map(String::from).collect()
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> ghost_infer::Result<Vec<i16>> {
        let audio = Kokoro::synthesize(self, text, voice, speed, LANG)?;
        Ok(ghost_infer::audio::to_i16(&audio))
    }
}

pub type SharedSynth = Arc<Mutex<dyn Synth>>;

/// No voice at all: every voice is known, nothing is rendered. The stand-in when a
/// session's voice isn't in the loaded model's file, so the session still runs.
#[derive(Debug, Default)]
pub struct NoSynth;

impl Synth for NoSynth {
    fn voices(&self) -> Vec<String> {
        vec![TtsSettings::default().voice]
    }

    fn synthesize(
        &mut self,
        _text: &str,
        _voice: &str,
        _speed: f32,
    ) -> ghost_infer::Result<Vec<i16>> {
        Ok(Vec::new())
    }
}

/// A session's voice: the Kokoro voice id and the speed.
#[derive(Debug, Clone, PartialEq)]
pub struct TtsSettings {
    pub voice: String,
    pub speed: f64,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            voice: "af_heart".into(),
            speed: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TtsError {
    #[error("Voice {0} not found in available voices")]
    UnknownVoice(String),
}

/// Kokoro as one session's TTS stage.
pub struct KokoroTts {
    synth: SharedSynth,
    settings: TtsSettings,
    clause_min_words: Option<usize>,
    aggregator: ClauseAggregator,
    turns: Option<Arc<TurnControl>>,
    speaking: bool,
    first_audio: bool,
}

impl KokoroTts {
    pub fn new(
        synth: SharedSynth,
        settings: TtsSettings,
        clause_min_words: Option<usize>,
    ) -> Result<Self, TtsError> {
        {
            let s = synth.lock().unwrap_or_else(|e| e.into_inner());
            if !s.voices().contains(&settings.voice) {
                return Err(TtsError::UnknownVoice(settings.voice.clone()));
            }
        }
        Ok(Self {
            synth,
            settings,
            clause_min_words,
            aggregator: ClauseAggregator::new(clause_min_words),
            turns: None,
            speaking: false,
            first_audio: true,
        })
    }

    pub fn with_turns(mut self, turns: Arc<TurnControl>) -> Self {
        self.turns = Some(turns);
        self
    }

    /// A fresh stage over the same loaded model, with the template's settings; a clause
    /// setting of its own when given.
    pub fn fork(&self) -> Self {
        self.fork_with(self.clause_min_words)
    }

    pub fn fork_with(&self, clause_min_words: Option<usize>) -> Self {
        Self {
            synth: self.synth.clone(),
            settings: self.settings.clone(),
            clause_min_words,
            aggregator: ClauseAggregator::new(clause_min_words),
            turns: self.turns.clone(),
            speaking: false,
            first_audio: true,
        }
    }

    pub fn synth(&self) -> &SharedSynth {
        &self.synth
    }

    pub fn settings(&self) -> &TtsSettings {
        &self.settings
    }

    pub fn clause_min_words(&self) -> Option<usize> {
        self.clause_min_words
    }

    pub fn voices(&self) -> Vec<String> {
        self.synth
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .voices()
    }

    fn token(&self) -> tokio_util::sync::CancellationToken {
        self.turns
            .as_ref()
            .map_or_else(tokio_util::sync::CancellationToken::new, |t| t.current())
    }

    /// One piece of a reply to the model and out.
    async fn say(&mut self, text: &str, out: &Out) {
        let text = spoken(text);
        if text.trim().is_empty() {
            return;
        }
        let turn = self.token();
        if turn.is_cancelled() {
            return;
        }
        if !self.speaking {
            self.speaking = true;
            self.first_audio = true;
            out.push(Frame::TtsStarted).await;
        }
        let synth = self.synth.clone();
        let (voice, speed) = (self.settings.voice.clone(), self.settings.speed as f32);
        let began = Instant::now();
        let text_for_model = text.clone();
        let rendered = tokio::task::spawn_blocking(move || {
            let mut s = synth.lock().unwrap_or_else(|e| e.into_inner());
            s.synthesize(&text_for_model, &voice, speed)
        })
        .await;
        let pcm = match rendered {
            Ok(Ok(pcm)) => pcm,
            Ok(Err(e)) => {
                warn!("tts: Kokoro failed: {e}");
                return;
            }
            Err(_) => return,
        };
        if turn.is_cancelled() {
            return;
        }
        if self.first_audio {
            self.first_audio = false;
            out.push(Frame::Metrics(Metric::Ttfb {
                value: began.elapsed().as_secs_f64(),
                model: None,
            }))
            .await;
        }
        out.push(Frame::TtsAudio {
            audio: Audio::new(pcm, SAMPLE_RATE),
            turn,
        })
        .await;
    }

    async fn stop_speaking(&mut self, out: &Out) {
        if self.speaking {
            self.speaking = false;
            out.push(Frame::TtsStopped).await;
        }
    }
}

#[async_trait]
impl Stage for KokoroTts {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction != Direction::Down {
            out.forward(frame, direction).await;
            return;
        }
        match &frame {
            Frame::LlmText(text) => {
                let clauses = self.aggregator.push(text);
                for clause in clauses {
                    self.say(&clause, out).await;
                }
                return; // the text was spoken; the taps upstream recorded it
            }
            Frame::LlmEnd => {
                if let Some(rest) = self.aggregator.flush() {
                    self.say(&rest, out).await;
                }
                self.stop_speaking(out).await;
            }
            Frame::Speak(text) => {
                let mut pieces = self.aggregator.push(text);
                pieces.extend(self.aggregator.flush());
                for piece in pieces {
                    self.say(&piece, out).await;
                }
                self.stop_speaking(out).await;
                return;
            }
            Frame::VoiceChange { voice, speed } => {
                if self.voices().contains(voice) {
                    self.settings = TtsSettings {
                        voice: voice.clone(),
                        speed: *speed,
                    };
                } else {
                    warn!(
                        "tts: voice {voice} not found in available voices; keeping {}",
                        self.settings.voice
                    );
                }
                return;
            }
            Frame::Interruption => {
                self.aggregator.reset();
                self.speaking = false;
            }
            Frame::End => {
                if let Some(rest) = self.aggregator.flush() {
                    self.say(&rest, out).await;
                }
                self.stop_speaking(out).await;
            }
            Frame::Cancel => {
                self.aggregator.reset();
                self.speaking = false;
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::drain;
    use crate::narration::NarrationCut;
    use regex::Regex;

    const LIST: &str = "Three need your sign-off: rotating the Discord webhook in infra, the dark mode toggle \
in app, and the Redis versus Postgres call for the session cache.";

    fn tokens(text: &str) -> Vec<String> {
        Regex::new(r"\S+\s*")
            .unwrap()
            .find_iter(text)
            .map(|m| m.as_str().to_string())
            .collect() // roughly how a model streams
    }

    fn chunks(text: &str, min_words: Option<usize>) -> Vec<String> {
        let mut agg = ClauseAggregator::new(min_words);
        let mut out = Vec::new();
        for tok in tokens(text) {
            out.extend(agg.push(&tok));
        }
        out.extend(agg.flush());
        out
    }

    #[test]
    fn test_a_list_answer_starts_at_its_first_clause() {
        assert_eq!(
            chunks(LIST, Some(4)),
            vec![
                "Three need your sign-off:",
                "rotating the Discord webhook in infra,",
                "the dark mode toggle in app,",
                "and the Redis versus Postgres call for the session cache.",
            ]
        );
    }

    #[test]
    fn test_short_openers_numbers_and_times_are_not_split() {
        assert_eq!(
            chunks("Sure, approved it.", Some(4)),
            vec!["Sure, approved it."]
        );
        assert_eq!(
            chunks(
                "We have 3,000 users and the 10:30 build failed, so hold off.",
                Some(4)
            ),
            vec![
                "We have 3,000 users and the 10:30 build failed,",
                "so hold off."
            ]
        );
        assert_eq!(
            chunks("Done - closed infra seventeen, as you asked.", Some(4)),
            vec!["Done - closed infra seventeen,", "as you asked."]
        );
    }

    #[test]
    fn test_sentences_still_end_chunks_and_clauses_concatenate_back() {
        let text = "Closed it. Issue one forty two is next, and it blocks two others.";
        let out = chunks(text, Some(4));
        assert_eq!(
            out,
            vec![
                "Closed it.",
                "Issue one forty two is next,",
                "and it blocks two others."
            ]
        );
        assert_eq!(out.join(" "), text);
    }

    #[test]
    fn test_interruption_drops_the_pending_clause() {
        let mut agg = ClauseAggregator::new(Some(4));
        let _ = agg.push("Three need your sign");
        agg.reset();
        assert_eq!(agg.flush(), None);
    }

    /// Stands in for the loaded model: records what it was asked to say.
    pub(crate) struct FakeKokoro {
        pub spoken: Vec<(String, String)>,
        pub voices: Vec<String>,
    }

    impl FakeKokoro {
        pub(crate) fn shared() -> SharedSynth {
            Arc::new(Mutex::new(Self {
                spoken: Vec::new(),
                voices: vec!["af_heart".into(), "am_adam".into()],
            }))
        }
    }

    impl Synth for FakeKokoro {
        fn voices(&self) -> Vec<String> {
            self.voices.clone()
        }
        fn synthesize(
            &mut self,
            text: &str,
            voice: &str,
            _speed: f32,
        ) -> ghost_infer::Result<Vec<i16>> {
            self.spoken.push((text.to_string(), voice.to_string()));
            Ok(vec![0; 2400])
        }
    }

    fn spoken_texts(synth: &SharedSynth) -> Vec<String> {
        FAKES
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| Arc::ptr_eq(p, synth))
            .map(|(_, v)| v.lock().unwrap().clone())
            .unwrap_or_default()
    }

    // The fake records through a side table (a trait object can't be downcast without
    // `Any` on the trait), keyed by the shared synth.
    type Log = Arc<Mutex<Vec<String>>>;
    static FAKES: Mutex<Vec<(SharedSynth, Log)>> = Mutex::new(Vec::new());

    struct Recording {
        inner: FakeKokoro,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Synth for Recording {
        fn voices(&self) -> Vec<String> {
            self.inner.voices()
        }
        fn synthesize(
            &mut self,
            text: &str,
            voice: &str,
            speed: f32,
        ) -> ghost_infer::Result<Vec<i16>> {
            self.log.lock().unwrap().push(format!("{text}|{voice}"));
            self.inner.synthesize(text, voice, speed)
        }
    }

    fn recording() -> SharedSynth {
        let log = Arc::new(Mutex::new(Vec::new()));
        let synth: SharedSynth = Arc::new(Mutex::new(Recording {
            inner: FakeKokoro {
                spoken: Vec::new(),
                voices: vec!["af_heart".into(), "am_adam".into()],
            },
            log: log.clone(),
        }));
        FAKES.lock().unwrap().push((synth.clone(), log));
        synth
    }

    fn said(synth: &SharedSynth) -> Vec<String> {
        spoken_texts(synth)
            .iter()
            .map(|s| s.split('|').next().unwrap().to_string())
            .collect()
    }

    /// A held reply released by NarrationCut at the end of the response, then TTS.
    async fn say_through_tts(clause_min_words: Option<usize>) -> Vec<String> {
        let synth = recording();
        let mut cut = NarrationCut::default();
        let (out, mut rx) = Out::capture();
        cut.process(Frame::LlmStart, Direction::Down, &out).await;
        for t in tokens(LIST) {
            cut.process(Frame::LlmText(t), Direction::Down, &out).await;
        }
        cut.process(Frame::LlmEnd, Direction::Down, &out).await;
        let released = drain(&mut rx).await;
        let held: String = released
            .iter()
            .filter_map(|f| match f {
                Frame::LlmText(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(held, LIST); // the whole reply, released at its end
        let mut tts =
            KokoroTts::new(synth.clone(), TtsSettings::default(), clause_min_words).unwrap();
        let (out, _rx) = Out::capture();
        for f in released {
            tts.process(f, Direction::Down, &out).await;
        }
        said(&synth)
    }

    #[tokio::test]
    async fn test_narration_cut_then_kokoro_clause_by_clause() {
        assert_eq!(
            say_through_tts(Some(4)).await,
            vec![
                "Three need your sign-off:",
                "rotating the Discord webhook in infra,",
                "the dark mode toggle in app,",
                "and the Redis versus Postgres call for the session cache.",
            ]
        );
    }

    #[tokio::test]
    async fn test_whole_sentences_when_clauses_are_off() {
        assert_eq!(say_through_tts(None).await, vec![LIST]);
    }

    #[test]
    fn test_fork_keeps_or_overrides_the_clause_setting() {
        let template =
            KokoroTts::new(FakeKokoro::shared(), TtsSettings::default(), Some(4)).unwrap();
        assert_eq!(template.fork().clause_min_words(), Some(4));
        assert_eq!(template.fork_with(None).clause_min_words(), None);
    }

    // --- each session's TTS over the one loaded model ------------------------------

    async fn speak(tts: &mut KokoroTts, then_cancel: bool) -> usize {
        let (out, mut rx) = Out::capture();
        tts.process(Frame::Speak("hello".into()), Direction::Down, &out)
            .await;
        if then_cancel {
            tts.process(Frame::Cancel, Direction::Down, &out).await;
        }
        drain(&mut rx)
            .await
            .iter()
            .filter(|f| matches!(f, Frame::TtsAudio { .. }))
            .count()
    }

    #[tokio::test]
    async fn test_session_after_a_cancelled_one_still_speaks() {
        let synth = recording();
        let template = KokoroTts::new(synth.clone(), TtsSettings::default(), None).unwrap();
        let mut first = template.fork();
        assert!(speak(&mut first, true).await > 0);
        let mut second = template.fork();
        assert!(Arc::ptr_eq(second.synth(), &synth)); // the loaded model is reused, not reloaded
        assert!(speak(&mut second, false).await > 0);
        assert_eq!(
            spoken_texts(&synth),
            vec!["hello|af_heart".to_string(), "hello|af_heart".to_string()]
        );
    }

    #[test]
    fn test_fork_keeps_the_template_settings() {
        let tts = KokoroTts::new(
            FakeKokoro::shared(),
            TtsSettings {
                voice: "am_adam".into(),
                speed: 1.2,
            },
            None,
        )
        .unwrap();
        let fork = tts.fork();
        assert_eq!(fork.settings().voice, "am_adam");
        assert_eq!(fork.settings().speed, 1.2);
    }

    #[test]
    fn test_unknown_voice_is_rejected() {
        let err = KokoroTts::new(
            FakeKokoro::shared(),
            TtsSettings {
                voice: "nope".into(),
                speed: 1.0,
            },
            None,
        )
        .err();
        assert_eq!(err, Some(TtsError::UnknownVoice("nope".into())));
    }
}
