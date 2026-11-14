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
use tracing::{info, warn};

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
        let report = |n: u64| {
            progress(&SpeechModelsFetch {
                state: SpeechModelsFetchState::Fetching,
                done_bytes: before + n,
                total_bytes: total,
                file: Some(w.pin.name.to_string()),
                error: None,
            })
        };
        fetch_one(client, w, &report).await?;
        done += w.pin.size;
    }
    Ok(())
}

/// One file into place, or Err in plain words with nothing of a bad file kept.
async fn fetch_one(
    client: &reqwest::Client,
    w: &Wanted,
    report: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), String> {
    let dir = w
        .dest
        .parent()
        .ok_or_else(|| format!("{} has no folder", w.dest.display()))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Ghost cannot make {}: {e}", dir.display()))?;
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
pub async fn cli(
    args: &[String],
    voice: &VoiceSettings,
    out: &mut (dyn std::io::Write + Send),
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
        Some("fetch") => {
            if wanted.is_empty() {
                let _ = writeln!(out, "every speech model is here");
                return 0;
            }
            let last = Mutex::new((String::new(), 0u64));
            let progress = |p: &SpeechModelsFetch| {
                let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
                let file = p.file.clone().unwrap_or_default();
                let pct = (p.done_bytes * 100).checked_div(p.total_bytes).unwrap_or(0);
                if file != l.0 || pct >= l.1 + 5 {
                    eprintln!("{pct:>3}% {file}");
                    *l = (file, pct);
                }
            };
            match fetch_all(&client(), &wanted, &progress).await {
                Ok(()) => {
                    let _ = writeln!(out, "every speech model is here");
                    0
                }
                Err(e) => {
                    let _ = writeln!(out, "{e}");
                    1
                }
            }
        }
        Some(other) => {
            let _ = writeln!(
                out,
                "unknown: ghostd models {other}; ghostd models [list|fetch]"
            );
            2
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
