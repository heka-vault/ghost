//! The realtime voice (ADR 0062): an optional speech-to-speech model in front of Ghost's
//! agent, off by default and chosen by the operator only.
//!
//! The local pipeline stays the default and is untouched when the mode is off: nothing in
//! this module is built for a session unless `GHOST_VOICE_MODE=realtime` and the session
//! is the desk's with the microphone open. When it is on, after the wake and for the length
//! of the session, the desk's microphone audio (already through Ghost's echo setup) also
//! goes to the provider; idle audio and the wake never do.
//!
//! The realtime model is a mouth and ears, not the agent. It is given exactly three tools:
//! `consult` (hand the request to Ghost's agent), `hang_up` and `hold`. Everything else
//! happens on the machine: Ghost's own recogniser reads every segment beside it (the words
//! of record: the stored transcript, the gate's evidence for a yes, the fixed intents such
//! as "stop listening"), and Ghost's agent answers a consult through the session's own model
//! stage, dispatcher and gate. The agent's answer is handed back as the exact text to say,
//! and what the provider actually says is checked word by word against it as it plays
//! ([`relay`]); a reading that strays is cut and the rest is said in the local voice.
//!
//! The seam is [`RealtimeProvider`] and [`RealtimeLink`] with the [`Event`]s a provider
//! reports; [`openai`] is the one shipped, [`fake`] the scripted server the tests run.

pub mod fake;
pub mod openai;
pub mod relay;
pub mod stage;

pub mod check;

pub use ghost_core::realtime::{settings, spend};

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub use ghost_core::realtime::{Prices, RealtimeSettings, Usage, provider_spec};

/// The provider the settings name, or why it cannot be built: the key is read by its
/// name from `get` (the daemon's environment, which the vault fills) and nowhere else.
pub fn provider(
    s: &RealtimeSettings,
    get: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn RealtimeProvider>, String> {
    let key = s.key(&get).ok_or_else(|| {
        format!(
            "the realtime voice's password {} is not in Ghost's vault",
            s.password
        )
    })?;
    Ok(Arc::new(openai::OpenAiRealtime::new(
        s.url(),
        key,
        &s.model,
        &s.voice,
        s.silence_ms,
        s.prices(),
    )))
}

/// The rate Ghost plays at; every provider's audio reaches the pipeline at it.
pub const OUT_RATE: u32 = 24_000;
/// The microphone's rate inside a session; a provider converts from it.
pub const IN_RATE: u32 = 16_000;

/// The one sentence said in the local voice when the realtime voice goes (ADR 0062).
pub const FALLBACK_SENTENCE: &str =
    "I lost the realtime voice, so I'm answering on this machine for now.";
/// Said in the local voice when the day's cap is reached mid-session.
pub const CAP_SENTENCE: &str =
    "That's today's limit for the realtime voice, so I'm answering on this machine now.";

/// What a provider is given when a session connects: the realtime model's instructions
/// and its tools, both fixed in Ghost's code (no app's text, no persona file).
#[derive(Clone, Debug)]
pub struct Setup {
    pub instructions: String,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

pub const CONSULT: &str = "consult";
pub const HANG_UP: &str = "hang_up";
pub const HOLD: &str = "hold";
/// The one key a relay's metadata carries, so its response is known when it starts.
pub const RELAY_TAG: &str = "ghost";

/// The realtime model's instructions. Ghost's agent is the one that knows and does; this
/// model only talks, and says the agent's answers exactly.
pub const INSTRUCTIONS: &str = "You are the voice of Ghost, a personal assistant for one person at their desk. \
You are only the voice: you know nothing about their work, their apps, their issues, their schedule or Ghost itself, \
and you cannot do anything. For any question about facts, any lookup, any change, anything about their work, \
apps, agents, schedules, messages or Ghost, call `consult` with their request in their own words, and before \
the call say only a few natural words such as \"One sec.\" or \"Let me check.\" Never guess, never answer a factual \
question yourself, and never say that something was done, changed, sent, closed or created: only Ghost's \
agent knows that. Small talk (a greeting, thanks, how are you) you may answer yourself in a few words. \
When you are told to say something exactly, say exactly those words, every one, in order, with nothing added, \
softened or left out, and nothing after them. When they say goodbye or are done, call `hang_up`. When they ask \
you to wait or hang on, call `hold`. Speak briefly and plainly, in English, like a calm colleague.";

/// The exact-reading instruction for a relay: the agent's words follow it.
pub const SAY_EXACTLY: &str = "Say exactly the following words, all of them, in order, and nothing else. \
Do not add, change, soften or leave out anything, do not answer it, and stop when they end:\n\n";

pub fn setup() -> Setup {
    Setup {
        instructions: INSTRUCTIONS.to_string(),
        tools: tools(),
    }
}

pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: CONSULT,
            description: "Hand the operator's request to Ghost's agent, which knows their work and \
                can look things up and make changes. Use it for anything beyond small talk.",
            parameters: json!({
                "type": "object",
                "properties": {"request": {"type": "string", "description": "What they asked, in their words."}},
                "required": ["request"]
            }),
        },
        ToolSpec {
            name: HANG_UP,
            description: "End the conversation: they said goodbye or that they are done.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSpec {
            name: HOLD,
            description: "Wait quietly: they asked you to hang on or wait.",
            parameters: json!({"type": "object", "properties": {}}),
        },
    ]
}

