//! ADR 0077 end to end: `ghostd run` over a temp folder, dictations through the control
//! socket with a fake microphone that plays a marked pattern and a recognizer that reads a
//! marked sentence, then a search for both everywhere Ghost could have put them: every file
//! under the temp folder (the store, the audit, the latency log, debug mode's file, the wake
//! takes, a diagnostic report saved with every box ticked), searched for the words and for
//! the audio's bytes; every line logged at any level with debug mode on; every event on the
//! bus (which the phone's stream and the desktop read); every answer the control API gave;
//! every notification. The only place the words may be is the clipboard.
//!
//! Where Xvfb is installed the clipboard is the real one, on a display of the test's own,
//! read back as another program pasting would; elsewhere a fake that keeps the same limit.
//! Parakeet is a fake here (no model in CI); the live trial reads with the real one.
//!
//! Before the dictation that lands, the paths that must keep nothing: a read cancelled while
//! it runs, listening turned off while recording, and a text too long for the clipboard.
//!
//! Its own test binary: the log capture is the process's global subscriber.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_api::GhostClient;
use ghost_api::dictation::DictationReason;
use ghost_core::MemoryKeyring;
use ghost_infer::TurnVerdict;
use ghost_voice::VoiceSettings;
use ghost_voice::frames::Audio;
use ghost_voice::llm::{ModelPort, ScriptedModel};
use ghost_voice::pipeline::Models;
use ghost_voice::stt::{Recognizer, SharedRecognizer};
use ghost_voice::transport::{MediaIn, Silent, scripted_media};
use ghost_voice::tts::{NoSynth, SharedSynth};
use ghost_voice::turns::TurnModel;
use ghostd::config::GhostdSettings;
use ghostd::daemon::{Daemon, LogMirror, Ports};
use ghostd::dictation::clipboard::{Auth, MAX_BYTES, XClipboard};
use ghostd::dictation::{Clipboard, Note, Notifier};
use serde_json::{Value, json};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use zeroize::Zeroizing;

/// The words Parakeet "reads", and the forms they could take on the way.
const SAID: &str = "Um, the quokka marigold password is seventeen.";
const CLEANED: &str = "The quokka marigold password is seventeen.";
const MARKS: [&str; 2] = ["quokka", "marigold"];
/// The audio: a pattern no room makes, cycled.
const PCM: [i16; 6] = [12_345, -23_456, 3_456, -4_567, 15_678, -26_789];

/// 0: reads the sentence. 1: waits for `gate`, then reads it. 2: reads it over and over,
/// longer than the clipboard takes.
struct ReadsTheMark {
    mode: Arc<AtomicU8>,
    gate: Arc<AtomicBool>,
}

impl Recognizer for ReadsTheMark {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        // Dictation's audio is loud; anything else Ghost reads here is silence.
        if !audio.iter().any(|x| x.abs() > 0.3) {
            return Ok(String::new());
        }
        match self.mode.load(Ordering::SeqCst) {
            1 => {
                while !self.gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(SAID.into())
            }
            2 => Ok(SAID.repeat(MAX_BYTES / SAID.len() + 2)),
            _ => Ok(SAID.into()),
        }
    }
}

struct Complete;

impl TurnModel for Complete {
    fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<TurnVerdict> {
        Ok(TurnVerdict {
            probability: 0.99,
            complete: true,
        })
    }
}

struct Marked;

#[async_trait::async_trait]
impl MediaIn for Marked {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some(Audio::new(
            PCM.iter().copied().cycle().take(320).collect(),
            16_000,
        ))
    }
}

/// Where no display can be had: a clipboard that keeps the real one's limit.
#[derive(Default)]
struct Board(Mutex<Vec<String>>);

