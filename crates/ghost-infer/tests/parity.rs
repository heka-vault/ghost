//! Parity with the Python daemon's models (ADR 0039): every test reads the fixtures
//! `scripts/fixtures/record.py` wrote from the Python wrappers over the same model files,
//! runs the Rust port on the same clips, and compares: Parakeet word for word, Silero and
//! Smart Turn within 1e-3 with the same verdicts, the wake scores within 1e-3 with the same
//! hits at the production thresholds, Kokoro's phonemes identical and its PCM within 1e-3
//! RMS. Each prints the matches out of the total and p50/p95 times on this CPU.
//!
//! The tests are ignored unless the fixture folder exists (`GHOST_FIXTURES`, or
//! `~/projects/active/ghost/.scratch/spike-fixtures`), so CI without fixtures stays green:
//!
//! ```text
//! cargo nextest run -p ghost-infer --release --run-ignored all --no-capture
//! ```
//!
//! The model files are read where the oracle has them: the Hugging Face cache for
//! Parakeet, the oracle's `site-packages` (`tools/oracle/.venv`, `GHOST_SITE_PACKAGES`) for
//! Silero, Smart Turn and the wake front end, `~/.cache/ghost/` for the wake classifiers
//! and Kokoro.
//!
//! Two runtimes. The Python ran ONNX Runtime 1.24.4; `ort` 2.0.0-rc.15's own download is
//! 1.31.0. On the Python's library every port is the oracle's: Parakeet 225/225
//! transcripts, and Kokoro's audio, Silero's probabilities, Smart Turn's probabilities and
//! the wake scores bit for bit. On 1.31.0 Silero and the wake chain stay there, and the
//! two int8 graphs and the vocoder move with the runtime's kernels: Parakeet flips a comma
//! or a capital on about one clip in twenty (its features are within 3e-6 of NumPy's, the
//! float32 log's ulp, and the Python model fed the Rust features matches its own
//! transcripts on every clip, `scripts/fixtures/check_parakeet.py`), Smart Turn's
//! probabilities spread while 99 % of its verdicts agree, and Kokoro's audio differs by a
//! few 1e-3 RMS as the vocoder integrates phase. So Ghost runs 1.24.4: the crate loads the
//! library at run time from Ghost's cache (`scripts/onnxruntime.sh` fetches it) or the
//! bundle, `ORT_DYLIB_PATH` overriding (`src/runtime.rs`), and the strict run is the
//! ordinary one with the strict bars on:
//!
//! ```text
//! GHOST_PARITY_STRICT=1 cargo nextest run -p ghost-infer --release --run-ignored all --no-capture
//! ```
//!
//! With `GHOST_PARITY_STRICT` the bars are the oracle's: every Parakeet transcript, Kokoro
//! within 1e-3 RMS at the same length, Smart Turn within 1e-3 with every verdict the same.
//! Without it they are what 1.31.0 reaches (`ORT_DYLIB_PATH` pointed at one): Parakeet
//! matching by words (case and punctuation aside) on all but one clip in a hundred and
//! exactly on nine in ten, Kokoro within 1e-2 RMS, 99 % of Smart Turn's verdicts.
//! `GHOST_PARITY_TIMING` adds the speed bars of ADR 0004 on an idle CPU.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use ghost_infer::{
    Kokoro, KokoroPaths, Parakeet, ParakeetPaths, Silero, SmartTurn, VadAnalyzer, VadParams,
    WakeModel, WakeScorer, audio, kokoro, npy,
    wake::{STEP, WINDOW, pick_hit},
};
use serde::Deserialize;

const FIXTURES_DEFAULT: &str = "/home/detroitpro/projects/active/ghost/.scratch/spike-fixtures";
/// The oracle's venv in this checkout (`uv sync --project tools/oracle`).
const SITE_PACKAGES_DEFAULT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tools/oracle/.venv/lib/python3.12/site-packages"
);

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("GHOST_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(FIXTURES_DEFAULT));
    if dir.join("clips.json").exists() {
        Some(dir)
    } else {
        eprintln!(
            "no fixtures at {}; run scripts/fixtures/record.py",
            dir.display()
        );
        None
    }
}

