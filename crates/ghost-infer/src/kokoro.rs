//! Kokoro v1.0 as `kokoro-onnx` runs it for `ghost_voice.kokoro`: text to phonemes through
//! espeak-ng (`phonemize`), phonemes to token ids over the model's vocabulary, the style
//! vector for the phoneme count from the voices file, the model (`tokens`, `style`,
//! `speed` -> 24 kHz float audio), librosa's `trim` of the silence the model leaves at the
//! ends, and the pause after a batch that ends on a sentence or clause mark. Measured on
//! the target CPU: the first sentence of a reply in about 0.37 s at roughly 4x real time
//! (docs/benchmarks/2026-10-16-speech-cpu.md). ONNX Runtime's default threads, as
//! `kokoro-onnx` leaves them.
//!
//! Ghost hands Kokoro a clause at a time (`clauses`), so the first clause's audio comes
//! out before the rest is rendered: `stream`.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
};

use ort::{session::Session, value::Tensor};

use crate::{
    InferError, Result,
    clauses::ClauseAggregator,
    espeak::Espeak,
    npy, phonemize,
    session::{self, SessionConfig},
};

#[path = "kokoro_vocab.rs"]
mod vocab;

pub const SAMPLE_RATE: u32 = 24_000;
/// The model's context, in phonemes.
pub const MAX_PHONEME_LENGTH: usize = 510;
/// Silence after a batch that ends a sentence, and one that ends a clause.
pub const SENTENCE_PAUSE: f32 = 0.25;
pub const CLAUSE_PAUSE: f32 = 0.1;
const SENTENCE_MARKS: &[char] = &['.', '!', '?', '…'];
const CLAUSE_MARKS: &[char] = &[',', ';', ':'];

/// The model files.
#[derive(Debug, Clone)]
pub struct KokoroPaths {
    pub model: PathBuf,
    pub voices: PathBuf,
    /// espeak-ng's data folder; `None` for the library's own.
    pub espeak_data: Option<PathBuf>,
}

impl KokoroPaths {
    /// `~/.cache/ghost/kokoro/`, where Ghost keeps them.
    pub fn in_cache() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        let dir = PathBuf::from(home).join(".cache/ghost/kokoro");
        Some(Self {
            model: dir.join("kokoro-v1.0.onnx"),
            voices: dir.join("voices-v1.0.bin"),
            espeak_data: None,
        })
    }
}

/// The loaded model with its voices and espeak-ng.
pub struct Kokoro {
    session: Session,
    voices: BTreeMap<String, Vec<f32>>,
    vocab: HashMap<char, i64>,
    espeak: &'static Espeak,
}

/// One rendered piece of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    /// The clause or sentence it renders.
    pub text: String,
    /// 24 kHz mono int16.
    pub pcm: Vec<i16>,
}

impl Kokoro {
    pub fn load(paths: &KokoroPaths) -> Result<Self> {
        let session = session::open(
            &paths.model,
            &SessionConfig {
                intra_threads: 0,
                inter_threads: 0,
                entries: Vec::new(),
            },
        )?;
        let mut voices = BTreeMap::new();
        for (name, array) in npy::read_npz_stored(&paths.voices)? {
            let name = name.strip_suffix(".npy").unwrap_or(&name).to_string();
            let data = array.to_f32()?;
            if array.shape.len() != 3 || array.shape[2] != 256 {
                return Err(InferError::Model(format!(
                    "{}: voice {name} has shape {:?}",
                    paths.voices.display(),
                    array.shape
                )));
            }
            voices.insert(name, data);
        }
        Ok(Self {
            session,
            voices,
            vocab: vocab::VOCAB.iter().copied().collect(),
            espeak: Espeak::init(paths.espeak_data.as_deref())?,
        })
    }

    /// The voices in the file, sorted.
    pub fn voices(&self) -> Vec<&str> {
        self.voices.keys().map(String::as_str).collect()
    }

    pub fn has_voice(&self, name: &str) -> bool {
        self.voices.contains_key(name)
    }

