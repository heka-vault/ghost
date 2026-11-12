//! The OpenAPI document (ADR 0013): what `packages/api/openapi.json` holds, built from the
//! routes and the types in `models`, operation for operation as FastAPI wrote it, so
//! `packages/client-ts` regenerates to the same client surface.
//!
//! The event payloads are one Rust enum (`models::Event`, tagged by `type`); the document
//! names each variant as the Python did (`StatusEvent`, …), so the shapes here are the
//! document's names for them, not a second wire type.

use chrono::{DateTime, Utc};
use serde_json::Value;
use utoipa::openapi::schema::{AdditionalProperties, ObjectBuilder};
use utoipa::openapi::{Info, OpenApi, RefOr, Schema};
use utoipa::{PartialSchema, ToSchema};

use crate::error::{HTTPValidationError, ValidationError};
use crate::models::{OutcomeVia, OutcomeView, PendingProposal, Status, Turn};

#[derive(ToSchema)]
pub struct StatusEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "status")]
    pub r#type: String,
    pub status: Status,
}

#[derive(ToSchema)]
pub struct TurnEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "turn")]
    pub r#type: String,
    pub turn: Turn,
}

/// `pending` None when it was resolved or cancelled.
#[derive(ToSchema)]
pub struct PendingEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "pending")]
    pub r#type: String,
    #[schema(required)]
    pub pending: Option<PendingProposal>,
}

#[derive(ToSchema)]
pub struct OutcomeEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "outcome")]
    pub r#type: String,
    pub outcome: OutcomeView,
    #[schema(inline)]
    pub via: OutcomeVia,
}

#[derive(ToSchema)]
pub struct LogEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "log")]
    pub r#type: String,
    pub level: String,
    pub message: String,
    pub at: DateTime<Utc>,
}

/// The session moved to another conversation, or one was created, changed or removed.
#[derive(ToSchema)]
pub struct ConversationEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "conversation")]
    pub r#type: String,
    #[schema(required)]
    pub active: Option<String>,
    #[serde(default)]
    pub changed: Option<String>,
}

/// The waiting list changed: read `GET /waiting` for it.
#[derive(ToSchema)]
pub struct WaitingEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "waiting")]
    pub r#type: String,
    pub count: u64,
}

/// A question was asked, answered, declined, expired or taken back (ADR 0051).
#[derive(ToSchema)]
pub struct QuestionEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "question")]
    pub r#type: String,
    pub question: crate::models::QuestionView,
}

/// An app was connected, disconnected or changed state: read `GET /apps`. With
/// `sign_in_url`, the app is waiting for the operator to sign in there; the desktop opens
/// it in the browser.
#[derive(ToSchema)]
pub struct AppsEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "apps")]
    pub r#type: String,
    #[serde(default)]
    pub changed: Option<String>,
    #[serde(default)]
    pub sign_in_url: Option<String>,
}

/// How loud Ghost's own voice is at the speaker, 0.0 to 1.0: the RMS of 50 ms of what is
/// playing, in dBFS, mapped linearly from -50 dBFS (0.0) to 0 dBFS (1.0). About twenty a
/// second while Ghost speaks, timed to when the audio is heard, then one `0.0` when the
/// voice stops; nothing at all while Ghost is silent. Ghost's output only: never the
/// microphone, never passive audio.
#[derive(ToSchema)]
pub struct VoiceEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "voice")]
    pub r#type: String,
    pub level: f32,
}

/// What the live session is doing (ADR 0055): names, ids and a duration, never words.
#[derive(ToSchema)]
pub struct ActivityEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "activity")]
    pub r#type: String,
    pub activity: crate::models::ActivityView,
}

/// The operator's spoken turn as it closed, with the words the model was given (ADR 0058).
#[derive(ToSchema)]
pub struct HeardEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "heard")]
    pub r#type: String,
    pub heard: crate::models::HeardView,
}

/// Which words of Ghost's reply are heard now (ADR 0058).
#[derive(ToSchema)]
pub struct SayingEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "saying")]
    pub r#type: String,
    pub saying: crate::models::SayingView,
}

/// The Hearing test's recorder (ADR 0042), about twenty a second while it is open: what
/// it is doing, the microphone's level 0.0 to 1.0 (the RMS of the last 50 ms, -50 dBFS to
/// 0 dBFS) and whether the VAD hears speech; then one with the state `closed`. Only while
/// the operator has the recorder open; never passive audio.
#[derive(ToSchema)]
pub struct HearEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "hear")]
    pub r#type: String,
    pub hear: crate::hear::HearView,
    pub level: f32,
    pub speaking: bool,
}

/// The operator asked by voice to see Ghost's window (ADR 0059): the desktop brings its main
/// window to the workspace they are on and to the front. Inside a session only.
#[derive(ToSchema)]
pub struct ShowAppEvent {
    #[serde(default)]
    #[schema(value_type = String, default = "show_app")]
    pub r#type: String,
    pub session_id: String,
}

