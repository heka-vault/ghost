//! The chime and the thinking sound as PCM assets loaded once. A port of
//! `ghostd.audio.chime` and `ghost_voice.sounds`: nothing is downloaded, the thinking loop
//! is rendered into the cache (`~/.cache/ghost/thinking-v2-<rate>.wav`, the file the Python
//! daemon made; `hound` reads it) and the chime is two short sine notes computed at start.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The chime's rate: the playback rate, so it goes straight into [`crate::Playback`].
pub const CHIME_RATE: u32 = 24_000;
/// The thinking loop's length, seconds.
pub const THINKING_SECONDS: f64 = 1.6;

#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    #[error("sound {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sound {path}: {source}")]
    Wav {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    #[error(
        "sound {path}: {rate} Hz, {channels} channel(s), {bits}-bit; need {want} Hz mono 16-bit"
    )]
    Format {
        path: PathBuf,
        rate: u32,
        channels: u16,
        bits: u16,
        want: u32,
    },
}

/// Two short sine notes at 24 kHz. Rising on wake, falling on sleep.
pub fn chime(up: bool) -> Vec<i16> {
    let sr = CHIME_RATE as f64;
    let notes: [f64; 2] = if up { [660.0, 880.0] } else { [880.0, 660.0] };
    let n = (sr * 0.09) as usize;
    let last = (n - 1) as f64 / sr;
    let mut out = Vec::with_capacity(2 * n);
    for f in notes {
        for i in 0..n {
            let t = i as f64 / sr;
            let env = 1.0_f64.min((t / 0.01).min((last - t) / 0.03));
            let v = (0.25 * env * (2.0 * PI * f * t).sin()) as f32; // the Python played float32
            out.push((v * 32767.0).round() as i16);
        }
    }
    out
}

/// One bouncing blip: fast attack, exponential decay, pitch dropping to `bend` x.
fn blip(rate: u32, freq: f64, length: f64, bend: f64) -> Vec<f64> {
    let sr = rate as f64;
    let n = (sr * length) as usize;
    let mut out = Vec::with_capacity(n);
    let mut cum = 0.0;
    for i in 0..n {
        let t = i as f64 / sr;
        let f = freq * (1.0 - (1.0 - bend) * t / length); // glides down like a bounce losing height
        cum += f;
        let phase = 2.0 * PI * cum / sr;
        let env = (t / 0.004).min(1.0) * (-t / (length * 0.28)).exp();
        out.push(env * (phase.sin() + 0.25 * (2.0 * phase).sin()));
    }
    out
}

/// A light, bouncy figure: four short blips hopping between notes at 150 BPM, each with a
/// small downward pitch bend, over a very soft low pulse. Loops seamlessly. Peak 0.55.
pub fn thinking_loop(rate: u32, seconds: f64) -> Vec<i16> {
    let sr = rate as f64;
    let n = (sr * seconds) as usize;
    let mut audio = vec![0.0f64; n];
    let beat = seconds / 4.0;
    for (i, freq) in [659.3, 830.6, 987.8, 830.6].into_iter().enumerate() {
        // E5 G#5 B5 G#5
        let start = (i as f64 * beat * sr) as usize;
        let b = blip(rate, freq, beat.min(0.18), 0.82);
        let end = n.min(start + b.len());
        for (k, v) in b.iter().take(end.saturating_sub(start)).enumerate() {
            audio[start + k] += v;
        }
    }
    for (i, a) in audio.iter_mut().enumerate() {
        let t = i as f64 / sr;
        let pulse = 0.5 - 0.5 * (2.0 * PI * t / seconds).cos();
        *a += 0.12 * pulse * (2.0 * PI * 164.8 * t).sin(); // E3 breathing underneath
    }
    let max = audio.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-6);
    let scale = 0.55 / max;
    audio
        .into_iter()
        .map(|v| (v * scale * 32767.0) as i16)
        .collect()
}

