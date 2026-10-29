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
    ("apps" = "#/components/schemas/AppsEvent"),
)))]
pub enum Event {
    Status(StatusEvent),
    Turn(TurnEvent),
    Pending(PendingEvent),
    Outcome(OutcomeEvent),
    Log(LogEvent),
    Conversation(ConversationEvent),
    Waiting(WaitingEvent),
    Apps(AppsEvent),
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
        ("AppsEvent", AppsEvent::schema()),
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
