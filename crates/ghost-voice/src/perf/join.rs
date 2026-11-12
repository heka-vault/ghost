//! What a sentence rendered in two pieces sounds like at the join, measured rather than
//! heard (`perf micro --only tts-join`): the silence between the pieces against the pause
//! Kokoro leaves at the same place when it renders the sentence whole, and how far the
//! loudness and the pitch jump there against the whole render. Nobody listens here; these
//! are the objective signs a seam would leave.

use serde_json::{Value, json};

/// Analysis frames: 10 ms.
const FRAME_SECS: f64 = 0.010;
/// Quieter than this (dBFS, a frame's RMS) is a pause.
const QUIET_DBFS: f64 = -45.0;
/// The stretch either side of the join that is compared: 60 ms of voice.
const EDGE_SECS: f64 = 0.060;

fn frame_db(pcm: &[i16], rate: u32) -> Vec<f64> {
    let n = ((f64::from(rate) * FRAME_SECS) as usize).max(1);
    pcm.chunks(n)
        .map(|c| {
            let p = c.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / c.len() as f64;
            20.0 * (p.sqrt() / 32768.0).max(1e-9).log10()
        })
        .collect()
}

fn ms(frames: usize) -> f64 {
    frames as f64 * FRAME_SECS * 1000.0
}

/// Quiet frames at the start and at the end.
fn edges(db: &[f64]) -> (usize, usize) {
    let head = db.iter().take_while(|d| **d < QUIET_DBFS).count();
    let tail = db.iter().rev().take_while(|d| **d < QUIET_DBFS).count();
    (head, tail.min(db.len() - head))
}

fn rms_db(pcm: &[i16]) -> f64 {
    if pcm.is_empty() {
        return -120.0;
    }
    let p = pcm.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / pcm.len() as f64;
    20.0 * (p.sqrt() / 32768.0).max(1e-9).log10()
}

/// The fundamental of a stretch of voice by autocorrelation, 70 to 400 Hz; None when it
/// is not voiced enough to say.
pub fn f0(pcm: &[i16], rate: u32) -> Option<f64> {
    let x: Vec<f64> = pcm.iter().map(|&s| f64::from(s)).collect();
    let mean = x.iter().sum::<f64>() / x.len().max(1) as f64;
    let x: Vec<f64> = x.iter().map(|v| v - mean).collect();
    let energy: f64 = x.iter().map(|v| v * v).sum();
    if energy <= 0.0 {
        return None;
    }
    let (lo, hi) = (
        (f64::from(rate) / 400.0) as usize,
        (f64::from(rate) / 70.0) as usize,
    );
    if hi >= x.len() {
        return None;
    }
    let mut best = (0usize, 0.0f64);
    for lag in lo..=hi {
        let r: f64 = (0..x.len() - lag).map(|i| x[i] * x[i + lag]).sum::<f64>() / energy;
        if r > best.1 {
            best = (lag, r);
        }
    }
    (best.1 > 0.3).then(|| f64::from(rate) / best.0 as f64)
}

fn semitones(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(12.0 * (b? / a?).log2())
}

/// One sentence's join.
#[derive(Debug, Clone, Default)]
pub struct Join {
    /// Silence at the end of the first piece and the start of the second, ms.
    pub tail_ms: f64,
    pub head_ms: f64,
    /// The pause Kokoro left at the same place in the whole render, ms (0: none).
    pub natural_ms: f64,
    /// Seconds of voice (edges trimmed): whole, and the two pieces together.
    pub voiced_whole: f64,
    pub voiced_pieces: f64,
    /// dB of the 60 ms before the join, pieces minus whole; the same after it.
    pub energy_before_db: f64,
    pub energy_after_db: f64,
    /// Pitch, semitones, pieces minus whole, before and after the join.
    pub pitch_before_st: Option<f64>,
    pub pitch_after_st: Option<f64>,
    /// Pitch across the join itself, semitones: second piece's start minus first's end,
    /// and the same in the whole render.
    pub jump_pieces_st: Option<f64>,
    pub jump_whole_st: Option<f64>,
}

impl Join {
    pub fn gap_ms(&self) -> f64 {
        self.tail_ms + self.head_ms
    }

