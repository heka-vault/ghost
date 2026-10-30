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
//! and reads as a word or two of what Ghost just started saying. A segment that began
//! while Ghost spoke keeps that floor at its final, which lands after a short reply ends.
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
//!
//! The reply is text with digits in it ("Issue 409", "October 29th") and the transcript is
//! what the voice made of them ("issue four hundred and nine", "october twenty ninth"), so
//! the reply's side of the comparison spells every number out (`spoken_words`); live,
//! "Issue 409" came back as "Issue four hundred and", shared one word in four with the
//! reply, and cut Ghost off as a barge-in, after which Ghost answered its own echo.

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;
use ghost_core::request::number_mentions;
use regex::Regex;

use crate::debug;
use crate::frames::{Direction, Frame, Out, Stage};
use crate::speaking::Speaking;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9']+").unwrap());
/// Two of Ghost's own words in an interim are enough to call it echo while Ghost speaks
/// (fewer than the barge-in needs, GHOST_BARGE_IN_MIN_WORDS, on purpose).
pub const INTERIM_MIN_WORDS: usize = 2;

pub fn words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    WORD.find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// The words of what Ghost said, with every number also as the voice says it: "409" is
/// followed by "four hundred nine", "four hundred and nine", "four oh nine" and the rest
/// of `number_mentions`, "29th" by "twenty ninth". The transcript's run of words then
/// finds its match whichever form the voice and the ear agreed on; the extra forms only
/// lengthen this side, which the share is not measured against.
pub(crate) fn spoken_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in words(text) {
        let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        let suffix = &w[digits.len()..];
        let n = match digits.parse::<i64>() {
            Ok(n) if !digits.is_empty() && digits.len() <= 4 => n,
            _ => {
                out.push(w);
                continue;
            }
        };
        let ordinal_ = matches!(suffix, "st" | "nd" | "rd" | "th");
        out.push(w.clone());
        for form in number_mentions(n) {
            if form == digits {
                continue;
            }
            let form = if ordinal_ { ordinal(&form) } else { form };
            out.extend(form.split_whitespace().map(str::to_string));
        }
    }
    out
}

