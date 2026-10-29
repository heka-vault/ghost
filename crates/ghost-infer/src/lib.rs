//! Ghost's local models on ONNX Runtime (ADR 0004, ADR 0025, ADR 0039): Parakeet TDT 0.6B v3
//! for speech to text, Silero for voice activity, Smart Turn v3 for the end of a turn, the
//! trained wake detectors with the transcript matcher behind them, and Kokoro for speech.
//!
//! The Python daemon is the oracle. Each module ports the Python that drove the same model
//! file (`onnx_asr`, Pipecat's VAD and turn analyzers, `livekit-wakeword`, `kokoro-onnx`
//! with `phonemizer`), with the session configured as the Python configured it (CPU, the
//! same thread counts), and `tests/parity.rs` compares the two on the operator's recorded
//! takes and the wake-review corpus, which `scripts/fixtures/record.py` turns into fixtures.
//!
//! Nothing here reads a microphone, keeps a transcript or writes a log line with words in
//! it: a model takes samples and returns text, a score or audio, and what the pipeline does
//! with a passive transcript is its rule (CLAUDE.md, "idle audio stays on the machine").

pub mod audio;
pub mod clauses;
pub mod espeak;
pub mod kokoro;
pub mod loudness;
pub mod mel;
pub mod models;
pub mod npy;
pub mod parakeet;
pub mod phonemize;
pub mod runtime;
pub mod session;
pub mod silero;
pub mod smart_turn;
pub mod wake;

pub use kokoro::{Kokoro, KokoroPaths};
pub use parakeet::{Parakeet, ParakeetPaths};
pub use session::{SessionConfig, environment};
pub use silero::{Silero, VadAnalyzer, VadParams, VadState};
pub use smart_turn::{SmartTurn, TurnVerdict};
pub use wake::{
    Hit, WakeDetector, WakeMatch, WakeModel, WakeScorer, match_opening, match_wake, parse_phrases,
};

/// What a model can fail with. Messages name files and shapes, never audio or words.
#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("onnxruntime: {0}")]
    Ort(#[from] ort::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// A model file that doesn't fit what the port expects (an input, a shape, a vocabulary).
    #[error("{0}")]
    Model(String),
    /// Audio that isn't 16 kHz mono int16, or a chunk of the wrong length.
    #[error("{0}")]
    Audio(String),
    #[error("espeak-ng: {0}")]
    Espeak(String),
    /// ONNX Runtime's library is not where `runtime` looks for it.
    #[error("{0}")]
    Runtime(String),
}

pub type Result<T> = std::result::Result<T, InferError>;
