//! webrtc-rs (`webrtc` 0.21): async, a driver task per peer on tokio over the sans-IO `rtc`
//! core. The crate binds the UDP socket on the configured address itself; this module
//! hands it the offer, pulls RTP from the remote track, paces samples into a local track,
//! and polls the data channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use rtc::media::Sample;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
use rtc::peer_connection::transport::RTCDtlsRole;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceGatheringState, RTCPeerConnectionState,
    RTCSessionDescription, Registry, SettingEngineBuilder, register_default_interceptors,
};

use super::{InboundPath, OutboundPath, Shared};
use crate::MediaError;
use crate::frame::FRAME_MS;
use crate::media::{CallStats, Frames, Link, Media, MediaConfig, Messages, Sink};
use crate::rtvi::Outbound;
use crate::sdp::{self, Fingerprint};

const FRAME: Duration = Duration::from_millis(FRAME_MS as u64);
/// How long to wait for the local candidates before answering (the desk never trickles).
const GATHER_TIMEOUT: Duration = Duration::from_secs(3);

fn rtc_err(e: impl std::fmt::Display) -> MediaError {
    MediaError::Rtc(e.to_string())
}

/// The handler the driver calls; what it learns goes to [`Shared`].
struct Handler {
    shared: Arc<Shared>,
    gathered: watch::Sender<bool>,
    channel: Mutex<Option<Arc<dyn DataChannel>>>,
    channel_open: Arc<AtomicBool>,
    /// Set while the peer is being replaced, so its `Closed` is not a `Gone`.
    restarting: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Handler {
    fn new(shared: Arc<Shared>, gathered: watch::Sender<bool>) -> Self {
        Handler {
            shared,
            gathered,
            channel: Mutex::new(None),
            channel_open: Arc::new(AtomicBool::new(false)),
            restarting: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
        }
    }

    fn channel(&self) -> Option<Arc<dyn DataChannel>> {
        if !self.channel_open.load(Ordering::Acquire) {
            return None;
        }
        self.channel.lock().expect("channel lock").clone()
    }

    /// Send what waits in the outbox, if the channel is open.
    async fn flush(&self) {
        let Some(dc) = self.channel() else {
            return;
        };
        for text in self.shared.drain_outbox() {
            if let Err(e) = dc.send_text(&text).await {
                debug!("rtc: channel send failed: {e}");
            }
        }
    }

    fn abort(&self) {
        for t in self.tasks.lock().expect("tasks lock").drain(..) {
            t.abort();
        }
    }
}

#[async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        debug!("rtc: connection {state:?}");
        match state {
            RTCPeerConnectionState::Connected => {
                info!("rtc: peer connected");
                self.shared.set_link(Link::Connected);
            }
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
                if !self.restarting.load(Ordering::Acquire) =>
            {
                self.shared.set_link(Link::Gone);
            }
            _ => {}
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let shared = self.shared.clone();
        let task = tokio::spawn(async move {
            let Ok(mut inbound) = InboundPath::new() else {
                warn!("rtc: Opus is not available");
                return;
            };
            while let Some(event) = track.poll().await {
                match event {
                    TrackRemoteEvent::OnRtpPacket(p) => inbound.packet(
                        &shared,
                        &p.payload,
                        p.header.sequence_number,
                        p.header.timestamp,
                        Instant::now(),
                    ),
                    TrackRemoteEvent::OnEnded => break,
                    _ => {}
                }
            }
        });
        self.tasks.lock().expect("tasks lock").push(task);
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        *self.channel.lock().expect("channel lock") = Some(dc.clone());
        let shared = self.shared.clone();
        let open = self.channel_open.clone();
        let task = tokio::spawn(async move {
            while let Some(event) = dc.poll().await {
                match event {
                    DataChannelEvent::OnOpen => {
                        debug!("rtc: channel open");
                        open.store(true, Ordering::Release);
                        for text in shared.drain_outbox() {
                            if let Err(e) = dc.send_text(&text).await {
                                debug!("rtc: channel send failed: {e}");
                            }
                        }
                    }
                    DataChannelEvent::OnMessage(message) => {
                        if let Ok(text) = std::str::from_utf8(&message.data) {
                            shared.on_channel_text(text);
                        }
                    }
                    DataChannelEvent::OnClose => {
                        open.store(false, Ordering::Release);
                        break;
                    }
                    _ => {}
                }
            }
        });
        self.tasks.lock().expect("tasks lock").push(task);
    }
}

