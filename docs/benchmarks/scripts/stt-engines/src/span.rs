//! The turn span the hear harness's `turn` read uses: each take through Ghost's real
//! speech-in stage (`ParakeetStt` with Silero and the session's VAD settings, fed in 20 ms
//! frames on the audio's own clock, 2 s of −70 dBFS room after the take), from the first
//! segment's pre-roll to the last segment's end. This is `hear_stage` in
//! `crates/ghost-voice/src/perf/hear/mod.rs` (private there), copied here keeping only the
//! VAD's frames. Numbers only: sample ranges per take.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ghost_infer::{Silero, VadAnalyzer};
use ghost_voice::config::VoiceSettings;
use ghost_voice::frames::{Audio, Direction, Frame, Out, Stage};
use ghost_voice::perf::hear::plan::{self, Item};
use ghost_voice::stt::{PRE_ROLL_SECS, ParakeetStt, SAMPLE_RATE, SharedRecognizer, TRAILING_SECS};

use crate::common::SR;

/// As the harness: 20 ms frames, 2 s of room after the take.
const FRAME: usize = 320;
pub const TAIL_SECS: f64 = 2.0;

/// (start, end) in samples of the take with its 2 s tail, or None when no segment started.
pub type Spans = BTreeMap<String, Option<(usize, usize)>>;

/// The take with the room after it, as the stage heard it.
pub fn padded(item: &Item) -> Vec<i16> {
    let mut p = item.pcm.clone();
    p.extend(plan::room_noise(TAIL_SECS, -70.0, 99));
    p
}

/// The turn's audio as `read_once` reads it: the span, 0.2 s of zeros, the dither.
pub fn turn_audio(item: &Item, span: (usize, usize)) -> Vec<f32> {
    let p = padded(item);
    let mut seg = p[span.0..span.1.min(p.len())].to_vec();
    seg.extend(std::iter::repeat_n(
        0i16,
        (TRAILING_SECS * SR as f64) as usize,
    ));
    ghost_voice::stt::dithered(&ghost_infer::audio::to_f32(&seg))
}

async fn span_of(
    model: &SharedRecognizer,
    settings: &VoiceSettings,
    silero: &Path,
    pcm: &[i16],
) -> anyhow::Result<Option<(usize, usize)>> {
    let vad = VadAnalyzer::new(Silero::load(silero)?, settings.vad_params());
    let pos = Arc::new(AtomicUsize::new(0));
    let clock_pos = pos.clone();
    let clock = Arc::new(move || 1.0e9 + clock_pos.load(Ordering::Relaxed) as f64 / 16_000.0);
    let mut stt = ParakeetStt::new(model.clone(), Some(vad), settings.vad_params(), 0.0)
        .with_vad_clock(clock)
        .with_early_read(settings.stt_early_read)
        .with_early_read_after(settings.stt_early_read_after_secs);
    let (out, mut rx) = Out::capture();
    let mut audio = pcm.to_vec();
    audio.extend(plan::room_noise(TAIL_SECS, -70.0, 99));
    let (mut starts, mut stops) = (Vec::new(), Vec::new());
    let mut end = 0usize;
    for chunk in audio.chunks(FRAME) {
        end += chunk.len();
        pos.store(end, Ordering::Relaxed);
        stt.process(
            Frame::AudioIn(Audio::new(chunk.to_vec(), SAMPLE_RATE)),
            Direction::Down,
            &out,
        )
        .await;
        while let Ok(p) = rx.try_recv() {
            match p.frame {
                Frame::VadStarted { .. } => starts.push(end - chunk.len()),
                Frame::VadStopped { .. } => stops.push(end),
                _ => {}
            }
        }
    }
    stt.stopped().await;
    let pre_roll = (PRE_ROLL_SECS * f64::from(SAMPLE_RATE)) as usize;
    Ok(match (starts.first(), stops.last()) {
        (Some(&s), Some(&e)) if e > s => Some((s.saturating_sub(pre_roll), e)),
        _ => None,
    })
}

/// Every take's span, with Ghost's fp32 Parakeet in the stage as the harness has it.
pub fn spans(items: &[Item], out: &Path) -> anyhow::Result<()> {
    let settings = VoiceSettings::from_env()?;
    let v = settings.vad_params();
    eprintln!(
        "vad: confidence {}, start {} s, stop {} s, min volume {}; early read {:?}",
        v.confidence, v.start_secs, v.stop_secs, v.min_volume, settings.stt_early_read
    );
    let (parakeet, _) = crate::engines::load_ort("fp32", 4)?;
    let model: SharedRecognizer = Arc::new(Mutex::new(parakeet));
    let silero = settings.silero_model.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut spans = Spans::new();
    for item in items {
        let s = rt.block_on(span_of(&model, &settings, &silero, &item.pcm))?;
        spans.insert(item.id.clone(), s);
    }
    std::fs::write(out, serde_json::to_string_pretty(&spans)?)?;
    eprintln!(
        "spans: {} of {} takes have a turn",
        spans.values().filter(|s| s.is_some()).count(),
        spans.len()
    );
    Ok(())
}

pub fn read(path: &Path) -> anyhow::Result<Spans> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
