//! The trained wake detectors, with Parakeet as their check (ADR 0025).
//!
//! While passive in `GHOST_WAKE_MODE=onnx` the microphone goes to small acoustic models
//! (`ghost_infer::wake`, ONNX) instead of to a transcript of every utterance. Every 80 ms
//! they score the last 2 s, sharing one pass of the audio features. Each model listens for
//! one thing (`model_meaning`):
//!
//! - `hey_ghost`: "hey ghost". The words after it are the first turn.
//! - `ghost_*` (`ghost_address`, or one model per opener): the operator addressing Ghost
//!   by name with a request, trained on openers such as "Ghost, I need you", "Ghost, let's
//!   work on", "Ghost, what is the status of" and "Ghost, where did we leave off".
//!   Everything after "Ghost" is the first turn, so the request arrives whole.
//!
//! A model's score at or above its threshold is a hit, decided the moment its phrase ends:
//!
//! - a weak hit, a score under the threshold but at the model's `checked_from` (the
//!   operator's "Hey Ghost" under music), is checked by Parakeet whatever the rule below;
//! - with the check on (`GHOST_WAKE_VERIFY`, the default), Parakeet transcribes from where
//!   the operator started talking (at least from just before the phrase) to their first
//!   pause, or 1 s after the hit if they keep talking, and the model's phrase must be in it:
//!   `match_wake` for "hey ghost", `match_opening` for "Ghost, …", which must open a
//!   sentence. A hit it doesn't confirm is read once more, whole, when they finish, and
//!   dropped only if that fails too;
//! - with the check off the hit alone wakes Ghost.
//!
//! Either way `on_heard` runs as soon as it is decided, so the chime can play while the
//! operator is still talking, and listening goes on until they finish. Only what the
//! models were trained on wakes Ghost in this mode.
//!
//! Nothing leaves the machine, and as with transcript wake the transcripts are dropped
//! here: logged only under `GHOST_DEBUG_AUDIO` (`debug`). The INFO log gets scores and
//! decisions, never words. Passive audio ends at this stage: it is never forwarded.

use std::sync::Arc;

use async_trait::async_trait;
use ghost_core::source::BoxFuture;
use ghost_infer::wake::detector::{
    ADDRESS_PHRASE, CHECK_SECS, MAX_LISTEN_SECS, MAX_PRE_SECS, OPENERS, OPENING_PRE_SECS,
    REMAINDER_FUZZY, RUN_GAP_SECS, SAMPLE_RATE, SPEECH_PAD_SECS, STEP, WINDOW, heard_from,
    pick_hit,
};
use ghost_infer::wake::{WakeMatch, WakeModel, WakeScorer, match_opening};
use tracing::info;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};

pub use ghost_infer::wake::detector::{DETECTOR_PHRASE, model_meaning};

/// The models' scores for the last 2 s: the loaded detectors, or a fake in tests.
pub trait Scorer: Send {
    fn score(&mut self, window: &[i16]) -> ghost_infer::Result<Vec<(String, f32)>>;
}

impl Scorer for WakeScorer {
    fn score(&mut self, window: &[i16]) -> ghost_infer::Result<Vec<(String, f32)>> {
        WakeScorer::score(self, window)
    }
}

/// float32 16 kHz -> text; Parakeet, off the async runtime.
pub type Recognize = Arc<dyn Fn(Vec<f32>) -> String + Send + Sync>;
pub type OnMatch = Arc<dyn Fn(WakeMatch) -> BoxFuture<'static, ()> + Send + Sync>;
pub type OnHeard = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// What the passive listener needs to wake on the trained models.
pub struct Detector {
    pub scorer: Box<dyn Scorer>,
    pub recognize: Recognize,
    pub models: Vec<WakeModel>,
    pub verify: bool,
    pub cooldown_secs: f64,
}