/// "twenty nine" as the voice says "29th": the last word made ordinal.
fn ordinal(cardinal: &str) -> String {
    let mut parts: Vec<&str> = cardinal.split_whitespace().collect();
    let last = match parts.pop() {
        Some(w) => w,
        None => return String::new(),
    };
    let ord = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    parts.push(&ord);
    parts.join(" ")
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

/// A word down to its stem, so an inflection the transcriber chose doesn't break a run:
/// "stopped" and "stopping" are one word here (2026-10-30: "Agreed, fully stopped." came
/// back for "Agreed, fully stopping the music", two of three words matched, and Ghost cut
/// itself off). Crude on purpose: the suffixes English puts on verbs and nouns, taken off
/// when enough of the word is left.
pub(crate) fn stem(word: &str) -> String {
    let base = ["ing", "ed", "es", "s", "ly", "er"]
        .iter()
        .find_map(|suffix| word.strip_suffix(suffix).filter(|rest| rest.len() >= 3))
        .unwrap_or(word);
    base.trim_end_matches('e').to_string()
}

/// How much of `heard` is a run of `spoken`, word by word, from 0 to 1. `spoken` is
/// Ghost's text, its numbers in digits; `heard` is the transcript, its numbers in words.
pub fn echo_share(heard: &str, spoken: &str) -> f64 {
    let stems = |ws: Vec<String>| -> Vec<String> { ws.iter().map(|w| stem(w)).collect() };
    let (h, s) = (stems(words(heard)), stems(spoken_words(spoken)));
    if h.is_empty() || s.is_empty() {
        return 0.0;
    }
    let mut h = h;
    // The last heard word may be cut short: an interim is read mid-word, and the
    // canceller chops a segment mid-word (2026-10-30 20:34: the interim "I do." of Ghost's
    // own "I don't have ..." shared one word of two and cut Ghost off). A stem of two
    // letters or more that opens one of Ghost's words is taken for that word.
    if let Some(last) = h.last().cloned()
        && last.chars().count() >= 2
        && !s.contains(&last)
        && let Some(whole) = s.iter().find(|w| w.starts_with(&last))
    {
        *h.last_mut().expect("a last word") = whole.clone();
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
    /// When the operator's current segment began (the VAD's start), on `clock`.
    vad_started_at: f64,
    /// Ghost was speaking when that segment began: its final is judged like an interim.
    vad_began_over_ghost: bool,
    /// ... and inside the canceller's warm-up after its voice (re)started.
    vad_in_warmup: bool,
}

/// How long after Ghost's voice (re)starts the canceller is taken to be leaking: a
/// segment that begins and is transcribed inside this window, while Ghost speaks, is its
/// voice, whatever the words. LiveKit ignores interruptions for 3 s after its agent
/// starts speaking; Ghost's canceller leaks about half a second, and the VAD's start lags
/// the leak by 0.2 s.
pub const WARMUP_SECS: f64 = 0.8;

/// Inside the warm-up a transcript this long, or one with a word that stops Ghost, is the
/// operator after all: the leak reads as a word or two of Ghost's own, never as "stop
/// that now" (the barge-in test, once the warm-up rule first ran live).
const WARMUP_KEEP_WORDS: usize = 3;
const INTERRUPT_WORDS: &[&str] = &[
    "stop", "wait", "hold", "hang", "no", "enough", "quiet", "pause", "shut",
];

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
            vad_started_at: f64::NEG_INFINITY,
            vad_began_over_ghost: false,
            vad_in_warmup: false,
        }
    }

    fn clearly_the_operator(text: &str) -> bool {
        let heard = words(text);
        heard.len() >= WARMUP_KEEP_WORDS
            || heard.iter().any(|w| INTERRUPT_WORDS.contains(&w.as_str()))
    }

    /// The current segment began inside the canceller's warm-up after Ghost's voice
    /// (re)started, and is still inside it: Ghost hearing itself, whatever the words.
    fn in_warmup(&self) -> bool {
        // Decided when the segment began (`drops`, at the VAD's start), and held for the
        // segment: its final lands after the window ("Get", 1.0 s after the opening's
        // onset, 2026-10-30 19:58) and is the same leak. Not only while Ghost speaks: a
        // reply shorter than the window ("I'm here.") has ended by the time its leak is
        // transcribed (18:12: "I'm" counted toward a barge-in after the reply had
        // stopped). Only a segment that began while the voice was playing: a short
        // greeting ("Yeah?") answered the moment it ends is the operator.
        self.vad_in_warmup
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
        // A segment that began over Ghost's voice is judged as its final arrives, often
        // just after the voice stopped (2026-10-30 19:27: "Go ahead." opened the session,
        // its interim was dropped, its 2-word final 0.6 s after the voice ended was not,
        // and the operator's turn began "Go ahead. Yeah, ...").
        let least = min_words.unwrap_or(if speaking || self.vad_began_over_ghost {
            INTERIM_MIN_WORDS
        } else {
            self.min_words
        });
        let heard = words(text);
        if heard.is_empty() || !(speaking || (self.clock)() - self.stopped_at <= self.window) {
            return false;
        }
        let share = self
            .spoken
            .iter()
            .chain((!self.current.is_empty()).then_some(&self.current))
            .map(|s| echo_share(text, s))
            .fold(0.0, f64::max);
        if heard.len() >= least {
            return share >= self.threshold;
        }
        // Under the floor: a segment that began over Ghost's voice and is nothing but its
        // words (2026-10-30 19:52: the one-word opening "Listening." reached the operator's
        // turn as its final, 1.3 s after the voice began).
        self.vad_began_over_ghost && share >= 1.0
    }
}

