//! Speech in: Silero's segments and Parakeet's transcripts (`ghost_voice.parakeet` as a
//! segmented STT service), with the interim re-reads while Ghost speaks.
//!
//! A segment is transcribed once the VAD says it ended, so an operator talking over Ghost
//! was heard, and could cut it off (`turns`), only after they paused. With `interim_secs`
//! above zero, while Ghost is speaking and the operator's segment is open, the segment so
//! far (the pre-roll included) is transcribed again every `interim_secs` and pushed as
//! [`Frame::Interim`]. Only the barge-in rule and the echo filter read those; the context,
//! the interceptors, the confirmation path and the transcript store take final
//! [`Frame::Transcript`]s only. The wake listener's stage has them off.
//!
//! One recognition runs at a time on the loaded model, across every session: a final waits
//! for an interim in flight, and an interim tick that finds the model busy is skipped.
//!
//! The VAD runs here (Pipecat ran it in the user aggregator and sent its frames back up):
//! the frames it makes are the same, `VadStarted` and `VadStopped`, and every stage below
//! sees them in order with the audio. Transcripts are logged only under
//! `GHOST_DEBUG_AUDIO` (`debug`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ghost_infer::parakeet::{InterimSchedule, interim_audio};
use ghost_infer::{Parakeet, VadAnalyzer, VadParams, VadState};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::debug;
use crate::frames::{Direction, Frame, Metric, Out, Stage};

pub const SAMPLE_RATE: u32 = 16_000;
/// Audio kept before the VAD's start, so the first word isn't clipped: the VAD fires its
/// `start_secs` and the model's onset after speech began, and Parakeet reads a first word
/// better with the room's silence before it (a 0.4 s pre-roll lost "What's" from "What's
/// waiting for triage?" on the replay rig).
pub const PRE_ROLL_SECS: f64 = 1.0;
/// Silence after a segment, so the model sees its end.
pub const TRAILING_SECS: f64 = 0.2;

/// The model behind the stage: Parakeet, or a fake in tests.
pub trait Recognizer: Send {
    /// float32 16 kHz -> text.
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String>;
}

impl Recognizer for Parakeet {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        Parakeet::transcribe(self, audio)
    }
}

/// One loaded model, shared by every session's stage and the wake listener's check.
pub type SharedRecognizer = Arc<Mutex<dyn Recognizer>>;

/// Transcribe in the calling thread (off the async runtime), taking a turn on the shared
/// model. With `wait` off, None without running when the model is busy.
pub fn recognize(model: &SharedRecognizer, audio: &[f32], wait: bool) -> Option<String> {
    let mut guard = if wait {
        model.lock().unwrap_or_else(|e| e.into_inner())
    } else {
        match model.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        }
    };
    match guard.transcribe(audio) {
        Ok(text) => Some(text.trim().to_string()),
        Err(e) => {
            warn!("Parakeet failed: {e}");
            Some(String::new())
        }
    }
}

/// The recognizer as the wake listener's check takes it: one reading, waiting its turn.
pub fn recognize_fn(model: &SharedRecognizer) -> crate::wakeword::Recognize {
    let model = model.clone();
    Arc::new(move |audio: Vec<f32>| recognize(&model, &audio, true).unwrap_or_default())
}

fn epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Parakeet TDT as one session's speech-in stage.
pub struct ParakeetStt {
    model: SharedRecognizer,
    vad: Option<VadAnalyzer>,
    params: VadParams,
    schedule: Arc<Mutex<InterimSchedule>>,
    interim_secs: f64,
    pre_roll: VecDeque<i16>,
    segment: Vec<i16>,
    user_speaking: bool,
    interim_task: Option<JoinHandle<()>>,
    zero: Instant,
}

impl ParakeetStt {
    /// `vad` None: the VAD frames come from upstream (a test, or a transport with its own).
    /// `interim_secs` is for sessions (see the module doc); passive audio gets none.
    pub fn new(
        model: SharedRecognizer,
        vad: Option<VadAnalyzer>,
        params: VadParams,
        interim_secs: f64,
    ) -> Self {
        Self {
            model,
            vad,
            params,
            schedule: Arc::new(Mutex::new(InterimSchedule::new(interim_secs))),
            interim_secs,
            pre_roll: VecDeque::new(),
            segment: Vec::new(),
            user_speaking: false,
            interim_task: None,
            zero: Instant::now(),
        }
    }

    pub fn model(&self) -> &SharedRecognizer {
        &self.model
    }