fn site_packages() -> PathBuf {
    std::env::var_os("GHOST_SITE_PACKAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SITE_PACKAGES_DEFAULT))
}

#[derive(Deserialize)]
struct Clip {
    id: String,
    wav: PathBuf,
    samples: usize,
}

fn clips(dir: &Path) -> Vec<Clip> {
    serde_json::from_str(&fs::read_to_string(dir.join("clips.json")).unwrap()).unwrap()
}

fn json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    serde_json::from_str(
        &fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn percentiles(secs: &mut [f64]) -> (f64, f64) {
    if secs.is_empty() {
        return (0.0, 0.0);
    }
    secs.sort_by(f64::total_cmp);
    let p = |q: f64| secs[((secs.len() - 1) as f64 * q).round() as usize];
    (p(0.5), p(0.95))
}

/// Peak resident set size of this process, in GB, from `/proc/self/status`.
fn peak_rss_gb() -> f64 {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0 / 1024.0)
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

/// `GHOST_PARITY_STRICT`: the bars for the Python's own ONNX Runtime.
fn strict() -> bool {
    env_flag("GHOST_PARITY_STRICT")
}

/// `GHOST_PARITY_TIMING`: assert the speed bars of ADR 0004 too (a run on an idle CPU;
/// the numbers are printed either way).
fn timing() -> bool {
    env_flag("GHOST_PARITY_TIMING")
}

/// Words, lowercased, punctuation and hyphens aside: what a transcript says.
fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && !c.is_whitespace(), " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[derive(Deserialize)]
struct ParakeetFixture {
    text: String,
    tokens: Vec<String>,
}

#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn parakeet_matches_word_for_word() {
    let Some(dir) = fixtures() else { return };
    let paths = ParakeetPaths::huggingface_cache()
        .expect("the Parakeet snapshot in the Hugging Face cache");
    let t0 = Instant::now();
    let mut parakeet = Parakeet::load(&paths, 4).unwrap();
    eprintln!("parakeet: loaded in {:.1}s", t0.elapsed().as_secs_f64());
    let clips = clips(&dir);
    let mut matches = 0;
    let mut word_matches = 0;
    let mut failures = Vec::new();
    let mut all = Vec::new();
    let mut two_seconds = Vec::new();
    for clip in &clips {
        let want: ParakeetFixture = json(&dir.join("parakeet").join(format!("{}.json", clip.id)));
        let pcm = audio::to_f32(&audio::read_wav_16k(&clip.wav).unwrap());
        let t = Instant::now();
        let got = parakeet.transcribe_tokens(&pcm).unwrap();
        let secs = t.elapsed().as_secs_f64();
        all.push(secs);
        if (1.5..=2.5).contains(&(clip.samples as f64 / 16000.0)) {
            two_seconds.push(secs);
        }
        if got.text == want.text {
            matches += 1;
        }
        if words_of(&got.text) == words_of(&want.text) {
            word_matches += 1;
        }
        if got.text != want.text {
            failures.push(format!(
                "{}: python {:?} {:?}\n    rust   {:?} {:?}",
                clip.id, want.text, want.tokens, got.text, got.tokens
            ));
        }
    }
    let (p50, p95) = percentiles(&mut all);
    let (p50_2s, p95_2s) = percentiles(&mut two_seconds);
    let n = clips.len();
    eprintln!(
        "parakeet: {matches}/{n} transcripts match exactly, {word_matches}/{n} by words (case and punctuation \
         aside); all clips p50 {p50:.3}s p95 {p95:.3}s; 2 s utterances (n={}) p50 {p50_2s:.3}s p95 {p95_2s:.3}s; \
         peak RSS {:.2} GB",
        two_seconds.len(),
        peak_rss_gb()
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    if strict() {
        assert_eq!(matches, n, "{matches} of {n} transcripts match exactly");
    } else {
        assert!(
            word_matches * 100 >= n * 99,
            "{word_matches} of {n} transcripts match by words"
        );
        assert!(
            matches * 10 >= n * 9,
            "{matches} of {n} transcripts match exactly"
        );
    }
    if timing() {
        assert!(
            p50_2s <= 0.16,
            "a 2 s utterance takes {p50_2s:.3}s p50, over the 0.16 s of ADR 0004"
        );
        assert!(
            peak_rss_gb() < 1.7,
            "peak RSS {:.2} GB, over the 1.7 GB of ADR 0004",
            peak_rss_gb()
        );
    }
}

/// The filterbanks the crate computes against the tables the Python used (NeMo's
/// `nemo128`, Smart Turn's Whisper 80).
#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn filterbanks_match_the_python_tables() {
    let Some(dir) = fixtures() else { return };
    let nemo = npy::read(&dir.join("nemo128_fbanks.npy")).unwrap();
    assert_eq!(nemo.shape, vec![257, 128]);
    let ours = ghost_infer::parakeet::nemo128_table();
    let d = max_abs_diff(&ours, &nemo.to_f32().unwrap());
    assert_eq!(
        ours,
        nemo.to_f32().unwrap(),
        "the embedded nemo128 table is not the file's"
    );
    let whisper = npy::read(&dir.join("whisper80_fbanks.npy")).unwrap();
    assert_eq!(whisper.shape, vec![201, 80]);
    let ours_w = ghost_infer::mel::slaney_filterbank(201, 80, 0.0, 8000.0, 16000);
    let dw = ours_w
        .iter()
        .zip(whisper.to_f64().unwrap())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f64, f64::max);
    eprintln!("filterbanks: nemo128 within {d:.2e}, whisper80 within {dw:.2e}");
    assert_eq!(d, 0.0, "nemo128 differs by {d}");
    assert!(dw <= 1e-12, "whisper80 differs by {dw}");
}

