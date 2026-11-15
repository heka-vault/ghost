//! The speech models the bundle does not carry, downloaded by Ghost itself (ADR 0066).
//!
//! Parakeet (hearing, 2.5 GB) and Kokoro (the voice, 340 MB) are too large for the .deb
//! and the AppImage. A person who installed a release gets them from the app's first run
//! or Settings (`GET /speech-models`, `POST /speech-models/fetch`), or from `ghostd models
//! fetch` on a desk with no display; nobody needs `scripts/models.sh`, which stays the
//! checkout's way and carries the same pins (`pins_match_the_script` holds them together).
//!
//! Each file is fetched from its upstream release at a pinned revision into a `.part`
//! beside its place, hashed as it arrives, refused when its size or SHA-256 is not the pin
//! (nothing of it is kept), and moved into place whole by one rename, so nothing half
//! written is ever read as a model. A file cut off part way resumes where it stopped. The
//! daemon's model load, which waits for missing files (ADR 0038), is woken when the
//! download ends. Nothing here is sent anywhere but the GETs, which carry no identifier.
//!
//! One download per file, whoever starts it (#135): the terminal's `ghostd models fetch`,
//! the app's first run, a second `fetch` and a restarted daemon all share the folder, not a
//! process. Before a byte is asked for, the fetcher takes an advisory lock (`flock`) on
//! `<file>.lock` beside the `.part`. The kernel lets it go when the holder closes it or
//! dies, so a crashed download never blocks the next. A fetcher that cannot take it asks
//! for nothing: it follows, reading the holder's progress from the size of the `.part`
//! four times a second and trying the lock each time; when the file is in place it checks
//! it against the pin, and when the holder went away without it, it takes the lock and
//! carries on from the `.part`. Where the lock cannot be taken at all (a file system with
//! no `flock`), each fetcher downloads as before, said at debug level.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::StreamExt;
use ghost_api::speech_models::{
    SpeechModelFile, SpeechModelsFetch, SpeechModelsFetchState, SpeechModelsPort, SpeechModelsView,
};
use ghost_voice::config::VoiceSettings;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// One file, as `scripts/models.sh` pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub model: &'static str,
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub size: u64,
}

const PARAKEET_URL: &str = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";
const KOKORO_URL: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";

/// Parakeet TDT 0.6B v3 at full precision (ADR 0043), CC-BY-4.0.
pub fn parakeet() -> Vec<Pin> {
    [
        (
            "encoder-model.onnx",
            "98a74b21b4cc0017c1e7030319a4a96f4a9506e50f0708f3a516d02a77c96bb1",
            41_770_866,
        ),
        (
            "encoder-model.onnx.data",
            "9a22d372c51455c34f13405da2520baefb7125bd16981397561423ed32d24f36",
            2_435_420_160,
        ),
        (
            "decoder_joint-model.onnx",
            "e978ddf6688527182c10fde2eb4b83068421648985ef23f7a86be732be8706c1",
            72_520_893,
        ),
        (
            "vocab.txt",
            "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            93_939,
        ),
        (
            "config.json",
            "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
            97,
        ),
    ]
    .into_iter()
    .map(|(name, sha256, size)| Pin {
        model: "Parakeet",
        name,
        url: format!("{PARAKEET_URL}/{name}"),
        sha256,
        size,
    })
    .collect()
}

/// Kokoro v1.0 (ADR 0004), Apache-2.0.
pub fn kokoro() -> Vec<Pin> {
    [
        (
            "kokoro-v1.0.onnx",
            "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
            325_532_387,
        ),
        (
            "voices-v1.0.bin",
            "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
            28_214_398,
        ),
    ]
    .into_iter()
    .map(|(name, sha256, size)| Pin {
        model: "Kokoro",
        name,
        url: format!("{KOKORO_URL}/{name}"),
        sha256,
        size,
    })
    .collect()
}

/// A file to fetch and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub pin: Pin,
    pub dest: PathBuf,
}

/// The files the daemon's settings name that are not there. Parakeet's int8 files are a
/// developer's choice (`GHOST_STT_QUANTIZATION=int8`) and are not fetched here.
pub fn missing(voice: &VoiceSettings) -> Vec<Wanted> {
    let mut out = Vec::new();
    if voice.stt_quantization.as_deref() != Some("int8") {
        let paths = voice.parakeet_paths();
        if let Some(dir) = paths.encoder.parent() {
            for pin in parakeet() {
                out.push(Wanted {
                    dest: dir.join(pin.name),
                    pin,
                });
            }
        }
    }
    let k = voice.kokoro_paths();
    for (pin, dest) in kokoro().into_iter().zip([k.model, k.voices]) {
        out.push(Wanted { pin, dest });
    }
    out.retain(|w| !w.dest.is_file());
    out
}

/// The waiting list's words while files are missing: what, how big, and where to get them.
pub fn missing_note(wanted: &[Wanted]) -> String {
    let ears = wanted.iter().any(|w| w.pin.model == "Parakeet");
    let voice = wanted.iter().any(|w| w.pin.model == "Kokoro");
    let what = match (ears, voice) {
        (true, true) => "Ghost's ears and voice are",
        (true, false) => "Ghost's ears are",
        _ => "Ghost's voice is",
    };
    format!(
        "{what} not downloaded yet ({}). Download them in the first run or Settings › Sound; after that speech never needs the internet.",
        gb(remaining(wanted))
    )
}

