//! A take as the Hearing test records it (ADR 0042): the tape that keeps the moment before
//! the operator presses, how a take ends, and the check shown before the next one.
//!
//! The operator says when a take starts and ends. Held: the press starts it and the release
//! ends it, with [`PRE_SECS`] before the press and [`POST_SECS`] after the release kept, so a
//! late press or an early release does not cut a word. Across the room the key is out of
//! reach: the take starts after a countdown and ends [`QUIET_SECS`] after the last word the
//! VAD hears. A silent take runs for the plan's time. Nothing ends on a guess of how long
//! the line takes to say, which is what cut the terminal recorder's takes short.
//!
//! The check reads the take once and lines the words up against the line. What Ghost heard
//! is for the screen only: nothing here writes it anywhere.

use std::collections::VecDeque;
use std::path::Path;

use ghost_infer::silero::{Silero, VadAnalyzer, VadParams, VadState};
use serde::Serialize;

use super::plan::Kind;
use super::score::{Op, align, normalize};

pub const RATE: u32 = 16_000;
/// Kept before the press.
pub const PRE_SECS: f64 = 0.5;
/// Kept after the release.
pub const POST_SECS: f64 = 0.5;
/// A take that ends on silence ends this long after the last word.
pub const QUIET_SECS: f64 = 2.0;
/// No take runs longer.
pub const MAX_SECS: f64 = 30.0;
/// Peaks under this are too quiet to trust; at or over it, clipping.
pub const QUIET_PEAK_DBFS: f64 = -35.0;
pub const CLIP_PEAK_DBFS: f64 = -0.5;
/// Speech this close to either end of the take ran into it.
const EDGE_SECS: f64 = 0.1;

fn samples(secs: f64, channels: usize) -> usize {
    (secs * f64::from(RATE)) as usize * channels
}

/// How a take ends.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "how")]
pub enum Ending {
    /// The operator holds the key: the release ends it.
    Hold,
    /// After a countdown; ends [`QUIET_SECS`] after the last word.
    Quiet,
    /// A silent take: the plan's seconds.
    Timed { seconds: f64 },
}

/// The microphone while the recorder is open: always the last [`PRE_SECS`], and from a
/// take's start everything until it is done. Interleaved samples, `channels` wide.
#[derive(Debug)]
pub struct Tape {
    channels: usize,
    pre: VecDeque<i16>,
    take: Option<Vec<i16>>,
    /// The take's length when it was asked to stop, and how much more to keep.
    stop_at: Option<(usize, usize)>,
}

impl Tape {
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            channels,
            pre: VecDeque::with_capacity(samples(PRE_SECS, channels) + 1024),
            take: None,
            stop_at: None,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Audio in, whole frames of `channels` samples.
    pub fn push(&mut self, pcm: &[i16]) {
        if let Some(take) = &mut self.take {
            let room = match self.stop_at {
                Some((at, more)) => (at + more).saturating_sub(take.len()),
                None => samples(MAX_SECS, self.channels).saturating_sub(take.len()),
            };
            take.extend_from_slice(&pcm[..pcm.len().min(room)]);
            return;
        }
        self.pre.extend(pcm.iter().copied());
        let keep = samples(PRE_SECS, self.channels);
        if self.pre.len() > keep {
            let drop = self.pre.len() - keep;
            self.pre.drain(..drop);
        }
    }

    /// A take starts with what was kept before it.
    pub fn start(&mut self) {
        let mut take: Vec<i16> = self.pre.drain(..).collect();
        take.reserve(samples(10.0, self.channels));
        self.take = Some(take);
        self.stop_at = None;
    }

    pub fn recording(&self) -> bool {
        self.take.is_some()
    }

    /// The take's length so far, in seconds.
    pub fn secs(&self) -> f64 {
        self.take
            .as_ref()
            .map_or(0.0, |t| (t.len() / self.channels) as f64 / f64::from(RATE))
    }

    /// End the take after `more` seconds of audio still to come.
    pub fn stop(&mut self, more: f64) {
        if let Some(take) = &self.take
            && self.stop_at.is_none()
        {
            self.stop_at = Some((take.len(), samples(more, self.channels)));
        }
    }

    /// The take has all it will get: its tail after a stop, or [`MAX_SECS`].
    pub fn done(&self) -> bool {
        let Some(take) = &self.take else {
            return false;
        };
        match self.stop_at {
            Some((at, more)) => take.len() >= at + more,
            None => take.len() >= samples(MAX_SECS, self.channels),
        }
    }

    /// The take, and the tape keeps listening for the next one.
    pub fn finish(&mut self) -> Vec<i16> {
        self.stop_at = None;
        self.take.take().unwrap_or_default()
    }

