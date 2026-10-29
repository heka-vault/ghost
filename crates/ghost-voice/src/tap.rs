//! Observe a session for the control API: phase changes and spoken turns.
//!
//! Two stages, because user text and assistant text live in different parts of the
//! pipeline. `UserTap` sits right after STT and sees every transcription (including the
//! ones an interceptor later takes). `AssistantTap` sits right before TTS and collects the
//! model's text per response, plus anything spoken directly (`Speak`). Nothing here touches
//! passive audio: both only exist inside a session.

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
}

impl AssistantTap {
    pub fn new(on_turn: OnTurn, on_marker: Option<OnMarker>) -> Self {
        Self {
            on_turn,
            on_marker,
            buf: Vec::new(),
        }
    }

    async fn flush(&mut self) {
        let text = std::mem::take(&mut self.buf).concat();
        let text = text.trim().to_string();
        if !text.is_empty() {
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
                self.flush().await;
                if let Some(on_marker) = &self.on_marker {
                    on_marker(slug);
                }
            }
            Frame::LlmText(text) => self.buf.push(text.clone()),
            Frame::LlmEnd => self.flush().await,
            Frame::Speak(text) if !text.trim().is_empty() => {
                (self.on_turn)("assistant", text.clone()).await;
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
