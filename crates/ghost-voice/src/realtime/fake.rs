//! A scripted realtime provider on a local WebSocket, speaking the subset of OpenAI's
//! Realtime protocol Ghost uses (ADR 0063): the tests' and the harness's stand-in, so every
//! path is exercised with no key and no network. It checks the key Ghost sends, records
//! every client event but the audio (which it only counts), answers `response.create` by
//! saying the instructions' words back (or a misreading of them, when told to), and plays
//! what a test scripts: the operator's speech heard, small talk, a consult with its few
//! words first, a goodbye, an error, a dropped connection.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

use super::SAY_EXACTLY;

/// Seconds of audio the fake says per word.
pub const WORD_SECS: f64 = 0.12;

#[derive(Default)]
struct State {
    received: Vec<Value>,
    audio_bytes: usize,
    auto_reply: bool,
    connections: usize,
    /// Every Authorization header seen, whether it was right or not.
    authorizations: usize,
    wrong_key: usize,
    misread: Option<(String, String)>,
    reject_status: Option<u16>,
    drop_mid_relay: bool,
    responses: u64,
    cancelled: HashSet<String>,
    active: Option<String>,
    conn: Option<mpsc::UnboundedSender<Cmd>>,
    /// Pause between events, so a reply streams faster than it plays.
    pace_ms: u64,
    /// The last written turn (`conversation.item.create` of a message).
    asked: Option<String>,
    /// Replies to speech it hears itself, as a provider's VAD would: after sound, a half
    /// second of silence ends the turn and the next of these answers it.
    on_speech: std::collections::VecDeque<Reply>,
    loud: bool,
    quiet_appends: usize,
}

enum Cmd {
    Send(Value),
    Close,
}

/// What the fake's next scripted response is.
#[derive(Clone, Debug)]
pub enum Reply {
    /// Words the realtime model says on its own.
    Talk(String),
    /// A few words, then a call to `consult` with this request.
    Consult { ack: String, request: String },
    /// A goodbye, then `hang_up`.
    HangUp(String),
}

#[derive(Clone)]
pub struct FakeRealtime {
    pub url: String,
    key: String,
    state: Arc<Mutex<State>>,
}

impl FakeRealtime {
    pub async fn start(key: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("ws://{}/v1/realtime", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            auto_reply: true,
            pace_ms: 4,
            ..State::default()
        }));
        let me = Self {
            url,
            key: key.to_string(),
            state: state.clone(),
        };
        let key = me.key.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = state.clone();
                let key = key.clone();
                tokio::spawn(async move { serve(stream, state, key).await });
            }
        });
        me
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }

    /// Every client event received, but the audio.
    pub fn received(&self) -> Vec<Value> {
        self.with(|s| s.received.clone())
    }

    /// The client events of one type.
    pub fn of(&self, kind: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|v| v["type"] == kind)
            .collect()
    }

    pub fn audio_bytes(&self) -> usize {
        self.with(|s| s.audio_bytes)
    }

    pub fn connections(&self) -> usize {
        self.with(|s| s.connections)
    }

    pub fn wrong_keys(&self) -> usize {
        self.with(|s| s.wrong_key)
    }

    pub fn auto_reply(&self) -> bool {
        self.with(|s| s.auto_reply)
    }

    /// Refuse the next handshakes with this HTTP status (401, 429, …).
    pub fn reject_with(&self, status: Option<u16>) {
        self.with(|s| s.reject_status = status);
    }

    /// Say `heard` where a relay's text has `word`.
    pub fn misread(&self, word: &str, heard: &str) {
        self.with(|s| s.misread = Some((word.to_string(), heard.to_string())));
    }

    /// Answer speech it hears with these, in order (the timing harness's tests).
    pub fn answer_speech(&self, replies: Vec<Reply>) {
        self.with(|s| s.on_speech.extend(replies));
    }

    /// Close the connection halfway through the next relay.
    pub fn drop_mid_relay(&self) {
        self.with(|s| s.drop_mid_relay = true);
    }

    fn send(&self, v: Value) {
        if let Some(c) = self.with(|s| s.conn.clone()) {
            let _ = c.send(Cmd::Send(v));
        }
    }

    /// Close the live connection, as a provider that went away.
    pub fn drop_connection(&self) {
        if let Some(c) = self.with(|s| s.conn.take()) {
            let _ = c.send(Cmd::Close);
        }
    }

    /// An `error` event, as the provider would send it.
    pub fn error(&self, kind: &str, code: &str) {
        self.send(
            json!({"type": "error", "error": {"type": kind, "code": code, "message": "scripted"}}),
        );
    }

    /// The provider's VAD heard the operator; with automatic replies on, `reply` follows.
    pub async fn heard(&self, reply: Option<Reply>) {
        self.send(json!({"type": "input_audio_buffer.speech_started", "audio_start_ms": 0, "item_id": "in"}));
        self.send(json!({"type": "input_audio_buffer.speech_stopped", "audio_end_ms": 500, "item_id": "in"}));
        if let Some(r) = reply
            && self.auto_reply()
        {
            self.respond(r, None).await;
        }
    }

    async fn respond(&self, reply: Reply, tag: Option<String>) {
        let Some(conn) = self.with(|s| s.conn.clone()) else {
            return;
        };
        let (id, pace, drop_mid) = self.with(|s| {
            s.responses += 1;
            let id = format!("resp_{}", s.responses);
            s.active = Some(id.clone());
            let drop_mid = tag.is_some() && std::mem::take(&mut s.drop_mid_relay);
            (id, s.pace_ms, drop_mid)
        });
        let state = self.state.clone();
        tokio::spawn(speak(conn, state, id, reply, tag, pace, drop_mid));
    }
}

