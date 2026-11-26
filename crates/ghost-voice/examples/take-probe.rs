//! A kept wake take through Parakeet as the check reads it, and in variants: from other
//! starting points, with silence or more lead-in before the speech, and louder.
//! For a hit the check turned down (#192). A debugging aid, run by hand:
//!
//!   cargo run -p ghost-voice --example take-probe -- TAKE.wav [FROM_SECS ...]
//!
//! 16 kHz mono WAV. Prints each variant and what Parakeet read.

use ghost_infer::Parakeet;
use ghost_voice::config::VoiceSettings;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let wav = args.next().expect("a take's WAV");
    let froms: Vec<f64> = args.map(|a| a.parse()).collect::<Result<_, _>>()?;
    let s = VoiceSettings::from_env()?;
    let pcm = ghost_voice::perf::audio::read_wav(std::path::Path::new(&wav))?;
    let audio = ghost_infer::audio::to_f32(&pcm);
    let mut parakeet = Parakeet::load(&s.parakeet_paths(), s.stt_threads)?.english_only(true);
    let secs = |n: usize| n as f64 / 16_000.0;
    let at = |t: f64| ((t * 16_000.0) as usize).min(audio.len());
    let mut read = |label: String, a: &[f32]| -> anyhow::Result<()> {
        // the check's own dither under every reading (`stt::recognize`)
        let text = parakeet.transcribe(&ghost_voice::stt::dithered(a))?;
        println!("{label:<44} {:>5.2}s  {text:?}", secs(a.len()));
        Ok(())
    };
    read("whole".into(), &audio)?;
    for from in froms {
        let a = &audio[at(from)..];
        read(format!("from {from:.2}s"), a)?;
        let mut zeros = vec![0.0f32; 8_000];
        zeros.extend_from_slice(a);
        read(format!("from {from:.2}s, 0.5 s silence first"), &zeros)?;
        let loud: Vec<f32> = a.iter().map(|x| (x * 2.0).clamp(-1.0, 1.0)).collect();
        read(format!("from {from:.2}s, +6 dB"), &loud)?;
    }
    Ok(())
}
