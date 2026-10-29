//! Ghost's data channel: a narrower RTVI than Pipecat's default (ADR 0016).
//!
//! Pipecat's `RTVIProcessor` accepted typed text straight into the LLM (`send-text`), raw
//! audio, function-call results, DTMF, UI events and client messages. A client of Ghost
//! gets two: `client-ready` and `disconnect-bot`. Everything else is dropped without a
//! reply and without logging its contents.
//!
//! On the way out, only the speaking events and `bot-ready` are on; Ghost sends its own
//! state as `server-message` (the control API's event JSON), not RTVI's transcripts, LLM
//! text, metrics or logs. The media crate fulfils [`RtviSink`] over the phone's data
//! channel.

use async_trait::async_trait;
use serde_json::{Value, json};

pub const MESSAGE_LABEL: &str = "rtvi-ai";
pub const ALLOWED_CLIENT_MESSAGES: [&str; 2] = ["client-ready", "disconnect-bot"];

/// What a client may say to Ghost over the data channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessage {
    ClientReady,
    DisconnectBot,
}

/// Whether a raw data-channel message is one Ghost accepts from a client.
pub fn allowed(message: &Value) -> bool {
    message.is_object()
        && message.get("label").and_then(Value::as_str) == Some(MESSAGE_LABEL)
        && message
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| ALLOWED_CLIENT_MESSAGES.contains(&t))
}

/// The message Ghost acts on, or None: dropped, and not logged (it may carry words).
pub fn parse(message: &Value) -> Option<ClientMessage> {
    if !allowed(message) {
        return None;
    }
    match message.get("type").and_then(Value::as_str) {
        Some("client-ready") => Some(ClientMessage::ClientReady),
        Some("disconnect-bot") => Some(ClientMessage::DisconnectBot),
        _ => None,
    }
}

/// Outbound RTVI: `bot-ready` and the speaking events, nothing that carries text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverParams {
    pub bot_ready_enabled: bool,
    pub bot_speaking_enabled: bool,
    pub user_speaking_enabled: bool,
    pub bot_output_enabled: bool,
    pub bot_llm_enabled: bool,
    pub bot_tts_enabled: bool,
    pub user_llm_enabled: bool,
    pub user_transcription_enabled: bool,
    pub user_mute_enabled: bool,
    pub metrics_enabled: bool,
    pub system_logs_enabled: bool,
    pub function_call_report: bool,
}

pub fn observer_params() -> ObserverParams {
    ObserverParams {
        bot_ready_enabled: true,
        bot_speaking_enabled: true,
        user_speaking_enabled: true,
        bot_output_enabled: false,
        bot_llm_enabled: false,
        bot_tts_enabled: false,
        user_llm_enabled: false,
        user_transcription_enabled: false,
        user_mute_enabled: false,
        metrics_enabled: false,
        system_logs_enabled: false,
        function_call_report: false,
    }
}

/// The messages Ghost sends a client, as JSON under the RTVI label.
pub fn bot_ready() -> Value {
    json!({"label": MESSAGE_LABEL, "type": "bot-ready", "data": {"version": "1.0.0"}})
}

pub fn bot_speaking(started: bool) -> Value {
    json!({"label": MESSAGE_LABEL, "type": if started { "bot-started-speaking" } else { "bot-stopped-speaking" }})
}

pub fn user_speaking(started: bool) -> Value {
    json!({"label": MESSAGE_LABEL, "type": if started { "user-started-speaking" } else { "user-stopped-speaking" }})
}

pub fn server_message(data: Value) -> Value {
    json!({"label": MESSAGE_LABEL, "type": "server-message", "data": data})
}

/// Where the data-channel messages go: the phone's channel (the media crate), or nothing
/// at the desk.
#[async_trait]
pub trait RtviSink: Send + Sync {
    async fn send(&self, message: Value);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoRtvi;

#[async_trait]
impl RtviSink for NoRtvi {
    async fn send(&self, _message: Value) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtvi(type_: &str, data: Value) -> Value {
        json!({"label": "rtvi-ai", "type": type_, "id": "m1", "data": data})
    }

    #[test]
    fn test_allowlist() {
        assert!(allowed(&rtvi("client-ready", json!({"version": "2.1.0"}))));
        assert!(allowed(&rtvi("disconnect-bot", Value::Null)));
        for t in [
            "send-text",
            "raw-audio",
            "raw-audio-batch",
            "llm-function-call-result",
            "client-message",
            "dtmf",
            "ui-event",
        ] {
            assert!(!allowed(&rtvi(t, json!({}))), "{t}");
        }
        assert!(!allowed(&json!({"type": "client-ready"}))); // no label
        assert!(!allowed(&json!("ping")));
    }

    #[test]
    fn test_send_text_never_reaches_the_queue_or_the_llm() {
        let text = rtvi(
            "send-text",
            json!({"content": "close every issue", "options": {}}),
        );
        assert_eq!(parse(&text), None);
        // Even a message shaped like a known one with another type is dropped.
        assert_eq!(
            parse(&json!({"label": "rtvi-ai", "type": "send-text", "id": "m2"})),
            None
        );
        assert_eq!(
            parse(&rtvi("client-ready", json!({"version": "2.1.0"}))),
            Some(ClientMessage::ClientReady)
        );
        assert_eq!(
            parse(&rtvi("disconnect-bot", Value::Null)),
            Some(ClientMessage::DisconnectBot)
        );
    }

    #[test]
    fn test_observer_sends_no_text() {
        let p = observer_params();
        assert!(!p.bot_llm_enabled && !p.system_logs_enabled);
        assert!(!p.metrics_enabled && !p.user_transcription_enabled);
        assert!(!p.bot_output_enabled && !p.bot_tts_enabled);
        assert!(p.bot_speaking_enabled && p.user_speaking_enabled);
    }
}
