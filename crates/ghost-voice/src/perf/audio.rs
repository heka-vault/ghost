//! Rendered user utterances (`ghostd.perf.audio`).
//!
//! [`Voice`] renders a line with Kokoro (in a voice that is not Ghost's), resamples it to
//! 16 kHz mono int16, and caches it under `.scratch/perf/utterances` with the same key the
//! Python harness used, so the utterances it rendered are reused as they are. A line may
//! contain `" ... "`: the parts are rendered separately and joined by a pause, to exercise
//! the turn hold ("what needs my sign-off ... in the app repo").
//!
//! The scripted devices that play them in and take Ghost's audio out are
//! `transport::scripted_media`.

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::tts::SharedSynth;

pub const IN_RATE: u32 = 16_000;
pub const OUT_RATE: u32 = 24_000;
pub const PAUSE_MARK: &str = " ... ";

#[derive(Debug, Clone)]
pub struct Utterance {
    pub text: String,
    /// 16 kHz mono int16.
    pub pcm: Vec<i16>,
}

impl Utterance {
    pub fn seconds(&self) -> f64 {
        self.pcm.len() as f64 / f64::from(IN_RATE)
    }
}

/// Renders and caches the operator's lines.
pub struct Voice {
    synth: SharedSynth,
    cache: PathBuf,
    voice: String,
    speed: f64,
    pause_secs: f64,
}

impl Voice {
    pub fn new(synth: SharedSynth, cache: &Path) -> Self {
        let _ = std::fs::create_dir_all(cache);
        Self {
            synth,
            cache: cache.to_path_buf(),
            voice: "am_michael".into(),
            speed: 1.0,
            pause_secs: 0.7,
        }
    }

    fn key(&self, text: &str) -> String {
        // The Python's f-string: floats as Python prints them ("1.0", "0.7").
        let spec = format!(
            "{}|{:?}|{:?}|{text}",
            self.voice, self.speed, self.pause_secs
        );
        let digest = Sha1::digest(spec.as_bytes());
        hex::encode(digest)[..16].to_string()
    }

    pub fn render(&self, text: &str) -> anyhow::Result<Utterance> {
        let path = self.cache.join(format!("{}.wav", self.key(text)));
        if path.exists() {
            return Ok(Utterance {
                text: text.into(),
                pcm: read_wav(&path)?,
            });
        }
        let parts: Vec<&str> = text
            .split(PAUSE_MARK.trim())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let gap = vec![0.0f32; (f64::from(IN_RATE) * self.pause_secs) as usize];
        let mut audio: Vec<f32> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                audio.extend_from_slice(&gap);
            }
            audio.extend(self.speak(part)?);
        }
        let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        let pcm: Vec<i16> = audio
            .iter()
            .map(|v| (v * (0.5 / peak) * 32767.0) as i16) // a normal speaking level at the mic
            .collect();
        write_wav(&path, &pcm)?;
        Ok(Utterance {
            text: text.into(),
            pcm,
        })
    }

    fn speak(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let pcm = self
            .synth
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .synthesize(text, &self.voice, self.speed as f32)?;
        let samples = ghost_infer::audio::to_f32(&pcm);
        Ok(resample(&samples, OUT_RATE, IN_RATE))
    }
}

/// Linear-interpolation resampling: enough for the rig's utterances, which the real
/// models then hear.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let n = (input.len() as f64 / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = input[j.min(input.len() - 1)];
            let b = input[(j + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

pub fn read_wav(path: &Path) -> anyhow::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path)?;
    let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
    Ok(samples?)
}

pub fn write_wav(path: &Path, pcm: &[i16]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: IN_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = path.with_extension("tmp");
    {
        let mut w = hound::WavWriter::create(&tmp, spec)?;
        for s in pcm {
            w.write_sample(*s)?;
        }
        w.finalize()?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_key_is_the_pythons() {
        // sha1("am_michael|1.0|0.7|hello")[:16], as the Python harness named the file.
        let v = Voice {
            synth: std::sync::Arc::new(std::sync::Mutex::new(crate::tts::NoSynth)),
            cache: PathBuf::from("/tmp"),
            voice: "am_michael".into(),
            speed: 1.0,
            pause_secs: 0.7,
        };
        let spec = "am_michael|1.0|0.7|hello";
        let want = hex::encode(Sha1::digest(spec.as_bytes()))[..16].to_string();
        assert_eq!(v.key("hello"), want);
    }

    #[test]
    fn resampling_keeps_the_length_in_proportion() {
        let input: Vec<f32> = (0..2400).map(|i| (i as f32 / 100.0).sin()).collect();
        let out = resample(&input, 24_000, 16_000);
        assert_eq!(out.len(), 1600);
        assert!((out[800] - input[1200]).abs() < 0.01);
    }
}
