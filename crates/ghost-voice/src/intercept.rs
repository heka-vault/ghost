//! Run turn interceptors before the model sees a user turn (ADR 0012's hook, kept for
//! Ghost's own fixed intents).
//!
//! Sits right after STT. Each final transcript spoken while Ghost is quiet is offered to
//! the interceptors in order, on its own and then joined with the earlier pieces of the
//! same turn ("I will talk to you" + "later"); the first `Handled` wins, the frame is
//! dropped, the turn's earlier pieces are taken back from the aggregator
//! (`ResetAggregation`), and the model never sees any of it. Interceptors can speak a line,
//! cancel a pending change, put the session on hold, or end it. Anything they do goes
//! through the same gate as everything else.
//!
//! While Ghost speaks, a transcript is offered only when it is a barge-in by the
//! aggregator's own rule (`GHOST_BARGE_IN_MIN_WORDS` words at least), and marked so
//! (`UserTurn::barged_in`): shorter ones are most likely Ghost's own voice past the echo
//! canceller, and a leaked "that's all" must not end the session.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ghost_core::Events;
use ghost_core::source::{Action, BoxFuture, EventKind, Handled, TurnInterceptor, UserTurn};
use serde_json::{Map, json};
use tracing::info;

use crate::confirmation::{LastAssistant, SessionGate};
use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::latency::{LatencyProbe, null_probe};
use crate::speaking::Speaking;

pub type SessionAction = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

pub fn no_action() -> SessionAction {
    Arc::new(|| Box::pin(async {}))
}

pub struct InterceptorProcessor {
    interceptors: Vec<Arc<dyn TurnInterceptor>>,
    gate: Arc<dyn SessionGate>,
    session_id: String,
    on_hang_up: SessionAction,
    on_hold: Option<SessionAction>,
    last_assistant: LastAssistant,
    probe: Arc<dyn LatencyProbe>,
    events: Arc<Events>,
    /// This turn's transcripts that went to the model.
    turn_pieces: Vec<String>,
    bot_speaking: bool,
    speaking: Speaking,
    /// The aggregator's barge-in rule, applied here to a transcript heard over Ghost.
    barge_in_min_words: usize,
    /// The turn's own state: a turn whose interim cut Ghost off is still an interruption
    /// when its final arrives after the cut (2026-10-30: "Okay, that's good. Stop." went to
    /// the model because Ghost had already stopped).
    turns: Option<Arc<crate::turns::TurnControl>>,
}

impl InterceptorProcessor {
    pub fn new(
        interceptors: Vec<Arc<dyn TurnInterceptor>>,
        gate: Arc<dyn SessionGate>,
        session_id: &str,
        on_hang_up: SessionAction,
    ) -> Self {
        Self {
            interceptors,
            gate,
            session_id: session_id.to_string(),
            on_hang_up,
            on_hold: None,
            last_assistant: Arc::new(String::new),
            probe: null_probe(),
            events: Arc::new(Events::default()),
            turn_pieces: Vec::new(),
            bot_speaking: false,
            speaking: Speaking::new(),
            barge_in_min_words: 2,
            turns: None,
        }
    }

