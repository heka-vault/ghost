//! Wake (ADR 0010, ADR 0025): the trained "hey ghost" and "Ghost, …" detectors that score
//! 80 ms frames of the last 2 s (`detector`, from `ghost_voice.wakeword` over
//! `livekit-wakeword`), and the transcript matcher that confirms a hit and splits off the
//! first turn (`matcher`, from `ghost_voice.wake`).
//!
//! A transcript without a phrase is dropped where it is matched; nothing here logs or
//! keeps one (CLAUDE.md).

pub mod detector;
mod difflib;
pub mod matcher;

pub use detector::{
    ADDRESS_MODEL_GLOB, ADDRESS_PHRASE, CHECK_SECS, DEFAULT_MODEL_NAME, DETECTOR_PHRASE, Hit,
    MAX_LISTEN_SECS, MAX_PRE_SECS, OPENERS, OPENING_PRE_SECS, PRE_SECS, REMAINDER_FUZZY,
    RUN_GAP_SECS, SPEECH_PAD_SECS, STEP, WINDOW, WakeDetector, WakeModel, WakeScorer, heard_from,
    model_meaning, pick_hit,
};
pub use matcher::{
    DEFAULT_WAKE_PHRASES, OPENING_LEAD, WakeMatch, match_opening, match_wake, parse_phrases,
};
