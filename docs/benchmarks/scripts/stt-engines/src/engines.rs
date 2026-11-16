//! The reads: transcribe.cpp batch and streaming, Ghost's own Parakeet through ort, and
//! Ghost's interim re-reads simulated on ort. Each writes `results.json` (Ghost's hear
//! format, numbers only) and `numbers.json` (timings, memory, arrival times).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ghost_voice::perf::hear::Results;
use ghost_voice::perf::hear::plan::Item;
use serde_json::{Value, json};
use transcribe_cpp::{
    Backend, BackendMask, Model, ModelOptions, RunOptions, SessionOptions, StreamOptions,
    TimestampKind,
};

use crate::common::{self, Arrival, FRAME, SR};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Device {
    Cpu,
    Vulkan,
}

pub struct Loaded {
    pub model: Model,
    pub session: transcribe_cpp::Session,
    pub load_secs: f64,
    pub info: Value,
}

pub fn init_backends(device: Device) -> anyhow::Result<()> {
    transcribe_cpp::init_logging();
    match device {
        Device::Cpu => transcribe_cpp::init_backends_with(None::<&Path>, BackendMask::CPU)?,
        Device::Vulkan => transcribe_cpp::init_backends_default()?,
    }
    Ok(())
}

pub fn load_tc(path: &Path, device: Device, threads: i32) -> anyhow::Result<Loaded> {
    common::warm_page_cache(path);
    let t = Instant::now();
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend: match device {
                Device::Cpu => Backend::Cpu,
                Device::Vulkan => Backend::Vulkan,
            },
            ..Default::default()
        },
    )?;
    let session = model.session_with(&SessionOptions {
        n_threads: threads,
        ..Default::default()
    })?;
    let load_secs = t.elapsed().as_secs_f64();
    let caps = model.capabilities()?;
    let device_name = model
        .device()
        .ok()
        .map(|d| format!("{} ({})", d.description, d.kind))
        .unwrap_or_default();
    let info = json!({
        "arch": model.arch(),
        "variant": model.variant(),
        "backend": model.backend(),
        "device": device_name,
        "streaming": caps.supports_streaming,
        "max_audio_ms": caps.max_audio_ms,
    });
    Ok(Loaded {
        model,
        session,
        load_secs,
        info,
    })
}

fn write(out: &Path, results: &Results, numbers: &Value) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(results)?,
    )?;
    std::fs::write(
        out.join("numbers.json"),
        serde_json::to_string_pretty(numbers)?,
    )?;
    Ok(())
}

fn meta(label: &str, what: &str) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("label".into(), json!(label));
    m.insert("speech to text".into(), json!(what));
    m.insert("source".into(), json!("recorded takes, session 2026-11-06"));
    m.insert("channel".into(), json!("aec"));
    m
}

