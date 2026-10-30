//! One session, from wake to end (`Daemon._conversation`): the line it opens in, the
//! first utterance, the typed turn, the chime and ducking around it, the record, and the
//! phone's helpers beside the media (ADR 0016).
//!
//! With a phone the audio is the phone's: no ducking, no desk chime, the thinking sound in
//! its track, and the session's events go to it over RTVI. `start_in` is the agent the
//! session opens in: Ghost, or the one the phone called directly. With `raised` Ghost
//! started it (ADR 0032): it greets with the operator's phrase, the items wait in the
//! context, and a reply within the window is the acknowledgment; silence ends it with
//! nothing kept.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use ghost_api::models::{SessionOrigin, TurnRole, TurnVia};
use ghost_api::{Phase, status_snapshot};
use ghost_core::conversations::{GHOST, Profile};
use ghost_core::routes::{Delivery, SpokenDigest};
use ghost_core::source::EventKind;
use ghost_infer::wake::parse_phrases;
use ghost_rtc::{ClientMessage, Link, Media as RtcMedia, Outbound};
use ghost_voice::conversation::OnSwitch;
use ghost_voice::intercept::SessionAction;
use ghost_voice::pipeline::{GhostSession, Greeting, SessionHooks, SessionParams, build_session};
use ghost_voice::tap::{OnPhase, OnTurn, SessionPhase};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::daemon::{Daemon, DeskMediaRequest, Heard, PendingPhone, SessionStart, lock};
use crate::phone::routes::{
    PHONE_CONNECT_SECS, PHONE_GREETING_LEAD_SECS, PHONE_PING_GRACE_SECS, PHONE_READY_SECS,
    PHONE_WIND_DOWN_SECS, PhoneEvents, forward_events, watchdog,
};
use crate::phone_host::{PhoneEventsSink, PhoneRtvi, phone_media};
use crate::routes::{
    GREETING_SECS, REPLY_WINDOW_SECS, Raised, ReplySession, context_note, digest_note, reply_window,
};
use crate::wake::Event;

/// The running session, as desk voice's window ends it on silence.
struct Ending(Arc<GhostSession>);

#[async_trait]
impl ReplySession for Ending {
    async fn cancel(&self) {
        self.0.cancel().await;
    }
}

fn role_of(role: &str) -> Option<TurnRole> {
    match role {
        "user" => Some(TurnRole::User),
        "assistant" => Some(TurnRole::Assistant),
        "system" => Some(TurnRole::System),
        _ => None,
    }
}

fn phase_of(phase: SessionPhase) -> Phase {
    match phase {
        SessionPhase::Listening => Phase::Listening,
        SessionPhase::Thinking => Phase::Thinking,
        SessionPhase::Speaking => Phase::Speaking,
        SessionPhase::Hold => Phase::Hold,
    }
}

/// What desk voice answers the route with, once: the window's result when it has one,
/// `not_heard` when the session ended first.
struct Answer(Mutex<Option<oneshot::Sender<Delivery>>>);

impl Answer {
    fn settle(&self, d: Delivery) {
        if let Some(tx) = lock(&self.0).take() {
            let _ = tx.send(d);
        }
    }
}

