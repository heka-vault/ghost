//! The data channel's RTVI subset (ADR 0016, `docs/phone-protocol.md`), as the Python's
//! `ghost_voice.rtvi` narrowed it. In: the keepalive `ping`, `client-ready` and
//! `disconnect-bot`; everything else is dropped without a reply and without its contents
//! reaching a log (it may carry words). Out: `bot-ready`, the four speaking events, and
//! `server-message` carrying one control API event, the same JSON the WebSocket sends.
//! The shapes are Pipecat's, null fields left out.

use serde::Serialize;
use serde_json::Value;

pub const LABEL: &str = "rtvi-ai";
/// The RTVI protocol version Ghost advertises in `bot-ready` (Pipecat's).
pub const PROTOCOL_VERSION: &str = "2.1.0";
/// What the Python did for a client whose major version is this: echo its version back.
pub const LEGACY_MAJOR: u64 = 1;
/// How many outbound messages wait for the channel to open (Pipecat's queue).
pub const OUTBOX_CAPACITY: usize = 50;

/// A client message that passed the subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// The phone's speaker route and remote track are up: greet. `version` is the
    /// client's protocol version (`2.1.0` from the phone).
    ClientReady { id: String, version: Option<String> },
    /// End the session, the same as `POST /end`.
    DisconnectBot { id: String },
}

/// One data-channel text, sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// The once-a-second keepalive.
    Ping,
    Client(ClientMessage),
    /// Not for Ghost: dropped, and its contents are not kept.
    Dropped,
}

