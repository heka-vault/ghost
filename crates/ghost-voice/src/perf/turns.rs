//! `ghost-perf turns`: recorded takes through the turn's close (ADR 0049).
//!
//! `ghost-perf hear` measures what Ghost heard; this measures when it would have taken the
//! turn for over. Each take is one turn the operator meant as one, pauses and all. It goes
//! through the real speech-in stage (the session's VAD, the real Parakeet), and every
//! segment's end is judged as a session judges it: Smart Turn on the audio since the
//! segment before, the words the aggregator would hold then (the whole turn read once from
//! the second segment on, ADR 0044), and the wait the rule gives that end
//! (`turns::TurnTiming::close`, the same function the stop strategy is held to).
//!
//! An end that is not the take's last is a pause mid-turn, and its length is known: the
//! audio between the VAD's stop and its next start. When the wait is shorter than the
//! pause, the turn would have closed there, too soon. That count, under the settings as
//! they are and under the one grace Ghost had before, is the safety number for a shorter
//! grace; the wait at each take's last end is what the shorter grace buys.
//!
//! A turn closed too soon is not yet a turn cut off: the operator going on before any of
//! the answer is heard reopens it whole (`turns::TurnControl`). So the early closes are
//! also counted by the pause's length, against how soon Ghost's voice could start.
//!
//! The pause is the harness's: the VAD's usual start time (0.2 s) is in it, where a live
//! session re-arms in 0.1 s while a close waits, so live pauses read 0.1 s shorter and the
//! counts here err toward more early closes. Results hold numbers and flags, never words.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ghost_infer::{Silero, TurnVerdict, VadAnalyzer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::audio::Voice;
use super::harness::{RunConfig, make_settings};
use super::hear::plan::{self, Item, Kind};
use super::hear::{HearArgs, rendered};
use crate::config::VoiceSettings;
use crate::frames::{Audio, Direction, Frame, Out, Stage};
use crate::latency::percentile;
use crate::pipeline::Models;
use crate::stt::{PRE_ROLL_SECS, ParakeetStt, SAMPLE_RATE, TRAILING_SECS, level_dbfs, recognize};
use crate::turns::{
    Close, EndWords, HEARD_NOTHING_MIN_DBFS, SmartTurnAnalyzer, SmartTurnParams, TurnTiming,
};

const FRAME: usize = 320;
const TAIL_SECS: f64 = 2.0;

/// One segment's end inside a take: what the close is decided on. No words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct End {
    pub take: String,
    pub source: String,
    pub tags: Vec<String>,
    /// Which segment of the take, from 0.
    pub segment: usize,
    /// The take's last end: the operator was done.
    pub last: bool,
    /// Seconds from this VAD stop to the next VAD start; None at the last end.
    pub pause: Option<f64>,
    pub complete: bool,
    pub probability: f32,
    pub ends_sentence: bool,
    pub unfinished: bool,
    pub number: bool,
    pub words: usize,
}

impl End {
    fn verdict(&self) -> TurnVerdict {
        TurnVerdict {
            probability: self.probability,
            complete: self.complete,
        }
    }

