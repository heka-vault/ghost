//! What Silero's analyzer says about a WAV, frame by frame: the state transitions and the
//! highest confidence seen. For the replay rig's utterances; numbers only.

use ghost_infer::{Silero, VadAnalyzer, VadParams, VadState};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (model, wav) = (&args[0], &args[1]);
    let mut vad = VadAnalyzer::new(Silero::load(std::path::Path::new(model))?, VadParams::default());
    let pcm = ghost_voice::perf::audio::read_wav(std::path::Path::new(wav))?;
    println!("{} samples, {:.2} s, peak {}", pcm.len(), pcm.len() as f64 / 16000.0, pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0));
    let mut state = VadState::Quiet;
    let mut t = 0.0f64;
    let mut best = 0.0f32;
    for frame in pcm.chunks(320) {
        let now = 1_700_000_000.0 + t;
        let after = vad.analyze(frame, now)?;
        if after != state {
            println!("{t:.2}s: {state:?} -> {after:?}");
            state = after;
        }
        t += frame.len() as f64 / 16000.0;
    }
    // confidence on 512-sample chunks, standalone
    let mut vad2 = VadAnalyzer::new(Silero::load(std::path::Path::new(model))?, VadParams::default());
    for chunk in pcm.chunks_exact(512) {
        let (c, v, _) = vad2.analyze_chunk(chunk, 1_700_000_000.0)?;
        if c > best {
            best = c;
        }
        let _ = v;
    }
    println!("best confidence {best:.3}");
    Ok(())
}
