//! Ghost's voice from ElevenLabs, with Kokoro beside it (proposed ADR 0026).
//!
//! With `GHOST_TTS_ENGINE=elevenlabs` a session speaks through ElevenLabs Flash v2.5 over
//! the multi-context websocket, with Ghost's spoken-text filter and clause aggregation,
//! 24 kHz PCM like Kokoro. Only the reply text Ghost is about to say goes there; the
//! microphone never does, and STT stays local. The key is `ELEVENLABS_API_KEY`, sent as
//! the `xi-api-key` header and nowhere else.
//!
//! Kokoro stays loaded, and Ghost never goes silent for the cloud voice's sake:
//!
//! - no key or no voice id when the session is built: the session is plain Kokoro, logged
//!   once per process;
//! - the socket fails to connect, drops, errors, or stops returning audio: [`ElevenLabsTts`]
//!   writes itself off for the rest of the session at the first sign of it (no retry loop,
//!   which would be silence), and [`TtsFailover`] switches to Kokoro and hands it the words
//!   of the reply in flight that were not heard yet ([`Unheard`]).
//!
//! An agent's voice (ADR 0014) reaches the TTS as Kokoro settings (a Kokoro voice name and
//! a speed). The cloud stage never sends that name to ElevenLabs: it speaks the agent's
//! ElevenLabs voice from `GHOST_ELEVENLABS_VOICES` (by slug) or the one voice in
//! `GHOST_ELEVENLABS_VOICE`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use ghost_infer::clauses::ClauseAggregator;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::warn;
use zeroize::Zeroizing;

use crate::config::VoiceSettings;
use crate::frames::{Audio, Direction, Frame, Out, Stage};
use crate::spoken::spoken;
use crate::tts::{KokoroTts, OnSay, SharedSynth, TtsSettings};
use crate::turns::TurnControl;

/// Kokoro's rate; the output transport runs at it.
pub const SAMPLE_RATE: u32 = 24_000;
pub const DEFAULT_URL: &str = "wss://api.elevenlabs.io";
/// The handshake is about 0.1-0.4 s normally. Past this the network is not there, and the
/// session's setup (and so its greeting) should not wait on it: Kokoro speaks instead.
pub const CONNECT_TIMEOUT_SECS: f64 = 3.0;
/// ElevenLabs takes 0.7-1.2; Kokoro's agent speeds (0.5-2.0) are clamped into that.
pub const SPEED_RANGE: (f64, f64) = (0.7, 1.2);

pub type LineFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// `GHOST_ELEVENLABS_VOICES` ("accounting=<voice id>,research=<voice id>") as a map by
/// agent slug, and the entries that could not be read.
pub fn line_voices(raw: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut voices = BTreeMap::new();
    let mut bad = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            Some((slug, voice)) if !slug.trim().is_empty() && !voice.trim().is_empty() => {
                voices.insert(slug.trim().to_string(), voice.trim().to_string());
            }
            _ => bad.push(entry.to_string()),
        }
    }
    (voices, bad)
}

/// What keeps the ElevenLabs voice from being used, without naming any value.
pub fn tts_problems(s: &VoiceSettings) -> Vec<String> {
    if s.tts_engine != "elevenlabs" {
        return Vec::new();
    }
    let mut problems = Vec::new();
    if s.elevenlabs_api_key.is_empty() {
        problems.push("GHOST_TTS_ENGINE=elevenlabs but ELEVENLABS_API_KEY is not set".into());
    }
    if s.elevenlabs_voice.is_empty() {
        problems.push("GHOST_TTS_ENGINE=elevenlabs but GHOST_ELEVENLABS_VOICE is not set".into());
    }
    problems
}

static WARNED: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(|| Mutex::new(BTreeSet::new()));

fn warn_once(message: &str) {
    if WARNED.lock().unwrap().insert(message.to_string()) {
        warn!("{message}");
    }
}

#[cfg(test)]
fn reset_warned() {
    WARNED.lock().unwrap().clear();
}

/// One line for `ghostd check` and the daemon's start-up log.
pub fn describe_tts(s: &VoiceSettings) -> String {
    let kokoro = format!("Kokoro {}", s.tts_voice);
    if s.tts_engine != "elevenlabs" {
        return kokoro;
    }
    let problems = tts_problems(s);
    if !problems.is_empty() {
        return format!("{kokoro} ({})", problems.join("; "));
    }
    let (voices, _) = line_voices(&s.elevenlabs_voices);
    let per_line = if voices.is_empty() {
        String::new()
    } else {
        format!(", {} agent(s) with their own", voices.len())
    };
    format!(
        "ElevenLabs {}, voice {}{per_line}; {kokoro} as the fallback",
        s.elevenlabs_model, s.elevenlabs_voice
    )
}

/// What a session gives its voice besides the voice's own settings.
pub struct VoiceHooks {
    pub turns: Arc<TurnControl>,
    /// Told each clause as it goes to the voice (the echo filter's feed).
    pub on_say: Option<OnSay>,
    /// The reply's first clause, rendered ahead (ADR 0049); Kokoro's only.
    pub prerender: Option<crate::tts::Prerender>,
}

/// The session's TTS stage: a fresh Kokoro stage, or ElevenLabs with that stage as its
/// fallback. `line` names the active agent, for its ElevenLabs voice. What is missing is
/// logged once per process, not once per session.
pub fn session_tts(
    s: &VoiceSettings,
    kokoro: SharedSynth,
    settings: TtsSettings,
    clause_min_words: Option<usize>,
    line: Option<LineFn>,
    hooks: VoiceHooks,
) -> Box<dyn Stage> {
    let VoiceHooks {
        turns,
        on_say,
        prerender,
    } = hooks;
    // Rendering ahead is Kokoro's (ADR 0049): behind the cloud voice it would render what
    // the cloud then says.
    let prerender = prerender.filter(|_| s.tts_engine != "elevenlabs");
    let local = match KokoroTts::new(kokoro, settings, clause_min_words) {
        Ok(k) => k
            .with_turns(turns.clone())
            .with_prerender(prerender)
            .with_first_clause(s.tts_first_clause_words),
        Err(e) => {
            warn!("tts: {e}; Kokoro's default voice speaks instead");
            KokoroTts::new(
                Arc::new(Mutex::new(crate::tts::NoSynth)),
                TtsSettings::default(),
                clause_min_words,
            )
            .expect("the empty synth takes any voice")
        }
    };
    let local = match &on_say {
        Some(f) => local.with_on_say(f.clone()),
        None => local,
    };
    if s.tts_engine != "elevenlabs" {
        return Box::new(local);
    }
    let (voices, bad) = line_voices(&s.elevenlabs_voices);
    if !bad.is_empty() {
        // counted, not echoed: whatever was pasted there stays out of the log
        warn_once(&format!(
            "tts: {} GHOST_ELEVENLABS_VOICES entries not read (want agent=id)",
            bad.len()
        ));
    }
    let problems = tts_problems(s);
    for p in &problems {
        warn_once(&format!("tts: {p}; Kokoro speaks instead"));
    }
    if !problems.is_empty() {
        return Box::new(local);
    }
    let cloud = ElevenLabsTts::new(ElevenLabsConfig {
        api_key: s.elevenlabs_api_key.clone(),
        voice: s.elevenlabs_voice.clone(),
        model: s.elevenlabs_model.clone(),
        voices,
        line,
        clause_min_words,
        first_clause_words: s.tts_first_clause_words,
        url: DEFAULT_URL.into(),
        connect_timeout: Duration::from_secs_f64(CONNECT_TIMEOUT_SECS),
    })
    .with_turns(turns);
    let cloud = match on_say {
        Some(f) => cloud.with_on_say(f),
        None => cloud,
    };
    Box::new(TtsFailover::new(cloud, local))
}

