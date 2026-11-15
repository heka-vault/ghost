//! Developer's live views (ADR 0073, amended 2026-11-14): the timing overlay's `timing`
//! event, one per turn of a live session, and the microphone meter's `meter` event, about
//! fifteen a second while the operator has the meter open.
//!
//! Numbers and fixed kinds only. A `timing` is built from the turn's latency record
//! (`ghost_voice::latency`, the same record `ghostd latency` and the diagnostic report read):
//! stage times, the model's name, its provider, fast or main. A `meter` is levels in dBFS,
//! whether the speech detector hears speech, the room gate's levels, and decisions by kind
//! (`MeterDecisionKind`). Neither type has a field that could carry a word that was said, a
//! transcript or audio: `the_meter_and_timing_carry_no_words` and ghostd's hostile fixtures
//! hold that.
//!
//! The operator's only, on the desktop's own event stream: the phone's stream and the
//! listener for outside tools forward neither (they take a closed list of event kinds), and
//! no tool of Ghost's own app reads them.

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};

// --- the timing overlay ------------------------------------------------------------------

/// One turn of a live session, timed (`Event::Timing`): seconds from when the person stopped
/// talking (a typed turn: from when it was sent), each None where the turn had no such
/// stage. Numbers and names from Ghost's own code and the model's settings, never words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TimingView {
    /// The session it happened in.
    pub session_id: String,
    /// When the record was made, at the turn's end.
    pub at: DateTime<Utc>,
    /// The turn's number in the session.
    pub turn: u64,
    /// `voice`, `text`, `opening` or `system`.
    pub kind: String,
    /// The model that answered first, as the provider names it.
    #[schema(required)]
    pub model: Option<String>,
    /// The provider of Ghost's model connection (`anthropic`, `openrouter`, …).
    #[schema(required)]
    pub provider: Option<String>,
    /// The fast model answered first (ADR 0050); false: the main model.
    pub fast: bool,
    /// The fast model handed the turn to the main one.
    pub handover: bool,
    /// The person's turn closed: hearing, speech to text, the turn's own wait.
    #[schema(required)]
    pub closed: Option<f64>,
    /// Speech to text: the VAD's stop to the transcript.
    #[schema(required)]
    pub stt: Option<f64>,
    /// The model's first word (its first text, else its first byte).
    #[schema(required)]
    pub first_word: Option<f64>,
    /// The first tool call, as the model asked for it.
    #[schema(required)]
    pub first_tool: Option<f64>,
    /// The first tool call's result came back.
    #[schema(required)]
    pub tool_result: Option<f64>,
    /// "One sec." or another acknowledgment, at the speaker.
    #[schema(required)]
    pub ack: Option<f64>,
    /// Ghost's first sound of the reply, at the speaker.
    #[schema(required)]
    pub first_sound: Option<f64>,
    /// The reply ended.
    #[schema(required)]
    pub reply_end: Option<f64>,
    /// The person talked over the reply: from when they began to when Ghost stopped.
    #[schema(required)]
    pub barge_in: Option<f64>,
    /// Model calls in the turn.
    pub calls: u32,
    /// Tool calls in the turn.
    pub tools: u32,
}

