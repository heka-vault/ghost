//! The trained wake detectors (`ghost_voice.wakeword` over `livekit-wakeword`, ADR 0025).
//!
//! Every 80 ms they score the last 2 s, sharing one pass of the audio features: the
//! melspectrogram graph (`x / 10 + 2` after it, openWakeWord's transform), Google's speech
//! embedding over 76-frame windows every 8 frames, and each classifier over the last 16
//! embeddings. Each model listens for one thing (`model_meaning`): `hey_ghost` for "hey
//! ghost", `ghost_*` for the operator addressing Ghost by name with a request. A model's
//! score at or above its threshold is a hit, decided the moment its phrase ends; the
//! pipeline then has Parakeet check it (`matcher`) and reads the first turn.
//!
//! The INFO log gets scores and decisions, never words.

use std::path::{Path, PathBuf};

use ort::{session::Session, value::Tensor};

use super::matcher::{WakeMatch, match_opening, match_wake};
use crate::{
    InferError, Result,
    session::{self, SessionConfig},
};

pub const SAMPLE_RATE: usize = 16_000;
/// 80 ms: how often the detector scores.
pub const STEP: usize = 1280;
/// 2 s: what each score covers.
pub const WINDOW: usize = 32_000;
/// Audio before the hit that the check and the first turn are read from.
pub const PRE_SECS: f64 = 2.0;
/// An opener ("Ghost, what did we leave off on") can fill the detector's whole 2 s window,
/// and the operator may pause after the name ("Ghost. I need you …"): the name, a pause of
/// up to 2 s and the opener must all fit.
pub const OPENING_PRE_SECS: f64 = 4.5;
/// The check runs this long after the hit at the latest.
pub const CHECK_SECS: f64 = 1.0;
/// Where the operator started talking: a pause shorter than `RUN_GAP_SECS` (after "Ghost.",
/// before the request) doesn't start anew, and the VAD says "started" about
/// `SPEECH_PAD_SECS` after the first sound. The check reads no further back than
/// `MAX_PRE_SECS` before the hit.
pub const RUN_GAP_SECS: f64 = 3.0;
pub const SPEECH_PAD_SECS: f64 = 0.5;
pub const MAX_PRE_SECS: f64 = 6.0;
/// After a hit, the most Ghost waits for the operator to finish.
pub const MAX_LISTEN_SECS: f64 = 12.0;
/// The detector already knew it heard "hey ghost"; finding where the phrase ends in the
/// transcript, to split off the first turn, may take a looser spelling ("he goes").
pub const REMAINDER_FUZZY: f64 = 0.8;

pub const DETECTOR_PHRASE: &str = "hey ghost";
/// What a `ghost_*.onnx` model hears: the name opening a request.
pub const ADDRESS_PHRASE: &str = "ghost";
/// The openers the `ghost_*` models were trained on
/// (docs/benchmarks/2026-10-23-ghost-openers.md): after a first word that is only shaped
/// like the name, one of these must follow.
pub const OPENERS: &[&str] = &[
    "i need you",
    "let's work on",
    "what is the status of",
    "what's the status of",
    "what did we leave off",
    "where did we leave off",
];
pub const DEFAULT_MODEL_NAME: &str = "hey_ghost.onnx";
/// Ghost addressed by name: one model, or one per opener.
pub const ADDRESS_MODEL_GLOB: &str = "ghost_*.onnx";

const EMBEDDING_WINDOW: usize = 76;
const EMBEDDING_STRIDE: usize = 8;
const MIN_EMBEDDINGS: usize = 16;
const MEL_BINS: usize = 32;
const EMBEDDING_DIM: usize = 96;

/// A model file's stem (how livekit-wakeword names its score) -> the phrase the check and
/// the first turn go by, and whether that phrase must open what was said.
pub fn model_meaning(stem: &str) -> (&'static str, bool) {
    if stem.starts_with("ghost_") {
        (ADDRESS_PHRASE, true)
    } else {
        (DETECTOR_PHRASE, false)
    }
}

/// One trained model and what its hit means.
#[derive(Debug, Clone, PartialEq)]
pub struct WakeModel {
    /// The model file's stem.
    pub name: String,
    pub phrase: String,
    pub threshold: f32,
    /// The phrase opens what was said; the rest is the request.
    pub opening: bool,
    /// Parakeet checks this model's hits; `None`: the detector's rule.
    pub verify: Option<bool>,
    /// A score from here, under `threshold`, is a weak hit: one Parakeet must confirm,
    /// whatever `verify` says. For a call the music drowned (2026-10-30: the operator's
    /// "Hey Ghost" over Spotify scored 0.25-0.62 against the 0.7 threshold; the
    /// canceller leaves the music in on the reSpeaker's beam). `None`: `threshold` only.
    pub checked_from: Option<f32>,
}