    /// Throw the take away.
    pub fn cancel(&mut self) {
        self.stop_at = None;
        self.take = None;
        self.pre.clear();
    }
}

/// When a take that ends on silence is over: [`QUIET_SECS`] after the last speech, once
/// there has been some.
#[derive(Debug, Default, Clone)]
pub struct QuietEnd {
    heard: bool,
    quiet_for: f64,
}

impl QuietEnd {
    /// Feed `secs` of audio and whether the VAD hears speech in it; true when the take is
    /// over.
    pub fn feed(&mut self, speaking: bool, secs: f64) -> bool {
        if speaking {
            self.heard = true;
            self.quiet_for = 0.0;
            return false;
        }
        if self.heard {
            self.quiet_for += secs;
        }
        self.heard && self.quiet_for >= QUIET_SECS
    }
}

/// `(rms, peak)` of int16 audio, in dBFS; -120 for digital silence.
pub fn levels(pcm: &[i16]) -> (f64, f64) {
    if pcm.is_empty() {
        return (-120.0, -120.0);
    }
    let sum: f64 = pcm.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / pcm.len() as f64).sqrt() / 32768.0;
    let peak = pcm
        .iter()
        .map(|&s| i32::from(s).unsigned_abs())
        .max()
        .unwrap_or(0) as f64
        / 32768.0;
    let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { -120.0 };
    (db(rms).max(-120.0), db(peak).max(-120.0))
}

/// One channel of interleaved audio.
pub fn channel(pcm: &[i16], channels: usize, which: usize) -> Vec<i16> {
    pcm.iter()
        .skip(which)
        .step_by(channels.max(1))
        .copied()
        .collect()
}

/// A VAD the recorder runs live and the check runs over a take: the session's settings,
/// on the audio's own clock so Silero resets every 5 s of audio as it does live.
pub struct Vad {
    vad: VadAnalyzer,
    pos: usize,
}

impl Vad {
    pub fn load(silero: &Path, params: VadParams) -> anyhow::Result<Self> {
        Ok(Self {
            vad: VadAnalyzer::new(Silero::load(silero)?, params),
            pos: 0,
        })
    }

    /// Mono audio in; whether it is speech after it.
    pub fn speaking(&mut self, pcm: &[i16]) -> bool {
        self.pos += pcm.len();
        let now = 1.0e9 + self.pos as f64 / f64::from(RATE);
        matches!(
            self.vad.analyze(pcm, now),
            Ok(VadState::Speaking | VadState::Stopping)
        )
    }
}

/// Where in a take the VAD hears speech, in seconds: from the first start to the last
/// stop, the start moved back by the VAD's own start time. `None`: no speech.
pub fn speech_span(vad: &mut Vad, pcm: &[i16], params: VadParams) -> Option<(f64, f64)> {
    const STEP: usize = 512;
    let (mut first, mut last, mut was) = (None, None, false);
    let mut at = 0usize;
    for chunk in pcm.chunks(STEP) {
        at += chunk.len();
        let now = vad.speaking(chunk);
        if now && !was && first.is_none() {
            first = Some(at);
        }
        if !now && was {
            last = Some(at);
        }
        if now {
            last = None;
        }
        was = now;
    }
    let first = first?;
    let secs = |n: usize| n as f64 / f64::from(RATE);
    let start =
        (secs(first) - f64::from(params.start_secs) - STEP as f64 / f64::from(RATE)).max(0.0);
    let end = match last {
        Some(n) => (secs(n) - f64::from(params.stop_secs)).max(start),
        None => secs(pcm.len()), // still speaking at the end
    };
    Some((start, end))
}

/// A word of the line against what Ghost heard.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WordOp {
    Same,
    /// Heard as another word.
    Misheard,
    /// Said and not heard.
    Missed,
    /// Heard and not said.
    Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub op: WordOp,
    pub said: Option<String>,
    pub heard: Option<String>,
}

/// What may be wrong with a take, worded for the operator.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Warning {
    TooQuiet {
        peak: f64,
    },
    Clipping,
    /// No speech heard in a take that has a line.
    NoSpeech,
    /// Speech heard in a silent take.
    SpeechInSilence,
    /// Speech already going when the take began.
    StartedEarly,
    /// Speech still going when the take ended.
    CutOff,
    /// The line's last words were not heard.
    MissedEnd {
        words: usize,
    },
}

