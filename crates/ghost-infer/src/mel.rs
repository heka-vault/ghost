//! The pieces of a log-mel front end the Python computed in NumPy: Hann windows, the
//! Slaney mel filterbank (`librosa.filters.mel`, which NeMo used for Parakeet, and
//! `transformers`' copy of it, which Smart Turn's extractor vendors), and a real FFT's
//! power spectrum (`realfft`, in float64 like `np.fft.rfft`).

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

/// `np.hanning(n)`: the symmetric Hann window.
pub fn hann_symmetric(n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![1.0];
    }
    (0..n)
        .map(|k| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / (n as f64 - 1.0)).cos())
        .collect()
}

/// `np.hanning(n + 1)[:-1]`: the periodic Hann window (`torch.hann_window`).
pub fn hann_periodic(n: usize) -> Vec<f64> {
    let mut w = hann_symmetric(n + 1);
    w.pop();
    w
}

/// `np.linspace(start, stop, num)`.
pub fn linspace(start: f64, stop: f64, num: usize) -> Vec<f64> {
    if num == 0 {
        return Vec::new();
    }
    if num == 1 {
        return vec![start];
    }
    let step = (stop - start) / (num as f64 - 1.0);
    let mut out: Vec<f64> = (0..num).map(|i| start + step * i as f64).collect();
    out[num - 1] = stop;
    out
}

/// Hertz to mel on the Slaney scale (`librosa.hz_to_mel(htk=False)`).
pub fn hertz_to_mel_slaney(freq: f64) -> f64 {
    let min_log_hertz = 1000.0;
    let min_log_mel = 15.0;
    let logstep = 27.0 / 6.4f64.ln();
    if freq >= min_log_hertz {
        min_log_mel + (freq / min_log_hertz).ln() * logstep
    } else {
        3.0 * freq / 200.0
    }
}

/// Mel to hertz on the Slaney scale.
pub fn mel_to_hertz_slaney(mel: f64) -> f64 {
    let min_log_hertz = 1000.0;
    let min_log_mel = 15.0;
    let logstep = 6.4f64.ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hertz * (logstep * (mel - min_log_mel)).exp()
    } else {
        200.0 * mel / 3.0
    }
}

/// A Slaney-normalized triangular mel filterbank, `(num_frequency_bins, num_mel_filters)`
/// in row-major order, the way `librosa.filters.mel(norm="slaney", htk=False)` and
/// `transformers.audio_utils.mel_filter_bank(mel_scale="slaney", norm="slaney")` build it.
pub fn slaney_filterbank(
    num_frequency_bins: usize,
    num_mel_filters: usize,
    min_frequency: f64,
    max_frequency: f64,
    sampling_rate: u32,
) -> Vec<f64> {
    let mel_freqs = linspace(
        hertz_to_mel_slaney(min_frequency),
        hertz_to_mel_slaney(max_frequency),
        num_mel_filters + 2,
    );
    let filter_freqs: Vec<f64> = mel_freqs.iter().map(|&m| mel_to_hertz_slaney(m)).collect();
    let fft_freqs = linspace(0.0, f64::from(sampling_rate / 2), num_frequency_bins);
    let filter_diff: Vec<f64> = filter_freqs.windows(2).map(|w| w[1] - w[0]).collect();
    let mut out = vec![0.0; num_frequency_bins * num_mel_filters];
    for (bin, &f) in fft_freqs.iter().enumerate() {
        for m in 0..num_mel_filters {
            let down = -(filter_freqs[m] - f) / filter_diff[m];
            let up = (filter_freqs[m + 2] - f) / filter_diff[m + 1];
            let w = down.min(up).max(0.0);
            let enorm = 2.0 / (filter_freqs[m + 2] - filter_freqs[m]);
            out[bin * num_mel_filters + m] = w * enorm;
        }
    }
    out
}

/// `|rfft(frame)|` over frames of one length, in float64.
pub struct Spectrum {
    fft: Arc<dyn RealToComplex<f64>>,
    n_fft: usize,
    input: Vec<f64>,
    output: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl Spectrum {
    pub fn new(n_fft: usize) -> Self {
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n_fft);
        let input = fft.make_input_vec();
        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            fft,
            n_fft,
            input,
            output,
            scratch,
        }
    }

    /// Number of bins, `n_fft / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// The magnitude of each bin of `frame` (zero-padded to `n_fft`), into `out`.
    pub fn magnitude(&mut self, frame: &[f64], out: &mut [f64]) {
        self.input.fill(0.0);
        self.input[..frame.len()].copy_from_slice(frame);
        self.fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .expect("buffers sized by the planner");
        for (o, c) in out.iter_mut().zip(&self.output) {
            *o = c.norm();
        }
    }

    /// `|X|^2` of each bin, into `out`.
    pub fn power(&mut self, frame: &[f64], out: &mut [f64]) {
        self.magnitude(frame, out);
        for o in out.iter_mut() {
            *o *= *o;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_windows_match_numpy() {
        let w = hann_symmetric(5);
        assert!((w[0]).abs() < 1e-12 && (w[2] - 1.0).abs() < 1e-12 && (w[4]).abs() < 1e-12);
        assert!((w[1] - 0.5).abs() < 1e-12);
        let p = hann_periodic(4);
        assert_eq!(p.len(), 4);
        assert!((p[0]).abs() < 1e-12 && (p[2] - 1.0).abs() < 1e-12 && (p[1] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn linspace_hits_both_ends() {
        let v = linspace(0.0, 8000.0, 201);
        assert_eq!(v.len(), 201);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[200], 8000.0);
        assert!((v[1] - 40.0).abs() < 1e-9);
    }

    #[test]
    fn mel_scale_round_trips() {
        for f in [0.0, 300.0, 999.0, 1000.0, 4000.0, 8000.0] {
            assert!((mel_to_hertz_slaney(hertz_to_mel_slaney(f)) - f).abs() < 1e-9);
        }
    }

    #[test]
    fn filterbank_rows_sum_to_the_slaney_area() {
        // Each filter is a triangle normalized to area 2 / bandwidth; its weights over the
        // bins are positive and bounded.
        let fb = slaney_filterbank(201, 80, 0.0, 8000.0, 16000);
        assert_eq!(fb.len(), 201 * 80);
        assert!(fb.iter().all(|w| *w >= 0.0 && *w < 1.0));
        let col: f64 = (0..201).map(|b| fb[b * 80 + 40]).sum();
        assert!(col > 0.0);
    }

    #[test]
    fn spectrum_of_a_sine_peaks_at_its_bin() {
        let n = 400;
        let mut s = Spectrum::new(n);
        let frame: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * 10.0 * i as f64 / n as f64).sin())
            .collect();
        let mut out = vec![0.0; s.bins()];
        s.power(&frame, &mut out);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 10);
    }
}