impl WakeModel {
    pub fn known(path: &Path, threshold: f32, verify: Option<bool>) -> Self {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (phrase, opening) = model_meaning(&name);
        Self {
            name,
            phrase: phrase.to_string(),
            threshold,
            opening,
            verify,
            checked_from: None,
        }
    }

    pub fn checked_from(mut self, from: Option<f32>) -> Self {
        self.checked_from = from;
        self
    }

    /// The least score that is a hit at all.
    pub fn floor(&self) -> f32 {
        self.checked_from
            .map_or(self.threshold, |c| c.min(self.threshold))
    }

    /// A hit at `score` waits for Parakeet: the model is checked, or the hit is weak.
    pub fn needs_check(&self, score: f32, detector_verify: bool) -> bool {
        score < self.threshold || self.verify.unwrap_or(detector_verify)
    }

    /// The model's phrase in `text`: `match_opening` for a name that must open what was
    /// said, `match_wake` otherwise.
    pub fn matches(&self, text: &str, fuzzy: f64) -> Option<WakeMatch> {
        if self.opening {
            match_opening(text, &self.phrase, fuzzy, OPENERS)
        } else {
            match_wake(text, &[self.phrase.as_str()], fuzzy)
        }
    }
}

struct Classifier {
    name: String,
    input: String,
    session: Session,
}

/// The models, loaded once: `score` takes the last 2 s and returns each model's score
/// (0-1) by name. The audio features are computed once for all of them.
pub struct WakeScorer {
    mel: Session,
    mel_input: String,
    embedding: Session,
    embedding_input: String,
    classifiers: Vec<Classifier>,
}

impl WakeScorer {
    /// `mel` and `embedding` are livekit-wakeword's `melspectrogram.onnx` and
    /// `embedding_model.onnx`; `models` the classifiers (`hey_ghost.onnx`, `ghost_*.onnx`).
    /// One intra-op thread by default in Ghost.
    pub fn load(mel: &Path, embedding: &Path, models: &[PathBuf], threads: usize) -> Result<Self> {
        let mut config = SessionConfig::threads(threads);
        // livekit-wakeword turns KleidiAI off on every session (its batch-1 convolutions
        // return wrong results on arm64); x86 ignores the entry.
        config.entries.push(("mlas.disable_kleidiai", "1"));
        let mel_session = session::open(mel, &config)?;
        let embedding_session = session::open(embedding, &config)?;
        let mut classifiers = Vec::with_capacity(models.len());
        for path in models {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let session = session::open(path, &config)?;
            let input = session::first_input_name(&session)?;
            classifiers.push(Classifier {
                name,
                input,
                session,
            });
        }
        Ok(Self {
            mel_input: session::first_input_name(&mel_session)?,
            mel: mel_session,
            embedding_input: session::first_input_name(&embedding_session)?,
            embedding: embedding_session,
            classifiers,
        })
    }

    /// The models' names, in load order.
    pub fn names(&self) -> Vec<&str> {
        self.classifiers.iter().map(|c| c.name.as_str()).collect()
    }

    /// Score 2 s of 16 kHz int16 PCM.
    pub fn score(&mut self, window: &[i16]) -> Result<Vec<(String, f32)>> {
        self.score_f32(&crate::audio::to_f32(window))
    }

