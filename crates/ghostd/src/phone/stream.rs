//! The phone hears at once (ADR 0051): one long-lived signed stream from the paired phone
//! to the desk, and the signed requests the phone answers with.
//!
//! `GET /stream` is Server-Sent Events. The request is signed like any other; the answer
//! is not buffered: its head is signed as if its body were empty, and every message
//! carries its own signature over the request's nonce and the message's bytes
//! (`signing::sign_event`). The desk pushes what the waiting list did (an item added,
//! changed, resolved), every question as it is asked and settled, an `alert` when the
//! phone route delivers (the phone shows a notification and answers `POST /seen`), a
//! `ring` when the call route rings (`POST /ring` says answered or declined), and a
//! heartbeat. On connect the phone gets a snapshot, then changes from a cursor; given a
//! cursor the desk still has, it gets only what it missed.
//!
//! What never goes on it: audio, a transcript, a turn, a log line. A question answered by
//! voice is shown by its choice, not its words.
//!
//! The phone's answers are signed `POST`s, each with an `id` of its own choosing: the
//! same `id` again gets the same answer and does nothing twice.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use ghost_api::models::{QuestionView, QuestionsView, WaitingItemView};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use tracing::info;

use super::listener::{ActionError, PhoneListener, StreamKey, error, reply};
use super::signing::sign_event;

/// A heartbeat this often while nothing else is said: a tunnel that dropped is noticed by
/// the phone within two of these.
pub const HEARTBEAT_SECS: u64 = 15;
/// How many changes the desk keeps for a phone that comes back.
pub const KEPT: usize = 512;
/// How long the phone route waits for the phone to say it showed an alert.
pub const SEEN_WAIT: Duration = Duration::from_secs(15);
/// How long a ring rings before it is not answered (issue #22).
pub const RING_WAIT: Duration = Duration::from_secs(30);
/// How long the desk remembers a request's `id`.
pub const IDS_KEPT: Duration = Duration::from_secs(600);
pub const MAX_ID: usize = 64;
pub const MAX_ANSWER: usize = 2000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the phone said to a ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingReply {
    Answered,
    Declined,
    /// Nobody answered in time, or no phone was there to ring.
    NoAnswer,
}

/// A ring, as the call route asks for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ring {
    /// The waiting-list key it is about.
    pub key: String,
    /// The agent the phone opens the call with.
    pub agent: String,
    /// The item's title.
    pub title: String,
    /// The question's spoken line, when the item is a question.
    pub question: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct SeenItem {
    changed_at: DateTime<Utc>,
    state: String,
    title: String,
}

struct Log {
    next: u64,
    ring: VecDeque<(u64, Value)>,
}

/// The desk's side of the live stream: a numbered log of what the phone is told, who is
/// listening, and what is waiting on the phone's word.
pub struct PhoneFeed {
    epoch: String,
    log: Mutex<Log>,
    changed: Notify,
    streams: AtomicUsize,
    items: Mutex<HashMap<String, SeenItem>>,
    seen: Mutex<HashMap<String, oneshot::Sender<()>>>,
    rings: Mutex<HashMap<String, oneshot::Sender<RingReply>>>,
    ids: Mutex<HashMap<String, (Instant, StatusCode, Value)>>,
    snapshot: Mutex<Option<Snapshot>>,
}

/// What the waiting list and the questions are now, as the phone may see them.
pub type Snapshot = Arc<dyn Fn() -> (Vec<WaitingItemView>, Vec<QuestionView>) + Send + Sync>;

impl Default for PhoneFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// A question as the phone may see it: an answer given by voice is the operator's words
/// from a session, so only its choice and its state go out (no transcripts on the phone
/// path).
pub fn phone_safe(mut q: QuestionView) -> QuestionView {
    if q.answered_via.as_deref() == Some("voice") {
        q.answer = None;
    }
    q
}

/// The same for an item that carries a question.
pub fn phone_safe_item(mut i: WaitingItemView) -> WaitingItemView {
    i.question = i.question.map(phone_safe);
    i
}