/// `np.save` of a float32 array, for the Python-side diagnostics.
fn write_npy_f32(path: &Path, shape: &[usize], data: &[f32]) {
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    let mut header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}{}), }}",
        dims.join(", "),
        if shape.len() == 1 { "," } else { "" }
    );
    while !(10 + header.len() + 1).is_multiple_of(64) {
        header.push(' ');
    }
    header.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend((header.len() as u16).to_le_bytes());
    out.extend(header.as_bytes());
    out.extend(data.iter().flat_map(|v| v.to_le_bytes()));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, out).unwrap();
}

/// Writes the Rust port's Parakeet features for every clip under
/// `<fixtures>/rust-parakeet-features/`, for `scripts/fixtures/check_parakeet.py` to run
/// the Python model on and to compare with the NumPy preprocessor's.
#[test]
#[ignore = "a diagnostic: needs the parity fixtures"]
fn parakeet_dump_features() {
    let Some(dir) = fixtures() else { return };
    let paths = ParakeetPaths::huggingface_cache()
        .expect("the Parakeet snapshot in the Hugging Face cache");
    let mut parakeet = Parakeet::load(&paths, 4).unwrap();
    for clip in clips(&dir) {
        let pcm = audio::to_f32(&audio::read_wav_16k(&clip.wav).unwrap());
        let (features, frames, valid) = parakeet.features(&pcm).unwrap();
        let n_mels = features.len() / frames;
        write_npy_f32(
            &dir.join("rust-parakeet-features")
                .join(format!("{}.npy", clip.id)),
            &[n_mels, frames],
            &features,
        );
        write_npy_f32(
            &dir.join("rust-parakeet-features")
                .join(format!("{}.len.npy", clip.id)),
            &[1],
            &[valid as f32],
        );
    }
}

#[derive(Deserialize)]
struct SileroFixture {
    probs: Vec<f32>,
    confidences: Vec<f32>,
    volumes: Vec<f64>,
    states: Vec<u8>,
}

