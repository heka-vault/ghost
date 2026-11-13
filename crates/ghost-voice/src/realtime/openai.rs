//! OpenAI's Realtime API over a WebSocket (ADR 0062), the first realtime provider.
//!
//! `wss://api.openai.com/v1/realtime?model=<model>` with the key as `Authorization: Bearer`,
//! sent there and nowhere else, never logged, never in an error. After `session.created`
//! the session is set once (`session.update`): audio out only, 24 kHz PCM both ways, the
//! provider's own VAD to end a turn (`server_vad`, `create_response` on and
//! `interrupt_response` off: Ghost's own barge-in rule decides an interruption, since the
//! provider's VAD alone would cut Ghost off on a leak of its own voice), the voice, the
//! fixed instructions and the three tools. The operator's 16 kHz audio is resampled to
//! 24 kHz here. A relay is `response.create` with the exact words as its instructions, an
//! empty `input` (the answer is all it reads from), `tool_choice: none`, and a metadata tag
//! the `response.created` event echoes. Usage comes on `response.done`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, warn};
use zeroize::Zeroizing;

use super::{
    Connection, Event, Failure, Prices, RELAY_TAG, RealtimeLink, RealtimeProvider, SAY_EXACTLY,
    Setup, Status, Usage,
};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

pub struct OpenAiRealtime {
    url: String,
    key: Zeroizing<String>,
    model: String,
    voice: String,
    silence_ms: u32,
    prices: Option<Prices>,
    connect_timeout: Duration,
}

impl OpenAiRealtime {
    pub fn new(
        url: String,
        key: Zeroizing<String>,
        model: &str,
        voice: &str,
        silence_ms: u32,
        prices: Option<Prices>,
    ) -> Self {
        Self {
            url,
            key,
            model: model.to_string(),
            voice: voice.to_string(),
            silence_ms,
            prices,
            connect_timeout: CONNECT_TIMEOUT,
        }
    }

    pub fn with_connect_timeout(mut self, t: Duration) -> Self {
        self.connect_timeout = t;
        self
    }

    fn endpoint(&self) -> String {
        let base = self.url.trim_end_matches('/');
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}model={}", self.model)
    }

    /// The one `session.update` a session is set with.
    pub fn session_update(&self, setup: &Setup) -> Value {
        session_update(&self.model, &self.voice, self.silence_ms, setup)
    }
}

pub fn session_update(model: &str, voice: &str, silence_ms: u32, setup: &Setup) -> Value {
    let tools: Vec<Value> = setup
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();
    json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "model": model,
            "output_modalities": ["audio"],
            "instructions": setup.instructions,
            "audio": {
                "input": {
                    "format": {"type": "audio/pcm", "rate": 24000},
                    "turn_detection": turn_detection(silence_ms, true),
                },
                "output": {
                    "format": {"type": "audio/pcm", "rate": 24000},
                    "voice": voice,
                },
            },
            "tools": tools,
            "tool_choice": "auto",
        }
    })
}

fn turn_detection(silence_ms: u32, reply: bool) -> Value {
    json!({
        "type": "server_vad",
        "silence_duration_ms": silence_ms,
        "create_response": reply,
        "interrupt_response": false,
    })
}

/// 16 kHz to 24 kHz, linear, carried across chunks: speech for a model, not for music.
#[derive(Default)]
pub struct Upsampler {
    last: Option<i16>,
    phase: u32,
}

impl Upsampler {
    /// Three output samples for every two in.
    pub fn push(&mut self, input: &[i16]) -> Vec<i16> {
        let mut out = Vec::with_capacity(input.len() * 3 / 2 + 2);
        for &s in input {
            let prev = self.last.unwrap_or(s);
            // Output positions fall at thirds of the input step: 0, 2/3, 4/3, ...
            // Each new input sample closes the interval (prev, s].
            loop {
                // phase counts output samples in units of 1/3 input step since `prev`.
                let pos = self.phase; // in thirds, 0..3
                if pos >= 3 {
                    self.phase -= 3;
                    break;
                }
                let v = f32::from(prev) + (f32::from(s) - f32::from(prev)) * pos as f32 / 3.0;
                out.push(v.round() as i16);
                self.phase += 2;
            }
            self.last = Some(s);
        }
        out
    }
}

fn pcm_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut b = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