/// Last stage of the passive pipeline in detector mode, after the VAD.
///
/// `end_secs` is the silence, beyond the VAD's own `stop_secs`, that ends the operator's
/// utterance once Ghost is woken: short pauses (a comma after "hey ghost") don't end it.
/// Positions are counted in samples received, not wall time, so a slow check never
/// shortens what is heard.
pub struct DetectorWakeProcessor {
    d: Detector,
    on_match: OnMatch,
    on_heard: Option<OnHeard>,
    fuzzy: f64,
    end: usize,
    keep: usize,
    buf: Vec<i16>,
    base: usize,
    n: usize,
    scored: usize,
    speaking: bool,
    stopped_at: Option<usize>,
    run_start: Option<usize>,
    heard_from_at: usize,
    checked: bool,
    hit: Option<usize>,
    model: usize,
    last_hit: i64,
    /// Where the last "best score under its threshold" was logged (GHOST_DEBUG_AUDIO).
    last_best_log: usize,
    woken: bool,
    done: bool,
}

impl DetectorWakeProcessor {
    pub fn new(
        detector: Detector,
        on_match: OnMatch,
        on_heard: Option<OnHeard>,
        fuzzy: f64,
        end_secs: f64,
    ) -> Self {
        Self {
            d: detector,
            on_match,
            on_heard,
            fuzzy,
            end: (end_secs * SAMPLE_RATE as f64) as usize,
            keep: ((OPENING_PRE_SECS + MAX_LISTEN_SECS + 1.0) * SAMPLE_RATE as f64) as usize,
            buf: Vec::new(),
            base: 0,
            n: 0,
            scored: 0,
            speaking: false,
            stopped_at: None,
            run_start: None,
            heard_from_at: 0,
            checked: false,
            hit: None,
            model: 0,
            last_hit: -(1i64 << 40),
            last_best_log: 0,
            woken: false,
            done: false,
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    // --- audio ----------------------------------------------------------------------

    async fn audio(&mut self, pcm: &[i16]) {
        if self.done {
            return;
        }
        self.buf.extend_from_slice(pcm);
        self.n += pcm.len();
        if self.buf.len() > self.keep {
            let drop = self.buf.len() - self.keep;
            self.buf.drain(..drop);
            self.base += drop;
        }
        while self.n - self.scored >= STEP && !self.done {
            self.scored += STEP;
            if self.hit.is_none() && self.scored >= WINDOW {
                self.score(self.scored).await;
            }
        }
        let Some(hit) = self.hit else { return };
        if self.done {
            return;
        }
        let ended =
            self.finished() || self.n - hit >= (MAX_LISTEN_SECS * SAMPLE_RATE as f64) as usize;
        if !self.woken && !self.checked {
            if self.n - hit >= (CHECK_SECS * SAMPLE_RATE as f64) as usize {
                self.check(false).await; // still talking a second after "ghost"
            }
        } else if !self.woken {
            if ended {
                self.check(true).await;
            }
        } else if ended {
            self.finish(None).await;
        }
    }

    fn finished(&self) -> bool {
        !self.speaking && self.stopped_at.is_some_and(|s| self.n - s >= self.end)
    }

    /// From where the operator started talking, or just before the hit's phrase if that
    /// is earlier, to now, as float32 for Parakeet.
    fn heard(&mut self) -> Vec<f32> {
        let hit = self.hit.expect("a hit");
        let opening = self.d.models[self.model].opening;
        let start = heard_from(hit, self.run_start, opening);
        let start = start.max(hit.saturating_sub((MAX_PRE_SECS * SAMPLE_RATE as f64) as usize));
        let start = start.saturating_sub(self.base).min(self.buf.len());
        self.heard_from_at = start + self.base;
        ghost_infer::audio::to_f32(&self.buf[start..])
    }

    /// What a reading covered, for the debug log: numbers only.
    fn describe(&self, audio: &[f32]) -> String {
        let hit = self.hit.unwrap_or(0);
        let rms = if audio.is_empty() {
            0.0
        } else {
            (audio.iter().map(|x| x * x).sum::<f32>() / audio.len() as f32).sqrt()
        };
        let level = 20.0 * rms.max(1e-9).log10();
        let before = hit.saturating_sub(self.heard_from_at) as f64 / SAMPLE_RATE as f64;
        let secs = audio.len() as f64 / SAMPLE_RATE as f64;
        // A sum of the samples' magnitudes, so two readings of "the same" stretch can be
        // told apart (2026-10-30 19:51: a 7.0 s check read a sentence, the final check of
        // 7.0 s at the same level read nothing).
        let sum: f64 = audio.iter().map(|x| f64::from(x.abs())).sum();
        format!(
            "{secs:.1}s from {before:.1}s before the hit, {level:.0} dBFS, {} samples, sum {sum:.0}",
            audio.len()
        )
    }

    // --- decisions ------------------------------------------------------------------

    async fn score(&mut self, end: usize) {
        if end < WINDOW + self.base {
            return;
        }
        let window = &self.buf[end - WINDOW - self.base..end - self.base];
        if window.len() < WINDOW {
            return;
        }
        let Ok(scores) = self.d.scorer.score(window) else {
            return;
        };
        let hit = pick_hit(&scores, &self.d.models);
        let Some((score, model)) = hit else {
            // A near miss, for the operator tuning the detector against the room: the best
            // score of this stretch, at most twice a second.
            if let Some((name, best)) = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1))
                && *best >= 0.2
                && end.saturating_sub(self.last_best_log) >= SAMPLE_RATE / 2
            {
                self.last_best_log = end;
                debug::audio(|| format!("wake: best {name} {best:.2}, under its threshold"));
            }
            return;
        };
        if (end as i64 - self.last_hit) < (self.d.cooldown_secs * SAMPLE_RATE as f64) as i64 {
            return;
        }
        self.hit = Some(end);
        self.last_hit = end as i64;
        self.model = model;
        let m = &self.d.models[model];
        if score < m.threshold {
            info!(
                "wake: detector heard {} ({score:.2}), under {:.2}: Parakeet checks it",
                m.name, m.threshold
            );
        } else {
            info!("wake: detector heard {} ({score:.2})", m.name);
        }
        if !m.needs_check(score, self.d.verify) {
            self.wake().await;
        }
    }