    pub fn line(&self) -> String {
        let st = |x: Option<f64>| x.map_or("-".to_string(), |v| format!("{v:+.1}"));
        format!(
            "join gap {:.0} ms (tail {:.0} + head {:.0}) against {:.0} ms whole; voice {:.2} s in two, {:.2} s whole; \
             loudness {:+.1} / {:+.1} dB before / after; pitch {} / {} st; jump across {} st in two, {} st whole",
            self.gap_ms(),
            self.tail_ms,
            self.head_ms,
            self.natural_ms,
            self.voiced_pieces,
            self.voiced_whole,
            self.energy_before_db,
            self.energy_after_db,
            st(self.pitch_before_st),
            st(self.pitch_after_st),
            st(self.jump_pieces_st),
            st(self.jump_whole_st),
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "tail_ms": self.tail_ms, "head_ms": self.head_ms, "gap_ms": self.gap_ms(),
            "natural_ms": self.natural_ms, "voiced_whole": self.voiced_whole,
            "voiced_pieces": self.voiced_pieces, "energy_before_db": self.energy_before_db,
            "energy_after_db": self.energy_after_db, "pitch_before_st": self.pitch_before_st,
            "pitch_after_st": self.pitch_after_st, "jump_pieces_st": self.jump_pieces_st,
            "jump_whole_st": self.jump_whole_st,
        })
    }
}

/// The join of `first` and `second` against `whole`, the same sentence rendered once.
pub fn measure(whole: &[i16], first: &[i16], second: &[i16], rate: u32) -> Join {
    let fl = ((f64::from(rate) * FRAME_SECS) as usize).max(1);
    let (dw, da, db) = (
        frame_db(whole, rate),
        frame_db(first, rate),
        frame_db(second, rate),
    );
    let (wh, wt) = edges(&dw);
    let (ah, at) = edges(&da);
    let (bh, bt) = edges(&db);
    let voiced = |n: usize, h: usize, t: usize| n.saturating_sub(h + t);
    let (va, vb, vw) = (
        voiced(da.len(), ah, at),
        voiced(db.len(), bh, bt),
        voiced(dw.len(), wh, wt),
    );
    // Where the join falls in the whole render: in proportion to the voice either side,
    // then the nearest pause within 300 ms of it, if Kokoro left one.
    let guess = wh as f64 + vw as f64 * va as f64 / (va + vb).max(1) as f64;
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (start, len) of quiet runs inside
    let mut i = wh;
    while i < dw.len() - wt {
        if dw[i] < QUIET_DBFS {
            let s = i;
            while i < dw.len() - wt && dw[i] < QUIET_DBFS {
                i += 1;
            }
            if i - s >= 2 {
                runs.push((s, i - s));
            }
        } else {
            i += 1;
        }
    }
    let near = runs
        .iter()
        .map(|&(s, l)| ((s as f64 + l as f64 / 2.0 - guess).abs(), s, l))
        .filter(|(d, _, _)| *d <= 30.0)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let (pause_start, pause_len) = match near {
        Some((_, s, l)) => (s, l),
        None => (guess.round() as usize, 0),
    };
    let edge = (f64::from(rate) * EDGE_SECS) as usize;
    let slice = |pcm: &'_ [i16], from: usize, to: usize| -> Vec<i16> {
        pcm[from.min(pcm.len())..to.min(pcm.len())].to_vec()
    };
    // The 60 ms of voice before the join and after it, in each.
    let a_end = (da.len() - at) * fl;
    let a_before = slice(first, a_end.saturating_sub(edge), a_end);
    let b_start = bh * fl;
    let b_after = slice(second, b_start, b_start + edge);
    let w_join = pause_start * fl;
    let w_before = slice(whole, w_join.saturating_sub(edge), w_join);
    let w_resume = (pause_start + pause_len) * fl;
    let w_after = slice(whole, w_resume, w_resume + edge);
    let (fa, fb, fwa, fwb) = (
        f0(&a_before, rate),
        f0(&b_after, rate),
        f0(&w_before, rate),
        f0(&w_after, rate),
    );
    Join {
        tail_ms: ms(at),
        head_ms: ms(bh),
        natural_ms: ms(pause_len),
        voiced_whole: vw as f64 * FRAME_SECS,
        voiced_pieces: (va + vb) as f64 * FRAME_SECS,
        energy_before_db: rms_db(&a_before) - rms_db(&w_before),
        energy_after_db: rms_db(&b_after) - rms_db(&w_after),
        pitch_before_st: semitones(fwa, fa),
        pitch_after_st: semitones(fwb, fb),
        jump_pieces_st: semitones(fa, fb),
        jump_whole_st: semitones(fwa, fwb),
    }
}