/// Sort a text the phone sent. Never logs it.
pub fn parse_inbound(text: &str) -> Inbound {
    if text.starts_with("ping") {
        return Inbound::Ping;
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Inbound::Dropped;
    };
    let Some(obj) = value.as_object() else {
        return Inbound::Dropped;
    };
    if obj.get("label").and_then(Value::as_str) != Some(LABEL) {
        return Inbound::Dropped;
    }
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match obj.get("type").and_then(Value::as_str) {
        Some("client-ready") => {
            let version = obj
                .get("data")
                .and_then(|d| d.get("version"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Inbound::Client(ClientMessage::ClientReady { id, version })
        }
        Some("disconnect-bot") => Inbound::Client(ClientMessage::DisconnectBot { id }),
        _ => Inbound::Dropped,
    }
}

/// The version `bot-ready` carries for a client that said `version`: Pipecat echoes a
/// legacy (1.x) client's own version and advertises its protocol version otherwise.
pub fn bot_ready_version(client_version: Option<&str>) -> String {
    if let Some(v) = client_version {
        let major = v.split('.').next().and_then(|m| m.parse::<u64>().ok());
        if major == Some(LEGACY_MAJOR) {
            return v.to_string();
        }
    }
    PROTOCOL_VERSION.to_string()
}

/// What Ghost sends the phone.
#[derive(Debug, Clone, PartialEq)]
pub enum Outbound {
    /// In reply to `client-ready`: Ghost is listening. `version` from
    /// [`bot_ready_version`].
    BotReady {
        id: String,
        version: String,
    },
    UserStartedSpeaking,
    UserStoppedSpeaking,
    BotStartedSpeaking,
    BotStoppedSpeaking,
    /// One control API event (`status`, `turn`, `pending`, `outcome`, `conversation`),
    /// exactly as the WebSocket sends it. Never a `log` event.
    ServerMessage(Value),
}

#[derive(Serialize)]
struct Plain {
    label: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct About {
    library: &'static str,
    library_version: &'static str,
}

#[derive(Serialize)]
struct BotReadyData {
    version: String,
    about: About,
}

#[derive(Serialize)]
struct BotReady {
    label: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    data: BotReadyData,
}

#[derive(Serialize)]
struct ServerMessage<'a> {
    label: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    data: &'a Value,
}

impl Outbound {
    /// The wire text.
    pub fn to_json(&self) -> String {
        match self {
            Outbound::BotReady { id, version } => serde_json::to_string(&BotReady {
                label: LABEL,
                kind: "bot-ready",
                id: id.clone(),
                data: BotReadyData {
                    version: version.clone(),
                    about: About {
                        library: "ghost",
                        library_version: env!("CARGO_PKG_VERSION"),
                    },
                },
            }),
            Outbound::UserStartedSpeaking => plain("user-started-speaking"),
            Outbound::UserStoppedSpeaking => plain("user-stopped-speaking"),
            Outbound::BotStartedSpeaking => plain("bot-started-speaking"),
            Outbound::BotStoppedSpeaking => plain("bot-stopped-speaking"),
            Outbound::ServerMessage(data) => serde_json::to_string(&ServerMessage {
                label: LABEL,
                kind: "server-message",
                data,
            }),
        }
        .expect("RTVI messages serialize")
    }

    /// `server-message` never carries a log event (`docs/phone-protocol.md`).
    pub fn server_message(event: Value) -> Option<Outbound> {
        if event.get("type").and_then(Value::as_str) == Some("log") {
            return None;
        }
        Some(Outbound::ServerMessage(event))
    }
}

fn plain(kind: &'static str) -> serde_json::Result<String> {
    serde_json::to_string(&Plain { label: LABEL, kind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_client_ready_and_disconnect_bot_get_in() {
        assert_eq!(parse_inbound("ping"), Inbound::Ping);
        assert_eq!(parse_inbound("ping 17"), Inbound::Ping);
        let ready = json!({"label": "rtvi-ai", "type": "client-ready", "id": "1",
            "data": {"version": "2.1.0", "about": {"library": "ghost-mobile"}}});
        assert_eq!(
            parse_inbound(&ready.to_string()),
            Inbound::Client(ClientMessage::ClientReady {
                id: "1".into(),
                version: Some("2.1.0".into())
            })
        );
        let bye = json!({"label": "rtvi-ai", "type": "disconnect-bot", "id": "2", "data": {}});
        assert_eq!(
            parse_inbound(&bye.to_string()),
            Inbound::Client(ClientMessage::DisconnectBot { id: "2".into() })
        );
        for text in [
            r#"{"label": "rtvi-ai", "type": "send-text", "id": "3", "data": {"content": "hi"}}"#,
            r#"{"label": "rtvi-ai", "type": "raw-audio", "data": {}}"#,
            r#"{"label": "other", "type": "client-ready"}"#,
            r#"{"type": "client-ready"}"#,
            r#"{"type": "signalling", "message": {"type": "trackStatus"}}"#,
            "[1, 2]",
            "not json",
            "",
        ] {
            assert_eq!(parse_inbound(text), Inbound::Dropped, "{text}");
        }
    }

    #[test]
    fn outbound_shapes_are_pipecats() {
        let ready = Outbound::BotReady {
            id: "7".into(),
            version: bot_ready_version(Some("2.1.0")),
        };
        let v: Value = serde_json::from_str(&ready.to_json()).unwrap();
        assert_eq!(v["label"], "rtvi-ai");
        assert_eq!(v["type"], "bot-ready");
        assert_eq!(v["id"], "7");
        assert_eq!(v["data"]["version"], "2.1.0");
        assert_eq!(v["data"]["about"]["library"], "ghost");
        assert_eq!(bot_ready_version(Some("1.2.0")), "1.2.0");
        assert_eq!(bot_ready_version(None), "2.1.0");
        assert_eq!(
            Outbound::UserStartedSpeaking.to_json(),
            r#"{"label":"rtvi-ai","type":"user-started-speaking"}"#
        );
        assert_eq!(
            Outbound::BotStoppedSpeaking.to_json(),
            r#"{"label":"rtvi-ai","type":"bot-stopped-speaking"}"#
        );
        let event = json!({"type": "pending", "pending": null});
        let msg = Outbound::server_message(event.clone()).unwrap();
        let v: Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(
            v,
            json!({"label": "rtvi-ai", "type": "server-message", "data": event})
        );
        assert!(Outbound::server_message(json!({"type": "log", "message": "x"})).is_none());
    }
}
