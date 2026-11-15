//! Dictation (ADR 0077): the operator starts it, speaks, stops it, and Ghost's own Parakeet
//! reads what was said; in this slice the text ends on the clipboard.
//!
//! These routes never carry the words. Every answer is a state, an outcome and its reason,
//! each from a closed list, and numbers (seconds, characters, milliseconds): the types here
//! have no string field, so that holds by construction, and `dictation_carries_no_words`
//! holds it by test. The sentence the CLI prints for an outcome is Ghost's own, chosen by
//! [`DictationResult::sentence`] from the kinds.
//!
//! The operator's routes only (ADR 0013): the phone listener, the listener for outside tools
//! and Ghost's own app serve none of this, and no event carries it.

use async_trait::async_trait;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult};

/// Where a dictation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DictationState {
    /// Nothing is recording or being read.
    Idle,
    /// The microphone is open for dictation; the wake is paused.
    Recording,
    /// The microphone is closed; Parakeet is reading what was said.
    Reading,
}

/// How a dictation, or a request about one, ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DictationOutcome {
    /// The text is on the clipboard.
    OnClipboard,
    /// Parakeet read nothing, or nothing was left after the fillers.
    NothingHeard,
    /// Parakeet read words, but the clipboard could not be set; the words are gone.
    NotOnClipboard,
    /// Stopped before the text reached the clipboard; nothing was kept.
    Cancelled,
    /// Not started, or the request did not apply.
    Refused,
}

/// Why, when the outcome needs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DictationReason {
    /// Not listening is on (ADR 0057): no capture is opened.
    NotListening,
    /// A session (or a phone call) has the microphone, or the passive loop is not waiting.
    Busy,
    /// The Hearing test has the microphone.
    HearingTest,
    /// Parakeet is not loaded yet, or not downloaded.
    NoSpeechModel,
    /// PipeWire shows no microphone at all.
    NoMicrophone,
    /// The microphone could not be opened, or gave no sound.
    MicrophoneFailed,
    /// A dictation is already recording.
    AlreadyDictating,
    /// A dictation is being read; wait for it.
    StillReading,
    /// `stop` or `cancel` with nothing to stop.
    NotDictating,
    /// It reached the length limit and stopped by itself; what was said is read.
    Limit,
    /// The microphone went away while recording; what was said is read.
    MicrophoneEnded,
    /// The operator cancelled it.
    Operator,
    /// Not listening was turned on while it ran.
    ListeningTurnedOff,
    /// A session opened while it ran.
    SessionOpened,
    /// Ghost is stopping.
    Stopping,
    /// No X display to own the clipboard on (no `DISPLAY`, or no libX11).
    NoDisplay,
    /// The display would not let Ghost own the clipboard.
    ClipboardFailed,
}

/// What a dictation, or a request, ended with: kinds and numbers, never words.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DictationResult {
    pub outcome: DictationOutcome,
    #[schema(required)]
    pub reason: Option<DictationReason>,
    /// Seconds of audio recorded.
    pub seconds: f64,
    /// Characters put on the clipboard (after the cleanup).
    pub chars: usize,
    /// From the start request to the first frame captured, in milliseconds: how much of a
    /// first word could be lost.
    #[schema(required)]
    pub first_frame_ms: Option<u64>,
    /// Waiting for the shared recognizer behind a read in flight, in milliseconds.
    #[schema(required)]
    pub queue_ms: Option<u64>,
    /// Parakeet's read itself, in milliseconds.
    #[schema(required)]
    pub read_ms: Option<u64>,
}

impl DictationResult {
    pub fn of(outcome: DictationOutcome, reason: Option<DictationReason>) -> Self {
        Self {
            outcome,
            reason,
            seconds: 0.0,
            chars: 0,
            first_frame_ms: None,
            queue_ms: None,
            read_ms: None,
        }
    }

    pub fn refused(reason: DictationReason) -> Self {
        Self::of(DictationOutcome::Refused, Some(reason))
    }

    pub fn cancelled(reason: DictationReason) -> Self {
        Self::of(DictationOutcome::Cancelled, Some(reason))
    }

