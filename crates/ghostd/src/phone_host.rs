//! The phone listener's side of the daemon (`PhoneHost`, ADR 0016, ADR 0020): the
//! views it serves, whether a call may start, the offer answered by the media crate, and
//! the phone's media as a session's transport: its frames in, Ghost's speech out with the
//! thinking sound in the same track, and the data channel for the RTVI events.
//!
//! Nothing from the phone's audio is logged or kept here; what goes over the data channel
//! is the same event JSON the control API's WebSocket sends, never a log line.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ghost_api::models::{AuditPage, ConversationView, Status, WaitingView};
use ghost_api::{audit_page, conversation_view, status_snapshot, waiting_view};
use ghost_rtc::{Frames, Media as RtcMedia, MediaConfig, MediaError, Outbound, Sink};
use ghost_voice::VoiceSettings;
use ghost_voice::frames::Audio;
use ghost_voice::rtvi::RtviSink;
use ghost_voice::transport::{Media, MediaIn, MediaOut};
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::daemon::{Daemon, LivePhone, lock};
use crate::phone::listener::parse_listen;
use crate::phone::routes::PHONE_WRITE_TIMEOUT_SECS;
use crate::phone::{Answer, Candidate, CandidateError, OfferError, PhoneHost, Refusal};

/// The phone's frames, as the pipeline's source takes them.
pub struct PhoneIn(pub Frames);

#[async_trait]
impl MediaIn for PhoneIn {
    async fn next_frame(&mut self) -> Option<Audio> {
        let f = self.0.next().await?;
        Some(Audio::new(f.samples, f.rate))
    }
}

/// Ghost's speech to the phone, and the thinking sound in the same track: while it is
/// on and nothing is queued, the loop fills the sink a frame at a time, so it plays in
/// the gaps and never over speech.
pub struct PhoneOut {
    media: Arc<dyn RtcMedia>,
    sink: Sink,
    thinking: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}

/// How much thinking sound the loop keeps queued.
const THINKING_AHEAD_MS: u32 = 120;

impl PhoneOut {
    pub fn new(media: Arc<dyn RtcMedia>, sound: Option<Arc<Vec<i16>>>, volume: f64) -> Self {
        let sink = media.sink();
        let thinking = Arc::new(AtomicBool::new(false));
        let task = sound.map(|sound| {
            let sink = sink.clone();
            let thinking = thinking.clone();
            let frame = ghost_rtc::frame_samples(ghost_rtc::OUT_RATE);
            tokio::spawn(async move {
                let mut at = 0usize;
                loop {
                    tokio::time::sleep(Duration::from_millis(ghost_rtc::FRAME_MS as u64)).await;
                    if !thinking.load(Ordering::SeqCst) || sound.is_empty() {
                        at = 0;
                        continue;
                    }
                    if sink.queued_ms() >= THINKING_AHEAD_MS {
                        continue;
                    }
                    let chunk: Vec<i16> = (0..frame)
                        .map(|i| {
                            let s = sound[(at + i) % sound.len()];
                            (f64::from(s) * volume) as i16
                        })
                        .collect();
                    at = (at + frame) % sound.len();
                    sink.try_send(&chunk);
                }
            })
        });
        Self {
            media,
            sink,
            thinking,
            task,
        }
    }
}

