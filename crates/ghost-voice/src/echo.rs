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
//! Short answers are never echo: a transcript under `min_words` always passes, so "yes" or
//! "close it" after "Close it?" still reach the confirmation path. An interim transcript
//! (only while Ghost speaks) serves nothing but barge-in, so two of Ghost's own words are
//! enough to drop one; the final that follows is judged as above.

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;
use regex::Regex;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};

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
    speaking: bool,
    stopped_at: f64,
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
            speaking: false,
            stopped_at: f64::NEG_INFINITY,
        }
    }

    /// What Ghost said, as the assistant tap records it (the greeting included).
    pub fn spoke(&mut self, text: &str) {
        if !text.trim().is_empty() {
            if self.spoken.len() == 3 {
                self.spoken.pop_front();
            }
            self.spoken.push_back(text.to_string());
        }
    }

    /// Ghost's voice started or stopped, as the output transport reports it.
    pub fn bot_speaking(&mut self, on: bool) {
        self.speaking = on;
        if !on {
            self.stopped_at = (self.clock)();
        }
    }

    pub fn is_echo(&self, text: &str, min_words: Option<usize>) -> bool {
        let least = min_words.unwrap_or(self.min_words);
        if self.spoken.is_empty() || words(text).len() < least {
            return false;
        }
        let recent = self.speaking || (self.clock)() - self.stopped_at <= self.window;
        if !recent {
            return false;
        }
        self.spoken
            .iter()
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
    fn test_short_answers_always_pass() {
        let (mut f, _clock) = filter(3.0);
        f.spoke("Infra 77 looks fixed. Close it with a note saying so?");
        f.bot_speaking(true);
        assert!(!f.is_echo("yes", None));
        assert!(!f.is_echo("close it", None));
        assert!(!f.is_echo("yeah close it", None));
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
