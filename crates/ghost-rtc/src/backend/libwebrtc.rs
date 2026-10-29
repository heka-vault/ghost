//! LiveKit's `libwebrtc` (Google's WebRTC, prebuilt, through the crate's C++ bindings).
//! The whole stack is inside it: Opus, the jitter buffer (NetEQ) and the resampling, so
//! this module asks for 16 kHz frames out and feeds 24 kHz frames in, and the crate's own
//! Opus and rubato paths are not on it. Callbacks come on libwebrtc's threads and hand off
//! to tokio. The media socket binds where libwebrtc decides (every interface but loopback);
//! `MediaConfig::bind` does not apply.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use libwebrtc::audio_frame::AudioFrame;
use libwebrtc::audio_source::AudioSourceOptions;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::data_channel::{DataBuffer, DataChannel, DataChannelState};
use libwebrtc::ice_candidate::IceCandidate;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{
    AnswerOptions, IceGatheringState, PeerConnection, PeerConnectionState, TrackEvent,
};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::session_description::{SdpType, SessionDescription};
use libwebrtc::stats::RtcStats;
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info};

use super::Shared;
use crate::MediaError;
use crate::frame::{IN_RATE, OUT_RATE, PcmFrame, frame_samples};
use crate::media::{CallStats, Frames, Link, Media, MediaConfig, Messages, Sink};
use crate::rtvi::Outbound;
use crate::sdp::{self, Fingerprint};

/// How long to wait for the local candidates before answering (the desk never trickles).
const GATHER_TIMEOUT: Duration = Duration::from_secs(3);
/// libwebrtc takes audio in 10 ms frames.
const TEN_MS: Duration = Duration::from_millis(10);
const TEN_MS_OUT: usize = (OUT_RATE / 100) as usize;
/// How much the source queues before `capture_frame` waits.
const SOURCE_QUEUE_MS: u32 = 50;
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// The inbound readers of one peer, spawned from `on_track`.
type Readers = Arc<Mutex<Vec<JoinHandle<()>>>>;

fn rtc_err(e: impl std::fmt::Display) -> MediaError {
    MediaError::Rtc(e.to_string())
}

/// What the callbacks and `send` share about one peer.
struct PeerState {
    channel: Mutex<Option<DataChannel>>,
    open: AtomicBool,
    restarting: AtomicBool,
}

impl PeerState {
    fn flush(&self, shared: &Shared) {
        if !self.open.load(Ordering::Acquire) {
            return;
        }
        let Some(dc) = self.channel.lock().expect("channel lock").clone() else {
            return;
        };
        for text in shared.drain_outbox() {
            if let Err(e) = dc.send(text.as_bytes(), false) {
                debug!("rtc: channel send failed: {e}");
            }
        }
    }
}

struct Peer {
    pc: PeerConnection,
    state: Arc<PeerState>,
    tasks: Vec<JoinHandle<()>>,
}

/// The libwebrtc stack behind [`Media`].
pub struct LibwebrtcMedia {
    shared: Arc<Shared>,
    #[allow(dead_code)] // libwebrtc picks its own interfaces; kept for the record
    config: MediaConfig,
    factory: PeerConnectionFactory,
    handle: tokio::runtime::Handle,
    peer: AsyncMutex<Option<Peer>>,
    live: Mutex<Option<Arc<PeerState>>>,
    /// The inbound readers, spawned from `on_track`, aborted with the call.
    readers: Mutex<Vec<Readers>>,
}

impl LibwebrtcMedia {
    /// Must be called inside a tokio runtime: the callbacks hand off to it.
    pub fn new(config: MediaConfig) -> Result<Self, MediaError> {
        Ok(LibwebrtcMedia {
            shared: Arc::new(Shared::new()),
            config,
            factory: PeerConnectionFactory::default(),
            handle: tokio::runtime::Handle::current(),
            peer: AsyncMutex::new(None),
            live: Mutex::new(None),
            readers: Mutex::new(Vec::new()),
        })
    }

    /// Take the live peer out without closing it yet (a restart starts the new one first:
    /// with no receiving stream left, libwebrtc's audio device stops its playout and the
    /// next peer's track never yields a frame).
    async fn take_peer(&self) -> (Option<Peer>, Vec<Readers>) {
        let peer = self.peer.lock().await.take();
        *self.live.lock().expect("live lock") = None;
        let readers = self
            .readers
            .lock()
            .expect("readers lock")
            .drain(..)
            .collect();
        (peer, readers)
    }

    fn close_peer(peer: Option<Peer>, readers: Vec<Readers>, restart: bool) {
        if let Some(peer) = peer {
            peer.state.restarting.store(restart, Ordering::Release);
            for t in peer.tasks {
                t.abort();
            }
            peer.pc.close();
        }
        for list in readers {
            for t in list.lock().expect("read tasks lock").drain(..) {
                t.abort();
            }
        }
    }