impl PhoneFeed {
    pub fn new() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            log: Mutex::new(Log {
                next: 1,
                ring: VecDeque::with_capacity(KEPT),
            }),
            changed: Notify::new(),
            streams: AtomicUsize::new(0),
            items: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            rings: Mutex::new(HashMap::new()),
            ids: Mutex::new(HashMap::new()),
            snapshot: Mutex::new(None),
        }
    }

    /// Where the snapshot comes from: the daemon's waiting list and questions.
    pub fn set_snapshot(&self, snapshot: Snapshot) {
        *lock(&self.snapshot) = Some(snapshot);
    }

    fn snapshot_now(&self) -> (Vec<WaitingItemView>, Vec<QuestionView>) {
        let f = lock(&self.snapshot).clone();
        let (items, questions) = f.map(|f| f()).unwrap_or_default();
        (
            items.into_iter().map(phone_safe_item).collect(),
            questions.into_iter().map(phone_safe).collect(),
        )
    }

    /// How many phones are listening now (one paired phone; two while it reconnects).
    pub fn streams(&self) -> usize {
        self.streams.load(Ordering::SeqCst)
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// `<epoch>.<seq>`: what the phone sends back to resume.
    pub fn cursor(&self) -> String {
        format!("{}.{}", self.epoch, lock(&self.log).next - 1)
    }

    fn push(&self, mut msg: Value) -> u64 {
        let seq = {
            let mut log = lock(&self.log);
            let seq = log.next;
            log.next += 1;
            if let Some(o) = msg.as_object_mut() {
                o.insert("seq".into(), json!(seq));
                o.insert("cursor".into(), json!(format!("{}.{seq}", self.epoch)));
            }
            if log.ring.len() >= KEPT {
                log.ring.pop_front();
            }
            log.ring.push_back((seq, msg));
            seq
        };
        self.changed.notify_waiters();
        seq
    }

    /// What came after `seq`; None when the desk no longer has all of it.
    fn since(&self, seq: u64) -> Option<Vec<(u64, Value)>> {
        let log = lock(&self.log);
        if seq >= log.next {
            return None; // from another run, or invented
        }
        let oldest = log.ring.front().map_or(log.next, |(n, _)| *n);
        if seq + 1 < oldest {
            return None;
        }
        Some(log.ring.iter().filter(|(n, _)| *n > seq).cloned().collect())
    }

    /// A cursor this run of the desk can resume from: its seq.
    fn resumable(&self, cursor: Option<&str>) -> Option<u64> {
        let (epoch, seq) = cursor?.split_once('.')?;
        (epoch == self.epoch).then_some(())?;
        let seq: u64 = seq.parse().ok()?;
        self.since(seq).map(|_| seq)
    }

    // --- what the phone is told ---------------------------------------------------------

    /// The waiting list now: each item added or changed since the last look is said with
    /// its whole view, each one gone as resolved.
    pub fn waiting_now(&self, items: &[WaitingItemView]) {
        let mut out = Vec::new();
        {
            let mut seen = lock(&self.items);
            let now: HashMap<String, SeenItem> = items
                .iter()
                .map(|i| {
                    (
                        i.key.clone(),
                        SeenItem {
                            changed_at: i.changed_at,
                            state: serde_json::to_value(i.state)
                                .ok()
                                .and_then(|v| v.as_str().map(String::from))
                                .unwrap_or_default(),
                            title: i.title.clone(),
                        },
                    )
                })
                .collect();
            for i in items {
                let change = match seen.get(&i.key) {
                    None => "added",
                    Some(was) if *was != now[&i.key] => "changed",
                    Some(_) => continue,
                };
                out.push(json!({"type": "item", "change": change, "key": i.key,
                                "item": phone_safe_item(i.clone())}));
            }
            for key in seen.keys() {
                if !now.contains_key(key) {
                    out.push(json!({"type": "item", "change": "resolved", "key": key}));
                }
            }
            *seen = now;
        }
        for m in out {
            self.push(m);
        }
    }

    /// A question asked or settled.
    pub fn question(&self, q: QuestionView) {
        self.push(json!({"type": "question", "question": phone_safe(q)}));
    }

    /// The phone route: tell the phone to show these, and wait for it to say it did. True
    /// when it did, in time.
    pub async fn alert(&self, items: &[(String, String)], wait: Duration) -> bool {
        if self.streams() == 0 {
            return false;
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let (tx, rx) = oneshot::channel();
        lock(&self.seen).insert(id.clone(), tx);
        let keys: Vec<&str> = items.iter().map(|(k, _)| k.as_str()).collect();
        let titles: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        self.push(
            json!({"type": "alert", "alert_id": id, "keys": keys, "titles": titles,
                         "count": items.len()}),
        );
        let shown = tokio::time::timeout(wait, rx)
            .await
            .is_ok_and(|r| r.is_ok());
        lock(&self.seen).remove(&id);
        shown
    }

    /// The phone showed an alert.
    pub fn seen(&self, alert_id: &str) -> bool {
        match lock(&self.seen).remove(alert_id) {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// The call route: ring the phone and wait for its answer.
    pub async fn ring(&self, ring: &Ring, wait: Duration) -> RingReply {
        if self.streams() == 0 {
            return RingReply::NoAnswer;
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let (tx, rx) = oneshot::channel();
        lock(&self.rings).insert(id.clone(), tx);
        self.push(json!({
            "type": "ring", "ring_id": id, "key": ring.key, "agent": ring.agent,
            "title": ring.title, "question": ring.question,
            "ring_secs": wait.as_secs(),
        }));
        let reply = match tokio::time::timeout(wait, rx).await {
            Ok(Ok(r)) => r,
            _ => RingReply::NoAnswer,
        };
        lock(&self.rings).remove(&id);
        let why = match reply {
            RingReply::Answered => "answered",
            RingReply::Declined => "declined",
            RingReply::NoAnswer => "timed_out",
        };
        self.push(json!({"type": "ring_over", "ring_id": id, "why": why}));
        reply
    }

    /// The phone answered or declined a ring.
    pub fn ring_reply(&self, ring_id: &str, reply: RingReply) -> bool {
        match lock(&self.rings).remove(ring_id) {
            Some(tx) => tx.send(reply).is_ok(),
            None => false,
        }
    }

    /// The answer an `id` already got, or None to do it now.
    fn done(&self, id: &str) -> Option<(StatusCode, Value)> {
        let mut ids = lock(&self.ids);
        ids.retain(|_, (at, _, _)| at.elapsed() < IDS_KEPT);
        ids.get(id).map(|(_, s, v)| (*s, v.clone()))
    }

    fn remember(&self, id: &str, status: StatusCode, body: &Value) {
        lock(&self.ids).insert(id.to_string(), (Instant::now(), status, body.clone()));
    }

    /// One stream, as the handler writes it: the hello, a snapshot or what was missed,
    /// then each change and a heartbeat, until the phone goes.
    async fn serve(
        self: Arc<Self>,
        key: StreamKey,
        cursor: Option<String>,
        tx: mpsc::Sender<Bytes>,
    ) {
        struct Open<'a>(&'a AtomicUsize);
        impl Drop for Open<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.streams.fetch_add(1, Ordering::SeqCst);
        let _open = Open(&self.streams);
        info!("phone: the live stream is open");
        let send = |msg: &Value| frame(&key, msg);
        let mut at = match self.resumable(cursor.as_deref()) {
            Some(seq) => {
                let hello = json!({"type": "hello", "epoch": self.epoch, "resumed": true,
                                   "heartbeat_secs": HEARTBEAT_SECS});
                if tx.send(send(&hello)).await.is_err() {
                    return;
                }
                seq
            }
            None => {
                // Read the cursor first: a change after it is sent again, never lost.
                let seq = lock(&self.log).next - 1;
                let (items, questions) = self.snapshot_now();
                lock(&self.items).extend(items.iter().map(|i| {
                    (
                        i.key.clone(),
                        SeenItem {
                            changed_at: i.changed_at,
                            state: serde_json::to_value(i.state)
                                .ok()
                                .and_then(|v| v.as_str().map(String::from))
                                .unwrap_or_default(),
                            title: i.title.clone(),
                        },
                    )
                }));
                let hello = json!({"type": "hello", "epoch": self.epoch, "resumed": false,
                                   "heartbeat_secs": HEARTBEAT_SECS});
                let snapshot = json!({"type": "snapshot",
                    "cursor": format!("{}.{seq}", self.epoch),
                    "items": items, "questions": questions});
                if tx.send(send(&hello)).await.is_err() || tx.send(send(&snapshot)).await.is_err() {
                    return;
                }
                seq
            }
        };
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let news = self.since(at).unwrap_or_default();
            if !news.is_empty() {
                for (seq, msg) in news {
                    if tx.send(send(&msg)).await.is_err() {
                        return;
                    }
                    at = seq;
                }
                continue;
            }
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep(Duration::from_secs(HEARTBEAT_SECS)) => {
                    let beat = json!({"type": "heartbeat", "cursor": format!("{}.{at}", self.epoch),
                                      "at": Utc::now()});
                    if tx.send(send(&beat)).await.is_err() {
                        return;
                    }
                }
                () = tx.closed() => return,
            }
        }
    }
}

/// One Server-Sent Event: `event:` the message's type, `id:` its cursor when it has one,
/// `data:` `{"msg": "<the message as JSON text>", "sig": "<signature over those bytes>"}`.
pub fn frame(key: &StreamKey, msg: &Value) -> Bytes {
    let text = msg.to_string();
    let sig = sign_event(&key.secret, &key.nonce, text.as_bytes());
    let data = json!({"msg": text, "sig": sig}).to_string();
    let kind = msg.get("type").and_then(Value::as_str).unwrap_or("message");
    let id = msg
        .get("cursor")
        .and_then(Value::as_str)
        .filter(|_| msg.get("seq").is_some())
        .map(|c| format!("id: {c}\n"))
        .unwrap_or_default();
    Bytes::from(format!("event: {kind}\n{id}data: {data}\n\n"))
}

// --- the routes ----------------------------------------------------------------------------

type Listener = State<Arc<PhoneListener>>;

pub fn routes() -> Router<Arc<PhoneListener>> {
    Router::new()
        .route("/stream", get(stream))
        .route("/questions", get(questions))
        .route("/questions/answer", post(answer))
        .route("/questions/decline", post(decline))
        .route("/changes/confirm", post(confirm_change))
        .route("/changes/cancel", post(cancel_change))
        .route("/seen", post(seen))
        .route("/ring", post(ring_reply))
}

fn cursor_of(query: Option<&str>) -> Option<String> {
    query?
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "cursor")
        .map(|(_, v)| v.to_string())
        .filter(|v| v.len() <= 64)
}

