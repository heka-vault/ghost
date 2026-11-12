//! What a session is doing, for the desktop's conversation screen (ADR 0055): the
//! operator's turn opening and closing, a call to a tool starting and coming back, the fast
//! model handing a turn to the main one, Ghost being cut off, and a turn that failed.
//!
//! An [`Observer`] on the pipeline: it sees every frame once, when the stage that made it
//! pushes it, and passes on a handful as an [`Activity`]. It carries names (a tool as the
//! model calls it), ids and a duration, never words: no transcript, no reply text, no
//! tool arguments or results, no audio. The operator's turn is the VAD's and the turn
//! rule's fact that someone is speaking, never what they said, so the screen can show a
//! calm sign of it without words arriving as they talk (#73). Everything here exists only
//! inside a session; the passive path has no pipeline to observe.
//!
//! The cost is one `match` per frame on the pushing stage's task and, for the few frames
//! that count, one callback: about four per spoken turn and two per tool call. The
//! callback must not block (the daemon's is a broadcast send that drops for a slow client).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::frames::{Direction, Frame, Observer};

/// What happened, as the apps show it.
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    /// The operator started speaking (a turn opened).
    YouStarted,
    /// The operator's turn closed: the model is about to answer it.
    YouStopped,
    /// The model called a tool; `name` as offered to it (`github_issue_read`).
    Call { name: String, id: String },
    /// The call came back (a result, an error or a refusal: the model reads which).
    Result { name: String, id: String },
    /// The fast model handed the turn to the main one (ADR 0050), with the calls it had
    /// started by name (none of them run) and how long it took to decide.
    HandOver { names: Vec<String>, secs: f64 },
    /// The operator cut in while Ghost was speaking: what was left of the reply is not said.
    Cut,
    /// The turn failed in the pipeline. The message stays in the log.
    Error,
}

/// Called with each activity. Must not block.
pub type OnActivity = Arc<dyn Fn(Activity) + Send + Sync>;

/// The pipeline observer that reports [`Activity`].
pub struct ActivityObserver {
    on: OnActivity,
    /// Ghost's reply is at the speaker (between `BotStarted` and `BotStopped`).
    speaking: AtomicBool,
}

impl ActivityObserver {
    pub fn new(on: OnActivity) -> Self {
        Self {
            on,
            speaking: AtomicBool::new(false),
        }
    }

    fn activity(&self, frame: &Frame) -> Option<Activity> {
        Some(match frame {
            Frame::UserStarted => Activity::YouStarted,
            Frame::UserStopped => Activity::YouStopped,
            Frame::FunctionCallInProgress { name, id } => Activity::Call {
                name: name.clone(),
                id: id.clone(),
            },
            Frame::FunctionCallResult { name, id } => Activity::Result {
                name: name.clone(),
                id: id.clone(),
            },
            Frame::HandOver { names, secs } => Activity::HandOver {
                names: names.clone(),
                secs: *secs,
            },
            Frame::BotStarted => {
                self.speaking.store(true, Ordering::Relaxed);
                return None;
            }
            Frame::BotStopped => {
                self.speaking.store(false, Ordering::Relaxed);
                return None;
            }
            // Only a cut of a reply being heard; an interruption with Ghost silent cuts
            // nothing the operator heard. Several stages may push one: the first counts.
            Frame::Interruption => {
                if !self.speaking.swap(false, Ordering::Relaxed) {
                    return None;
                }
                Activity::Cut
            }
            Frame::Error(_) => Activity::Error,
            _ => return None,
        })
    }
}

impl Observer for ActivityObserver {
    fn on_frame(&self, frame: &Frame, _source: &'static str, _at: f64, _direction: Direction) {
        if let Some(a) = self.activity(frame) {
            (self.on)(a);
        }
    }
}

/// Several observers as one: the latency timeline and the activity both watch a session.
pub struct Observers(pub Vec<Arc<dyn Observer>>);

impl Observer for Observers {
    fn on_frame(&self, frame: &Frame, source: &'static str, at: f64, direction: Direction) {
        for o in &self.0 {
            o.on_frame(frame, source, at, direction);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn recorder() -> (ActivityObserver, Arc<Mutex<Vec<Activity>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let obs = ActivityObserver::new(Arc::new(move |a| s.lock().unwrap().push(a)));
        (obs, seen)
    }

    fn feed(obs: &ActivityObserver, frames: Vec<Frame>) {
        for f in frames {
            obs.on_frame(&f, "test", 0.0, Direction::Down);
        }
    }

    #[test]
    fn a_turn_with_a_call_reports_names_and_ids_only() {
        let (obs, seen) = recorder();
        feed(
            &obs,
            vec![
                Frame::UserStarted,
                Frame::Transcript("close one five one".into()),
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::LlmText("Let me look.".into()),
                Frame::FunctionCallInProgress {
                    name: "github_issue_read".into(),
                    id: "c1".into(),
                },
                Frame::FunctionCallResult {
                    name: "github_issue_read".into(),
                    id: "c1".into(),
                },
                Frame::LlmEnd,
            ],
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                Activity::YouStarted,
                Activity::YouStopped,
                Activity::Call {
                    name: "github_issue_read".into(),
                    id: "c1".into()
                },
                Activity::Result {
                    name: "github_issue_read".into(),
                    id: "c1".into()
                },
            ]
        );
    }

    #[test]
    fn words_never_pass() {
        let (obs, seen) = recorder();
        feed(
            &obs,
            vec![
                Frame::Interim("hello".into()),
                Frame::Transcript("hello".into()),
                Frame::EarlyTranscript("hello".into()),
                Frame::LlmText("hi".into()),
                Frame::Speak("hi".into()),
                Frame::TtsText("hi".into()),
            ],
        );
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_cut_counts_only_while_ghost_is_heard_and_once() {
        let (obs, seen) = recorder();
        feed(&obs, vec![Frame::Interruption]);
        assert!(seen.lock().unwrap().is_empty(), "silent: nothing was cut");
        feed(
            &obs,
            vec![Frame::BotStarted, Frame::Interruption, Frame::Interruption],
        );
        assert_eq!(*seen.lock().unwrap(), vec![Activity::Cut]);
        feed(
            &obs,
            vec![Frame::BotStarted, Frame::BotStopped, Frame::Interruption],
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "played out: nothing was cut");
    }

    #[test]
    fn a_handover_and_an_error_are_reported() {
        let (obs, seen) = recorder();
        feed(
            &obs,
            vec![
                Frame::HandOver {
                    names: vec!["github_issue_write".into()],
                    secs: 0.4,
                },
                Frame::Error("model: 529 overloaded".into()),
            ],
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                Activity::HandOver {
                    names: vec!["github_issue_write".into()],
                    secs: 0.4
                },
                Activity::Error
            ]
        );
    }

    #[test]
    fn observers_fan_out() {
        let (a, seen_a) = recorder();
        let (b, seen_b) = recorder();
        let both = Observers(vec![Arc::new(a), Arc::new(b)]);
        both.on_frame(&Frame::UserStarted, "test", 0.0, Direction::Down);
        assert_eq!(seen_a.lock().unwrap().len(), 1);
        assert_eq!(seen_b.lock().unwrap().len(), 1);
    }
}