    async fn stop_peer(&self, restart: bool) {
        let (peer, readers) = self.take_peer().await;
        Self::close_peer(peer, readers, restart);
    }

    async fn start_peer(&self, offer_sdp: &str) -> Result<String, MediaError> {
        let mut configuration = RtcConfiguration::default(); // no ICE servers (ADR 0016)
        configuration.continual_gathering_policy = ContinualGatheringPolicy::GatherOnce;
        let pc = self
            .factory
            .create_peer_connection(configuration)
            .map_err(rtc_err)?;
        let state = Arc::new(PeerState {
            channel: Mutex::new(None),
            open: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
        });
        let (gathered_tx, mut gathered_rx) = watch::channel(false);
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();
        let read_tasks: Readers = Arc::new(Mutex::new(Vec::new()));

        {
            let shared = self.shared.clone();
            let st = state.clone();
            pc.on_connection_state_change(Some(Box::new(move |s| {
                debug!("rtc: connection {s:?}");
                match s {
                    PeerConnectionState::Connected => {
                        info!("rtc: peer connected");
                        shared.set_link(Link::Connected);
                    }
                    PeerConnectionState::Failed | PeerConnectionState::Closed
                        if !st.restarting.load(Ordering::Acquire) =>
                    {
                        shared.set_link(Link::Gone);
                    }
                    _ => {}
                }
            })));
        }
        pc.on_ice_gathering_state_change(Some(Box::new(move |s| {
            debug!("rtc: gathering {s:?}");
            if s == IceGatheringState::Complete {
                let _ = gathered_tx.send(true);
            }
        })));
        {
            let shared = self.shared.clone();
            let handle = self.handle.clone();
            let read_tasks = read_tasks.clone();
            pc.on_track(Some(Box::new(move |event: TrackEvent| {
                debug!("rtc: track");
                if let MediaStreamTrack::Audio(track) = event.track {
                    let stream = NativeAudioStream::new(track, IN_RATE as i32, 1);
                    let shared = shared.clone();
                    let task = handle.spawn(async move { read_frames(stream, shared).await });
                    read_tasks.lock().expect("read tasks lock").push(task);
                }
            })));
        }
        {
            let shared = self.shared.clone();
            let st = state.clone();
            pc.on_data_channel(Some(Box::new(move |dc: DataChannel| {
                debug!("rtc: data channel {:?}", dc.label());
                let for_messages = shared.clone();
                dc.on_message(Some(Box::new(move |buf: DataBuffer| {
                    if !buf.binary
                        && let Ok(text) = std::str::from_utf8(buf.data)
                    {
                        for_messages.on_channel_text(text);
                    }
                })));
                let for_state = st.clone();
                let shared_for_state = shared.clone();
                dc.on_state_change(Some(Box::new(move |s| {
                    debug!("rtc: channel {s:?}");
                    match s {
                        DataChannelState::Open => {
                            for_state.open.store(true, Ordering::Release);
                            for_state.flush(&shared_for_state);
                        }
                        DataChannelState::Closing | DataChannelState::Closed => {
                            for_state.open.store(false, Ordering::Release);
                        }
                        DataChannelState::Connecting => {}
                    }
                })));
                let already_open = dc.state() == DataChannelState::Open;
                *st.channel.lock().expect("channel lock") = Some(dc);
                if already_open {
                    st.open.store(true, Ordering::Release);
                    st.flush(&shared);
                }
            })));
        }

        let source =
            NativeAudioSource::new(AudioSourceOptions::default(), OUT_RATE, 1, SOURCE_QUEUE_MS);
        let track = self.factory.create_audio_track("ghost", source.clone());
        pc.add_track(MediaStreamTrack::Audio(track), &["ghost"])
            .map_err(rtc_err)?;

        let started = std::time::Instant::now();
        let offer = SessionDescription::parse(offer_sdp, SdpType::Offer)
            .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))?;
        let answer = pc
            .create_answer(AnswerOptions::default())
            .await
            .map_err(rtc_err)?;
        pc.set_local_description(answer).await.map_err(rtc_err)?;
        let _ = tokio::time::timeout(GATHER_TIMEOUT, async {
            while !*gathered_rx.borrow() && gathered_rx.changed().await.is_ok() {}
        })
        .await;
        let sdp = pc
            .current_local_description()
            .ok_or_else(|| rtc_err("no local description"))?
            .to_string();
        let facts =
            sdp::check_answer(&sdp).map_err(|e| MediaError::Rtc(format!("our answer: {e}")))?;
        debug!(
            "rtc: answering with {} candidate(s) after {:?}",
            facts.candidates.len(),
            started.elapsed()
        );
        *self.shared.fingerprint.lock().expect("fingerprint lock") = sdp::fingerprint_of(&sdp);
        self.shared.reset_for_peer();
        self.shared.set_link(Link::Waiting);

        // The pacer: 10 ms of the sink into the source, which waits when its queue is full.
        {
            let shared = self.shared.clone();
            let link = self.shared.link.subscribe();
            tasks.push(tokio::spawn(async move {
                let mut ticks = tokio::time::interval(TEN_MS);
                let mut half = false;
                loop {
                    ticks.tick().await;
                    if *link.borrow() != Link::Connected {
                        continue;
                    }
                    let pcm = shared.sink.pull(TEN_MS_OUT);
                    let frame = AudioFrame {
                        data: Cow::Owned(pcm),
                        sample_rate: OUT_RATE,
                        num_channels: 1,
                        samples_per_channel: TEN_MS_OUT as u32,
                    };
                    if let Err(e) = source.capture_frame(&frame).await {
                        debug!("rtc: capture_frame failed: {e}");
                    }
                    half = !half;
                    if !half {
                        shared.stats.lock().expect("stats lock").packets_sent += 1;
                    }
                }
            }));
        }
        // The stats, from libwebrtc's own report.
        {
            let shared = self.shared.clone();
            let pc = pc.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(STATS_INTERVAL).await;
                    let Ok(report) = pc.get_stats().await else {
                        continue;
                    };
                    let mut stats = shared.stats.lock().expect("stats lock");
                    for entry in report {
                        match entry {
                            RtcStats::InboundRtp(i) if i.stream.kind == "audio" => {
                                stats.packets_received = i.received.packets_received;
                                stats.packets_lost = i.received.packets_lost.max(0) as u64;
                                stats.jitter_ms = i.received.jitter * 1000.0;
                                stats.bytes_received = i.inbound.bytes_received;
                            }
                            RtcStats::OutboundRtp(o) if o.stream.kind == "audio" => {
                                stats.packets_sent = o.sent.packets_sent;
                                stats.bytes_sent = o.sent.bytes_sent;
                            }
                            _ => {}
                        }
                    }
                }
            }));
        }
        *self.live.lock().expect("live lock") = Some(state.clone());
        *self.peer.lock().await = Some(Peer { pc, state, tasks });
        // The inbound readers are aborted on close through their own list.
        self.readers.lock().expect("readers lock").push(read_tasks);
        Ok(sdp)
    }
}