/// An agent's speed for ElevenLabs: None (its default) at 1.0, else clamped.
pub fn cloud_speed(speed: f64) -> Option<f64> {
    if (speed - 1.0).abs() < 1e-6 {
        return None;
    }
    Some(speed.clamp(SPEED_RANGE.0, SPEED_RANGE.1))
}

// --- what was heard ----------------------------------------------------------------

fn key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The words handed to the cloud voice whose audio has not come back yet.
///
/// Text is added as it is routed to ElevenLabs (LLM tokens, a whole spoken line); each
/// word ElevenLabs aligns with returned audio is struck off. A word that matches nothing
/// is skipped; the next match a few words on strikes off what lies before it, so a
/// Markdown marker or a link the spoken-text filter dropped doesn't leave the two out of
/// step for long. What is left when the socket fails is what Kokoro says.
#[derive(Debug, Default)]
pub struct Unheard {
    words: VecDeque<String>,
    partial: String, // a word still arriving token by token
}

impl Unheard {
    pub const LOOKAHEAD: usize = 8;

    pub fn add(&mut self, text: &str) {
        let text = format!("{}{text}", self.partial);
        let mut words: Vec<&str> = text.split_whitespace().collect();
        self.partial = if !text.is_empty() && !text.ends_with(char::is_whitespace) {
            words.pop().unwrap_or("").to_string()
        } else {
            String::new()
        };
        self.words.extend(words.into_iter().map(String::from));
    }

    /// The text so far is complete: its last word is whole.
    pub fn end(&mut self) {
        if !self.partial.is_empty() {
            self.words.push_back(std::mem::take(&mut self.partial));
        }
    }

    pub fn heard(&mut self, text: &str) {
        for word in text.split_whitespace() {
            let k = key(word);
            if k.is_empty() {
                continue;
            }
            let found = self
                .words
                .iter()
                .take(Self::LOOKAHEAD)
                .position(|pending| key(pending) == k);
            match found {
                Some(i) => {
                    for _ in 0..=i {
                        self.words.pop_front();
                    }
                }
                None => {
                    if !self.partial.is_empty() && key(&self.partial) == k {
                        self.partial.clear();
                    }
                }
            }
        }
    }

    pub fn partial(&self) -> bool {
        !self.partial.is_empty()
    }

    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = self.words.iter().map(String::as_str).collect();
        if !self.partial.is_empty() {
            parts.push(&self.partial);
        }
        parts.join(" ").trim().to_string()
    }

    pub fn clear(&mut self) {
        self.words.clear();
        self.partial.clear();
    }
}

// --- the cloud voice -------------------------------------------------------------------

#[derive(Clone)]
pub struct ElevenLabsConfig {
    pub api_key: Zeroizing<String>,
    /// The ElevenLabs voice id for every agent; `voices` overrides it by slug.
    pub voice: String,
    pub model: String,
    pub voices: BTreeMap<String, String>,
    pub line: Option<LineFn>,
    pub clause_min_words: Option<usize>,
    /// A reply's first piece cut early, as Kokoro's is (`GHOST_TTS_FIRST_CLAUSE_WORDS`):
    /// with `auto_mode` the cloud starts on each piece as it arrives, so a shorter first
    /// piece is sooner audio here too.
    pub first_clause_words: usize,
    pub url: String,
    pub connect_timeout: Duration,
}

/// What the stage and its socket reader share.
struct Shared {
    written_off: AtomicBool,
    failure: Mutex<String>,
    unheard: Mutex<Unheard>,
    /// A response is streaming into ElevenLabs.
    open: AtomicBool,
    /// ElevenLabs holds a response end it hasn't sent on.
    end_owed: AtomicBool,
    /// The context whose audio is being played, and whether speech was announced.
    speaking: AtomicBool,
    /// The reply's token, as the model stage stamped it.
    turn: Mutex<Option<tokio_util::sync::CancellationToken>>,
    /// The context being spoken in, for the keep-alive.
    context: std::sync::atomic::AtomicU64,
}

/// How often a quiet socket's context is told it is still wanted (ElevenLabs'
/// `KeepContextAlive`, an empty text), well inside the 180 s `inactivity_timeout`.
pub const KEEP_ALIVE: Duration = Duration::from_secs(60);

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Socket {
    tx: mpsc::Sender<String>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    voice: String,
}

/// ElevenLabs over its multi-context websocket, the way Ghost uses it: speech, not
/// Markdown; released a clause at a time; written off at the first error.
pub struct ElevenLabsTts {
    cfg: ElevenLabsConfig,
    shared: Arc<Shared>,
    socket: Option<Socket>,
    aggregator: ClauseAggregator,
    context_id: u64,
    turns: Option<Arc<TurnControl>>,
    speed: Option<f64>,
    /// Kokoro, taking over on failure: set by the failover.
    fallback: Option<Arc<tokio::sync::Mutex<KokoroTts>>>,
    on_say: Option<OnSay>,
    /// A handshake started ahead (`warm`), for this voice.
    pending: Option<(String, JoinHandle<Result<Ws, String>>)>,
    /// Connect only when there are words to send, as before ADR 0053 (the harness's
    /// before-and-after).
    cold: bool,
}

impl ElevenLabsTts {
    pub fn new(cfg: ElevenLabsConfig) -> Self {
        let aggregator =
            ClauseAggregator::new(cfg.clause_min_words).with_first_clause(cfg.first_clause_words);
        Self {
            cfg,
            shared: Arc::new(Shared {
                written_off: AtomicBool::new(false),
                failure: Mutex::new(String::new()),
                unheard: Mutex::new(Unheard::default()),
                open: AtomicBool::new(false),
                end_owed: AtomicBool::new(false),
                speaking: AtomicBool::new(false),
                turn: Mutex::new(None),
                context: std::sync::atomic::AtomicU64::new(0),
            }),
            socket: None,
            aggregator,
            context_id: 0,
            turns: None,
            speed: None,
            fallback: None,
            on_say: None,
            pending: None,
            cold: false,
        }
    }

    /// Connect only when the first words are ready, never ahead (the measurement's before).
    pub fn cold(mut self) -> Self {
        self.cold = true;
        self
    }