/// The thinking loop's file for this rate, written once.
pub fn ensure_thinking_sound(cache_dir: &Path, rate: u32) -> Result<PathBuf, SoundError> {
    let path = cache_dir.join(format!("thinking-v2-{rate}.wav"));
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(cache_dir).map_err(|source| SoundError::Io {
        path: cache_dir.to_path_buf(),
        source,
    })?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).map_err(|source| SoundError::Wav {
        path: path.clone(),
        source,
    })?;
    for s in thinking_loop(rate, THINKING_SECONDS) {
        writer.write_sample(s).map_err(|source| SoundError::Wav {
            path: path.clone(),
            source,
        })?;
    }
    writer.finalize().map_err(|source| SoundError::Wav {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// A mono 16-bit WAV at `rate`, as samples. Anything else is an error that names the file.
pub fn load_wav(path: &Path, rate: u32) -> Result<Vec<i16>, SoundError> {
    let mut reader = hound::WavReader::open(path).map_err(|source| SoundError::Wav {
        path: path.to_path_buf(),
        source,
    })?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
        || spec.sample_rate != rate
    {
        return Err(SoundError::Format {
            path: path.to_path_buf(),
            rate: spec.sample_rate,
            channels: spec.channels,
            bits: spec.bits_per_sample,
            want: rate,
        });
    }
    reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SoundError::Wav {
            path: path.to_path_buf(),
            source,
        })
}

/// The thinking sound for the playback rate: rendered if missing, then loaded once.
pub fn thinking_sound(cache_dir: &Path, rate: u32) -> Result<Arc<Vec<i16>>, SoundError> {
    let path = ensure_thinking_sound(cache_dir, rate)?;
    Ok(Arc::new(load_wav(&path, rate)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_two_notes_of_90_ms() {
        let up = chime(true);
        assert_eq!(up.len(), 2 * 2160);
        assert_eq!(up[0], 0, "starts on the attack ramp");
        assert_eq!(up[2159], 0, "ends on the release ramp");
        let peak = up.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!((7900..=8192).contains(&peak), "0.25 full scale, {peak}");
        let down = chime(false);
        assert_eq!(down.len(), up.len());
        // the second note of the rising chime is the first of the falling one
        assert_eq!(&up[2160..2160 + 200], &down[..200]);
    }

    #[test]
    fn thinking_loop_is_seconds_long_and_peaks_at_0_55() {
        let s = thinking_loop(24_000, THINKING_SECONDS);
        assert_eq!(s.len(), 38_400);
        let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert_eq!(peak, (0.55 * 32767.0) as u16);
        // four blips: the sample right at each beat is loud soon after
        for beat in 0..4 {
            let start = beat * 9_600;
            let loud = s[start..start + 480]
                .iter()
                .map(|v| v.unsigned_abs())
                .max()
                .unwrap();
            assert!(loud > 4000, "beat {beat} peaks at {loud}");
        }
        assert_eq!(thinking_loop(16_000, 1.0).len(), 16_000);
    }

    #[test]
    fn thinking_sound_is_rendered_once_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("ghost");
        let path = ensure_thinking_sound(&cache, 24_000).unwrap();
        assert_eq!(path, cache.join("thinking-v2-24000.wav"));
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        let again = ensure_thinking_sound(&cache, 24_000).unwrap();
        assert_eq!(again, path);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        let samples = load_wav(&path, 24_000).unwrap();
        assert_eq!(samples, thinking_loop(24_000, THINKING_SECONDS));
        let shared = thinking_sound(&cache, 24_000).unwrap();
        assert_eq!(shared.len(), 38_400);
    }

    #[test]
    fn a_wav_at_the_wrong_rate_or_shape_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = ensure_thinking_sound(dir.path(), 16_000).unwrap();
        let err = load_wav(&path, 24_000).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("thinking-v2-16000.wav"), "{text}");
        assert!(
            text.contains("16000 Hz, 1 channel(s), 16-bit; need 24000 Hz"),
            "{text}"
        );
        let stereo = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&stereo, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        assert!(matches!(
            load_wav(&stereo, 24_000),
            Err(SoundError::Format { channels: 2, .. })
        ));
        assert!(matches!(
            load_wav(&dir.path().join("missing.wav"), 24_000),
            Err(SoundError::Wav { .. })
        ));
    }
}