impl Warning {
    pub fn text(&self) -> String {
        match self {
            Warning::TooQuiet { peak } => format!(
                "Too quiet (peak {peak:.0} dBFS): is the mic muted, or are you far from it?"
            ),
            Warning::Clipping => "Clipping: back off a little, or turn the mic gain down.".into(),
            Warning::NoSpeech => "No speech heard.".into(),
            Warning::SpeechInSilence => {
                "Speech heard in a silent take: redo it if you said something.".into()
            }
            Warning::StartedEarly => {
                "You were already speaking when the take began: the first word may be cut.".into()
            }
            Warning::CutOff => {
                "You were still speaking when the take ended: the last word may be cut.".into()
            }
            Warning::MissedEnd { words } => format!(
                "Ghost didn't hear the last {}: did the take end too soon?",
                if *words == 1 {
                    "word".to_string()
                } else {
                    format!("{words} words")
                }
            ),
        }
    }

    /// Something that makes the take wrong, not just worth a look: the operator redoes it.
    pub fn redo(&self) -> bool {
        matches!(
            self,
            Warning::NoSpeech
                | Warning::StartedEarly
                | Warning::CutOff
                | Warning::Clipping
                | Warning::SpeechInSilence
        )
    }
}

/// The check shown after a take.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub seconds: f64,
    pub rms: f64,
    pub peak: f64,
    /// Where the VAD hears speech, in seconds into the take.
    pub speech: Option<(f64, f64)>,
    /// What Ghost heard, normalized as the score is (lowercase, numbers as digits).
    pub heard: String,
    pub words: Vec<Word>,
    /// Words of the line heard right, of the words in the line.
    pub matched: usize,
    pub said: usize,
    pub warnings: Vec<Warning>,
}

/// The line against what was heard, word by word, as the score aligns them.
pub fn words(line: &str, heard: &str) -> Vec<Word> {
    align(&normalize(line), &normalize(heard))
        .into_iter()
        .map(|op| match op {
            Op::Hit(w) => Word {
                op: WordOp::Same,
                said: Some(w.clone()),
                heard: Some(w),
            },
            Op::Sub(said, heard) => Word {
                op: WordOp::Misheard,
                said: Some(said),
                heard: Some(heard),
            },
            Op::Del(w) => Word {
                op: WordOp::Missed,
                said: Some(w),
                heard: None,
            },
            Op::Ins(w) => Word {
                op: WordOp::Extra,
                said: None,
                heard: Some(w),
            },
        })
        .collect()
}

