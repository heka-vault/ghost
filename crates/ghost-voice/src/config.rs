//! Model and endpoint settings, under the names the Python daemon read (`ghost_voice.config`,
//! prefix `GHOST_`). The model connection itself is `ghost_core::connection` (ADR 0037); the
//! key is read by name from the environment when a target is built, so none is a field
//! here and none is in a `Debug`.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ghost_core::connection::{Env, ModelConnection, Target};
use zeroize::Zeroizing;

pub use ghost_audio::config::{parse_bool, parse_list};

pub const TTS_ENGINES: [&str; 2] = ["kokoro", "elevenlabs"];
/// Output cap per reply, tool calls included (`ghost_llm::LLM_MAX_TOKENS`).
pub const LLM_MAX_TOKENS: u32 = ghost_llm::LLM_MAX_TOKENS;
/// The VoiceSettings fields that make up the model connection, and the variables they
/// mirror (ghost_core.connection). ghostd refreshes them when the desktop saves.
pub const LLM_FIELDS: [(&str, &str); 11] = [
    ("llm_provider", "GHOST_LLM_PROVIDER"),
    ("llm_endpoint", "GHOST_LLM_ENDPOINT"),
    ("llm_model", "GHOST_LLM_MODEL"),
    ("llm_fast_model", "GHOST_LLM_FAST_MODEL"),
    ("llm_protocol", "GHOST_LLM_PROTOCOL"),
    ("llm_base_url", "GHOST_LLM_BASE_URL"),
    ("llm_password", "GHOST_LLM_PASSWORD"),
    ("llm_headers", "GHOST_LLM_HEADERS"),
    ("llm_timeout_secs", "GHOST_LLM_TIMEOUT_SECS"),
    ("anthropic_base_url", "ANTHROPIC_BASE_URL"),
    ("cliproxy_base_url", "CLIPROXY_BASE_URL"),
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("GHOST_TTS_ENGINE must be one of {}", TTS_ENGINES.join(", "))]
    Engine,
    #[error("GHOST_LLM_ENDPOINT must be one of proxy, openrouter")]
    Endpoint,
    #[error("{0} is not a number")]
    Number(String),
    #[error("GHOST_TTS_STREAM must be whole or windowed")]
    Stream,
}

/// How Kokoro renders a piece (`GHOST_TTS_STREAM`, ADR 0053).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtsStream {
    /// The whole piece, then it plays.
    #[default]
    Whole,
    /// The vocoder a window of frames at a time; the first window plays while the rest
    /// renders.
    Windowed,
}

/// The voice package's settings. Defaults are the Python's.
#[derive(Clone)]
pub struct VoiceSettings {
    // The model connection (ADR 0037), as the environment names it.
    pub llm_env: HashMap<String, String>,
    pub llm_thinking: String,
    pub llm_max_tokens: u32,
    /// `GHOST_LLM_FAST_READS`: the fast model (ADR 0050) makes reads itself and answers
    /// from their results while they are no longer than `GHOST_LLM_FAST_READ_MAX_CHARS`,
    /// for at most `GHOST_LLM_FAST_READ_ROUNDS` rounds; off, any call hands the turn over.
    pub llm_fast_reads: bool,
    pub llm_fast_read_max_chars: usize,
    pub llm_fast_read_rounds: usize,

    // STT
    pub stt_model: String,
    /// `GHOST_STT_QUANTIZATION`: unset or `none`, the full-precision files (ADR 0043);
    /// `int8`, the quantized ones the parity fixtures were recorded with.
    pub stt_quantization: Option<String>,
    pub stt_threads: usize,
    /// `GHOST_STT_DIR`: where Parakeet's files are; unset, Ghost's cache
    /// (`models/parakeet-tdt-0.6b-v3/`, `scripts/models.sh parakeet`), and for int8 the
    /// Hugging Face cache.
    pub stt_dir: Option<PathBuf>,
    /// `GHOST_STT_EARLY_READ`: a segment is read from the VAD's first quiet frame, so its
    /// words are ready when the stop is confirmed (`on`); at the stop, as before (`off`);
    /// or both, keeping the stop's words and counting where they differ (`verify`, for
    /// the harness).
    pub stt_early_read: crate::stt::EarlyReadMode,
    /// `GHOST_STT_EARLY_READ_AFTER_SECS`: the quiet that starts the early read.
    pub stt_early_read_after_secs: f64,

