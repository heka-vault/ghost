//! Drop transcripts that are Ghost hearing itself.
//!
//! The barge-in rule (`turns`) only guards the time Ghost is speaking. On the phone the
//! echo comes back late: Ghost's audio crosses the network, plays on the speakerphone,
//! leaks past the phone's echo canceller, and crosses back, so its transcript lands after
//! `BotStopped` and reads as a new user turn ("Hey, what can I do?" right after the
//! greeting). This filter sits right after STT. A final transcript that arrives while Ghost
//! is speaking, or within `window_secs` after, and whose words are mostly a run of what
//! Ghost just said, is dropped before anything else sees it.
//!
//! Short answers are never echo once Ghost has stopped: a transcript under `min_words`
//! passes then, so "yes" or "close it" after "Close it?" still reach the confirmation
//! path. While Ghost speaks a transcript, interim or final, serves nothing but barge-in
//! (the confirmation processor ignores it), so two of Ghost's own words are enough to drop
//! one: the first half second of a reply leaks past the echo canceller while it converges,
//! and reads as a word or two of what Ghost just started saying.
//!
//! The filter knows the reply in progress a clause ahead of the speaker (`saying`, fed by
//! the TTS stage as it renders), not only the finished replies (`spoke`, fed by the
//! assistant tap at the reply's end), and that Ghost is speaking from the output stage's
//! shared flag (`crate::speaking`) as much as from the frames, which reach it late.
//!
//! An acknowledgment (`crate::acks`, "Sure.", "On it.") is a short clip after silence, the
//! moment the canceller leaks most, and a leaked "sure" heard once it has played would read
//! as a one-word answer and cut off the tool call Ghost just started. For `window_secs`
//! after one, a transcript made only of acknowledgment words is dropped, whatever its
//! length.

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;
use regex::Regex;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::speaking::Speaking;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9']+").unwrap());
/// An interim this long cuts Ghost off (GHOST_BARGE_IN_MIN_WORDS default).
pub const INTERIM_MIN_WORDS: usize = 2;

pub fn words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    WORD.find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// `difflib.SequenceMatcher.get_matching_blocks` over words, summed: the longest common
/// block, then recursively on either side (what `ratio` rests on).
pub(crate) fn matching(a: &[String], b: &[String]) -> usize {
    fn longest(a: &[String], b: &[String]) -> (usize, usize, usize) {
        let (mut bi, mut bj, mut bk) = (0, 0, 0);
        let mut prev = vec![0usize; b.len() + 1];
        for (i, wa) in a.iter().enumerate() {
            let mut cur = vec![0usize; b.len() + 1];
            for (j, wb) in b.iter().enumerate() {
                if wa == wb {
                    let k = prev[j] + 1;
                    cur[j + 1] = k;
                    if k > bk {
                        bi = i + 1 - k;
                        bj = j + 1 - k;
                        bk = k;
                    }
                }
            }
            prev = cur;
        }
        (bi, bj, bk)
    }
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (i, j, k) = longest(a, b);
    if k == 0 {
        return 0;
    }
    k + matching(&a[..i], &b[..j]) + matching(&a[i + k..], &b[j + k..])
}

/// How much of `heard` is a run of `spoken`, word by word, from 0 to 1.
pub fn echo_share(heard: &str, spoken: &str) -> f64 {
    let (h, s) = (words(heard), words(spoken));
    if h.is_empty() || s.is_empty() {
        return 0.0;
    }
    matching(&h, &s) as f64 / h.len() as f64
}

pub struct EchoFilter {
    window: f64,
    min_words: usize,
    threshold: f64,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    spoken: VecDeque<String>,
    /// The reply being spoken, a clause ahead of the speaker.
    current: String,
    speaking: bool,
    speaking_now: Speaking,
    stopped_at: f64,
    /// When Ghost last acknowledged a request, and every word an acknowledgment can say.
    acked_at: f64,
    ack_words: std::collections::HashSet<String>,
}