fn f(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn add(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(a? + b?)
}

fn round3(x: Option<f64>) -> Option<f64> {
    x.map(|v| (v * 1000.0).round() / 1000.0)
}

/// A turn's latency record as the overlay shows it. `provider` and `fast_model` are the
/// session's model settings. None for a record that is not a turn's (the realtime voice's
/// own exchanges).
pub fn timing_of(
    record: &Value,
    provider: Option<&str>,
    fast_model: Option<&str>,
) -> Option<TimingView> {
    if record.get("kind").and_then(Value::as_str) == Some("realtime") {
        return None;
    }
    let turn = record.get("turn")?.as_u64()?;
    let calls: &[Value] = record
        .get("llm")
        .and_then(|l| l.get("calls"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let tools: &[Value] = record
        .get("tools")
        .and_then(|l| l.get("calls"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let first = calls.first();
    let first_word = first.and_then(|c| {
        add(
            f(c.get("at")),
            f(c.get("first_text")).or_else(|| f(c.get("ttfb"))),
        )
    });
    let first_tool = calls
        .iter()
        .find_map(|c| add(f(c.get("at")), f(c.get("first_tool"))));
    let tool_result = tools
        .first()
        .and_then(|t| add(f(t.get("at")), f(t.get("secs"))));
    let reply = f(record.get("reply"));
    let model = record
        .get("model")
        .and_then(Value::as_str)
        .map(String::from);
    let fast = matches!((model.as_deref(), fast_model), (Some(m), Some(fm)) if m == fm);
    Some(TimingView {
        session_id: String::new(), // the state fills it from the live session
        at: Utc::now(),
        turn,
        kind: record
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| ["voice", "text", "opening", "system"].contains(k))
            .unwrap_or("system")
            .to_string(),
        model,
        provider: provider.filter(|p| !p.is_empty()).map(String::from),
        fast,
        handover: record.get("handover").is_some(),
        closed: f(record.get("hear").and_then(|h| h.get("total"))),
        stt: f(record.get("hear").and_then(|h| h.get("stt"))),
        first_word: round3(first_word),
        first_tool: round3(first_tool),
        tool_result: round3(tool_result),
        ack: f(record.get("ack")),
        first_sound: reply,
        reply_end: round3(add(reply, f(record.get("speech")))),
        barge_in: f(record.get("barge_in")),
        calls: calls.len() as u32,
        tools: tools.len() as u32,
    })
}

// --- the microphone meter ----------------------------------------------------------------

/// What the microphone is doing for Ghost now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeterState {
    /// Listening is off (ADR 0057): no capture is open, so there is nothing to measure.
    ListeningOff,
    /// Waiting for the wake: the passive listener's audio, local models only.
    Passive,
    /// A desk session.
    Session,
    /// Nothing that listens has sent a frame lately (the Hearing test has the microphone, a
    /// typed session, the phone's call, or Ghost is starting).
    Quiet,
}

/// Why the room gate (ADR 0072) runs here or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoomGateWhy {
    /// On: this microphone hears the room nearly as loud as the person.
    ThisMicrophone,
    /// Off: the on-chip array (the reSpeaker XVF3800) aims at the talker itself.
    OnChipArray,
    /// Off or on by `GHOST_ROOM_GATE`.
    Setting,
    /// It runs in a desk session only: none is open.
    NoSession,
}

/// The room gate as the meter sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RoomGateView {
    pub on: bool,
    #[schema(inline)]
    pub why: RoomGateWhy,
    /// The room's loud level, dBFS, once five seconds were heard.
    #[schema(required)]
    pub room: Option<f32>,
    /// The person's level, dBFS, once heard.
    #[schema(required)]
    pub person: Option<f32>,
    /// The line under which a segment is held as the room.
    #[schema(required)]
    pub line: Option<f32>,
}

/// What Ghost decided about a stretch of sound, by kind only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeterDecisionKind {
    /// Read as the person's and passed on (`words` how many).
    Taken,
    /// Read, and no words in it.
    NoWords,
    /// Held as the room: below the person's level.
    HeldRoom,
    /// A held segment let go at its end.
    LetGo,
    /// A held segment that began with Ghost's name, so taken.
    TakenByName,
    /// Dropped as Ghost's own voice coming back.
    Echo,
    /// Heard over Ghost, too short to interrupt it.
    TooShort,
    /// The person talked over Ghost and Ghost stopped.
    BargeIn,
    /// The wake detector scored a hit (`woke` once decided).
    Wake,
    /// A stretch's best wake score, under the threshold.
    NearMiss,
}

/// One decision. Numbers only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MeterDecisionView {
    #[schema(inline)]
    pub kind: MeterDecisionKind,
    /// How long ago, in milliseconds, at the time of the event.
    pub ago_ms: u32,
    /// How many words were read (a count, never the words).
    #[serde(default)]
    pub words: Option<u32>,
    /// The segment's level, dBFS.
    #[serde(default)]
    pub dbfs: Option<f32>,
    /// The person's level when it was held, dBFS.
    #[serde(default)]
    pub person: Option<f32>,
    /// The segment's length, seconds.
    #[serde(default)]
    pub secs: Option<f32>,
    /// The wake detector's score, 0 to 1.
    #[serde(default)]
    pub score: Option<f32>,
    /// A wake hit: woke Ghost, or Parakeet's check refused it; None while undecided.
    #[serde(default)]
    pub woke: Option<bool>,
}