impl EchoFilter {
    /// Every frame of the session passes here, in order: Ghost's voice starting and
    /// stopping and the operator's segments are noted, and a transcript that is Ghost
    /// hearing itself is dropped (true). The one path for the stage and for the desk
    /// pipeline's shared filter (`pipeline::SharedEcho`): until 2026-10-30 the latter
    /// judged the words alone and never saw a VAD start, so the warm-up rule and the
    /// segment's start never applied live ("I'm here." opened the operator's turn twice).
    pub fn drops(&mut self, frame: &Frame) -> bool {
        match frame {
            Frame::BotStarted => self.bot_speaking(true),
            Frame::BotStopped => self.bot_speaking(false),
            Frame::VadStarted { .. } => {
                self.vad_started_at = (self.clock)();
                self.vad_began_over_ghost = self.speaking();
                self.vad_in_warmup = self.vad_began_over_ghost
                    && self
                        .speaking_now
                        .onset()
                        .is_some_and(|onset| onset.elapsed().as_secs_f64() <= WARMUP_SECS);
            }
            Frame::Transcript(text) | Frame::Interim(text)
                if self.in_warmup() && !Self::clearly_the_operator(text) =>
            {
                debug::audio(|| {
                    format!(
                        "echo: dropped {text:?} (inside the canceller's warm-up after Ghost's voice resumed)"
                    )
                });
                return true;
            }
            Frame::Transcript(text) if self.is_echo(text, None) => {
                debug::audio(|| format!("echo: dropped {text:?}"));
                return true;
            }
            Frame::Interim(text) if self.is_echo(text, Some(INTERIM_MIN_WORDS)) => {
                debug::audio(|| format!("echo: dropped {text:?}"));
                return true;
            }
            _ => {}
        }
        false
    }
}

#[async_trait]
impl Stage for EchoFilter {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if self.drops(&frame) {
            return;
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
    fn test_numbers_are_heard_as_the_voice_says_them() {
        // Live, 2026-10-30: the leak of "Issue 409 is open" read as "Issue four hundred
        // and", one word in four, and cut Ghost off; the next reading was answered as the
        // operator's turn.
        assert_eq!(echo_share("Issue four hundred and nine", "Issue 409"), 1.0);
        assert_eq!(
            echo_share("issue four oh nine is open", "Issue 409 is open"),
            1.0
        );
        assert_eq!(
            echo_share(
                "last updated october twenty ninth",
                "last updated October 29th."
            ),
            1.0
        );
        assert_eq!(echo_share("count to five", "count to 5"), 1.0);
        // an inflection the transcriber chose is the same word
        assert_eq!(
            echo_share(
                "Agreed, fully stopped.",
                "Agreed, fully stopping the music while we talk"
            ),
            1.0
        );
        assert!(echo_share("I do.", "I don't have a note on that.") >= 0.8); // cut mid-word
        assert!(echo_share("no stop", "nothing stops you") < 0.8); // only the last word
        assert_eq!(stem("stopped"), stem("stopping"));
        assert_eq!(stem("issues"), stem("issue"));
        assert_ne!(stem("read"), stem("ready"));
        assert!(echo_share("issue four twelve", "Issue 409 is open") < 0.8);
        assert_eq!(
            spoken_words("the 1st and 20th"),
            ["the", "1st", "first", "and", "20th", "twentieth"]
        );
        assert_eq!(spoken_words("in 2026"), ["in", "2026"]); // beyond the forms: digits only

        let speaking = Speaking::new();
        let (f, _clock) = filter(3.0);
        let mut f = f.with_speaking(speaking.clone());
        speaking.set(true);
        f.saying("Issue 409 is open, last updated October 29th.");
        assert!(f.is_echo("Issue four hundred and", Some(INTERIM_MIN_WORDS)));
        assert!(f.is_echo("Issue four hundred and nine", None));
        assert!(!f.is_echo("wait stop", Some(INTERIM_MIN_WORDS)));
    }

