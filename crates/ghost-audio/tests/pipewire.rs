//! Against the live PipeWire graph: ignored by default, because CI has no PipeWire and the
//! workstation's microphone is the operator's. Run them by hand, with `ghost-echo-cancel`
//! up (the `ghost.mic` node) and nothing on the speakers: every playback here goes to a
//! null sink the test loads and unloads itself.
//!
//! ```bash
//! cargo nextest run -p ghost-audio --run-ignored ignored-only
//! # or one at a time
//! cargo test -p ghost-audio --test pipewire -- --ignored capture_from_ghost_mic_yields_frames_without_overruns
//! ```
//!
//! Nothing from the microphone is stored or printed by these tests; they count frames.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ghost_audio::{
    CAPTURE_RATE, Capture, CaptureConfig, Ducker, PLAYBACK_RATE, Playback, PlaybackConfig, Session,
    chime, duck, frame_samples,
};

/// A null sink for the test's lifetime.
struct NullSink {
    name: String,
    module: String,
}

impl NullSink {
    fn load(name: &str) -> NullSink {
        let out = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
            ])
            .output()
            .expect("pactl is installed");
        assert!(
            out.status.success(),
            "pactl load-module: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let module = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Give WirePlumber a moment to announce the node.
        std::thread::sleep(Duration::from_millis(300));
        NullSink {
            name: name.to_string(),
            module,
        }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.module])
            .status();
    }
}

/// `pw-play` of a quiet tone into a sink, under a name the ducker can match.
struct TestTone {
    child: Child,
    _dir: tempfile::TempDir,
}

impl TestTone {
    fn start(sink: &str, app_name: &str) -> TestTone {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..24_000 * 60 {
            let v =
                (300.0 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 24_000.0).sin()) as i16;
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let child = Command::new("pw-play")
            .args([
                "--target",
                sink,
                "-P",
                &format!("{{ application.name = \"{app_name}\" media.name = \"test tone\" }}"),
            ])
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pw-play is installed");
        std::thread::sleep(Duration::from_millis(500));
        TestTone { child, _dir: dir }
    }
}

impl Drop for TestTone {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
#[ignore = "needs PipeWire and ghost.mic"]
fn capture_from_ghost_mic_yields_frames_without_overruns() {
    runtime().block_on(async {
        let mut capture = Capture::open(CaptureConfig::default()).expect("ghost.mic is there");
        assert_eq!(capture.source(), "ghost.mic");
        assert_eq!(capture.frame_samples(), frame_samples(CAPTURE_RATE));
        let started = Instant::now();
        let mut frames = 0;
        while started.elapsed() < Duration::from_secs(2) {
            let frame = capture.next_frame().await.expect("the stream runs");
            assert_eq!(frame.samples.len(), 320);
            assert_eq!(frame.rate, 16_000);
            assert!(frame.captured_at.elapsed() < Duration::from_millis(100));
            frames += 1;
        }
        let c = capture.counters();
        assert!(
            (90..=110).contains(&frames),
            "50 frames a second, got {frames} in 2 s"
        );
        assert_eq!(c.overruns, 0);
        assert_eq!(c.empty_callbacks, 0);
        assert!(c.samples >= 28_800, "{} samples in 2 s", c.samples);
        assert_eq!(capture.error(), None);
    });
}

#[test]
#[ignore = "needs PipeWire"]
fn a_missing_capture_node_is_named_and_the_fallback_is_explicit() {
    let err = Capture::open(CaptureConfig {
        node: Some("ghost.no-such-node".into()),
        ..CaptureConfig::default()
    })
    .err()
    .expect("no such node");
    assert_eq!(
        err.to_string(),
        "audio in: no PipeWire node named \"ghost.no-such-node\" (is ghost-echo-cancel running?)"
    );
    let capture = Capture::open(CaptureConfig {
        node: Some("ghost.no-such-node".into()),
        fallback_to_default: true,
        ..CaptureConfig::default()
    })
    .expect("falls back to the default source");
    assert_eq!(capture.source(), "default source");
}

#[test]
#[ignore = "needs PipeWire"]
fn playback_writes_clears_and_drains_into_a_null_sink() {
    let sink = NullSink::load("ghost-test-playback");
    runtime().block_on(async {
        let mut playback = Playback::open(PlaybackConfig {
            sink: Some(sink.name.clone()),
            sound: Some(std::sync::Arc::new(ghost_audio::thinking_loop(
                PLAYBACK_RATE,
                1.6,
            ))),
            ..PlaybackConfig::default()
        })
        .expect("the null sink is there");
        assert_eq!(playback.sink(), sink.name);
        assert_eq!(playback.frame_samples(), 480);

        // The chime: 180 ms of audio drains in about that long.
        let pcm = chime(true);
        let started = Instant::now();
        playback.write(&pcm).await.unwrap();
        playback.drain().await.unwrap();
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(170),
            "drained too soon: {took:?}"
        );
        assert!(
            took < Duration::from_millis(600),
            "drained too late: {took:?}"
        );
        let c = playback.counters();
        assert_eq!(c.played, pcm.len() as u64);
        assert_eq!(c.underruns, 0);

        // Barge-in: a second of audio queued, cleared within one quantum.
        let second = vec![1000i16; PLAYBACK_RATE as usize / 2]; // the ring holds half a second
        playback.write(&second).await.unwrap();
        assert!(playback.queued() > 10_000);
        let started = Instant::now();
        playback.clear().await;
        assert!(
            started.elapsed() < Duration::from_millis(60),
            "clear took {:?}",
            started.elapsed()
        );
        assert_eq!(playback.queued(), 0);

        // The mixer plays on its own when nothing is queued, and is a switch.
        let mixer = playback.mixer();
        assert!(!mixer.enabled());
        assert_eq!(mixer.volume(), 0.12);
        mixer.set_enabled(true);
        mixer.set_volume(0.2);
        tokio::time::sleep(Duration::from_millis(200)).await;
        mixer.set_enabled(false);
        assert_eq!(playback.mixer().volume(), 0.2);
        assert_eq!(playback.error(), None);
    });
    let missing = Playback::open(PlaybackConfig {
        sink: Some("ghost-no-such-sink".into()),
        ..PlaybackConfig::default()
    })
    .err()
    .expect("no such sink");
    assert_eq!(
        missing.to_string(),
        "audio out: no PipeWire sink named \"ghost-no-such-sink\""
    );
}