    pub fn with_turns(mut self, turns: Arc<TurnControl>) -> Self {
        self.turns = Some(turns);
        self
    }

    /// Told each clause as it goes to the cloud, before its audio comes back (`crate::tts::OnSay`).
    pub fn with_on_say(mut self, on_say: OnSay) -> Self {
        self.on_say = Some(on_say);
        self
    }

    pub fn written_off(&self) -> bool {
        self.shared.written_off.load(Ordering::SeqCst)
    }

    /// The first error, with the key scrubbed.
    pub fn failure(&self) -> String {
        self.shared.failure.lock().unwrap().clone()
    }

    /// The ElevenLabs voice an agent speaks in.
    pub fn voice_for(&self, slug: Option<&str>) -> String {
        slug.and_then(|s| self.cfg.voices.get(s).cloned())
            .unwrap_or_else(|| self.cfg.voice.clone())
    }

    fn active_voice(&self) -> String {
        let slug = self.cfg.line.as_ref().and_then(|l| l());
        self.voice_for(slug.as_deref())
    }

    /// Kokoro settings for an agent, as ElevenLabs settings: the agent's ElevenLabs voice,
    /// never the Kokoro name; the speed clamped to what ElevenLabs takes.
    pub fn line_delta(&self, kokoro: &TtsSettings) -> (String, Option<f64>) {
        (self.active_voice(), cloud_speed(kokoro.speed))
    }

    fn scrub(&self, text: &str) -> String {
        if self.cfg.api_key.is_empty() {
            text.to_string()
        } else {
            text.replace(self.cfg.api_key.as_str(), "[ELEVENLABS_API_KEY]")
        }
    }

    fn write_off(&mut self, reason: &str) {
        let reason = self.scrub(reason);
        if !self.shared.written_off.swap(true, Ordering::SeqCst) {
            let mut f = self.shared.failure.lock().unwrap();
            if f.is_empty() {
                *f = reason.clone();
            }
        }
        if let Some(s) = self.socket.take() {
            s.reader.abort();
            s.writer.abort();
        }
        if let Some((_, h)) = self.pending.take() {
            h.abort();
        }
    }

    /// The socket's handshake for `voice`, on a task of its own: TLS and the websocket
    /// upgrade, 0.1 to 0.4 s, bounded by the connect timeout. The result is the stream or
    /// the reason, with nothing of the key in it.
    fn connect(&self, voice: &str) -> JoinHandle<Result<Ws, String>> {
        // `inactivity_timeout` at its documented maximum: a context left quiet for the
        // default 20 s between turns would otherwise be closed under a session that is
        // only listening (the keep-alive below covers longer).
        let url = format!(
            "{}/v1/text-to-speech/{voice}/multi-stream-input?model_id={}&output_format=pcm_{SAMPLE_RATE}&auto_mode=true&inactivity_timeout=180",
            self.cfg.url.trim_end_matches('/'),
            self.cfg.model
        );
        let key = self.cfg.api_key.clone();
        let timeout = self.cfg.connect_timeout;
        tokio::spawn(async move {
            let mut request =
                tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                    url.as_str(),
                )
                .map_err(|e| format!("bad url: {e}"))?;
            let header = key
                .as_str()
                .parse()
                .map_err(|_| "the key can't be sent as a header".to_string())?;
            request.headers_mut().insert("xi-api-key", header);
            match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request)).await {
                Ok(Ok((ws, _))) => Ok(ws),
                Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) => {
                    Err(format!("HTTP {}", r.status().as_u16()))
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("connect timed out".to_string()),
            }
        })
    }

    /// Start the handshake ahead of the first words (ADR 0053): at the session's first
    /// frame and after an agent's voice change, so a reply never waits for it. Nothing when
    /// a socket for the voice is up, a connect is under way, or the voice is written off.
    fn warm(&mut self) {
        if self.cold || self.written_off() || self.pending.is_some() {
            return;
        }
        let voice = self.active_voice();
        if self
            .socket
            .as_ref()
            .is_some_and(|s| s.voice == voice && !s.reader.is_finished())
        {
            return;
        }
        let handle = self.connect(&voice);
        self.pending = Some((voice, handle));
    }

    /// Connect (once per voice) and spawn the reader; false when written off.
    async fn ensure_socket(&mut self, out: &Out) -> bool {
        if self.written_off() {
            return false;
        }
        let voice = self.active_voice();
        if let Some(s) = &self.socket
            && s.voice == voice
            && !s.reader.is_finished()
        {
            return true;
        }
        if let Some(s) = self.socket.take() {
            let _ = s.tx.send(json!({"close_socket": true}).to_string()).await;
            s.reader.abort();
            s.writer.abort();
        }
        // A connect started ahead for this voice (`warm`), else one now.
        let connecting = match self.pending.take() {
            Some((v, handle)) if v == voice => handle,
            Some((_, handle)) => {
                handle.abort();
                self.connect(&voice)
            }
            None => self.connect(&voice),
        };
        let ws = match connecting.await {
            Ok(Ok(ws)) => ws,
            Ok(Err(reason)) => {
                self.write_off(&reason);
                return false;
            }
            Err(_) => {
                self.write_off("the connect was dropped");
                return false;
            }
        };
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let alive = self.shared.clone();
        let writer = tokio::spawn(async move {
            let mut keep =
                tokio::time::interval_at(tokio::time::Instant::now() + KEEP_ALIVE, KEEP_ALIVE);
            loop {
                let text = tokio::select! {
                    t = rx.recv() => match t {
                        Some(t) => t,
                        None => break,
                    },
                    _ = keep.tick() => json!({
                        "text": "",
                        "context_id": alive.context.load(Ordering::SeqCst).to_string(),
                    })
                    .to_string(),
                };
                if sink.send(WsMessage::Text(text.into())).await.is_err() {
                    break;
                }
            }
        });
        let shared = self.shared.clone();
        let out = out.clone();
        let turns = self.turns.clone();
        let fallback = self.fallback.clone();
        let reader = tokio::spawn(async move {
            let reason = loop {
                match stream.next().await {
                    Some(Ok(WsMessage::Text(text))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&text) else {
                            continue;
                        };
                        if let Some(b64) = v.get("audio").and_then(Value::as_str) {
                            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
                            {
                                let pcm: Vec<i16> = bytes
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|c| i16::from_le_bytes(*c))
                                    .collect();
                                if !pcm.is_empty() {
                                    if !shared.speaking.swap(true, Ordering::SeqCst) {
                                        out.push(Frame::TtsStarted).await;
                                    }
                                    let turn =
                                        shared.turn.lock().unwrap().clone().unwrap_or_else(|| {
                                            turns.as_ref().map_or_else(
                                                tokio_util::sync::CancellationToken::new,
                                                |t| t.current(),
                                            )
                                        });
                                    out.push(Frame::TtsAudio {
                                        audio: Audio::new(pcm, SAMPLE_RATE),
                                        turn,
                                    })
                                    .await;
                                }
                            }
                            if let Some(chars) = v
                                .get("alignment")
                                .and_then(|a| a.get("chars"))
                                .and_then(Value::as_array)
                            {
                                let heard: String =
                                    chars.iter().filter_map(Value::as_str).collect();
                                shared.unheard.lock().unwrap().heard(&heard);
                                out.push(Frame::TtsText(heard)).await;
                            }
                        }
                        if v.get("isFinal").and_then(Value::as_bool) == Some(true) {
                            if shared.speaking.swap(false, Ordering::SeqCst) {
                                out.push(Frame::TtsStopped).await;
                            }
                            if shared.end_owed.swap(false, Ordering::SeqCst) {
                                out.push(Frame::LlmEnd).await;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        break frame.map_or("closed".to_string(), |f| {
                            format!("closed: {} {}", u16::from(f.code), f.reason)
                        });
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break e.to_string(),
                    None => break "the socket ended".to_string(),
                }
            };
            // Written off: Kokoro speaks for the rest of this session, starting with what
            // was not heard.
            if !shared.written_off.swap(true, Ordering::SeqCst) {
                let mut f = shared.failure.lock().unwrap();
                if f.is_empty() {
                    *f = reason.clone();
                }
            }
            warn!("tts: ElevenLabs failed ({reason}); Kokoro speaks for the rest of this session");
            if let Some(fallback) = fallback {
                let (text, partial) = {
                    let u = shared.unheard.lock().unwrap();
                    (u.text(), u.partial())
                };
                let open = shared.open.load(Ordering::SeqCst);
                let end_owed = shared.end_owed.swap(false, Ordering::SeqCst);
                shared.unheard.lock().unwrap().clear();
                let mut frames = Vec::new();
                if !text.is_empty() && open {
                    frames.push(Frame::LlmText(if partial {
                        text
                    } else {
                        format!("{text} ")
                    }));
                } else if !text.is_empty() {
                    frames.push(Frame::Speak(text));
                }
                if !open && end_owed {
                    frames.push(Frame::LlmEnd);
                }
                if shared.speaking.swap(false, Ordering::SeqCst) {
                    out.push(Frame::TtsStopped).await;
                }
                let mut k = fallback.lock().await;
                for f in frames {
                    k.process(f, Direction::Down, &out).await;
                }
            }
        });
        let tx_clone = tx.clone();
        self.socket = Some(Socket {
            tx,
            reader,
            writer,
            voice,
        });
        // The context's first message carries the voice settings.
        self.context_id += 1;
        self.shared.context.store(self.context_id, Ordering::SeqCst);
        let mut init = json!({"text": " ", "context_id": self.context_id.to_string()});
        if let Some(speed) = self.speed {
            init["voice_settings"] = json!({"speed": speed});
        }
        let _ = tx_clone.send(init.to_string()).await;
        true
    }

    async fn send(&mut self, msg: Value) {
        if let Some(s) = &self.socket
            && s.tx.send(msg.to_string()).await.is_err()
        {
            self.write_off("the socket's writer is gone");
        }
    }

    async fn say(&mut self, text: &str, out: &Out) {
        let text = spoken(text);
        if text.trim().is_empty() {
            return;
        }
        if let Some(on_say) = &self.on_say {
            on_say(&text);
        }
        if !self.ensure_socket(out).await {
            return;
        }
        let ctx = self.context_id.to_string();
        let text = if text.ends_with(' ') {
            text
        } else {
            format!("{text} ")
        };
        self.send(json!({"text": text, "context_id": ctx})).await;
    }

    async fn flush(&mut self) {
        if self.socket.is_none() || self.written_off() {
            return;
        }
        let ctx = self.context_id.to_string();
        self.send(json!({"context_id": ctx, "flush": true})).await;
    }
}