    /// The output stage's flag (`crate::speaking`), read next to the frames.
    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking = speaking;
        self
    }

    /// GHOST_BARGE_IN_MIN_WORDS, as the aggregator has it; 0 means nothing interrupts.
    pub fn with_barge_in_min_words(mut self, words: usize) -> Self {
        self.barge_in_min_words = words;
        self
    }

    pub fn with_turns(mut self, turns: Arc<crate::turns::TurnControl>) -> Self {
        self.turns = Some(turns);
        self
    }

    fn bot_speaking(&self) -> bool {
        self.bot_speaking || self.speaking.is_on()
    }

    /// This transcript interrupts Ghost: it is speaking now, or the turn it belongs to cut
    /// it off already (an interim did, and this is the final).
    fn barged_in(&self) -> bool {
        self.bot_speaking() || self.turns.as_ref().is_some_and(|t| t.barged_in())
    }

    /// A transcript the interceptors should see: any while Ghost is quiet, a barge-in's
    /// while it speaks.
    fn offered(&self, text: &str) -> bool {
        !self.bot_speaking()
            || (self.barge_in_min_words > 0
                && text.split_whitespace().count() >= self.barge_in_min_words)
    }

    pub fn with_hold(mut self, on_hold: SessionAction) -> Self {
        self.on_hold = Some(on_hold);
        self
    }

    pub fn with_last_assistant(mut self, last: LastAssistant) -> Self {
        self.last_assistant = last;
        self
    }

    pub fn with_probe(mut self, probe: Arc<dyn LatencyProbe>) -> Self {
        self.probe = probe;
        self
    }

    pub fn with_events(mut self, events: Arc<Events>) -> Self {
        self.events = events;
        self
    }

    async fn handle(&mut self, h: Handled, out: &Out) {
        // The model is not going to run on this turn: no thinking sound for it.
        out.push(Frame::ThinkingStop).await;
        match h.action {
            Some(Action::Cancel) => self.gate.cancel(&self.session_id, "operator cancelled"),
            Some(Action::HangUp) => {
                // The change it was waiting on stays on the waiting list (ADR 0030).
                self.gate.set_aside(&self.session_id, "hung up");
                // A reply to an earlier fragment ("okay, um") may still be on its way: cut
                // it, so the goodbye is the last thing said and the session ends now.
                out.push(Frame::Interruption).await;
            }
            Some(Action::Hold) => {
                // Whatever Ghost was saying stops; a pending question stays pending for later.
                out.push(Frame::Interruption).await;
            }
            Some(Action::Quiet) => {
                let mut data = Map::new();
                data.insert("until".into(), json!(h.until.map(|t| t.to_rfc3339())));
                self.events.emit(EventKind::Quiet, data).await; // the route engine listens
            }
            None => {}
        }
        if let Some(say) = h.say.filter(|s| !s.trim().is_empty()) {
            out.push(Frame::Speak(say)).await;
        }
        match h.action {
            Some(Action::HangUp) => {
                info!("interceptor: hang up");
                (self.on_hang_up)().await;
            }
            Some(Action::Hold) => {
                if let Some(on_hold) = &self.on_hold {
                    info!("interceptor: hold");
                    on_hold().await;
                }
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Stage for InterceptorProcessor {
    fn name(&self) -> &'static str {
        "interceptors"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::BotStarted => self.bot_speaking = true,
            Frame::BotStopped => self.bot_speaking = false,
            // a new turn, or this one has gone to the model
            Frame::UserStarted | Frame::UserStopped => self.turn_pieces.clear(),
            Frame::Transcript(text) if !self.interceptors.is_empty() && self.offered(text) => {
                let barged_in = self.barged_in();
                let pending = self.gate.pending(&self.session_id);
                let last = (self.last_assistant)();
                let began = Instant::now();
                // The piece on its own, then the whole turn so far: STT hands a goodbye
                // over in pieces, and the earlier ones have already gone down to the
                // aggregator.
                let mut texts = vec![text.clone()];
                if !self.turn_pieces.is_empty() {
                    let mut whole = self.turn_pieces.clone();
                    whole.push(text.clone());
                    texts.push(whole.join(" "));
                }
                for t in texts {
                    let turn = UserTurn {
                        text: t.clone(),
                        pending,
                        last_assistant: last.clone(),
                        barged_in,
                    };
                    for interceptor in self.interceptors.clone() {
                        // An interceptor that panics must not eat the turn.
                        let fut = std::panic::AssertUnwindSafe(interceptor.on_user_turn(&turn));
                        let handled = match futures::FutureExt::catch_unwind(fut).await {
                            Ok(h) => h,
                            Err(_) => {
                                tracing::error!("interceptor {} failed", interceptor.name());
                                continue;
                            }
                        };
                        if let Some(handled) = handled {
                            let name = interceptor.name().to_string();
                            debug::audio(|| format!("intercepted by {name}: {t:?} -> {handled:?}"));
                            self.probe.intercepted(&name, began.elapsed().as_secs_f64());
                            if !self.turn_pieces.is_empty() {
                                // the earlier pieces never run the model
                                out.push(Frame::ResetAggregation).await;
                            }
                            self.turn_pieces.clear();
                            self.handle(handled, out).await;
                            return; // the model never sees this turn
                        }
                    }
                }
                self.probe.interceptors(began.elapsed().as_secs_f64());
                self.turn_pieces.push(text.clone());
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
    use crate::confirmation::fake::FakeGate;
    use crate::frames::drain;
    use crate::intents::QuickIntents;

    struct Always(Option<Handled>);

    #[async_trait]
    impl TurnInterceptor for Always {
        fn name(&self) -> &str {
            "always"
        }
        async fn on_user_turn(&self, _turn: &UserTurn) -> Option<Handled> {
            self.0.clone()
        }
    }

    fn handled(say: Option<&str>, action: Option<Action>) -> Handled {
        Handled {
            say: say.map(String::from),
            action,
            until: None,
        }
    }

    fn make(
        interceptors: Vec<Arc<dyn TurnInterceptor>>,
        gate: Arc<FakeGate>,
    ) -> (InterceptorProcessor, Arc<Mutex<Vec<bool>>>) {
        let slept = Arc::new(Mutex::new(Vec::new()));
        let s = slept.clone();
        let on_sleep: SessionAction = Arc::new(move || {
            let s = s.clone();
            Box::pin(async move {
                s.lock().unwrap().push(true);
            })
        });
        (
            InterceptorProcessor::new(interceptors, gate, "s", on_sleep),
            slept,
        )
    }

    fn said(text: &str) -> Frame {
        Frame::Transcript(text.into())
    }

    fn spoken(frames: &[Frame]) -> Vec<String> {
        frames
            .iter()
            .filter_map(|f| match f {
                Frame::Speak(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn has_transcript(frames: &[Frame]) -> bool {
        frames.iter().any(|f| matches!(f, Frame::Transcript(_)))
    }

    fn dropped(frames: &[Frame]) -> bool {
        frames.iter().any(|f| matches!(f, Frame::ResetAggregation))
    }

    /// Keeps every turn it was offered, with its `barged_in`.
    struct Offered(Arc<Mutex<Vec<(String, bool)>>>);

    #[async_trait]
    impl TurnInterceptor for Offered {
        fn name(&self) -> &str {
            "offered"
        }
        async fn on_user_turn(&self, turn: &UserTurn) -> Option<Handled> {
            self.0
                .lock()
                .unwrap()
                .push((turn.text.clone(), turn.barged_in));
            None
        }
    }

    #[tokio::test]
    async fn test_over_ghost_only_a_barge_in_is_offered_and_marked() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let speaking = Speaking::new();
        let (p, _) = make(vec![Arc::new(Offered(seen.clone()))], FakeGate::new(false));
        let mut p = p.with_speaking(speaking.clone()).with_barge_in_min_words(3);
        let (out, mut rx) = Out::capture();
        speaking.set(true);
        p.process(said("that's all"), Direction::Down, &out).await; // a leak's size
        p.process(said("hang on a sec"), Direction::Down, &out)
            .await; // a barge-in
        speaking.set(false);
        p.process(said("that's all"), Direction::Down, &out).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                ("hang on a sec".to_string(), true),
                ("that's all".to_string(), false),
                // the piece joined with the earlier one of the turn, as always
                ("hang on a sec that's all".to_string(), false),
            ]
        );
        assert!(has_transcript(&drain(&mut rx).await));
    }

    #[tokio::test]
    async fn test_pass_through_when_nobody_handles() {
        let (mut p, _) = make(vec![Arc::new(Always(None))], FakeGate::new(false));
        let (out, mut rx) = Out::capture();
        p.process(said("hello"), Direction::Down, &out).await;
        assert!(has_transcript(&drain(&mut rx).await));
    }

    #[tokio::test]
    async fn test_handled_turn_is_dropped_and_spoken() {
        let (mut p, slept) = make(
            vec![Arc::new(Always(Some(handled(
                Some("Okay."),
                Some(Action::HangUp),
            ))))],
            FakeGate::new(false),
        );
        let (out, mut rx) = Out::capture();
        p.process(said("go to sleep"), Direction::Down, &out).await;
        let pushed = drain(&mut rx).await;
        assert!(!has_transcript(&pushed));
        assert_eq!(spoken(&pushed), vec!["Okay.".to_string()]);
        assert_eq!(*slept.lock().unwrap(), vec![true]);
    }

    #[tokio::test]
    async fn test_cancel_action() {
        let gate = FakeGate::new(true);
        let (mut p, _) = make(
            vec![Arc::new(Always(Some(handled(
                Some("Dropped."),
                Some(Action::Cancel),
            ))))],
            gate.clone(),
        );
        let (out, mut rx) = Out::capture();
        p.process(said("never mind"), Direction::Down, &out).await;
        assert_eq!(
            *gate.cancelled.lock().unwrap(),
            vec!["operator cancelled".to_string()]
        );
        assert_eq!(spoken(&drain(&mut rx).await), vec!["Dropped.".to_string()]);
    }

    #[tokio::test]
    async fn test_undo_that_reaches_the_model() {
        let gate = FakeGate::new(false);
        let (mut p, slept) = make(vec![Arc::new(QuickIntents)], gate.clone());
        let (out, mut rx) = Out::capture();
        p.process(said("Undo that."), Direction::Down, &out).await;
        let pushed = drain(&mut rx).await;
        let transcripts: Vec<String> = pushed
            .iter()
            .filter_map(|f| match f {
                Frame::Transcript(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(transcripts, vec!["Undo that.".to_string()]);
        assert!(spoken(&pushed).is_empty());
        assert!(gate.cancelled.lock().unwrap().is_empty() && slept.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_not_evidence_while_bot_speaks() {
        let (mut p, slept) = make(
            vec![Arc::new(Always(Some(handled(None, Some(Action::HangUp)))))],
            FakeGate::new(false),
        );
        let (out, mut rx) = Out::capture();
        p.process(Frame::BotStarted, Direction::Down, &out).await;
        // Under the barge-in's size: most likely Ghost's own voice past the canceller.
        p.process(said("sleep"), Direction::Down, &out).await;
        assert!(slept.lock().unwrap().is_empty());
        assert!(has_transcript(&drain(&mut rx).await));
    }

    #[tokio::test]
    async fn test_broken_interceptor_is_skipped() {
        struct Boom;
        #[async_trait]
        impl TurnInterceptor for Boom {
            fn name(&self) -> &str {
                "boom"
            }
            async fn on_user_turn(&self, _turn: &UserTurn) -> Option<Handled> {
                panic!("x")
            }
        }
        let (mut p, _) = make(
            vec![
                Arc::new(Boom),
                Arc::new(Always(Some(handled(Some("hi"), None)))),
            ],
            FakeGate::new(false),
        );
        let (out, mut rx) = Out::capture();
        p.process(said("x"), Direction::Down, &out).await;
        assert_eq!(spoken(&drain(&mut rx).await), vec!["hi".to_string()]);
    }

    #[tokio::test]
    async fn test_a_goodbye_in_pieces_is_caught_and_the_earlier_pieces_are_taken_back() {
        // STT hands "All right. Good." and then "I will talk to you later." as two frames.
        // The first went down to the aggregator; the goodbye takes it back (ResetAggregation)
        // so the model never runs on it, and the whole turn is judged together.
        let (mut p, slept) = make(vec![Arc::new(QuickIntents)], FakeGate::new(false));
        let (out, mut rx) = Out::capture();
        p.process(Frame::UserStarted, Direction::Down, &out).await;
        p.process(said("All right. Good."), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert!(has_transcript(&pushed) && slept.lock().unwrap().is_empty());
        p.process(said("I will talk to you later."), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert_eq!(*slept.lock().unwrap(), vec![true]);
        assert!(dropped(&pushed));

        let (mut p, slept) = make(vec![Arc::new(QuickIntents)], FakeGate::new(false));
        let (out, mut rx) = Out::capture();
        p.process(Frame::UserStarted, Direction::Down, &out).await;
        p.process(said("I will talk to you"), Direction::Down, &out)
            .await;
        p.process(said("later."), Direction::Down, &out).await;
        let pushed = drain(&mut rx).await;
        assert_eq!(*slept.lock().unwrap(), vec![true]);
        assert!(dropped(&pushed)); // neither piece alone is a goodbye

        // a lone piece that is not a goodbye still passes, and nothing is taken back
        let (mut p, slept) = make(vec![Arc::new(QuickIntents)], FakeGate::new(false));
        let (out, mut rx) = Out::capture();
        p.process(said("what needs my sign off"), Direction::Down, &out)
            .await;
        let pushed = drain(&mut rx).await;
        assert!(slept.lock().unwrap().is_empty() && !dropped(&pushed));
        assert!(matches!(pushed.last(), Some(Frame::Transcript(_))));
    }
}