/// Why the realtime voice could not be used, or stopped. The provider's own words are
/// kept short and never carry the key (`openai` never puts it in an error).
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The provider's address did not answer, or the connection did not complete in time.
    Unreachable(String),
    /// The provider refused the key or the account (401, 403, quota).
    Refused(String),
    /// The provider said slow down (429, a rate limit).
    RateLimited(String),
    /// The connection closed or broke mid-session.
    Dropped(String),
    /// The provider answered an error Ghost cannot work around.
    Provider(String),
    /// Today's spend reached the operator's cap.
    Cap,
}

impl Failure {
    /// What the operator hears, once, in the local voice.
    pub fn sentence(&self) -> &'static str {
        match self {
            Failure::Cap => CAP_SENTENCE,
            _ => FALLBACK_SENTENCE,
        }
    }

    /// The note on the waiting list (shown, not said).
    pub fn note(&self) -> String {
        match self {
            Failure::Unreachable(w) => {
                format!("The realtime voice could not be reached ({w}); that session used the voice on this machine.")
            }
            Failure::Refused(w) => format!(
                "The realtime voice's provider refused the key ({w}); that session used the voice on this machine. Check the password in Settings › Sound."
            ),
            Failure::RateLimited(w) => format!(
                "The realtime voice's provider asked Ghost to slow down ({w}); that session went on with the voice on this machine."
            ),
            Failure::Dropped(w) => format!(
                "The realtime voice dropped mid-session ({w}); Ghost went on with the voice on this machine."
            ),
            Failure::Provider(w) => format!(
                "The realtime voice's provider answered an error ({w}); Ghost went on with the voice on this machine."
            ),
            Failure::Cap => "Today's spend on the realtime voice reached your cap; Ghost uses the voice on this machine until tomorrow. The cap is in Settings › Sound.".into(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Failure::Unreachable(_) => "unreachable",
            Failure::Refused(_) => "refused",
            Failure::RateLimited(_) => "rate_limited",
            Failure::Dropped(_) => "dropped",
            Failure::Provider(_) => "provider",
            Failure::Cap => "cap",
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Unreachable(w)
            | Failure::Refused(w)
            | Failure::RateLimited(w)
            | Failure::Dropped(w)
            | Failure::Provider(w) => write!(f, "{}: {w}", self.kind()),
            Failure::Cap => write!(f, "cap"),
        }
    }
}

/// How a response ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Completed,
    Cancelled,
    Incomplete,
    Failed(String),
}

/// What a provider reports. Audio is mono s16 at [`OUT_RATE`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The provider's own VAD heard speech start or stop (it decides its turn on these).
    SpeechStarted,
    SpeechStopped,
    /// A response began; `tag` is the relay tag Ghost gave it, when it asked for it.
    Started {
        response: String,
        tag: Option<String>,
    },
    Audio {
        response: String,
        item: String,
        pcm: Vec<i16>,
    },
    /// The words of the response's audio, as the provider transcribes its own speech.
    Said {
        response: String,
        delta: String,
    },
    SaidAll {
        response: String,
        text: String,
    },
    Call {
        response: String,
        call_id: String,
        name: String,
        args: Value,
    },
    Done {
        response: String,
        status: Status,
        usage: Option<Usage>,
    },
    /// An error the session survives: `code` as the provider names it.
    Rejected {
        code: String,
        message: String,
    },
    /// The session cannot go on.
    Failed(Failure),
}

/// A connected session's controls. Every method queues and returns: none waits on the
/// network, so a stage never stalls on the provider.
pub trait RealtimeLink: Send + Sync {
    /// The operator's audio, 16 kHz mono s16, as the session hears it.
    fn audio(&self, pcm: &[i16]);
    /// Stop the response being made.
    fn cancel(&self);
    /// What of an item's audio was heard, so the provider's record matches.
    fn truncate(&self, item: &str, played_ms: u64);
    /// A tool call's result.
    fn answer(&self, call_id: &str, output: &str);
    /// Say `text` exactly, as a response tagged `tag`, with no tool allowed.
    fn say(&self, text: &str, tag: &str);
    /// Whether the provider answers the operator's turns on its own.
    fn auto_reply(&self, on: bool);
    /// A written turn and a reply to it: the connection check's only (`check`).
    fn ask(&self, text: &str);
    fn close(&self);
}

pub struct Connection {
    pub link: Arc<dyn RealtimeLink>,
    pub events: mpsc::UnboundedReceiver<Event>,
}

/// A realtime provider: one implementation per service, the rest of Ghost unchanged.
#[async_trait]
pub trait RealtimeProvider: Send + Sync {
    /// The provider's id (`openai`).
    fn name(&self) -> &str;
    /// The model it speaks with.
    fn model(&self) -> &str;
    /// Its price list, for the spend estimate; `None` when Ghost does not know it.
    fn prices(&self) -> Option<Prices>;
    async fn connect(&self, setup: &Setup) -> Result<Connection, Failure>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_realtime_model_has_exactly_three_tools_and_no_app() {
        let names: Vec<&str> = tools().iter().map(|t| t.name).collect();
        assert_eq!(names, vec![CONSULT, HANG_UP, HOLD]);
        // Its instructions are Ghost's own text: it is told it knows nothing and does nothing.
        assert!(INSTRUCTIONS.contains("call `consult`"));
        assert!(INSTRUCTIONS.contains("never say that something was done"));
    }

    #[test]
    fn a_failure_is_said_in_one_sentence_and_noted_with_its_reason() {
        let f = Failure::Dropped("closed by the provider".into());
        assert_eq!(f.sentence(), FALLBACK_SENTENCE);
        assert!(f.note().contains("closed by the provider"));
        assert_eq!(Failure::Cap.sentence(), CAP_SENTENCE);
        assert!(Failure::Cap.note().contains("Settings › Sound"));
    }
}