async fn stream(
    State(l): Listener,
    RawQuery(query): RawQuery,
    req: axum::extract::Request,
) -> Response {
    let Some(feed) = l.host.feed() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, None);
    };
    let Some(key) = req.extensions().get::<StreamKey>().cloned() else {
        return error(StatusCode::UNAUTHORIZED, None);
    };
    let (tx, mut rx) = mpsc::channel::<Bytes>(64);
    tokio::spawn(feed.serve(key, cursor_of(query.as_deref()), tx));
    let body = futures::stream::poll_fn(move |cx| {
        rx.poll_recv(cx).map(|b| b.map(Ok::<Bytes, Infallible>))
    });
    let mut resp = Response::new(Body::from_stream(body));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp
}

async fn questions(State(l): Listener) -> Response {
    let mut view: QuestionsView = l.host.questions();
    view.questions = view.questions.into_iter().map(phone_safe).collect();
    reply(StatusCode::OK, view)
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Option<T> {
    serde_json::from_slice(body).ok()
}

fn id_ok(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.chars().count())
}

/// Run `act` once per `id`: the same `id` again answers what it answered.
async fn once<F, Fut>(l: &PhoneListener, id: &str, act: F) -> Response
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = (StatusCode, Value)>,
{
    let feed = l.host.feed();
    if let Some((status, body)) = feed.as_ref().and_then(|f| f.done(id)) {
        return reply(status, body);
    }
    let (status, body) = act().await;
    if let Some(f) = &feed {
        f.remember(id, status, &body);
    }
    reply(status, body)
}

