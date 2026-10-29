//! Ghost's own voice pipeline (ADR 0039, W7 of heka-vault/ghost#39): the port of
//! `ghost_voice`, with Pipecat's ideas and none of its code. One session is a chain of
//! stages, one tokio task each, passing typed [`Frame`]s over bounded channels; a
//! [`CancellationToken`](tokio_util::sync::CancellationToken) per session and per turn
//! means a barge-in or an end drains every stage without a stale frame reaching the speaker.
//!
//! The order of the stages is the Python `pipeline.py`'s, and the specification:
//! audio in, speech to text (the VAD, Parakeet, the interim re-reads), the hold gate, the
//! echo filter, the user tap, the interceptors, the confirmation processor, the user turn
//! (the barge-in start rule, Smart Turn and the patient stop with the grace), the model,
//! the narration cut, the thinking sound, the assistant tap, text to speech (Kokoro, or
//! ElevenLabs with Kokoro behind it), the thinking-sound cut, audio out.
//!
//! The rules (CLAUDE.md) hold here as everywhere: nothing passive reaches a session, a
//! transcript is logged only through [`debug::audio`] under `GHOST_DEBUG_AUDIO=1`, every
//! change goes through the gate, confirmation is observed in the pipeline's own transcript,
//! and the latency timeline keeps timings, never words.

pub mod config;
pub mod confirmation;
pub mod conversation;
pub mod debug;
pub mod echo;
pub mod elevenlabs;
pub mod frames;
pub mod hold;
pub mod intents;
pub mod intercept;
pub mod latency;
pub mod llm;
pub mod narration;
pub mod perf;
pub mod pipeline;
pub mod prompt;
pub mod rtvi;
pub mod spoken;
pub mod stt;
pub mod tap;
pub mod thinking;
pub mod tools;
pub mod transport;
pub mod tts;
pub mod turns;
pub mod wakeword;

pub use config::VoiceSettings;
pub use frames::{Audio, Direction, Frame, Metric, Observer, Out, Pushed, Stage, assemble};
pub use pipeline::{GhostSession, Models, SessionParams, build_session};