#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn silero_matches_within_1e_3() {
    let Some(dir) = fixtures() else { return };
    let model = site_packages().join("pipecat/audio/vad/data/silero_vad.onnx");
    let clips = clips(&dir);
    let (mut prob_ok, mut state_ok, mut conf_ok, mut vol_ok) = (0, 0, 0, 0);
    let mut worst_prob = 0.0f32;
    let mut worst_vol = 0.0f64;
    let mut secs = Vec::new();
    let mut failures = Vec::new();
    for clip in &clips {
        let want: SileroFixture = json(&dir.join("silero").join(format!("{}.json", clip.id)));
        let pcm = audio::read_wav_16k(&clip.wav).unwrap();
        let chunks: Vec<&[i16]> = pcm
            .as_chunks::<512>()
            .0
            .iter()
            .map(|c| c.as_slice())
            .collect();
        let mut raw = Silero::load(&model).unwrap();
        let mut probs = Vec::with_capacity(chunks.len());
        for c in &chunks {
            let t = Instant::now();
            probs.push(raw.probability(&audio::to_f32(c)).unwrap());
            secs.push(t.elapsed().as_secs_f64());
        }
        let d = max_abs_diff(&probs, &want.probs);
        worst_prob = worst_prob.max(d);
        if probs.len() == want.probs.len() && d <= 1e-3 {
            prob_ok += 1;
        } else {
            failures.push(format!("{}: probabilities differ by {d}", clip.id));
        }
        let mut analyzer = VadAnalyzer::new(Silero::load(&model).unwrap(), VadParams::default());
        let mut states = Vec::new();
        let mut confidences = Vec::new();
        let mut volumes = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let now = (i + 1) as f64 * 512.0 / 16000.0;
            let (conf, vol, state) = analyzer.analyze_chunk(c, now).unwrap();
            confidences.push(conf);
            volumes.push(vol);
            states.push(state as u8);
        }
        if states == want.states {
            state_ok += 1;
        } else {
            failures.push(format!(
                "{}: VAD states differ\n    python {:?}\n    rust   {:?}",
                clip.id, want.states, states
            ));
        }
        let dc = max_abs_diff(&confidences, &want.confidences);
        if dc <= 1e-3 {
            conf_ok += 1;
        } else {
            failures.push(format!("{}: analyzer confidences differ by {dc}", clip.id));
        }
        let dv = volumes
            .iter()
            .zip(&want.volumes)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        worst_vol = worst_vol.max(dv);
        if dv <= 1e-3 {
            vol_ok += 1;
        } else {
            failures.push(format!("{}: volumes differ by {dv}", clip.id));
        }
    }
    let (p50, p95) = percentiles(&mut secs);
    eprintln!(
        "silero: probabilities {prob_ok}/{} within 1e-3 (worst {worst_prob:.2e}); analyzer states {state_ok}/{}, \
         confidences {conf_ok}/{}, volumes {vol_ok}/{} (worst {worst_vol:.2e}); per chunk p50 {:.3}ms p95 {:.3}ms",
        clips.len(),
        clips.len(),
        clips.len(),
        clips.len(),
        p50 * 1000.0,
        p95 * 1000.0
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
}

#[derive(Deserialize)]
struct SmartTurnFixture {
    probability: f32,
    complete: bool,
}

#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn smart_turn_matches_within_1e_3() {
    let Some(dir) = fixtures() else { return };
    let model = site_packages().join("pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx");
    let mut smart_turn = SmartTurn::load(&model, 2).unwrap();
    let clips = clips(&dir);
    let (mut feat_ok, mut prob_ok, mut verdict_ok) = (0, 0, 0);
    let mut worst_feat = 0.0f32;
    let mut worst_prob = 0.0f32;
    let mut secs = Vec::new();
    let mut failures = Vec::new();
    for clip in &clips {
        let want: SmartTurnFixture =
            json(&dir.join("smart_turn").join(format!("{}.json", clip.id)));
        let want_features = npy::read(&dir.join("smart_turn").join(format!("{}.npy", clip.id)))
            .unwrap()
            .to_f32()
            .unwrap();
        let pcm = audio::to_f32(&audio::read_wav_16k(&clip.wav).unwrap());
        let t = Instant::now();
        let features = smart_turn.features(&pcm);
        let verdict = smart_turn.predict_features(&features).unwrap();
        secs.push(t.elapsed().as_secs_f64());
        let df = max_abs_diff(&features, &want_features);
        worst_feat = worst_feat.max(df);
        if features.len() == want_features.len() && df <= 1e-3 {
            feat_ok += 1;
        } else {
            failures.push(format!("{}: features differ by {df}", clip.id));
        }
        let dp = (verdict.probability - want.probability).abs();
        worst_prob = worst_prob.max(dp);
        if dp <= 1e-3 {
            prob_ok += 1;
        } else {
            failures.push(format!(
                "{}: probability {} vs python {}",
                clip.id, verdict.probability, want.probability
            ));
        }
        if verdict.complete == want.complete {
            verdict_ok += 1;
        } else {
            failures.push(format!(
                "{}: verdict {} vs python {}",
                clip.id, verdict.complete, want.complete
            ));
        }
    }
    let (p50, p95) = percentiles(&mut secs);
    let n = clips.len();
    eprintln!(
        "smart_turn: features {feat_ok}/{n} within 1e-3 (worst {worst_feat:.2e}); probabilities {prob_ok}/{n} \
         (worst {worst_prob:.2e}); verdicts {verdict_ok}/{n}; per turn p50 {p50:.3}s p95 {p95:.3}s"
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    assert_eq!(feat_ok, n, "features differ");
    if strict() {
        assert_eq!(verdict_ok, n, "verdicts differ");
        assert_eq!(prob_ok, n, "{prob_ok} of {n} probabilities within 1e-3");
    } else {
        assert!(
            verdict_ok * 100 >= n * 99,
            "{verdict_ok} of {n} verdicts agree"
        );
    }
}