    #[tokio::test]
    async fn test_a_short_final_of_a_segment_that_began_over_ghost_is_its_echo() {
        // 2026-10-30 19:27: the session opened with "Go ahead."; the interim was dropped,
        // the 2-word final landed 0.6 s after the voice stopped and passed as the
        // operator's, so their turn read "Go ahead. Yeah, I don't like ...".
        use crate::frames::drain;
        let (mut f, clock) = filter(3.0);
        let (out, mut rx) = Out::capture();
        f.saying("Go ahead.");
        f.bot_speaking(true);
        *clock.lock().unwrap() = 100.0;
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 100.5;
        f.process(Frame::BotStopped, Direction::Down, &out).await;
        *clock.lock().unwrap() = 101.1;
        f.process(Frame::Transcript("Go ahead.".into()), Direction::Down, &out)
            .await;
        // a one-word opening ("Listening.") in a segment that began over the voice: its own
        f.saying("Listening.");
        f.bot_speaking(true);
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 101.2;
        f.process(Frame::BotStopped, Direction::Down, &out).await;
        *clock.lock().unwrap() = 101.4;
        f.process(
            Frame::Transcript("Listening.".into()),
            Direction::Down,
            &out,
        )
        .await;
        // the operator's short answer in the next segment, after the voice, still passes
        f.process(Frame::VadStopped { stop_secs: 0.5 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 101.5;
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 102.0;
        f.process(Frame::Transcript("Go on.".into()), Direction::Down, &out)
            .await;
        let passed: Vec<String> = drain(&mut rx)
            .await
            .into_iter()
            .filter_map(|p| match p {
                Frame::Transcript(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(passed, vec!["Go on.".to_string()]);
    }

    #[tokio::test]
    async fn test_a_segment_inside_the_warmup_after_ghosts_voice_resumes_is_dropped() {
        // 2026-10-30: "I think." and "Issue four.", garbled residuals of the first half
        // second after a clause gap, matched none of Ghost's words and cut it off.
        use crate::frames::drain;
        let speaking = Speaking::new();
        let (f, clock) = filter(3.0);
        let mut f = f.with_speaking(speaking.clone());
        speaking.set(true);
        let (out, mut rx) = Out::capture();
        // the voice resumed 0.4 s ago; the VAD fired 0.2 s after it; the words arrive now
        speaking.mark_onset_at(Instant::now() - std::time::Duration::from_millis(400));
        *clock.lock().unwrap() = 100.2;
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 100.4;
        f.process(Frame::Interim("I think".into()), Direction::Down, &out)
            .await;
        f.process(Frame::Transcript("I think.".into()), Direction::Down, &out)
            .await;
        let passed = drain(&mut rx).await;
        assert!(
            !passed
                .iter()
                .any(|p| matches!(p, Frame::Interim(_) | Frame::Transcript(_))),
            "{passed:?}"
        );
        // inside the same window, a stop or three words is the operator
        f.process(Frame::Interim("wait".into()), Direction::Down, &out)
            .await;
        f.process(Frame::Interim("no not that".into()), Direction::Down, &out)
            .await;
        let passed = drain(&mut rx).await;
        assert_eq!(
            passed
                .iter()
                .filter(|p| matches!(p, Frame::Interim(_)))
                .count(),
            2,
            "{passed:?}"
        );
        // a short reply ends right after the VAD fires on its leak; the leak's transcript
        // lands inside the window, after the voice
        speaking.mark_onset_at(Instant::now() - std::time::Duration::from_millis(300));
        *clock.lock().unwrap() = 101.5;
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        speaking.set(false);
        *clock.lock().unwrap() = 101.8;
        f.process(Frame::Transcript("I'm".into()), Direction::Down, &out)
            .await;
        assert!(
            !drain(&mut rx)
                .await
                .iter()
                .any(|p| matches!(p, Frame::Transcript(_)))
        );
        // a segment that began well after the onset is the operator's
        speaking.mark_onset_at(Instant::now() - std::time::Duration::from_secs(3));
        *clock.lock().unwrap() = 103.5;
        f.process(Frame::VadStarted { start_secs: 0.2 }, Direction::Down, &out)
            .await;
        *clock.lock().unwrap() = 103.9;
        f.process(
            Frame::Transcript("wait stop that".into()),
            Direction::Down,
            &out,
        )
        .await;
        let passed = drain(&mut rx).await;
        assert!(
            passed.iter().any(|p| matches!(p, Frame::Transcript(_))),
            "{passed:?}"
        );
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
