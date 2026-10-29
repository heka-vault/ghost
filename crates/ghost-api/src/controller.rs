//! What the daemon lets the API do (the Python `Controller` protocol, and the `Daemon`
//! methods the phone listener calls). Every method is safe to call at any time. The
//! daemon implements it (W8/W9 of heka-vault/ghost#39); `Unavailable` is the double a
//! daemon with nothing behind it (the schema export, the first start) answers with.

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use ghost_core::models::Outcome;
use ghost_core::policy::{AutonomyPolicy, PolicyError};
use ghost_core::routes::RouteName;
use serde::{Deserialize, Serialize};

use crate::models::{PhonePairing, PhoneView};

/// Why a phone session is refused (`ghostd.phone`'s `Refusal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    NotReady,
    Busy,
    NoSuchLine,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::NotReady => "not_ready",
            Refusal::Busy => "busy",
            Refusal::NoSuchLine => "no_such_line",
        }
    }
}

/// A phone's WebRTC connection, as the phone listener hands it to the daemon: opaque here,
/// typed by the daemon (W9).
pub type PhoneConnection = Box<dyn std::any::Any + Send>;

/// What a Test connection measured (`ghost_voice.llm.Probe`): request to first token and
/// to the end, the first words, the model that answered, or why not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    /// Seconds.
    pub ttft: Option<f64>,
    pub total: Option<f64>,
    pub text: String,
    pub model: String,
    pub error: Option<String>,
}

/// The environment a model connection is resolved from, as a draft edits it.
pub type ModelEnv = HashMap<String, String>;

#[async_trait]
pub trait Controller: Send + Sync {
    /// `conversation`: the line the session opens in (the desktop's Call button); Ghost
    /// when unset. A retired or unknown line is refused.
    async fn wake(&self, first_utterance: Option<String>, conversation: Option<String>) -> bool;
    async fn sleep(&self) -> bool;
    /// A typed turn into the session; with none running, the one it opens.
    async fn say(&self, text: String, conversation: Option<String>) -> bool;
    async fn confirm_pending(&self) -> Option<Outcome>;
    async fn cancel_pending(&self) -> bool;
    /// A change on the waiting list, by id (ADR 0030); with or without a session.
    async fn confirm_proposal(&self, proposal_id: &str) -> Option<Outcome>;
    async fn cancel_proposal(&self, proposal_id: &str) -> bool;
    /// The file, and Ghost's folder with it (ADR 0035).
    fn reload_policy(&self) -> Result<AutonomyPolicy, PolicyError>;
    /// Apply what can change without a restart; the keys that are live now.
    fn apply_settings(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String>;
    /// The model connection's keys, live (ADR 0037): the ones taken.
    fn apply_model(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String>;
    /// Test connection (ADR 0037): one tiny request through `env`. None when the daemon
    /// can't test one now.
    async fn probe_model(&self, env: ModelEnv, prompt: &str, max_tokens: u32) -> Option<Probe>;
    /// The routes this daemon has something behind (`RouteEngine.routes`); the digest
    /// is always one.
    fn routes_available(&self) -> Vec<RouteName>;
    // conversations (ADR 0014)
    /// In a session only.
    async fn switch_conversation(&self, slug: &str) -> bool;
    /// Asks the model, in session.
    async fn compact_conversation(&self, slug: &str) -> bool;
    fn voices(&self) -> Vec<String>;
    async fn sample_voice(&self, voice: &str, speed: f64, text: Option<String>) -> bool;
    // the paired phone (ADR 0016)
    /// None when the listener is off.
    fn phone_pairing(&self) -> Option<PhonePairing>;
    fn phone_view(&self) -> PhoneView;
    /// True if a phone was paired.
    async fn forget_phone(&self) -> bool;
    /// The phone listener's side: whether a call may start now.
    fn admit_phone(&self) -> Option<Refusal>;
    async fn start_phone(
        &self,
        connection: PhoneConnection,
        conversation: Option<String>,
    ) -> Option<Refusal>;
    async fn end_phone(&self) -> bool;
    // the vault opened after it was locked (ADR 0036): Ghost's settings and the apps follow
    async fn passwords_opened(&self);
}

/// A controller with nothing behind it: every call is refused, nothing is pending.
#[derive(Debug, Default, Clone, Copy)]
pub struct Unavailable;

#[async_trait]
impl Controller for Unavailable {
    async fn wake(&self, _first: Option<String>, _conversation: Option<String>) -> bool {
        false
    }

    async fn sleep(&self) -> bool {
        false
    }

    async fn say(&self, _text: String, _conversation: Option<String>) -> bool {
        false
    }

    async fn confirm_pending(&self) -> Option<Outcome> {
        None
    }

    async fn cancel_pending(&self) -> bool {
        false
    }

    async fn confirm_proposal(&self, _proposal_id: &str) -> Option<Outcome> {
        None
    }

    async fn cancel_proposal(&self, _proposal_id: &str) -> bool {
        false
    }

    fn reload_policy(&self) -> Result<AutonomyPolicy, PolicyError> {
        Ok(AutonomyPolicy::default())
    }

    fn apply_settings(&self, _values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        Vec::new()
    }

    fn apply_model(&self, _values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        Vec::new()
    }

    async fn probe_model(&self, _env: ModelEnv, _prompt: &str, _max: u32) -> Option<Probe> {
        None
    }

    fn routes_available(&self) -> Vec<RouteName> {
        vec![RouteName::Digest]
    }

    async fn switch_conversation(&self, _slug: &str) -> bool {
        false
    }

    async fn compact_conversation(&self, _slug: &str) -> bool {
        false
    }

    fn voices(&self) -> Vec<String> {
        Vec::new()
    }

    async fn sample_voice(&self, _voice: &str, _speed: f64, _text: Option<String>) -> bool {
        false
    }

    fn phone_pairing(&self) -> Option<PhonePairing> {
        None
    }

    fn phone_view(&self) -> PhoneView {
        PhoneView {
            address: None,
            addresses: Vec::new(),
            listen: Vec::new(),
            device: None,
        }
    }

    async fn forget_phone(&self) -> bool {
        false
    }

    fn admit_phone(&self) -> Option<Refusal> {
        Some(Refusal::NotReady)
    }

    async fn start_phone(
        &self,
        _connection: PhoneConnection,
        _conversation: Option<String>,
    ) -> Option<Refusal> {
        Some(Refusal::NotReady)
    }

    async fn end_phone(&self) -> bool {
        false
    }

    async fn passwords_opened(&self) {}
}
