//! Plays a soft loop while Ghost is thinking.
//!
//! "Thinking" is any gap where the model or a tool is working and nothing is being spoken:
//! after the operator's turn ends, while a tool call runs, and during the follow-up
//! inference after a tool result. The sound comes from the playback's mixer
//! (`ghost_audio::MixerHandle`); this stage only switches it on and off with
//! `MixerEnable`. A short delay avoids a blip on fast replies.
//!
//! It sits between the LLM and TTS. From there it sees, going downstream, the user-speaking
//! frames, the LLM's response and function-call frames, and `ThinkingStart` (queued by the
//! session for turns that did not come through the microphone); going upstream, the
//! transport's bot-speaking frames.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};

#[derive(Default)]
struct State {
    on: bool,
    busy: bool,
    bot_speaking: bool,
    calls_in_progress: usize,
}

pub type Held = Arc<dyn Fn() -> bool + Send + Sync>;

/// Sound is on when the model or a tool is busy and Ghost is not speaking.
pub struct ThinkingSoundProcessor {
    delay: Duration,
    held: Held, // on hold nothing is being waited for
    state: Arc<Mutex<State>>,
    timer: Option<JoinHandle<()>>,
}

impl ThinkingSoundProcessor {
    pub fn new(delay_secs: f64, held: Option<Held>) -> Self {
        Self {
            delay: Duration::from_secs_f64(delay_secs.max(0.0)),
            held: held.unwrap_or_else(|| Arc::new(|| false)),
            state: Arc::new(Mutex::new(State::default())),
            timer: None,
        }
    }

    pub fn playing(&self) -> bool {
        self.state.lock().unwrap().on
    }

    async fn reconcile(&mut self, out: &Out) {
        let want = {
            let s = self.state.lock().unwrap();
            s.busy && !s.bot_speaking && !(self.held)()
        };
        let on = self.state.lock().unwrap().on;
        if want && !on {
            if self.timer.as_ref().is_none_or(JoinHandle::is_finished) {
                let state = self.state.clone();
                let out = out.clone();
                let delay = self.delay;
                self.timer = Some(tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let turn_on = {
                        let mut s = state.lock().unwrap();
                        if s.busy && !s.bot_speaking && !s.on {
                            s.on = true;
                            true
                        } else {
                            false
                        }
                    };
                    if turn_on {
                        debug::audio(|| "thinking sound on".into());
                        out.push(Frame::MixerEnable(true)).await;
                    }
                }));
            }
        } else if !want {
            self.cancel_timer();
            let turn_off = {
                let mut s = self.state.lock().unwrap();
                if s.on {
                    s.on = false;
                    true
                } else {
                    false
                }
            };
            if turn_off {
                debug::audio(|| "thinking sound off".into());
                out.push(Frame::MixerEnable(false)).await;
            }
        }
    }

    fn cancel_timer(&mut self) {
        if let Some(t) = self.timer.take() {
            t.abort();
        }
    }
}

impl Drop for ThinkingSoundProcessor {
    fn drop(&mut self) {
        self.cancel_timer();
    }
}

#[async_trait]
impl Stage for ThinkingSoundProcessor {
    fn name(&self) -> &'static str {
        "thinking"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        {
            let mut s = self.state.lock().unwrap();
            match &frame {
                Frame::UserStopped | Frame::ThinkingStart => s.busy = true,
                Frame::ThinkingStop => s.busy = false,
                Frame::FunctionCallInProgress { name, .. } => {
                    s.calls_in_progress += 1;
                    s.busy = true;
                    debug::audio(|| format!("thinking: tool {name} started"));
                }
                Frame::FunctionCallResult { name, .. } => {
                    s.calls_in_progress = s.calls_in_progress.saturating_sub(1);
                    s.busy = true; // the model runs again on the result
                    debug::audio(|| format!("thinking: tool {name} returned"));
                }
                Frame::LlmEnd => {
                    // Text-only response finished: TTS is about to speak. If a tool is
                    // still running the model is not done.
                    if s.calls_in_progress == 0 {
                        s.busy = false;
                    }
                }
                Frame::BotStarted => s.bot_speaking = true,
                Frame::BotStopped => s.bot_speaking = false,
                Frame::UserStarted => s.busy = false, // interruption: the operator has the floor
                Frame::End | Frame::Cancel => s.busy = false,
                _ => {}
            }
        }
        self.reconcile(out).await;
        if matches!(frame, Frame::ThinkingStart | Frame::ThinkingStop) {
            return;
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.cancel_timer();
    }
}

