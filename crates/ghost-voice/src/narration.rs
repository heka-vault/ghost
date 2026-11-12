//! Don't speak what Ghost says while it reaches for a tool.
//!
//! A response that calls a tool usually opens with narration ("Let me pull up the first one",
//! "I'll read the comments too"). The operator wants the answer, not the steps, and a prompt
//! rule alone didn't stop it. This sits right after the LLM: a response's text is held until
//! the response ends, then spoken; if the response calls a tool instead, the held text is
//! dropped, never spoken and never added to the context. The answer comes in the next
//! response, after the tool's result, and is spoken as usual.
//!
//! A response that calls only `hang_up` or `hold` is the exception: its text is the goodbye
//! (or the "take your time") and is spoken.
//!
//! Once the session is going to end (`close()`), nothing later is spoken: a reply to
//! something said just before the goodbye ("thanks" -> "You're welcome") would otherwise come
//! after it and hold the call open. The model's own goodbye response is kept
//! (`keep_current`); the no-model shortcut cuts the current response too.
//!
//! Holding costs the time to generate a reply (a sentence or two) before TTS starts. Past
//! `max_chars` the text is released as it comes: a reply that long isn't a preamble.
//!
//! One exception, once a turn: the words before the turn's first call. When a reply starts
//! a call ([`Frame::ToolStarting`]) and wrote a few words first ("Checking four thirteen."),
//! those words are what Ghost says while it works, instead of one of its fixed ones
//! (`crate::acks`): they go on in the frame's `said`, not as the reply's text, so they are
//! played as an acknowledgment is (not a turn, not in the transcript). Longer words, the
//! words before `think_harder`, and every later round's narration are still dropped
//! (2026-11-12: five seconds of the thinking sound while the model's own "Issue 415 was in
//! the ecosystem repository, so I'll read that one" went unsaid).

use async_trait::async_trait;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::intents::{GOODBYES, pick};
use crate::llm::THINK_HARDER_TOOL as THINK_HARDER;

pub const SPOKEN_WITH: &[&str] = &["hang_up", "hold"];

/// The most words of a preamble that are said for a turn's first call; longer is narration.
pub const PREAMBLE_MAX_WORDS: usize = 8;

/// The words before a turn's first call, when they are short enough to say for it.
pub fn preamble(words: &str) -> Option<String> {
    let words = words.trim();
    let n = words.split_whitespace().count();
    (n > 0 && n <= PREAMBLE_MAX_WORDS).then(|| words.to_string())
}

pub struct NarrationCut {
    max: usize,
    held: Vec<String>,
    calling: bool,  // this response called a tool: drop its text
    released: bool, // this response is long: pass its text through
    closed: bool,   // going to sleep: say nothing more
    closing: bool,  // say nothing after the current response
    saw_text: bool, // this response produced words
    first_call_seen: bool, // this turn has started a call: no later words are its preamble
    /// A `hold` in a turn that cut Ghost off is declined (`tools`): its words go unsaid.
    turns: Option<std::sync::Arc<crate::turns::TurnControl>>,
}

impl Default for NarrationCut {
    fn default() -> Self {
        Self::new(240)
    }
}

impl NarrationCut {
    pub fn new(max_chars: usize) -> Self {
        Self {
            max: max_chars,
            held: Vec::new(),
            calling: false,
            released: false,
            closed: false,
            closing: false,
            saw_text: false,
            first_call_seen: false,
            turns: None,
        }
    }

    pub fn with_turns(mut self, turns: std::sync::Arc<crate::turns::TurnControl>) -> Self {
        self.turns = Some(turns);
        self
    }

    /// The words with these calls are spoken: a goodbye, or a hold the operator asked for
    /// rather than an interruption's.
    fn spoken_with(&self, names: &[String]) -> bool {
        !names.is_empty()
            && names.iter().all(|n| SPOKEN_WITH.contains(&n.as_str()))
            && !(names.iter().any(|n| n == "hold")
                && self.turns.as_ref().is_some_and(|t| t.barged_in()))
    }

    pub fn close(&mut self, keep_current: bool) {
        if keep_current {
            self.closing = true;
            return;
        }
        self.closed = true;
        self.held.clear();
    }

    fn reset(&mut self) {
        self.held.clear();
        self.calling = false;
        self.released = false;
        self.saw_text = false;
    }

    async fn release(&mut self, out: &Out) {
        for text in std::mem::take(&mut self.held) {
            out.push(Frame::LlmText(text)).await;
        }
    }
}