impl Clipboard for Board {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        if text.len() > MAX_BYTES {
            return Err(DictationReason::TooLong);
        }
        self.0.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

#[derive(Default)]
struct Shown(Mutex<Vec<String>>);

impl Notifier for Shown {
    fn show(&self, note: Note) {
        let line = match note {
            Note::Recording => "Dictating".to_string(),
            Note::Ended(r) => r.sentence(),
        };
        self.0.lock().unwrap().push(line);
    }
}

/// Every log line, at every level.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The audio as Ghost holds it: 16-bit and float samples, little-endian.
fn pcm_marks() -> Vec<Vec<u8>> {
    let i16s: Vec<u8> = PCM.iter().flat_map(|s| s.to_le_bytes()).collect();
    let f32s: Vec<u8> = PCM[..3]
        .iter()
        .flat_map(|&s| (f32::from(s) / 32768.0).to_le_bytes())
        .collect();
    vec![i16s, f32s]
}

/// The sound as Parakeet is given it: float, with the −72 dBFS dither added, so not to the
/// bit. Three samples of the pattern in a row, each within the dither's reach, at any byte
/// offset, little-endian.
fn has_dithered_floats(bytes: &[u8]) -> bool {
    let want: Vec<f32> = PCM.iter().map(|&s| f32::from(s) / 32768.0).collect();
    let reach = 10f32.powf(ghost_voice::stt::DITHER_DBFS / 20.0) * 2.0;
    for offset in 0..4.min(bytes.len()) {
        let floats: Vec<f32> = bytes[offset..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        for w in floats.windows(3) {
            for start in 0..PCM.len() {
                if (0..3).all(|k| (w[k] - want[(start + k) % PCM.len()]).abs() <= reach) {
                    return true;
                }
            }
        }
    }
    false
}

fn has_mark(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).to_lowercase();
    if let Some(m) = MARKS.iter().find(|m| text.contains(*m)) {
        return Some((*m).to_string());
    }
    if let Some(p) = pcm_marks()
        .iter()
        .find(|p| bytes.windows(p.len()).any(|w| w == p.as_slice()))
    {
        return Some(format!("the audio ({} bytes)", p.len()));
    }
    has_dithered_floats(bytes).then(|| "the audio, as dithered floats".to_string())
}

/// Every file under `dir`, with the mark if it holds it.
fn files_with_mark(dir: &Path, found: &mut Vec<String>, seen: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_with_mark(&p, found, seen);
        } else if let Ok(bytes) = std::fs::read(&p) {
            *seen += 1;
            if let Some(m) = has_mark(&bytes) {
                found.push(format!("{} holds {m}", p.display()));
            }
        }
    }
}

// --- a display of the test's own ---------------------------------------------------------------

struct Xvfb {
    child: Child,
    number: u16,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill(); // this test's own Xvfb only
        let _ = self.child.wait();
    }
}

