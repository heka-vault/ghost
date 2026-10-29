//! `audio-probe`: the engine against the live PipeWire graph, from a terminal.
//!
//! ```text
//! audio-probe list                                   nodes and their formats
//! audio-probe capture [--seconds N] [--node NAME] [--fallback]
//!                                                    frames from ghost.mic: levels and counts only
//! audio-probe loopback --sink NAME [--seconds N] [--node NAME]
//!                                                    capture to a playback on the given sink
//! audio-probe duck --pattern X [--seconds N] [--level L] [--state FILE]
//! audio-probe restore [--state FILE]
//! audio-probe chime --sink NAME [--down] [--thinking SECS]
//! ```
//!
//! Nothing from the microphone is stored or printed: `capture` and `loopback` print RMS
//! and peak per second and the counters. Play only into a null sink
//! (`pactl load-module module-null-sink sink_name=ghost-test`), never the operator's
//! speakers, unless they said so.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ghost_audio::{
    AudioSettings, Capture, CaptureConfig, Ducker, Playback, PlaybackConfig, Session, chime,
    default_state_file, restore_from_state,
};

struct Args {
    command: String,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse() -> Args {
        let mut argv = std::env::args().skip(1);
        let command = argv.next().unwrap_or_default();
        let mut flags = Vec::new();
        let mut argv = argv.peekable();
        while let Some(a) = argv.next() {
            let name = a.trim_start_matches("--").to_string();
            let value = match argv.peek() {
                Some(v) if !v.starts_with("--") => argv.next(),
                _ => None,
            };
            flags.push((name, value));
        }
        Args { command, flags }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn seconds(&self) -> u64 {
        self.get("seconds")
            .and_then(|s| s.parse().ok())
            .unwrap_or(5)
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GHOST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    let args = Args::parse();
    let result = match args.command.as_str() {
        "list" => list(),
        "capture" => capture(&args),
        "loopback" => loopback(&args),
        "duck" => duck(&args),
        "restore" => restore(&args),
        "chime" => play_chime(&args),
        other => {
            eprintln!("audio-probe: unknown command {other:?}; see the source for the list");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("audio-probe: {e}");
        std::process::exit(1);
    }
}

fn list() -> anyhow::Result<()> {
    let session = Session::connect("audio-probe")?;
    let nodes = session.nodes()?;
    let audio: Vec<_> = nodes
        .iter()
        .filter(|n| n.media_class().starts_with("Audio/") || n.media_class().starts_with("Stream/"))
        .collect();
    let bound = session.bind(audio.iter().copied())?;
    println!(
        "{:>4} {:>6} {:<10} {:<22} {:<22} {:<40} description",
        "id", "serial", "state", "class", "format", "name"
    );
    for b in &bound {
        let facts = b.facts();
        let format = facts
            .format
            .map(|f| f.to_string())
            .or_else(|| {
                let rate = b.props.get("audio.rate")?;
                let ch = b.props.get("audio.channels")?;
                Some(format!("({ch}ch {rate} Hz)"))
            })
            .unwrap_or_default();
        let volumes = facts
            .channel_volumes
            .map(|v| {
                let pct: Vec<String> = v
                    .iter()
                    .map(|&l| format!("{:.0}%", ghost_audio::duck::to_pulse(l) * 100.0))
                    .collect();
                format!(" vol {}", pct.join("/"))
            })
            .unwrap_or_default();
        println!(
            "{:>4} {:>6} {:<10} {:<22} {:<22} {:<40} {}{}",
            b.id,
            b.serial.map(|s| s.to_string()).unwrap_or_default(),
            facts.state.map(|s| s.to_string()).unwrap_or_default(),
            b.props.get("media.class").map_or("", String::as_str),
            format,
            b.name(),
            b.props.get("node.description").map_or("", String::as_str),
            volumes,
        );
    }
    Ok(())
}

fn capture_config(args: &Args) -> CaptureConfig {
    let settings = AudioSettings::from_env();
    CaptureConfig {
        node: args
            .get("node")
            .map(str::to_string)
            .or(settings.input_device)
            .or_else(|| Some("ghost.mic".into())),
        fallback_to_default: args.has("fallback"),
        ..CaptureConfig::default()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn capture(args: &Args) -> anyhow::Result<()> {
    let seconds = args.seconds();
    let mut capture = Capture::open(capture_config(args))?;
    println!(
        "capturing {} @ {} Hz, {} samples a frame, for {seconds} s (levels only; nothing stored)",
        capture.source(),
        capture.rate(),
        capture.frame_samples()
    );
    let started = Instant::now();
    let mut second = Instant::now();
    let mut frames_this_second = 0u64;
    let mut frames = 0u64;
    let mut rms_sum = 0.0f64;
    let mut peak = 0i16;
    let mut lag_max = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(seconds) {
        let Some(frame) = capture.next_frame().await else {
            break;
        };
        let lag = frame.captured_at.elapsed();
        lag_max = lag_max.max(lag);
        frames += 1;
        frames_this_second += 1;
        rms_sum += f64::from(frame.rms());
        peak = peak.max(frame.peak());
        if second.elapsed() >= Duration::from_secs(1) {
            let c = capture.counters();
            println!(
                "{:>5.1}s frames {frames_this_second:>3} rms {:>7.1} peak {peak:>6} | callbacks {} samples {} overruns {} empty {} delay {} smp | max ring lag {:?}",
                started.elapsed().as_secs_f32(),
                rms_sum / frames_this_second as f64,
                c.callbacks,
                c.samples,
                c.overruns,
                c.empty_callbacks,
                c.delay_samples,
                lag_max,
            );
            second = Instant::now();
            frames_this_second = 0;
            rms_sum = 0.0;
            peak = 0;
            lag_max = Duration::ZERO;
        }
    }
    let c = capture.counters();
    println!(
        "done: {frames} frames in {:.1} s; callbacks {} samples {} overruns {} empty {}; stream error: {}",
        started.elapsed().as_secs_f32(),
        c.callbacks,
        c.samples,
        c.overruns,
        c.empty_callbacks,
        capture.error().unwrap_or_else(|| "none".into()),
    );
    if c.overruns > 0 || c.empty_callbacks > 0 {
        std::process::exit(3);
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn loopback(args: &Args) -> anyhow::Result<()> {
    let seconds = args.seconds();
    let Some(sink) = args.get("sink") else {
        anyhow::bail!("loopback needs --sink NAME (a null sink, never the speakers)");
    };
    let capture_cfg = capture_config(args);
    let rate = capture_cfg.rate;
    let mut capture = Capture::open(capture_cfg)?;
    let mut playback = Playback::open(PlaybackConfig {
        sink: Some(sink.to_string()),
        rate,
        ..PlaybackConfig::default()
    })?;
    println!(
        "loopback {} -> {} @ {rate} Hz for {seconds} s",
        capture.source(),
        playback.sink()
    );
    let started = Instant::now();
    let mut latencies: Vec<Duration> = Vec::new();
    let mut frames = 0u64;
    let mut skipped = 0u64;
    let frame_len =
        Duration::from_secs_f64((playback.frame_samples() - 1) as f64 / f64::from(rate));
    let mut second = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        let Some(frame) = capture.next_frame().await else {
            break;
        };
        frames += 1;
        // Both streams run off one clock, so a backlog from any hiccup (a resync of the
        // mic, a late wake) would stay forever: a loopback must police its queue. One
        // frame waiting is the bound; a frame beyond it is dropped and counted.
        if playback.queued() >= playback.frame_samples() {
            skipped += 1;
            continue;
        }
        playback
            .write_stamped(&frame.samples, frame.captured_at)
            .await?;
        if second.elapsed() >= Duration::from_secs(1) {
            let mut fresh = Vec::new();
            playback.take_latencies(&mut fresh);
            let (ci, co) = (capture.counters(), playback.counters());
            let engine = summarize(&fresh);
            println!(
                "{:>5.1}s frames {frames} skipped {skipped} | engine latency to the frame's first sample {engine} | in: delay {} smp overruns {} | out: delay {} smp underruns {} silent {} | PipeWire-reported in+out delay {:.1} ms",
                started.elapsed().as_secs_f32(),
                ci.delay_samples,
                ci.overruns,
                co.delay_samples,
                co.underruns,
                co.silent_cycles,
                (ci.delay_samples.max(0) + co.delay_samples.max(0)) as f64 * 1000.0
                    / f64::from(rate),
            );
            latencies.extend(fresh);
            second = Instant::now();
        }
    }
    let mut fresh = Vec::new();
    playback.take_latencies(&mut fresh);
    latencies.extend(fresh);
    let (ci, co) = (capture.counters(), playback.counters());
    let device_ms =
        (ci.delay_samples.max(0) + co.delay_samples.max(0)) as f64 * 1000.0 / f64::from(rate);
    println!(
        "done: {frames} frames, {skipped} skipped to hold the queue; engine latency to the frame's first sample over {} frames: {}; to its last sample add the frame itself, {:.1} ms; PipeWire-reported device delays in+out {device_ms:.1} ms; in overruns {} empty {}; out underruns {} empty {}",
        latencies.len(),
        summarize(&latencies),
        frame_len.as_secs_f64() * 1000.0,
        ci.overruns,
        ci.empty_callbacks,
        co.underruns,
        co.empty_callbacks,
    );
    playback.drain().await?;
    Ok(())
}

fn summarize(latencies: &[Duration]) -> String {
    if latencies.is_empty() {
        return "no samples yet".into();
    }
    let mut sorted: Vec<Duration> = latencies.to_vec();
    sorted.sort();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let mean = sorted.iter().sum::<Duration>() / sorted.len() as u32;
    format!(
        "mean {:.2} ms p50 {:.2} ms p95 {:.2} ms p99 {:.2} ms max {:.2} ms",
        mean.as_secs_f64() * 1000.0,
        at(0.5).as_secs_f64() * 1000.0,
        at(0.95).as_secs_f64() * 1000.0,
        at(0.99).as_secs_f64() * 1000.0,
        sorted[sorted.len() - 1].as_secs_f64() * 1000.0,
    )
}

fn state_file(args: &Args) -> PathBuf {
    args.get("state")
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file)
}

fn duck(args: &Args) -> anyhow::Result<()> {
    let settings = AudioSettings::from_env();
    let patterns: Vec<String> = match args.get("pattern") {
        Some(p) => p.split(',').map(str::to_string).collect(),
        None => settings.duck_apps.clone(),
    };
    let level: f64 = args
        .get("level")
        .and_then(|l| l.parse().ok())
        .unwrap_or(settings.duck_level);
    let state = state_file(args);
    let seconds = args.seconds();
    let ducker = Ducker::new(&patterns, level, state.clone());
    println!(
        "ducking {patterns:?} to {level} for {seconds} s; state in {}",
        state.display()
    );
    let before = volumes_of(&patterns)?;
    let ducked = ducker.duck()?;
    if ducked.is_empty() {
        println!("nothing matched (an uncorked Stream/Output/Audio whose name contains a pattern)");
        return Ok(());
    }
    for d in &ducked {
        println!(
            "  #{} {:?}: {:?} -> {:?}",
            d.serial,
            d.name,
            pct(&d.before),
            pct(&d.after)
        );
    }
    let during = volumes_of(&patterns)?;
    std::thread::sleep(Duration::from_secs(seconds));
    let restored = ducker.restore()?;
    let after = volumes_of(&patterns)?;
    println!(
        "restored {} stream(s); state file gone: {}",
        restored.len(),
        !state.exists()
    );
    let mut exact = true;
    for (serial, b) in &before {
        let d = during.get(serial);
        let a = after.get(serial);
        let same = a == Some(b);
        exact &= same;
        println!(
            "  #{serial} linear channelVolumes before {:?} during {:?} after {:?} -> {}",
            b,
            d,
            a,
            if same { "exact" } else { "DIFFERENT" }
        );
    }
    if !exact {
        std::process::exit(3);
    }
    Ok(())
}

fn restore(args: &Args) -> anyhow::Result<()> {
    let state = state_file(args);
    let restored = restore_from_state(&state)?;
    println!(
        "restored {} stream(s) from {}",
        restored.len(),
        state.display()
    );
    Ok(())
}

/// The linear channel volumes of the matching output streams, by serial, read back from
/// PipeWire: what "exact" is judged on.
fn volumes_of(patterns: &[String]) -> anyhow::Result<std::collections::BTreeMap<u64, Vec<f32>>> {
    let ducker = Ducker::new(patterns, 1.0, PathBuf::from("/nonexistent"));
    let session = Session::connect("audio-probe")?;
    let nodes = session.nodes()?;
    let wanted: Vec<_> = nodes
        .iter()
        .filter(|n| n.media_class() == "Stream/Output/Audio" && ducker.matches(&n.props))
        .collect();
    let bound = session.bind(wanted)?;
    Ok(bound
        .iter()
        .filter_map(|b| Some((b.serial_or_id(), b.facts().channel_volumes?)))
        .collect())
}

fn pct(fractions: &[f64]) -> Vec<String> {
    fractions
        .iter()
        .map(|f| format!("{:.1}%", f * 100.0))
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn play_chime(args: &Args) -> anyhow::Result<()> {
    let Some(sink) = args.get("sink") else {
        anyhow::bail!("chime needs --sink NAME (a null sink, never the speakers)");
    };
    let settings = AudioSettings::from_env();
    let thinking_secs: f64 = args
        .get("thinking")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let sound = if thinking_secs > 0.0 {
        Some(ghost_audio::sounds::thinking_sound(
            &settings.cache_dir,
            ghost_audio::PLAYBACK_RATE,
        )?)
    } else {
        None
    };
    let mut playback = Playback::open(PlaybackConfig {
        sink: Some(sink.to_string()),
        sound,
        sound_volume: settings.thinking_volume,
        ..PlaybackConfig::default()
    })?;
    let pcm = chime(!args.has("down"));
    let started = Instant::now();
    playback.write(&pcm).await?;
    playback.drain().await?;
    println!(
        "chime: {} samples written and drained in {:?}; counters {:?}",
        pcm.len(),
        started.elapsed(),
        playback.counters()
    );
    if thinking_secs > 0.0 {
        let mixer = playback.mixer();
        mixer.set_enabled(true);
        tokio::time::sleep(Duration::from_secs_f64(thinking_secs)).await;
        mixer.set_enabled(false);
        println!(
            "thinking sound for {thinking_secs} s; counters {:?}",
            playback.counters()
        );
    }
    Ok(())
}