    /// The wait the rule gives this end, on what was read off its words (the words
    /// themselves are not kept).
    pub fn close(&self, timing: &TurnTiming) -> (Close, f64) {
        timing.close_on(
            self.verdict(),
            EndWords {
                sentence: self.ends_sentence,
                unfinished: self.unfinished,
                number: self.number,
            },
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Results {
    pub meta: BTreeMap<String, Value>,
    pub ends: Vec<End>,
}

/// A take through the stage and Smart Turn: its segment ends.
async fn ends_of(
    models: &Models,
    settings: &VoiceSettings,
    item: &Item,
) -> anyhow::Result<Vec<End>> {
    let vad = VadAnalyzer::new(Silero::load(&models.silero)?, settings.vad_params());
    let pos = Arc::new(AtomicUsize::new(0));
    let clock_pos = pos.clone();
    let clock = Arc::new(move || 1.0e9 + clock_pos.load(Ordering::Relaxed) as f64 / 16_000.0);
    let mut stt = ParakeetStt::new(models.stt.clone(), Some(vad), settings.vad_params(), 0.0)
        .with_early_read(settings.stt_early_read)
        .with_early_read_after(settings.stt_early_read_after_secs)
        .with_vad_clock(clock);
    let mut analyzer = SmartTurnAnalyzer::new(
        models.turn.clone(),
        SmartTurnParams {
            stop_secs: settings.turn_stop_secs,
            unfinished_secs: settings.turn_unfinished_secs,
            sentence_min_prob: settings.turn_sentence_min_prob,
            number_secs: settings.turn_number_secs,
            pre_speech_secs: settings.turn_pre_speech_secs,
            ..SmartTurnParams::default()
        },
    );
    let (out, mut rx) = Out::capture();
    let mut audio = item.pcm.clone();
    audio.extend(plan::room_noise(TAIL_SECS, -70.0, 99));

    struct Segment {
        start: usize,
        stop: usize,
        verdict: Option<TurnVerdict>,
        text: String,
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut speaking = false;
    let mut end = 0usize;
    for chunk in audio.chunks(FRAME) {
        end += chunk.len();
        pos.store(end, Ordering::Relaxed);
        stt.process(
            Frame::AudioIn(Audio::new(chunk.to_vec(), SAMPLE_RATE)),
            Direction::Down,
            &out,
        )
        .await;
        let mut stopped = false;
        while let Ok(p) = rx.try_recv() {
            match p.frame {
                Frame::VadStarted { .. } => {
                    speaking = true;
                    segments.push(Segment {
                        start: end - chunk.len(),
                        stop: 0,
                        verdict: None,
                        text: String::new(),
                    });
                }
                Frame::VadStopped { .. } => {
                    speaking = false;
                    stopped = true;
                    if let Some(s) = segments.last_mut() {
                        s.stop = end;
                    }
                }
                Frame::Transcript(t) => {
                    if let Some(s) = segments.last_mut() {
                        s.text = t;
                    }
                }
                _ => {}
            }
        }
        // As the stop strategy hears it: the audio, then the verdict at the VAD's stop.
        analyzer.append_audio(chunk, speaking || stopped);
        if stopped {
            let verdict = analyzer.analyze_end_of_turn().await.map(|(v, _)| v);
            if let Some(s) = segments.last_mut() {
                s.verdict = verdict;
            }
        }
    }
    stt.stopped().await;
    segments.retain(|s| s.stop > s.start);

    let pre_roll = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
    let first = segments
        .first()
        .map_or(0, |s| s.start.saturating_sub(pre_roll));
    let mut ends = Vec::new();
    let mut pieces: Vec<String> = Vec::new();
    for (i, s) in segments.iter().enumerate() {
        if !s.text.is_empty() {
            pieces.push(s.text.clone());
        }
        // What the aggregator holds at this end: the pieces, or from the second segment
        // on the whole turn read once; a segment read as nothing with speech's level in it
        // is read whole too (`UserAggregator::heard_nothing`).
        let mut words = pieces.join(" ");
        let heard = !s.text.is_empty()
            || level_dbfs(&audio[s.start.min(audio.len())..s.stop.min(audio.len())])
                >= HEARD_NOTHING_MIN_DBFS;
        if i > 0 && settings.turn_reread && heard {
            let mut span = audio[first..s.stop.min(audio.len())].to_vec();
            span.extend(std::iter::repeat_n(
                0i16,
                (TRAILING_SECS * f64::from(SAMPLE_RATE)) as usize,
            ));
            let pcm = ghost_infer::audio::to_f32(&span);
            let model = models.stt.clone();
            let whole = tokio::task::spawn_blocking(move || recognize(&model, &pcm, true))
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            if !whole.is_empty() {
                words = whole;
            }
        }
        let Some(verdict) = s.verdict else { continue };
        let next = segments.get(i + 1);
        // The segment's own words weigh against a last comma the whole read wrote, when
        // Smart Turn is sure (`EndWords::of_turn`), as the stop strategy weighs them.
        let own = if s.text.is_empty() { &words } else { &s.text };
        let sure_voice = verdict.complete
            && verdict.probability >= settings.turn_confident_prob
            && settings.turn_confident_grace_secs < settings.turn_grace_secs;
        let read = EndWords::of_turn(&words, own, sure_voice);
        ends.push(End {
            take: item.id.clone(),
            source: item.source.clone(),
            tags: item.tags.clone(),
            segment: i,
            last: next.is_none(),
            pause: next.map(|n| n.start.saturating_sub(s.stop) as f64 / f64::from(SAMPLE_RATE)),
            complete: verdict.complete,
            probability: verdict.probability,
            ends_sentence: read.sentence,
            unfinished: read.unfinished,
            number: read.number,
            words: words.split_whitespace().count(),
        });
    }
    Ok(ends)
}

/// What a set of waits does to the takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Pauses mid-turn, and how many the wait is shorter than.
    pub pauses: usize,
    pub closed_early: usize,
    /// Of those, the ones the short grace closed.
    pub confident_early: usize,
    /// Takes with at least one early close.
    pub takes_early: usize,
    /// Early closes whose pause ran past each of [`VOICE_AT`]: the turn is still closed
    /// when Ghost's voice could start that long after the VAD's stop.
    pub still_closed: Vec<usize>,
    /// The wait at each take's last end.
    pub last_waits: Vec<f64>,
    /// How each last end was timed.
    pub last_bands: BTreeMap<&'static str, usize>,
}

/// Seconds after the VAD's stop at which Ghost's voice could start: a fast model with its
/// first clause rendered, the main model, and a slow reply.
pub const VOICE_AT: [f64; 3] = [0.8, 1.2, 2.0];

pub fn outcome(ends: &[End], timing: &TurnTiming) -> Outcome {
    let mut o = Outcome {
        pauses: 0,
        closed_early: 0,
        confident_early: 0,
        takes_early: 0,
        still_closed: vec![0; VOICE_AT.len()],
        last_waits: Vec::new(),
        last_bands: BTreeMap::new(),
    };
    let mut early_takes: Vec<(&str, &str)> = Vec::new();
    for e in ends {
        let (band, wait) = e.close(timing);
        match e.pause {
            Some(pause) => {
                o.pauses += 1;
                if pause > wait {
                    o.closed_early += 1;
                    o.confident_early += usize::from(band == Close::Confident);
                    let key = (e.take.as_str(), e.source.as_str());
                    if !early_takes.contains(&key) {
                        early_takes.push(key);
                    }
                    for (n, at) in o.still_closed.iter_mut().zip(VOICE_AT) {
                        *n += usize::from(pause > wait.max(at));
                    }
                }
            }
            None => {
                o.last_waits.push(wait);
                *o.last_bands.entry(band.as_str()).or_insert(0) += 1;
            }
        }
    }
    o.takes_early = early_takes.len();
    o
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

fn f2(x: Option<f64>) -> String {
    x.map_or("-".into(), |x| format!("{x:.2}"))
}

fn row(name: &str, o: &Outcome) -> String {
    let bands = o
        .last_bands
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "| {name} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {bands} |",
        o.closed_early,
        o.pauses,
        o.confident_early,
        o.takes_early,
        o.still_closed[0],
        o.still_closed[1],
        o.still_closed[2],
        f2(mean(&o.last_waits)),
        f2(percentile(&o.last_waits, 50.0)),
    )
}

/// The one grace for every turn: Ghost before ADR 0049.
pub fn one_grace(timing: &TurnTiming) -> TurnTiming {
    TurnTiming {
        confident_grace_secs: timing.grace_secs,
        ..*timing
    }
}

pub fn render(results: &Results, timing: &TurnTiming) -> String {
    let ends = &results.ends;
    let takes: Vec<(&str, &str)> = ends.iter().fold(Vec::new(), |mut v, e| {
        let k = (e.take.as_str(), e.source.as_str());
        if !v.contains(&k) {
            v.push(k);
        }
        v
    });
    let pauses: Vec<f64> = ends.iter().filter_map(|e| e.pause).collect();
    let in_pieces = takes
        .iter()
        .filter(|k| {
            ends.iter()
                .any(|e| (e.take.as_str(), e.source.as_str()) == **k && !e.last)
        })
        .count();
    let mut out = vec![
        format!(
            "# Ghost turn closes, {}",
            results
                .meta
                .get("date")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
        String::new(),
    ];
    for (k, v) in &results.meta {
        if k != "date" {
            out.push(format!(
                "- {k}: {}",
                v.as_str().map_or_else(|| v.to_string(), String::from)
            ));
        }
    }
    out.extend([
        String::new(),
        format!(
            "{} takes, each one turn as the operator meant it; {} reached the stage in pieces, \
             with {} pauses mid-turn between them (p50 {} s, p95 {} s, longest {} s).",
            takes.len(),
            in_pieces,
            pauses.len(),
            f2(percentile(&pauses, 50.0)),
            f2(percentile(&pauses, 95.0)),
            f2(pauses.iter().copied().fold(None, |m: Option<f64>, p| Some(
                m.map_or(p, |m| m.max(p))
            ))),
        ),
        String::new(),
        "## Before and after".into(),
        String::new(),
        "Closed early: a pause mid-turn longer than the wait its end got, so the turn was \
         handed over before the operator went on. Still closed at: of those, the pauses that \
         also ran past the moment Ghost's voice could start, that long after the VAD's stop; \
         the others are reopened whole with nothing heard. Last-end wait: what a turn the \
         operator was done with waits before it closes."
            .into(),
        String::new(),
        "| waits | closed early | by the short grace | takes | still closed at 0.8 s | at 1.2 s | at 2.0 s | last-end wait mean | p50 | last ends by band |".into(),
        "|---|---|---|---|---|---|---|---|---|---|".into(),
        row(
            &format!("one grace, {:.2} s (before)", timing.grace_secs),
            &outcome(ends, &one_grace(timing)),
        ),
        row(
            &format!(
                "short grace {:.2} s at {:.2} or surer (as set)",
                timing.confident_grace_secs, timing.confident_prob
            ),
            &outcome(ends, timing),
        ),
        String::new(),
        "## The short grace, swept".into(),
        String::new(),
        "| short grace s | probability | closed early | by the short grace | takes | still closed at 0.8 s | at 1.2 s | at 2.0 s | last-end wait mean | p50 | last ends by band |".into(),
        "|---|---|---|---|---|---|---|---|---|---|---|".into(),
    ]);
    for grace in [0.2, 0.35, 0.5, 0.8] {
        for prob in [0.5, 0.8, 0.9, 0.95, 0.98] {
            let t = TurnTiming {
                confident_grace_secs: grace,
                confident_prob: prob,
                ..*timing
            };
            out.push(row(&format!("{grace:.2} | {prob:.2}"), &outcome(ends, &t)));
        }
    }
    out.extend([
        String::new(),
        "## Every pause mid-turn".into(),
        String::new(),
        "| take | source | segment | pause s | Smart Turn | words end | wait before | wait as set |".into(),
        "|---|---|---|---|---|---|---|---|".into(),
    ]);
    let before = one_grace(timing);
    for e in ends.iter().filter(|e| e.pause.is_some()) {
        let pause = e.pause.unwrap_or_default();
        let (b0, w0) = e.close(&before);
        let (b1, w1) = e.close(timing);
        let mark = |w: f64| if pause > w { " (early)" } else { "" };
        out.push(format!(
            "| {} | {} | {} | {pause:.2} | {} {:.2} | {} | {} {w0:.2}{} | {} {w1:.2}{} |",
            e.take,
            e.source,
            e.segment + 1,
            if e.complete { "done" } else { "not done" },
            e.probability,
            match (e.unfinished, e.number, e.ends_sentence) {
                (true, _, _) => "mid-thought",
                (false, true, _) => "a number",
                (false, false, true) => "a sentence",
                (false, false, false) => "no stop",
            },
            b0.as_str(),
            mark(w0),
            b1.as_str(),
            mark(w1),
        ));
    }
    out.extend([
        String::new(),
        "## Every last end".into(),
        String::new(),
        "| Smart Turn | words end | n | wait before | wait as set |".into(),
        "|---|---|---|---|---|".into(),
    ]);
    let mut groups: BTreeMap<(bool, &str, String, String), usize> = BTreeMap::new();
    for e in ends.iter().filter(|e| e.last) {
        let words = match (e.unfinished, e.number, e.ends_sentence) {
            (true, _, _) => "mid-thought",
            (false, true, _) => "a number",
            (false, false, true) => "a sentence",
            (false, false, false) => "no stop",
        };
        let (b0, w0) = e.close(&before);
        let (b1, w1) = e.close(timing);
        *groups
            .entry((
                e.complete,
                words,
                format!("{} {w0:.2}", b0.as_str()),
                format!("{} {w1:.2}", b1.as_str()),
            ))
            .or_insert(0) += 1;
    }
    for ((complete, words, w0, w1), n) in groups {
        out.push(format!(
            "| {} | {words} | {n} | {w0} | {w1} |",
            if complete { "done" } else { "not done" }
        ));
    }
    let sure: Vec<f64> = ends
        .iter()
        .filter(|e| e.last && e.complete)
        .map(|e| f64::from(e.probability))
        .collect();
    out.extend([
        String::new(),
        format!(
            "Smart Turn's probability at the last ends it called done ({}): p5 {}, p50 {}, lowest {}.",
            sure.len(),
            f2(percentile(&sure, 5.0)),
            f2(percentile(&sure, 50.0)),
            f2(sure.iter().copied().fold(None, |m: Option<f64>, p| Some(
                m.map_or(p, |m| m.min(p))
            ))),
        ),
    ]);
    out.join("\n") + "\n"
}

pub fn read_results(path: &Path) -> anyhow::Result<Results> {
    let text =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// Run the takes, write `turns.json` and `turns.md`.
pub async fn run_turns(a: &HearArgs) -> anyhow::Result<i32> {
    let base = VoiceSettings::from_env()?;
    crate::debug::enable(base.debug_audio); // the operator's own debugging session only
    let cfg = RunConfig {
        overrides: a.overrides.clone(),
        ..RunConfig::default()
    };
    let settings = make_settings(&base, &cfg).map_err(|e| anyhow::anyhow!(e))?;
    let t = Instant::now();
    let models = Models::load(&settings)?;
    println!("turns: models loaded in {:.1}s", t.elapsed().as_secs_f64());
    let (items, source) = match &a.takes {
        Some(dir) => (
            plan::recorded(dir, a.channel, &a.blocks)?.0,
            format!("recorded takes in {}", dir.display()),
        ),
        None => {
            let voice = Voice::new(models.tts.clone(), &a.cache.join("utterances"));
            (
                rendered(&voice, &a.voices, &a.blocks)?,
                format!(
                    "the plan's desk blocks spoken by Kokoro ({})",
                    a.voices.join(", ")
                ),
            )
        }
    };
    let items: Vec<Item> = items
        .into_iter()
        .filter(|i| i.kind != Kind::Silence)
        .collect();
    anyhow::ensure!(!items.is_empty(), "no takes to run");
    println!("turns: {} takes from {source}", items.len());
    let mut ends = Vec::new();
    for item in &items {
        let of = ends_of(&models, &settings, item).await?;
        println!(
            "  {} {:>4.1}s  segments {}  pauses {}",
            item.id,
            item.seconds(),
            of.len(),
            of.iter()
                .filter_map(|e| e.pause)
                .map(|p| format!("{p:.2}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        ends.extend(of);
    }
    let timing = settings.turn_timing();
    let mut meta: BTreeMap<String, Value> = BTreeMap::new();
    meta.insert(
        "date".into(),
        json!(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
    );
    meta.insert("source".into(), json!(source));
    meta.insert(
        "speech to text".into(),
        json!(format!(
            "{} ({})",
            settings.stt_model,
            settings.stt_quantization.as_deref().unwrap_or("fp32")
        )),
    );
    let v = settings.vad_params();
    meta.insert(
        "vad".into(),
        json!(format!(
            "start {} s, stop {} s (a live session re-arms in {} s while a close waits)",
            v.start_secs, v.stop_secs, settings.vad_closing_start_secs
        )),
    );
    meta.insert(
        "waits".into(),
        json!(format!(
            "grace {} s, short grace {} s at {} or surer, a number +{} s, a sentence Smart Turn doubts {} s, unfinished {} s",
            timing.grace_secs,
            timing.confident_grace_secs,
            timing.confident_prob,
            timing.number_secs,
            timing.stop_secs,
            timing.unfinished_secs
        )),
    );
    meta.insert(
        "wall".into(),
        json!(format!("{:.0}s", t.elapsed().as_secs_f64())),
    );
    let results = Results { meta, ends };
    std::fs::write(
        a.out.join("turns.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    let text = render(&results, &timing);
    std::fs::write(a.out.join("turns.md"), &text)?;
    let before = outcome(&results.ends, &one_grace(&timing));
    let after = outcome(&results.ends, &timing);
    println!(
        "\nturns: {} pauses mid-turn; closed early {} before, {} as set ({} by the short grace); \
         last-end wait {} s before, {} s as set",
        after.pauses,
        before.closed_early,
        after.closed_early,
        after.confident_early,
        f2(mean(&before.last_waits)),
        f2(mean(&after.last_waits)),
    );
    println!("turns: report {}", a.out.join("turns.md").display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> TurnTiming {
        TurnTiming {
            grace_secs: 1.2,
            confident_grace_secs: 0.35,
            confident_prob: 0.9,
            stop_secs: 1.6,
            unfinished_secs: 3.0,
            sentence_min_prob: 0.1,
            number_secs: 0.5,
        }
    }

    fn end(pause: Option<f64>, probability: f32, sentence: bool) -> End {
        End {
            take: "B02".into(),
            source: "take".into(),
            tags: vec!["pause".into()],
            segment: 0,
            last: pause.is_none(),
            pause,
            complete: true,
            probability,
            ends_sentence: sentence,
            unfinished: false,
            number: false,
            words: 4,
        }
    }

    #[test]
    fn test_a_pause_longer_than_the_wait_is_an_early_close() {
        // "What's waiting for triage?" then 0.7 s, then "in the app repo": the grace held
        // it, the short grace closes it, and the operator going on reopens it unheard.
        let ends = vec![end(Some(0.7), 0.97, true), end(None, 0.98, true)];
        let before = outcome(&ends, &one_grace(&timing()));
        assert_eq!((before.pauses, before.closed_early), (1, 0));
        assert_eq!(before.last_waits, vec![1.2]);
        let after = outcome(&ends, &timing());
        assert_eq!((after.closed_early, after.confident_early), (1, 1));
        assert_eq!(after.takes_early, 1);
        assert_eq!(
            after.still_closed,
            vec![0, 0, 0],
            "reopened before any voice"
        );
        assert_eq!(after.last_waits, vec![0.35]);
        assert_eq!(after.last_bands.get("confident"), Some(&1));
    }

    #[test]
    fn test_an_unsure_end_keeps_the_grace_and_the_flags_read_as_the_words_did() {
        let ends = vec![end(Some(0.7), 0.6, true), end(Some(0.7), 0.97, false)];
        let after = outcome(&ends, &timing());
        assert_eq!(after.closed_early, 0);
        let mut unfinished = end(Some(2.0), 0.99, true);
        unfinished.unfinished = true;
        assert_eq!(unfinished.close(&timing()), (Close::Unfinished, 3.0));
        let mut number = end(None, 0.99, true);
        number.number = true;
        assert_eq!(number.close(&timing()), (Close::Number, 1.7));
    }

    #[test]
    fn test_the_report_names_no_words() {
        let results = Results {
            meta: BTreeMap::new(),
            ends: vec![end(Some(0.7), 0.97, true), end(None, 0.98, true)],
        };
        let text = render(&results, &timing());
        assert!(
            text.contains("| B02 | take | 1 | 0.70 | done 0.97 | a sentence |"),
            "{text}"
        );
        assert!(text.contains("closed early"), "{text}");
    }
}