impl EchoFilter {
    pub fn new(window_secs: f64) -> Self {
        let zero = Instant::now();
        Self::with_clock(window_secs, move || zero.elapsed().as_secs_f64())
    }

    pub fn with_clock(window_secs: f64, clock: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self {
            window: window_secs,
            min_words: 3,
            threshold: 0.8,
            clock: Box::new(clock),
            spoken: VecDeque::with_capacity(3),
            current: String::new(),
            speaking: false,
            speaking_now: Speaking::new(),
            stopped_at: f64::NEG_INFINITY,
            acked_at: f64::NEG_INFINITY,
            ack_words: crate::acks::words(),
        }
    }

    /// Ghost is saying an acknowledgment now.
    pub fn acked(&mut self, _text: &str) {
        self.acked_at = (self.clock)();
    }

    /// Only acknowledgment words, soon after one: its echo.
    fn is_ack_echo(&self, text: &str) -> bool {
        let heard = words(text);
        !heard.is_empty()
            && (self.clock)() - self.acked_at <= self.window
            && heard.iter().all(|w| self.ack_words.contains(w))
    }

    pub fn with_speaking(mut self, speaking: Speaking) -> Self {
        self.speaking_now = speaking;
        self
    }

    /// What Ghost said, as the assistant tap records it (the greeting included): the reply
    /// is complete, so what was being said is this.
    pub fn spoke(&mut self, text: &str) {
        if !text.trim().is_empty() {
            if self.spoken.len() == 3 {
                self.spoken.pop_front();
            }
            self.spoken.push_back(text.to_string());
        }
        self.current.clear();
    }

    /// A clause Ghost is about to say, as the TTS stage hands it to the voice.
    pub fn saying(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.current.is_empty() {
            self.current.push(' ');
        }
        self.current.push_str(text);
    }

    /// Ghost's voice started or stopped, as the output transport reports it.
    pub fn bot_speaking(&mut self, on: bool) {
        self.speaking = on;
        if !on {
            self.stopped_at = (self.clock)();
        }
    }

    /// Ghost's voice is at the speaker: the frames said so, or the output stage's flag does.
    fn speaking(&self) -> bool {
        self.speaking || self.speaking_now.is_on()
    }

    pub fn is_echo(&self, text: &str, min_words: Option<usize>) -> bool {
        if self.is_ack_echo(text) {
            return true;
        }
        let speaking = self.speaking();
        let least = min_words.unwrap_or(if speaking {
            INTERIM_MIN_WORDS
        } else {
            self.min_words
        });
        if words(text).len() < least {
            return false;
        }
        if !(speaking || (self.clock)() - self.stopped_at <= self.window) {
            return false;
        }
        self.spoken
            .iter()
            .chain((!self.current.is_empty()).then_some(&self.current))
            .map(|s| echo_share(text, s))
            .fold(0.0, f64::max)
            >= self.threshold
    }
}