fn refused(e: ActionError) -> (StatusCode, Value) {
    match e {
        ActionError::NotFound => (StatusCode::NOT_FOUND, json!({"error": "not_found"})),
        ActionError::Settled(state) => (
            StatusCode::CONFLICT,
            json!({"error": "settled", "state": state}),
        ),
        ActionError::BadAnswer(why) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error": "bad_answer", "detail": why}),
        ),
    }
}

#[derive(Deserialize)]
struct AnswerBody {
    id: String,
    key: String,
    text: String,
}

async fn answer(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<AnswerBody>(&body).filter(|r| {
        id_ok(&r.id)
            && (1..=super::routes::MAX_KEY).contains(&r.key.chars().count())
            && (1..=MAX_ANSWER).contains(&r.text.chars().count())
    }) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let host = l.host.clone();
    once(&l, &req.id, || async move {
        match host.answer_question(&req.key, &req.text) {
            Ok(q) => (
                StatusCode::OK,
                json!({"ok": true, "question": phone_safe(q)}),
            ),
            Err(e) => refused(e),
        }
    })
    .await
}

#[derive(Deserialize)]
struct KeyBody {
    id: String,
    key: String,
}

async fn decline(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<KeyBody>(&body)
        .filter(|r| id_ok(&r.id) && (1..=super::routes::MAX_KEY).contains(&r.key.chars().count()))
    else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let host = l.host.clone();
    once(&l, &req.id, || async move {
        match host.decline_question(&req.key) {
            Ok(q) => (
                StatusCode::OK,
                json!({"ok": true, "question": phone_safe(q)}),
            ),
            Err(e) => refused(e),
        }
    })
    .await
}