pub async fn conversation(d: &Arc<Daemon>, start: SessionStart) {
    let SessionStart {
        first_utterance,
        phone,
        start_in,
        raised,
    } = start;
    let origin = if phone.is_some() {
        SessionOrigin::Phone
    } else {
        SessionOrigin::Desk
    };
    let origin_name = match origin {
        SessionOrigin::Phone => "phone",
        SessionOrigin::Desk => "desk",
    };
    d.note_passwords_in_use();
    let cfg = d.cfg();
    let voice = d.voice();
    let ducker = lock(&d.ducker).clone();
    let already_chimed = d.chimed.swap(false, Ordering::SeqCst);
    let (media, phone) = match phone {
        None => {
            if !already_chimed && let Some(du) = &ducker {
                du.duck();
                if cfg.pause_players {
                    d.ports.players.pause_playing().await;
                }
            }
            let opened = (d.ports.desk_media)(DeskMediaRequest {
                audio: d.audio_settings(false), // ducked here, not by the devices
                thinking_sound: voice.thinking_sound,
                chime: cfg.chime && !already_chimed,
            })
            .await;
            match opened {
                Ok(m) => (m, None),
                Err(e) => {
                    error!("session: the desk's devices did not open: {e}");
                    if let Some(du) = &ducker {
                        du.restore();
                    }
                    d.ports.players.resume().await;
                    return;
                }
            }
        }
        Some(p) => match phone_media(&p.media, &voice) {
            Some(m) => (m, Some(p)),
            None => {
                error!("session: the phone's media was already taken");
                p.media.close().await;
                return;
            }
        },
    };

    // ADR 0031: a bare wake in Ghost opens with what is waiting, said before anything
    // else (the digest); a request in the same breath is answered instead. The greeting
    // going out delivers them; the operator's answer acknowledges only the titles said
    // (ADR 0029), so a voice that failed or "and two more" leaves them alerting.
    let routes = d.routes();
    let waiting_now = match &routes {
        Some(r) if start_in == GHOST && first_utterance.is_none() && raised.is_none() => {
            r.digest(None).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let digest: Arc<Mutex<Option<SpokenDigest>>> =
        Arc::new(Mutex::new(match (&routes, waiting_now.is_empty()) {
            (Some(r), false) => Some(SpokenDigest::new(r.clone(), &waiting_now)),
            _ => None,
        }));
    let (greeting, context_note) = if let Some(r) = &raised {
        (
            Greeting::Text(r.policy.greeting.clone()),
            Some(context_note(&r.items)),
        )
    } else if let Some(dg) = lock(&digest).as_ref() {
        (Greeting::Text(dg.text.clone()), Some(digest_note()))
    } else {
        (Greeting::Varied, None)
    };
    let replied = Arc::new(Event::new());
    let first_reply: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let heard: Heard = {
        let d = Arc::downgrade(d);
        let digest = digest.clone();
        let replied = replied.clone();
        let first_reply = first_reply.clone();
        let raised = raised.is_some();
        Arc::new(move |role: &'static str, text: String| {
            let d = d.clone();
            let digest = digest.clone();
            let replied = replied.clone();
            let first_reply = first_reply.clone();
            Box::pin(async move {
                if let Some(dg) = lock(&digest).as_mut() {
                    let _ = dg.on_turn(role, &text, None); // the greeting went out; the operator answered
                }
                if role == "user" {
                    if let Some(d) = d.upgrade() {
                        d.refresh_pending();
                    }
                    if raised && !replied.is_set() {
                        *lock(&first_reply) = Some(text);
                        replied.set();
                    }
                }
            })
        })
    };
    let on_turn: OnTurn = {
        let d = Arc::downgrade(d);
        let heard = heard.clone();
        Arc::new(move |role, text| {
            let d = d.clone();
            let heard = heard.clone();
            Box::pin(async move {
                if let Some(d) = d.upgrade()
                    && let Some(r) = role_of(role)
                {
                    d.state.add_turn(r, &text, TurnVia::Voice);
                }
                heard(role, text).await;
            })
        })
    };
    let on_phase: OnPhase = {
        let d = Arc::downgrade(d);
        Arc::new(move |phase| {
            let d = d.clone();
            Box::pin(async move {
                if let Some(d) = d.upgrade() {
                    let id = d.state.session_id();
                    d.state.set_phase(phase_of(phase), id.as_deref(), None);
                    if phase == SessionPhase::Listening {
                        d.refresh_pending();
                    }
                }
            })
        })
    };
    let on_conversation: OnSwitch = {
        let d = Arc::downgrade(d);
        Arc::new(move |p: Profile| {
            let d = d.clone();
            Box::pin(async move {
                if let Some(d) = d.upgrade() {
                    d.state.set_conversation(Some(&p.slug), None);
                    d.refresh_pending();
                }
            })
        })
    };
    let on_hang_up: SessionAction = {
        let d = Arc::downgrade(d);
        Arc::new(move || {
            let d = d.clone();
            Box::pin(async move {
                if let Some(d) = d.upgrade() {
                    d.state.set_ending(); // the phone and the desktop app show "Hanging up"
                }
            })
        })
    };
    let model = match d.model() {
        Ok(m) => m,
        Err(e) => {
            error!("session: no model client: {e}");
            if let Some(du) = &ducker {
                du.restore();
            }
            d.ports.players.resume().await;
            if let Some(p) = phone {
                p.media.close().await;
            }
            return;
        }
    };
    let (Some(gate), Some(models)) = (d.gate(), d.models()) else {
        error!("session: the daemon is not running");
        if let Some(du) = &ducker {
            du.restore();
        }
        d.ports.players.resume().await;
        if let Some(p) = phone {
            p.media.close().await;
        }
        return;
    };
    let mut sources = d.apps().map(|a| a.sources()).unwrap_or_default();
    if let Some(s) = d.late.skills.get() {
        sources.push(s.source());
    }
    let events = d.events();
    let session = Arc::new(build_session(SessionParams {
        settings: voice.clone(),
        models,
        model,
        sources,
        gate: gate.clone(),
        events: Some(events.clone()),
        media,
        greeting,
        first_utterance: first_utterance.clone(),
        hooks: SessionHooks {
            on_turn: Some(on_turn),
            on_phase: Some(on_phase),
            on_hang_up: Some(on_hang_up),
            on_conversation: Some(on_conversation),
            on_latency: None,
        },
        wake_phrases: Some(parse_phrases(Some(&cfg.wake_phrases))),
        wake_fuzzy: cfg.wake_fuzzy,
        conversations: d.conversations(),
        start_in: start_in.clone(),
        latency_log: Some(cfg.latency_path()),
        reach: d.reach(),
        context_note,
    }));
    if let Some(p) = &phone {
        session.set_rtvi(Arc::new(PhoneRtvi(p.media.clone())));
    }
    let session_id = session.session_id.clone();
    *lock(&d.session) = Some(session.clone());
    *lock(&d.heard) = Some(heard);
    if origin == SessionOrigin::Desk {
        *lock(&d.last_desk_session) = Some(Instant::now());
    }
    *lock(&d.origin) = Some(origin);
    d.state
        .set_phase(Phase::Listening, Some(&session_id), Some(origin));
    d.state.set_conversation(Some(&start_in), None);
    if let Some(first) = &first_utterance {
        let via = std::mem::replace(&mut *lock(&d.first_via), TurnVia::Voice);
        d.state.add_turn(TurnRole::User, first, via);
    }
    *lock(&d.first_via) = TurnVia::Voice;
    info!("session {session_id} start ({origin_name})");
    let mut data = Map::new();
    data.insert("session_id".into(), json!(session_id));
    data.insert("origin".into(), json!(origin_name));
    events.emit(EventKind::SessionStart, data).await;

    let mut helpers: Vec<JoinHandle<()>> = Vec::new();
    let mut waits: Vec<BoxFuture<'static, ()>> = vec![{
        let stop = d.stop.clone();
        Box::pin(async move { stop.cancelled().await })
    }];
    let peer_gone = Arc::new(Event::new());
    if let Some(p) = &phone {
        helpers = phone_helpers(d, &session, &p.media, peer_gone.clone());
        let gone = peer_gone.clone();
        waits.push(Box::pin(async move { gone.wait().await }));
        let d2 = d.clone();
        waits.push(Box::pin(async move { d2.phone_end.wait().await }));
    }
    let answer = Arc::new(Answer(Mutex::new(None)));
    if let Some(raised) = raised {
        d.state
            .set_phase(Phase::Asking, Some(&session_id), Some(origin));
        let Raised {
            items,
            policy,
            reply,
        } = raised;
        *lock(&answer.0) = Some(reply);
        let (inner_tx, inner_rx) = oneshot::channel();
        let window = Duration::from_secs_f64(REPLY_WINDOW_SECS + GREETING_SECS);
        let waiting = d.waiting();
        let s = Ending(session.clone());
        let replied_words = {
            let replied = replied.clone();
            let first_reply = first_reply.clone();
            async move {
                replied.wait().await;
                lock(&first_reply).clone().unwrap_or_default()
            }
        };
        helpers.push(tokio::spawn(async move {
            reply_window(
                &s,
                Raised {
                    items,
                    policy,
                    reply: inner_tx,
                },
                replied_words,
                waiting.as_ref().map(|w| &w.items),
                window,
            )
            .await;
        }));
        helpers.push(tokio::spawn({
            let answer = answer.clone();
            async move {
                if let Ok(r) = inner_rx.await {
                    answer.settle(r);
                }
            }
        }));
    }

    session.start(phone.is_none()).await;
    let mut run_task = {
        let s = session.clone();
        tokio::spawn(async move { s.run().await })
    };
    let ran = tokio::select! {
        _ = &mut run_task => true,
        _ = futures::future::select_all(waits) => false,
    };
    if !ran {
        if phone.is_some() && !d.stop.is_cancelled() {
            let why = if d.phone_end.is_set() {
                "phone ended it"
            } else {
                "phone went away"
            };
            info!("session {session_id}: {why}");
        }
        // Cancel rather than end: for the phone an end would wait on writes to a dead
        // peer. Safe because each session forks its own stages from the models, so a
        // cancelled one can't leave the next one mute.
        session.cancel().await;
        if tokio::time::timeout(Duration::from_secs_f64(PHONE_WIND_DOWN_SECS), &mut run_task)
            .await
            .is_err()
        {
            // The pipeline is still draining to a dead peer; let it finish on its own so
            // the desk is free for the next Start now.
            info!("session {session_id}: still winding down; moving on");
        }
    }

    for h in helpers {
        h.abort();
    }
    answer.settle(Delivery::NotHeard);
    match &phone {
        Some(p) => {
            p.media.close().await; // the session ending closes the peer
            d.phone_end.clear();
            *lock(&d.live_phone) = None;
        }
        None => {
            if cfg.chime {
                let play = d.ports.play.clone();
                if let Err(e) = play(
                    ghost_audio::chime(false),
                    ghost_audio::CHIME_RATE,
                    voice.output_device.clone(),
                )
                .await
                {
                    tracing::warn!("chime failed: {e}");
                }
            }
        }
    }
    if let Some(du) = &ducker {
        du.restore();
    }
    d.ports.players.resume().await;
    // A change the session was still waiting on stays on the waiting list (ADR 0030).
    if gate
        .set_aside(&session_id, "session ended")
        .ok()
        .flatten()
        .is_some()
    {
        d.refresh_waiting_now();
    }
    *lock(&d.session) = None;
    *lock(&d.heard) = None;
    *lock(&d.origin) = None;
    if origin == SessionOrigin::Desk {
        *lock(&d.last_desk_session) = Some(Instant::now());
    }
    info!("session {session_id} end");
    let mut data = Map::new();
    data.insert("session_id".into(), json!(session_id));
    data.insert("origin".into(), json!(origin_name));
    events.emit(EventKind::SessionEnd, data).await;
}