/// Sits directly before the output transport and switches the mixer off the moment TTS
/// starts producing speech.
///
/// The `ThinkingSoundProcessor` upstream also sends an off, but only when playback has
/// started, and that frame then queues behind whatever the TTS service is synthesising;
/// the loop used to run about half a second into Ghost's reply.
#[derive(Debug, Default)]
pub struct ThinkingSoundCut;

#[async_trait]
impl Stage for ThinkingSoundCut {
    fn name(&self) -> &'static str {
        "thinking_cut"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if matches!(frame, Frame::TtsStarted) && direction == Direction::Down {
            debug::audio(|| "thinking sound cut: speech starting".into());
            out.push(Frame::MixerEnable(false)).await;
        }
        out.forward(frame, direction).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Pushed;
    use crate::frames::drain;
    use tokio::sync::mpsc::Receiver;

    fn processor(delay: f64) -> (ThinkingSoundProcessor, Out, Receiver<Pushed>) {
        let (out, rx) = Out::capture();
        (ThinkingSoundProcessor::new(delay, None), out, rx)
    }

    async fn mixer_frames(rx: &mut Receiver<Pushed>, seen: &mut Vec<bool>) -> Vec<bool> {
        for f in drain(rx).await {
            if let Frame::MixerEnable(on) = f {
                seen.push(on);
            }
        }
        seen.clone()
    }