    pub fn espeak(&self) -> &'static Espeak {
        self.espeak
    }

    /// `Tokenizer.phonemize`: espeak-ng through phonemizer, then only the phonemes the
    /// model knows, stripped.
    pub fn phonemize(&self, text: &str, lang: &str) -> Result<String> {
        let phonemes = phonemize::phonemize(self.espeak, text.trim(), lang)?;
        Ok(phonemes
            .chars()
            .filter(|c| self.vocab.contains_key(c))
            .collect::<String>()
            .trim()
            .to_string())
    }

    /// `Tokenizer.tokenize`: token ids, anything outside the vocabulary dropped.
    pub fn tokenize(&self, phonemes: &str) -> Result<Vec<i64>> {
        let n = phonemes.chars().count();
        if n > MAX_PHONEME_LENGTH {
            return Err(InferError::Model(format!(
                "text is too long, must be less than {MAX_PHONEME_LENGTH} phonemes"
            )));
        }
        Ok(phonemes
            .chars()
            .filter_map(|c| self.vocab.get(&c).copied())
            .collect())
    }

    /// The style row for `tokens` phonemes: `voice[min(n, 510) - 1]`.
    pub fn style(&self, voice: &str, tokens: usize) -> Result<&[f32]> {
        let v = self.voices.get(voice).ok_or_else(|| {
            InferError::Model(format!("Voice {voice} not found in available voices"))
        })?;
        let rows = v.len() / 256;
        let row = tokens.min(rows).max(1) - 1;
        Ok(&v[row * 256..(row + 1) * 256])
    }

    /// `Kokoro._infer`: the model on one window of tokens, padded with the model's 0.
    pub fn infer(&mut self, tokens: &[i64], style: &[f32], speed: f32) -> Result<Vec<f32>> {
        let mut padded = Vec::with_capacity(tokens.len() + 2);
        padded.push(0);
        padded.extend_from_slice(tokens);
        padded.push(0);
        let n = padded.len();
        let outputs = self.session.run(ort::inputs![
            "tokens" => Tensor::from_array(([1usize, n], padded))?,
            "style" => Tensor::from_array(([1usize, 256], style.to_vec()))?,
            "speed" => Tensor::from_array(([1usize], vec![speed]))?,
        ])?;
        let (_, audio) = session::first_output_f32(&outputs)?;
        Ok(audio)
    }

    /// `Kokoro._create_batch`: one batch of phonemes, trimmed, with `pause` seconds of
    /// silence after it when trimming.
    pub fn synthesize_batch(
        &mut self,
        phonemes: &str,
        voice: &str,
        speed: f32,
        trim: bool,
        pause: f32,
    ) -> Result<Vec<f32>> {
        let tokens = self.tokenize(phonemes)?;
        if tokens.is_empty() {
            return Err(InferError::Model(format!(
                "No phonemes of {phonemes:?} are in the model vocabulary"
            )));
        }
        let style = self.style(voice, tokens.len())?.to_vec();
        let mut audio = self.infer(&tokens, &style, speed)?;
        if trim {
            let (start, end) = trim_silence(&audio);
            audio = audio[start..end].to_vec();
            if pause > 0.0 {
                audio.extend(std::iter::repeat_n(
                    0.0,
                    (pause * SAMPLE_RATE as f32) as usize,
                ));
            }
        }
        Ok(audio)
    }

    /// `Kokoro._prepare` without the voice: the batches and the pause after each.
    pub fn batches(&self, text: &str, lang: &str) -> Result<Vec<(String, f32)>> {
        let phonemes = self.phonemize(text, lang)?;
        let phonemes = phonemes.split_whitespace().collect::<Vec<_>>().join(" ");
        let batched = split_phonemes(&phonemes, MAX_PHONEME_LENGTH);
        if batched.is_empty() {
            return Err(InferError::Model(format!(
                "Nothing to synthesize, {text:?} produced no phonemes"
            )));
        }
        let last = batched.len() - 1;
        Ok(batched
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let pause = if i == last {
                    0.0
                } else {
                    pause_after(&b, SENTENCE_PAUSE, CLAUSE_PAUSE)
                };
                (b, pause)
            })
            .collect())
    }

    /// `Kokoro.create(text, voice, speed, lang)`: the whole text as 24 kHz float audio.
    pub fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
        lang: &str,
    ) -> Result<Vec<f32>> {
        if !(0.5..=2.0).contains(&speed) {
            return Err(InferError::Model(format!(
                "Speed should be between 0.5 and 2.0, got {speed}"
            )));
        }
        if !self.has_voice(voice) {
            return Err(InferError::Model(format!(
                "Voice {voice} not found in available voices"
            )));
        }
        let mut out = Vec::new();
        for (phonemes, pause) in self.batches(text, lang)? {
            out.extend(self.synthesize_batch(&phonemes, voice, speed, true, pause)?);
        }
        Ok(out)
    }

    /// The reply a clause at a time (`ClauseTextAggregator` with `clause_min_words`;
    /// `None` for whole sentences), each rendered when the iterator reaches it, so the
    /// first clause's audio comes out before the rest is rendered.
    pub fn stream<'a>(
        &'a mut self,
        text: &str,
        voice: &str,
        speed: f32,
        lang: &str,
        clause_min_words: Option<usize>,
    ) -> Result<Stream<'a>> {
        if !self.has_voice(voice) {
            return Err(InferError::Model(format!(
                "Voice {voice} not found in available voices"
            )));
        }
        let mut aggregator = ClauseAggregator::new(clause_min_words);
        let mut pieces: VecDeque<String> = aggregator.push(text).into();
        pieces.extend(aggregator.flush());
        Ok(Stream {
            kokoro: self,
            pieces,
            batches: VecDeque::new(),
            voice: voice.to_string(),
            speed,
            lang: lang.to_string(),
        })
    }
}

