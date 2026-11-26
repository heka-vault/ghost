//! WAVs through the trained wake detector alone, as the daemon runs it with no Parakeet
//! check (`GHOST_WAKE_VERIFY=false`): the melspectrogram and embedding graphs, the
//! classifier, a hit at or over each threshold, 2 s of cooldown. For comparing a detector
//! with transcript wake on the same recordings (#190, `wake-probe`). Run by hand:
//!
//!   cargo run --release -p ghost-voice --example detector-probe -- MODEL.onnx a.wav b.wav ...
//!
//! 16 kHz mono WAVs. Prints, per threshold, each file's hits and the totals.

use std::path::{Path, PathBuf};

use ghost_infer::models::{EMBEDDING, MELSPECTROGRAM, resolve_for_process};
use ghost_infer::wake::detector::{WakeDetector, WakeModel, WakeScorer};

const THRESHOLDS: [f32; 2] = [0.5, 0.7];
const COOLDOWN_SECS: f64 = 2.0;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(args.next().expect("a model .onnx"));
    let files: Vec<String> = args.collect();
    anyhow::ensure!(!files.is_empty(), "WAV paths");
    let mel = resolve_for_process(MELSPECTROGRAM)?;
    let embedding = resolve_for_process(EMBEDDING)?;
    let quiet = std::env::var_os("DETECTOR_PROBE_QUIET").is_some();
    for threshold in THRESHOLDS {
        let (mut hits_total, mut files_hit) = (0usize, 0usize);
        for wav in &files {
            // Two seconds of silence before and one after, as a stream has around a short
            // clip: the detector scores a two-second window.
            let clip = ghost_voice::perf::audio::read_wav(Path::new(wav))?;
            let mut pcm = vec![0i16; 2 * 16_000];
            pcm.extend_from_slice(&clip);
            pcm.extend(std::iter::repeat_n(0i16, 16_000));
            let scorer = WakeScorer::load(&mel, &embedding, std::slice::from_ref(&model), 2)?;
            let mut det = WakeDetector::new(
                scorer,
                vec![WakeModel::known(&model, threshold, Some(false))],
                COOLDOWN_SECS,
            );
            let mut hits = Vec::new();
            for chunk in pcm.chunks(320) {
                if let Some(h) = det.push(chunk, 16_000 * 4)? {
                    hits.push((h.end as f64 / 16_000.0, h.score));
                    det.clear_hit();
                }
            }
            hits_total += hits.len();
            files_hit += usize::from(!hits.is_empty());
            if !quiet {
                let at: Vec<String> = hits
                    .iter()
                    .map(|(t, s)| format!("{t:.1}s:{s:.2}"))
                    .collect();
                println!(
                    "[{threshold}] {wav}: {} hit(s) {}",
                    hits.len(),
                    at.join(" ")
                );
            }
        }
        println!(
            "== threshold {threshold}: {} file(s), {files_hit} with a hit, {hits_total} hit(s)",
            files.len()
        );
    }
    Ok(())
}