    /// Score 2 s of float32 PCM (`WakeWordModel.predict`).
    pub fn score_f32(&mut self, window: &[f32]) -> Result<Vec<(String, f32)>> {
        if self.classifiers.is_empty() {
            return Ok(Vec::new());
        }
        let zeros = |s: &Self| {
            s.classifiers
                .iter()
                .map(|c| (c.name.clone(), 0.0))
                .collect()
        };
        let outputs = self.mel.run(ort::inputs![
            self.mel_input.as_str() => Tensor::from_array(([1usize, window.len()], window.to_vec()))?,
        ])?;
        let (shape, mel) = session::first_output_f32(&outputs)?;
        drop(outputs);
        let frames = match shape.as_slice() {
            [_, _, t, b] | [_, t, b] if *b as usize == MEL_BINS => *t as usize,
            _ => {
                return Err(InferError::Model(format!(
                    "melspectrogram output has shape {shape:?}"
                )));
            }
        };
        if frames < EMBEDDING_WINDOW {
            return Ok(zeros(self));
        }
        let mel: Vec<f32> = mel.iter().map(|v| v / 10.0 + 2.0).collect();
        let mut embeddings: Vec<Vec<f32>> = Vec::new();
        let mut start = 0;
        while start + EMBEDDING_WINDOW <= frames {
            let slice = mel[start * MEL_BINS..(start + EMBEDDING_WINDOW) * MEL_BINS].to_vec();
            let outputs = self.embedding.run(ort::inputs![
                self.embedding_input.as_str() => Tensor::from_array(([1usize, EMBEDDING_WINDOW, MEL_BINS, 1], slice))?,
            ])?;
            let (_, emb) = session::first_output_f32(&outputs)?;
            if emb.len() != EMBEDDING_DIM {
                return Err(InferError::Model(format!(
                    "embedding has {} values",
                    emb.len()
                )));
            }
            embeddings.push(emb);
            start += EMBEDDING_STRIDE;
        }
        if embeddings.len() < MIN_EMBEDDINGS {
            return Ok(zeros(self));
        }
        let last: Vec<f32> = embeddings[embeddings.len() - MIN_EMBEDDINGS..].concat();
        let mut scores = Vec::with_capacity(self.classifiers.len());
        for c in &mut self.classifiers {
            let outputs = c.session.run(ort::inputs![
                c.input.as_str() => Tensor::from_array(([1usize, MIN_EMBEDDINGS, EMBEDDING_DIM], last.clone()))?,
            ])?;
            let (_, out) = session::first_output_f32(&outputs)?;
            scores.push((c.name.clone(), out.first().copied().unwrap_or(0.0)));
        }
        Ok(scores)
    }
}

/// A detector's decision on one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Index into the detector's models.
    pub model: usize,
    pub name: String,
    pub score: f32,
    /// The sample index the scored window ended at.
    pub end: usize,
}

/// The model that fired highest among those at or over their threshold
/// (`DetectorWakeProcessor._score`): its score and index, the first on a tie.
pub fn pick_hit(scores: &[(String, f32)], models: &[WakeModel]) -> Option<(f32, usize)> {
    let mut best: Option<(f32, usize)> = None;
    for (i, m) in models.iter().enumerate() {
        let score = scores
            .iter()
            .find(|(n, _)| *n == m.name)
            .map(|(_, s)| *s)
            .unwrap_or(0.0);
        if score >= m.floor() && best.is_none_or(|(b, _)| score > b) {
            best = Some((score, i));
        }
    }
    best
}

/// Where Parakeet's check reads from (`DetectorWakeProcessor._heard`): from where the
/// operator started talking, or just before the hit's phrase if that is earlier, and no
/// further back than `MAX_PRE_SECS` before the hit.
pub fn heard_from(hit: usize, run_start: Option<usize>, opening: bool) -> usize {
    let pre = if opening { OPENING_PRE_SECS } else { PRE_SECS };
    let mut start = hit as i64 - (pre * SAMPLE_RATE as f64) as i64;
    if let Some(run) = run_start {
        start = start
            .min(run as i64)
            .max(hit as i64 - (MAX_PRE_SECS * SAMPLE_RATE as f64) as i64);
    }
    start.max(0) as usize
}

/// The scoring cadence of the passive processor: audio in, a score every `STEP` once
/// `WINDOW` is in, a hit when a model clears its threshold outside the cooldown. After a
/// hit, scoring pauses until `clear_hit` (the pipeline confirms or drops it), as the
/// Python's `_hit` does.
pub struct WakeDetector {
    scorer: WakeScorer,
    models: Vec<WakeModel>,
    cooldown: usize,
    buf: Vec<i16>,
    base: usize,
    n: usize,
    scored: usize,
    last_hit: i64,
    pending: bool,
}

impl WakeDetector {
    pub fn new(scorer: WakeScorer, models: Vec<WakeModel>, cooldown_secs: f64) -> Self {
        Self {
            scorer,
            models,
            cooldown: (cooldown_secs * SAMPLE_RATE as f64) as usize,
            buf: Vec::new(),
            base: 0,
            n: 0,
            scored: 0,
            last_hit: -(1i64 << 40),
            pending: false,
        }
    }

    pub fn models(&self) -> &[WakeModel] {
        &self.models
    }

    /// Samples received so far.
    pub fn received(&self) -> usize {
        self.n
    }

