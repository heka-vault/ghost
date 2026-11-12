//! Observe a session for the control API: phase changes and spoken turns.
//!
//! Two stages, because user text and assistant text live in different parts of the
//! pipeline. `UserTap` sits right after STT and sees every transcription (including the
//! ones an interceptor later takes). `AssistantTap` sits right before TTS and collects the
//! model's text per response, plus anything spoken directly (`Speak`). Nothing here touches
//! passive audio: both only exist inside a session.
//!
//! A reply is recorded once Ghost's voice has started on it, not when the model finished
//! writing it: a reply the operator went on over before any of it was heard (the turn is
//! reopened, `turns::TurnControl`) was never said, and with a short grace (ADR 0049) that
//! happens often enough that the record would fill with answers nobody heard.

use std::sync::Arc;

use async_trait::async_trait;
use ghost_core::source::BoxFuture;

use crate::frames::{Direction, Frame, Out, Stage};

/// Where a session is, as the apps show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Listening,
    Thinking,
    Speaking,
    /// Checked out (ADR 0021).
    Hold,
}

impl SessionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionPhase::Listening => "listening",
            SessionPhase::Thinking => "thinking",
            SessionPhase::Speaking => "speaking",
            SessionPhase::Hold => "hold",
        }
    }
}

/// `(role, text)`: a spoken turn, user or assistant.
pub type OnTurn = Arc<dyn Fn(&'static str, String) -> BoxFuture<'static, ()> + Send + Sync>;
pub type OnPhase = Arc<dyn Fn(SessionPhase) -> BoxFuture<'static, ()> + Send + Sync>;
pub type OnMarker = Arc<dyn Fn(&str) + Send + Sync>;

pub fn no_turn() -> OnTurn {
    Arc::new(|_, _| Box::pin(async {}))
}

pub fn no_phase() -> OnPhase {
    Arc::new(|_| Box::pin(async {}))
}

pub struct UserTap {
    on_turn: OnTurn,
    on_phase: OnPhase,
    // On hold (ADR 0021) the end of Ghost's "take your time" is not "listening".
    held: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl UserTap {
    pub fn new(
        on_turn: OnTurn,
        on_phase: OnPhase,
        held: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Self {
        Self {
            on_turn,
            on_phase,
            held: held.unwrap_or_else(|| Arc::new(|| false)),
        }
    }
}

#[async_trait]
impl Stage for UserTap {
    fn name(&self) -> &'static str {
        "user_tap"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::Transcript(text) if !text.trim().is_empty() => {
                (self.on_turn)("user", text.clone()).await;
            }
            Frame::UserStopped if !(self.held)() => {
                // The turn is over (after the grace hold), so the model is about to run. A
                // transcript on its own isn't that: sentences arrive in pieces while the
                // operator is still talking, and the phone showed Thinking mid-sentence.
                // On hold the room's voices open turns with no words in them; not thinking.
                (self.on_phase)(SessionPhase::Thinking).await;
            }
            Frame::BotStarted => (self.on_phase)(SessionPhase::Speaking).await,
            Frame::BotStopped => {
                let phase = if (self.held)() {
                    SessionPhase::Hold
                } else {
                    SessionPhase::Listening
                };
                (self.on_phase)(phase).await;
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }
}

pub struct AssistantTap {
    on_turn: OnTurn,
    on_marker: Option<OnMarker>,
    buf: Vec<String>,
    /// A reply the model finished that has not reached the speaker yet.
    unheard: Option<String>,
    /// Ghost's voice is at the speaker.
    speaking: bool,
}

impl AssistantTap {
    pub fn new(on_turn: OnTurn, on_marker: Option<OnMarker>) -> Self {
        Self {
            on_turn,
            on_marker,
            buf: Vec::new(),
            unheard: None,
            speaking: false,
        }
    }

    /// The reply the model finished is the record's.
    async fn flush(&mut self) {
        self.finish();
        self.heard().await;
    }

    /// The model finished a reply: it waits for Ghost's voice.
    fn finish(&mut self) {
        let text = std::mem::take(&mut self.buf).concat();
        let text = text.trim().to_string();
        if !text.is_empty() {
            self.unheard = Some(match self.unheard.take() {
                Some(before) => format!("{before} {text}"),
                None => text,
            });
        }
    }

    /// Ghost's voice is on the reply (or nothing will ever say otherwise): record it.
    async fn heard(&mut self) {
        if let Some(text) = self.unheard.take() {
            (self.on_turn)("assistant", text).await;
        }
    }
}

#[async_trait]
impl Stage for AssistantTap {
    fn name(&self) -> &'static str {
        "assistant_tap"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::Marker(slug) => {
                // Text generated so far was the old agent's; flush it before moving on.
                // A transfer is announced aloud, so what was written is recorded now.
                self.flush().await;
                if let Some(on_marker) = &self.on_marker {
                    on_marker(slug);
                }
            }
            Frame::LlmText(text) => self.buf.push(text.clone()),
            Frame::LlmEnd => {
                self.finish();
                if self.speaking {
                    self.heard().await; // a long reply, already being spoken
                }
            }
            // The next reply, or the session's end: a reply that made no sound (nothing in
            // it to say aloud) was still the reply.
            Frame::LlmStart | Frame::End => self.heard().await,
            Frame::BotStarted => {
                self.speaking = true;
                self.heard().await;
            }
            Frame::BotStopped => self.speaking = false,
            // The operator spoke before any of it was heard: it was never said.
            Frame::Interruption => self.unheard = None,
            Frame::Speak(text) if !text.trim().is_empty() => {
                (self.on_turn)("assistant", text.clone()).await;
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        self.heard().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn recorder() -> (OnTurn, Arc<Mutex<Vec<String>>>) {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let t = turns.clone();
        let on_turn: OnTurn = Arc::new(move |role, text| {
            let t = t.clone();
            Box::pin(async move {
                t.lock().unwrap().push(format!("{role}:{text}"));
            })
        });
        (on_turn, turns)
    }

    async fn feed(tap: &mut AssistantTap, frames: Vec<(Frame, Direction)>) {
        let (out, _rx) = Out::capture();
        for (frame, direction) in frames {
            tap.process(frame, direction, &out).await;
        }
    }

    fn reply(text: &str) -> Vec<(Frame, Direction)> {
        vec![
            (Frame::LlmStart, Direction::Down),
            (Frame::LlmText(text.into()), Direction::Down),
            (Frame::LlmEnd, Direction::Down),
        ]
    }

    #[tokio::test]
    async fn test_a_reply_is_recorded_when_ghosts_voice_starts_on_it() {
        let (on_turn, turns) = recorder();
        let mut tap = AssistantTap::new(on_turn, None);
        feed(&mut tap, reply("Three are waiting.")).await;
        assert!(turns.lock().unwrap().is_empty(), "written, not yet said");
        feed(&mut tap, vec![(Frame::BotStarted, Direction::Up)]).await;
        assert_eq!(*turns.lock().unwrap(), vec!["assistant:Three are waiting."]);
        // A long reply still being written while it is spoken is recorded at its end.
        feed(&mut tap, reply("And the oldest is from Monday.")).await;
        assert_eq!(turns.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_a_reply_the_operator_went_on_over_before_it_was_heard_is_not_recorded() {
        // The turn is reopened (`turns::TurnControl`): the half turn's answer was never
        // said, and the whole turn's answer is the one the record keeps.
        let (on_turn, turns) = recorder();
        let mut tap = AssistantTap::new(on_turn, None);
        feed(&mut tap, reply("Three are waiting.")).await;
        feed(&mut tap, vec![(Frame::Interruption, Direction::Down)]).await;
        feed(&mut tap, reply("Three are waiting in the app repo.")).await;
        feed(&mut tap, vec![(Frame::BotStarted, Direction::Up)]).await;
        assert_eq!(
            *turns.lock().unwrap(),
            vec!["assistant:Three are waiting in the app repo."]
        );
    }

    #[tokio::test]
    async fn test_a_reply_that_made_no_sound_is_recorded_by_the_next_one_or_the_end() {
        let (on_turn, turns) = recorder();
        let mut tap = AssistantTap::new(on_turn, None);
        feed(&mut tap, reply("**")).await; // nothing in it to say aloud
        feed(&mut tap, reply("Done.")).await;
        assert_eq!(*turns.lock().unwrap(), vec!["assistant:**"]);
        tap.stopped().await;
        assert_eq!(
            *turns.lock().unwrap(),
            vec!["assistant:**", "assistant:Done."]
        );
    }

    #[tokio::test]
    async fn test_fragments_record_turns_but_thinking_waits_for_the_turn_end() {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let phases = Arc::new(Mutex::new(Vec::new()));
        let t = turns.clone();
        let on_turn: OnTurn = Arc::new(move |role, text| {
            let t = t.clone();
            Box::pin(async move {
                t.lock().unwrap().push((role, text));
            })
        });
        let p = phases.clone();
        let on_phase: OnPhase = Arc::new(move |phase| {
            let p = p.clone();
            Box::pin(async move {
                p.lock().unwrap().push(phase);
            })
        });
        let mut tap = UserTap::new(on_turn, on_phase, None);
        let (out, _rx) = Out::capture();
        for text in ["Yes, I'm going to do it.", "Setting should follow.", "The"] {
            tap.process(Frame::Transcript(text.into()), Direction::Down, &out)
                .await;
        }
        let roles: Vec<&str> = turns.lock().unwrap().iter().map(|(r, _)| *r).collect();
        assert_eq!(roles, vec!["user", "user", "user"]);
        assert!(phases.lock().unwrap().is_empty()); // still the operator's turn
        tap.process(Frame::UserStopped, Direction::Up, &out).await;
        assert_eq!(*phases.lock().unwrap(), vec![SessionPhase::Thinking]);
        tap.process(Frame::BotStarted, Direction::Up, &out).await;
        tap.process(Frame::BotStopped, Direction::Up, &out).await;
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                SessionPhase::Thinking,
                SessionPhase::Speaking,
                SessionPhase::Listening
            ]
        );
    }
}