    async fn sleep(secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    #[tokio::test]
    async fn test_sound_starts_after_delay_and_stops_when_bot_speaks() {
        let (mut p, out, mut rx) = processor(0.05);
        let mut seen = Vec::new();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, Vec::<bool>::new());
        sleep(0.1).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true]);
        p.process(Frame::BotStarted, Direction::Up, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true, false]);
    }

    #[tokio::test]
    async fn test_fast_reply_never_starts_sound() {
        let (mut p, out, mut rx) = processor(0.2);
        let mut seen = Vec::new();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        p.process(Frame::BotStarted, Direction::Up, &out).await;
        sleep(0.3).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, Vec::<bool>::new());
    }

    #[tokio::test]
    async fn test_silent_llm_turn_stops_sound() {
        let (mut p, out, mut rx) = processor(0.05);
        let mut seen = Vec::new();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        sleep(0.1).await;
        p.process(Frame::LlmEnd, Direction::Down, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true, false]);
    }

    #[test]
    fn test_loop_is_quiet_and_seamless() {
        let a = ghost_audio::thinking_loop(24000, 1.6);
        assert_eq!(a.len(), (24000.0 * 1.6) as usize);
        assert!(a[0].abs() < 50 && a[a.len() - 1].abs() < 50); // starts and ends near silence
        let peak = a.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(2000 < peak && peak < 20000);
    }

    #[tokio::test]
    async fn test_injected_turn_also_arms_sound_and_frame_is_swallowed() {
        let (mut p, out, mut rx) = processor(0.05);
        p.process(Frame::ThinkingStart, Direction::Down, &out).await;
        sleep(0.1).await;
        let pushed = drain(&mut rx).await;
        let mixer: Vec<bool> = pushed
            .iter()
            .filter_map(|f| match f {
                Frame::MixerEnable(on) => Some(*on),
                _ => None,
            })
            .collect();
        assert_eq!(mixer, vec![true]);
        assert!(!pushed.iter().any(|f| matches!(f, Frame::ThinkingStart)));
    }

    #[tokio::test]
    async fn test_tool_call_keeps_sound_through_response_end() {
        let (mut p, out, mut rx) = processor(0.05);
        let mut seen = Vec::new();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        sleep(0.1).await;
        p.process(
            Frame::FunctionCallInProgress {
                name: "f".into(),
                id: "t1".into(),
            },
            Direction::Down,
            &out,
        )
        .await;
        p.process(Frame::LlmEnd, Direction::Down, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true]); // still on: a tool is running
        p.process(
            Frame::FunctionCallResult {
                name: "f".into(),
                id: "t1".into(),
            },
            Direction::Down,
            &out,
        )
        .await;
        p.process(Frame::BotStarted, Direction::Up, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true, false]);
    }

    #[tokio::test]
    async fn test_sound_resumes_after_interim_sentence_while_tool_runs() {
        // 'Let me check' is spoken while the tool runs; the tone pauses for it and resumes.
        let (mut p, out, mut rx) = processor(0.05);
        let mut seen = Vec::new();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        sleep(0.1).await;
        p.process(
            Frame::FunctionCallInProgress {
                name: "f".into(),
                id: "t1".into(),
            },
            Direction::Down,
            &out,
        )
        .await;
        p.process(Frame::BotStarted, Direction::Up, &out).await;
        assert_eq!(mixer_frames(&mut rx, &mut seen).await, vec![true, false]);
        p.process(Frame::BotStopped, Direction::Up, &out).await;
        sleep(0.1).await;
        assert_eq!(
            mixer_frames(&mut rx, &mut seen).await,
            vec![true, false, true]
        );
    }

    #[tokio::test]
    async fn test_cut_switches_mixer_off_as_speech_starts() {
        // The cut sits before the transport: TtsStarted is immediately preceded by a
        // mixer-off, so no queued speech plays over the loop.
        let mut cut = ThinkingSoundCut;
        let (out, mut rx) = Out::capture();
        cut.process(Frame::TtsStarted, Direction::Down, &out).await;
        cut.process(Frame::TtsStopped, Direction::Down, &out).await;
        let kinds: Vec<String> = drain(&mut rx)
            .await
            .iter()
            .map(|f| match f {
                Frame::MixerEnable(on) => format!("MixerEnable({on})"),
                f => f.kind().to_string(),
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["MixerEnable(false)", "TtsStarted", "TtsStopped"]
        );
    }

    #[tokio::test]
    async fn test_stop_frame_ends_the_wait_after_a_tool_call_with_no_reply() {
        // hold (ADR 0021) runs no response after the result, so the session sends a stop;
        // without it the loop would play on.
        let (mut p, out, mut rx) = processor(0.05);
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        p.process(
            Frame::FunctionCallInProgress {
                name: "hold".into(),
                id: "t".into(),
            },
            Direction::Down,
            &out,
        )
        .await;
        p.process(
            Frame::FunctionCallResult {
                name: "hold".into(),
                id: "t".into(),
            },
            Direction::Down,
            &out,
        )
        .await;
        sleep(0.08).await;
        assert!(p.playing());
        p.process(Frame::ThinkingStop, Direction::Down, &out).await;
        assert!(!p.playing());
        assert!(
            !drain(&mut rx)
                .await
                .iter()
                .any(|f| matches!(f, Frame::ThinkingStop))
        ); // swallowed
    }

    #[tokio::test]
    async fn test_no_sound_while_held() {
        let mut p = ThinkingSoundProcessor::new(0.02, Some(Arc::new(|| true)));
        let (out, _rx) = Out::capture();
        p.process(Frame::UserStopped, Direction::Down, &out).await;
        sleep(0.06).await;
        assert!(!p.playing());
    }
}