/// One reading of the meter (`Event::Meter`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MeterView {
    /// False on the last one: the meter closed.
    pub open: bool,
    #[schema(inline)]
    pub state: MeterState,
    /// The mean level since the last reading, dBFS.
    #[schema(required)]
    pub level: Option<f32>,
    /// The loudest 20 ms since the last reading, dBFS.
    #[schema(required)]
    pub peak: Option<f32>,
    /// The speech detector hears speech.
    pub speaking: bool,
    pub room_gate: RoomGateView,
    /// What was decided since the last reading, oldest first.
    pub decisions: Vec<MeterDecisionView>,
}

/// How long the meter stays open without the view asking again.
pub const METER_LEASE_SECS: u64 = 10;
/// Readings a second while it is open.
pub const METER_PER_SECOND: u32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MeterRequest {
    /// Open (or keep open) the meter for `METER_LEASE_SECS`; false closes it now.
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MeterLeaseView {
    pub open: bool,
    /// When it closes unless asked again (RFC 3339).
    #[schema(required)]
    pub until: Option<String>,
    /// Readings a second while open.
    pub per_second: u32,
}

/// The daemon's meter (`ghostd::meter`).
pub trait MeterPort: Send + Sync {
    fn set(&self, open: bool) -> MeterLeaseView;
}

/// Open the microphone meter for a few seconds, or close it. The view asks again while it is
/// open; when it stops asking, the meter closes by itself. It opens no microphone: it reads
/// what Ghost's listening already measures.
#[utoipa::path(put, path = "/developer/meter", operation_id = "developer_meter_put",
    tag = "developer", summary = "Open or close the microphone meter", request_body = MeterRequest,
    responses((status = 200, description = "Successful Response", body = MeterLeaseView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_meter(
    State(deps): State<SharedDeps>,
    Body(req): Body<MeterRequest>,
) -> ApiResult<Json<MeterLeaseView>> {
    let port = deps
        .meter
        .clone()
        .ok_or_else(|| ApiError::unavailable("the meter is not available in this daemon"))?;
    Ok(Json(port.set(req.open)))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new().routes(routes!(put_meter))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A voice turn with a tool call, as `ghost_voice::latency::build_record` writes it.
    pub fn a_tool_turn() -> Value {
        json!({
            "v": 1, "ts": "2026-11-14T10:00:00Z", "session": "s1", "origin": "desk",
            "turn": 3, "kind": "voice", "model": "claude-haiku-5-5",
            "reply": 2.41, "spoke": true, "ack": 1.2, "speech": 3.5,
            "hear": {"total": 0.62, "vad": 0.2, "stt": 0.18, "turn": 0.1, "hold": 0.0},
            "handover": {"n": 1, "secs": 0.3},
            "llm": {"total": 1.4, "calls": [
                {"at": 0.65, "ttfb": 0.31, "first_tool": 0.4, "first_text": null, "dur": 0.5,
                 "tools": ["github_issue_read"], "text": false, "thinking": false},
                {"at": 1.5, "ttfb": 0.25, "first_tool": null, "first_text": 0.3, "dur": 0.6,
                 "tools": [], "text": true, "thinking": false}
            ]},
            "tools": {"total": 0.3, "calls": [{"name": "github_issue_read", "at": 1.06, "secs": 0.41}]},
            "tts": {"total": 0.3},
        })
    }

    #[test]
    fn a_tool_turn_reads_as_its_stages() {
        let t = timing_of(&a_tool_turn(), Some("openrouter"), Some("claude-haiku-5-5")).unwrap();
        assert_eq!(t.turn, 3);
        assert_eq!(t.closed, Some(0.62));
        assert_eq!(t.stt, Some(0.18));
        assert_eq!(t.first_word, Some(0.96)); // the first call's first byte: no text in it
        assert_eq!(t.first_tool, Some(1.05));
        assert_eq!(t.tool_result, Some(1.47));
        assert_eq!(t.first_sound, Some(2.41));
        assert_eq!(t.reply_end, Some(5.91));
        assert_eq!(t.ack, Some(1.2));
        assert!(t.fast && t.handover);
        assert_eq!((t.calls, t.tools), (2, 1));
        assert_eq!(t.provider.as_deref(), Some("openrouter"));
    }

    #[test]
    fn what_a_turns_timing_costs() {
        // ADR 0018: the overlay adds this once per turn, after the reply has ended, and only
        // while the desktop listens. Measured 2026-11-14 on the workstation: 4.7 µs in a
        // release build, 15 µs in a debug one; the bound is generous for CI.
        let rec = a_tool_turn();
        let n = 2_000;
        let began = std::time::Instant::now();
        for _ in 0..n {
            let t = timing_of(&rec, Some("anthropic"), Some("claude-haiku-5-5")).unwrap();
            let e = crate::models::Event::Timing { timing: t };
            std::hint::black_box(serde_json::to_string(&e).unwrap());
        }
        let each = began.elapsed() / n;
        eprintln!("a turn's timing: {each:?}");
        assert!(each < std::time::Duration::from_millis(1), "{each:?}");
    }

    #[test]
    fn a_typed_turn_with_no_tools_has_no_hearing_or_tool_times() {
        let rec = json!({"turn": 1, "kind": "text", "model": "claude-opus-5-5", "reply": 1.1,
            "llm": {"calls": [{"at": 0.0, "ttfb": 0.5, "first_text": 0.6}]}});
        let t = timing_of(&rec, Some("anthropic"), Some("claude-haiku-5-5")).unwrap();
        assert_eq!(
            (t.closed, t.stt, t.first_tool, t.tool_result),
            (None, None, None, None)
        );
        assert_eq!(t.first_word, Some(0.6));
        assert!(!t.fast && !t.handover);
        assert_eq!(t.reply_end, None);
    }

    #[test]
    fn a_realtime_exchange_is_not_a_turn() {
        assert!(timing_of(&json!({"realtime": "talk", "secs": 0.4}), None, None).is_none());
        let rt = json!({"kind": "realtime", "turn": 2, "what": "talk"});
        assert!(timing_of(&rt, None, None).is_none());
    }

    #[test]
    fn the_meter_and_timing_carry_no_words() {
        // A record can only reach the overlay through `timing_of`, which keeps numbers and
        // the model's name: a record with words in places a future field might put them
        // (it never should) loses them all.
        let mut rec = a_tool_turn();
        let hostile = "SECRET-TRANSCRIPT the operator said this";
        rec["text"] = json!(hostile);
        rec["hear"]["transcript"] = json!(hostile);
        rec["tools"]["calls"][0]["args"] = json!({"q": hostile});
        rec["kind"] = json!(hostile);
        let t = timing_of(&rec, Some("anthropic"), None).unwrap();
        let out = serde_json::to_string(&t).unwrap();
        assert!(!out.contains("SECRET-TRANSCRIPT"), "{out}");
        // The meter's own types: every string field is a fixed kind.
        let m = MeterView {
            open: true,
            state: MeterState::Session,
            level: Some(-40.0),
            peak: Some(-30.0),
            speaking: true,
            room_gate: RoomGateView {
                on: true,
                why: RoomGateWhy::ThisMicrophone,
                room: Some(-50.0),
                person: Some(-24.0),
                line: Some(-32.0),
            },
            decisions: vec![MeterDecisionView {
                kind: MeterDecisionKind::HeldRoom,
                ago_ms: 40,
                words: None,
                dbfs: Some(-41.0),
                person: Some(-24.0),
                secs: None,
                score: None,
                woke: None,
            }],
        };
        let v = serde_json::to_value(&m).unwrap();
        fn strings(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::String(s) => out.push(s.clone()),
                Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
                Value::Object(o) => o.values().for_each(|x| strings(x, out)),
                _ => {}
            }
        }
        let mut all = Vec::new();
        strings(&v, &mut all);
        all.sort();
        assert_eq!(all, ["held_room", "session", "this_microphone"]);
    }
}