/// The rows' means, as one line.
pub fn summarize(rows: &[Value]) -> String {
    let mean = |key: &str| -> String {
        let xs: Vec<f64> = rows
            .iter()
            .filter_map(|r| r["join"][key].as_f64())
            .collect();
        if xs.is_empty() {
            return "-".into();
        }
        format!("{:.1}", xs.iter().sum::<f64>() / xs.len() as f64)
    };
    let abs_mean = |key: &str| -> String {
        let xs: Vec<f64> = rows
            .iter()
            .filter_map(|r| r["join"][key].as_f64())
            .map(f64::abs)
            .collect();
        if xs.is_empty() {
            return "-".into();
        }
        format!("{:.1}", xs.iter().sum::<f64>() / xs.len() as f64)
    };
    let saved: Vec<f64> = rows
        .iter()
        .filter_map(|r| Some(r["render_whole"].as_f64()? - r["render_first"].as_f64()?))
        .collect();
    let saved_usual: Vec<f64> = rows
        .iter()
        .filter_map(|r| Some(r["render_usual"].as_f64()? - r["render_first"].as_f64()?))
        .collect();
    let underruns = rows
        .iter()
        .filter(|r| r["render_rest"].as_f64() > r["first_audio_secs"].as_f64())
        .count();
    format!(
        "  mean over {} sentences: first audio {:.0} ms sooner than whole, {:.0} ms sooner than the usual rule; join gap {} ms against {} ms whole; \
         voice {} s in two against {} s whole; loudness {} / {} dB; |pitch| {} / {} st before / after; \
         |jump across| {} st in two, {} st whole; the rest not ready in time {underruns}",
        rows.len(),
        saved.iter().sum::<f64>() / saved.len().max(1) as f64 * 1000.0,
        saved_usual.iter().sum::<f64>() / saved_usual.len().max(1) as f64 * 1000.0,
        mean("gap_ms"),
        mean("natural_ms"),
        mean("voiced_pieces"),
        mean("voiced_whole"),
        mean("energy_before_db"),
        mean("energy_after_db"),
        abs_mean("pitch_before_st"),
        abs_mean("pitch_after_st"),
        abs_mean("jump_pieces_st"),
        abs_mean("jump_whole_st"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, secs: f64, amp: f64) -> Vec<i16> {
        (0..(24_000.0 * secs) as usize)
            .map(|i| (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / 24_000.0).sin()) as i16)
            .collect()
    }

    #[test]
    fn a_join_with_the_whole_renders_pause_measures_as_natural() {
        let silence = |s: f64| vec![0i16; (24_000.0 * s) as usize];
        let whole: Vec<i16> = [tone(150.0, 0.5, 8000.0), silence(0.15), tone(150.0, 0.5, 8000.0)]
            .concat();
        let first: Vec<i16> = [tone(150.0, 0.5, 8000.0), silence(0.05)].concat();
        let second: Vec<i16> = [silence(0.03), tone(150.0, 0.5, 8000.0)].concat();
        let j = measure(&whole, &first, &second, 24_000);
        assert!((j.natural_ms - 150.0).abs() <= 20.0, "{j:?}");
        assert!((j.gap_ms() - 80.0).abs() <= 20.0, "{j:?}");
        assert!(j.energy_before_db.abs() < 1.0 && j.energy_after_db.abs() < 1.0, "{j:?}");
        assert!(j.jump_pieces_st.unwrap().abs() < 0.5, "{j:?}");
        let f = f0(&tone(200.0, 0.06, 8000.0), 24_000).unwrap();
        assert!((f - 200.0).abs() < 5.0, "{f}");
    }
}