struct Peer {
    pc: Arc<dyn PeerConnection>,
    handler: Arc<Handler>,
    pacer: JoinHandle<()>,
}

/// The webrtc-rs stack behind [`Media`].
pub struct WebrtcMedia {
    shared: Arc<Shared>,
    config: MediaConfig,
    peer: AsyncMutex<Option<Peer>>,
    live: Mutex<Option<Arc<Handler>>>,
}

impl WebrtcMedia {
    pub fn new(config: MediaConfig) -> Result<Self, MediaError> {
        Ok(WebrtcMedia {
            shared: Arc::new(Shared::new()),
            config,
            peer: AsyncMutex::new(None),
            live: Mutex::new(None),
        })
    }

    async fn stop_peer(&self, restart: bool) {
        let peer = self.peer.lock().await.take();
        *self.live.lock().expect("live lock") = None;
        if let Some(peer) = peer {
            peer.handler.restarting.store(restart, Ordering::Release);
            peer.pacer.abort();
            if let Err(e) = tokio::time::timeout(Duration::from_secs(2), peer.pc.close()).await {
                debug!("rtc: close timed out: {e}");
            }
            peer.handler.abort();
            drop(peer);
            // Let the old driver release its socket before a new one binds. Known
            // limit (README): a second peer in the same process, the protocol's
            // restart_pc, connects ICE and DTLS but its SCTP never opens with webrtc 0.21.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn start_peer(&self, offer_sdp: &str, opus_pt: u8) -> Result<String, MediaError> {
        let mut engine = MediaEngine::default();
        engine.register_default_codecs().map_err(rtc_err)?;
        let registry =
            register_default_interceptors(Registry::new(), &mut engine).map_err(rtc_err)?;
        let configuration = RTCConfigurationBuilder::new().build(); // no ICE servers (ADR 0016)
        // Answer as the DTLS server so the phone opens SCTP: as the client, rtc 0.21's SCTP
        // endpoint refuses the phone's reply ("refusing first packet due to empty
        // server_config") and the data channel never opens.
        let settings = SettingEngineBuilder::new()
            .with_answering_dtls_role(RTCDtlsRole::Server)
            .build();
        let (gathered_tx, mut gathered_rx) = watch::channel(false);
        let handler = Arc::new(Handler::new(self.shared.clone(), gathered_tx));
        let pc = PeerConnectionBuilder::new()
            .with_configuration(configuration)
            .with_media_engine(engine)
            .with_setting_engine(settings)
            .with_interceptor_registry(registry)
            .with_handler(handler.clone())
            .with_udp_addrs(vec![format!("{}:0", self.config.bind)])
            .build()
            .await
            .map_err(rtc_err)?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);
        let ssrc: u32 = rand::random();
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    "ghost".to_string(),
                    "ghost-audio".to_string(),
                    "ghost".to_string(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(ssrc),
                            ..Default::default()
                        },
                        codec: RTCRtpCodec {
                            mime_type: MIME_TYPE_OPUS.to_owned(),
                            clock_rate: 48_000,
                            channels: 2,
                            sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                            rtcp_feedback: vec![],
                        },
                        ..Default::default()
                    }],
                ),
            )
            .map_err(rtc_err)?,
        );
        pc.add_track(track.clone() as Arc<dyn TrackLocal>)
            .await
            .map_err(rtc_err)?;
        let offer = RTCSessionDescription::offer(offer_sdp.to_string())
            .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))?;
        let answer = pc.create_answer(None).await.map_err(rtc_err)?;
        pc.set_local_description(answer).await.map_err(rtc_err)?;
        let _ = tokio::time::timeout(GATHER_TIMEOUT, async {
            while !*gathered_rx.borrow() && gathered_rx.changed().await.is_ok() {}
        })
        .await;
        let sdp = pc
            .local_description()
            .await
            .ok_or_else(|| rtc_err("no local description"))?
            .sdp;
        let facts =
            sdp::check_answer(&sdp).map_err(|e| MediaError::Rtc(format!("our answer: {e}")))?;
        debug!(
            "rtc: answering with {} candidate(s)",
            facts.candidates.len()
        );
        *self.shared.fingerprint.lock().expect("fingerprint lock") = sdp::fingerprint_of(&sdp);
        self.shared.reset_for_peer();
        self.shared.set_link(Link::Waiting);

        let pacer = {
            let shared = self.shared.clone();
            let link = self.shared.link.subscribe();
            tokio::spawn(async move { pace(shared, track, link, ssrc, opus_pt).await })
        };
        *self.live.lock().expect("live lock") = Some(handler.clone());
        *self.peer.lock().await = Some(Peer { pc, handler, pacer });
        Ok(sdp)
    }
}