#[derive(Deserialize)]
struct WakeHit {
    model: String,
    frame: usize,
}

#[derive(Deserialize)]
struct WakeFixture {
    padded: usize,
    first_frame: usize,
    scores: std::collections::BTreeMap<String, Vec<f32>>,
    hit_checked: Option<WakeHit>,
    hit_unchecked: Option<WakeHit>,
}

#[derive(Deserialize)]
struct WakeMeta {
    models: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct Meta {
    wake: WakeMeta,
}

#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn wake_scores_match_and_hits_agree() {
    let Some(dir) = fixtures() else { return };
    let meta: Meta = json(&dir.join("meta.json"));
    let resources = site_packages().join("livekit/wakeword/resources");
    let mut scorer = WakeScorer::load(
        &resources.join("melspectrogram.onnx"),
        &resources.join("embedding_model.onnx"),
        &meta.wake.models,
        1,
    )
    .unwrap();
    let checked: Vec<WakeModel> = meta
        .wake
        .models
        .iter()
        .map(|p| WakeModel::known(p, 0.3, None))
        .collect();
    let unchecked: Vec<WakeModel> = meta
        .wake
        .models
        .iter()
        .map(|p| WakeModel::known(p, 0.7, None))
        .collect();
    let clips = clips(&dir);
    let (mut score_ok, mut hit_ok) = (0, 0);
    let mut worst = 0.0f32;
    let mut frames_total = 0;
    let mut secs = Vec::new();
    let mut failures = Vec::new();
    let first_hit = |frames: &[Vec<(String, f32)>], models: &[WakeModel], first: usize| {
        frames.iter().enumerate().find_map(|(k, scores)| {
            pick_hit(scores, models).map(|(_, m)| (models[m].name.clone(), first + k))
        })
    };
    for clip in &clips {
        let want: WakeFixture = json(&dir.join("wake").join(format!("{}.json", clip.id)));
        let mut pcm = vec![0i16; want.padded];
        pcm.extend(audio::read_wav_16k(&clip.wav).unwrap());
        let mut frames = Vec::new();
        let mut k = WINDOW / STEP;
        while k * STEP <= pcm.len() {
            let t = Instant::now();
            frames.push(scorer.score(&pcm[k * STEP - WINDOW..k * STEP]).unwrap());
            secs.push(t.elapsed().as_secs_f64());
            k += 1;
        }
        frames_total += frames.len();
        let mut clip_worst = 0.0f32;
        let mut same_shape = true;
        for (name, want_scores) in &want.scores {
            let got: Vec<f32> = frames
                .iter()
                .map(|f| {
                    f.iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, s)| *s)
                        .unwrap_or(f32::NAN)
                })
                .collect();
            same_shape &= got.len() == want_scores.len();
            clip_worst = clip_worst.max(max_abs_diff(&got, want_scores));
        }
        worst = worst.max(clip_worst);
        if same_shape && clip_worst <= 1e-3 {
            score_ok += 1;
        } else {
            failures.push(format!(
                "{}: scores differ by {clip_worst} over {} frames",
                clip.id,
                frames.len()
            ));
        }
        let got_checked = first_hit(&frames, &checked, want.first_frame);
        let got_unchecked = first_hit(&frames, &unchecked, want.first_frame);
        let want_checked = want
            .hit_checked
            .as_ref()
            .map(|h| (h.model.clone(), h.frame));
        let want_unchecked = want
            .hit_unchecked
            .as_ref()
            .map(|h| (h.model.clone(), h.frame));
        if got_checked == want_checked && got_unchecked == want_unchecked {
            hit_ok += 1;
        } else {
            failures.push(format!(
                "{}: hits differ: checked {got_checked:?} vs {want_checked:?}, unchecked {got_unchecked:?} vs {want_unchecked:?}",
                clip.id
            ));
        }
    }
    let (p50, p95) = percentiles(&mut secs);
    eprintln!(
        "wake: scores {score_ok}/{} clips within 1e-3 over {frames_total} frames (worst {worst:.2e}); hits {hit_ok}/{} \
         at 0.3 and 0.7; per 80 ms frame p50 {:.1}ms p95 {:.1}ms",
        clips.len(),
        clips.len(),
        p50 * 1000.0,
        p95 * 1000.0
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
}