    /// Ghost's one plain sentence for it: the CLI prints it and the notification shows it.
    /// Fixed words and the character count, never the dictated text.
    pub fn sentence(&self) -> String {
        use DictationOutcome as O;
        use DictationReason as R;
        match (self.outcome, self.reason) {
            (O::OnClipboard, Some(R::Limit)) => format!(
                "Stopped at the limit. On your clipboard: {} characters. Press Ctrl+V.",
                self.chars
            ),
            (O::OnClipboard, _) => format!(
                "On your clipboard: {} characters. Press Ctrl+V.",
                self.chars
            ),
            (O::NothingHeard, _) => "Didn't catch anything.".into(),
            (O::NotOnClipboard, Some(R::NoDisplay)) => {
                "Ghost couldn't reach the clipboard: there is no display. Nothing was kept.".into()
            }
            (O::NotOnClipboard, _) => {
                "Ghost couldn't put it on the clipboard. Nothing was kept.".into()
            }
            (O::Cancelled, Some(R::ListeningTurnedOff)) => {
                "Dictation stopped: Ghost isn't listening now. Nothing was kept.".into()
            }
            (O::Cancelled, Some(R::SessionOpened)) => {
                "Dictation stopped for the conversation. Nothing was kept.".into()
            }
            (O::Cancelled, Some(R::Stopping)) => {
                "Dictation stopped: Ghost is stopping. Nothing was kept.".into()
            }
            (O::Cancelled, _) => "Dictation cancelled. Nothing was kept.".into(),
            (O::Refused, Some(r)) => refusal_sentence(r).into(),
            (O::Refused, None) => "Dictation didn't start.".into(),
        }
    }
}

fn refusal_sentence(r: DictationReason) -> &'static str {
    use DictationReason as R;
    match r {
        R::NotListening => "Ghost isn't listening. Turn listening on to dictate.",
        R::Busy => "Ghost is in a conversation. Dictate when it ends.",
        R::HearingTest => "The Hearing test has the microphone. Finish it first.",
        R::NoSpeechModel => "Dictation needs Ghost's speech model, which isn't ready yet.",
        R::NoMicrophone => "Ghost can't find a microphone.",
        R::MicrophoneFailed => "Ghost couldn't open the microphone.",
        R::AlreadyDictating => "Already dictating. Stop it first.",
        R::StillReading => "Still writing the last one down. Try again in a moment.",
        R::NotDictating => "Not dictating.",
        _ => "Dictation didn't start.",
    }
}

/// The state now, and a result: what this request ended with, or for `GET` the last one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DictationView {
    pub state: DictationState,
    /// The hard limit on one dictation's recording, in seconds.
    pub limit_secs: f64,
    /// Seconds since recording started, while it records.
    #[schema(required)]
    pub recording_secs: Option<f64>,
    /// While recording: from the start request to the first frame, in milliseconds.
    #[schema(required)]
    pub first_frame_ms: Option<u64>,
    #[schema(required)]
    pub result: Option<DictationResult>,
}

/// What the daemon does for dictation (`ghostd::dictation`).
#[async_trait]
pub trait DictationPort: Send + Sync {
    fn view(&self) -> DictationView;
    /// Open the microphone and record, or refuse with a reason.
    async fn start(&self) -> DictationView;
    /// Stop recording and read; answers once the text is on the clipboard (or not).
    async fn stop(&self) -> DictationView;
    /// Start when idle, stop when recording, refuse while reading.
    async fn toggle(&self) -> DictationView;
    /// Drop it, recorded or being read; nothing reaches the clipboard.
    async fn cancel(&self) -> DictationView;
}

fn port(deps: &SharedDeps) -> ApiResult<&dyn DictationPort> {
    deps.dictation
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("dictation is not available in this daemon"))
}

/// The state and the last result. Never the words.
#[utoipa::path(get, path = "/dictation", operation_id = "dictation_get",
    tag = "dictation", summary = "Dictation's state",
    responses((status = 200, description = "Successful Response", body = DictationView)))]
async fn dictation_get(State(deps): State<SharedDeps>) -> ApiResult<Json<DictationView>> {
    Ok(Json(port(&deps)?.view()))
}

/// Start dictating: the microphone opens and the wake pauses. Refused while not listening,
/// in a session, without the speech model or a microphone.
#[utoipa::path(post, path = "/dictation/start", operation_id = "dictation_start",
    tag = "dictation", summary = "Start dictating",
    responses((status = 200, description = "Successful Response", body = DictationView)))]
async fn dictation_start(State(deps): State<SharedDeps>) -> ApiResult<Json<DictationView>> {
    Ok(Json(port(&deps)?.start().await))
}

/// Stop dictating and read it; answers when the text is on the clipboard, with its length.
#[utoipa::path(post, path = "/dictation/stop", operation_id = "dictation_stop",
    tag = "dictation", summary = "Stop dictating",
    responses((status = 200, description = "Successful Response", body = DictationView)))]
async fn dictation_stop(State(deps): State<SharedDeps>) -> ApiResult<Json<DictationView>> {
    Ok(Json(port(&deps)?.stop().await))
}

/// Start when idle, stop when recording: for one keyboard shortcut.
#[utoipa::path(post, path = "/dictation/toggle", operation_id = "dictation_toggle",
    tag = "dictation", summary = "Start or stop dictating",
    responses((status = 200, description = "Successful Response", body = DictationView)))]
async fn dictation_toggle(State(deps): State<SharedDeps>) -> ApiResult<Json<DictationView>> {
    Ok(Json(port(&deps)?.toggle().await))
}