#[async_trait]
impl Stage for ElevenLabsTts {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction != Direction::Down || self.written_off() {
            if !matches!(frame, Frame::LlmText(_) | Frame::Speak(_)) {
                out.forward(frame, direction).await;
            }
            return;
        }
        if !matches!(
            frame,
            Frame::End | Frame::Cancel | Frame::VoiceChange { .. }
        ) {
            self.warm();
        }
        match &frame {
            Frame::LlmStart => {
                self.shared.open.store(true, Ordering::SeqCst);
                self.shared.end_owed.store(false, Ordering::SeqCst);
            }
            Frame::ReplyTurn(turn) => {
                *self.shared.turn.lock().unwrap() = Some(turn.clone());
            }
            Frame::LlmText(text) => {
                for clause in self.aggregator.push(text) {
                    self.say(&clause, out).await;
                }
                return;
            }
            Frame::LlmEnd => {
                if let Some(rest) = self.aggregator.flush() {
                    self.say(&rest, out).await;
                }
                self.shared.open.store(false, Ordering::SeqCst);
                if self.socket.is_some() && !self.written_off() {
                    // The end follows the audio: sent on when the context says isFinal.
                    self.shared.end_owed.store(true, Ordering::SeqCst);
                    self.flush().await;
                    if self.written_off() {
                        self.shared.end_owed.store(false, Ordering::SeqCst);
                        out.forward(frame, direction).await;
                    }
                    return;
                }
            }
            Frame::Speak(text) => {
                let mut pieces = self.aggregator.push(text);
                pieces.extend(self.aggregator.flush());
                for p in pieces {
                    self.say(&p, out).await;
                }
                self.flush().await;
                return;
            }
            Frame::VoiceChange { voice: _, speed } => {
                self.speed = cloud_speed(*speed);
                // The agent's ElevenLabs voice: reconnect for the next text.
                if let Some(s) = self.socket.take() {
                    let _ = s.tx.send(json!({"close_socket": true}).to_string()).await;
                    s.reader.abort();
                    s.writer.abort();
                }
                self.shared.speaking.store(false, Ordering::SeqCst);
                return;
            }
            Frame::Interruption => {
                self.aggregator.reset();
                *self.shared.turn.lock().unwrap() = None;
                self.shared.unheard.lock().unwrap().clear();
                self.shared.open.store(false, Ordering::SeqCst);
                self.shared.end_owed.store(false, Ordering::SeqCst);
                if self.socket.is_some() {
                    let ctx = self.context_id.to_string();
                    self.send(json!({"context_id": ctx, "close_context": true}))
                        .await;
                    self.context_id += 1;
                    self.shared.context.store(self.context_id, Ordering::SeqCst);
                    let ctx = self.context_id.to_string();
                    self.send(json!({"text": " ", "context_id": ctx})).await;
                }
                self.shared.speaking.store(false, Ordering::SeqCst);
            }
            Frame::End | Frame::Cancel => {
                if let Some((_, h)) = self.pending.take() {
                    h.abort();
                }
                if let Some(s) = self.socket.take() {
                    let _ = s.tx.send(json!({"close_socket": true}).to_string()).await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    s.reader.abort();
                    s.writer.abort();
                }
            }
            _ => {}
        }
        out.forward(frame, direction).await;
    }

    async fn stopped(&mut self) {
        if let Some((_, h)) = self.pending.take() {
            h.abort();
        }
        if let Some(s) = self.socket.take() {
            s.reader.abort();
            s.writer.abort();
        }
    }
}

