//! What every engine shares: the takes, the audio each read gets, scoring with Ghost's own
//! scorer, and the process's memory. Nothing here writes or prints a transcript.

use std::path::Path;

use ghost_voice::perf::hear::plan::{self, Channel, Item, Kind};
use ghost_voice::perf::hear::score::{self, Class};
use ghost_voice::perf::hear::{Read, TakeResult};

pub const SR: usize = 16_000;
/// 100 ms, the frame a streaming model is fed.
pub const FRAME: usize = 1_600;

/// The recorded session's takes on the channel Ghost hears (`*.aec.wav`), in id order.
pub fn takes(session: &Path) -> anyhow::Result<Vec<Item>> {
    let (items, missing) = plan::recorded(session, Channel::Aec, &[])?;
    anyhow::ensure!(missing.is_empty(), "takes without audio: {missing:?}");
    Ok(items)
}

pub fn spoken(item: &Item) -> bool {
    item.kind != Kind::Silence
}

/// The audio every engine reads for a take: the whole take, `TRAILING_SECS` of zeros after
/// it and Ghost's −72 dBFS dither under it, exactly as the hear harness's `file` read.
pub fn audio(item: &Item) -> Vec<f32> {
    let mut pcm = item.pcm.clone();
    pcm.extend(std::iter::repeat_n(
        0i16,
        (ghost_voice::stt::TRAILING_SECS * SR as f64) as usize,
    ));
    ghost_voice::stt::dithered(&ghost_infer::audio::to_f32(&pcm))
}

/// `text` scored against what was said, as the hear harness scores a read.
pub fn scored(item: &Item, text: &str) -> Read {
    let s = score::score(&item.text, text, &item.terms);
    Read {
        words: s.hyp_words,
        sub: s.sub,
        del: s.del,
        ins: s.ins,
        classes: Class::ALL
            .iter()
            .map(|&c| {
                let k = s.class(c);
                [k.words, k.sub, k.del, k.ins]
            })
            .collect(),
        terms_said: s.terms_said,
        terms_heard: s.terms_heard,
        terms_inserted: s.terms_inserted,
        fillers: s.fillers_heard,
        segments: 1,
        ..Read::default()
    }
}

pub fn take_result(item: &Item, modes: Vec<(&str, Read)>) -> TakeResult {
    TakeResult {
        id: item.id.clone(),
        block: item.block.clone(),
        condition: item.condition.clone(),
        kind: item.kind,
        tags: item.tags.clone(),
        source: item.source.clone(),
        seconds: item.seconds(),
        ref_words: score::normalize(&item.text).len(),
        modes: modes.into_iter().map(|(m, r)| (m.to_string(), r)).collect(),
    }
}

/// (VmRSS, VmHWM) of this process in MB.
pub fn memory_mb() -> (f64, f64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map_or(0.0, |kb| kb / 1024.0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Read a file once so a timed load finds it in the page cache (every load here is warm).
pub fn warm_page_cache(path: &Path) {
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = std::io::copy(&mut f, &mut std::io::sink());
    }
}

pub fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    Some(v[((q * (v.len() - 1) as f64).round() as usize).min(v.len() - 1)])
}

/// Words as the scorer counts them (fillers out, numbers as digits).
pub fn words(text: &str) -> Vec<String> {
    score::normalize(text)
}

/// When a growing hypothesis first holds `n` words, on a simulated real-time clock.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Arrival {
    /// Seconds from the take's start (audio position plus the compute it waited on).
    pub first1: Option<f64>,
    pub first2: Option<f64>,
    /// The first two words shown were the first two said.
    pub first2_right: Option<bool>,
}

impl Arrival {
    pub fn see(&mut self, at: f64, hyp: &[String], reference: &[String]) {
        if self.first1.is_none() && !hyp.is_empty() {
            self.first1 = Some(at);
        }
        if self.first2.is_none() && hyp.len() >= 2 {
            self.first2 = Some(at);
            self.first2_right = Some(reference.len() >= 2 && hyp[..2] == reference[..2]);
        }
    }
}