    // TTS
    pub tts_voice: String,
    pub tts_speed: f64,
    pub tts_model_path: Option<PathBuf>,
    pub tts_voices_path: Option<PathBuf>,
    /// Speak from the first clause instead of the whole sentence (`ghost_infer::clauses`).
    pub tts_clauses: bool,
    pub tts_clause_min_words: usize,
    /// `GHOST_TTS_PRERENDER`: render a reply's first clause while the reply is still
    /// held (ADR 0049). Unset: on for Kokoro, off for a paid voice, where a withdrawn
    /// reply would be paid characters.
    pub tts_prerender: Option<bool>,
    /// `GHOST_TTS_FIRST_CLAUSE_WORDS`: a reply's first piece is cut once this many words
    /// are pending, at a comma or before a phrase's first word, so its audio starts sooner
    /// (`ghost_infer::clauses::ClauseAggregator::with_first_clause`); 0, the usual rule.
    pub tts_first_clause_words: usize,
    /// `GHOST_TTS_STREAM`: how Kokoro renders a piece (ADR 0053). `whole` (the default):
    /// the whole piece, then it plays. `windowed`: the model's two halves
    /// (`ghost_infer::kokoro_windowed`), the vocoder a window of frames at a time, so the
    /// first audio comes after the encoder and one short window; the halves' files missing,
    /// Kokoro renders whole and the waiting list says so.
    pub tts_stream: TtsStream,
    /// `GHOST_TTS_STREAM_DIR`: the two halves' folder; unset, `kokoro/windowed/` in Ghost's
    /// cache (`scripts/models.sh kokoro-windowed`).
    pub tts_stream_dir: Option<PathBuf>,
    /// `GHOST_TTS_WINDOW_FRAMES` (8, 25 ms each) and `GHOST_TTS_WINDOW_CONTEXT` (16): the
    /// first window's frames and the context decoded on each side of every window.
    pub tts_window_frames: usize,
    pub tts_window_context: usize,
    /// kokoro (local, the default) or elevenlabs (ADR 0026), Kokoro behind it.
    pub tts_engine: String,
    pub elevenlabs_api_key: Zeroizing<String>,
    pub elevenlabs_voice: String,
    pub elevenlabs_voices: String,
    pub elevenlabs_model: String,

    // Turn taking
    pub vad_confidence: f32,
    pub vad_start_secs: f32,
    pub vad_stop_secs: f32,
    pub vad_min_volume: f32,
    /// The VAD while Ghost speaks (`vad_params_over_ghost`).
    pub vad_over_ghost_confidence: f32,
    pub vad_over_ghost_start_secs: f32,
    pub vad_over_ghost_min_volume: f32,
    pub vad_over_ghost_stop_secs: f32,
    /// The VAD's start while a turn's close waits.
    pub vad_closing_start_secs: f32,
    pub wake_vad_stop_secs: f32,
    pub barge_in_min_words: usize,
    pub barge_in_interim_secs: f64,
    pub echo_window_secs: f64,
    pub turn_grace_secs: f64,
    /// `GHOST_TURN_CONFIDENT_GRACE_SECS`, `GHOST_TURN_CONFIDENT_PROB` (ADR 0049): the grace
    /// when Smart Turn is at least this sure and the words end a sentence. A grace at or
    /// over `turn_grace_secs`, or a probability over 1, is the one grace for every turn.
    pub turn_confident_grace_secs: f64,
    pub turn_confident_prob: f32,
    /// `GHOST_TURN_REOPEN_STARTED_SECS`: words from the operator whose speech began
    /// within this long of Ghost's first sample reopen the turn; 0 keeps a turn whose
    /// answer began, and the words are a barge-in.
    pub turn_reopen_started_secs: f64,
    pub turn_speculate: bool,
    /// `GHOST_TURN_REREAD`: a second and later segment replaces the turn's pieces with the
    /// whole turn read once (ADR 0044); off, the pieces as they came.
    pub turn_reread: bool,
    pub turn_stop_secs: f64,
    pub turn_unfinished_secs: f64,
    /// A full stop shortens the wait only when Smart Turn is at least this unsure.
    pub turn_sentence_min_prob: f32,
    /// Added to the grace when the words end on a number.
    pub turn_number_secs: f64,
    /// Audio before the speech that Smart Turn also hears.
    pub turn_pre_speech_secs: f64,
    pub idle_timeout_secs: f64,
    pub hold_timeout_secs: f64,
    pub smart_turn_threads: usize,
    /// `GHOST_SILERO_MODEL`, `GHOST_SMART_TURN_MODEL`: the ONNX files; unset, where
    /// `ghost_infer::models` finds them (`GHOST_MODELS_DIR`, the bundle's resources, then
    /// `~/.cache/ghost/models/`, where `scripts/models.sh` puts them), else the cache's
    /// path, so a load error names where the script would have put the file.
    pub silero_model: PathBuf,
    pub smart_turn_model: PathBuf,

