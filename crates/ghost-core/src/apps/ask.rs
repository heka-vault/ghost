//! An app's question for the operator, in the middle of a call (ADR 0046; the protocol
//! calls it elicitation).
//!
//! The client side is whole: the question arrives, is redacted and bounded, goes to an
//! [`Asker`] with a time limit, and the answer goes back to the app. What is not here yet
//! is the operator: the default asker, [`DeclineAll`], declines every question, and the
//! real one (speak it, show it, wait for the answer) plugs in through `Apps::set_asker`.
//!
//! A question's text is the app's: data, never an instruction. A question that sends the
//! operator to a web address (URL mode) is declined before it reaches an asker.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value};

/// A question may not be longer than this; the rest is cut.
pub const QUESTION_MAX: usize = 500;

/// One question from an app.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    /// The app's name in `apps.json`.
    pub app: String,
    /// How the operator knows the app.
    pub title: String,
    /// What it asks, redacted and cut to [`QUESTION_MAX`].
    pub message: String,
    /// The JSON Schema of the answer it wants: an object of plain fields.
    pub schema: Value,
    /// The values to choose from, when the answer is one field with a fixed list.
    pub choices: Vec<String>,
    /// How long the app waits. Past it the question counts as declined.
    pub timeout: Duration,
}

/// What the operator did with it.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The fields the schema asked for.
    Answered(Map<String, Value>),
    /// Not answering; the app may go on without.
    Declined,
    /// Stop the whole call.
    Cancelled,
}

/// Whoever can put a question to the operator and bring back the answer.
#[async_trait]
pub trait Asker: Send + Sync {
    async fn ask(&self, question: Question) -> Answer;
}

/// The asker until the operator's path exists: every question is declined, at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeclineAll;

#[async_trait]
impl Asker for DeclineAll {
    async fn ask(&self, _question: Question) -> Answer {
        Answer::Declined
    }
}

/// The asker every app of one `Apps` shares, replaceable while apps are connected.
pub struct AskerSlot {
    asker: RwLock<Arc<dyn Asker>>,
    set: AtomicBool,
}

impl Default for AskerSlot {
    fn default() -> Self {
        Self {
            asker: RwLock::new(Arc::new(DeclineAll)),
            set: AtomicBool::new(false),
        }
    }
}

impl AskerSlot {
    /// With an asker already in it.
    pub fn with(asker: Arc<dyn Asker>) -> Self {
        let slot = Self::default();
        slot.set(asker);
        slot
    }

    pub fn set(&self, asker: Arc<dyn Asker>) {
        *self.asker.write().unwrap_or_else(|e| e.into_inner()) = asker;
        self.set.store(true, Ordering::SeqCst);
    }

    pub fn get(&self) -> Arc<dyn Asker> {
        self.asker.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether someone can be asked: an app is told Ghost takes questions only then.
    pub fn asks(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }
}

/// The fixed list to choose from, when the schema is one field with an `enum` (or a
/// `oneOf` of constants).
pub(crate) fn choices_of(schema: &Value) -> Vec<String> {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    if props.len() != 1 {
        return Vec::new();
    }
    let field = props.values().next().expect("one field");
    if let Some(Value::Array(values)) = field.get("enum") {
        return values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    if let Some(Value::Array(options)) = field.get("oneOf") {
        return options
            .iter()
            .filter_map(|o| o.get("const").and_then(Value::as_str).map(String::from))
            .collect();
    }
    Vec::new()
}

#[cfg(test)]
mod unit {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_field_with_a_fixed_list_gives_the_choices() {
        let one = json!({"type": "object", "properties":
            {"env": {"type": "string", "enum": ["staging", "production"]}}});
        assert_eq!(choices_of(&one), vec!["staging", "production"]);
        let titled = json!({"properties": {"env": {"oneOf":
            [{"const": "a", "title": "A"}, {"const": "b", "title": "B"}]}}});
        assert_eq!(choices_of(&titled), vec!["a", "b"]);
        let two = json!({"properties": {"a": {"enum": ["x"]}, "b": {"type": "string"}}});
        assert!(choices_of(&two).is_empty());
        assert!(choices_of(&json!({"properties": {"a": {"type": "string"}}})).is_empty());
    }
}
