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
pub const LLM_FIELDS: [(&str, &str); 10] = [
    ("llm_provider", "GHOST_LLM_PROVIDER"),
    ("llm_endpoint", "GHOST_LLM_ENDPOINT"),
    ("llm_model", "GHOST_LLM_MODEL"),
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
}

/// The voice package's settings. Defaults are the Python's.
#[derive(Clone)]
pub struct VoiceSettings {
    // The model connection (ADR 0037), as the environment names it.
    pub llm_env: HashMap<String, String>,
    pub llm_thinking: String,
    pub llm_max_tokens: u32,

    // STT
    pub stt_model: String,
    pub stt_quantization: Option<String>,
    pub stt_threads: usize,
    /// `GHOST_STT_DIR`: where Parakeet's files are; unset, the Hugging Face cache.
    pub stt_dir: Option<PathBuf>,

    // TTS
    pub tts_voice: String,
    pub tts_speed: f64,
    pub tts_model_path: Option<PathBuf>,
    pub tts_voices_path: Option<PathBuf>,
    /// Speak from the first clause instead of the whole sentence (`ghost_infer::clauses`).
    pub tts_clauses: bool,
    pub tts_clause_min_words: usize,
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
    pub wake_vad_stop_secs: f32,
    pub barge_in_min_words: usize,
    pub barge_in_interim_secs: f64,
    pub echo_window_secs: f64,
    pub turn_grace_secs: f64,
    pub turn_speculate: bool,
    pub turn_stop_secs: f64,
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
            stt_model: string("GHOST_STT_MODEL", "nemo-parakeet-tdt-0.6b-v3"),
            stt_quantization: match get("GHOST_STT_QUANTIZATION") {
                None => Some("int8".into()),
                Some(v) if v.trim().is_empty() || v.trim() == "none" => None,
                Some(v) => Some(v.trim().to_string()),
            },
            stt_threads: usize_or("GHOST_STT_THREADS", 4)?,
            stt_dir: path("GHOST_STT_DIR"),
            tts_voice: string("GHOST_TTS_VOICE", "af_heart"),
            tts_speed: f64_or("GHOST_TTS_SPEED", 1.0)?,
            tts_model_path: path("GHOST_TTS_MODEL_PATH"),
            tts_voices_path: path("GHOST_TTS_VOICES_PATH"),
            tts_clauses: flag("GHOST_TTS_CLAUSES", true),
            tts_clause_min_words: usize_or("GHOST_TTS_CLAUSE_MIN_WORDS", 4)?,
            tts_engine,
            elevenlabs_api_key: Zeroizing::new(string("ELEVENLABS_API_KEY", "")),
            elevenlabs_voice: string("GHOST_ELEVENLABS_VOICE", ""),
            elevenlabs_voices: string("GHOST_ELEVENLABS_VOICES", ""),
            elevenlabs_model: string("GHOST_ELEVENLABS_MODEL", "eleven_flash_v2_5"),
            vad_confidence: f64_or("GHOST_VAD_CONFIDENCE", 0.7)? as f32,
            vad_start_secs: f64_or("GHOST_VAD_START_SECS", 0.2)? as f32,
            vad_stop_secs: f64_or("GHOST_VAD_STOP_SECS", 0.2)? as f32,
            vad_min_volume: f64_or("GHOST_VAD_MIN_VOLUME", 0.6)? as f32,
            wake_vad_stop_secs: f64_or("GHOST_WAKE_VAD_STOP_SECS", 0.5)? as f32,
            barge_in_min_words: usize_or("GHOST_BARGE_IN_MIN_WORDS", 2)?,
            barge_in_interim_secs: f64_or("GHOST_BARGE_IN_INTERIM_SECS", 0.3)?,
            echo_window_secs: f64_or("GHOST_ECHO_WINDOW_SECS", 3.0)?,
            turn_grace_secs: f64_or("GHOST_TURN_GRACE_SECS", 0.8)?,
            turn_speculate: flag("GHOST_TURN_SPECULATE", true),
            turn_stop_secs: f64_or("GHOST_TURN_STOP_SECS", 1.6)?,
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
            thinking_sound: flag("GHOST_THINKING_SOUND", true),
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

    /// Parakeet's files: `GHOST_STT_DIR`, else the Hugging Face cache.
    pub fn parakeet_paths(&self) -> Option<ghost_infer::ParakeetPaths> {
        match &self.stt_dir {
            Some(dir) => Some(ghost_infer::ParakeetPaths::in_dir(
                Path::new(dir),
                self.stt_quantization.as_deref(),
            )),
            None => ghost_infer::ParakeetPaths::huggingface_cache(),
        }
    }

    pub fn vad_params(&self) -> ghost_infer::VadParams {
        ghost_infer::VadParams {
            confidence: self.vad_confidence,
            start_secs: self.vad_start_secs,
            stop_secs: self.vad_stop_secs,
            min_volume: self.vad_min_volume,
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
        assert_eq!(s.turn_grace_secs, 0.8);
        assert!(s.turn_speculate);
        assert_eq!(s.turn_stop_secs, 1.6);
        assert_eq!(s.barge_in_min_words, 2);
        assert_eq!(s.barge_in_interim_secs, 0.3);
        assert_eq!(s.vad_stop_secs, 0.2);
        assert_eq!(s.hold_timeout_secs, 1800.0);
        assert_eq!(s.compact_trigger_tokens, 24000);
        assert_eq!(s.stt_quantization.as_deref(), Some("int8"));
        assert!(s.latency);
        assert!(!s.debug_audio);
        assert_eq!(s.llm_thinking, "between_tools");
        assert_eq!(s.llm_max_tokens, 2000);
    }

    #[test]
    fn reads_names_and_refuses_bad_ones() {
        let s = VoiceSettings::from_vars(vars(&[
            ("GHOST_TTS_ENGINE", "ElevenLabs"),
            ("ELEVENLABS_API_KEY", "sk_secret_value_0000"),
            ("GHOST_TURN_GRACE_SECS", "0.4"),
            ("GHOST_TURN_SPECULATE", "no"),
            ("GHOST_STT_QUANTIZATION", "none"),
        ]))
        .unwrap();
        assert_eq!(s.tts_engine, "elevenlabs");
        assert_eq!(s.turn_grace_secs, 0.4);
        assert!(!s.turn_speculate);
        assert_eq!(s.stt_quantization, None);
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