/// libwebrtc's 10 ms frames at 16 kHz, paired into the pipeline's 20 ms frames.
async fn read_frames(mut stream: NativeAudioStream, shared: Arc<Shared>) {
    let want = frame_samples(IN_RATE);
    let mut pending: Vec<i16> = Vec::with_capacity(want * 2);
    let mut first = true;
    while let Some(frame) = stream.next().await {
        if first {
            debug!(
                "rtc: first inbound frame ({} samples at {} Hz)",
                frame.data.len(),
                frame.sample_rate
            );
            first = false;
        }
        pending.extend_from_slice(&frame.data);
        while pending.len() >= want {
            let samples: Vec<i16> = pending.drain(..want).collect();
            if shared
                .frames_tx
                .try_send(PcmFrame::new(samples, IN_RATE))
                .is_err()
            {
                shared.stats.lock().expect("stats lock").frames_dropped += 1;
            }
        }
    }
}

#[async_trait]
impl Media for LibwebrtcMedia {
    async fn answer(&self, offer_sdp: &str, restart: bool) -> Result<String, MediaError> {
        sdp::parse_offer(offer_sdp).map_err(|e| MediaError::BadOffer(e.to_string()))?;
        if restart {
            info!("rtc: restarting the peer (restart_pc)");
            let (old, old_readers) = self.take_peer().await;
            let answer = self.start_peer(offer_sdp).await;
            Self::close_peer(old, old_readers, true);
            return answer;
        }
        let live = self.peer.lock().await.as_ref().map(|p| p.pc.clone());
        if let Some(pc) = live {
            let offer = SessionDescription::parse(offer_sdp, SdpType::Offer)
                .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))?;
            pc.set_remote_description(offer)
                .await
                .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))?;
            let answer = pc
                .create_answer(AnswerOptions::default())
                .await
                .map_err(rtc_err)?;
            pc.set_local_description(answer).await.map_err(rtc_err)?;
            let sdp = pc
                .current_local_description()
                .ok_or_else(|| rtc_err("no local description"))?
                .to_string();
            *self.shared.fingerprint.lock().expect("fingerprint lock") = sdp::fingerprint_of(&sdp);
            return Ok(sdp);
        }
        self.start_peer(offer_sdp).await
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
        let parsed = IceCandidate::parse("0", 0, bare).map_err(|_| MediaError::BadCandidate)?;
        pc.add_ice_candidate(parsed)
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
        let live = self.live.lock().expect("live lock").clone();
        if let Some(state) = live {
            state.flush(&self.shared);
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
