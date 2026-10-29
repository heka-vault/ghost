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

use async_trait::async_trait;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::intents::{GOODBYES, pick};

pub const SPOKEN_WITH: &[&str] = &["hang_up", "hold"];

pub struct NarrationCut {
    max: usize,
    held: Vec<String>,
    calling: bool,  // this response called a tool: drop its text
    released: bool, // this response is long: pass its text through
    closed: bool,   // going to sleep: say nothing more
    closing: bool,  // say nothing after the current response
    saw_text: bool, // this response produced words
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
        }
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
            Frame::FunctionCallsStarted(names)
                if !names.is_empty() && names.iter().all(|n| SPOKEN_WITH.contains(&n.as_str())) =>
            {
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
            Frame::Interruption => self.reset(),
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