fn xvfb() -> Option<Xvfb> {
    if Command::new("Xvfb")
        .arg("-help")
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("no Xvfb: the clipboard is a fake in this run");
        return None;
    }
    for number in (110u16..150).map(|n| n + (std::process::id() % 5) as u16) {
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
        if socket.exists() || Path::new(&format!("/tmp/.X{number}-lock")).exists() {
            continue;
        }
        let child = Command::new("Xvfb")
            .args([
                format!(":{number}").as_str(),
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let x = Xvfb { child, number };
        for _ in 0..100 {
            if socket.exists() {
                return Some(x);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    None
}

/// What another program pasting would get from that display: CLIPBOARD as UTF8_STRING.
fn paste(number: u16) -> Option<String> {
    let s = UnixStream::connect(format!("/tmp/.X11-unix/X{number}")).ok()?;
    let (stream, _) = DefaultStream::from_unix_stream(s).ok()?;
    let conn = RustConnection::connect_to_stream(stream, 0).ok()?;
    let root = conn.setup().roots[0].root;
    let win = conn.generate_id().ok()?;
    conn.create_window(
        0,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .ok()?;
    let atom = |n: &[u8]| {
        conn.intern_atom(false, n)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    let (clipboard, utf8, prop) = (
        atom(b"CLIPBOARD")?,
        atom(b"UTF8_STRING")?,
        atom(b"GHOST_TEST")?,
    );
    conn.convert_selection(win, clipboard, utf8, prop, x11rb::CURRENT_TIME)
        .ok()?;
    conn.flush().ok()?;
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        match conn.poll_for_event().ok()? {
            Some(Event::SelectionNotify(e)) => {
                if e.property == x11rb::NONE {
                    return None;
                }
                let v = conn
                    .get_property(false, win, prop, AtomEnum::ANY, 0, 1 << 20)
                    .ok()?
                    .reply()
                    .ok()?;
                return String::from_utf8(v.value).ok();
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    None
}

/// A dictation start, once the passive loop waits for the microphone again.
async fn start(client: &GhostClient, answers: &mut Vec<Value>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let v = client.dictation(Some("start")).await.unwrap();
        answers.push(v.clone());
        if v["state"] == "recording" {
            return;
        }
        assert!(Instant::now() < deadline, "dictation never started: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictated_words_reach_only_the_clipboard() {
    // The search finds the audio as a frame of it lies in memory or a file.
    let frame: Vec<u8> = PCM
        .iter()
        .copied()
        .cycle()
        .take(320)
        .flat_map(|s| s.to_le_bytes())
        .collect();
    assert!(has_mark(&frame).is_some(), "the audio search is blind");
    assert!(has_mark(b"nothing here").is_none());
    // and it finds the floats Parakeet is given, dither and all, at any byte offset
    let floats: Vec<f32> = PCM
        .iter()
        .copied()
        .cycle()
        .take(64)
        .map(|s| f32::from(s) / 32768.0)
        .collect();
    let mut shifted = vec![7u8];
    shifted.extend(
        ghost_voice::stt::dithered(&floats)
            .iter()
            .flat_map(|f| f.to_le_bytes()),
    );
    assert!(has_mark(&shifted).is_some(), "the dithered search is blind");
    let lines = Lines::default();
    let mirror = LogMirror::new();
    let capture = ghostd::debug_audio::Capture::new();
    {
        let lines = lines.clone();
        tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::TRACE)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || lines.clone()),
            )
            .with(mirror.clone())
            .with(capture.clone())
            .init();
    }
    let tmp = tempfile::tempdir().unwrap();
    let t = |p: &str| tmp.path().join(p);
    let cfg = GhostdSettings {
        data_dir: t("data"),
        db_path: t("data/ghost.db"),
        policy_path: t("config/policy.toml"),
        config_dir: t("config"),
        catalog_path: t("cache/catalog.json"),
        skills_dir: t("config/skills"),
        home_skills_dir: t("home-skills"),
        api_socket: Some(t("dictation.sock")),
        settings_path: t("config/settings.env"),
        env_path: t("config/env"),
        vault_env_path: t("no-heka"),
        infra_dir: t("infra"),
        wake_mode: "file".into(),
        chime: false,
        mcp_listen: String::new(),
        nerd_dictation_flag: t("rt/nerd-dictation"),
        ..GhostdSettings::default()
    };
    let cache = t("cache").to_string_lossy().into_owned();
    let voice = VoiceSettings::from_vars(|name| match name {
        "GHOST_CACHE_DIR" => Some(cache.clone()),
        "GHOST_THINKING_SOUND" => Some("false".into()),
        "GHOST_LATENCY" => Some("1".into()),
        "GHOST_SILERO_MODEL" => Some(t("nonexistent/silero.onnx").to_string_lossy().into_owned()),
        "GHOST_SMART_TURN_MODEL" => Some(t("nonexistent/turn.onnx").to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    std::fs::create_dir_all(t("rt")).unwrap();
    let display = xvfb();
    let board = Arc::new(Board::default());
    let clipboard: Arc<dyn Clipboard> = match &display {
        Some(x) => Arc::new(XClipboard::at(
            format!("/tmp/.X11-unix/X{}", x.number),
            x.number,
            Auth::None,
        )),
        None => board.clone(),
    };
    let on_clipboard = |display: &Option<Xvfb>| -> Option<String> {
        match display {
            Some(x) => paste(x.number),
            None => board.0.lock().unwrap().last().cloned(),
        }
    };
    let shown = Arc::new(Shown::default());
    let mode = Arc::new(AtomicU8::new(0));
    let gate = Arc::new(AtomicBool::new(false));
    let mut ports = Ports::headless();
    ports.keyring = Arc::new(MemoryKeyring::new());
    ports.load_models = {
        let (mode, gate) = (mode.clone(), gate.clone());
        Arc::new(move |s: &VoiceSettings| {
            let stt: SharedRecognizer = Arc::new(Mutex::new(ReadsTheMark {
                mode: mode.clone(),
                gate: gate.clone(),
            }));
            let tts: SharedSynth = Arc::new(Mutex::new(NoSynth));
            Ok(Models {
                stt,
                tts,
                turn: Arc::new(Mutex::new(Complete)),
                silero: s.silero_model.clone(),
                voices: vec!["af_heart".into()],
                acks: ghost_voice::acks::Acks::new(None),
                problem: None,
            })
        })
    };
    ports.model = Arc::new(|_: &VoiceSettings, _: &HashMap<String, String>| {
        Ok(ScriptedModel::new(vec![], Duration::from_millis(5)) as Arc<dyn ModelPort>)
    });
    ports.desk_media = Arc::new(|_req| {
        Box::pin(async move {
            let (media, _mouth, _ear) = scripted_media();
            Ok(media)
        })
    });
    ports.passive_media = Some(Arc::new(|| Ok(Box::new(Silent) as Box<dyn MediaIn>)));
    ports.dictation_media = Some(Arc::new(|| Ok(Box::new(Marked) as Box<dyn MediaIn>)));
    ports.clipboard = clipboard;
    ports.dictation_notes = shown.clone();
    let daemon = Arc::new(
        Daemon::new(cfg.clone(), voice, ports)
            .with_runtime_dir(t("rt"))
            .with_models_retry(Duration::from_millis(200))
            .with_log_mirror(mirror.clone())
            .with_debug_capture(capture),
    );
    let client =
        GhostClient::with_timeout(cfg.api_socket.clone().unwrap(), Duration::from_secs(10));
    let run = {
        let d = daemon.clone();
        tokio::spawn(async move { d.run().await })
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(v) = client.status().await
            && v["phase"] == "passive"
            && v["models_ready"] == true
        {
            break;
        }
        assert!(Instant::now() < deadline, "the daemon never got ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Every event from here on, as the desktop and the phone's stream read the bus.
    let mut bus = daemon.state.bus.tap();
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let collecting = {
        let events = events.clone();
        tokio::spawn(async move {
            while let Ok(e) = bus.recv().await {
                events.lock().unwrap().push(e);
            }
        })
    };
    // Debug mode on: what Ghost hears is logged now, and kept in its file.
    let on = client.debug_audio(Some(true)).await.unwrap();
    assert_eq!(on["on"], true);
    let mut answers = Vec::new();

    // 1. A read cancelled while it runs: nothing reaches the clipboard.
    mode.store(1, Ordering::SeqCst);
    start(&client, &mut answers).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stopping = {
        let socket = cfg.api_socket.clone().unwrap();
        tokio::spawn(async move {
            GhostClient::with_timeout(socket, Duration::from_secs(10))
                .dictation(Some("stop"))
                .await
                .unwrap()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.dictation(None).await.unwrap()["state"] != "reading" {
        assert!(Instant::now() < deadline, "never reading");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cancelled = client.dictation(Some("cancel")).await.unwrap();
    assert_eq!(cancelled["result"]["outcome"], "cancelled", "{cancelled}");
    gate.store(true, Ordering::SeqCst); // the read finishes late
    let late = stopping.await.unwrap();
    assert_eq!(late["result"]["outcome"], "cancelled", "{late}");
    answers.extend([cancelled, late]);
    assert_eq!(
        on_clipboard(&display),
        None,
        "a cancelled read reached the clipboard"
    );

    // 2. Listening turned off while recording: dropped, nothing read.
    mode.store(0, Ordering::SeqCst);
    start(&client, &mut answers).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.set_listening(false, None).await.unwrap();
    let v = client.dictation(None).await.unwrap();
    assert_eq!(v["result"]["reason"], "listening_turned_off", "{v}");
    answers.push(v);
    client.set_listening(true, None).await.unwrap();
    assert_eq!(on_clipboard(&display), None);

    // 3. Too long for the clipboard: said so, nothing kept.
    mode.store(2, Ordering::SeqCst);
    start(&client, &mut answers).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let v = client.dictation(Some("stop")).await.unwrap();
    assert_eq!(v["result"]["outcome"], "not_on_clipboard", "{v}");
    assert_eq!(v["result"]["reason"], "too_long", "{v}");
    answers.push(v);
    assert_eq!(on_clipboard(&display), None);

    // 4. The one that lands.
    mode.store(0, Ordering::SeqCst);
    start(&client, &mut answers).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    answers.push(client.dictation(None).await.unwrap());
    let stopped = client.dictation(Some("stop")).await.unwrap();
    assert_eq!(stopped["result"]["outcome"], "on_clipboard", "{stopped}");
    assert_eq!(stopped["result"]["chars"], CLEANED.len());
    answers.push(stopped);
    answers.push(client.dictation(None).await.unwrap());
    answers.push(client.status().await.unwrap());
    assert_eq!(on_clipboard(&display).as_deref(), Some(CLEANED));

    // A diagnostic report with every box ticked, saved into the temp folder.
    std::fs::create_dir_all(t("reports")).unwrap();
    let report = client
        .diagnose(&json!({
            "transcripts": true,
            "debug_log": true,
            "wake_recordings": true,
            "folder": t("reports").to_string_lossy(),
        }))
        .await
        .unwrap();
    answers.push(report);
    // Debug mode's file is still there while it is on: searched now, then turned off.
    let mut found = Vec::new();
    let mut seen = 0;
    files_with_mark(tmp.path(), &mut found, &mut seen);
    assert!(seen > 3, "the walk read the daemon's files ({seen})");
    assert!(
        t("reports").read_dir().unwrap().count() >= 1,
        "a report was saved"
    );
    client.debug_audio(Some(false)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    daemon.stop.cancel();
    let ran = tokio::time::timeout(Duration::from_secs(20), run).await;
    assert!(
        matches!(ran, Ok(Ok(Ok(())))),
        "the daemon did not wind down cleanly in time: {ran:?}"
    );
    collecting.abort();
    files_with_mark(tmp.path(), &mut found, &mut seen);
    if display.is_some() {
        // Ghost stopped: its owner gave the clipboard up and was joined.
        assert_eq!(on_clipboard(&display), None, "the owner outlived Ghost");
    }

    for a in &answers {
        if let Some(m) = has_mark(a.to_string().as_bytes()) {
            found.push(format!("an API answer holds {m}: {a}"));
        }
    }
    for e in events.lock().unwrap().iter() {
        if let Some(m) = has_mark(e.as_bytes()) {
            found.push(format!("an event holds {m}: {e}"));
        }
    }
    let logged = lines.0.lock().unwrap().clone();
    let logged_text = String::from_utf8_lossy(&logged);
    assert!(
        logged_text.contains("dictation: OnClipboard")
            && logged_text.contains("dictation: cancelled"),
        "the log capture saw the dictations' numbers"
    );
    if let Some(m) = has_mark(&logged) {
        found.push(format!("a log line holds {m}"));
    }
    for line in mirror.tail() {
        if let Some(m) = has_mark(line.as_bytes()) {
            found.push(format!("the log mirror holds {m}"));
        }
    }
    for s in shown.0.lock().unwrap().iter() {
        if let Some(m) = has_mark(s.as_bytes()) {
            found.push(format!("a notification holds {m}"));
        }
    }
    assert!(
        found.is_empty(),
        "dictated words or audio escaped: {found:#?}"
    );
}