    /// Parakeet's reading of the hit. The first reading is of the stretch around it, so
    /// the chime can play while the operator is still talking; live, Parakeet often wrote
    /// nothing for that stretch while their whole sentence read fine, so a miss is read
    /// again, whole, once they finish (`final_`), and only that drops the hit.
    async fn check(&mut self, final_: bool) {
        let audio = self.heard();
        let described = self.describe(&audio);
        let recognize = self.d.recognize.clone();
        let text = tokio::task::spawn_blocking(move || recognize(audio))
            .await
            .unwrap_or_default();
        debug::audio(|| {
            format!(
                "passive: check{} read {described}",
                if final_ { " (final)" } else { "" }
            )
        });
        if self.d.models[self.model]
            .matches(&text, self.fuzzy)
            .is_some()
        {
            info!(
                "wake: confirmed{}",
                if final_ { " once they finished" } else { "" }
            );
            self.wake().await;
            if final_ {
                self.finish(Some(text)).await;
            } else if self.finished() {
                self.finish(None).await;
            }
            return;
        }
        // Transcripts are logged only under GHOST_DEBUG_AUDIO (debug).
        debug::audio(|| format!("passive: detector hit not confirmed: {text:?}"));
        if !final_ {
            info!("wake: not confirmed yet; reading it all when they finish");
            self.checked = true;
            return;
        }
        info!("wake: not confirmed");
        self.hit = None;
        self.checked = false;
    }

    async fn wake(&mut self) {
        self.woken = true;
        if let Some(on_heard) = &self.on_heard {
            on_heard().await;
        }
    }

    async fn finish(&mut self, text: Option<String>) {
        self.done = true;
        let text = match text {
            Some(t) => t,
            None => {
                let audio = self.heard();
                let described = self.describe(&audio);
                let recognize = self.d.recognize.clone();
                let t = tokio::task::spawn_blocking(move || recognize(audio))
                    .await
                    .unwrap_or_default();
                debug::audio(|| format!("passive: first turn read {described}"));
                t
            }
        };
        let model = &self.d.models[self.model];
        let fuzzy = self.fuzzy.min(REMAINDER_FUZZY);
        // hey_ghost also fires on the bare name ("Ghost, let's work on …" scored 0.74 on
        // the replay, ahead of the opener's own model): what follows the name is the
        // request.
        let m = model
            .matches(&text, fuzzy)
            .or_else(|| match_opening(&text, ADDRESS_PHRASE, fuzzy, OPENERS));
        let name = model.name.clone();
        debug::audio(|| format!("passive: woken by {name}, heard {text:?}"));
        let remainder = match m {
            Some(m) => m.remainder,
            // The model heard "Ghost, <opener>" but the transcript lost the name ("Coast,
            // what is…"): the request is still all of it, which beats dropping it.
            None if model.opening => text.trim().to_string(),
            None => String::new(),
        };
        (self.on_match)(WakeMatch::new(&model.phrase, &remainder)).await;
    }
}

