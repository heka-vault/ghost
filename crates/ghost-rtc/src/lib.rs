//! The phone's WebRTC audio (ADR 0016, ADR 0020; W8 of heka-vault/ghost#39): one [`Media`]
//! trait the daemon talks to, and three stacks behind Cargo features, one chosen by
//! measurement (`README.md`). It replaces Pipecat's `SmallWebRTCConnection` and
//! `SmallWebRTCTransport` and the Python daemon's `ghost_voice.rtvi`.
//!
//! What the trait carries: [`Media::answer`] (the phone's SDP offer in, the answer out;
//! `restart` is the protocol's `restart_pc`, a fresh peer), inbound 20 ms PCM frames at
//! 16 kHz mono s16 (Opus decoded at 48 kHz and resampled), an outbound [`Sink`] of 24 kHz
//! frames (resampled to 48 kHz and Opus-encoded), the data channel narrowed to the RTVI
//! subset (`client-ready` and `disconnect-bot` in; `bot-ready`, the speaking events and
//! `server-message` out), the [`Link`] state (`Connected` / `Gone`), the DTLS fingerprint
//! the answer carries (the phone pins it through the signed answer), and `close`.
//!
//! The rules hold here (CLAUDE.md): nothing from the phone's audio is logged or stored by
//! this crate, and a data-channel message outside the subset is dropped without its contents
//! reaching a log. No ICE servers, ever: the phone reaches the desk over the LAN or the
//! operator's own tunnel (ADR 0020). The listener that signs the exchange is not here; this
//! crate answers an offer it is handed.

pub mod backend;
pub mod codec;
pub mod frame;
pub mod marker;
pub mod media;
pub mod probe;
pub mod resample;
pub mod rtvi;
pub mod sdp;
pub mod stats;

pub use frame::{FRAME_MS, IN_RATE, OUT_RATE, PcmFrame, WIRE_RATE, frame_samples};
pub use media::{CallStats, Frames, Link, Media, MediaConfig, Messages, Sink};
pub use rtvi::{ClientMessage, Outbound};
pub use sdp::Fingerprint;

#[cfg(feature = "libwebrtc")]
pub use backend::libwebrtc::LibwebrtcMedia;
#[cfg(feature = "str0m")]
pub use backend::str0m::Str0mMedia;
#[cfg(feature = "webrtc")]
pub use backend::webrtc::WebrtcMedia;

/// What can go wrong. Messages name the step, never the offer's text or the audio.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// The offer does not have the shape the phone sends (the listener answers 400
    /// `bad_offer`). The reason is ours, never a line of the offer.
    #[error("bad offer: {0}")]
    BadOffer(String),
    #[error("bad candidate")]
    BadCandidate,
    #[error("no live peer")]
    NoPeer,
    #[error("the peer is gone")]
    Gone,
    #[error("opus: {0}")]
    Opus(String),
    #[error("resample: {0}")]
    Resample(String),
    #[error("rtc: {0}")]
    Rtc(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Fifty messages are held for the data channel to open; past that they are dropped,
    /// as the Python did.
    #[error("the data channel's outbox is full")]
    OutboxFull,
    #[error("the sink is closed")]
    SinkClosed,
    #[error(
        "no media stack was compiled in (enable one of the str0m, webrtc or libwebrtc features)"
    )]
    NoBackend,
}

/// The name of the stack [`new_media`] builds: the first feature enabled, in the order
/// `str0m`, `webrtc`, `libwebrtc`.
pub const BACKEND: &str = if cfg!(feature = "str0m") {
    "str0m"
} else if cfg!(feature = "webrtc") {
    "webrtc"
} else if cfg!(feature = "libwebrtc") {
    "libwebrtc"
} else {
    "none"
};

/// The media stack the daemon runs: the first enabled feature. Must be called inside a tokio
/// runtime.
#[allow(unreachable_code, unused_variables)]
pub fn new_media(config: MediaConfig) -> Result<Box<dyn Media>, MediaError> {
    #[cfg(feature = "str0m")]
    return Ok(Box::new(Str0mMedia::new(config)?));
    #[cfg(feature = "webrtc")]
    return Ok(Box::new(WebrtcMedia::new(config)?));
    #[cfg(feature = "libwebrtc")]
    return Ok(Box::new(LibwebrtcMedia::new(config)?));
    Err(MediaError::NoBackend)
}