/// The check, from the take's audio, where its speech is and what Ghost heard.
pub fn check(
    kind: Kind,
    line: &str,
    pcm: &[i16],
    speech: Option<(f64, f64)>,
    heard: &str,
) -> Check {
    let seconds = pcm.len() as f64 / f64::from(RATE);
    let (rms, peak) = levels(pcm);
    let words = if kind == Kind::Silence {
        words("", heard)
    } else {
        words(line, heard)
    };
    let said = words.iter().filter(|w| w.said.is_some()).count();
    let matched = words.iter().filter(|w| w.op == WordOp::Same).count();
    let mut warnings = Vec::new();
    if peak >= CLIP_PEAK_DBFS {
        warnings.push(Warning::Clipping);
    }
    match kind {
        Kind::Silence => {
            if speech.is_some() {
                warnings.push(Warning::SpeechInSilence);
            }
        }
        Kind::Speech | Kind::Reply => {
            if peak < QUIET_PEAK_DBFS {
                warnings.push(Warning::TooQuiet { peak });
            }
            match speech {
                None => warnings.push(Warning::NoSpeech),
                Some((start, end)) => {
                    if start < EDGE_SECS {
                        warnings.push(Warning::StartedEarly);
                    }
                    if end > seconds - EDGE_SECS {
                        warnings.push(Warning::CutOff);
                    }
                }
            }
            let missed_end = words
                .iter()
                .rev()
                .filter(|w| w.op != WordOp::Extra)
                .take_while(|w| w.op == WordOp::Missed)
                .count();
            if speech.is_some() && missed_end > 0 && missed_end < said {
                warnings.push(Warning::MissedEnd { words: missed_end });
            }
        }
    }
    Check {
        seconds,
        rms,
        peak,
        speech,
        heard: normalize(heard).join(" "),
        words,
        matched,
        said,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f64, amp: i16) -> Vec<i16> {
        (0..samples(secs, 1))
            .map(|i| if i % 2 == 0 { amp } else { -amp })
            .collect()
    }

    #[test]
    fn a_take_keeps_the_half_second_before_the_press_and_after_the_release() {
        let mut tape = Tape::new(1);
        tape.push(&tone(2.0, 100)); // long before: only the last 0.5 s is kept
        tape.start();
        assert!((tape.secs() - PRE_SECS).abs() < 1e-9);
        tape.push(&tone(1.0, 200));
        tape.stop(POST_SECS);
        assert!(!tape.done());
        tape.push(&tone(0.3, 300));
        assert!(!tape.done());
        tape.push(&tone(0.3, 300)); // more than the tail: the rest is not kept
        assert!(tape.done());
        let take = tape.finish();
        assert_eq!(take.len(), samples(PRE_SECS + 1.0 + POST_SECS, 1));
        assert_eq!(take[0], 100);
        assert!(!tape.recording());
    }

    #[test]
    fn a_take_stops_at_the_cap_and_a_stop_twice_keeps_the_first() {
        let mut tape = Tape::new(2);
        tape.start();
        tape.push(&tone(MAX_SECS + 1.0, 1).repeat(2));
        assert!(tape.done());
        assert_eq!(tape.finish().len(), samples(MAX_SECS, 2));

        let mut tape = Tape::new(1);
        tape.start();
        tape.push(&tone(1.0, 1));
        tape.stop(0.0);
        tape.push(&tone(1.0, 1));
        tape.stop(5.0); // ignored: the first stop stands
        assert!(tape.done());
        assert_eq!(tape.finish().len(), samples(1.0, 1));
    }

    #[test]
    fn a_cancelled_take_leaves_nothing_and_no_pre_roll() {
        let mut tape = Tape::new(1);
        tape.push(&tone(1.0, 1));
        tape.start();
        tape.cancel();
        assert!(!tape.recording());
        tape.start();
        assert_eq!(tape.secs(), 0.0);
    }

    #[test]
    fn a_take_on_silence_ends_two_seconds_after_the_last_word_and_not_before_any() {
        let mut q = QuietEnd::default();
        assert!(!q.feed(false, 10.0)); // the operator is still walking over
        assert!(!q.feed(true, 1.0));
        assert!(!q.feed(false, 1.5)); // a pause
        assert!(!q.feed(true, 0.5));
        assert!(!q.feed(false, 1.9));
        assert!(q.feed(false, 0.1));
    }

    #[test]
    fn levels_are_dbfs() {
        let (rms, peak) = levels(&[16384, -16384]);
        assert!((rms - -6.02).abs() < 0.01, "{rms}");
        assert!((peak - -6.02).abs() < 0.01, "{peak}");
        assert_eq!(levels(&[0, 0]), (-120.0, -120.0));
        assert_eq!(channel(&[1, 2, 3, 4], 2, 1), vec![2, 4]);
    }

    #[test]
    fn the_words_line_up_like_the_score() {
        let w = words("Read me issue four oh three.", "read me issue 403 please");
        let ops: Vec<&WordOp> = w.iter().map(|w| &w.op).collect();
        assert_eq!(
            ops,
            [
                &WordOp::Same,
                &WordOp::Same,
                &WordOp::Same,
                &WordOp::Same,
                &WordOp::Extra
            ]
        );
        assert_eq!(w[3].said.as_deref(), Some("403"));
    }

    fn loud() -> Vec<i16> {
        tone(3.0, 3000)
    }

    #[test]
    fn a_take_whose_last_words_were_lost_says_so() {
        let c = check(
            Kind::Speech,
            "Close issue one forty two in the app repo.",
            &loud(),
            Some((0.6, 2.95)),
            "close issue 142 in the",
        );
        assert_eq!(c.said, 7);
        assert_eq!(c.matched, 5);
        assert!(c.warnings.contains(&Warning::MissedEnd { words: 2 }));
        assert!(c.warnings.contains(&Warning::CutOff));
        assert!(c.warnings.iter().any(Warning::redo));
    }

    #[test]
    fn a_clean_take_has_no_warnings() {
        let c = check(
            Kind::Speech,
            "What needs my sign-off today?",
            &loud(),
            Some((0.6, 2.0)),
            "What needs my sign off today?",
        );
        assert_eq!(c.warnings, vec![]);
        assert_eq!(c.matched, c.said);
    }

    #[test]
    fn silence_quiet_early_starts_and_clipping_are_flagged() {
        let c = check(
            Kind::Silence,
            "",
            &tone(3.0, 30),
            Some((1.0, 1.4)),
            "thank you",
        );
        assert_eq!(c.warnings, vec![Warning::SpeechInSilence]);
        assert_eq!(c.words.len(), 2);
        assert!(c.words.iter().all(|w| w.op == WordOp::Extra));
        assert!(
            check(Kind::Silence, "", &tone(3.0, 30), None, "")
                .warnings
                .is_empty()
        );

        let c = check(Kind::Reply, "Yes.", &tone(2.0, 30), None, "");
        assert!(c.warnings.contains(&Warning::NoSpeech));
        assert!(matches!(c.warnings[0], Warning::TooQuiet { .. }));

        let c = check(
            Kind::Reply,
            "Yes.",
            &tone(2.0, 32767),
            Some((0.0, 1.0)),
            "yes",
        );
        assert!(c.warnings.contains(&Warning::Clipping));
        assert!(c.warnings.contains(&Warning::StartedEarly));
    }
}