    // Compaction (ADR 0014)
    pub compact_trigger_tokens: u64,
    pub compact_keep_messages: usize,
    pub compact_summary_tokens: u32,

    pub quick_intents: bool,
    /// `GHOST_ACK`: a one- or two-word acknowledgment as Ghost goes to an app (`acks`).
    pub ack: bool,
    pub thinking_sound: bool,
    pub thinking_volume: f64,
    pub thinking_delay_secs: f64,

    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub cache_dir: PathBuf,

    pub latency: bool,
    pub latency_max_mb: f64,
    pub debug_audio: bool,
    pub log_level: String,
}

impl fmt::Debug for VoiceSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The key and the connection's environment (which may hold one) stay out.
        f.debug_struct("VoiceSettings")
            .field("tts_engine", &self.tts_engine)
            .field("tts_voice", &self.tts_voice)
            .field("elevenlabs_voice", &self.elevenlabs_voice)
            .field("elevenlabs_model", &self.elevenlabs_model)
            .field("stt_model", &self.stt_model)
            .field("turn_grace_secs", &self.turn_grace_secs)
            .field("turn_reread", &self.turn_reread)
            .finish_non_exhaustive()
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self::from_vars(|_| None).expect("the defaults are valid")
    }
}

impl VoiceSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// From any lookup of `GHOST_*` names. A value that does not parse is an error, as
    /// pydantic's was; an empty one keeps the default.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let text = |name: &str| get(name).filter(|v| !v.trim().is_empty());
        let number = |name: &str| -> Result<Option<f64>, ConfigError> {
            match text(name) {
                None => Ok(None),
                Some(v) => v
                    .trim()
                    .parse::<f64>()
                    .map(Some)
                    .map_err(|_| ConfigError::Number(name.to_string())),
            }
        };
        let flag =
            |name: &str, default: bool| text(name).and_then(|v| parse_bool(&v)).unwrap_or(default);
        let string = |name: &str, default: &str| text(name).unwrap_or_else(|| default.to_string());
        let path = |name: &str| text(name).map(PathBuf::from);
        let f64_or = |name: &str, default: f64| number(name).map(|v| v.unwrap_or(default));
        let usize_or = |name: &str, default: usize| {
            number(name).map(|v| v.map_or(default, |x| x.max(0.0) as usize))
        };

        let mut llm_env = HashMap::new();
        for (_, env) in LLM_FIELDS {
            if let Some(v) = text(env) {
                llm_env.insert(env.to_string(), v);
            }
        }
        if let Some(endpoint) = llm_env.get("GHOST_LLM_ENDPOINT") {
            let e = endpoint.trim().to_lowercase();
            if !e.is_empty() && !ghost_core::connection::LEGACY_ENDPOINTS.contains(&e.as_str()) {
                return Err(ConfigError::Endpoint);
            }
        }
        let tts_engine = string("GHOST_TTS_ENGINE", "kokoro").trim().to_lowercase();
        if !TTS_ENGINES.contains(&tts_engine.as_str()) {
            return Err(ConfigError::Engine);
        }
        let cache_dir =
            path("GHOST_CACHE_DIR").unwrap_or_else(|| home().join(".cache").join("ghost"));
        // The small model files, where `ghost_infer::models` finds them; one that is
        // nowhere is named at the cache's path, which is where scripts/models.sh puts it.
        let exe = std::env::current_exe().ok();
        let home_dir = home();
        let model_file = |name: &str| {
            ghost_infer::models::resolve(
                name,
                |var| get(var).map(std::ffi::OsString::from),
                exe.as_deref(),
                Some(&home_dir),
            )
            .unwrap_or_else(|_| cache_dir.join(ghost_infer::models::FOLDER).join(name))
        };
        let silero_model =
            path("GHOST_SILERO_MODEL").unwrap_or_else(|| model_file(ghost_infer::models::SILERO));
        let smart_turn_model = path("GHOST_SMART_TURN_MODEL")
            .unwrap_or_else(|| model_file(ghost_infer::models::SMART_TURN));
        Ok(Self {
            llm_env,
            llm_thinking: string("GHOST_LLM_THINKING", ghost_llm::DEFAULT_THINKING),
            llm_max_tokens: number("GHOST_LLM_MAX_TOKENS")?.map_or(LLM_MAX_TOKENS, |v| v as u32),
            llm_fast_reads: flag("GHOST_LLM_FAST_READS", true),
            llm_fast_read_max_chars: usize_or("GHOST_LLM_FAST_READ_MAX_CHARS", 4000)?,
            llm_fast_read_rounds: usize_or("GHOST_LLM_FAST_READ_ROUNDS", 2)?,
            stt_model: string("GHOST_STT_MODEL", "nemo-parakeet-tdt-0.6b-v3"),
            stt_quantization: get("GHOST_STT_QUANTIZATION")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty() && v != "none"),
            stt_threads: usize_or("GHOST_STT_THREADS", 4)?,
            stt_dir: path("GHOST_STT_DIR"),
            stt_early_read_after_secs: f64_or("GHOST_STT_EARLY_READ_AFTER_SECS", 0.0)?,
            stt_early_read: match text("GHOST_STT_EARLY_READ")
                .map(|v| v.trim().to_lowercase())
                .as_deref()
            {
                None => crate::stt::EarlyReadMode::Off,
                Some("verify") => crate::stt::EarlyReadMode::Verify,
                Some("speculate") => crate::stt::EarlyReadMode::Speculate,
                Some(v) => match parse_bool(v) {
                    Some(true) => crate::stt::EarlyReadMode::On,
                    _ => crate::stt::EarlyReadMode::Off,
                },
            },
            tts_voice: string("GHOST_TTS_VOICE", "af_heart"),
            tts_speed: f64_or("GHOST_TTS_SPEED", 1.0)?,
            tts_model_path: path("GHOST_TTS_MODEL_PATH"),
            tts_voices_path: path("GHOST_TTS_VOICES_PATH"),
            tts_clauses: flag("GHOST_TTS_CLAUSES", true),
            tts_clause_min_words: usize_or("GHOST_TTS_CLAUSE_MIN_WORDS", 4)?,
            tts_prerender: text("GHOST_TTS_PRERENDER").and_then(|v| parse_bool(&v)),
            tts_first_clause_words: usize_or("GHOST_TTS_FIRST_CLAUSE_WORDS", 4)?,
            tts_stream: match text("GHOST_TTS_STREAM")
                .map(|v| v.trim().to_lowercase())
                .as_deref()
            {
                None | Some("") | Some("whole") | Some("off") => TtsStream::Whole,
                Some("windowed") | Some("on") => TtsStream::Windowed,
                Some(_) => return Err(ConfigError::Stream),
            },
            tts_stream_dir: path("GHOST_TTS_STREAM_DIR"),
            tts_window_frames: usize_or("GHOST_TTS_WINDOW_FRAMES", 8)?.max(1),
            tts_window_context: usize_or("GHOST_TTS_WINDOW_CONTEXT", 16)?,
            tts_engine,
            elevenlabs_api_key: Zeroizing::new(string("ELEVENLABS_API_KEY", "")),
            elevenlabs_voice: string("GHOST_ELEVENLABS_VOICE", ""),
            elevenlabs_voices: string("GHOST_ELEVENLABS_VOICES", ""),
            elevenlabs_model: string("GHOST_ELEVENLABS_MODEL", "eleven_flash_v2_5"),
            vad_confidence: f64_or("GHOST_VAD_CONFIDENCE", 0.7)? as f32,
            vad_start_secs: f64_or("GHOST_VAD_START_SECS", 0.2)? as f32,
            vad_stop_secs: f64_or("GHOST_VAD_STOP_SECS", 0.2)? as f32,
            vad_min_volume: f64_or("GHOST_VAD_MIN_VOLUME", 0.6)? as f32,
            vad_over_ghost_confidence: f64_or("GHOST_VAD_OVER_GHOST_CONFIDENCE", 0.5)? as f32,
            vad_over_ghost_start_secs: f64_or("GHOST_VAD_OVER_GHOST_START_SECS", 0.1)? as f32,
            vad_over_ghost_min_volume: f64_or("GHOST_VAD_OVER_GHOST_MIN_VOLUME", 0.3)? as f32,
            vad_over_ghost_stop_secs: f64_or("GHOST_VAD_OVER_GHOST_STOP_SECS", 0.6)? as f32,
            vad_closing_start_secs: f64_or("GHOST_VAD_CLOSING_START_SECS", 0.1)? as f32,
            wake_vad_stop_secs: f64_or("GHOST_WAKE_VAD_STOP_SECS", 0.5)? as f32,
            // 2 again (2026-10-30 evening, operator's call): the warm-up guard and the echo
            // filter carry the leak risk that 3 carried; "okay, okay" alone must cut.
            barge_in_min_words: usize_or("GHOST_BARGE_IN_MIN_WORDS", 2)?,
            barge_in_interim_secs: f64_or("GHOST_BARGE_IN_INTERIM_SECS", 0.3)?,
            echo_window_secs: f64_or("GHOST_ECHO_WINDOW_SECS", 3.0)?,
            turn_grace_secs: f64_or("GHOST_TURN_GRACE_SECS", 1.2)?,
            turn_confident_grace_secs: f64_or("GHOST_TURN_CONFIDENT_GRACE_SECS", 0.35)?,
            turn_confident_prob: f64_or("GHOST_TURN_CONFIDENT_PROB", 0.9)? as f32,
            turn_reopen_started_secs: f64_or("GHOST_TURN_REOPEN_STARTED_SECS", 0.15)?,
            turn_speculate: flag("GHOST_TURN_SPECULATE", true),
            turn_reread: flag("GHOST_TURN_REREAD", true),
            turn_stop_secs: f64_or("GHOST_TURN_STOP_SECS", 1.6)?,
            turn_unfinished_secs: f64_or("GHOST_TURN_UNFINISHED_SECS", 3.0)?,
            turn_sentence_min_prob: f64_or("GHOST_TURN_SENTENCE_MIN_PROB", 0.1)? as f32,
            turn_number_secs: f64_or("GHOST_TURN_NUMBER_SECS", 0.5)?,
            turn_pre_speech_secs: f64_or("GHOST_TURN_PRE_SPEECH_SECS", 1.0)?,
            idle_timeout_secs: f64_or("GHOST_IDLE_TIMEOUT_SECS", 45.0)?,
            hold_timeout_secs: f64_or("GHOST_HOLD_TIMEOUT_SECS", 1800.0)?,
            smart_turn_threads: usize_or("GHOST_SMART_TURN_THREADS", 2)?,
            silero_model,
            smart_turn_model,
            compact_trigger_tokens: f64_or("GHOST_COMPACT_TRIGGER_TOKENS", 24000.0)? as u64,
            compact_keep_messages: usize_or("GHOST_COMPACT_KEEP_MESSAGES", 20)?,
            compact_summary_tokens: f64_or("GHOST_COMPACT_SUMMARY_TOKENS", 3000.0)? as u32,
            quick_intents: flag("GHOST_QUICK_INTENTS", true),
            ack: flag("GHOST_ACK", true),
            // Off (2026-10-30): the pulses leak through the echo canceller, the VAD takes the
            // residual for a voice, and the operator finds it unpleasant; how Ghost shows it
            // is working is to be rethought.
            thinking_sound: flag("GHOST_THINKING_SOUND", false),
            thinking_volume: f64_or("GHOST_THINKING_VOLUME", 0.12)?,
            thinking_delay_secs: f64_or("GHOST_THINKING_DELAY_SECS", 0.5)?,
            input_device: text("GHOST_INPUT_DEVICE"),
            output_device: text("GHOST_OUTPUT_DEVICE"),
            cache_dir,
            latency: flag("GHOST_LATENCY", true),
            latency_max_mb: f64_or("GHOST_LATENCY_MAX_MB", 5.0)?,
            debug_audio: flag("GHOST_DEBUG_AUDIO", false),
            log_level: string("GHOST_LOG_LEVEL", "INFO"),
        })
    }

    pub fn llm_connection(&self) -> ModelConnection {
        ModelConnection::from_env(&self.llm_env)
    }

    /// Where model calls go now, the key read by name from `env` (the process environment
    /// by default, where the operator's Passwords are).
    pub fn llm_target(&self, env: Option<&HashMap<String, String>>) -> Target {
        match env {
            Some(env) => self.llm_connection().target(env),
            None => self.llm_connection().target(&process_env()),
        }
    }

    /// Where a spoken turn's first call goes when the connection names a fast model
    /// (ADR 0050); None, and the one model answers everything.
    pub fn llm_fast_target(&self, env: Option<&HashMap<String, String>>) -> Option<Target> {
        match env {
            Some(env) => self.llm_connection().fast_target(env),
            None => self.llm_connection().fast_target(&process_env()),
        }
    }

    /// What stops the model from being reachable, without naming any value.
    pub fn llm_problems(&self, env: Option<&HashMap<String, String>>) -> Vec<String> {
        match env {
            Some(env) => self.llm_connection().problems(env, false),
            None => self.llm_connection().problems(&process_env(), false),
        }
    }

    pub fn clause_min_words(&self) -> Option<usize> {
        self.tts_clauses.then_some(self.tts_clause_min_words)
    }

    /// How a reply is cut for the voice: clauses as `clause_min_words` says, and the
    /// first piece early when `tts_first_clause_words` says so. The TTS stage and the
    /// first clause rendered ahead cut by this one rule.
    pub fn clause_aggregator(&self) -> ghost_infer::clauses::ClauseAggregator {
        ghost_infer::clauses::ClauseAggregator::new(self.clause_min_words())
            .with_first_clause(self.tts_first_clause_words)
    }

    /// A reply's first clause is rendered while the reply is held: as set, else only
    /// with the local voice, where what is thrown away costs nothing but a moment of CPU.
    pub fn prerender(&self) -> bool {
        self.tts_prerender
            .unwrap_or(self.tts_engine != "elevenlabs")
    }

    /// The waits a turn's close runs by (`turns::TurnTiming`).
    pub fn turn_timing(&self) -> crate::turns::TurnTiming {
        crate::turns::TurnTiming {
            grace_secs: self.turn_grace_secs,
            confident_grace_secs: self.turn_confident_grace_secs,
            confident_prob: self.turn_confident_prob,
            stop_secs: self.turn_stop_secs,
            unfinished_secs: self.turn_unfinished_secs,
            sentence_min_prob: self.turn_sentence_min_prob,
            number_secs: self.turn_number_secs,
        }
    }

    /// The Kokoro files: the configured paths, else Ghost's cache.
    pub fn kokoro_paths(&self) -> ghost_infer::KokoroPaths {
        let dir = self.cache_dir.join("kokoro");
        ghost_infer::KokoroPaths {
            model: self
                .tts_model_path
                .clone()
                .unwrap_or_else(|| dir.join("kokoro-v1.0.onnx")),
            voices: self
                .tts_voices_path
                .clone()
                .unwrap_or_else(|| dir.join("voices-v1.0.bin")),
            espeak_data: None,
        }
    }

    /// The halves of Kokoro that render a window at a time (ADR 0053):
    /// `GHOST_TTS_STREAM_DIR`, else `kokoro/windowed/` in Ghost's cache.
    pub fn kokoro_halves_paths(&self) -> ghost_infer::kokoro_windowed::HalvesPaths {
        let dir = self
            .tts_stream_dir
            .clone()
            .unwrap_or_else(|| self.cache_dir.join("kokoro").join("windowed"));
        ghost_infer::kokoro_windowed::HalvesPaths::in_dir(&dir)
    }

    /// How a piece is cut into windows when Kokoro renders a window at a time.
    pub fn window_plan(&self) -> ghost_infer::kokoro_windowed::WindowPlan {
        ghost_infer::kokoro_windowed::WindowPlan {
            first: self.tts_window_frames,
            context: self.tts_window_context,
            ..Default::default()
        }
    }

    /// Parakeet's files: `GHOST_STT_DIR`; else Ghost's cache (`scripts/models.sh parakeet`);
    /// for int8 only, else the Hugging Face cache's snapshot. A folder with nothing in it
    /// is still named, so `ghostd check` can say what is missing and where.
    pub fn parakeet_paths(&self) -> ghost_infer::ParakeetPaths {
        let quantization = self.stt_quantization.as_deref();
        if let Some(dir) = &self.stt_dir {
            return ghost_infer::ParakeetPaths::in_dir(Path::new(dir), quantization);
        }
        let cached = ghost_infer::ParakeetPaths::ghost_cache(&self.cache_dir, quantization);
        if cached.encoder.is_file() || quantization != Some("int8") {
            return cached;
        }
        ghost_infer::ParakeetPaths::huggingface_cache().unwrap_or(cached)
    }

    pub fn vad_params(&self) -> ghost_infer::VadParams {
        ghost_infer::VadParams {
            confidence: self.vad_confidence,
            start_secs: self.vad_start_secs,
            stop_secs: self.vad_stop_secs,
            min_volume: self.vad_min_volume,
        }
    }

    /// The VAD while Ghost speaks: the echo canceller passes the operator's voice under
    /// Ghost's audio in 150–350 ms pieces (measured 2026-10-30), which the room's
    /// settings never start on. Quicker to start, and easier on the residual-mixed signal;
    /// what it starts on is judged by the warm-up guard, the echo filter and the barge-in
    /// rule, not by the VAD.
    pub fn vad_params_over_ghost(&self) -> ghost_infer::VadParams {
        ghost_infer::VadParams {
            confidence: self.vad_over_ghost_confidence,
            start_secs: self.vad_over_ghost_start_secs,
            // Longer than the room's: "okay, okay [0.4 s] stop" is one segment, and the
            // interim re-reads see all three words while it is open (2026-10-30, take 7:
            // the segment closed in the gap and the two 150 ms pieces of "stop" started
            // nothing).
            stop_secs: self.vad_over_ghost_stop_secs,
            min_volume: self.vad_over_ghost_min_volume,
        }
    }
}