#[test]
#[ignore = "needs PipeWire and ghost.mic"]
fn loopback_into_a_null_sink_is_under_40_ms() {
    let sink = NullSink::load("ghost-test-loopback");
    runtime().block_on(async {
        let mut capture = Capture::open(CaptureConfig::default()).expect("ghost.mic is there");
        let mut playback = Playback::open(PlaybackConfig {
            sink: Some(sink.name.clone()),
            rate: CAPTURE_RATE,
            ..PlaybackConfig::default()
        })
        .unwrap();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            let frame = capture.next_frame().await.unwrap();
            if playback.queued() >= playback.frame_samples() {
                continue; // a loopback holds its queue to one frame (see audio-probe)
            }
            playback
                .write_stamped(&frame.samples, frame.captured_at)
                .await
                .unwrap();
        }
        let mut latencies = Vec::new();
        playback.take_latencies(&mut latencies);
        assert!(latencies.len() > 100, "{} latency samples", latencies.len());
        latencies.sort();
        let p99 = latencies[latencies.len() * 99 / 100];
        let max = *latencies.last().unwrap();
        eprintln!(
            "loopback engine latency to the first sample: p99 {p99:?} max {max:?} over {} frames",
            latencies.len()
        );
        assert!(max < Duration::from_millis(40), "max {max:?}");
        assert_eq!(capture.counters().overruns, 0);
    });
}

#[test]
#[ignore = "needs PipeWire and pw-play"]
fn ducking_restores_the_exact_previous_volume_and_forgets_the_file() {
    let sink = NullSink::load("ghost-test-duck");
    let _tone = TestTone::start(&sink.name, "ghost-duck-test");
    let dir = tempfile::tempdir().unwrap();
    let state: PathBuf = dir.path().join(duck::DUCK_STATE);

    // Set the stream to 34 % the way the operator would, through pulse.
    let serial = {
        let session = Session::connect("ghost-audio-test").unwrap();
        let nodes = session.nodes().unwrap();
        let node = nodes
            .iter()
            .find(|n| {
                n.props.get("application.name").map(String::as_str) == Some("ghost-duck-test")
            })
            .expect("the test tone is a stream");
        node.serial_or_id()
    };
    let status = Command::new("pactl")
        .args(["set-sink-input-volume", &serial.to_string(), "34%"])
        .status()
        .unwrap();
    assert!(status.success());
    std::thread::sleep(Duration::from_millis(200));

    let volumes = || -> Vec<f32> {
        let session = Session::connect("ghost-audio-test").unwrap();
        let nodes = session.nodes().unwrap();
        let node = nodes.iter().find(|n| n.serial_or_id() == serial).unwrap();
        let bound = session.bind([node]).unwrap();
        bound[0].facts().channel_volumes.unwrap()
    };
    let before = volumes();
    assert!(
        (duck::to_pulse(before[0]) - 0.34).abs() < 0.002,
        "{before:?}"
    );

    let ducker = Ducker::new(["GHOST-DUCK"], 0.25, state.clone());
    let ducked = ducker.duck().unwrap();
    assert_eq!(ducked.len(), 1);
    assert_eq!(ducked[0].serial, serial);
    assert_eq!(ducked[0].name, "test tone");
    assert_eq!(ducked[0].after, vec![0.25]);
    assert!(
        state.exists(),
        "the state file is written before the volume changes"
    );
    let saved = duck::read_state(&state).unwrap().unwrap();
    assert_eq!(saved[&serial.to_string()], ducked[0].before);
    assert_eq!(volumes(), vec![duck::to_linear(0.25)]);

    let restored = ducker.restore().unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        volumes().iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        before.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "bit-exact restore"
    );
    assert!(!state.exists());

    // Nothing to restore is not an error, and a pattern that matches nothing ducks nothing.
    assert!(ducker.restore().unwrap().is_empty());
    let none = Ducker::new(["ghost-no-such-app"], 0.25, state.clone());
    assert!(none.duck().unwrap().is_empty());
    assert!(!state.exists());
}