/// The outbound packets, 20 ms apart while the link is up.
async fn pace(
    shared: Arc<Shared>,
    track: Arc<TrackLocalStaticSample>,
    link: watch::Receiver<Link>,
    ssrc: u32,
    payload_type: u8,
) {
    let Ok(mut outbound) = OutboundPath::new() else {
        warn!("rtc: Opus is not available");
        return;
    };
    let mut ticks = tokio::time::interval(FRAME);
    loop {
        ticks.tick().await;
        if *link.borrow() != Link::Connected {
            continue;
        }
        match outbound.next_packet(&shared) {
            Ok(packet) => {
                let sample = Sample {
                    data: Bytes::from(packet),
                    timestamp: Instant::now(),
                    duration: FRAME,
                    packet_timestamp: 0,
                    prev_dropped_packets: 0,
                    prev_padding_packets: 0,
                };
                if let Err(e) = track.write_sample(ssrc, payload_type, &sample, &[]).await {
                    debug!("rtc: write failed: {e}");
                }
            }
            Err(e) => debug!("rtc: encode failed: {e}"),
        }
    }
}

#[async_trait]
impl Media for WebrtcMedia {
    async fn answer(&self, offer_sdp: &str, restart: bool) -> Result<String, MediaError> {
        let parsed =
            sdp::parse_offer(offer_sdp).map_err(|e| MediaError::BadOffer(e.to_string()))?;
        if restart {
            info!("rtc: restarting the peer (restart_pc)");
            self.stop_peer(true).await;
            return self.start_peer(offer_sdp, parsed.audio.opus_pt).await;
        }
        let live = self.peer.lock().await.as_ref().map(|p| p.pc.clone());
        if let Some(pc) = live {
            let offer = RTCSessionDescription::offer(offer_sdp.to_string())
                .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))?;
            pc.set_remote_description(offer)
                .await
                .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))?;
            let answer = pc.create_answer(None).await.map_err(rtc_err)?;
            pc.set_local_description(answer).await.map_err(rtc_err)?;
            let sdp = pc
                .local_description()
                .await
                .ok_or_else(|| rtc_err("no local description"))?
                .sdp;
            *self.shared.fingerprint.lock().expect("fingerprint lock") = sdp::fingerprint_of(&sdp);
            return Ok(sdp);
        }
        self.start_peer(offer_sdp, parsed.audio.opus_pt).await
    }

    async fn add_remote_candidate(&self, candidate: &str) -> Result<(), MediaError> {
        let Some(bare) = sdp::bare_candidate(candidate) else {
            return Ok(());
        };
        let pc = self
            .peer
            .lock()
            .await
            .as_ref()
            .map(|p| p.pc.clone())
            .ok_or(MediaError::NoPeer)?;
        pc.add_ice_candidate(RTCIceCandidateInit {
            candidate: bare.to_string(),
            sdp_mid: None,
            sdp_mline_index: None,
            username_fragment: None,
            url: None,
        })
        .await
        .map_err(|_| MediaError::BadCandidate)
    }

    fn fingerprint(&self) -> Option<Fingerprint> {
        self.shared
            .fingerprint
            .lock()
            .expect("fingerprint lock")
            .clone()
    }

    fn take_frames(&self) -> Option<Frames> {
        self.shared.take_frames()
    }

    fn sink(&self) -> Sink {
        self.shared.sink.clone()
    }

    fn take_messages(&self) -> Option<Messages> {
        self.shared.take_messages()
    }

    fn send(&self, message: &Outbound) -> Result<(), MediaError> {
        self.shared.queue_outbound(message)?;
        let handler = self.live.lock().expect("live lock").clone();
        if let Some(handler) = handler {
            tokio::spawn(async move { handler.flush().await });
        }
        Ok(())
    }

    fn link(&self) -> watch::Receiver<Link> {
        self.shared.link.subscribe()
    }

    fn is_connected(&self) -> bool {
        self.shared.is_connected()
    }

    fn stats(&self) -> CallStats {
        self.shared.stats()
    }

    async fn close(&self) {
        self.stop_peer(false).await;
        self.shared.finish();
    }
}