/// `Kokoro::stream`'s pieces, rendered on demand.
pub struct Stream<'a> {
    kokoro: &'a mut Kokoro,
    pieces: VecDeque<String>,
    batches: VecDeque<(String, String, f32)>,
    voice: String,
    speed: f32,
    lang: String,
}

impl Iterator for Stream<'_> {
    type Item = Result<Spoken>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((text, phonemes, pause)) = self.batches.pop_front() {
                return Some(
                    self.kokoro
                        .synthesize_batch(&phonemes, &self.voice, self.speed, true, pause)
                        .map(|pcm| Spoken {
                            text,
                            pcm: crate::audio::to_i16(&pcm),
                        }),
                );
            }
            let text = self.pieces.pop_front()?;
            match self.kokoro.batches(&text, &self.lang) {
                Ok(batches) => {
                    for (phonemes, pause) in batches {
                        self.batches.push_back((text.clone(), phonemes, pause));
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Seconds of silence a batch ending with this text should be followed by
/// (`kokoro_onnx.chunker.pause_after`).
pub fn pause_after(phonemes: &str, sentence: f32, clause: f32) -> f32 {
    match phonemes.trim_end().chars().last() {
        Some(c) if SENTENCE_MARKS.contains(&c) => sentence,
        Some(c) if CLAUSE_MARKS.contains(&c) => clause,
        _ => 0.0,
    }
}

/// `re.split(r"(?<=[marks])\s+", text)`: cut at every whitespace run right after a mark.
fn split_after(text: &str, marks: &[char]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut piece = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() && i > 0 && (marks.is_empty() || marks.contains(&chars[i - 1]))
        {
            pieces.push(std::mem::take(&mut piece));
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            continue;
        }
        piece.push(chars[i]);
        i += 1;
    }
    pieces.push(piece);
    pieces
}

/// `chunker._atoms`: pieces of `phonemes` no longer than `max_length`, cut at the least
/// disruptive boundary available (sentence, clause, word, then mid-word).
fn atoms(phonemes: &str, max_length: usize, level: usize, out: &mut Vec<String>) {
    if phonemes.chars().count() <= max_length {
        if !phonemes.is_empty() {
            out.push(phonemes.to_string());
        }
        return;
    }
    let boundaries: [&[char]; 3] = [SENTENCE_MARKS, CLAUSE_MARKS, &[]];
    for (index, marks) in boundaries.iter().enumerate().skip(level) {
        let pieces = split_after(phonemes, marks);
        if pieces.len() > 1 {
            for piece in pieces {
                atoms(piece.trim(), max_length, index + 1, out);
            }
            return;
        }
    }
    let chars: Vec<char> = phonemes.chars().collect();
    for start in (0..chars.len()).step_by(max_length) {
        out.push(
            chars[start..(start + max_length).min(chars.len())]
                .iter()
                .collect(),
        );
    }
}

/// `chunker._pack`: consecutive atoms grouped into batches within `limit`, as index ranges.
fn pack(lengths: &[usize], limit: usize) -> Vec<(usize, usize)> {
    let mut batches = Vec::new();
    let (mut start, mut size) = (0usize, 0usize);
    for (index, &length) in lengths.iter().enumerate() {
        let candidate = if index == start {
            length
        } else {
            size + 1 + length
        };
        if candidate > limit && index > start {
            batches.push((start, index));
            start = index;
            size = length;
        } else {
            size = candidate;
        }
    }
    if !lengths.is_empty() {
        batches.push((start, lengths.len()));
    }
    batches
}

/// `chunker.split_phonemes`: batches of at most `max_length`, balanced so a short last
/// batch isn't spoken at a different rate and loudness than its neighbours.
pub fn split_phonemes(phonemes: &str, max_length: usize) -> Vec<String> {
    let mut parts = Vec::new();
    atoms(phonemes.trim(), max_length, 0, &mut parts);
    if parts.is_empty() {
        return Vec::new();
    }
    let lengths: Vec<usize> = parts.iter().map(|a| a.chars().count()).collect();
    let fewest = pack(&lengths, max_length).len();
    let (mut low, mut high) = (*lengths.iter().max().expect("non-empty"), max_length);
    while low < high {
        let middle = (low + high) / 2;
        if pack(&lengths, middle).len() <= fewest {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    pack(&lengths, low)
        .into_iter()
        .map(|(s, e)| parts[s..e].join(" "))
        .collect()
}

/// `librosa.effects.trim(y)` with its defaults (top_db 60, frames of 2048 every 512, the
/// peak RMS as reference): the non-silent interval `[start, end)`.
pub fn trim_silence(y: &[f32]) -> (usize, usize) {
    const FRAME: usize = 2048;
    const HOP: usize = 512;
    const TOP_DB: f32 = 60.0;
    const AMIN: f32 = 1e-10;
    let pad = FRAME / 2;
    let padded_len = y.len() + 2 * pad;
    if padded_len < FRAME {
        return (0, 0);
    }
    let frames = 1 + (padded_len - FRAME) / HOP;
    let sample = |i: usize| -> f32 {
        if i < pad || i >= pad + y.len() {
            0.0
        } else {
            y[i - pad]
        }
    };
    let mut rms = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * HOP;
        let power: f64 = (start..start + FRAME)
            .map(|i| {
                let v = sample(i);
                f64::from(v * v)
            })
            .sum::<f64>()
            / FRAME as f64;
        rms.push((power as f32).sqrt());
    }
    let reference = rms.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let ref_db = 10.0 * (reference * reference).max(AMIN).log10();
    let non_silent: Vec<bool> = rms
        .iter()
        .map(|&r| 10.0 * (r * r).max(AMIN).log10() - ref_db > -TOP_DB)
        .collect();
    let first = non_silent.iter().position(|&b| b);
    let last = non_silent.iter().rposition(|&b| b);
    match (first, last) {
        (Some(first), Some(last)) => (first * HOP, y.len().min((last + 1) * HOP)),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_follow_the_last_mark() {
        assert_eq!(pause_after("dˈʌn.", 0.25, 0.1), 0.25);
        assert_eq!(pause_after("dˈʌn, ", 0.25, 0.1), 0.1);
        assert_eq!(pause_after("dˈʌn", 0.25, 0.1), 0.0);
        assert_eq!(pause_after("", 0.25, 0.1), 0.0);
    }

    #[test]
    fn short_phonemes_are_one_batch() {
        assert_eq!(split_phonemes("a b c.", 510), vec!["a b c.".to_string()]);
        assert_eq!(split_phonemes("   ", 510), Vec::<String>::new());
    }

    #[test]
    fn long_phonemes_split_at_sentences_then_clauses_then_words() {
        let sentence = "aaaa bbbb cccc. ";
        let text = sentence.repeat(10).trim().to_string(); // 159 chars
        let out = split_phonemes(&text, 40);
        assert!(out.iter().all(|b| b.chars().count() <= 40), "{out:?}");
        assert!(out.iter().all(|b| b.ends_with('.')), "{out:?}");
        assert_eq!(out.concat().replace(' ', ""), text.replace(' ', ""));
        // Balanced: the batches are near equal.
        let lens: Vec<usize> = out.iter().map(|b| b.len()).collect();
        assert!(
            lens.iter().max().unwrap() - lens.iter().min().unwrap() <= 17,
            "{lens:?}"
        );
        let words = "ab ".repeat(30).trim().to_string();
        let out = split_phonemes(&words, 10);
        assert!(out.iter().all(|b| b.len() <= 10), "{out:?}");
        let run = "x".repeat(25);
        assert_eq!(
            split_phonemes(&run, 10),
            vec!["x".repeat(10), "x".repeat(10), "x".repeat(5)]
        );
    }

    #[test]
    fn split_after_cuts_only_after_marks() {
        assert_eq!(
            split_after("a. b, c d", SENTENCE_MARKS),
            vec!["a.", "b, c d"]
        );
        assert_eq!(split_after("a. b, c d", CLAUSE_MARKS), vec!["a. b,", "c d"]);
        assert_eq!(split_after("a. b,  c d", &[]), vec!["a.", "b,", "c", "d"]);
    }

    #[test]
    fn pack_groups_within_the_limit() {
        assert_eq!(pack(&[3, 3, 3], 7), vec![(0, 2), (2, 3)]);
        assert_eq!(pack(&[8, 1], 7), vec![(0, 1), (1, 2)]);
        assert_eq!(pack(&[], 7), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn trim_cuts_the_quiet_ends() {
        let mut y = vec![0.0f32; 24_000];
        for (i, v) in y.iter_mut().enumerate().take(18_000).skip(6_000) {
            *v = 0.5 * ((i as f32) * 0.3).sin();
        }
        let (start, end) = trim_silence(&y);
        assert!(start <= 6_000 && start > 4_000, "{start}");
        assert!((18_000..20_000).contains(&end), "{end}");
        assert_eq!(trim_silence(&[0.0; 100]), (0, 100)); // all equal: nothing is quieter than the peak
        assert_eq!(trim_silence(&[]), (0, 0));
    }
}