/// Where the files go: the folder of the first missing one, else Ghost's cache.
fn folder(voice: &VoiceSettings, wanted: &[Wanted]) -> PathBuf {
    wanted
        .first()
        .and_then(|w| w.dest.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| voice.cache_dir.clone())
}

/// Free bytes for an unprivileged writer at `path` or its nearest existing parent.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let mut p = Some(path);
    while let Some(here) = p {
        if here.exists() {
            let s = rustix::fs::statvfs(here).ok()?;
            return Some(s.f_bavail.saturating_mul(s.f_frsize));
        }
        p = here.parent();
    }
    None
}

/// Bytes still to fetch: what each `.part` already holds is not counted again.
fn remaining(wanted: &[Wanted]) -> u64 {
    wanted
        .iter()
        .map(|w| {
            let have = std::fs::metadata(part_path(&w.dest))
                .map(|m| m.len())
                .unwrap_or(0);
            w.pin.size.saturating_sub(have.min(w.pin.size))
        })
        .sum()
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// The advisory lock beside a file's `.part`; removed once the file is in place.
fn lock_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    dest.with_file_name(name)
}

/// How often a follower reads the holder's progress and tries the lock again: the app's
/// bar lags the download by this at most, and nothing spins.
const FOLLOW_TICK: Duration = Duration::from_millis(250);

/// Whose turn it is to download a file.
enum Turn {
    /// This fetcher holds the lock until the `File` is dropped (or the process dies).
    Mine(std::fs::File),
    /// Another fetcher, in this process or another, holds it.
    Busy,
    /// The folder takes no locks; download as before.
    NoLocks(String),
}

