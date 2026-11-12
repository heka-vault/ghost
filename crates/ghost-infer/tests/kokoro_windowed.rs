//! Kokoro's two halves against the whole model (ADR 0053), in the style of `parity.rs`.
//!
//! The halves are the whole model's graph cut in two by
//! `tools/voice-trial/voice_trial/split_kokoro.py --phase`, so run one after the other over
//! every frame they must give the whole model's samples, and a window decoded with the
//! carried phase and enough context must give the whole render's samples for its frames.
//! Ignored unless Kokoro and the halves are on the machine (`GHOST_TTS_STREAM_DIR`, or
//! `~/.cache/ghost/kokoro/windowed/`):
//!
//! ```text
//! GHOST_TTS_STREAM_DIR=... cargo nextest run -p ghost-infer --release --run-ignored all \
//!     --no-capture -E 'binary(kokoro_windowed)'
//! ```

use std::path::PathBuf;

use ghost_infer::kokoro_windowed::{HOP, Halves, HalvesPaths, WindowPlan};
use ghost_infer::{Kokoro, KokoroPaths};

const LINES: [&str; 4] = [
    "Labeled it.",
    "Three issues are waiting for your sign-off today.",
    "Issue one forty two asks whether to use Redis or Postgres for the session cache today.",
    "I found three issues waiting, and two of them need your sign-off before the launch next week.",
];

fn models() -> Option<(Kokoro, Halves)> {
    let kokoro = KokoroPaths::in_cache()?;
    let dir = std::env::var_os("GHOST_TTS_STREAM_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ghost/kokoro/windowed"))
        })?;
    let halves = HalvesPaths::in_dir(&dir);
    if !kokoro.model.is_file() || !halves.exist() {
        eprintln!("skipped: Kokoro or its halves are not on this machine");
        return None;
    }
    Some((
        Kokoro::load(&kokoro).unwrap(),
        Halves::load(&halves).unwrap(),
    ))
}

fn rms(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    (a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(x - y).powi(2))
        .sum::<f64>()
        / n as f64)
        .sqrt()
}

fn tokens_and_style(kokoro: &Kokoro, text: &str) -> (Vec<i64>, Vec<f32>) {
    let phonemes = kokoro.phonemize(text, "en-us").unwrap();
    let tokens = kokoro.tokenize(&phonemes).unwrap();
    let style = kokoro.style("af_heart", tokens.len()).unwrap().to_vec();
    (tokens, style)
}

#[test]
#[ignore = "needs Kokoro and its halves (scripts/models.sh kokoro-windowed)"]
fn the_halves_over_every_frame_are_the_whole_model() {
    let Some((mut kokoro, mut halves)) = models() else {
        return;
    };
    for text in LINES {
        let (tokens, style) = tokens_and_style(&kokoro, text);
        let whole = kokoro.infer(&tokens, &style, 1.0).unwrap();
        let enc = halves.encode(&tokens, &style, 1.0).unwrap();
        let split = halves.decode(&enc, 0, enc.frames, &style).unwrap();
        assert_eq!(whole.len(), split.len(), "{text}");
        assert_eq!(whole.len(), enc.frames * HOP, "{text}");
        let d = rms(&whole, &split);
        eprintln!("{text:?}: {} frames, RMS difference {d:.2e}", enc.frames);
        assert!(d < 1e-5, "{text}: {d}");
        assert_eq!(
            enc.durations.iter().sum::<i64>() as usize,
            enc.frames,
            "the durations are the frames"
        );
    }
}

#[test]
#[ignore = "needs Kokoro and its halves (scripts/models.sh kokoro-windowed)"]
fn a_window_with_the_whole_as_context_is_the_whole_render_there() {
    // The carried phase: a window decoded over every frame but starting its output late
    // must be the whole render's samples for its frames. Without the phase the harmonic
    // source restarts at the window and the samples differ.
    let Some((mut kokoro, mut halves)) = models() else {
        return;
    };
    for text in LINES {
        let (tokens, style) = tokens_and_style(&kokoro, text);
        let whole = kokoro.infer(&tokens, &style, 1.0).unwrap();
        let enc = halves.encode(&tokens, &style, 1.0).unwrap();
        let s = enc.frames / 3;
        // Every frame from `s` to the end: the whole render's samples from `s` on.
        let late = halves.decode(&enc, s, enc.frames, &style).unwrap();
        let want = &whole[s * HOP..];
        let d = rms(&late[HOP * 4..want.len() - HOP * 4], &want[HOP * 4..want.len() - HOP * 4]);
        let peak = want.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        eprintln!("{text:?}: from frame {s}, RMS difference {d:.4} (peak {peak:.2})");
        // The AdaIN layers see fewer frames, so the samples are near, not equal; the bound
        // is a tenth of the peak, which a restarted phase would not meet on voiced audio.
        assert!(d < f64::from(peak) * 0.1, "{text}: {d}");
    }
}

#[test]
#[ignore = "needs Kokoro and its halves (scripts/models.sh kokoro-windowed)"]
fn the_windowed_render_comes_sooner_and_is_as_long() {
    let Some((kokoro, halves)) = models() else {
        return;
    };
    let mut windowed = kokoro.with_windows(halves, WindowPlan::default());
    let mut whole = Kokoro::load(&KokoroPaths::in_cache().unwrap()).unwrap();
    let text = LINES[3];
    let t = std::time::Instant::now();
    let reference = whole.synthesize(text, "af_heart", 1.0, "en-us").unwrap();
    let whole_secs = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let mut first = None;
    let mut audio = Vec::new();
    let mut windows = 0;
    windowed
        .stream_render(text, "af_heart", 1.0, "en-us", &mut |a| {
            first.get_or_insert(t.elapsed().as_secs_f64());
            windows += 1;
            audio.extend(a);
            true
        })
        .unwrap();
    let first = first.unwrap();
    eprintln!(
        "whole {whole_secs:.3} s for {:.2} s of audio; windowed first audio {first:.3} s, {windows} windows, {:.2} s",
        reference.len() as f64 / 24_000.0,
        audio.len() as f64 / 24_000.0
    );
    assert!(windows > 1);
    assert!(first < whole_secs / 2.0, "{first} against {whole_secs}");
    let (a, b) = (audio.len() as f64, reference.len() as f64);
    assert!((a - b).abs() / b < 0.1, "{a} samples against {b}");
    assert!(audio.iter().all(|x| x.abs() < 1.0), "clipped");
}