async fn speak(
    conn: mpsc::UnboundedSender<Cmd>,
    state: Arc<Mutex<State>>,
    id: String,
    reply: Reply,
    tag: Option<String>,
    pace: u64,
    drop_mid: bool,
) {
    let item = format!("item_{id}");
    let cancelled = |state: &Arc<Mutex<State>>| state.lock().unwrap().cancelled.contains(&id);
    let send = |v: Value| {
        let _ = conn.send(Cmd::Send(v));
    };
    let meta = match &tag {
        Some(t) => json!({"ghost": t}),
        None => Value::Null,
    };
    send(
        json!({"type": "response.created", "response": {"id": id, "status": "in_progress", "metadata": meta}}),
    );
    let (words, call): (String, Option<(String, Value)>) = match reply {
        Reply::Talk(t) => (t, None),
        Reply::Consult { ack, request } => {
            (ack, Some(("consult".into(), json!({"request": request}))))
        }
        Reply::HangUp(t) => (t, Some(("hang_up".into(), json!({})))),
    };
    let list: Vec<&str> = words.split_whitespace().collect();
    let mut spoken = String::new();
    let mut status = "completed";
    for (i, w) in list.iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(pace)).await;
        if cancelled(&state) {
            status = "cancelled";
            break;
        }
        if drop_mid && i == list.len() / 2 {
            let _ = conn.send(Cmd::Close);
            return;
        }
        let piece = if i + 1 < list.len() {
            format!("{w} ")
        } else {
            w.to_string()
        };
        spoken.push_str(&piece);
        send(
            json!({"type": "response.output_audio_transcript.delta", "response_id": id, "item_id": item, "delta": piece}),
        );
        let pcm = vec![12000i16; (24000.0 * WORD_SECS) as usize];
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        send(
            json!({"type": "response.output_audio.delta", "response_id": id, "item_id": item, "delta": B64.encode(bytes)}),
        );
    }
    if status == "completed" {
        send(
            json!({"type": "response.output_audio_transcript.done", "response_id": id, "item_id": item, "transcript": spoken}),
        );
        if let Some((name, args)) = call {
            send(
                json!({"type": "response.function_call_arguments.done", "response_id": id,
                "item_id": format!("call_item_{id}"), "call_id": format!("call_{id}"), "name": name,
                "arguments": args.to_string()}),
            );
        }
    } else {
        send(
            json!({"type": "response.output_audio_transcript.done", "response_id": id, "item_id": item, "transcript": spoken}),
        );
    }
    let n = list.len() as u64;
    send(
        json!({"type": "response.done", "response": {"id": id, "status": status, "usage": {
            "total_tokens": 600 + n * 4, "input_tokens": 500, "output_tokens": 100 + n * 4,
            "input_token_details": {"text_tokens": 400, "audio_tokens": 100, "cached_tokens": 300,
                "cached_tokens_details": {"text_tokens": 300, "audio_tokens": 0}},
            "output_token_details": {"text_tokens": n, "audio_tokens": n * 3}
        }}}),
    );
    let mut s = state.lock().unwrap();
    if s.active.as_deref() == Some(id.as_str()) {
        s.active = None;
    }
}