// --- the failover ----------------------------------------------------------------------

/// The TTS slot with ElevenLabs active and Kokoro behind it.
///
/// An agent's voice update reaches both, so Kokoro already speaks in the active agent's
/// voice when it takes over. When `ElevenLabsTts` writes itself off, the switch happens at
/// once (its socket reader hands Kokoro what of the reply in flight was not heard: as more
/// of the same response when the model is still streaming it, else as a line of its own),
/// the error stays inside this stage (the session keeps going).
pub struct TtsFailover {
    primary: ElevenLabsTts,
    fallback: Arc<tokio::sync::Mutex<KokoroTts>>,
    on_fallback: bool,
}

impl TtsFailover {
    pub fn new(mut primary: ElevenLabsTts, fallback: KokoroTts) -> Self {
        let fallback = Arc::new(tokio::sync::Mutex::new(fallback));
        primary.fallback = Some(fallback.clone());
        Self {
            primary,
            fallback,
            on_fallback: false,
        }
    }

    pub fn primary(&self) -> &ElevenLabsTts {
        &self.primary
    }

    pub fn fallback(&self) -> &Arc<tokio::sync::Mutex<KokoroTts>> {
        &self.fallback
    }

    pub fn on_fallback(&self) -> bool {
        self.on_fallback || self.primary.written_off()
    }

