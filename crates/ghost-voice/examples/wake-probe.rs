//! WAVs through the passive listener's transcript wake, outside the daemon: the VAD with the
//! wake's stop time, Parakeet on each segment, then `match_wake` at the daemon's fuzziness,
//! with Parakeet's English-only decode off and on (issue #190). A debugging aid, run by
//! hand:
//!
//!   cargo run --release -p ghost-voice --example wake-probe -- a.wav b.wav ...
//!
//! 16 kHz mono WAVs. Prints, per file and mode, the segments, how many woke, and each
//! segment's reading with `*` when it woke and `!` when it has a character outside ASCII.

use std::sync::{Arc, Mutex};

use ghost_infer::wake::matcher::{match_wake, parse_phrases};
use ghost_infer::{Parakeet, Silero, VadAnalyzer, VadParams};
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Direction, Frame, Out, Stage};
use ghost_voice::stt::{ParakeetStt, SharedRecognizer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(!files.is_empty(), "WAV paths");
    let s = VoiceSettings::from_env()?;
    let phrases = parse_phrases(None);
    let fuzzy = 0.9;
    let params = VadParams {
        confidence: s.vad_confidence,
        start_secs: s.vad_start_secs,
        stop_secs: s.wake_vad_stop_secs,
        min_volume: s.vad_min_volume,
    };
    let verbose = std::env::var_os("WAKE_PROBE_QUIET").is_none();
    for english_only in [false, true] {
        let parakeet =
            Parakeet::load(&s.parakeet_paths(), s.stt_threads)?.english_only(english_only);
        let model: SharedRecognizer = Arc::new(Mutex::new(parakeet));
        let (mut woke_total, mut segs_total, mut files_woke) = (0usize, 0usize, 0usize);
        for wav in &files {
            let pcm = ghost_voice::perf::audio::read_wav(std::path::Path::new(wav))?;
            let vad = VadAnalyzer::new(Silero::load(&s.silero_model)?, params);
            let mut stt = ParakeetStt::new(model.clone(), Some(vad), params, 0.0);
            let (out, mut rx) = Out::capture();
            let mut frames: Vec<Vec<i16>> = pcm.chunks(320).map(|c| c.to_vec()).collect();
            frames.extend(std::iter::repeat_n(vec![0i16; 320], 75));
            for f in frames {
                stt.process(Frame::AudioIn(Audio::new(f, 16000)), Direction::Down, &out)
                    .await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let (mut segs, mut woke) = (0usize, 0usize);
            let mut lines = Vec::new();
            while let Ok(p) = rx.try_recv() {
                if let Frame::Transcript(text) = p.frame {
                    segs += 1;
                    let hit = match_wake(&text, &phrases, fuzzy).is_some();
                    woke += usize::from(hit);
                    let odd = !text.is_ascii();
                    lines.push(format!(
                        "    {}{} {text:?}",
                        if hit { "*" } else { " " },
                        if odd { "!" } else { " " }
                    ));
                }
            }
            segs_total += segs;
            woke_total += woke;
            files_woke += usize::from(woke > 0);
            println!("[english_only={english_only}] {wav}: {segs} segment(s), {woke} woke");
            if verbose {
                for l in lines {
                    println!("{l}");
                }
            }
        }
        println!(
            "== english_only={english_only}: {} file(s), {files_woke} woke at least once, \
             {woke_total} wake(s) in {segs_total} segment(s)",
            files.len()
        );
    }
    Ok(())
}
