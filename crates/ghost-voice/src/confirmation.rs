//! Feeds the operator's transcribed turns to the write gate.
//!
//! Sits between STT and the user aggregator. Every final transcript it sees is the
//! pipeline's own evidence of what the operator said (ADR 0007). When a proposal is
//! pending (a `confirm`-level operation, ADR 0011), the gate's classifier decides; on an
//! affirmative the gate executes and the verified outcome is placed in the LLM context as
//! a system note so the model speaks the real result. Ghost's last reply (read from the LLM
//! context, since TTS text never travels upstream) is passed along as the question being
//! answered, so a classifier can read "yeah" in context.
//!
//! A transcript that arrives while Ghost is still talking is not confirmation evidence:
//! the operator has not heard the whole readback yet, and before echo cancellation it is
//! most likely Ghost's own voice. Such frames pass through untouched and the proposal stays
//! pending until a turn after the readback ends.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ghost_core::gate::Classification;
use ghost_core::source::EventKind;
use ghost_core::{Events, Outcome, OutcomeStatus, Verdict, WriteGate};
use serde_json::{Map, json};
use tracing::{info, warn};

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::latency::{LatencyProbe, null_probe};
use crate::speaking::Speaking;

/// What the pipeline asks of the gate inside a session: whether a change waits for a yes,
/// the operator's transcribed turn as the answer, and what to do with a change when the
/// operator cancels or leaves. `WriteGate` is the one; tests use a fake.
#[async_trait]
pub trait SessionGate: Send + Sync {
    fn pending(&self, session_id: &str) -> bool;
    fn cancel(&self, session_id: &str, reason: &str);
    fn set_aside(&self, session_id: &str, reason: &str);
    async fn on_user_turn(
        &self,
        session_id: &str,
        transcript: &str,
        question: &str,
    ) -> (Verdict, Option<Outcome>);
    fn last_classification(&self) -> Option<Classification>;
}

#[async_trait]
impl SessionGate for WriteGate {
    fn pending(&self, session_id: &str) -> bool {
        matches!(WriteGate::pending(self, session_id, None), Ok(Some(_)))
    }

    fn cancel(&self, session_id: &str, reason: &str) {
        if let Err(e) = WriteGate::cancel(self, session_id, reason) {
            warn!("gate: cancel failed: {e}");
        }
    }

    fn set_aside(&self, session_id: &str, reason: &str) {
        if let Err(e) = WriteGate::set_aside(self, session_id, reason) {
            warn!("gate: set aside failed: {e}");
        }
    }

    async fn on_user_turn(
        &self,
        session_id: &str,
        transcript: &str,
        question: &str,
    ) -> (Verdict, Option<Outcome>) {
        match WriteGate::on_user_turn(self, session_id, transcript, None, question).await {
            Ok(v) => v,
            Err(e) => {
                warn!("gate: the operator's turn could not be judged: {e}");
                (Verdict::Unclear, None)
            }
        }
    }

    fn last_classification(&self) -> Option<Classification> {
        WriteGate::last_classification(self)
    }
}

pub type LastAssistant = Arc<dyn Fn() -> String + Send + Sync>;

/// `OutcomeStatus` as the audit and the events spell it.
pub fn status_str(status: OutcomeStatus) -> &'static str {
    match status {
        OutcomeStatus::Success => "success",
        OutcomeStatus::Partial => "partial",
        OutcomeStatus::Failed => "failed",
    }
}

pub struct ConfirmationProcessor {
    gate: Arc<dyn SessionGate>,
    session_id: String,
    bot_speaking: bool,
    speaking: Speaking,
    last_assistant: LastAssistant,
    events: Arc<Events>,
    probe: Arc<dyn LatencyProbe>,
}

impl ConfirmationProcessor {
    pub fn new(
        gate: Arc<dyn SessionGate>,
        session_id: &str,
        last_assistant: LastAssistant,
    ) -> Self {
        Self {
            gate,
            session_id: session_id.to_string(),
            bot_speaking: false,
            speaking: Speaking::new(),
            last_assistant,
            events: Arc::new(Events::default()),
            probe: null_probe(),
        }
    }

    /// The output stage's flag (`crate::speaking`): the operator has not heard the
    /// question yet while it is on, whether or not `BotStarted` has arrived here.
    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking = speaking;
        self
    }

    pub fn with_events(mut self, events: Arc<Events>) -> Self {
        self.events = events;
        self
    }

    pub fn with_probe(mut self, probe: Arc<dyn LatencyProbe>) -> Self {
        self.probe = probe;
        self
    }

    pub fn bot_speaking(&self) -> bool {
        self.bot_speaking || self.speaking.is_on()
    }
}

