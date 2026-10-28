//! Ghost core: the write gate, the permissions, the audit, the agents and the apps
//! (ADR 0007, 0011, 0014, 0034). Imports nothing from the other crates.
//!
//! Flow: a source (a connected app's tool, an agent operation) turns the model's arguments
//! into a typed change set -> the gate resolves the operator's permissions for that call ->
//! either the change happens now (`announce`/`quiet`) or it waits for a spoken yes
//! observed in the pipeline's own transcript (`confirm`) -> the steps, the audit log.

pub mod codec;
pub mod confirmation;
pub mod connection;
pub mod conversations;
pub mod events;
#[cfg(any(test, feature = "fakes"))]
pub mod fakes;
pub mod gate;
pub mod levels;
pub mod mock_github;
pub mod models;
pub mod permissions;
pub mod policy;
pub mod redact;
pub mod request;
pub mod scripts_server;
pub mod source;
pub mod store;
pub mod time;

pub use confirmation::{
    ConfirmationClassifier, ConfirmationContext, RegexClassifier, Verdict, classify_confirmation,
    has_negation,
};
pub use events::Events;
pub use gate::{GateError, Submission, WriteGate};
pub use levels::{Autonomy, Risk};
pub use models::{
    Change, ChangeSet, Outcome, OutcomeStatus, Proposal, ProposalStatus, Snapshot, Target,
};
pub use permissions::{Mode, Permissions, Rule};
pub use policy::{AutonomyPolicy, PolicyError};
pub use redact::{also_redact, child_env, password_names, redact_json, redact_text};
pub use request::{asked_for, number_mentions, words};
pub use source::{
    Event, EventKind, EventListener, Handled, Operation, Prepared, ReadTool, Slot, ToolSource,
    TurnInterceptor, UserTurn, Writer,
};
pub use store::Store;
