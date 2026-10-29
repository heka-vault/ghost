//! ITU-R BS.1770 integrated loudness as Pipecat's volume gate measures it
//! (`pipecat.audio.utils.calculate_audio_volume` over the `loudness` package, a binding of
//! libloudness, which follows libebur128): K-weighting, 400 ms gating blocks, the -70 LUFS
//! absolute gate, and the 0.1 LU histogram the gated mean is taken over, which is why every
//! value lands on a `.x5`. Pipecat normalizes the LUFS from `[-110, -10]` to `[0, 1]`.

/// The 0.1 LU histogram of libebur128: bin `i` holds the energy of `i / 10 - 69.95` LUFS and
/// starts at `i / 10 - 70` LUFS.
const BINS: usize = 1000;

fn energy_from_loudness(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

fn energy_to_loudness(energy: f64) -> f64 {
    10.0 * energy.log10() - 0.691
}

/// The K-weighting filter: a high shelf then a high pass, each a biquad with libebur128's
/// coefficients for the sample rate, combined into one 5-tap IIR.
struct KWeighting {
    b: [f64; 5],
    a: [f64; 5],
}

impl KWeighting {
    fn new(sample_rate: u32) -> Self {
        let fs = f64::from(sample_rate);
        let f0 = 1681.974450955533;
        let g = 3.999843853973347;
        let q = 0.7071752369554196;
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        let pb = [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ];
        let pa = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];

        let f0 = 38.13547087602444;
        let q = 0.5003270373238773;
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let rb = [1.0, -2.0, 1.0];
        let ra = [
            1.0,
            2.0 * (k * k - 1.0) / (1.0 + k / q + k * k),
            (1.0 - k / q + k * k) / (1.0 + k / q + k * k),
        ];
        let conv = |x: [f64; 3], y: [f64; 3]| {
            let mut out = [0.0; 5];
            for (i, xi) in x.iter().enumerate() {
                for (j, yj) in y.iter().enumerate() {
                    out[i + j] += xi * yj;
                }
            }
            out
        };
        Self {
            b: conv(pb, rb),
            a: conv(pa, ra),
        }
    }

    /// Mean square of the filtered signal, from a zero state.
    #[cfg(test)]
    fn mean_square(&self, samples: &[f32]) -> f64 {
        let mut v = [0.0f64; 5];
        let mut sum = 0.0;
        for &x in samples {
            v[0] = f64::from(x)
                - self.a[1] * v[1]
                - self.a[2] * v[2]
                - self.a[3] * v[3]
                - self.a[4] * v[4];
            let y = self.b[0] * v[0]
                + self.b[1] * v[1]
                + self.b[2] * v[2]
                + self.b[3] * v[3]
                + self.b[4] * v[4];
            v[4] = v[3];
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            sum += y * y;
        }
        sum / samples.len() as f64
    }
}