#[derive(Deserialize)]
struct ChangeBody {
    id: String,
    change: String,
}

fn change_body(body: &Bytes) -> Option<ChangeBody> {
    parse::<ChangeBody>(body).filter(|r| id_ok(&r.id) && (1..=100).contains(&r.change.len()))
}

/// A change waiting for a yes, confirmed from the paired phone: the gate's own confirm by
/// id, recorded with the classifier `phone`. Evidence only because the request is signed
/// by the paired device.
async fn confirm_change(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = change_body(&body) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let host = l.host.clone();
    once(&l, &req.id, || async move {
        match host.confirm_change(&req.change).await {
            Some(outcome) => (StatusCode::OK, json!({"ok": true, "outcome": outcome})),
            None => (StatusCode::CONFLICT, json!({"error": "not_waiting"})),
        }
    })
    .await
}

async fn cancel_change(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = change_body(&body) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let host = l.host.clone();
    once(&l, &req.id, || async move {
        if host.cancel_change(&req.change).await {
            (StatusCode::OK, json!({"ok": true}))
        } else {
            (StatusCode::CONFLICT, json!({"error": "not_waiting"}))
        }
    })
    .await
}

#[derive(Deserialize)]
struct SeenBody {
    alert_id: String,
}

/// The phone showed an alert's notification: that, and only that, is the phone route's
/// delivery.
async fn seen(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<SeenBody>(&body).filter(|r| id_ok(&r.alert_id)) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let ok = l.host.feed().is_some_and(|f| f.seen(&req.alert_id));
    reply(StatusCode::OK, json!({"ok": ok}))
}

#[derive(Deserialize)]
struct RingBody {
    ring_id: String,
    answer: String,
}

async fn ring_reply(State(l): Listener, body: Bytes) -> Response {
    let Some(req) = parse::<RingBody>(&body).filter(|r| id_ok(&r.ring_id)) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, None);
    };
    let reply_ = match req.answer.as_str() {
        "answered" => RingReply::Answered,
        "declined" => RingReply::Declined,
        _ => return error(StatusCode::UNPROCESSABLE_ENTITY, None),
    };
    let ok = l
        .host
        .feed()
        .is_some_and(|f| f.ring_reply(&req.ring_id, reply_));
    reply(StatusCode::OK, json!({"ok": ok}))
}