async fn serve(stream: tokio::net::TcpStream, state: Arc<Mutex<State>>, key: String) {
    let st = state.clone();
    let check = move |req: &Request, resp: Response| {
        let mut s = st.lock().unwrap();
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        s.authorizations += 1;
        let status = if auth != format!("Bearer {key}") {
            s.wrong_key += 1;
            Some(401)
        } else {
            s.reject_status
        };
        if let Some(status) = status {
            let mut e = ErrorResponse::new(Some("no".into()));
            *e.status_mut() = status.try_into().unwrap();
            return Err(e);
        }
        Ok(resp)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
    {
        let mut s = state.lock().unwrap();
        s.connections += 1;
        s.conn = Some(tx.clone());
        s.auto_reply = true;
    }
    let _ = tx.send(Cmd::Send(
        json!({"type": "session.created", "session": {"id": "sess_fake", "type": "realtime"}}),
    ));
    let writer = tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Send(v) => {
                    if sink
                        .send(WsMessage::Text(v.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Cmd::Close => {
                    let _ = sink.close().await;
                    break;
                }
            }
        }
    });
    let me = FakeRealtime {
        url: String::new(),
        key: String::new(),
        state: state.clone(),
    };
    while let Some(Ok(msg)) = source.next().await {
        let WsMessage::Text(raw) = msg else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "input_audio_buffer.append" => {
                let bytes = B64
                    .decode(v["audio"].as_str().unwrap_or(""))
                    .unwrap_or_default();
                let sound = bytes
                    .chunks_exact(2)
                    .any(|c| i16::from_le_bytes([c[0], c[1]]).abs() > 300);
                let ended = {
                    let mut s = state.lock().unwrap();
                    s.audio_bytes += bytes.len();
                    if sound {
                        s.loud = true;
                        s.quiet_appends = 0;
                        false
                    } else if s.loud && !s.on_speech.is_empty() {
                        s.quiet_appends += 1;
                        if s.quiet_appends >= 25 {
                            s.loud = false;
                            s.quiet_appends = 0;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if ended {
                    let reply = state.lock().unwrap().on_speech.pop_front();
                    me.heard(reply).await;
                }
                continue;
            }
            "session.update" => {
                if let Some(on) =
                    v["session"]["audio"]["input"]["turn_detection"]["create_response"].as_bool()
                {
                    state.lock().unwrap().auto_reply = on;
                }
                let _ = tx.send(Cmd::Send(
                    json!({"type": "session.updated", "session": v["session"]}),
                ));
            }
            "response.cancel" => {
                let mut s = state.lock().unwrap();
                if let Some(a) = s.active.clone() {
                    s.cancelled.insert(a);
                }
            }
            "conversation.item.create" if v["item"]["type"] == "message" => {
                let text = v["item"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                state.lock().unwrap().asked = Some(text);
            }
            "response.create" if !v["response"]["instructions"].is_string() => {
                // A written turn: the voice hands anything real to the agent.
                let asked = state.lock().unwrap().asked.take().unwrap_or_default();
                me.respond(
                    Reply::Consult {
                        ack: "One sec.".into(),
                        request: asked,
                    },
                    None,
                )
                .await;
            }
            "response.create" => {
                let instructions = v["response"]["instructions"].as_str().unwrap_or("");
                let tag = v["response"]["metadata"]["ghost"]
                    .as_str()
                    .map(str::to_string);
                let text = instructions
                    .strip_prefix(SAY_EXACTLY)
                    .unwrap_or(instructions)
                    .to_string();
                let said = match state.lock().unwrap().misread.clone() {
                    Some((w, heard)) => text.replacen(&w, &heard, 1),
                    None => text,
                };
                let busy = state.lock().unwrap().active.is_some();
                if busy {
                    let _ = tx.send(Cmd::Send(
                        json!({"type": "error", "error": {"type": "invalid_request_error",
                        "code": "conversation_already_has_active_response", "message": "busy"}}),
                    ));
                } else {
                    me.respond(Reply::Talk(said), tag).await;
                }
            }
            _ => {}
        }
        state.lock().unwrap().received.push(v);
    }
    let mut s = state.lock().unwrap();
    if s.conn.as_ref().is_some_and(|c| c.same_channel(&tx)) {
        s.conn = None;
    }
    drop(s);
    writer.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::openai::OpenAiRealtime;
    use crate::realtime::{Event, Failure, RealtimeProvider, setup};
    use zeroize::Zeroizing;

    fn client(f: &FakeRealtime, key: &str) -> OpenAiRealtime {
        OpenAiRealtime::new(
            f.url.clone(),
            Zeroizing::new(key.into()),
            "gpt-realtime-2.1-mini",
            "marin",
            500,
            None,
        )
    }

    #[tokio::test]
    async fn the_client_connects_with_its_key_sets_the_session_and_hears_a_reply() {
        let f = FakeRealtime::start("sk-fake").await;
        let mut c = client(&f, "sk-fake")
            .connect(&setup())
            .await
            .expect("connected");
        c.link.audio(&[0; 320]);
        f.heard(Some(Reply::Talk("Hi there.".into()))).await;
        let mut said = String::new();
        let mut done = false;
        while let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(2), c.events.recv()).await
        {
            match e {
                Event::Said { delta, .. } => said.push_str(&delta),
                Event::Done { usage, .. } => {
                    assert!(usage.is_some());
                    done = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(done);
        assert_eq!(said, "Hi there.");
        assert_eq!(f.of("session.update").len(), 1);
        assert_eq!(f.audio_bytes(), 480 * 2, "16 kHz in, 24 kHz out");
    }

    #[tokio::test]
    async fn a_wrong_key_is_refused_and_a_rate_limit_is_said_as_one() {
        let f = FakeRealtime::start("sk-fake").await;
        let e = client(&f, "sk-wrong")
            .connect(&setup())
            .await
            .err()
            .unwrap();
        assert!(matches!(e, Failure::Refused(_)), "{e:?}");
        f.reject_with(Some(429));
        let e = client(&f, "sk-fake").connect(&setup()).await.err().unwrap();
        assert!(matches!(e, Failure::RateLimited(_)), "{e:?}");
        assert!(!format!("{e:?}").contains("sk-"));
    }
}