    pub fn interim_secs(&self) -> f64 {
        self.interim_secs
    }

    fn now(&self) -> f64 {
        self.zero.elapsed().as_secs_f64()
    }

    fn busy(&self) -> bool {
        self.interim_task.as_ref().is_some_and(|t| !t.is_finished())
            || matches!(
                self.model.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            )
    }

    async fn vad_started(&mut self, out: &Out) {
        self.schedule.lock().unwrap().vad_started();
        self.user_speaking = true;
        self.segment = self.pre_roll.iter().copied().collect();
        out.push(Frame::VadStarted {
            start_secs: self.params.start_secs,
        })
        .await;
    }

    async fn vad_stopped(&mut self, out: &Out) {
        self.schedule.lock().unwrap().vad_stopped();
        self.user_speaking = false;
        out.push(Frame::VadStopped {
            stop_secs: self.params.stop_secs,
        })
        .await;
        self.final_(out).await;
    }

    /// The segment's transcript: waits for an interim in flight, then takes the model.
    async fn final_(&mut self, out: &Out) {
        let mut segment = std::mem::take(&mut self.segment);
        if segment.is_empty() {
            return;
        }
        segment.extend(std::iter::repeat_n(
            0i16,
            (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize,
        ));
        let audio = ghost_infer::audio::to_f32(&segment);
        let model = self.model.clone();
        let began = Instant::now();
        let text = tokio::task::spawn_blocking(move || recognize(&model, &audio, true))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        out.push(Frame::Metrics(Metric::Ttfb {
            value: began.elapsed().as_secs_f64(),
            model: None,
        }))
        .await;
        if !text.is_empty() {
            debug::audio(|| format!("heard: {text:?}"));
            out.push(Frame::Transcript(text)).await;
        }
    }

    fn maybe_interim(&mut self, out: &Out) {
        if self.interim_secs <= 0.0 {
            return;
        }
        let busy = self.busy();
        let due = self.schedule.lock().unwrap().tick(self.now(), busy);
        let Some(segment_id) = due else { return };
        let window = (f64::from(SAMPLE_RATE)
            * f64::from(ghost_infer::parakeet::INTERIM_WINDOW_SECS)) as usize;
        let start = self.segment.len().saturating_sub(window);
        let trailing = vec![0i16; (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize];
        let audio = interim_audio(&self.segment[start..], &trailing);
        let model = self.model.clone();
        let schedule = self.schedule.clone();
        let out = out.clone();
        self.interim_task = Some(tokio::spawn(async move {
            let text = tokio::task::spawn_blocking(move || recognize(&model, &audio, false))
                .await
                .ok()
                .flatten();
            let Some(text) = text.filter(|t| !t.is_empty()) else {
                return;
            };
            // The segment ended (its final is queued behind this) or Ghost went quiet: stale.
            if !schedule.lock().unwrap().is_current(segment_id) {
                return;
            }
            debug::audio(|| format!("interim: {text:?}"));
            out.push(Frame::Interim(text)).await;
        }));
    }

    fn cancel_interim(&mut self) {
        if let Some(t) = self.interim_task.take() {
            t.abort();
        }
    }
}

impl Drop for ParakeetStt {
    fn drop(&mut self) {
        self.cancel_interim();
    }
}

#[async_trait]
impl Stage for ParakeetStt {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::AudioIn(audio) => {
                if let Some(vad) = &mut self.vad {
                    // Pipecat's rule: only Speaking and Quiet are states the pipeline sees;
                    // a dip through Stopping and back is the same segment.
                    let after = vad.analyze(&audio.samples, epoch()).unwrap_or(vad.state());
                    if !self.user_speaking && after == VadState::Speaking {
                        self.vad_started(out).await;
                    }
                    if self.user_speaking {
                        self.segment.extend_from_slice(&audio.samples);
                    } else {
                        self.pre_roll.extend(audio.samples.iter().copied());
                        let keep = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
                        while self.pre_roll.len() > keep {
                            self.pre_roll.pop_front();
                        }
                    }
                    if self.user_speaking && after == VadState::Quiet {
                        out.forward(frame.clone(), direction).await;
                        self.vad_stopped(out).await;
                        return;
                    }
                } else if self.user_speaking {
                    self.segment.extend_from_slice(&audio.samples);
                } else {
                    self.pre_roll.extend(audio.samples.iter().copied());
                    let keep = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
                    while self.pre_roll.len() > keep {
                        self.pre_roll.pop_front();
                    }
                }
                self.maybe_interim(out);
            }
            // VAD frames from upstream (a transport with its own VAD, or a test).
            Frame::VadStarted { .. } if self.vad.is_none() => {
                self.schedule.lock().unwrap().vad_started();
                self.user_speaking = true;
                self.segment = self.pre_roll.iter().copied().collect();
                out.forward(frame, direction).await;
                return;
            }
            Frame::VadStopped { .. } if self.vad.is_none() => {
                self.schedule.lock().unwrap().vad_stopped();
                self.user_speaking = false;
                out.forward(frame, direction).await;
                self.final_(out).await;
                return;
            }
            Frame::BotStarted => self.schedule.lock().unwrap().bot_started(),
            // an interruption silences Ghost before BotStopped
            Frame::BotStopped | Frame::Interruption => self.schedule.lock().unwrap().bot_stopped(),
            Frame::End | Frame::Cancel => self.cancel_interim(),
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.cancel_interim();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::confirmation::{ConfirmationProcessor, SessionGate};
    use crate::echo::EchoFilter;
    use crate::frames::{Audio, drain};
    use crate::hold::{HoldGate, OnResume};
    use crate::intercept::{InterceptorProcessor, no_action};
    use crate::tap::{UserTap, no_phase};
    use ghost_core::gate::Classification;
    use ghost_core::source::{Handled, TurnInterceptor, UserTurn};
    use ghost_core::{Outcome, Verdict};
    use tokio::sync::mpsc::Receiver;

    const RATE: u32 = 16000;

    /// `recognize` with a delay, counting calls that overlap.
    struct FakeModel {
        text: String,
        delay: Duration,
        calls: Arc<Mutex<(usize, usize, usize)>>, // calls, active, most at once
    }

    impl Recognizer for FakeModel {
        fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
            {
                let mut c = self.calls.lock().unwrap();
                c.0 += 1;
                c.1 += 1;
                c.2 = c.2.max(c.1);
            }
            std::thread::sleep(self.delay);
            self.calls.lock().unwrap().1 -= 1;
            Ok(self.text.clone())
        }
    }

    struct Model {
        shared: SharedRecognizer,
        calls: Arc<Mutex<(usize, usize, usize)>>,
    }

    fn model(text: &str, delay: f64) -> Model {
        let calls = Arc::new(Mutex::new((0, 0, 0)));
        let shared: SharedRecognizer = Arc::new(Mutex::new(FakeModel {
            text: text.into(),
            delay: Duration::from_secs_f64(delay),
            calls: calls.clone(),
        }));
        Model { shared, calls }
    }

    impl Model {
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().0
        }
        fn most_at_once(&self) -> usize {
            self.calls.lock().unwrap().2
        }
        fn fork(&self, interim_secs: f64) -> ParakeetStt {
            ParakeetStt::new(
                self.shared.clone(),
                None,
                VadParams::default(),
                interim_secs,
            )
        }
    }