fn take_turn(dest: &Path) -> Turn {
    let path = lock_path(dest);
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Turn::NoLocks(format!("{}: {e}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => Turn::Mine(file),
        Err(std::fs::TryLockError::WouldBlock) => Turn::Busy,
        Err(std::fs::TryLockError::Error(e)) => Turn::NoLocks(format!("{}: {e}", path.display())),
    }
}

/// Whether another fetcher is downloading this file now. Makes no lock file: one that is
/// not there is held by nobody. Taken and let go at once when free.
pub fn held_elsewhere(dest: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path(dest))
    else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Fetch every file in `wanted`, in order. Err in plain words; what was fetched whole is
/// kept and a `.part` is resumed next time.
pub async fn fetch_all(
    client: &reqwest::Client,
    wanted: &[Wanted],
    progress: &(dyn Fn(&SpeechModelsFetch) + Send + Sync),
) -> Result<(), String> {
    let total: u64 = wanted.iter().map(|w| w.pin.size).sum();
    if let Some(dir) = wanted.first().and_then(|w| w.dest.parent()) {
        let need = remaining(wanted);
        if let Some(free) = free_bytes(dir)
            && free < need
        {
            return Err(format!(
                "Ghost needs {} free in {} for its speech models and there is {}. Free some \
                 space and try again.",
                gb(need),
                dir.display(),
                gb(free)
            ));
        }
    }
    let mut done = 0u64;
    for w in wanted {
        let before = done;
        let report = |n: u64, following: bool| {
            progress(&SpeechModelsFetch {
                state: SpeechModelsFetchState::Fetching,
                done_bytes: before + n,
                total_bytes: total,
                file: Some(w.pin.name.to_string()),
                error: None,
                following,
            })
        };
        fetch_one(client, w, &report).await?;
        done += w.pin.size;
    }
    Ok(())
}

/// One file into place, by this fetcher or by the one that holds its lock; Err in plain
/// words with nothing of a bad file kept. `report` gets the bytes there so far and
/// whether another fetcher is the one fetching them.
async fn fetch_one(
    client: &reqwest::Client,
    w: &Wanted,
    report: &(dyn Fn(u64, bool) + Send + Sync),
) -> Result<(), String> {
    let dir = w
        .dest
        .parent()
        .ok_or_else(|| format!("{} has no folder", w.dest.display()))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Ghost cannot make {}: {e}", dir.display()))?;
    let mine = |n: u64| report(n, false);
    let mut following = false;
    loop {
        if w.dest.is_file() {
            // Put in place by the fetcher this one followed (or before the list was made).
            report(w.pin.size, following);
            return check_placed(w).await;
        }
        match take_turn(&w.dest) {
            Turn::Mine(lock) => {
                if w.dest.is_file() {
                    continue;
                }
                if following {
                    info!(
                        "speech models: the other download of {} stopped; carrying on from it",
                        w.pin.name
                    );
                }
                let r = download(client, w, &mine).await;
                if r.is_ok() {
                    // In place first, so whoever opened the old lock finds the file.
                    let _ = std::fs::remove_file(lock_path(&w.dest));
                }
                drop(lock);
                return r;
            }
            Turn::Busy => {
                if !following {
                    info!(
                        "speech models: {} is already downloading; following it",
                        w.pin.name
                    );
                    following = true;
                }
                // The holder's `.part`, which only grows by what it wrote. Gone between
                // the holder's rename and the next look: nothing new to say.
                if let Ok(m) = tokio::fs::metadata(part_path(&w.dest)).await {
                    report(m.len().min(w.pin.size), true);
                }
                tokio::time::sleep(FOLLOW_TICK).await;
            }
            Turn::NoLocks(e) => {
                debug!("speech models: no lock here ({e}); downloading without one");
                return download(client, w, &mine).await;
            }
        }
    }
}

/// A file another fetcher put in place, checked against its pin like one fetched here.
/// One that is not the pin is removed, so the next fetch gets it again.
async fn check_placed(w: &Wanted) -> Result<(), String> {
    let p = w.dest.clone();
    let (size, hash) = tokio::task::spawn_blocking(move || -> std::io::Result<(u64, String)> {
        let mut h = Sha256::new();
        let mut f = std::fs::File::open(&p)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut n_all = 0u64;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                return Ok((n_all, hex::encode(h.finalize())));
            }
            n_all += n as u64;
            h.update(&buf[..n]);
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Ghost cannot read {}: {e}", w.dest.display()))?;
    if size == w.pin.size && hash == w.pin.sha256 {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(&w.dest).await;
    Err(format!(
        "{} on this computer is not the file Ghost expects (its SHA-256 does not match the \
         pin), so it was removed. Try again",
        w.pin.name
    ))
}

/// The download itself, by the fetcher whose turn it is.
async fn download(
    client: &reqwest::Client,
    w: &Wanted,
    report: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), String> {
    let part = part_path(&w.dest);
    // What a stopped download left: hashed again, then asked for from where it ends.
    let mut have = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if have >= w.pin.size {
        let _ = tokio::fs::remove_file(&part).await;
        have = 0;
    }
    let mut hasher = Sha256::new();
    if have > 0 {
        let p = part.clone();
        hasher = tokio::task::spawn_blocking(move || -> std::io::Result<Sha256> {
            let mut h = Sha256::new();
            let mut f = std::fs::File::open(&p)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    return Ok(h);
                }
                h.update(&buf[..n]);
            }
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Ghost cannot read {}: {e}", part.display()))?;
    }
    let mut req = client.get(&w.pin.url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let host = reqwest::Url::parse(&w.pin.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Ghost could not reach {host} for {}: {e}", w.pin.name))?;
    let status = resp.status();
    let resumed = have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !status.is_success() {
        return Err(format!(
            "{host} answered {status} for {}; try again later",
            w.pin.name
        ));
    }
    let mut file = if resumed {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        have = 0;
        hasher = Sha256::new();
        tokio::fs::File::create(&part).await
    }
    .map_err(|e| format!("Ghost cannot write {}: {e}", part.display()))?;
    let mut got = have;
    report(got);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            format!(
                "The download of {} from {host} stopped: {e}. Try again: it carries on where it stopped",
                w.pin.name
            )
        })?;
        got += chunk.len() as u64;
        if got > w.pin.size {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!(
                "{} is larger than it should be; nothing of it is kept",
                w.pin.name
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Ghost cannot write {}: {e}", part.display()))?;
        report(got);
    }
    file.flush().await.map_err(|e| e.to_string())?;
    file.sync_all().await.map_err(|e| e.to_string())?;
    drop(file);
    let hash = hex::encode(hasher.finalize());
    if got != w.pin.size || hash != w.pin.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        if got < w.pin.size {
            return Err(format!(
                "The download of {} from {host} ended early ({got} of {} bytes). Try again",
                w.pin.name, w.pin.size
            ));
        }
        return Err(format!(
            "{} from {host} is not the file Ghost expects (its SHA-256 does not match the \
             pin), so nothing of it is kept",
            w.pin.name
        ));
    }
    tokio::fs::rename(&part, &w.dest)
        .await
        .map_err(|e| format!("Ghost cannot move {} into place: {e}", w.pin.name))?;
    info!("speech models: {} ({} bytes)", w.dest.display(), w.pin.size);
    Ok(())
}

/// A client for the downloads: no cookies, no identifier, the crate's own user agent.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("ghostd/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client")
}

/// What the daemon serves: the view, and one download at a time.
pub struct SpeechModelsDesk {
    daemon: Weak<crate::daemon::Daemon>,
    state: Arc<Mutex<SpeechModelsFetch>>,
    running: Arc<AtomicBool>,
    /// Woken when a download ends, so the model load tries again at once.
    loaded: Arc<tokio::sync::Notify>,
}

impl SpeechModelsDesk {
    pub fn new(daemon: Weak<crate::daemon::Daemon>, loaded: Arc<tokio::sync::Notify>) -> Arc<Self> {
        Arc::new(Self {
            daemon,
            state: Arc::new(Mutex::new(SpeechModelsFetch::idle())),
            running: Arc::new(AtomicBool::new(false)),
            loaded,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SpeechModelsFetch> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SpeechModelsPort for SpeechModelsDesk {
    fn view(&self) -> SpeechModelsView {
        let Some(d) = self.daemon.upgrade() else {
            return SpeechModelsView {
                ready: false,
                missing: Vec::new(),
                missing_bytes: 0,
                folder: String::new(),
                free_bytes: None,
                fetch: self.lock().clone(),
            };
        };
        let voice = d.voice();
        let wanted = missing(&voice);
        let folder = folder(&voice, &wanted);
        SpeechModelsView {
            ready: d.state.models_ready(),
            missing: wanted
                .iter()
                .map(|w| SpeechModelFile {
                    model: w.pin.model.to_string(),
                    name: w.pin.name.to_string(),
                    bytes: w.pin.size,
                })
                .collect(),
            missing_bytes: remaining(&wanted),
            free_bytes: free_bytes(&folder),
            folder: folder.display().to_string(),
            fetch: self.lock().clone(),
        }
    }

    fn fetch(&self) -> Result<SpeechModelsView, String> {
        let d = self
            .daemon
            .upgrade()
            .ok_or_else(|| "Ghost is stopping".to_string())?;
        let wanted = missing(&d.voice());
        if wanted.is_empty() {
            return Ok(self.view());
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(self.view());
        }
        let total = wanted.iter().map(|w| w.pin.size).sum();
        *self.lock() = SpeechModelsFetch {
            state: SpeechModelsFetchState::Fetching,
            done_bytes: 0,
            total_bytes: total,
            file: wanted.first().map(|w| w.pin.name.to_string()),
            error: None,
            following: false,
        };
        let state = self.state.clone();
        let running = self.running.clone();
        let loaded = self.loaded.clone();
        let stop = d.stop.clone();
        tokio::spawn(async move {
            let shown = state.clone();
            let progress = move |p: &SpeechModelsFetch| {
                *shown.lock().unwrap_or_else(|e| e.into_inner()) = p.clone();
            };
            let http = client();
            let result = tokio::select! {
                r = fetch_all(&http, &wanted, &progress) => r,
                _ = stop.cancelled() => Err("Ghost stopped during the download".to_string()),
            };
            {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        info!("speech models: every file is here");
                        s.state = SpeechModelsFetchState::Done;
                        s.done_bytes = s.total_bytes;
                        s.file = None;
                        s.error = None;
                        s.following = false;
                    }
                    Err(e) => {
                        warn!("speech models: {e}");
                        // ADR 0067: by kind; the operator's own stop is not a failure.
                        if !stop.is_cancelled() {
                            ghost_reports::failure(ghost_reports::Failure::new(
                                ghost_reports::FailureKind::SpeechModelsDownload,
                            ));
                        }
                        s.state = SpeechModelsFetchState::Failed;
                        s.error = Some(e);
                        s.following = false;
                    }
                }
            }
            running.store(false, Ordering::SeqCst);
            loaded.notify_one();
        });
        Ok(self.view())
    }
}

/// `ghostd models` and `ghostd models fetch`: the same files and pins, from a terminal.
/// Only progress and the outcome go to `out` (stdout); log lines go to stderr (#139).
/// `tty` is whether `out` is a terminal: there one line is redrawn, elsewhere (a log file,
/// a pipe) a line is written per percent at most.
pub async fn cli(
    args: &[String],
    voice: &VoiceSettings,
    out: &mut (dyn std::io::Write + Send),
    tty: bool,
) -> i32 {
    let wanted = missing(voice);
    match args.first().map(String::as_str) {
        None | Some("list") => {
            if wanted.is_empty() {
                let _ = writeln!(out, "every speech model is here");
                return 0;
            }
            for w in &wanted {
                let _ = writeln!(
                    out,
                    "missing: {} {} ({}) -> {}",
                    w.pin.model,
                    w.pin.name,
                    gb(w.pin.size),
                    w.dest.display()
                );
            }
            let _ = writeln!(
                out,
                "ghostd models fetch downloads them ({})",
                gb(remaining(&wanted))
            );
            1
        }
        Some("fetch") => fetch_cli(&client(), &wanted, out, tty).await,
        Some("voice-test") => voice_test(voice, out),
        Some(other) => {
            let _ = writeln!(
                out,
                "unknown: ghostd models {other}; ghostd models [list|fetch|voice-test]"
            );
            2
        }
    }
}

/// `ghostd models voice-test` (#168): Kokoro and its tokenizer loaded as `ghostd run` loads
/// them, one fixed sentence rendered into memory, never played. Says how long it took, or in
/// plain words why Ghost's voice cannot start.
pub fn voice_test(voice: &VoiceSettings, out: &mut (dyn std::io::Write + Send)) -> i32 {
    const SENTENCE: &str = "Ghost can speak on this computer.";
    let espeak = match ghost_voice::pipeline::espeak_ready(voice) {
        Ok(e) => e,
        Err(e) => {
            let _ = writeln!(out, "{}", e.plain());
            return 1;
        }
    };
    let _ = writeln!(out, "{}", crate::check::espeak_line(espeak));
    let started = std::time::Instant::now();
    let mut kokoro = match ghost_infer::Kokoro::load(&voice.kokoro_paths()) {
        Ok(k) => k,
        Err(e) => {
            let e = ghost_voice::pipeline::LoadError::new(ghost_reports::SpeechPart::Kokoro, e);
            let _ = writeln!(out, "{}", e.plain());
            return 1;
        }
    };
    let loaded = started.elapsed();
    let rendered = std::time::Instant::now();
    match kokoro.synthesize(
        SENTENCE,
        &voice.tts_voice,
        voice.tts_speed as f32,
        ghost_voice::tts::LANG,
    ) {
        Ok(audio) if !audio.is_empty() => {
            let _ = writeln!(
                out,
                "voice: rendered {:.2} s of audio ({} samples at 24 kHz) in {} ms; Kokoro loaded \
                 in {} ms",
                audio.len() as f64 / 24_000.0,
                audio.len(),
                rendered.elapsed().as_millis(),
                loaded.as_millis()
            );
            0
        }
        Ok(_) => {
            let _ = writeln!(out, "voice: Kokoro rendered nothing");
            1
        }
        Err(e) => {
            let _ = writeln!(out, "voice: {e}");
            1
        }
    }
}

/// `ghostd models fetch` over `wanted`: progress lines, then the outcome.
async fn fetch_cli(
    http: &reqwest::Client,
    wanted: &[Wanted],
    out: &mut (dyn std::io::Write + Send),
    tty: bool,
) -> i32 {
    if wanted.is_empty() {
        let _ = writeln!(out, "every speech model is here");
        return 0;
    }
    let printer = Mutex::new((Printer::new(tty), out));
    let progress = |p: &SpeechModelsFetch| {
        let mut g = printer.lock().unwrap_or_else(|e| e.into_inner());
        let (printer, out) = &mut *g;
        if let Some(text) = printer.line(p) {
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
    };
    let result = fetch_all(http, wanted, &progress).await;
    let (printer, out) = printer.into_inner().unwrap_or_else(|e| e.into_inner());
    let end = if printer.open_line { "\n" } else { "" };
    match result {
        Ok(()) => {
            let _ = writeln!(out, "{end}every speech model is here");
            0
        }
        Err(e) => {
            let _ = writeln!(out, "{end}{e}");
            1
        }
    }
}

/// What `ghostd models fetch` prints for each report: a line when the percent or the file
/// changes, and once, in words, when the download was already under way elsewhere or
/// when this process took it over.
struct Printer {
    tty: bool,
    file: String,
    pct: Option<u64>,
    following: bool,
    said_following: bool,
    /// A terminal's line redrawn with `\r`, not yet ended.
    open_line: bool,
}

impl Printer {
    fn new(tty: bool) -> Self {
        Self {
            tty,
            file: String::new(),
            pct: None,
            following: false,
            said_following: false,
            open_line: false,
        }
    }

    fn line(&mut self, p: &SpeechModelsFetch) -> Option<String> {
        let file = p.file.clone().unwrap_or_default();
        let pct = (p.done_bytes * 100).checked_div(p.total_bytes).unwrap_or(0);
        let was_following = self.following;
        self.following = p.following;
        let words = if p.following && !self.said_following {
            self.said_following = true;
            Some(format!(
                "Already downloading: {pct}% {file} (another Ghost started it; this follows it)"
            ))
        } else if was_following && !p.following && p.done_bytes < p.total_bytes {
            Some(format!(
                "The other download stopped; carrying on from {pct}% {file}"
            ))
        } else {
            None
        };
        let changed = file != self.file || self.pct.is_none_or(|last| pct > last);
        self.file = file.clone();
        self.pct = Some(pct);
        let end = if self.open_line { "\n" } else { "" };
        if let Some(words) = words {
            self.open_line = false;
            return Some(format!("{end}{words}\n"));
        }
        if !changed {
            return None;
        }
        if self.tty {
            self.open_line = true;
            Some(format!("\r{pct:>3}% {file}\x1b[K"))
        } else {
            Some(format!("{pct:>3}% {file}\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;

    /// The pins here are the script's, file for file (scripts/models.sh).
    #[test]
    fn pins_match_the_script() {
        let script = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/models.sh"),
        )
        .unwrap();
        for pin in parakeet().iter().chain(kokoro().iter()) {
            let line = script
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("\"{}|", pin.name)))
                .unwrap_or_else(|| panic!("{} is not in scripts/models.sh", pin.name));
            let fields: Vec<&str> = line.trim().trim_matches('"').split('|').collect();
            assert_eq!(fields[2], pin.sha256, "{}", pin.name);
            assert_eq!(fields[3], pin.size.to_string(), "{}", pin.name);
            let tail = fields[1].rsplit('/').next().unwrap();
            assert_eq!(tail, pin.name);
        }
        assert!(script.contains("PARAKEET_REV=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"));
        assert!(PARAKEET_URL.ends_with("8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"));
        assert!(script.contains(&format!("KOKORO_URL={KOKORO_URL}")));
    }

    /// A file server that honours one Range request, the way Hugging Face and GitHub do.
    async fn serve(body: Vec<u8>) -> String {
        async fn file(
            State(body): State<Arc<Vec<u8>>>,
            headers: HeaderMap,
        ) -> axum::response::Response {
            if let Some(from) = headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.strip_suffix('-'))
                .and_then(|v| v.parse::<usize>().ok())
            {
                return (StatusCode::PARTIAL_CONTENT, body[from..].to_vec()).into_response();
            }
            (StatusCode::OK, body.to_vec()).into_response()
        }
        let app = axum::Router::new()
            .route("/f", axum::routing::get(file))
            .with_state(Arc::new(body));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}/f")
    }

    fn wanted(url: String, body: &[u8], dest: PathBuf) -> Wanted {
        let sha = hex::encode(Sha256::digest(body));
        Wanted {
            pin: Pin {
                model: "Test",
                name: "model.onnx",
                url,
                sha256: Box::leak(sha.into_boxed_str()),
                size: body.len() as u64,
            },
            dest,
        }
    }

    #[tokio::test]
    async fn a_file_lands_whole_after_its_pin_matches() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub/model.onnx");
        let w = wanted(url, &body, dest.clone());
        let seen = Mutex::new(Vec::new());
        fetch_all(&client(), std::slice::from_ref(&w), &|p| {
            seen.lock().unwrap().push(p.done_bytes)
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists());
        let seen = seen.into_inner().unwrap();
        assert_eq!(*seen.last().unwrap(), body.len() as u64);
    }

    #[tokio::test]
    async fn a_file_that_is_not_the_pin_is_refused_and_nothing_is_kept() {
        let body = b"the real model".to_vec();
        let url = serve(b"something else!".to_vec()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let w = wanted(url, &body, dest.clone());
        let e = fetch_all(&client(), &[w], &|_| {}).await.unwrap_err();
        assert!(
            e.contains("larger than it should be") || e.contains("SHA-256"),
            "{e}"
        );
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[tokio::test]
    async fn a_stopped_download_carries_on_where_it_stopped() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
        let url = serve(body.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(part_path(&dest), &body[..40_000]).unwrap();
        let w = wanted(url, &body, dest.clone());
        assert_eq!(remaining(std::slice::from_ref(&w)), 60_000);
        let first = Mutex::new(None);
        fetch_all(&client(), &[w], &|p| {
            first.lock().unwrap().get_or_insert(p.done_bytes);
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(first.into_inner().unwrap(), Some(40_000));
    }

    #[tokio::test]
    async fn an_unreachable_host_is_said_in_plain_words() {
        let dir = tempfile::tempdir().unwrap();
        // A port nothing listens on.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", l.local_addr().unwrap());
        drop(l);
        let w = wanted(url, b"x", dir.path().join("m"));
        let e = fetch_all(&client(), &[w], &|_| {}).await.unwrap_err();
        assert!(e.starts_with("Ghost could not reach 127.0.0.1"), "{e}");
    }

    /// A slow file server that counts its requests: `chunks` pieces of `body`, `pause`
    /// apart, honouring one Range request.
    struct Slow {
        url: String,
        requests: Arc<std::sync::atomic::AtomicUsize>,
        /// The `Range` start each request asked for, in order.
        ranges: Arc<Mutex<Vec<Option<usize>>>>,
    }

    async fn serve_slow(body: Vec<u8>, chunks: usize, pause: Duration) -> Slow {
        #[derive(Clone)]
        struct S {
            body: Arc<Vec<u8>>,
            chunks: usize,
            pause: Duration,
            requests: Arc<std::sync::atomic::AtomicUsize>,
            ranges: Arc<Mutex<Vec<Option<usize>>>>,
        }
        async fn file(State(s): State<S>, headers: HeaderMap) -> axum::response::Response {
            s.requests.fetch_add(1, Ordering::SeqCst);
            let from = headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.strip_suffix('-'))
                .and_then(|v| v.parse::<usize>().ok());
            s.ranges.lock().unwrap().push(from);
            let status = if from.is_some() {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            };
            let rest = s.body[from.unwrap_or(0)..].to_vec();
            let size = rest.len().div_ceil(s.chunks).max(1);
            let pieces: Vec<Vec<u8>> = rest.chunks(size).map(<[u8]>::to_vec).collect();
            let pause = s.pause;
            let stream = futures::stream::iter(pieces).then(move |p| async move {
                tokio::time::sleep(pause).await;
                Ok::<_, std::io::Error>(p)
            });
            (status, axum::body::Body::from_stream(stream)).into_response()
        }
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .route("/f", axum::routing::get(file))
            .with_state(S {
                body: Arc::new(body),
                chunks,
                pause,
                requests: requests.clone(),
                ranges: ranges.clone(),
            });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        Slow {
            url: format!("http://{addr}/f"),
            requests,
            ranges,
        }
    }

    /// #135: the terminal's fetch and the app's, on one folder at once, are one download.
    /// Each fetcher here has its own state, as two processes would; only the folder is
    /// shared.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_fetchers_on_one_folder_make_one_request_and_both_end_ready() {
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        let stub = serve_slow(body.clone(), 40, Duration::from_millis(20)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let run = |w: Wanted| async move {
            let seen = Arc::new(Mutex::new(Vec::<SpeechModelsFetch>::new()));
            let s = seen.clone();
            let r = fetch_all(&client(), &[w], &move |p| s.lock().unwrap().push(p.clone())).await;
            (r, Arc::try_unwrap(seen).unwrap().into_inner().unwrap())
        };
        let a = tokio::spawn(run(wanted(stub.url.clone(), &body, dest.clone())));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let b = tokio::spawn(run(wanted(stub.url.clone(), &body, dest.clone())));
        let (ra, sa) = a.await.unwrap();
        let (rb, sb) = b.await.unwrap();
        ra.unwrap();
        rb.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body, "the file is the pin");
        assert_eq!(
            stub.requests.load(Ordering::SeqCst),
            1,
            "one request for the file, whoever asked"
        );
        assert!(
            sa.iter().any(|p| p.done_bytes > 0),
            "the first reports progress"
        );
        assert!(
            sb.iter()
                .filter(|p| p.done_bytes > 0 && p.done_bytes < body.len() as u64)
                .count()
                >= 2,
            "the second reports the first's progress as it goes"
        );
        assert!(!part_path(&dest).exists());
    }

    /// Fetch one file and keep every report, as the terminal or the app would.
    async fn fetch_seen(w: Wanted) -> (Result<(), String>, Vec<SpeechModelsFetch>) {
        let seen = Arc::new(Mutex::new(Vec::<SpeechModelsFetch>::new()));
        let s = seen.clone();
        let r = fetch_all(&client(), &[w], &move |p| s.lock().unwrap().push(p.clone())).await;
        let seen = seen.lock().unwrap().clone();
        (r, seen)
    }

    const CHILD: &str = "GHOST_TEST_SPEECH_FETCH_CHILD";

    /// Not a test on its own: the other process of the two-process tests below, this test
    /// binary run again for this one test. Does nothing unless the parent set `CHILD`.
    #[tokio::test]
    async fn child_fetcher() {
        let Ok(spec) = std::env::var(CHILD) else {
            return;
        };
        let f: Vec<&str> = spec.split('|').collect();
        let w = Wanted {
            pin: Pin {
                model: "Test",
                name: "model.onnx",
                url: f[0].to_string(),
                sha256: Box::leak(f[1].to_string().into_boxed_str()),
                size: f[2].parse().unwrap(),
            },
            dest: PathBuf::from(f[3]),
        };
        fetch_all(&client(), &[w], &|_| {}).await.unwrap();
    }

    /// The test binary again, fetching `w` in a process of its own.
    fn child(w: &Wanted) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "speech_models::tests::child_fetcher",
                "--test-threads=1",
                "--quiet",
            ])
            .env(
                CHILD,
                format!(
                    "{}|{}|{}|{}",
                    w.pin.url,
                    w.pin.sha256,
                    w.pin.size,
                    w.dest.display()
                ),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Until `f` holds, looking every 10 ms for at most 10 s.
    async fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..1000 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// #135 across processes, as the terminal and the app are: one request, both ready.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_processes_on_one_folder_make_one_request_and_both_end_ready() {
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 249) as u8).collect();
        let stub = serve_slow(body.clone(), 50, Duration::from_millis(20)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let w = wanted(stub.url.clone(), &body, dest.clone());
        let mut other = child(&w);
        let part = part_path(&dest);
        until("the other process to start writing", || {
            std::fs::metadata(&part).is_ok_and(|m| m.len() > 0)
        })
        .await;
        let (r, seen) = fetch_seen(w).await;
        r.unwrap();
        assert!(
            other.wait().unwrap().success(),
            "the other process ended ready"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(stub.requests.load(Ordering::SeqCst), 1, "{:?}", stub.ranges);
        let followed: Vec<u64> = seen
            .iter()
            .filter(|p| p.following)
            .map(|p| p.done_bytes)
            .collect();
        assert!(followed.len() >= 2, "{seen:?}");
        assert!(
            followed.windows(2).any(|w| w[1] > w[0]),
            "the bar moves: {followed:?}"
        );
        assert!(
            !lock_path(&dest).exists(),
            "the lock goes with the download"
        );
    }

    /// The holder dies part way (SIGKILL: no clean-up of any kind); the follower takes the
    /// lock the kernel let go and carries on from the `.part`, asking only for the rest.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_follower_finishes_when_the_holder_is_killed() {
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 241) as u8).collect();
        let stub = serve_slow(body.clone(), 100, Duration::from_millis(30)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let w = wanted(stub.url.clone(), &body, dest.clone());
        let mut holder = child(&w);
        let part = part_path(&dest);
        until("the holder to start writing", || {
            std::fs::metadata(&part).is_ok_and(|m| m.len() > 0)
        })
        .await;
        let follower = tokio::spawn(fetch_seen(w));
        tokio::time::sleep(Duration::from_millis(600)).await;
        holder.kill().unwrap();
        holder.wait().unwrap();
        let cut = std::fs::metadata(&part).unwrap().len() as usize;
        assert!(cut > 0 && cut < body.len(), "killed part way: {cut}");
        let (r, seen) = follower.await.unwrap();
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let ranges = stub.ranges.lock().unwrap().clone();
        assert_eq!(ranges.len(), 2, "{ranges:?}");
        assert_eq!(ranges[0], None);
        assert!(ranges[1].is_some_and(|from| from > 0), "{ranges:?}");
        assert!(seen.iter().any(|p| p.following), "it followed first");
        assert!(
            seen.last()
                .is_some_and(|p| !p.following && p.done_bytes == body.len() as u64),
            "then finished it itself: {:?}",
            seen.last()
        );
    }

    /// A follower that arrives at 99% follows to the end and checks the file against the
    /// pin; nothing is asked for twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_follower_arriving_at_99_percent_ends_ready() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 239) as u8).collect();
        let stub = serve_slow(body.clone(), 10, Duration::from_millis(60)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(part_path(&dest), &body[..99_000]).unwrap();
        let holder = tokio::spawn(fetch_seen(wanted(stub.url.clone(), &body, dest.clone())));
        until("the holder's request", || {
            stub.requests.load(Ordering::SeqCst) == 1
        })
        .await;
        let (r, seen) = fetch_seen(wanted(stub.url.clone(), &body, dest.clone())).await;
        r.unwrap();
        holder.await.unwrap().0.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(*stub.ranges.lock().unwrap(), vec![Some(99_000)]);
        let first = seen.first().unwrap();
        assert!(first.following && first.done_bytes >= 99_000, "{first:?}");
        assert_eq!(seen.last().unwrap().done_bytes, body.len() as u64);
    }

    /// A lock file nobody holds (its process is gone) blocks nothing.
    #[tokio::test]
    async fn a_lock_file_left_behind_does_not_block() {
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 7) as u8).collect();
        let stub = serve_slow(body.clone(), 2, Duration::from_millis(1)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(lock_path(&dest), b"").unwrap();
        let (r, seen) = fetch_seen(wanted(stub.url.clone(), &body, dest.clone())).await;
        r.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(seen.iter().all(|p| !p.following));
        assert!(!held_elsewhere(&dest));
    }

    /// Where the lock is held, `held_elsewhere` says so, and makes no lock file where
    /// there is none.
    #[test]
    fn held_elsewhere_reads_the_lock_and_makes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m");
        assert!(!held_elsewhere(&dest));
        assert!(!lock_path(&dest).exists());
        let Turn::Mine(lock) = take_turn(&dest) else {
            panic!("free")
        };
        assert!(held_elsewhere(&dest));
        assert!(matches!(take_turn(&dest), Turn::Busy));
        drop(lock);
        assert!(!held_elsewhere(&dest));
    }

    fn report(done: u64, file: &str, following: bool) -> SpeechModelsFetch {
        SpeechModelsFetch {
            state: SpeechModelsFetchState::Fetching,
            done_bytes: done,
            total_bytes: 1000,
            file: Some(file.into()),
            error: None,
            following,
        }
    }

    /// #139, #135: a line per percent at most when the output is not a terminal, and
    /// "already downloading" said once when another process holds the download.
    #[test]
    fn the_terminal_prints_a_line_per_percent_and_says_once_it_follows() {
        let mut p = Printer::new(false);
        let mut lines = Vec::new();
        for (done, file, following) in [
            (410, "a", true),
            (412, "a", true),
            (415, "a", true),
            (421, "a", true),
            (430, "a", false),
            (431, "a", false),
            (1000, "b", false),
        ] {
            if let Some(l) = p.line(&report(done, file, following)) {
                lines.push(l);
            }
        }
        assert_eq!(
            lines,
            [
                "Already downloading: 41% a (another Ghost started it; this follows it)\n",
                " 42% a\n",
                "The other download stopped; carrying on from 43% a\n",
                "100% b\n",
            ]
        );
        assert!(lines.iter().all(|l| !l.contains('\r')));
        let mut t = Printer::new(true);
        assert_eq!(
            t.line(&report(10, "a", false)).unwrap(),
            "\r  1% a\x1b[K",
            "a terminal's line is redrawn"
        );
    }

    /// #139: what `ghostd models fetch` writes to stdout is progress and the outcome,
    /// nothing else, also when it follows another fetcher.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_fetch_commands_stdout_is_progress_only() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 233) as u8).collect();
        let stub = serve_slow(body.clone(), 20, Duration::from_millis(20)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        let first = tokio::spawn(fetch_seen(wanted(stub.url.clone(), &body, dest.clone())));
        until("the first fetcher's request", || {
            stub.requests.load(Ordering::SeqCst) == 1
        })
        .await;
        let mut out = Vec::new();
        let code = fetch_cli(
            &client(),
            &[wanted(stub.url.clone(), &body, dest.clone())],
            &mut out,
            false,
        )
        .await;
        first.await.unwrap().0.unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{out}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("Already downloading: "), "{out}");
        assert_eq!(*lines.last().unwrap(), "every speech model is here");
        for l in &lines[1..lines.len() - 1] {
            assert!(
                l.ends_with("% model.onnx") && !l.contains("INFO"),
                "only progress: {l:?}"
            );
        }
        assert_eq!(stub.requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn what_is_missing_follows_the_settings_and_skips_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let mut voice = VoiceSettings {
            cache_dir: dir.path().to_path_buf(),
            stt_dir: None,
            stt_quantization: None,
            tts_model_path: None,
            tts_voices_path: None,
            ..Default::default()
        };
        let all = missing(&voice);
        assert_eq!(all.len(), 7);
        let total: u64 = all.iter().map(|w| w.pin.size).sum();
        assert!(total > 2_800_000_000 && total < 3_000_000_000, "{total}");
        let k = voice.kokoro_paths();
        std::fs::create_dir_all(k.voices.parent().unwrap()).unwrap();
        std::fs::write(&k.voices, b"v").unwrap();
        assert_eq!(missing(&voice).len(), 6);
        assert!(
            missing_note(&missing(&voice))
                .starts_with("Ghost's ears and voice are not downloaded yet (2.9 GB)"),
            "{}",
            missing_note(&missing(&voice))
        );
        voice.stt_quantization = Some("int8".into());
        assert_eq!(
            missing(&voice).len(),
            1,
            "int8 is a developer's, not fetched"
        );
        assert!(missing_note(&missing(&voice)).starts_with("Ghost's voice is not downloaded"));
    }
}