#[async_trait]
impl Stage for NarrationCut {
    fn name(&self) -> &'static str {
        "narration"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction != Direction::Down {
            out.forward(frame, direction).await;
            return;
        }
        match &frame {
            Frame::HangUp { keep_current } => self.close(*keep_current),
            Frame::LlmStart => {
                self.reset();
                if self.closing {
                    self.closed = true;
                }
            }
            Frame::UserStarted | Frame::UserStopped | Frame::ThinkingStart => {
                self.first_call_seen = false;
            }
            Frame::HandOver { .. } => {
                // The main model's reply starts here, in the same model run.
                self.reset();
            }
            Frame::ToolStarting { name, said } if !SPOKEN_WITH.contains(&name.as_str()) => {
                let held = std::mem::take(&mut self.held).concat();
                let first = !self.first_call_seen;
                self.first_call_seen = true;
                self.calling = true;
                let words = if held.trim().is_empty() {
                    said.clone().unwrap_or_default()
                } else {
                    held
                };
                let said = (first && !self.closed && !self.released && name != THINK_HARDER)
                    .then(|| preamble(&words))
                    .flatten();
                if said.is_none() && !words.trim().is_empty() {
                    debug::audio(|| format!("narration: dropped {words:?}"));
                }
                out.push(Frame::ToolStarting {
                    name: name.clone(),
                    said,
                })
                .await;
                return;
            }
            Frame::FunctionCallsStarted(names) if self.spoken_with(names) => {
                self.released = true; // the goodbye: speak what was held and what follows
                self.release(out).await;
                if !self.saw_text && names.iter().all(|n| n == "hang_up") {
                    // The model hung up without a word (seen 2026-10-20: the call closed
                    // in silence). The goodbye is Ghost's, not optional: say one of its own.
                    out.push(Frame::Speak(pick(GOODBYES))).await;
                }
            }
            Frame::FunctionCallsStarted(_) => {
                if !self.held.is_empty() {
                    let held = self.held.concat();
                    debug::audio(|| format!("narration: dropped {held:?}"));
                }
                self.held.clear();
                self.calling = true;
            }
            Frame::LlmText(text) => {
                self.saw_text = true;
                if self.calling || self.closed {
                    return;
                }
                if self.released {
                    out.forward(frame, direction).await;
                    return;
                }
                self.held.push(text.clone());
                if self.held.iter().map(String::len).sum::<usize>() > self.max {
                    self.released = true;
                    self.release(out).await;
                }
                return;
            }
            Frame::LlmEnd => {
                if !self.calling {
                    self.release(out).await;
                }
                self.reset();
            }
            Frame::Interruption => {
                self.reset();
                self.first_call_seen = false;
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

    fn cut(max_chars: usize) -> NarrationCut {
        NarrationCut::new(max_chars)
    }

    async fn feed(c: &mut NarrationCut, out: &Out, frames: Vec<Frame>) {
        for f in frames {
            c.process(f, Direction::Down, out).await;
        }
    }

    fn texts(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match f {
                Frame::LlmText(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
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

    #[tokio::test]
    async fn test_narration_before_a_tool_call_is_dropped() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Let me pull up ".into()),
                Frame::LlmText("the first one.".into()),
                Frame::FunctionCallsStarted(vec!["get_issue".into()]),
                Frame::LlmText(" (late text from the same response)".into()),
                Frame::LlmEnd,
            ],
        )
        .await;
        let pushed = drain(&mut rx).await;
        assert_eq!(texts(&pushed), "");
        assert!(
            pushed
                .iter()
                .any(|f| matches!(f, Frame::FunctionCallsStarted(_)))
        );
        assert!(matches!(pushed.last(), Some(Frame::LlmEnd)));
    }

    #[tokio::test]
    async fn test_an_answer_is_spoken_whole_at_the_end() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Two are ".into()),
                Frame::LlmText("waiting.".into()),
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), ""); // held until the response ends
        feed(&mut c, &out, vec![Frame::LlmEnd]).await;
        let pushed = drain(&mut rx).await;
        assert_eq!(texts(&pushed), "Two are waiting.");
        assert!(matches!(pushed.last(), Some(Frame::LlmEnd)));
    }

    #[tokio::test]
    async fn test_a_long_reply_is_released_as_it_comes() {
        let mut c = cut(10);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Issue 142 is ".into()),
                Frame::LlmText("the cache.".into()),
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "Issue 142 is the cache.");
    }

    #[tokio::test]
    async fn test_nothing_more_is_said_once_going_to_sleep() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        c.close(false);
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("You're welcome.".into()),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "");
    }

    #[tokio::test]
    async fn test_the_goodbye_with_hang_up_is_spoken_and_nothing_after_and_a_hold_keeps_its_line() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Sounds good, talk later!".into()),
            ],
        )
        .await;
        c.close(true); // the tool ran
        feed(
            &mut c,
            &out,
            vec![
                Frame::FunctionCallsStarted(vec!["hang_up".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "Sounds good, talk later!");
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("You're welcome.".into()),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "");

        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Take your time.".into()),
                Frame::FunctionCallsStarted(vec!["hold".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "Take your time.");
    }

    fn starting(frames: &[Frame]) -> Vec<Option<String>> {
        frames
            .iter()
            .filter_map(|f| match f {
                Frame::ToolStarting { said, .. } => Some(said.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn the_words_before_a_turns_first_call_go_with_it_once() {
        // The main model's words stream before its call: held, then handed on with the
        // call's start for the voice to say, not spoken as the reply.
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::LlmText("Checking four ".into()),
                Frame::LlmText("thirteen.".into()),
                Frame::ToolStarting {
                    name: "github_issue_read".into(),
                    said: None,
                },
                Frame::FunctionCallsStarted(vec!["github_issue_read".into()]),
                Frame::LlmEnd,
                // The next round's narration is dropped as before.
                Frame::LlmStart,
                Frame::LlmText("It was in the other repository.".into()),
                Frame::ToolStarting {
                    name: "github_issue_read".into(),
                    said: None,
                },
                Frame::FunctionCallsStarted(vec!["github_issue_read".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        let pushed = drain(&mut rx).await;
        assert_eq!(texts(&pushed), "");
        assert_eq!(
            starting(&pushed),
            vec![Some("Checking four thirteen.".to_string()), None]
        );
    }

    #[tokio::test]
    async fn the_fast_models_held_words_come_in_the_frame_and_long_ones_are_dropped() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        let long = "Let me have a careful look at the tracker for you because a lot is going on";
        feed(
            &mut c,
            &out,
            vec![
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::ToolStarting {
                    name: "ghost_recall".into(),
                    said: Some("Let me think back.".into()),
                },
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::LlmText(long.into()),
                Frame::ToolStarting {
                    name: "github_issue_read".into(),
                    said: None,
                },
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::ToolStarting {
                    name: "think_harder".into(),
                    said: Some("Hmm, hard one.".into()),
                },
            ],
        )
        .await;
        let pushed = drain(&mut rx).await;
        assert_eq!(
            starting(&pushed),
            vec![Some("Let me think back.".to_string()), None, None]
        );
        assert_eq!(texts(&pushed), "");
    }

    #[tokio::test]
    async fn after_a_hand_over_the_main_models_answer_is_spoken() {
        // The fast model started a call (its words went with it) and handed over; the
        // main model answers in the same run. Its words are the reply.
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::UserStopped,
                Frame::LlmStart,
                Frame::ToolStarting {
                    name: "think_harder".into(),
                    said: None,
                },
                Frame::HandOver {
                    names: vec!["think_harder".into()],
                    secs: 1.0,
                },
                Frame::LlmText("By tenant first.".into()),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "By tenant first.");
    }

    #[tokio::test]
    async fn a_goodbyes_call_start_changes_nothing() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![
                Frame::LlmStart,
                Frame::LlmText("Talk soon!".into()),
                Frame::ToolStarting {
                    name: "hang_up".into(),
                    said: None,
                },
            ],
        )
        .await;
        c.close(true);
        feed(
            &mut c,
            &out,
            vec![
                Frame::FunctionCallsStarted(vec!["hang_up".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        assert_eq!(texts(&drain(&mut rx).await), "Talk soon!");
    }

    #[tokio::test]
    async fn test_a_hang_up_without_words_gets_a_goodbye_of_ghosts_own() {
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(&mut c, &out, vec![Frame::LlmStart]).await;
        c.close(true);
        feed(
            &mut c,
            &out,
            vec![
                Frame::FunctionCallsStarted(vec!["hang_up".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        let said = spoken(&drain(&mut rx).await);
        assert_eq!(said.len(), 1);
        assert!(GOODBYES.contains(&said[0].as_str()));

        // with words of its own, no extra goodbye
        let mut c = cut(240);
        let (out, mut rx) = Out::capture();
        feed(
            &mut c,
            &out,
            vec![Frame::LlmStart, Frame::LlmText("Talk soon!".into())],
        )
        .await;
        c.close(true);
        feed(
            &mut c,
            &out,
            vec![
                Frame::FunctionCallsStarted(vec!["hang_up".into()]),
                Frame::LlmEnd,
            ],
        )
        .await;
        let pushed = drain(&mut rx).await;
        assert!(spoken(&pushed).is_empty());
        assert_eq!(texts(&pushed), "Talk soon!");
    }
}