fn histogram_index(energy: f64) -> usize {
    let boundary = |i: usize| energy_from_loudness(i as f64 / 10.0 - 70.0);
    let (mut lo, mut hi) = (0usize, BINS);
    while hi - lo != 1 {
        let mid = (lo + hi) / 2;
        if energy >= boundary(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `loudness.integrated_loudness(samples, sample_rate)` in LUFS; `None` where the library
/// says `-inf` (every block under the -70 LUFS absolute gate). Needs at least 400 ms.
pub fn integrated_loudness(samples: &[f32], sample_rate: u32) -> Option<f64> {
    let block = (sample_rate as usize) * 4 / 10;
    let step = (sample_rate as usize) / 10;
    if samples.len() < block {
        return None;
    }
    let filter = KWeighting::new(sample_rate);
    // Blocks of 400 ms every 100 ms, as libebur128 gates; the filter runs across the whole
    // signal, so each block's energy is read off the filtered stream.
    let mut v = [0.0f64; 5];
    let mut filtered = Vec::with_capacity(samples.len());
    for &x in samples {
        v[0] = f64::from(x)
            - filter.a[1] * v[1]
            - filter.a[2] * v[2]
            - filter.a[3] * v[3]
            - filter.a[4] * v[4];
        let y = filter.b[0] * v[0]
            + filter.b[1] * v[1]
            + filter.b[2] * v[2]
            + filter.b[3] * v[3]
            + filter.b[4] * v[4];
        v[4] = v[3];
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        filtered.push(y * y);
    }
    let mut hist = vec![0u64; BINS];
    let mut end = block;
    while end <= samples.len() {
        let energy: f64 = filtered[end - block..end].iter().sum::<f64>() / block as f64;
        if energy >= energy_from_loudness(-70.0) {
            hist[histogram_index(energy)] += 1;
        }
        end += step;
    }
    let bin_energy = |i: usize| energy_from_loudness(i as f64 / 10.0 - 69.95);
    let count: u64 = hist.iter().sum();
    if count == 0 {
        return None;
    }
    let mean = (0..BINS)
        .map(|i| bin_energy(i) * hist[i] as f64)
        .sum::<f64>()
        / count as f64;
    let relative = energy_to_loudness(mean) - 10.0;
    let start = if relative < -70.0 {
        0
    } else {
        let mut i = histogram_index(energy_from_loudness(relative));
        if energy_from_loudness(relative) > bin_energy(i) {
            i += 1;
        }
        i
    };
    let gated_count: u64 = hist[start..].iter().sum();
    if gated_count == 0 {
        return None;
    }
    let gated = (start..BINS)
        .map(|i| bin_energy(i) * hist[i] as f64)
        .sum::<f64>()
        / gated_count as f64;
    Some(energy_to_loudness(gated))
}

/// `calculate_audio_volume`: the loudness normalized from `[-110, -10]` LUFS to `[0, 1]`.
pub fn volume(samples: &[i16], sample_rate: u32) -> f64 {
    let audio = crate::audio::to_f32(samples);
    match integrated_loudness(&audio, sample_rate) {
        Some(level) => ((level + 110.0) / 100.0).clamp(0.0, 1.0),
        None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, amplitude: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / 16000.0).sin() as f32
            })
            .collect()
    }

    #[test]
    fn silence_is_under_the_absolute_gate() {
        assert_eq!(integrated_loudness(&vec![0.0; 6400], 16000), None);
        assert_eq!(volume(&vec![0; 6400], 16000), 0.0);
    }

    #[test]
    fn needs_a_full_gating_block() {
        assert_eq!(integrated_loudness(&sine(1000.0, 0.5, 6000), 16000), None);
    }

    #[test]
    fn matches_the_loudness_package_on_sines() {
        // Values read from `loudness.integrated_loudness` (libloudness) on this machine.
        let cases = [(1000.0, -8.95), (100.0, -10.75), (5000.0, -5.65)];
        for (hz, want) in cases {
            let got = integrated_loudness(&sine(hz, 0.5, 6400), 16000).unwrap();
            assert!((got - want).abs() < 1e-9, "{hz} Hz: got {got}, want {want}");
        }
    }

    #[test]
    fn a_second_block_of_silence_is_gated_out() {
        let mut s = sine(5000.0, 0.5, 6400);
        s.extend(vec![0.0; 9600]);
        let got = integrated_loudness(&s, 16000).unwrap();
        assert!(got < -5.0 && got > -20.0, "{got}");
    }

    #[test]
    fn volume_is_the_normalized_level() {
        // -8.95 LUFS is louder than the top of Pipecat's range: 1. A sine 20 dB down is
        // -28.95 LUFS: 0.8105.
        assert_eq!(
            volume(&crate::audio::to_i16(&sine(1000.0, 0.5, 6400)), 16000),
            1.0
        );
        let v = volume(&crate::audio::to_i16(&sine(1000.0, 0.05, 6400)), 16000);
        assert!((v - (110.0 - 28.95) / 100.0).abs() < 1e-3, "{v}");
    }

    #[test]
    fn k_weighting_combines_two_biquads() {
        let k = KWeighting::new(16000);
        assert_eq!(k.a[0], 1.0);
        assert!(k.mean_square(&sine(1000.0, 0.5, 6400)) > 0.1);
    }
}