#[async_trait]
impl Stage for DetectorWakeProcessor {
    fn name(&self) -> &'static str {
        "wake"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::AudioIn(audio) => {
                self.audio(&audio.samples).await;
                return; // passive audio ends here
            }
            Frame::VadStarted { .. } => {
                let gap = (RUN_GAP_SECS * SAMPLE_RATE as f64) as usize;
                if self.stopped_at.is_none_or(|s| self.n - s >= gap) {
                    self.run_start = Some(
                        self.n
                            .saturating_sub((SPEECH_PAD_SECS * SAMPLE_RATE as f64) as usize),
                    );
                }
                self.speaking = true;
                self.stopped_at = None;
            }
            Frame::VadStopped { .. } => {
                self.speaking = false;
                self.stopped_at = Some(self.n);
                if self.hit.is_some() && !self.woken && !self.checked {
                    self.check(false).await;
                }
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::frames::Audio;

    const RATE: usize = 16000;
    const FIRST_SCORE: usize = WINDOW / STEP; // the frame whose end completes the first 2 s window

    fn hey() -> WakeModel {
        WakeModel {
            name: "hey_ghost".into(),
            phrase: "hey ghost".into(),
            threshold: 0.5,
            opening: false,
            verify: None,
            checked_from: None,
        }
    }

    fn address() -> WakeModel {
        WakeModel {
            name: "ghost_address".into(),
            phrase: "ghost".into(),
            threshold: 0.5,
            opening: true,
            verify: None,
            checked_from: None,
        }
    }

    /// `model` fires on the given 1-based score calls.
    struct FakeScorer {
        hits: Vec<usize>,
        score: f32,
        model: String,
        calls: usize,
    }

    impl FakeScorer {
        fn new(hits: &[usize]) -> Self {
            Self::with(hits, 0.9, "hey_ghost")
        }
        fn with(hits: &[usize], score: f32, model: &str) -> Self {
            Self {
                hits: hits.to_vec(),
                score,
                model: model.into(),
                calls: 0,
            }
        }
    }

    impl Scorer for FakeScorer {
        fn score(&mut self, _window: &[i16]) -> ghost_infer::Result<Vec<(String, f32)>> {
            self.calls += 1;
            let s = if self.hits.contains(&self.calls) {
                self.score
            } else {
                0.0
            };
            Ok(vec![(self.model.clone(), s)])
        }
    }

    #[derive(Default)]
    struct RecognizerState {
        calls: usize,
        secs: Vec<f64>, // how much audio each call got
    }

    struct Recognizer {
        texts: Vec<String>,
        state: Arc<Mutex<RecognizerState>>,
    }

    impl Recognizer {
        fn new(texts: &[&str]) -> Self {
            Self {
                texts: texts.iter().map(|s| s.to_string()).collect(),
                state: Arc::new(Mutex::new(RecognizerState::default())),
            }
        }

        fn recognize(&self) -> Recognize {
            let texts = self.texts.clone();
            let state = self.state.clone();
            Arc::new(move |pcm: Vec<f32>| {
                let mut s = state.lock().unwrap();
                s.calls += 1;
                s.secs.push(pcm.len() as f64 / RATE as f64);
                texts[(s.calls.min(texts.len())) - 1].clone()
            })
        }

        fn calls(&self) -> usize {
            self.state.lock().unwrap().calls
        }

        fn secs(&self) -> Vec<f64> {
            self.state.lock().unwrap().secs.clone()
        }
    }

    fn audio(secs: f64) -> Vec<Frame> {
        (0..((secs * RATE as f64 / STEP as f64).round() as usize))
            .map(|_| Frame::AudioIn(Audio::new(vec![0; STEP], RATE as u32)))
            .collect()
    }

    fn started() -> Frame {
        Frame::VadStarted { start_secs: 0.2 }
    }

    fn stopped() -> Frame {
        Frame::VadStopped { stop_secs: 0.2 }
    }

    fn detector(scorer: FakeScorer, recognize: &Recognizer, verify: bool) -> Detector {
        Detector {
            scorer: Box::new(scorer),
            recognize: recognize.recognize(),
            models: vec![hey(), address()],
            verify,
            cooldown_secs: 2.0,
        }
    }

    /// What happened, in order: "heard", then "match:<phrase>|<remainder>".
    async fn run_with(
        detector: Detector,
        frames: Vec<Frame>,
        heard_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Vec<String> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let on_heard: OnHeard = Arc::new(move || {
            let e = e.clone();
            let hook = heard_hook.clone();
            Box::pin(async move {
                if let Some(h) = hook {
                    h();
                }
                e.lock().unwrap().push("heard".to_string());
            })
        });
        let e = events.clone();
        let on_match: OnMatch = Arc::new(move |m| {
            let e = e.clone();
            Box::pin(async move {
                e.lock()
                    .unwrap()
                    .push(format!("match:{}|{}", m.phrase, m.remainder));
            })
        });
        let mut proc_ = DetectorWakeProcessor::new(detector, on_match, Some(on_heard), 0.9, 0.3);
        let (out, _rx) = Out::capture();
        for f in frames {
            proc_.process(f, Direction::Down, &out).await;
        }
        let seen: Vec<String> = events.lock().unwrap().clone();
        seen
    }

    async fn run(detector: Detector, frames: Vec<Frame>) -> Vec<String> {
        run_with(detector, frames, None).await
    }

    fn frames(parts: Vec<Vec<Frame>>) -> Vec<Frame> {
        parts.into_iter().flatten().collect()
    }

    #[tokio::test]
    async fn test_a_confirmed_hit_wakes_with_what_followed() {
        let recognize = Recognizer::new(&["Hey Ghost, what needs my sign-off?"]);
        let events = run(
            detector(FakeScorer::new(&[3]), &recognize, true), // "ghost" ends about 2.2 s in
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:hey ghost|what needs my sign-off?"]
        );
    }

    #[tokio::test]
    async fn test_an_unconfirmed_hit_is_dropped() {
        let recognize = Recognizer::new(&["He goes to the gym every morning."]);
        let events = run(
            detector(FakeScorer::new(&[3]), &recognize, true),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(1.0),
            ]),
        )
        .await;
        assert!(events.is_empty());
        assert_eq!(recognize.calls(), 2); // at the pause, then the whole utterance once it ended
    }

    #[tokio::test]
    async fn test_a_hit_the_first_check_missed_is_checked_again_when_they_finish() {
        let recognize = Recognizer::new(&["", "Ghost, I need you to make wake words work."]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                true,
            ),
            frames(vec![
                vec![started()],
                audio(3.4),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|I need you to make wake words work."]
        );
        assert_eq!(recognize.calls(), 2); // the first turn reuses the second reading
    }

    #[tokio::test]
    async fn test_a_pause_after_the_name_keeps_it_in_the_check() {
        let recognize = Recognizer::new(&["Ghost. I need you to look at the build."]);
        let events = run(
            detector(
                FakeScorer::with(&[14], 0.9, "ghost_address"),
                &recognize,
                true,
            ), // the hit at 3.04 s
            frames(vec![
                audio(0.5),
                vec![started()], // "Ghost." from about 0.3 s
                audio(0.4),
                vec![stopped()],
                audio(1.5),
                vec![started()], // "I need you to look at the build"
                audio(1.0),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|I need you to look at the build."]
        );
        assert!(recognize.secs()[0] >= 3.3); // from before the name, not from the request
    }

    #[tokio::test]
    async fn test_a_misspelled_name_passes_when_the_opener_follows() {
        let recognize = Recognizer::new(&[
            "Coach, I need you to work on",
            "I need you to work on some of the things.",
        ]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                true,
            ),
            frames(vec![
                vec![started()],
                audio(3.4),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec![
                "heard",
                "match:ghost|I need you to work on some of the things."
            ]
        );
    }

    #[tokio::test]
    async fn test_the_check_reads_from_where_the_operator_started_talking() {
        let recognize = Recognizer::new(&["Ghost, I need you to look at the build."]);
        let events = run(
            detector(
                FakeScorer::with(&[26], 0.9, "ghost_address"),
                &recognize,
                true,
            ), // the hit at 4.0 s
            frames(vec![
                audio(0.5),
                vec![started()], // "Ghost," from about 0.3 s
                audio(0.4),
                vec![stopped()],
                audio(0.24),
                vec![started()], // "I need you to look at the build"
                audio(3.5),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|I need you to look at the build."]
        );
        assert!(recognize.secs()[0] >= 4.4); // from the start of the speech, not 2.5 s before the hit
    }

    #[tokio::test]
    async fn test_without_the_check_the_hit_alone_wakes_at_once() {
        // The chime can play the moment the detector fires; the first turn still comes
        // from the whole utterance, where a looser spelling finds the phrase's end.
        let recognize = Recognizer::new(&["He goes, what needs my sign-off?"]);
        let heard_before_stop = Arc::new(Mutex::new(Vec::new()));
        let h = heard_before_stop.clone();
        let state = recognize.state.clone();
        let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            h.lock().unwrap().push(state.lock().unwrap().calls == 0);
        });
        let events = run_with(
            detector(FakeScorer::new(&[3]), &recognize, false),
            frames(vec![
                vec![started()],
                audio(3.0),
                vec![stopped()],
                audio(0.4),
            ]),
            Some(hook),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:hey ghost|what needs my sign-off?"]
        );
        assert_eq!(*heard_before_stop.lock().unwrap(), vec![true]);
        assert_eq!(recognize.calls(), 1); // only to split off the first turn
    }

    #[tokio::test]
    async fn test_a_pause_after_the_phrase_doesnt_end_the_utterance() {
        let recognize = Recognizer::new(&["Hey Ghost.", "Hey Ghost, any new issues today?"]);
        let events = run(
            detector(FakeScorer::new(&[3]), &recognize, true),
            frames(vec![
                vec![started()],
                audio(2.4),
                vec![stopped()], // the comma: checked here, and confirmed
                audio(0.16),
                vec![started()], // ... and they go on
                audio(1.0),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:hey ghost|any new issues today?"]
        );
        assert_eq!(recognize.calls(), 2);
    }

    #[tokio::test]
    async fn test_talking_on_is_checked_a_second_after_the_hit() {
        let recognize =
            Recognizer::new(&["Hey Ghost, read me", "Hey Ghost, read me the latest bug."]);
        let events = run(
            detector(FakeScorer::new(&[3]), &recognize, true),
            frames(vec![vec![started()], audio(3.4)]), // no pause yet
        )
        .await;
        assert_eq!(events, vec!["heard"]);
        assert_eq!(recognize.calls(), 1);
    }

    #[tokio::test]
    async fn test_a_weak_hit_wakes_only_when_parakeet_reads_the_phrase() {
        // 2026-10-30: "Hey Ghost" over Spotify scored 0.25-0.62 against 0.7. A score at
        // the checked floor is read back, with the check off for the model.
        let recognize = Recognizer::new(&["Hey Ghost, what needs my sign-off?"]);
        let mut d = detector(FakeScorer::with(&[3], 0.4, "hey_ghost"), &recognize, false);
        d.models[0].checked_from = Some(0.3);
        let events = run(
            d,
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:hey ghost|what needs my sign-off?"]
        );
        // the music alone at the same score: read, not confirmed, dropped
        let recognize = Recognizer::new(&["He goes to the gym every morning."]);
        let mut d = detector(FakeScorer::with(&[3], 0.4, "hey_ghost"), &recognize, false);
        d.models[0].checked_from = Some(0.3);
        let events = run(
            d,
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(1.0),
            ]),
        )
        .await;
        assert!(events.is_empty());
        assert_eq!(recognize.calls(), 2);
        // without the floor the same score is no hit at all
        let recognize = Recognizer::new(&["Hey Ghost, what needs my sign-off?"]);
        let events = run(
            detector(FakeScorer::with(&[3], 0.4, "hey_ghost"), &recognize, false),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(1.0),
            ]),
        )
        .await;
        assert!(events.is_empty());
        assert_eq!(recognize.calls(), 0);
    }

    #[tokio::test]
    async fn test_a_low_score_is_not_a_hit() {
        let recognize = Recognizer::new(&["Hey Ghost."]);
        let events = run(
            Detector {
                scorer: Box::new(FakeScorer::with(&[3], 0.2, "hey_ghost")),
                recognize: recognize.recognize(),
                models: vec![hey()],
                verify: true,
                cooldown_secs: 2.0,
            },
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert!(events.is_empty());
        assert_eq!(recognize.calls(), 0); // Parakeet never ran
    }

    #[tokio::test]
    async fn test_addressing_ghost_by_name_wakes_with_the_whole_request() {
        // "Ghost, what is the status of …": the model fires as the opener ends, and the
        // request starts after the name, not after the opener.
        let recognize = Recognizer::new(&["Ghost, what is the status of the infra repo?"]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                true,
            ),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|what is the status of the infra repo?"]
        );
    }

    #[tokio::test]
    async fn test_the_name_inside_a_sentence_is_not_addressing_ghost() {
        let recognize = Recognizer::new(&["The ghost story we read last night was great."]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                true,
            ),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(1.0),
            ]),
        )
        .await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_hey_ghost_firing_on_the_name_keeps_the_request() {
        // hey_ghost also fires on the name ("Ghost, let's work on …" scored 0.74 on the
        // replay), before an opener model gets there; with no "hey ghost" in the
        // transcript the request is still what follows the name, not a greeting that
        // drops it.
        let recognize = Recognizer::new(&["Ghost, let's work on the wake word."]);
        let events = run(
            detector(FakeScorer::new(&[3]), &recognize, false),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:hey ghost|let's work on the wake word."]
        );
    }

    #[tokio::test]
    async fn test_without_the_check_a_misheard_name_keeps_the_request() {
        // The model heard "Ghost, …" but Parakeet wrote something else: shaped like the
        // name with an opener after it, the request is what follows; otherwise it is all
        // of it, rather than a greeting that drops what the operator asked.
        let recognize = Recognizer::new(&["Coast, let's work on the wake word."]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                false,
            ),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|let's work on the wake word."]
        );
        let recognize = Recognizer::new(&["Toast, can you check the build."]);
        let events = run(
            detector(
                FakeScorer::with(&[3], 0.9, "ghost_address"),
                &recognize,
                false,
            ),
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert_eq!(
            events,
            vec!["heard", "match:ghost|Toast, can you check the build."]
        );
    }

    #[tokio::test]
    async fn test_each_model_answers_to_its_own_threshold() {
        let recognize = Recognizer::new(&["Ghost, I need you to look at the build."]);
        let events = run(
            Detector {
                scorer: Box::new(FakeScorer::with(&[3], 0.6, "ghost_address")),
                recognize: recognize.recognize(),
                models: vec![
                    hey(),
                    WakeModel {
                        threshold: 0.7,
                        ..address()
                    },
                ],
                verify: true,
                cooldown_secs: 2.0,
            },
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(0.4),
            ]),
        )
        .await;
        assert!(events.is_empty());
        assert_eq!(recognize.calls(), 0);
    }

    #[tokio::test]
    async fn test_a_model_can_be_checked_while_the_others_are_not() {
        // "hey ghost" wakes at once; a "Ghost, …" hit waits for Parakeet.
        let recognize = Recognizer::new(&["He goes to the gym every morning."]);
        let events = run(
            Detector {
                scorer: Box::new(FakeScorer::with(&[3], 0.9, "ghost_address")),
                recognize: recognize.recognize(),
                models: vec![
                    hey(),
                    WakeModel {
                        verify: Some(true),
                        ..address()
                    },
                ],
                verify: false,
                cooldown_secs: 2.0,
            },
            frames(vec![
                vec![started()],
                audio(2.6),
                vec![stopped()],
                audio(1.0),
            ]),
        )
        .await;
        assert!(events.is_empty()); // checked, and turned down
        assert_eq!(recognize.calls(), 2); // at the pause, then whole
    }

    #[test]
    fn the_first_score_completes_the_window() {
        assert_eq!(FIRST_SCORE, 25);
    }
}