/// Each take read whole (`file`), and with `STT_ENGINES_SPANS` naming a spans file
/// (`stt-engines spans`), its turn span too (`turn`), as the harness reads them.
fn read_takes(
    items: &[Item],
    mut read: impl FnMut(&[f32]) -> String,
) -> anyhow::Result<(Vec<ghost_voice::perf::hear::TakeResult>, Vec<Value>)> {
    let spans = match std::env::var_os("STT_ENGINES_SPANS") {
        Some(p) => Some(crate::span::read(Path::new(&p))?),
        None => None,
    };
    let mut takes = Vec::new();
    let mut per_take = Vec::new();
    for item in items {
        let audio = common::audio(item);
        let t = Instant::now();
        let text = read(&audio);
        let secs = t.elapsed().as_secs_f64();
        let mut r = common::scored(item, &text);
        r.close_secs = Some(secs);
        r.read_secs = secs;
        let mut modes = vec![("file", r)];
        let mut row =
            json!({"id": item.id, "audio_secs": audio.len() as f64 / SR as f64, "read_secs": secs});
        if let Some(spans) = &spans {
            let span = spans
                .get(&item.id)
                .ok_or_else(|| anyhow::anyhow!("no span for {}", item.id))?;
            let (text, secs) = match span {
                Some(sp) => {
                    let a = crate::span::turn_audio(item, *sp);
                    row["turn_audio_secs"] = json!(a.len() as f64 / SR as f64);
                    let t = Instant::now();
                    let text = read(&a);
                    let secs = t.elapsed().as_secs_f64();
                    row["turn_read_secs"] = json!(secs);
                    (text, Some(secs))
                }
                None => (String::new(), None),
            };
            let mut r = common::scored(item, &text);
            r.close_secs = secs;
            r.read_secs = secs.unwrap_or(0.0);
            r.segments = usize::from(span.is_some());
            modes.push(("turn", r));
        }
        eprintln!(
            "  {} {:>5.1}s audio  read {:.3}s  errors {}",
            item.id,
            item.seconds(),
            secs,
            modes
                .iter()
                .map(|(m, r)| format!("{m} {}", r.errors()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        per_take.push(row);
        takes.push(common::take_result(item, modes));
    }
    Ok((takes, per_take))
}

/// Every take read whole, once, after one warm-up read.
pub fn tc_batch(
    label: &str,
    model_path: &Path,
    device: Device,
    threads: i32,
    items: &[Item],
    out: &Path,
) -> anyhow::Result<()> {
    init_backends(device)?;
    let (rss0, _) = common::memory_mb();
    let mut l = load_tc(model_path, device, threads)?;
    let (rss_loaded, _) = common::memory_mb();
    let opts = RunOptions {
        timestamps: TimestampKind::None,
        ..Default::default()
    };
    let first = items
        .iter()
        .find(|i| common::spoken(i))
        .expect("a spoken take");
    let t = Instant::now();
    l.session.run(&common::audio(first), &opts)?;
    let warmup = t.elapsed().as_secs_f64();
    let (takes, per_take) = read_takes(items, |a| match l.session.run(a, &opts) {
        Ok(tr) => tr.text,
        Err(e) => {
            eprintln!("  a read failed: {e}");
            String::new()
        }
    })?;
    let (rss_after, hwm) = common::memory_mb();
    let numbers = json!({
        "label": label, "engine": "transcribe-cpp 0.4.2", "model": model_path.file_name().unwrap().to_string_lossy(),
        "device": format!("{device:?}"), "threads": threads, "info": l.info,
        "load_secs": l.load_secs, "warmup_read_secs": warmup,
        "rss_mb_before_load": rss0, "rss_mb_after_load": rss_loaded, "rss_mb_after_reads": rss_after, "hwm_mb": hwm,
        "takes": per_take,
    });
    let results = Results {
        meta: meta(
            label,
            &format!("{label} via transcribe-cpp, {threads} threads"),
        ),
        takes,
    };
    drop(l);
    write(out, &results, &numbers)
}

/// The end of the first and second spoken words in each take, from Parakeet TDT v3's word
/// timestamps: the clock arrival times are measured against. Numbers only.
pub fn clock(model_path: &Path, items: &[Item], out: &Path) -> anyhow::Result<()> {
    init_backends(Device::Cpu)?;
    let mut l = load_tc(model_path, Device::Cpu, 4)?;
    let opts = RunOptions {
        timestamps: TimestampKind::Word,
        ..Default::default()
    };
    let mut clock = BTreeMap::new();
    for item in items.iter().filter(|i| common::spoken(i)) {
        let tr = l.session.run(&common::audio(item), &opts)?;
        let mut ends = Vec::new();
        for w in &tr.words {
            for _ in common::words(&w.text) {
                ends.push(w.t1_ms as f64 / 1000.0);
            }
            if ends.len() >= 2 {
                break;
            }
        }
        let starts: Vec<f64> = tr
            .words
            .iter()
            .find(|w| !common::words(&w.text).is_empty())
            .map(|w| vec![w.t0_ms as f64 / 1000.0])
            .unwrap_or_default();
        clock.insert(
            item.id.clone(),
            json!({"first_start": starts.first(), "end1": ends.first(), "end2": ends.get(1)}),
        );
    }
    std::fs::write(out, serde_json::to_string_pretty(&clock)?)?;
    Ok(())
}

/// Did the shown words change anywhere but in the last word? (A last word growing token by
/// token is not a rewrite a reader would notice; anything before it is.)
fn rewritten(old: &[String], new: &[String]) -> (bool, bool) {
    let strict = !(new.len() >= old.len() && new[..old.len()] == old[..]);
    let keep = old.len().saturating_sub(1);
    let loose = !(new.len() >= keep && new[..keep] == old[..keep]);
    (strict, loose)
}

/// Each take fed in 100 ms frames on a simulated real-time clock (frame k arrives at its
/// end; compute starts when both it and the previous frame's compute are done), then
/// finalized.
pub fn tc_stream(
    label: &str,
    model_path: &Path,
    device: Device,
    threads: i32,
    knob: Option<&str>,
    items: &[Item],
    out: &Path,
) -> anyhow::Result<()> {
    init_backends(device)?;
    let mut l = load_tc(model_path, device, threads)?;
    let (rss_loaded, _) = common::memory_mb();
    let run = RunOptions::default();
    let sopts = stream_options(knob)?;
    // Warm-up: one take streamed and thrown away.
    {
        let first = items
            .iter()
            .find(|i| common::spoken(i))
            .expect("a spoken take");
        let mut s = l.session.stream(&run, &sopts)?;
        for f in common::audio(first).chunks(FRAME) {
            s.feed(f)?;
        }
        s.finalize()?;
    }
    let mut takes = Vec::new();
    let mut per_take = Vec::new();
    for item in items {
        let audio = common::audio(item);
        let reference = common::words(&item.text);
        let end = audio.len() as f64 / SR as f64;
        let mut s = l.session.stream(&run, &sopts)?;
        let (mut clock, mut feed_secs, mut max_feed) = (0.0f64, 0.0f64, 0.0f64);
        let mut shown: Vec<String> = Vec::new();
        let mut committed: Vec<String> = Vec::new();
        let (mut changes, mut strict, mut loose, mut committed_rewrites) = (0, 0, 0, 0);
        let mut arrival = Arrival::default();
        let mut fed = 0usize;
        for f in audio.chunks(FRAME) {
            fed += f.len();
            let arrive = fed as f64 / SR as f64;
            let start = clock.max(arrive);
            let t = Instant::now();
            let up = s.feed(f)?;
            let text = (up.committed_changed || up.tentative_changed).then(|| s.text());
            let dt = t.elapsed().as_secs_f64();
            feed_secs += dt;
            max_feed = max_feed.max(dt);
            clock = start + dt;
            if let Some(text) = text {
                let now = common::words(&text.full);
                if now != shown {
                    changes += 1;
                    let (a, b) = rewritten(&shown, &now);
                    strict += usize::from(a);
                    loose += usize::from(b);
                    shown = now;
                    arrival.see(clock, &shown, &reference);
                }
                let c = common::words(&text.committed);
                if !(c.len() >= committed.len() && c[..committed.len()] == committed[..]) {
                    committed_rewrites += 1;
                }
                committed = c;
            }
        }
        let backlog = (clock - end).max(0.0);
        let t = Instant::now();
        s.finalize()?;
        let final_text = s.text().full;
        let fin = t.elapsed().as_secs_f64();
        drop(s);
        let mut r = common::scored(item, &final_text);
        r.close_secs = Some(backlog + fin);
        r.read_secs = feed_secs + fin;
        // Rewrites the finalize made to what was on screen.
        let fw = common::words(&final_text);
        let (fs, fl) = rewritten(&shown, &fw);
        eprintln!(
            "  {} {:>5.1}s  feed {:.2}s (max frame {:.0} ms)  left {:.3}s  changes {} rewrites {}/{}  errors {}",
            item.id,
            item.seconds(),
            feed_secs,
            max_feed * 1000.0,
            backlog + fin,
            changes,
            loose,
            strict,
            r.errors()
        );
        per_take.push(json!({
            "id": item.id, "spoken": common::spoken(item), "audio_secs": end,
            "feed_secs": feed_secs, "max_frame_secs": max_feed, "backlog_secs": backlog, "finalize_secs": fin,
            "changes": changes, "rewrites_strict": strict, "rewrites": loose, "committed_rewrites": committed_rewrites,
            "final_rewrote_shown": fl, "final_rewrote_shown_strict": fs,
            "arrival": arrival,
        }));
        takes.push(common::take_result(item, vec![("file", r)]));
    }
    let (rss_after, hwm) = common::memory_mb();
    let numbers = json!({
        "label": label, "engine": "transcribe-cpp 0.4.2 stream", "model": model_path.file_name().unwrap().to_string_lossy(),
        "device": format!("{device:?}"), "threads": threads, "info": l.info, "frame_ms": 100, "stream_setting": knob.unwrap_or("model default"),
        "load_secs": l.load_secs, "rss_mb_after_load": rss_loaded, "rss_mb_after_reads": rss_after, "hwm_mb": hwm,
        "takes": per_take,
    });
    let results = Results {
        meta: meta(
            label,
            &format!("{label} streamed via transcribe-cpp, {threads} threads"),
        ),
        takes,
    };
    write(out, &results, &numbers)
}

/// `rc=N`: a cache-aware model's lookahead in encoder frames (Nemotron); `lcr=L,C,R`: a
/// buffered model's left, chunk and right context in ms (Parakeet Unified); none: the
/// model's default, its max-accuracy and max-latency entry, which Handy uses.
fn stream_options(knob: Option<&str>) -> anyhow::Result<StreamOptions> {
    use transcribe_cpp::{ParakeetBufferedStreamOptions, ParakeetStreamOptions, StreamExtension};
    let family = match knob {
        None => None,
        Some(k) if k.starts_with("rc=") => {
            Some(StreamExtension::ParakeetStream(ParakeetStreamOptions {
                att_context_right: Some(k[3..].parse()?),
            }))
        }
        Some(k) if k.starts_with("lcr=") => {
            let v: Vec<i32> = k[4..]
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            anyhow::ensure!(v.len() == 3, "lcr=L,C,R");
            Some(StreamExtension::ParakeetBuffered(
                ParakeetBufferedStreamOptions {
                    left_ms: Some(v[0]),
                    chunk_ms: Some(v[1]),
                    right_ms: Some(v[2]),
                },
            ))
        }
        Some(k) => anyhow::bail!("stream setting {k}?"),
    };
    Ok(StreamOptions {
        family,
        ..Default::default()
    })
}

fn ort_paths(quant: &str) -> anyhow::Result<ghost_infer::ParakeetPaths> {
    let home = PathBuf::from(std::env::var("HOME")?);
    let cache = home.join(".cache/ghost");
    Ok(match quant {
        "fp32" => ghost_infer::ParakeetPaths::ghost_cache(&cache, None),
        "int8" => ghost_infer::ParakeetPaths::huggingface_cache()
            .ok_or_else(|| anyhow::anyhow!("no int8 snapshot"))?,
        q => anyhow::bail!("quantization {q}?"),
    })
}

pub fn load_ort(quant: &str, threads: usize) -> anyhow::Result<(ghost_infer::Parakeet, f64)> {
    let paths = ort_paths(quant)?;
    for p in [&paths.encoder, &paths.decoder_joint] {
        common::warm_page_cache(p);
        common::warm_page_cache(&p.with_extension("onnx.data"));
    }
    let t = Instant::now();
    let m = ghost_infer::Parakeet::load(&paths, threads)?;
    Ok((m, t.elapsed().as_secs_f64()))
}

/// Ghost's own Parakeet through ort, read as the harness's `file` read, with the same
/// timings and memory as the transcribe.cpp runs.
pub fn ort_batch(label: &str, quant: &str, items: &[Item], out: &Path) -> anyhow::Result<()> {
    let (rss0, _) = common::memory_mb();
    let (mut m, load_secs) = load_ort(quant, 4)?;
    let (rss_loaded, _) = common::memory_mb();
    let first = items
        .iter()
        .find(|i| common::spoken(i))
        .expect("a spoken take");
    let t = Instant::now();
    m.transcribe(&common::audio(first))?;
    let warmup = t.elapsed().as_secs_f64();
    let (takes, per_take) = read_takes(items, |a| {
        m.transcribe(a).unwrap_or_default().trim().to_string()
    })?;
    let (rss_after, hwm) = common::memory_mb();
    let numbers = json!({
        "label": label, "engine": "ort 1.24.4 (ghost_infer::Parakeet)", "model": format!("parakeet-tdt-0.6b-v3 {quant}"),
        "device": "Cpu", "threads": 4, "load_secs": load_secs, "warmup_read_secs": warmup,
        "rss_mb_before_load": rss0, "rss_mb_after_load": rss_loaded, "rss_mb_after_reads": rss_after, "hwm_mb": hwm,
        "takes": per_take,
    });
    let results = Results {
        meta: meta(
            label,
            &format!("parakeet-tdt-0.6b-v3 ({quant}) via ort, 4 threads"),
        ),
        takes,
    };
    write(out, &results, &numbers)
}

/// Ghost's barge-in path today, simulated: from the VAD's start (the manifest's speech
/// start, which `speech_span` moved back by the VAD's start time, plus that time again and
/// one 32 ms step), every `interim` seconds the segment so far with its 1 s pre-roll, at
/// most the last 4 s (`INTERIM_WINDOW_SECS`), plus the trailing silence, is read again; a
/// tick is skipped while a read is in flight (`InterimSchedule`).
pub fn ort_interim(
    label: &str,
    quant: &str,
    interim: f64,
    session: &Path,
    items: &[Item],
    out: &Path,
) -> anyhow::Result<()> {
    let manifest = std::fs::read_to_string(session.join("takes.jsonl"))?;
    let speech: BTreeMap<String, f64> = ghost_voice::perf::hear::plan::read_manifest(&manifest)
        .into_iter()
        .filter_map(|e| Some((e.id, e.speech?.0)))
        .collect();
    let vad_start_after = 0.2 + 512.0 / SR as f64; // VadParams start_secs and one step
    let (mut m, _) = load_ort(quant, 4)?;
    let window = (ghost_infer::parakeet::INTERIM_WINDOW_SECS as f64 * SR as f64) as usize;
    let trailing = (ghost_voice::stt::TRAILING_SECS * SR as f64) as usize;
    m.transcribe(&common::audio(&items[0]))?;
    let mut per_take = Vec::new();
    for item in items.iter().filter(|i| common::spoken(i)) {
        let reference = common::words(&item.text);
        let pcm = &item.pcm;
        let mut arrival = Arrival::default();
        let (mut clock, mut reads, mut busy_secs) = (0.0f64, 0usize, 0.0f64);
        let Some(&speech_start) = speech.get(&item.id) else {
            continue;
        };
        let vad_start = speech_start + vad_start_after;
        let seg_start =
            ((vad_start - ghost_voice::stt::PRE_ROLL_SECS).max(0.0) * SR as f64) as usize;
        let mut k = 1;
        loop {
            let tick = vad_start + k as f64 * interim;
            let pos = (tick * SR as f64) as usize;
            if pos > pcm.len() {
                break;
            }
            k += 1;
            if clock > tick {
                continue; // busy: skipped
            }
            let seg = &pcm[pos.saturating_sub(window).max(seg_start)..pos];
            let mut a = ghost_infer::audio::to_f32(seg);
            a.extend(std::iter::repeat_n(0.0f32, trailing));
            let a = ghost_voice::stt::dithered(&a);
            let t = Instant::now();
            let text = m.transcribe(&a).unwrap_or_default();
            let dt = t.elapsed().as_secs_f64();
            reads += 1;
            busy_secs += dt;
            clock = tick + dt;
            arrival.see(clock, &common::words(&text), &reference);
            if arrival.first2.is_some() {
                break;
            }
        }
        eprintln!(
            "  {} reads {} first1 {:?} first2 {:?}",
            item.id, reads, arrival.first1, arrival.first2
        );
        per_take.push(
            json!({"id": item.id, "reads": reads, "busy_secs": busy_secs, "arrival": arrival}),
        );
    }
    let numbers = json!({"label": label, "engine": "ort interim simulation", "interim_secs": interim, "quant": quant, "threads": 4, "takes": per_take});
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("numbers.json"),
        serde_json::to_string_pretty(&numbers)?,
    )?;
    Ok(())
}

pub fn devices() -> anyhow::Result<()> {
    init_backends(Device::Vulkan)?;
    for d in transcribe_cpp::devices() {
        println!(
            "{:?} {} | {} | {:?} | {} MB",
            d.index,
            d.kind,
            d.description,
            d.device_type,
            d.memory_total / (1 << 20)
        );
    }
    Ok(())
}