fn pcm_of(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// The provider's usage on `response.done`, cached input apart.
pub fn usage_of(u: &Value) -> Option<Usage> {
    if !u.is_object() {
        return None;
    }
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let input = &u["input_token_details"];
    let cached = &input["cached_tokens_details"];
    let output = &u["output_token_details"];
    let text_cached = n(&cached["text_tokens"]);
    let audio_cached = n(&cached["audio_tokens"]);
    Some(Usage {
        text_in: n(&input["text_tokens"]).saturating_sub(text_cached),
        audio_in: n(&input["audio_tokens"]).saturating_sub(audio_cached),
        text_cached,
        audio_cached,
        text_out: n(&output["text_tokens"]),
        audio_out: n(&output["audio_tokens"]),
    })
}

/// A provider event as Ghost reads it; `None` for one it does not need.
pub fn event_of(v: &Value) -> Option<Event> {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    match v["type"].as_str()? {
        "input_audio_buffer.speech_started" => Some(Event::SpeechStarted),
        "input_audio_buffer.speech_stopped" => Some(Event::SpeechStopped),
        "response.created" => Some(Event::Started {
            response: v["response"]["id"].as_str().unwrap_or("").to_string(),
            tag: v["response"]["metadata"][RELAY_TAG]
                .as_str()
                .map(str::to_string),
        }),
        "response.output_audio.delta" => {
            let bytes = B64.decode(v["delta"].as_str().unwrap_or("")).ok()?;
            Some(Event::Audio {
                response: s("response_id"),
                item: s("item_id"),
                pcm: pcm_of(&bytes),
            })
        }
        "response.output_audio_transcript.delta" => Some(Event::Said {
            response: s("response_id"),
            delta: s("delta"),
        }),
        "response.output_audio_transcript.done" => Some(Event::SaidAll {
            response: s("response_id"),
            text: s("transcript"),
        }),
        "response.function_call_arguments.done" => Some(Event::Call {
            response: s("response_id"),
            call_id: s("call_id"),
            name: s("name"),
            args: serde_json::from_str(v["arguments"].as_str().unwrap_or("{}"))
                .unwrap_or(Value::Null),
        }),
        "response.done" => {
            let r = &v["response"];
            let status = match r["status"].as_str().unwrap_or("") {
                "completed" => Status::Completed,
                "cancelled" => Status::Cancelled,
                "incomplete" => Status::Incomplete,
                "failed" => Status::Failed(
                    r["status_details"]["error"]["code"]
                        .as_str()
                        .or_else(|| r["status_details"]["error"]["type"].as_str())
                        .unwrap_or("failed")
                        .to_string(),
                ),
                _ => Status::Completed,
            };
            Some(Event::Done {
                response: r["id"].as_str().unwrap_or("").to_string(),
                status,
                usage: usage_of(&r["usage"]),
            })
        }
        "error" => {
            let e = &v["error"];
            let code = e["code"]
                .as_str()
                .or_else(|| e["type"].as_str())
                .unwrap_or("error")
                .to_string();
            let kind = e["type"].as_str().unwrap_or("");
            let message: String = e["message"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            Some(fatal(kind, &code).unwrap_or(Event::Rejected { code, message }))
        }
        _ => None,
    }
}

/// An error the session cannot go on after.
fn fatal(kind: &str, code: &str) -> Option<Event> {
    let c = code.to_ascii_lowercase();
    if c.contains("rate_limit") {
        return Some(Event::Failed(Failure::RateLimited(code.to_string())));
    }
    if c.contains("api_key") || c.contains("quota") || c.contains("auth") || c.contains("billing") {
        return Some(Event::Failed(Failure::Refused(code.to_string())));
    }
    if kind == "server_error" || c.contains("session_expired") {
        return Some(Event::Failed(Failure::Provider(code.to_string())));
    }
    None
}

/// What the link queues for the socket's writer.
enum Out {
    Json(Value),
    Close,
}

struct Link {
    tx: mpsc::UnboundedSender<Out>,
    up: Mutex<Upsampler>,
    silence_ms: u32,
}

impl Link {
    fn send(&self, v: Value) {
        let _ = self.tx.send(Out::Json(v));
    }
}

impl RealtimeLink for Link {
    fn audio(&self, pcm: &[i16]) {
        let up = self.up.lock().unwrap().push(pcm);
        if up.is_empty() {
            return;
        }
        self.send(json!({
            "type": "input_audio_buffer.append",
            "audio": B64.encode(pcm_bytes(&up)),
        }));
    }

    fn cancel(&self) {
        self.send(json!({"type": "response.cancel"}));
    }

    fn truncate(&self, item: &str, played_ms: u64) {
        if item.is_empty() {
            return;
        }
        self.send(json!({
            "type": "conversation.item.truncate",
            "item_id": item,
            "content_index": 0,
            "audio_end_ms": played_ms,
        }));
    }

    fn answer(&self, call_id: &str, output: &str) {
        self.send(json!({
            "type": "conversation.item.create",
            "item": {"type": "function_call_output", "call_id": call_id, "output": output},
        }));
    }

    fn say(&self, text: &str, tag: &str) {
        self.send(json!({
            "type": "response.create",
            "response": {
                "instructions": format!("{SAY_EXACTLY}{text}"),
                "input": [],
                "tool_choice": "none",
                "metadata": {RELAY_TAG: tag},
            }
        }));
    }

    fn auto_reply(&self, on: bool) {
        self.send(json!({
            "type": "session.update",
            "session": {
                "type": "realtime",
                "audio": {"input": {"turn_detection": turn_detection(self.silence_ms, on)}},
            }
        }));
    }

    fn ask(&self, text: &str) {
        self.send(json!({
            "type": "conversation.item.create",
            "item": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]},
        }));
        self.send(json!({"type": "response.create"}));
    }

    fn close(&self) {
        let _ = self.tx.send(Out::Close);
    }
}