    enum Step {
        F(Frame),
        Sleep(f64),
    }

    fn audio(secs: f64) -> Vec<Step> {
        let mut out = Vec::new();
        for _ in 0..((secs / 0.02).round() as usize) {
            out.push(Step::F(Frame::AudioIn(Audio::new(vec![0; 320], RATE))));
            out.push(Step::Sleep(0.02));
        }
        out
    }

    fn vad_start() -> Step {
        Step::F(Frame::VadStarted { start_secs: 0.2 })
    }

    fn vad_stop() -> Step {
        Step::F(Frame::VadStopped { stop_secs: 0.2 })
    }

    fn bot_started() -> Step {
        Step::F(Frame::BotStarted)
    }

    fn bot_stopped() -> Step {
        Step::F(Frame::BotStopped)
    }

    fn seq(parts: Vec<Vec<Step>>) -> Vec<Step> {
        parts.into_iter().flatten().collect()
    }

    async fn run(mut stt: ParakeetStt, steps: Vec<Step>) -> Vec<Frame> {
        let (out, mut rx) = Out::capture();
        for s in steps {
            match s {
                Step::F(f) => stt.process(f, Direction::Down, &out).await,
                Step::Sleep(secs) => tokio::time::sleep(Duration::from_secs_f64(secs)).await,
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        drain(&mut rx).await
    }

    fn interims(down: &[Frame]) -> Vec<String> {
        down.iter()
            .filter_map(|f| match f {
                Frame::Interim(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn finals(down: &[Frame]) -> Vec<String> {
        down.iter()
            .filter_map(|f| match f {
                Frame::Transcript(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    // --- when they are made -------------------------------------------------------

    #[tokio::test]
    async fn test_interims_while_ghost_speaks_and_the_operator_is_mid_sentence() {
        let m = model("stop that", 0.0);
        let down = run(
            m.fork(0.05),
            seq(vec![
                vec![bot_started(), vad_start()],
                audio(0.3),
                vec![vad_stop()],
            ]),
        )
        .await;
        assert!(interims(&down).len() >= 3, "{:?}", interims(&down));
        assert!(interims(&down).iter().all(|t| t == "stop that"));
        assert_eq!(finals(&down), vec!["stop that"]); // the final still comes, once, at the pause
    }

    #[tokio::test]
    async fn test_no_interims_outside_ghost_speaking_over_an_open_segment() {
        let cases: Vec<(&str, Vec<Step>)> = vec![
            // Ghost is quiet: an ordinary turn, which waits for its final
            (
                "ghost-quiet",
                seq(vec![vec![vad_start()], audio(0.3), vec![vad_stop()]]),
            ),
            // Ghost speaks, the operator doesn't
            ("operator-quiet", seq(vec![vec![bot_started()], audio(0.3)])),
            // the operator's segment ended: nothing more until the next one opens
            (
                "segment-closed",
                seq(vec![
                    vec![bot_started(), vad_start(), vad_stop()],
                    audio(0.3),
                ]),
            ),
            // Ghost stopped (or was interrupted) while the operator goes on
            (
                "ghost-stopped",
                seq(vec![
                    vec![bot_started(), vad_start(), bot_stopped()],
                    audio(0.3),
                ]),
            ),
        ];
        for (name, steps) in cases {
            let m = model("stop that", 0.0);
            let down = run(m.fork(0.05), steps).await;
            assert!(interims(&down).is_empty(), "{name}");
            assert_eq!(m.calls(), finals(&down).len(), "{name}"); // and no pass was spent on one
        }
    }

    #[tokio::test]
    async fn test_off_at_zero_and_for_the_wake_listener() {
        // ghostd builds the passive listener with the default stage, which never makes an
        // interim, whatever the frames say.
        for interim_secs in [0.0, 0.0] {
            let m = model("stop that", 0.0);
            let down = run(
                m.fork(interim_secs),
                seq(vec![vec![bot_started(), vad_start()], audio(0.3)]),
            )
            .await;
            assert!(interims(&down).is_empty());
            assert_eq!(m.calls(), 0);
        }
    }

    // --- one recognition at a time ---------------------------------------------------

    #[tokio::test]
    async fn test_recognitions_never_overlap_and_busy_ticks_are_skipped() {
        let m = model("stop that", 0.1); // slower than the interval
        let down = run(
            m.fork(0.03),
            seq(vec![
                vec![bot_started(), vad_start()],
                audio(0.5),
                vec![vad_stop(), Step::Sleep(0.2)],
            ]),
        )
        .await;
        assert_eq!(m.most_at_once(), 1);
        let n = interims(&down).len();
        assert!((2..=6).contains(&n), "{n}"); // not one per 0.03 s tick
        assert_eq!(finals(&down), vec!["stop that"]);
    }

    #[tokio::test]
    async fn test_an_interim_in_flight_at_the_pause_is_dropped_and_the_final_waits() {
        for goes_on in [false, true] {
            let m = model("stop that", 0.2);
            // the first interim starts 0.1 s in and is still running when the segment
            // ends; the operator may already be in the next one when it returns, with the
            // old one's words
            let then = if goes_on {
                seq(vec![vec![vad_start()], audio(0.1)])
            } else {
                Vec::new()
            };
            let down = run(
                m.fork(0.1),
                seq(vec![
                    vec![bot_started(), vad_start()],
                    audio(0.14),
                    vec![vad_stop()],
                    then,
                    vec![Step::Sleep(0.5)],
                ]),
            )
            .await;
            assert!(interims(&down).is_empty(), "goes_on={goes_on}"); // stale: the final is the word
            assert_eq!(finals(&down), vec!["stop that"]);
            assert_eq!(m.calls(), 2, "goes_on={goes_on}");
            assert_eq!(m.most_at_once(), 1);
        }
    }

    #[test]
    fn test_forks_share_one_model_and_one_turn_at_it() {
        let m = model("stop that", 0.0);
        let (session, wake) = (m.fork(0.3), m.fork(0.0));
        assert!(Arc::ptr_eq(session.model(), wake.model()));
        let pcm = vec![0.0f32; 1600];
        {
            let _busy = wake.model().lock().unwrap(); // another pipeline's final is running
            assert_eq!(recognize(session.model(), &pcm, false), None);
        }
        assert_eq!(m.calls(), 0);
        assert_eq!(
            recognize(session.model(), &pcm, false).as_deref(),
            Some("stop that")
        );
    }

    // --- only barge-in reads them -------------------------------------------------------

    fn interim(text: &str) -> Frame {
        Frame::Interim(text.into())
    }

    struct Catcher(Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl TurnInterceptor for Catcher {
        fn name(&self) -> &str {
            "always"
        }
        async fn on_user_turn(&self, turn: &UserTurn) -> Option<Handled> {
            self.0.lock().unwrap().push(turn.text.clone());
            None
        }
    }

    struct Gate(Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl SessionGate for Gate {
        fn pending(&self, _s: &str) -> bool {
            true // a change waits for a yes
        }
        fn cancel(&self, _s: &str, _r: &str) {}
        fn set_aside(&self, _s: &str, _r: &str) {}
        async fn on_user_turn(&self, _s: &str, text: &str, _q: &str) -> (Verdict, Option<Outcome>) {
            self.0.lock().unwrap().push(text.to_string());
            (Verdict::Unclear, None)
        }
        fn last_classification(&self) -> Option<Classification> {
            None
        }
    }

    async fn interims_of(rx: &mut Receiver<crate::frames::Pushed>) -> Vec<String> {
        interims(&drain(rx).await)
    }

    #[tokio::test]
    async fn test_interims_reach_no_transcript_store_interceptor_or_confirmation() {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let t = turns.clone();
        let on_turn: crate::tap::OnTurn = Arc::new(move |role, text| {
            let t = t.clone();
            Box::pin(async move {
                t.lock().unwrap().push((role, text));
            })
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let judged = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Gate(judged.clone()));
        let mut tap = UserTap::new(on_turn, no_phase(), None);
        let mut intercept = InterceptorProcessor::new(
            vec![Arc::new(Catcher(seen.clone()))],
            gate.clone(),
            "s",
            no_action(),
        );
        let mut confirm = ConfirmationProcessor::new(gate, "s", Arc::new(String::new));
        let stages: Vec<&mut dyn Stage> = vec![&mut tap, &mut intercept, &mut confirm];
        for stage in stages {
            let (out, mut rx) = Out::capture();
            // Ghost is quiet: the moment a final would be taken as a turn or a yes
            stage
                .process(interim("yes close it"), Direction::Down, &out)
                .await;
            assert_eq!(interims_of(&mut rx).await, vec!["yes close it"]); // passed on, for the turn strategy below
        }
        assert!(turns.lock().unwrap().is_empty());
        assert!(seen.lock().unwrap().is_empty());
        assert!(judged.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_hold_drops_interims() {
        let on_resume: OnResume = Arc::new(|_rest| {
            Box::pin(async {
                panic!("an interim never resumes a hold");
            })
        });
        let mut hold = HoldGate::new(on_resume, None, 0.0);
        let (out, mut rx) = Out::capture();
        hold.hold();
        hold.process(interim("okay ghost we're back"), Direction::Down, &out)
            .await;
        assert!(drain(&mut rx).await.is_empty());
    }

    #[tokio::test]
    async fn test_an_interim_echo_of_two_of_ghosts_words_is_dropped() {
        let mut echo = EchoFilter::new(3.0);
        let (out, mut rx) = Out::capture();
        echo.spoke("I found three open issues in the app repository.");
        echo.process(Frame::BotStarted, Direction::Down, &out).await;
        for f in [
            interim("three issues"),
            interim("wait stop"),
            interim("stop the"),
        ] {
            echo.process(f, Direction::Down, &out).await;
        }
        echo.process(
            Frame::Transcript("three issues".into()),
            Direction::Down,
            &out,
        )
        .await;
        let kept: Vec<String> = drain(&mut rx)
            .await
            .iter()
            .filter_map(|f| match f {
                Frame::Interim(t) | Frame::Transcript(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        // a two-word final is still a short answer, judged by the usual rule
        assert_eq!(kept, vec!["wait stop", "stop the", "three issues"]);
    }
}