#[async_trait]
impl MediaOut for PhoneOut {
    async fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        // The phone closes the call itself after the goodbye; a write to the gone peer
        // should fail fast, not hold the session.
        match tokio::time::timeout(
            Duration::from_secs_f64(PHONE_WRITE_TIMEOUT_SECS),
            self.sink.send(pcm),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("the phone is not taking audio".into()),
        }
    }

    async fn clear(&mut self) {
        self.sink.clear();
    }

    async fn drain(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.sink.queued() > 0 && tokio::time::Instant::now() < deadline {
            if !self.media.is_connected() && self.media.stats().packets_sent == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(ghost_rtc::FRAME_MS as u64)).await;
        }
    }

    fn set_thinking(&self, on: bool) {
        self.thinking.store(on, Ordering::SeqCst);
    }

    async fn send_message(&self, message: Value) {
        if let Some(out) = Outbound::server_message(message) {
            let _ = self.media.send(&out);
        }
    }

    fn close(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// The phone's media as a session's transport. None when its frames were already taken
/// (a second session on the same peer).
pub fn phone_media(media: &Arc<dyn RtcMedia>, voice: &VoiceSettings) -> Option<Media> {
    let frames = media.take_frames()?;
    let sound = voice
        .thinking_sound
        .then(
            || match ghost_audio::sounds::thinking_sound(&voice.cache_dir, ghost_rtc::OUT_RATE) {
                Ok(s) => Some(s),
                Err(e) => {
                    warn!("thinking sound: {e}");
                    None
                }
            },
        )
        .flatten();
    Some(Media {
        input: Box::new(PhoneIn(frames)),
        output: Box::new(PhoneOut::new(media.clone(), sound, voice.thinking_volume)),
        origin: "phone",
    })
}

/// The session's RTVI messages to the phone's data channel.
pub struct PhoneRtvi(pub Arc<dyn RtcMedia>);

#[async_trait]
impl RtviSink for PhoneRtvi {
    async fn send(&self, message: Value) {
        let out = match message.get("type").and_then(Value::as_str) {
            Some("server-message") => message
                .get("data")
                .cloned()
                .and_then(Outbound::server_message),
            Some("bot-started-speaking") => Some(Outbound::BotStartedSpeaking),
            Some("bot-stopped-speaking") => Some(Outbound::BotStoppedSpeaking),
            Some("user-started-speaking") => Some(Outbound::UserStartedSpeaking),
            Some("user-stopped-speaking") => Some(Outbound::UserStoppedSpeaking),
            _ => None,
        };
        if let Some(out) = out {
            let _ = self.0.send(&out);
        }
    }
}

/// The desk's events to the phone (`forward_events`).
pub struct PhoneEventsSink(pub Arc<dyn RtcMedia>);

/// Where the phone's media binds: the listen address the offer came in on, so the
/// candidates in the answer are on the network the phone reached (ADR 0020); a wildcard
/// listen address can't say, and is said so.
fn media_bind(d: &Daemon) -> IpAddr {
    let listener = lock(&d.phone_listener).clone();
    let address = listener.as_ref().and_then(|l| {
        l.store
            .device()
            .and_then(|dev| dev.last_address)
            .or_else(|| Some(l.address.clone()))
    });
    let ip = address
        .as_deref()
        .and_then(|a| parse_listen(a).ok())
        .and_then(|(host, _)| host.parse::<IpAddr>().ok());
    match ip {
        Some(ip) if !ip.is_unspecified() => ip,
        _ => {
            warn!(
                "phone: GHOST_PHONE_LISTEN names no specific address; the media binds the \
                 loopback and a phone across the network cannot reach it"
            );
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
    }
}

#[async_trait]
impl PhoneHost for Daemon {
    fn status(&self) -> Status {
        match self.deps() {
            Some(deps) => status_snapshot(&deps),
            None => self.state.bare_status(),
        }
    }

    fn audit(&self, limit: i64, offset: i64) -> AuditPage {
        let empty = AuditPage {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        };
        let Some(store) = self.late.store.get() else {
            return empty;
        };
        audit_page(store, limit, offset, None, None, None).unwrap_or(empty)
    }

    /// The phone's read of the waiting list; the fetch is the phone route's delivery.
    fn waiting_view(&self) -> WaitingView {
        let view = match self.deps() {
            Some(deps) => waiting_view(&deps).unwrap_or(WaitingView {
                items: Vec::new(),
                count: 0,
            }),
            None => WaitingView {
                items: Vec::new(),
                count: 0,
            },
        };
        self.phone_route.fetched();
        view
    }

    fn acknowledge_item(&self, key: &str) -> bool {
        self.waiting()
            .is_some_and(|w| w.acknowledge(key, None, None).unwrap_or(false))
    }

    fn snooze_item(&self, key: &str, minutes: i64) -> bool {
        let until = ghost_core::time::utcnow() + chrono::Duration::minutes(minutes);
        self.waiting()
            .is_some_and(|w| w.snooze(key, until).unwrap_or(false))
    }

    /// The agents a phone call can start with: Ghost first, retired ones left out.
    fn agents(&self) -> Vec<ConversationView> {
        let Some(convs) = self.conversations() else {
            return Vec::new();
        };
        let active = self.state.conversation();
        convs
            .list(false)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| conversation_view(&convs, p, active.as_deref()).ok())
            .collect()
    }

    fn admit_phone(&self) -> Option<Refusal> {
        Daemon::admit_phone(self)
    }

    /// Take the phone's offer: build the peer, and run the session on it, in
    /// `conversation` or in Ghost. The answer the phone applies.
    async fn start_phone(
        &self,
        sdp: &str,
        conversation: Option<&str>,
    ) -> Result<Answer, OfferError> {
        let line = conversation.unwrap_or(ghost_core::conversations::GHOST);
        if line != ghost_core::conversations::GHOST {
            let profile = self
                .conversations()
                .and_then(|c| c.get(line).ok().flatten());
            match profile {
                Some(p) if !p.retired() => {}
                _ => return Err(OfferError::Refused(Refusal::NoSuchLine)),
            }
        }
        if let Some(r) = self.admit_phone() {
            return Err(OfferError::Refused(r));
        }
        let media: Arc<dyn RtcMedia> = match ghost_rtc::new_media(MediaConfig {
            bind: media_bind(self),
            ..MediaConfig::default()
        }) {
            Ok(m) => Arc::from(m),
            Err(e) => {
                warn!("phone: no media: {e}");
                return Err(OfferError::Internal);
            }
        };
        let answer = match media.answer(sdp, false).await {
            Ok(a) => a,
            Err(MediaError::BadOffer(why)) => {
                info!("phone: bad offer ({why})");
                media.close().await;
                return Err(OfferError::BadOffer);
            }
            Err(e) => {
                warn!("phone: the offer got no answer: {e}");
                media.close().await;
                return Err(OfferError::Internal);
            }
        };
        let pc_id = uuid::Uuid::new_v4().simple().to_string();
        *lock(&self.live_phone) = Some(LivePhone {
            pc_id: pc_id.clone(),
            media: media.clone(),
        });
        if let Some(r) = self.queue_phone(media.clone(), Some(line)) {
            *lock(&self.live_phone) = None;
            media.close().await;
            return Err(OfferError::Refused(r));
        }
        info!("phone: offer accepted ({})", ghost_rtc::BACKEND);
        Ok(Answer::new(answer, pc_id))
    }

    /// A new offer for the live peer (`pc_id`): a renegotiation, an ICE restart.
    async fn renegotiate(
        &self,
        pc_id: &str,
        sdp: &str,
        restart_pc: bool,
    ) -> Result<Answer, OfferError> {
        let live = lock(&self.live_phone)
            .as_ref()
            .filter(|l| l.pc_id == pc_id)
            .map(|l| l.media.clone());
        let Some(media) = live else {
            return Err(OfferError::NoSuchPeer);
        };
        match media.answer(sdp, restart_pc).await {
            Ok(a) => Ok(Answer::new(a, pc_id)),
            Err(MediaError::BadOffer(_)) => Err(OfferError::BadOffer),
            Err(e) => {
                warn!("phone: the offer got no answer: {e}");
                Err(OfferError::Internal)
            }
        }
    }

    async fn add_candidates(
        &self,
        pc_id: &str,
        candidates: &[Candidate],
    ) -> Result<(), CandidateError> {
        let live = lock(&self.live_phone)
            .as_ref()
            .filter(|l| l.pc_id == pc_id)
            .map(|l| l.media.clone());
        let Some(media) = live else {
            return Err(CandidateError::NoSuchPeer);
        };
        for c in candidates {
            media
                .add_remote_candidate(&c.candidate)
                .await
                .map_err(|_| CandidateError::BadCandidate)?;
        }
        Ok(())
    }

    async fn end_phone(&self) -> bool {
        Daemon::end_phone(self).await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! A fake peer for the daemon's phone tests: no network, no audio.

    use std::sync::Mutex;

    use ghost_rtc::{CallStats, Fingerprint, Link, Messages};
    use tokio::sync::watch;

    use super::*;

    pub(crate) struct FakeMedia {
        link: watch::Sender<Link>,
        sink: Sink,
        pub sent: Mutex<Vec<Outbound>>,
        pub closed: AtomicBool,
    }

    impl Default for FakeMedia {
        fn default() -> Self {
            let (link, _) = watch::channel(Link::Waiting);
            Self {
                link,
                sink: Sink::new(24_000),
                sent: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl RtcMedia for FakeMedia {
        async fn answer(&self, _offer: &str, _restart: bool) -> Result<String, MediaError> {
            Ok("v=0\r\n".into())
        }
        async fn add_remote_candidate(&self, _c: &str) -> Result<(), MediaError> {
            Ok(())
        }
        fn fingerprint(&self) -> Option<Fingerprint> {
            None
        }
        fn take_frames(&self) -> Option<Frames> {
            None
        }
        fn sink(&self) -> Sink {
            self.sink.clone()
        }
        fn take_messages(&self) -> Option<Messages> {
            None
        }
        fn send(&self, message: &Outbound) -> Result<(), MediaError> {
            self.sent.lock().unwrap().push(message.clone());
            Ok(())
        }
        fn link(&self) -> watch::Receiver<Link> {
            self.link.subscribe()
        }
        fn is_connected(&self) -> bool {
            *self.link.borrow() == Link::Connected
        }
        fn stats(&self) -> CallStats {
            CallStats::default()
        }
        async fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            let _ = self.link.send(Link::Gone);
        }
    }

    #[tokio::test]
    async fn the_rtvi_sink_sends_server_messages_and_the_speaking_events_but_never_a_log() {
        let media = Arc::new(FakeMedia::default());
        let sink = PhoneRtvi(media.clone());
        sink.send(ghost_voice::rtvi::server_message(
            serde_json::json!({"type": "status", "phase": "listening"}),
        ))
        .await;
        sink.send(ghost_voice::rtvi::server_message(
            serde_json::json!({"type": "log", "message": "secret"}),
        ))
        .await;
        sink.send(ghost_voice::rtvi::bot_speaking(true)).await;
        let sent = media.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert!(matches!(&sent[0], Outbound::ServerMessage(v) if v["type"] == "status"));
        assert_eq!(sent[1], Outbound::BotStartedSpeaking);
    }

    #[tokio::test]
    async fn the_phone_output_fails_fast_on_a_full_sink_and_clears_on_barge_in() {
        let media: Arc<dyn RtcMedia> = Arc::new(FakeMedia::default());
        let mut out = PhoneOut::new(media.clone(), None, 0.1);
        out.write(&vec![0; 24_000]).await.unwrap(); // a second, fits
        assert!(out.write(&vec![0; 24_000]).await.is_err()); // nobody drains: fails in 0.5 s
        out.clear().await;
        assert_eq!(media.sink().queued(), 0);
        out.close();
    }
}
