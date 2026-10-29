//! A WAV through the speech-in stage (the real Silero and Parakeet), outside a session:
//! the segments it makes and what Parakeet reads in each, against the whole file read
//! once. For the replay rig's utterances; a debugging aid, run by hand.

use std::sync::{Arc, Mutex};

use ghost_infer::{Parakeet, Silero, VadAnalyzer, VadParams};
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Direction, Frame, Out, Stage};
use ghost_voice::stt::{ParakeetStt, SharedRecognizer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let wav = std::env::args().nth(1).expect("a WAV path");
    let s = VoiceSettings::from_env()?;
    let pcm = ghost_voice::perf::audio::read_wav(std::path::Path::new(&wav))?;
    let mut parakeet = Parakeet::load(
        &s.parakeet_paths().expect("Parakeet's files"),
        s.stt_threads,
    )?;
    let whole = parakeet.transcribe(&ghost_infer::audio::to_f32(&pcm))?;
    println!(
        "whole file ({:.2} s): {whole:?}",
        pcm.len() as f64 / 16000.0
    );
    let model: SharedRecognizer = Arc::new(Mutex::new(parakeet));
    let vad = VadAnalyzer::new(Silero::load(&s.silero_model)?, VadParams::default());
    let mut stt = ParakeetStt::new(model, Some(vad), VadParams::default(), 0.0);
    let (out, mut rx) = Out::capture();
    let mut frames: Vec<Vec<i16>> = pcm.chunks(320).map(|c| c.to_vec()).collect();
    frames.extend(std::iter::repeat_n(vec![0i16; 320], 50)); // a second of silence after
    for f in frames {
        stt.process(Frame::AudioIn(Audio::new(f, 16000)), Direction::Down, &out)
            .await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut t = 0usize;
    while let Ok(p) = rx.try_recv() {
        match p.frame {
            Frame::AudioIn(a) => t += a.samples.len(),
            Frame::VadStarted { .. } => println!("{:.2}s VadStarted", t as f64 / 16000.0),
            Frame::VadStopped { .. } => println!("{:.2}s VadStopped", t as f64 / 16000.0),
            Frame::Transcript(text) => println!("  transcript: {text:?}"),
            other => println!("  {}", other.kind()),
        }
    }
    Ok(())
}
