//! Samples in and out: 16 kHz mono int16 is what the pipeline carries, float32 in
//! `[-1, 1)` is what the models take (`int16 / 32768`, as every Python wrapper divides).

use std::path::Path;

use crate::{InferError, Result};

pub const SAMPLE_RATE: u32 = 16_000;

/// Read a 16 kHz mono 16-bit WAV, as the fixture clips are.
pub fn read_wav_16k(path: &Path) -> Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| InferError::Audio(format!("{}: {e}", path.display())))?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE || spec.channels != 1 || spec.bits_per_sample != 16 {
        return Err(InferError::Audio(format!(
            "{}: want 16 kHz mono int16, got {} Hz, {} channels, {} bits",
            path.display(),
            spec.sample_rate,
            spec.channels,
            spec.bits_per_sample
        )));
    }
    reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| InferError::Audio(format!("{}: {e}", path.display())))
}

/// `int16 -> float32 / 32768`.
pub fn to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| f32::from(s) / 32768.0).collect()
}

/// Pipecat's `(samples * 32767).astype(np.int16)`, clamped instead of wrapped where a
/// sample is outside `[-1, 1]`.
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}