    /// Keep the text going into ElevenLabs, as the stage decides what it speaks.
    fn routed(&self, frame: &Frame) {
        let shared = &self.primary.shared;
        match frame {
            Frame::Interruption => {
                shared.unheard.lock().unwrap().clear(); // cut off on purpose: nothing to make up for
            }
            Frame::Speak(text) => {
                let mut u = shared.unheard.lock().unwrap();
                u.add(&spoken(text));
                u.end();
            }
            Frame::LlmText(text) => {
                shared.unheard.lock().unwrap().add(&spoken(text));
            }
            Frame::LlmEnd => {
                shared.unheard.lock().unwrap().end();
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Stage for TtsFailover {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
        if direction == Direction::Down
            && matches!(frame, Frame::VoiceChange { .. } | Frame::ReplyTurn(_))
        {
            // an agent's voice is for both engines, and so is the reply's turn
            self.fallback
                .lock()
                .await
                .process(frame.clone(), direction, out)
                .await;
            if !self.on_fallback() {
                self.primary.process(frame, direction, out).await;
            }
            return;
        }
        if !self.on_fallback() {
            if direction == Direction::Down {
                self.routed(&frame);
            }
            let keep = frame.clone();
            self.primary.process(frame, direction, out).await;
            if self.primary.written_off() {
                // Written off on this very frame (a failed connect): Kokoro takes it.
                self.on_fallback = true;
                if matches!(keep, Frame::LlmText(_) | Frame::Speak(_)) {
                    let open = self.primary.shared.open.load(Ordering::SeqCst);
                    let text = {
                        let mut u = self.primary.shared.unheard.lock().unwrap();
                        let t = u.text();
                        u.clear();
                        t
                    };
                    if !text.is_empty() {
                        let f = if open && matches!(keep, Frame::LlmText(_)) {
                            Frame::LlmText(format!("{text} "))
                        } else {
                            Frame::Speak(text)
                        };
                        self.fallback.lock().await.process(f, direction, out).await;
                    }
                }
            }
            return;
        }
        self.on_fallback = true;
        self.fallback
            .lock()
            .await
            .process(frame, direction, out)
            .await;
    }

    async fn stopped(&mut self) {
        self.primary.stopped().await;
        self.fallback.lock().await.stopped().await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::result_large_err)]

    use super::*;
    use crate::frames::drain;
    use crate::tts::Synth;
    use std::collections::HashMap;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    const KEY: &str = "sk_test_do_not_print_0123456789abcdef";

    struct FakeKokoro {
        spoken: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Synth for FakeKokoro {
        fn voices(&self) -> Vec<String> {
            vec!["af_heart".into(), "am_adam".into()]
        }
        fn synthesize(
            &mut self,
            text: &str,
            voice: &str,
            _speed: f32,
        ) -> ghost_infer::Result<Vec<i16>> {
            self.spoken
                .lock()
                .unwrap()
                .push((text.into(), voice.into()));
            Ok(vec![0; 2400])
        }
    }

    type Spoken = Arc<Mutex<Vec<(String, String)>>>;

    fn kokoro() -> (SharedSynth, Spoken) {
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let synth: SharedSynth = Arc::new(Mutex::new(FakeKokoro {
            spoken: spoken.clone(),
        }));
        (synth, spoken)
    }

    fn settings(env: &[(&str, &str)]) -> VoiceSettings {
        let map: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        VoiceSettings::from_vars(move |name| map.get(name).cloned()).unwrap()
    }

    fn closed_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    /// Enough of ElevenLabs' multi-stream-input socket: audio with character alignment
    /// for each text message, `isFinal` when a context closes. `drop_after` closes the
    /// socket with an error once that many texts have been answered; `reject` refuses the
    /// handshake with that HTTP status.
    struct FakeElevenLabs {
        url: String,
        paths: Arc<Mutex<Vec<String>>>,
        keys: Arc<Mutex<Vec<Option<String>>>>,
        texts: Arc<Mutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl FakeElevenLabs {
        async fn start(drop_after: Option<usize>, reject: Option<u16>) -> Self {
            Self::start_with(drop_after, reject, Duration::ZERO, Duration::ZERO).await
        }

        /// With the service's own times: `handshake` before the upgrade is answered (TLS
        /// and the upgrade, 0.1 to 0.4 s from this desk), `audio_after` from a text to its
        /// first audio (Flash v2.5, about 0.25 s, ADR 0026).
        async fn start_with(
            drop_after: Option<usize>,
            reject: Option<u16>,
            handshake: Duration,
            audio_after: Duration,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let paths = Arc::new(Mutex::new(Vec::new()));
            let keys = Arc::new(Mutex::new(Vec::new()));
            let texts = Arc::new(Mutex::new(Vec::new()));
            let (p, k, t) = (paths.clone(), keys.clone(), texts.clone());
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let (p, k, t) = (p.clone(), k.clone(), t.clone());
                    tokio::spawn(async move {
                        let check = |req: &Request, resp: Response| {
                            p.lock().unwrap().push(req.uri().to_string());
                            k.lock().unwrap().push(
                                req.headers()
                                    .get("xi-api-key")
                                    .map(|v| v.to_str().unwrap_or("").to_string()),
                            );
                            if let Some(status) = reject {
                                let mut e = ErrorResponse::new(Some("no\n".into()));
                                *e.status_mut() = status.try_into().unwrap();
                                return Err(e);
                            }
                            Ok(resp)
                        };
                        tokio::time::sleep(handshake).await;
                        let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await
                        else {
                            return;
                        };
                        let (mut sink, mut stream) = ws.split();
                        let mut answered = 0usize;
                        while let Some(Ok(msg)) = stream.next().await {
                            let WsMessage::Text(raw) = msg else { continue };
                            let Ok(m) = serde_json::from_str::<Value>(&raw) else {
                                continue;
                            };
                            let ctx = m.get("context_id").cloned();
                            if m.get("close_socket").and_then(Value::as_bool) == Some(true) {
                                let _ = sink.close().await;
                                return;
                            }
                            if m.get("close_context").is_some() || m.get("flush").is_some() {
                                let _ = sink
                                    .send(WsMessage::Text(
                                        json!({"isFinal": true, "contextId": ctx})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                                continue;
                            }
                            let text = m
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if text.trim().is_empty() || ctx.is_none() {
                                continue; // context init or keepalive
                            }
                            t.lock().unwrap().push(text.clone());
                            if drop_after.is_some_and(|d| answered >= d) {
                                continue; // the network is gone: nothing comes back for this one
                            }
                            let spoken = if text.ends_with(' ') {
                                text.clone()
                            } else {
                                format!("{text} ")
                            };
                            tokio::time::sleep(audio_after).await;
                            let chars: Vec<String> =
                                spoken.chars().map(|c| c.to_string()).collect();
                            let n = chars.len();
                            let _ = sink
                                .send(WsMessage::Text(
                                    json!({
                                        "audio": base64::engine::general_purpose::STANDARD.encode(vec![0u8; 4800]),
                                        "alignment": {
                                            "chars": chars,
                                            "charStartTimesMs": (0..n).map(|i| 10 * i).collect::<Vec<_>>(),
                                            "charDurationsMs": vec![10; n],
                                        },
                                        "contextId": ctx,
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                            answered += 1;
                            if drop_after.is_some_and(|d| answered >= d) {
                                tokio::time::sleep(Duration::from_millis(300)).await; // let the heard words land first
                                let _ = sink
                                    .send(WsMessage::Close(Some(
                                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                            code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Error,
                                            reason: "gone".into(),
                                        },
                                    )))
                                    .await;
                                return;
                            }
                        }
                    });
                }
            });
            Self {
                url: format!("ws://{}:{}", addr.ip(), addr.port()),
                paths,
                keys,
                texts,
                task,
            }
        }

        fn paths(&self) -> Vec<String> {
            self.paths.lock().unwrap().clone()
        }
        fn keys(&self) -> Vec<Option<String>> {
            self.keys.lock().unwrap().clone()
        }
        fn texts(&self) -> Vec<String> {
            self.texts.lock().unwrap().clone()
        }
    }

    impl Drop for FakeElevenLabs {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn cloud(url: &str, voices: BTreeMap<String, String>, line: Option<LineFn>) -> ElevenLabsTts {
        ElevenLabsTts::new(ElevenLabsConfig {
            api_key: Zeroizing::new(KEY.into()),
            voice: "el_default".into(),
            model: "eleven_flash_v2_5".into(),
            voices,
            line,
            clause_min_words: Some(4),
            first_clause_words: 0,
            url: url.into(),
            connect_timeout: Duration::from_secs(3),
        })
    }

    fn failover(
        url: &str,
        synth: SharedSynth,
        voices: BTreeMap<String, String>,
        line: Option<LineFn>,
    ) -> TtsFailover {
        let local = KokoroTts::new(synth, TtsSettings::default(), Some(4)).unwrap();
        TtsFailover::new(cloud(url, voices, line), local)
    }

    enum Step {
        F(Frame),
        Sleep(f64),
    }

    async fn run(tts: &mut TtsFailover, steps: Vec<Step>) -> Vec<Frame> {
        let (out, mut rx) = Out::capture();
        for s in steps {
            match s {
                Step::F(f) => tts.process(f, Direction::Down, &out).await,
                Step::Sleep(secs) => tokio::time::sleep(Duration::from_secs_f64(secs)).await,
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        drain(&mut rx).await
    }

    // --- what Ghost adds before the cloud's first audio (ADR 0053) -------------------

    /// One reply as the model streams it, from the end of the operator's words: the
    /// model's first token after `model`, then a token every 3.4 ms (Haiku 5.5's 290 a
    /// second). The seconds from the first token to the first audio frame out of the stage.
    type Heard = Arc<Mutex<Vec<std::time::Instant>>>;

    /// The stage's output, every audio frame's arrival kept.
    fn listening() -> (Out, Heard, JoinHandle<()>) {
        let (out, mut rx) = Out::capture();
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        let watch = tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                if matches!(p.frame, Frame::TtsAudio { .. }) {
                    h.lock().unwrap().push(std::time::Instant::now());
                }
            }
        });
        (out, heard, watch)
    }

    async fn first_audio(
        tts: &mut TtsFailover,
        (out, heard): (&Out, &Heard),
        reply: &str,
        model: Duration,
    ) -> f64 {
        tts.process(Frame::UserStopped, Direction::Down, out).await;
        tokio::time::sleep(model).await;
        let began = std::time::Instant::now();
        tts.process(Frame::LlmStart, Direction::Down, out).await;
        for word in reply.split_inclusive(' ') {
            tts.process(Frame::LlmText(word.into()), Direction::Down, out)
                .await;
            tokio::time::sleep(Duration::from_micros(3400)).await;
        }
        tts.process(Frame::LlmEnd, Direction::Down, out).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(at) = heard.lock().unwrap().iter().find(|t| **t > began) {
                return at.duration_since(began).as_secs_f64();
            }
            assert!(std::time::Instant::now() < deadline, "no audio came back");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_the_cloud_voice_waits_for_the_service_and_not_for_ghost() {
        // A local stand-in with the service's times: 150 ms to connect, 250 ms from a
        // text to its audio. Before: the socket opened when the first clause was ready, and
        // the first clause was a whole clause. After: the socket opens at the turn's first
        // frame, and the first piece is cut early as Kokoro's is.
        let (handshake, service) = (Duration::from_millis(150), Duration::from_millis(250));
        let reply = "I found three issues waiting and two of them need your sign-off today.";
        let mut rows = Vec::new();
        for (label, cold, first_words) in [("before", true, 0usize), ("after", false, 4)] {
            let server = FakeElevenLabs::start_with(None, None, handshake, service).await;
            let mut cfg_tts = cloud(&server.url, BTreeMap::new(), None);
            cfg_tts.cfg.first_clause_words = first_words;
            cfg_tts.aggregator = ClauseAggregator::new(Some(4)).with_first_clause(first_words);
            if cold {
                cfg_tts = cfg_tts.cold();
            }
            let (synth, _) = kokoro();
            let local = KokoroTts::new(synth, TtsSettings::default(), Some(4)).unwrap();
            let mut tts = TtsFailover::new(cfg_tts, local);
            // The first reply of a session (a fresh socket), then one on the open socket.
            let (out, heard, watch) = listening();
            let model = Duration::from_millis(500);
            let fresh = first_audio(&mut tts, (&out, &heard), reply, model).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let open = first_audio(&mut tts, (&out, &heard), reply, model).await;
            watch.abort();
            assert!(!tts.on_fallback(), "{label}: fell back");
            eprintln!(
                "{label}: first token to first audio {:.0} ms on a fresh socket, {:.0} ms on an open one \
                 (the service's own {:.0} ms)",
                fresh * 1000.0,
                open * 1000.0,
                service.as_secs_f64() * 1000.0
            );
            rows.push((fresh, open));
        }
        let ghost = |t: f64| t - service.as_secs_f64();
        let (before, after) = (rows[0], rows[1]);
        // Before, a fresh socket paid the handshake after the first clause was written.
        assert!(ghost(before.0) > handshake.as_secs_f64(), "{before:?}");
        // After, the handshake is done while the model thinks, and the first piece goes as
        // soon as its four words are there: Ghost's share is the writing of those words
        // (about 15 ms here) and the hops, on either socket.
        assert!(ghost(after.0) < 0.08, "{after:?}");
        assert!(ghost(after.1) < 0.08, "{after:?}");
    }

    // --- choosing the engine ---------------------------------------------------------

    fn session(s: &VoiceSettings, synth: SharedSynth) -> Box<dyn Stage> {
        session_tts(
            s,
            synth,
            TtsSettings::default(),
            Some(4),
            None,
            VoiceHooks {
                turns: TurnControl::new(tokio_util::sync::CancellationToken::new()),
                on_say: None,
                prerender: None,
            },
        )
    }

    #[test]
    fn test_kokoro_is_the_default() {
        let (synth, _) = kokoro();
        let tts = session(&settings(&[]), synth);
        assert_eq!(tts.name(), "tts");
        assert_eq!(describe_tts(&settings(&[])), "Kokoro af_heart");
    }

    #[test]
    fn test_elevenlabs_is_chosen_with_kokoro_behind_it() {
        let s = settings(&[
            ("GHOST_TTS_ENGINE", "ElevenLabs"),
            ("ELEVENLABS_API_KEY", KEY),
            ("GHOST_ELEVENLABS_VOICE", "el_default"),
        ]);
        assert!(tts_problems(&s).is_empty());
        assert_eq!(
            describe_tts(&s),
            "ElevenLabs eleven_flash_v2_5, voice el_default; Kokoro af_heart as the fallback"
        );
        let (synth, _) = kokoro();
        let tts = session(&s, synth);
        assert_eq!(tts.name(), "tts");
        let c = cloud("ws://127.0.0.1:1", BTreeMap::new(), None);
        assert_eq!(c.cfg.model, "eleven_flash_v2_5");
        assert_eq!(c.cfg.voice, "el_default");
        assert_eq!(SAMPLE_RATE, 24000); // pcm_24000, like Kokoro
        assert_eq!(c.aggregator.pending(), ""); // the clause aggregator, text in whole clauses
    }

    #[test]
    fn test_unknown_engine_is_refused() {
        let map: HashMap<String, String> =
            [("GHOST_TTS_ENGINE".to_string(), "piper".to_string())].into();
        assert!(VoiceSettings::from_vars(move |n| map.get(n).cloned()).is_err());
    }

    #[test]
    fn test_missing_key_or_voice_is_kokoro_logged_once() {
        for env in [
            vec![("GHOST_ELEVENLABS_VOICE", "el_default")], // no key
            vec![("ELEVENLABS_API_KEY", KEY)],              // no voice
        ] {
            reset_warned();
            let mut e = vec![("GHOST_TTS_ENGINE", "elevenlabs")];
            e.extend(env);
            let s = settings(&e);
            for _ in 0..3 {
                let (synth, _) = kokoro();
                let tts = session(&s, synth);
                assert_eq!(tts.name(), "tts");
            }
            let warned = WARNED.lock().unwrap().clone();
            let about_kokoro: Vec<&String> = warned
                .iter()
                .filter(|w| w.contains("Kokoro speaks instead"))
                .collect();
            assert_eq!(about_kokoro.len(), 1);
            assert!(!warned.iter().any(|w| w.contains(KEY)));
        }
    }

    #[test]
    fn test_the_key_stays_out_of_the_settings_repr_and_the_description() {
        let s = settings(&[
            ("GHOST_TTS_ENGINE", "elevenlabs"),
            ("ELEVENLABS_API_KEY", KEY),
            ("GHOST_ELEVENLABS_VOICE", "el_default"),
        ]);
        assert_eq!(s.elevenlabs_api_key.as_str(), KEY);
        assert!(!format!("{s:?}").contains(KEY));
        assert!(!describe_tts(&s).contains(KEY));
    }

    #[test]
    fn test_line_voices_are_read_by_slug() {
        let (voices, bad) = line_voices(" accounting = el_acc ,research=el_res,,oops, =x");
        assert_eq!(
            voices,
            BTreeMap::from([
                ("accounting".to_string(), "el_acc".to_string()),
                ("research".to_string(), "el_res".to_string())
            ])
        );
        assert_eq!(bad, vec!["oops", "=x"]);
    }

    // --- an agent's voice -------------------------------------------------------------

    #[test]
    fn test_a_kokoro_voice_name_never_becomes_the_elevenlabs_voice() {
        let active = Arc::new(Mutex::new("ghost".to_string()));
        let a = active.clone();
        let line: LineFn = Arc::new(move || Some(a.lock().unwrap().clone()));
        let c = cloud(
            "ws://127.0.0.1:1",
            BTreeMap::from([("accounting".to_string(), "el_acc".to_string())]),
            Some(line),
        );
        let delta = TtsSettings {
            voice: "am_adam".into(),
            speed: 1.0,
        };
        assert_eq!(c.line_delta(&delta).0, "el_default");
        *active.lock().unwrap() = "accounting".into();
        let out = c.line_delta(&delta);
        assert_eq!(out.0, "el_acc");
        assert_eq!(out.1, None); // 1.0 is ElevenLabs' own default
        assert_eq!(
            c.line_delta(&TtsSettings {
                voice: "am_adam".into(),
                speed: 1.6
            })
            .1,
            Some(1.2)
        );
        assert_eq!(
            c.line_delta(&TtsSettings {
                voice: "am_adam".into(),
                speed: 0.5
            })
            .1,
            Some(0.7)
        );
    }

    #[tokio::test]
    async fn test_a_transfer_reconnects_in_the_lines_elevenlabs_voice() {
        let active = Arc::new(Mutex::new("ghost".to_string()));
        let a = active.clone();
        let line: LineFn = Arc::new(move || Some(a.lock().unwrap().clone()));
        let (synth, spoken) = kokoro();
        let server = FakeElevenLabs::start(None, None).await;
        let mut tts = failover(
            &server.url,
            synth,
            BTreeMap::from([("accounting".to_string(), "el_acc".to_string())]),
            Some(line),
        );
        let down = run(
            &mut tts,
            vec![
                Step::F(Frame::Speak("Patching you through.".into())),
                Step::Sleep(0.3),
                // the runtime's voice frame: Kokoro settings for the agent, and the switch
                Step::F(Frame::VoiceChange {
                    voice: "am_adam".into(),
                    speed: 1.0,
                }),
            ],
        )
        .await;
        *active.lock().unwrap() = "accounting".into(); // what ConversationRuntime.switch does
        let down2 = run(
            &mut tts,
            vec![
                Step::F(Frame::Speak("Accounting here.".into())),
                Step::Sleep(0.5),
            ],
        )
        .await;
        let paths = server.paths();
        assert!(
            paths[0].starts_with("/v1/text-to-speech/el_default/multi-stream-input"),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with("/v1/text-to-speech/el_acc/")),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.contains("am_adam")));
        assert!(paths[0].contains("model_id=eleven_flash_v2_5"));
        assert!(paths[0].contains("output_format=pcm_24000"));
        // Sent as the header, never in the URL.
        let keys = server.keys();
        assert!(!keys.is_empty() && keys.iter().all(|k| k.as_deref() == Some(KEY)));
        assert!(!paths.iter().any(|p| p.contains(KEY)));
        assert!(spoken.lock().unwrap().is_empty()); // ElevenLabs spoke; Kokoro only took the voice
        assert_eq!(tts.fallback().lock().await.settings().voice, "am_adam");
        assert!(
            down.iter()
                .chain(down2.iter())
                .any(|f| matches!(f, Frame::TtsAudio { .. }))
        );
    }

    // --- failing over -------------------------------------------------------------------

    #[tokio::test]
    async fn test_no_network_at_the_start_is_kokoro_for_the_session() {
        let (synth, spoken) = kokoro();
        let mut tts = failover(
            &format!("ws://127.0.0.1:{}", closed_port()),
            synth,
            BTreeMap::new(),
            None,
        );
        let down = run(
            &mut tts,
            vec![
                Step::F(Frame::Speak("Ghost here.".into())),
                Step::Sleep(0.3),
            ],
        )
        .await;
        assert_eq!(
            *spoken.lock().unwrap(),
            vec![("Ghost here.".to_string(), "af_heart".to_string())]
        );
        assert!(down.iter().any(|f| matches!(f, Frame::TtsAudio { .. })));
        assert!(tts.primary().written_off());
        assert!(tts.on_fallback());
        assert!(!down.iter().any(|f| matches!(f, Frame::Error(_)))); // kept inside the stage
        assert!(!tts.primary().failure().contains(KEY));
    }

    #[tokio::test]
    async fn test_a_rejected_key_is_kokoro_and_never_printed() {
        let (synth, spoken) = kokoro();
        let server = FakeElevenLabs::start(None, Some(401)).await;
        let mut tts = failover(&server.url, synth, BTreeMap::new(), None);
        let down = run(
            &mut tts,
            vec![
                Step::F(Frame::Speak("Ghost here.".into())),
                Step::Sleep(0.3),
            ],
        )
        .await;
        assert_eq!(
            *spoken.lock().unwrap(),
            vec![("Ghost here.".to_string(), "af_heart".to_string())]
        );
        assert!(
            tts.primary().failure().contains("401"),
            "{}",
            tts.primary().failure()
        );
        assert!(!down.iter().any(|f| matches!(f, Frame::Error(_))));
        assert!(!tts.primary().failure().contains(KEY));
    }

    #[tokio::test]
    async fn test_a_socket_that_drops_mid_reply_hands_kokoro_the_rest() {
        let (synth, spoken) = kokoro();
        let server = FakeElevenLabs::start(Some(1), None).await;
        let mut tts = failover(&server.url, synth, BTreeMap::new(), None);
        let down = run(
            &mut tts,
            vec![
                Step::F(Frame::LlmStart),
                Step::F(Frame::LlmText("Three issues are waiting, ".into())),
                Step::F(Frame::LlmText("the oldest is from Monday.".into())),
                Step::F(Frame::LlmEnd),
                Step::Sleep(1.0),
                Step::F(Frame::Speak("Anything else?".into())),
                Step::Sleep(0.3),
            ],
        )
        .await;
        assert!(server.texts()[0].starts_with("Three issues are waiting,"));
        // The first clause was heard from ElevenLabs; Kokoro says the rest, then what follows.
        let said: Vec<String> = spoken
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        assert_eq!(said, vec!["the oldest is from Monday.", "Anything else?"]);
        assert!(tts.primary().written_off());
        assert!(!down.iter().any(|f| matches!(f, Frame::Error(_))));
        assert_eq!(
            down.iter().filter(|f| matches!(f, Frame::LlmEnd)).count(),
            1
        );
    }

    #[tokio::test]
    async fn test_a_drop_while_the_reply_is_streaming_continues_it_in_kokoro() {
        let (synth, spoken) = kokoro();
        let server = FakeElevenLabs::start(Some(1), None).await;
        let mut tts = failover(&server.url, synth, BTreeMap::new(), None);
        run(
            &mut tts,
            vec![
                Step::F(Frame::LlmStart),
                // The first clause goes to ElevenLabs; "the old" waits for more.
                Step::F(Frame::LlmText("Three issues are waiting, the old".into())),
                Step::Sleep(0.8), // the socket drops here, mid-response
                Step::F(Frame::LlmText("est is ".into())),
                Step::F(Frame::LlmText("from Monday.".into())),
                Step::F(Frame::LlmEnd),
                Step::Sleep(0.3),
            ],
        )
        .await;
        assert_eq!(server.texts(), vec!["Three issues are waiting, "]);
        // The unheard "the old" and the rest of the stream join up into one sentence.
        let said: Vec<String> = spoken
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        assert_eq!(said, vec!["the oldest is from Monday."]);
    }

    // --- what was heard -----------------------------------------------------------------

    #[test]
    fn test_unheard_strikes_off_spoken_words_as_tokens_arrive() {
        let mut u = Unheard::default();
        for token in ["Three", " iss", "ues are", " waiting, the", " old"] {
            u.add(token);
        }
        for word in ["Three", "issues", "are"] {
            u.heard(word);
        }
        assert_eq!(u.text(), "waiting, the old");
        assert!(u.partial());
        u.add("est.");
        u.end();
        assert_eq!(u.text(), "waiting, the oldest.");
        assert!(!u.partial());
    }

    #[test]
    fn test_unheard_steps_over_what_the_filter_dropped() {
        let mut u = Unheard::default();
        u.add("- **Infra 17**: see [the PR](https://x.y/1) today. Then lunch. ");
        for word in ["Infra", "17:", "see", "the", "today.", "Then"] {
            u.heard(word);
        }
        assert_eq!(u.text(), "lunch.");
    }
}