#[derive(Deserialize)]
struct KokoroFixture {
    text: String,
    phonemes: String,
    tokens: Vec<i64>,
    trim: [usize; 2],
}

fn rms_diff(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    (a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(x - y).powi(2))
        .sum::<f64>()
        / a.len() as f64)
        .sqrt()
}

#[test]
#[ignore = "needs the parity fixtures (scripts/fixtures/record.py)"]
fn kokoro_matches() {
    let Some(dir) = fixtures() else { return };
    let mut paths = KokoroPaths::in_cache().expect("~/.cache/ghost/kokoro");
    // GHOST_ESPEAK_DATA points the port at another espeak-ng-data (the one espeakng_loader
    // ships in the venv), to tell a data difference from a port difference.
    paths.espeak_data = std::env::var_os("GHOST_ESPEAK_DATA").map(PathBuf::from);
    let t0 = Instant::now();
    let mut kokoro = Kokoro::load(&paths).unwrap();
    eprintln!(
        "kokoro: loaded in {:.1}s; espeak-ng {} with data at {}",
        t0.elapsed().as_secs_f64(),
        kokoro.espeak().version(),
        kokoro.espeak().data_path().display()
    );
    let sentences: Vec<String> = json(&dir.join("kokoro/sentences.json"));
    let (mut phon_ok, mut tok_ok, mut trim_ok, mut raw_ok, mut pcm_ok) = (0, 0, 0, 0, 0);
    let mut worst_raw = 0.0f64;
    let mut worst_pcm = 0.0f64;
    let mut secs = Vec::new();
    let mut first_clause = Vec::new();
    let mut first_clause_bench = Vec::new();
    let mut failures = Vec::new();
    for (i, text) in sentences.iter().enumerate() {
        let want: KokoroFixture = json(&dir.join("kokoro").join(format!("{i}.json")));
        assert_eq!(&want.text, text);
        let phonemes = kokoro.phonemize(text, "en-us").unwrap();
        if phonemes == want.phonemes {
            phon_ok += 1;
        } else {
            failures.push(format!(
                "{i}: phonemes\n    python {:?}\n    rust   {phonemes:?}",
                want.phonemes
            ));
        }
        let batches = kokoro::split_phonemes(
            &phonemes.split_whitespace().collect::<Vec<_>>().join(" "),
            kokoro::MAX_PHONEME_LENGTH,
        );
        let tokens = kokoro
            .tokenize(batches.first().map(String::as_str).unwrap_or(""))
            .unwrap();
        if tokens == want.tokens {
            tok_ok += 1;
        } else {
            failures.push(format!(
                "{i}: tokens {tokens:?} vs python {:?}",
                want.tokens
            ));
        }
        // The model's raw output on the Python's tokens, then the trim on it.
        let style = kokoro
            .style("af_heart", want.tokens.len())
            .unwrap()
            .to_vec();
        let raw = kokoro.infer(&want.tokens, &style, 1.0).unwrap();
        let want_raw = npy::read(&dir.join("kokoro").join(format!("{i}.raw.npy")))
            .unwrap()
            .to_f32()
            .unwrap();
        let dr = if raw.len() == want_raw.len() {
            rms_diff(&raw, &want_raw)
        } else {
            f64::INFINITY
        };
        worst_raw = worst_raw.max(dr);
        if dr <= 1e-3 {
            raw_ok += 1;
        } else if strict() || dr > 1e-2 {
            failures.push(format!(
                "{i}: raw audio differs by {dr:.2e} RMS ({} vs {} samples)",
                raw.len(),
                want_raw.len()
            ));
        }
        let trim = kokoro::trim_silence(&want_raw);
        if trim == (want.trim[0], want.trim[1]) {
            trim_ok += 1;
        } else {
            failures.push(format!("{i}: trim {trim:?} vs python {:?}", want.trim));
        }
        // What Ghost plays: the whole text through create().
        let t = Instant::now();
        let pcm = kokoro.synthesize(text, "af_heart", 1.0, "en-us").unwrap();
        secs.push(t.elapsed().as_secs_f64());
        let want_pcm = npy::read(&dir.join("kokoro").join(format!("{i}.npy")))
            .unwrap()
            .to_f32()
            .unwrap();
        let one_batch = want_pcm.len() == want.trim[1] - want.trim[0];
        let dp = if pcm.len() == want_pcm.len() {
            rms_diff(&pcm, &want_pcm)
        } else if !strict()
            && one_batch
            && tokens == want.tokens
            && pcm.len().abs_diff(want_pcm.len()) <= 512
        {
            // The trim boundary moved a hop on the runtime's noise: what Ghost plays is
            // the Rust audio over the Python's interval, a hop longer or shorter.
            rms_diff(&raw[want.trim[0]..want.trim[1]], &want_pcm)
        } else {
            f64::INFINITY
        };
        worst_pcm = worst_pcm.max(dp);
        if dp <= 1e-3 {
            pcm_ok += 1;
        } else if strict() || dp > 1e-2 {
            failures.push(format!(
                "{i}: PCM differs by {dp:.2e} RMS ({} vs {} samples)",
                pcm.len(),
                want_pcm.len()
            ));
        }
        // The first clause's audio, as the pipeline waits for it.
        let t = Instant::now();
        let mut stream = kokoro
            .stream(text, "af_heart", 1.0, "en-us", Some(4))
            .unwrap();
        let first = stream.next().expect("a first clause").unwrap();
        let took = t.elapsed().as_secs_f64();
        assert!(!first.pcm.is_empty());
        first_clause.push(took);
        // ADR 0004's bar is for a first sentence of 1.6 to 1.9 s of audio
        // (docs/benchmarks/2026-10-16-speech-cpu.md).
        let audio_secs = first.pcm.len() as f64 / f64::from(kokoro::SAMPLE_RATE);
        if (1.6..=1.9).contains(&audio_secs) {
            first_clause_bench.push(took);
        }
    }
    let (p50, p95) = percentiles(&mut secs);
    let (fp50, fp95) = percentiles(&mut first_clause);
    let (bp50, bp95) = percentiles(&mut first_clause_bench);
    eprintln!(
        "kokoro: phonemes {phon_ok}/{n}, tokens {tok_ok}/{n}, trim {trim_ok}/{n}, raw audio {raw_ok}/{n} within 1e-3 RMS \
         (worst {worst_raw:.2e}), PCM {pcm_ok}/{n} (worst {worst_pcm:.2e}); sentence p50 {p50:.3}s p95 {p95:.3}s; \
         first clause p50 {fp50:.3}s p95 {fp95:.3}s; first clauses of 1.6-1.9 s audio (n={}) p50 {bp50:.3}s p95 {bp95:.3}s",
        first_clause_bench.len(),
        n = sentences.len()
    );
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
    if timing() {
        assert!(
            bp50 <= 0.37,
            "a first clause of 1.6-1.9 s of audio takes {bp50:.3}s p50, over the 0.37 s of ADR 0004"
        );
    }
}