fn process_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

impl Env for VoiceSettings {
    fn var(&self, key: &str) -> Option<&str> {
        self.llm_env.get(key).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_are_the_pythons() {
        let s = VoiceSettings::default();
        assert_eq!(s.tts_engine, "kokoro");
        assert_eq!(s.tts_voice, "af_heart");
        assert_eq!(s.turn_grace_secs, 1.2);
        assert_eq!(s.turn_confident_grace_secs, 0.35);
        assert_eq!(s.turn_confident_prob, 0.9);
        assert_eq!(s.turn_reopen_started_secs, 0.15);
        assert!(s.prerender(), "Kokoro renders the first clause ahead");
        assert_eq!(s.tts_first_clause_words, 4);
        assert_eq!(s.stt_early_read, crate::stt::EarlyReadMode::Off);
        assert!(
            s.llm_fast_reads,
            "with a fast model set, it makes reads itself"
        );
        assert!(s.turn_speculate);
        assert!(s.turn_reread);
        assert_eq!(s.turn_stop_secs, 1.6);
        assert_eq!(s.turn_unfinished_secs, 3.0);
        assert_eq!(s.turn_sentence_min_prob, 0.1);
        assert_eq!(s.turn_number_secs, 0.5);
        assert_eq!(s.turn_pre_speech_secs, 1.0);
        assert_eq!(s.vad_closing_start_secs, 0.1);
        assert_eq!(s.barge_in_min_words, 2);
        assert_eq!(s.barge_in_interim_secs, 0.3);
        assert_eq!(s.vad_stop_secs, 0.2);
        assert_eq!(s.vad_over_ghost_start_secs, 0.1);
        assert_eq!(s.vad_over_ghost_min_volume, 0.3);
        assert_eq!(s.vad_over_ghost_stop_secs, 0.6);
        assert_eq!(s.hold_timeout_secs, 1800.0);
        assert_eq!(s.compact_trigger_tokens, 24000);
        assert_eq!(s.stt_quantization, None);
        assert!(s.latency);
        assert!(!s.debug_audio);
        assert_eq!(s.llm_thinking, "between_tools");
        assert_eq!(s.llm_max_tokens, 2000);
    }

    #[test]
    fn rendering_ahead_is_kokoros_by_default_and_never_a_paid_voices_unless_set() {
        // ADR 0049: a reply that is withdrawn has rendered a clause for nothing; with a
        // paid voice that would be paid characters.
        assert!(VoiceSettings::default().prerender());
        let cloud = VoiceSettings::from_vars(vars(&[("GHOST_TTS_ENGINE", "elevenlabs")])).unwrap();
        assert!(!cloud.prerender());
        let forced = VoiceSettings::from_vars(vars(&[
            ("GHOST_TTS_ENGINE", "elevenlabs"),
            ("GHOST_TTS_PRERENDER", "true"),
        ]))
        .unwrap();
        assert!(forced.prerender());
        let off = VoiceSettings::from_vars(vars(&[("GHOST_TTS_PRERENDER", "false")])).unwrap();
        assert!(!off.prerender());
    }

    #[test]
    fn the_turns_waits_are_settings_and_the_one_grace_is_reachable() {
        let s = VoiceSettings::from_vars(vars(&[
            ("GHOST_TURN_CONFIDENT_GRACE_SECS", "0.5"),
            ("GHOST_TURN_CONFIDENT_PROB", "0.95"),
            ("GHOST_TURN_REOPEN_STARTED_SECS", "0"),
        ]))
        .unwrap();
        let t = s.turn_timing();
        assert_eq!((t.confident_grace_secs, t.confident_prob), (0.5, 0.95));
        assert!(t.confident(0.96) && !t.confident(0.9));
        assert_eq!(s.turn_reopen_started_secs, 0.0);
        // The grace as it was for every turn: the short one set to it.
        let old =
            VoiceSettings::from_vars(vars(&[("GHOST_TURN_CONFIDENT_GRACE_SECS", "1.2")])).unwrap();
        assert!(!old.turn_timing().confident(1.0));
    }

    #[test]
    fn parakeet_is_read_from_the_folder_named_else_ghosts_cache() {
        let s = VoiceSettings::from_vars(vars(&[("GHOST_CACHE_DIR", "/c")])).unwrap();
        assert_eq!(
            s.parakeet_paths().encoder,
            PathBuf::from("/c/models/parakeet-tdt-0.6b-v3/encoder-model.onnx")
        );
        let s = VoiceSettings::from_vars(vars(&[
            ("GHOST_CACHE_DIR", "/c"),
            ("GHOST_STT_DIR", "/elsewhere"),
            ("GHOST_STT_QUANTIZATION", "int8"),
        ]))
        .unwrap();
        assert_eq!(
            s.parakeet_paths().encoder,
            PathBuf::from("/elsewhere/encoder-model.int8.onnx")
        );
    }

    #[test]
    fn reads_names_and_refuses_bad_ones() {
        let s = VoiceSettings::from_vars(vars(&[
            ("GHOST_TTS_ENGINE", "ElevenLabs"),
            ("ELEVENLABS_API_KEY", "sk_secret_value_0000"),
            ("GHOST_TURN_GRACE_SECS", "0.4"),
            ("GHOST_TURN_SPECULATE", "no"),
            ("GHOST_STT_QUANTIZATION", "int8"),
        ]))
        .unwrap();
        assert_eq!(s.tts_engine, "elevenlabs");
        assert_eq!(s.turn_grace_secs, 0.4);
        assert!(!s.turn_speculate);
        assert_eq!(s.stt_quantization.as_deref(), Some("int8"));
        let shown = format!("{s:?}");
        assert!(!shown.contains("sk_secret"), "{shown}");
        assert_eq!(
            VoiceSettings::from_vars(vars(&[("GHOST_TTS_ENGINE", "polly")])).err(),
            Some(ConfigError::Engine)
        );
        assert_eq!(
            VoiceSettings::from_vars(vars(&[("GHOST_LLM_ENDPOINT", "mars")])).err(),
            Some(ConfigError::Endpoint)
        );
        assert_eq!(
            VoiceSettings::from_vars(vars(&[("GHOST_VAD_STOP_SECS", "soon")])).err(),
            Some(ConfigError::Number("GHOST_VAD_STOP_SECS".into()))
        );
    }
}