/// Drop the dictation; nothing reaches the clipboard.
#[utoipa::path(post, path = "/dictation/cancel", operation_id = "dictation_cancel",
    tag = "dictation", summary = "Cancel dictating",
    responses((status = 200, description = "Successful Response", body = DictationView)))]
async fn dictation_cancel(State(deps): State<SharedDeps>) -> ApiResult<Json<DictationView>> {
    Ok(Json(port(&deps)?.cancel().await))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(dictation_get))
        .routes(routes!(dictation_start))
        .routes(routes!(dictation_stop))
        .routes(routes!(dictation_toggle))
        .routes(routes!(dictation_cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const OUTCOMES: [DictationOutcome; 5] = [
        DictationOutcome::OnClipboard,
        DictationOutcome::NothingHeard,
        DictationOutcome::NotOnClipboard,
        DictationOutcome::Cancelled,
        DictationOutcome::Refused,
    ];
    const REASONS: [DictationReason; 17] = [
        DictationReason::NotListening,
        DictationReason::Busy,
        DictationReason::HearingTest,
        DictationReason::NoSpeechModel,
        DictationReason::NoMicrophone,
        DictationReason::MicrophoneFailed,
        DictationReason::AlreadyDictating,
        DictationReason::StillReading,
        DictationReason::NotDictating,
        DictationReason::Limit,
        DictationReason::MicrophoneEnded,
        DictationReason::Operator,
        DictationReason::ListeningTurnedOff,
        DictationReason::SessionOpened,
        DictationReason::Stopping,
        DictationReason::NoDisplay,
        DictationReason::ClipboardFailed,
    ];

    fn kinds() -> Vec<String> {
        let mut out = Vec::new();
        for o in OUTCOMES {
            out.push(
                serde_json::to_value(o)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        for r in REASONS {
            out.push(
                serde_json::to_value(r)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        for s in [
            DictationState::Idle,
            DictationState::Recording,
            DictationState::Reading,
        ] {
            out.push(
                serde_json::to_value(s)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        out
    }

    /// Every string anywhere in `v`.
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            Value::Object(m) => m.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }

    #[test]
    fn dictation_carries_no_words() {
        // ADR 0077: what crosses the control API is kinds and numbers. Every view these types
        // can make serialises to strings from the closed lists alone, and the document's
        // schemas for them have no free string: an enum, a number or a boolean.
        let kinds = kinds();
        for o in OUTCOMES {
            for r in REASONS.iter().copied().map(Some).chain([None]) {
                let view = DictationView {
                    state: DictationState::Reading,
                    limit_secs: 30.0,
                    recording_secs: Some(1.0),
                    first_frame_ms: Some(40),
                    result: Some(DictationResult {
                        seconds: 3.0,
                        chars: 12,
                        first_frame_ms: Some(1),
                        queue_ms: Some(2),
                        read_ms: Some(3),
                        ..DictationResult::of(o, r)
                    }),
                };
                let mut found = Vec::new();
                strings(&serde_json::to_value(view).unwrap(), &mut found);
                for s in found {
                    assert!(kinds.contains(&s), "a free string in a dictation view: {s}");
                }
            }
        }
        let doc = serde_json::to_value(crate::openapi::document()).unwrap();
        let schemas = &doc["components"]["schemas"];
        for name in [
            "DictationView",
            "DictationResult",
            "DictationState",
            "DictationOutcome",
            "DictationReason",
        ] {
            let schema = &schemas[name];
            assert!(!schema.is_null(), "{name} is in the document");
            free_strings(name, schema);
        }
    }

    /// A `"type": "string"` without an `enum` anywhere in the schema is a free string.
    fn free_strings(name: &str, v: &Value) {
        match v {
            Value::Object(m) => {
                let is_string = match m.get("type") {
                    Some(Value::String(t)) => t == "string",
                    Some(Value::Array(a)) => a.iter().any(|t| t == "string"),
                    _ => false,
                };
                assert!(
                    !is_string || m.contains_key("enum"),
                    "{name} has a free string: {v}"
                );
                m.values().for_each(|x| free_strings(name, x));
            }
            Value::Array(a) => a.iter().for_each(|x| free_strings(name, x)),
            _ => {}
        }
    }

    #[test]
    fn every_result_has_a_sentence_and_none_is_empty() {
        for o in OUTCOMES {
            for r in REASONS.iter().copied().map(Some).chain([None]) {
                let s = DictationResult::of(o, r).sentence();
                assert!(!s.is_empty() && s.ends_with('.'), "{o:?} {r:?}: {s}");
            }
        }
        let on = DictationResult {
            chars: 42,
            ..DictationResult::of(DictationOutcome::OnClipboard, None)
        };
        assert_eq!(
            on.sentence(),
            "On your clipboard: 42 characters. Press Ctrl+V."
        );
    }
}
