//! Ghost's audio engine (ADR 0039; W4 of heka-vault/ghost#39): the echo-cancelled
//! microphone, playback with the thinking sound mixed under speech, the chime, and the
//! music ducking, all on PipeWire through the project's own binding. It replaces the
//! Python daemon's `ghost_voice.transport` (a sounddevice port of Pipecat's PyAudio
//! transport), `ghostd.audio` (the `Ducker` and the chime), `ghost_voice.sounds` and the
//! mixer under `ghost_voice.thinking`.
//!
//! The rules (CLAUDE.md) hold here as everywhere: idle audio stays on the machine, and
//! nothing from the microphone is logged or stored. This crate never writes a sample
//! anywhere but into a ring the daemon reads, and its counters are numbers, never audio.
//! `GHOST_DEBUG_AUDIO` is the daemon's one exception and stays off; nothing here reads it.
//!
//! Shape: [`Capture`] and [`Playback`] each run a PipeWire stream on PipeWire's own thread,
//! with a lock-free ring (`rtrb`) between that thread and tokio; the process callbacks copy
//! samples and allocate nothing. [`Ducker`] lowers the configured music streams' volumes
//! through the registry and the node `Props` param and restores them to their exact previous
//! values, with the state file the Python daemon keeps (`duck.json`) so a crash leaves nothing
//! ducked. [`sounds`] holds the chime and the thinking sound as PCM loaded once.

pub mod capture;
pub mod config;
pub mod duck;
pub mod frame;
pub mod mixer;
pub mod playback;
pub mod pw;
pub mod ring;
pub mod sounds;
pub mod wake;

pub use capture::{Capture, CaptureConfig, CaptureCounters};
pub use config::AudioSettings;
pub use duck::{
    DUCK_STATE, DuckError, DuckedStream, Ducker, default_state_file, restore_from_state,
};
pub use frame::{CAPTURE_RATE, FRAME_MS, Frame, PLAYBACK_RATE, frame_samples};
pub use mixer::{Mixer, mix_sample};
pub use playback::{MixerHandle, Playback, PlaybackConfig, PlaybackCounters};
pub use pw::{BoundNode, GlobalNode, NodeFacts, NodeFormat, NodeState, PwError, Session};
pub use sounds::{CHIME_RATE, SoundError, chime, ensure_thinking_sound, load_wav, thinking_loop};

/// What can go wrong opening or running a stream. Messages name the node or sink the
/// operator configured; none carries audio.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio: {0}")]
    PipeWire(#[from] PwError),
    /// The configured capture node is not there (`ghost-echo-cancel` not running, or the
    /// name in `GHOST_INPUT_DEVICE` is wrong).
    #[error("audio in: no PipeWire node named {name:?} (is ghost-echo-cancel running?)")]
    NoSuchSource { name: String },
    /// The configured sink is not there.
    #[error("audio out: no PipeWire sink named {name:?}")]
    NoSuchSink { name: String },
    #[error("audio: the stream failed: {0}")]
    Stream(String),
    /// The PipeWire thread went away (the stream errored or the daemon was stopped).
    #[error("audio: the stream is closed")]
    Closed,
    #[error("audio: {0}")]
    Sound(#[from] SoundError),
}