    /// Feed audio; the first hit in it, if any. Keeps `keep` samples of history.
    pub fn push(&mut self, audio: &[i16], keep: usize) -> Result<Option<Hit>> {
        self.buf.extend_from_slice(audio);
        self.n += audio.len();
        if self.buf.len() > keep.max(WINDOW) {
            let drop = self.buf.len() - keep.max(WINDOW);
            self.buf.drain(..drop);
            self.base += drop;
        }
        let mut hit = None;
        while self.n - self.scored >= STEP {
            self.scored += STEP;
            if self.pending || self.scored < WINDOW || hit.is_some() {
                continue;
            }
            let end = self.scored;
            if end < WINDOW + self.base {
                continue;
            }
            let window = &self.buf[end - WINDOW - self.base..end - self.base];
            let scores = self.scorer.score(window)?;
            let Some((score, model)) = pick_hit(&scores, &self.models) else {
                continue;
            };
            if (end as i64 - self.last_hit) < self.cooldown as i64 {
                continue;
            }
            self.last_hit = end as i64;
            self.pending = true;
            hit = Some(Hit {
                model,
                name: self.models[model].name.clone(),
                score,
                end,
            });
        }
        Ok(hit)
    }

    /// The hit was handled (confirmed and finished, or dropped): score again.
    pub fn clear_hit(&mut self) {
        self.pending = false;
    }

    /// The samples from `from` (an absolute index) to now, as float32 for Parakeet.
    pub fn audio_from(&self, from: usize) -> Vec<f32> {
        let start = from.saturating_sub(self.base).min(self.buf.len());
        crate::audio::to_f32(&self.buf[start..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str, threshold: f32) -> WakeModel {
        WakeModel::known(Path::new(&format!("/x/{name}.onnx")), threshold, None)
    }

    #[test]
    fn model_meaning_by_stem() {
        assert_eq!(model_meaning("hey_ghost"), ("hey ghost", false));
        assert_eq!(model_meaning("ghost_address"), ("ghost", true));
        assert_eq!(model_meaning("ghost_need"), ("ghost", true));
        let m = model("ghost_work", 0.3);
        assert!(m.opening && m.phrase == "ghost" && m.name == "ghost_work");
    }

    #[test]
    fn a_weak_score_is_a_hit_only_with_a_checked_floor_and_then_waits_for_parakeet() {
        let plain = model("hey_ghost", 0.7);
        let scores = vec![("hey_ghost".to_string(), 0.4)];
        assert_eq!(pick_hit(&scores, std::slice::from_ref(&plain)), None);
        let tiered = plain.checked_from(Some(0.3));
        assert_eq!(
            pick_hit(&scores, std::slice::from_ref(&tiered)),
            Some((0.4, 0))
        );
        assert!(tiered.needs_check(0.4, false)); // weak: checked whatever the rule
        assert!(!tiered.needs_check(0.8, false)); // strong: the detector's rule
        assert!(tiered.needs_check(0.8, true));
        // a floor above the threshold changes nothing
        assert_eq!(model("hey_ghost", 0.3).checked_from(Some(0.5)).floor(), 0.3);
    }

    #[test]
    fn pick_hit_takes_the_highest_over_its_threshold() {
        let models = vec![model("hey_ghost", 0.5), model("ghost_address", 0.7)];
        let scores = vec![
            ("hey_ghost".to_string(), 0.6),
            ("ghost_address".to_string(), 0.65),
        ];
        assert_eq!(pick_hit(&scores, &models), Some((0.6, 0)));
        let scores = vec![
            ("hey_ghost".to_string(), 0.2),
            ("ghost_address".to_string(), 0.9),
        ];
        assert_eq!(pick_hit(&scores, &models), Some((0.9, 1)));
        let scores = vec![("hey_ghost".to_string(), 0.4)];
        assert_eq!(pick_hit(&scores, &models), None);
        let tie = vec![
            ("hey_ghost".to_string(), 0.8),
            ("ghost_address".to_string(), 0.8),
        ];
        assert_eq!(pick_hit(&tie, &models), Some((0.8, 0)));
    }

    #[test]
    fn heard_from_starts_where_the_speech_did() {
        let hit = 10 * SAMPLE_RATE;
        assert_eq!(heard_from(hit, None, false), 8 * SAMPLE_RATE);
        assert_eq!(heard_from(hit, None, true), hit - 72_000);
        assert_eq!(
            heard_from(hit, Some(3 * SAMPLE_RATE), false),
            4 * SAMPLE_RATE
        ); // capped at 6 s before
        assert_eq!(
            heard_from(hit, Some(9 * SAMPLE_RATE), false),
            8 * SAMPLE_RATE
        ); // the earlier of the two
        assert_eq!(heard_from(SAMPLE_RATE, None, true), 0);
    }
}