/// Greet once the phone's audio is up, forward the desk's events to it once its data
/// channel says client-ready, notice when its keepalive stops, and take the data
/// channel's own messages (client-ready, disconnect-bot).
fn phone_helpers(
    d: &Arc<Daemon>,
    session: &Arc<GhostSession>,
    media: &Arc<dyn RtcMedia>,
    peer_gone: Arc<Event>,
) -> Vec<JoinHandle<()>> {
    let connected = Arc::new(Event::new());
    let ready = Arc::new(Event::new());
    let mut tasks = Vec::new();
    // the link: connected once, gone when the peer left
    tasks.push(tokio::spawn({
        let media = media.clone();
        let connected = connected.clone();
        let peer_gone = peer_gone.clone();
        async move {
            let mut link = media.link();
            loop {
                match *link.borrow_and_update() {
                    Link::Connected => {
                        if !connected.is_set() {
                            let path = media.stats().path;
                            info!(
                                "phone: connected{}",
                                path.map(|p| format!(" over {p}")).unwrap_or_default()
                            );
                            connected.set();
                        }
                    }
                    Link::Gone => {
                        peer_gone.set();
                        return;
                    }
                    Link::Waiting => {}
                }
                if link.changed().await.is_err() {
                    peer_gone.set();
                    return;
                }
            }
        }
    }));
    // the data channel's messages (docs/phone-protocol.md)
    if let Some(mut messages) = media.take_messages() {
        tasks.push(tokio::spawn({
            let media = media.clone();
            let ready = ready.clone();
            let d = d.clone();
            async move {
                while let Some(m) = messages.next().await {
                    match m {
                        ClientMessage::ClientReady { id, version } => {
                            let _ = media.send(&Outbound::BotReady {
                                id,
                                version: ghost_rtc::rtvi::bot_ready_version(version.as_deref()),
                            });
                            ready.set();
                        }
                        ClientMessage::DisconnectBot { .. } => d.phone_end.set(),
                    }
                }
            }
        }));
    }
    // greet: the phone says client-ready once its speaker route and the remote track are
    // up; greeting on the bare connection lost the first half-second. An older app that
    // never says it still gets greeted.
    tasks.push(tokio::spawn({
        let session = session.clone();
        let connected = connected.clone();
        let ready = ready.clone();
        async move {
            connected.wait().await;
            if tokio::time::timeout(Duration::from_secs_f64(PHONE_READY_SECS), ready.wait())
                .await
                .is_err()
            {
                info!("phone: no client-ready; greeting anyway");
            }
            tokio::time::sleep(Duration::from_secs_f64(PHONE_GREETING_LEAD_SECS)).await;
            session.open().await;
        }
    }));
    // forward the desk's events: the same JSON the control API's WebSocket sends; never
    // logged, and the log events are not sent at all
    tasks.push(tokio::spawn({
        let d = d.clone();
        let media = media.clone();
        let ready = ready.clone();
        let events = d.state.bus.subscribe();
        async move {
            let sink = PhoneEventsSink(media);
            forward_events(
                events,
                async move { ready.wait().await },
                || match d.deps() {
                    Some(deps) => status_snapshot(&deps),
                    None => d.state.bare_status(),
                },
                &sink,
            )
            .await;
        }
    }));
    // the keepalive
    tasks.push(tokio::spawn({
        let media = media.clone();
        let connected = connected.clone();
        let peer_gone = peer_gone.clone();
        async move {
            let why = watchdog(
                async move { connected.wait().await },
                || media.is_connected(),
                Duration::from_secs_f64(PHONE_CONNECT_SECS),
                Duration::from_secs(1),
                PHONE_PING_GRACE_SECS,
            )
            .await;
            info!("phone: {why:?}");
            peer_gone.set();
        }
    }));
    tasks
}

/// The phone's media, with `origin` for the latency records, as `conversation` takes it.
pub fn pending_phone(media: Arc<dyn RtcMedia>, line: &str) -> PendingPhone {
    PendingPhone {
        media,
        line: line.to_string(),
    }
}

#[async_trait]
impl PhoneEvents for PhoneEventsSink {
    async fn send_server_message(&self, data: &Value) {
        if let Some(out) = Outbound::server_message(data.clone()) {
            let _ = self.0.send(&out);
        }
    }
}