#[async_trait]
impl Stage for ConfirmationProcessor {
    fn name(&self) -> &'static str {
        "confirmation"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::BotStarted => self.bot_speaking = true,
            Frame::BotStopped => self.bot_speaking = false,
            Frame::Transcript(text)
                if self.bot_speaking() && self.gate.pending(&self.session_id) =>
            {
                debug::audio(|| format!("confirmation: ignored {text:?}, Ghost still speaking"));
            }
            _ => {}
        }
        if let Frame::Transcript(text) = &frame
            && !self.bot_speaking()
            && self.gate.pending(&self.session_id)
        {
            let began = Instant::now();
            let (verdict, outcome) = self
                .gate
                .on_user_turn(&self.session_id, text, &(self.last_assistant)())
                .await;
            let took = self.gate.last_classification();
            self.probe.confirmation(
                took.as_ref().map_or("unknown", |c| c.classifier.as_str()),
                took.as_ref().map_or(0.0, |c| c.seconds),
                verdict.as_str(),
                began.elapsed().as_secs_f64(),
            );
            debug::audio(|| format!("confirmation: {} for {text:?}", verdict.as_str()));
            let note = match (verdict, outcome) {
                (Verdict::Affirmative, Some(outcome)) => {
                    info!(
                        "executed {}: {}",
                        outcome.proposal_id,
                        status_str(outcome.status)
                    );
                    let mut data = Map::new();
                    data.insert("operation".into(), json!("confirmed"));
                    data.insert("status".into(), json!(status_str(outcome.status)));
                    self.events.emit(EventKind::Outcome, data).await;
                    format!(
                        "[system] The operator said yes. Verified result: {} Say what happened in a few words.",
                        outcome.summary()
                    )
                }
                (Verdict::Negative, _) => {
                    "[system] The operator said no. Nothing was changed. Acknowledge briefly."
                        .into()
                }
                _ => "[system] The operator did not clearly say yes, so the change was not made. \
                      It waits on their waiting list; don't mention it unless they ask."
                    .into(),
            };
            out.push(Frame::user_text(note, false)).await;
        }
        out.forward(frame, direction).await;
    }
}

/// Ghost's most recent reply, as plain text, for the confirmation classifier.
pub fn last_assistant_text(context: &ghost_llm::Context) -> String {
    for m in context.messages.iter().rev() {
        if let ghost_llm::Message::Assistant { text, .. } = m {
            return text.trim().to_string();
        }
    }
    String::new()
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::Mutex;

    use super::*;

    /// A gate that records what it was asked: whether a proposal is pending, the turns it
    /// judged, what was cancelled or set aside.
    pub struct FakeGate {
        pub pending: Mutex<bool>,
        pub turns: Mutex<Vec<String>>,
        pub question: Mutex<String>,
        pub cancelled: Mutex<Vec<String>>,
        pub set_aside_for: Mutex<Vec<String>>,
        pub verdict: Mutex<Verdict>,
    }

    impl FakeGate {
        pub fn new(pending: bool) -> Arc<Self> {
            Arc::new(Self {
                pending: Mutex::new(pending),
                turns: Mutex::new(Vec::new()),
                question: Mutex::new(String::new()),
                cancelled: Mutex::new(Vec::new()),
                set_aside_for: Mutex::new(Vec::new()),
                verdict: Mutex::new(Verdict::Negative),
            })
        }
    }

    #[async_trait]
    impl SessionGate for FakeGate {
        fn pending(&self, _session_id: &str) -> bool {
            *self.pending.lock().unwrap()
        }

        fn cancel(&self, _session_id: &str, reason: &str) {
            self.cancelled.lock().unwrap().push(reason.to_string());
        }

        fn set_aside(&self, _session_id: &str, reason: &str) {
            self.set_aside_for.lock().unwrap().push(reason.to_string());
        }

        async fn on_user_turn(
            &self,
            _session_id: &str,
            transcript: &str,
            question: &str,
        ) -> (Verdict, Option<Outcome>) {
            self.turns.lock().unwrap().push(transcript.to_string());
            *self.question.lock().unwrap() = question.to_string();
            (*self.verdict.lock().unwrap(), None)
        }

        fn last_classification(&self) -> Option<Classification> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeGate;
    use super::*;
    use crate::frames::drain;
    use ghost_llm::{Context, Message};

    fn processor() -> (ConfirmationProcessor, Arc<FakeGate>) {
        let gate = FakeGate::new(true);
        let proc_ = ConfirmationProcessor::new(gate.clone(), "s1", Arc::new(|| "Close it?".into()));
        (proc_, gate)
    }

    #[tokio::test]
    async fn test_transcript_while_bot_speaking_is_not_evidence() {
        let (mut p, gate) = processor();
        let (out, mut rx) = Out::capture();
        p.process(Frame::BotStarted, Direction::Down, &out).await;
        p.process(Frame::Transcript("yes".into()), Direction::Down, &out)
            .await;
        assert!(gate.turns.lock().unwrap().is_empty());
        let pushed = drain(&mut rx).await;
        assert!(
            !pushed
                .iter()
                .any(|f| matches!(f, Frame::ContextAppend { .. }))
        );
        assert!(pushed.iter().any(|f| matches!(f, Frame::Transcript(_)))); // still passed downstream
    }

    #[tokio::test]
    async fn test_transcript_after_readback_reaches_gate() {
        let (mut p, gate) = processor();
        let (out, mut rx) = Out::capture();
        p.process(Frame::BotStarted, Direction::Down, &out).await;
        p.process(Frame::BotStopped, Direction::Down, &out).await;
        p.process(Frame::Transcript("no".into()), Direction::Down, &out)
            .await;
        assert_eq!(*gate.turns.lock().unwrap(), vec!["no".to_string()]);
        assert_eq!(*gate.question.lock().unwrap(), "Close it?");
        let pushed = drain(&mut rx).await;
        assert!(
            pushed
                .iter()
                .any(|f| matches!(f, Frame::ContextAppend { .. }))
        );
    }

    #[test]
    fn test_last_assistant_text_reads_context() {
        let mut ctx = Context::new("").user("close it");
        ctx.push(Message::assistant("Close 151?"));
        assert_eq!(last_assistant_text(&ctx), "Close 151?");
        assert_eq!(last_assistant_text(&Context::new("")), "");
    }
}