/// What the event stream carries, discriminated by `type`.
#[derive(ToSchema)]
#[serde(untagged)]
#[schema(discriminator(property_name = "type", mapping(
    ("status" = "#/components/schemas/StatusEvent"),
    ("turn" = "#/components/schemas/TurnEvent"),
    ("pending" = "#/components/schemas/PendingEvent"),
    ("outcome" = "#/components/schemas/OutcomeEvent"),
    ("log" = "#/components/schemas/LogEvent"),
    ("conversation" = "#/components/schemas/ConversationEvent"),
    ("waiting" = "#/components/schemas/WaitingEvent"),
    ("question" = "#/components/schemas/QuestionEvent"),
    ("apps" = "#/components/schemas/AppsEvent"),
    ("voice" = "#/components/schemas/VoiceEvent"),
    ("activity" = "#/components/schemas/ActivityEvent"),
    ("heard" = "#/components/schemas/HeardEvent"),
    ("saying" = "#/components/schemas/SayingEvent"),
    ("hear" = "#/components/schemas/HearEvent"),
    ("show_app" = "#/components/schemas/ShowAppEvent"),
)))]
pub enum Event {
    Status(StatusEvent),
    Turn(TurnEvent),
    Pending(PendingEvent),
    Outcome(OutcomeEvent),
    Log(LogEvent),
    Conversation(ConversationEvent),
    Waiting(WaitingEvent),
    Question(QuestionEvent),
    Apps(AppsEvent),
    Voice(VoiceEvent),
    Activity(ActivityEvent),
    Heard(HeardEvent),
    Saying(SayingEvent),
    Hear(HearEvent),
    ShowApp(ShowAppEvent),
}

/// `POST /conversations/{slug}/notes` takes a free object with `text` in it.
pub struct NoteBody;

impl PartialSchema for NoteBody {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .title(Some("Body"))
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .build()
            .into()
    }
}

impl ToSchema for NoteBody {}

/// The document's title, version and words, and the schemas no route refers to by type.
pub fn finish(mut api: OpenApi) -> OpenApi {
    api.info = Info::new(crate::app::TITLE, crate::models::API_VERSION);
    api.info.description = Some(crate::app::DESCRIPTION.to_string());
    let components = api.components.get_or_insert_default();
    for (name, schema) in [
        ("HTTPValidationError", HTTPValidationError::schema()),
        ("ValidationError", ValidationError::schema()),
        ("StatusEvent", StatusEvent::schema()),
        ("TurnEvent", TurnEvent::schema()),
        ("PendingEvent", PendingEvent::schema()),
        ("OutcomeEvent", OutcomeEvent::schema()),
        ("LogEvent", LogEvent::schema()),
        ("ConversationEvent", ConversationEvent::schema()),
        ("WaitingEvent", WaitingEvent::schema()),
        ("QuestionEvent", QuestionEvent::schema()),
        ("AppsEvent", AppsEvent::schema()),
        ("VoiceEvent", VoiceEvent::schema()),
        ("ActivityEvent", ActivityEvent::schema()),
        ("HeardEvent", HeardEvent::schema()),
        ("SayingEvent", SayingEvent::schema()),
        ("HeardView", crate::models::HeardView::schema()),
        ("SayingView", crate::models::SayingView::schema()),
        ("HearEvent", HearEvent::schema()),
        ("HearView", crate::hear::HearView::schema()),
        ("ShowAppEvent", ShowAppEvent::schema()),
        ("ActivityView", crate::models::ActivityView::schema()),
    ] {
        components.schemas.entry(name.to_string()).or_insert(schema);
    }
    api
}

/// The document as JSON, its keys sorted at every level as `json.dump(sort_keys=True)`
/// wrote the committed file.
pub fn to_json(api: &OpenApi) -> Value {
    sorted(serde_json::to_value(api).unwrap_or(Value::Null))
}

fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut pairs: Vec<(String, Value)> = m.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = serde_json::Map::new();
            for (k, v) in pairs {
                out.insert(k, sorted(v));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// The document of a daemon with nothing behind it: what `ghost-api-export` prints.
pub fn document() -> Value {
    use std::sync::Arc;

    use ghost_core::gate::WriteGate;
    use ghost_core::store::Store;

    use crate::controller::Unavailable;
    use crate::deps::Deps;
    use crate::state::DaemonState;

    let store = Arc::new(Store::in_memory().expect("an in-memory store"));
    let gate = Arc::new(WriteGate::new(store.clone()));
    let state = Arc::new(DaemonState::new("schema", "file"));
    let deps = Arc::new(Deps::minimal(state, Arc::new(Unavailable), store, gate));
    let (_, api) = crate::app::create_app(deps);
    to_json(&api)
}
