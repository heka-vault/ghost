//! The phone's way in (ADR 0016): pairing, signed requests, WebRTC signalling.
//!
//! This slice is the foundation: the signing middleware, the pairing store and codes,
//! the networks, and the pairing route's logic as functions the listener mounts. The
//! routes that need the daemon's views and the media (`/hello`, `/audit`, `/waiting`,
//! `/conversations`, `/offer`, `/end`) and the servers on each address arrive with them.

pub mod listener;
pub mod networks;
pub mod pairing;
pub mod signing;

use std::sync::{Arc, LazyLock};
use std::time::Instant;

pub use listener::{
    LocalAddr, PairRequest, PhoneHost, PhoneListener, Reach, Refusal, Signing, pair_link,
    parse_listen, parse_listen_list, sign_requests,
};
pub use networks::Networks;
pub use pairing::{Device, PairingCodes, PhoneStore};
pub use signing::Verifier;

/// Seconds from some origin: `wall` (Unix time, for the clock window) or `monotonic`
/// (for what expires). Tests hand in their own.
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Unix seconds, as `time.time()`.
pub fn wall_clock() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Seconds since the process started, as `time.monotonic()` (its origin is arbitrary too).
pub fn monotonic() -> f64 {
    STARTED.elapsed().as_secs_f64()
}

pub fn wall() -> Clock {
    Arc::new(wall_clock)
}

pub fn mono() -> Clock {
    Arc::new(monotonic)
}