fn http_failure(status: u16) -> Failure {
    match status {
        401 | 403 => Failure::Refused(format!("HTTP {status}")),
        429 => Failure::RateLimited(format!("HTTP {status}")),
        _ => Failure::Provider(format!("HTTP {status}")),
    }
}

#[async_trait]
impl RealtimeProvider for OpenAiRealtime {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn prices(&self) -> Option<Prices> {
        self.prices
    }

    async fn connect(&self, setup: &Setup) -> Result<Connection, Failure> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let endpoint = self.endpoint();
        let mut request = endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| Failure::Unreachable("the address does not read".into()))?;
        let bearer = Zeroizing::new(format!("Bearer {}", self.key.as_str()));
        let header = bearer
            .as_str()
            .parse()
            .map_err(|_| Failure::Refused("the key can't be sent as a header".into()))?;
        request.headers_mut().insert("authorization", header);
        let host = request.uri().host().unwrap_or("the provider").to_string();
        let ws = match tokio::time::timeout(
            self.connect_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) => {
                return Err(http_failure(r.status().as_u16()));
            }
            Ok(Err(e)) => {
                // tungstenite's messages name the host and the cause, never a header
                return Err(Failure::Unreachable(format!(
                    "{host}: {}",
                    short(&e.to_string())
                )));
            }
            Err(_) => return Err(Failure::Unreachable(format!("{host}: no answer in time"))),
        };
        let (mut sink, mut stream) = ws.split();
        // The provider speaks first: wait for its session, then set ours.
        let created = tokio::time::timeout(self.connect_timeout, async {
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(WsMessage::Text(raw)) => {
                        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                            continue;
                        };
                        match v["type"].as_str() {
                            Some("session.created") => return Ok(()),
                            Some("error") => {
                                return Err(match event_of(&v) {
                                    Some(Event::Failed(f)) => f,
                                    _ => Failure::Provider(
                                        v["error"]["code"].as_str().unwrap_or("error").into(),
                                    ),
                                });
                            }
                            _ => {}
                        }
                    }
                    Ok(WsMessage::Close(_)) | Err(_) => {
                        return Err(Failure::Dropped("closed before the session opened".into()));
                    }
                    _ => {}
                }
            }
            Err(Failure::Dropped("closed before the session opened".into()))
        })
        .await
        .map_err(|_| Failure::Unreachable(format!("{host}: no session in time")))?;
        created?;
        let update = self.session_update(setup);
        sink.send(WsMessage::Text(update.to_string().into()))
            .await
            .map_err(|e| Failure::Dropped(short(&e.to_string())))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
        let (etx, erx) = mpsc::unbounded_channel::<Event>();
        // The writer: everything queued, in order.
        tokio::spawn(async move {
            while let Some(out) = rx.recv().await {
                match out {
                    Out::Json(v) => {
                        if sink
                            .send(WsMessage::Text(v.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Out::Close => {
                        let _ = sink.close().await;
                        break;
                    }
                }
            }
        });
        // The reader: every event Ghost reads, then why it ended.
        let etx2 = etx.clone();
        tokio::spawn(async move {
            let why = loop {
                match stream.next().await {
                    Some(Ok(WsMessage::Text(raw))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                            continue;
                        };
                        if let Some(e) = event_of(&v) {
                            if let Event::Rejected { code, .. } = &e {
                                debug!("realtime: the provider declined a request ({code})");
                            }
                            let failed = matches!(e, Event::Failed(_));
                            if etx2.send(e).is_err() || failed {
                                return;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        break frame
                            .map(|f| format!("closed by the provider ({})", u16::from(f.code)))
                            .unwrap_or_else(|| "closed by the provider".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break short(&e.to_string()),
                    None => break "the connection ended".into(),
                }
            };
            warn!("realtime: {why}");
            let _ = etx2.send(Event::Failed(Failure::Dropped(why)));
        });
        Ok(Connection {
            link: Arc::new(Link {
                tx,
                up: Mutex::new(Upsampler::default()),
                silence_ms: self.silence_ms,
            }),
            events: erx,
        })
    }
}

fn short(s: &str) -> String {
    s.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_samples_in_are_three_out_across_chunks() {
        let mut up = Upsampler::default();
        let a = up.push(&[0, 300]);
        let b = up.push(&[600, 900, 1200, 1500]);
        let all: Vec<i16> = a.into_iter().chain(b).collect();
        // 6 in, 9 out (give or take the first, which has no sample before it)
        assert!((8..=10).contains(&all.len()), "{all:?}");
        // monotone on a ramp: no glitch at the chunk's edge
        assert!(all.windows(2).all(|w| w[1] >= w[0]), "{all:?}");
        let mut up = Upsampler::default();
        let n: usize = (0..50).map(|_| up.push(&[0; 320]).len()).sum();
        assert!((n as i64 - 24_000).abs() <= 2, "{n}");
    }

    #[test]
    fn usage_counts_cached_input_apart() {
        let u = usage_of(&json!({
            "total_tokens": 2000, "input_tokens": 1500, "output_tokens": 500,
            "input_token_details": {"text_tokens": 900, "audio_tokens": 600, "cached_tokens": 800,
                "cached_tokens_details": {"text_tokens": 700, "audio_tokens": 100}},
            "output_token_details": {"text_tokens": 100, "audio_tokens": 400}
        }))
        .unwrap();
        assert_eq!(
            u,
            Usage {
                text_in: 200,
                audio_in: 500,
                text_cached: 700,
                audio_cached: 100,
                text_out: 100,
                audio_out: 400
            }
        );
    }

    #[test]
    fn events_read_as_ghost_needs_them() {
        let e = event_of(
            &json!({"type": "response.created", "response": {"id": "r1", "metadata": {"ghost": "relay-1"}}}),
        );
        assert_eq!(
            e,
            Some(Event::Started {
                response: "r1".into(),
                tag: Some("relay-1".into())
            })
        );
        let e = event_of(
            &json!({"type": "response.function_call_arguments.done", "response_id": "r2",
            "call_id": "c1", "name": "consult", "arguments": "{\"request\":\"what's open\"}"}),
        );
        assert!(
            matches!(e, Some(Event::Call { ref name, ref args, .. }) if name == "consult" && args["request"] == "what's open")
        );
        let e = event_of(
            &json!({"type": "error", "error": {"type": "invalid_request_error", "code": "conversation_already_has_active_response", "message": "busy"}}),
        );
        assert!(matches!(e, Some(Event::Rejected { .. })));
        let e = event_of(
            &json!({"type": "error", "error": {"type": "invalid_request_error", "code": "rate_limit_exceeded"}}),
        );
        assert!(matches!(e, Some(Event::Failed(Failure::RateLimited(_)))));
        let e = event_of(
            &json!({"type": "error", "error": {"type": "invalid_request_error", "code": "invalid_api_key"}}),
        );
        assert!(matches!(e, Some(Event::Failed(Failure::Refused(_)))));
        assert_eq!(event_of(&json!({"type": "rate_limits.updated"})), None);
    }

    #[test]
    fn the_session_is_set_with_ghosts_turn_rule_and_three_tools() {
        let v = session_update(
            "gpt-realtime-2.1-mini",
            "marin",
            500,
            &super::super::setup(),
        );
        let td = &v["session"]["audio"]["input"]["turn_detection"];
        assert_eq!(td["interrupt_response"], false);
        assert_eq!(td["create_response"], true);
        assert_eq!(v["session"]["audio"]["output"]["voice"], "marin");
        assert_eq!(v["session"]["tools"].as_array().unwrap().len(), 3);
        assert_eq!(v["session"]["output_modalities"], json!(["audio"]));
    }
}