#[async_trait]
impl Stage for EchoFilter {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        match &frame {
            Frame::BotStarted => self.bot_speaking(true),
            Frame::BotStopped => self.bot_speaking(false),
            Frame::Transcript(text) if self.is_echo(text, None) => {
                debug::audio(|| format!("echo: dropped {text:?}"));
                return;
            }
            Frame::Interim(text) if self.is_echo(text, Some(INTERIM_MIN_WORDS)) => {
                debug::audio(|| format!("echo: dropped {text:?}"));
                return;
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct Clock(Arc<Mutex<f64>>);

    fn filter(window: f64) -> (EchoFilter, Arc<Mutex<f64>>) {
        let t = Arc::new(Mutex::new(100.0));
        let c = Clock(t.clone());
        let f = EchoFilter::with_clock(window, move || *c.0.lock().unwrap());
        (f, t)
    }

    fn stopped(f: &mut EchoFilter, clock: &Arc<Mutex<f64>>, at: f64) {
        *clock.lock().unwrap() = at;
        f.bot_speaking(false);
    }

    #[test]
    fn test_echo_share_counts_runs_of_spoken_words() {
        assert_eq!(
            echo_share("Hey, what can I do?", "Hey. What can I do?"),
            1.0
        );
        assert!(echo_share("yes close it", "Close it with a note saying so?") < 0.8);
        assert_eq!(echo_share("anything", ""), 0.0);
    }

    #[test]
    fn test_late_echo_of_the_greeting_is_dropped() {
        let (mut f, clock) = filter(3.0);
        f.spoke("Hey. What can I do?");
        stopped(&mut f, &clock, 100.0);
        *clock.lock().unwrap() = 101.5; // back over the network, after Ghost stopped
        assert!(f.is_echo("Hey, what can I do?", None));
    }

    #[test]
    fn test_echo_outside_the_window_is_kept() {
        let (mut f, clock) = filter(3.0);
        f.spoke("Hey. What can I do?");
        stopped(&mut f, &clock, 100.0);
        *clock.lock().unwrap() = 104.0;
        assert!(!f.is_echo("Hey, what can I do?", None));
    }

    #[test]
    fn test_short_answers_pass_once_ghost_has_stopped() {
        let (mut f, clock) = filter(3.0);
        f.spoke("Infra 77 looks fixed. Close it with a note saying so?");
        f.bot_speaking(true);
        assert!(!f.is_echo("yes", None));
        assert!(!f.is_echo("yeah close it", None)); // a word of the operator's own
        assert!(f.is_echo("close it", None)); // two of Ghost's words mid-question: a leak
        stopped(&mut f, &clock, 100.0);
        *clock.lock().unwrap() = 100.5;
        assert!(!f.is_echo("yes", None));
        assert!(!f.is_echo("close it", None)); // the answer to the question
        assert!(f.is_echo("close it with a note", None));
    }

    #[test]
    fn test_an_acknowledgments_echo_is_dropped_for_the_window_after_it() {
        let (mut f, clock) = filter(3.0);
        assert!(!f.is_echo("sure", None)); // no acknowledgment yet: an answer
        *clock.lock().unwrap() = 200.0;
        f.acked("Sure.");
        *clock.lock().unwrap() = 200.9; // the clip has played; its tail is transcribed
        assert!(f.is_echo("Sure.", None));
        assert!(f.is_echo("sure sure", Some(INTERIM_MIN_WORDS)));
        assert!(f.is_echo("okay", None)); // any acknowledgment's words
        assert!(!f.is_echo("sure but skip the closed ones", None)); // the operator's own
        assert!(!f.is_echo("wait", None));
        *clock.lock().unwrap() = 203.5;
        assert!(!f.is_echo("okay", None)); // past the window: the operator
    }

    #[test]
    fn test_the_reply_in_progress_is_known_a_clause_ahead_of_the_speaker() {
        let speaking = Speaking::new();
        let (f, _clock) = filter(3.0);
        let mut f = f.with_speaking(speaking.clone());
        f.spoke("Yeah?");
        speaking.set(true); // the first sample is at the speaker; no BotStarted frame yet
        f.saying("Here is everything,");
        assert!(f.is_echo("here is everything", Some(INTERIM_MIN_WORDS)));
        assert!(f.is_echo("is everything", None)); // a short final, mid-reply
        assert!(!f.is_echo("wait stop", Some(INTERIM_MIN_WORDS)));
        f.saying("in great detail,");
        assert!(f.is_echo("everything in great detail", None));
        f.spoke("Here is everything, in great detail."); // the tap, at the reply's end
        assert!(f.is_echo("in great detail", None));
        f.saying("Next:"); // a new reply: the old one is kept as finished, not as current
        assert!(f.is_echo("in great detail", None));
    }

    #[test]
    fn test_new_words_are_not_echo() {
        let (mut f, clock) = filter(3.0);
        f.spoke("Approved 142. Want to move on to the next one?");
        stopped(&mut f, &clock, 100.0);
        *clock.lock().unwrap() = 100.5;
        assert!(!f.is_echo("no let's look at the dark mode one", None));
    }
}
