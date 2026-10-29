//! str0m: sans-IO. This crate owns the I/O: one UDP socket per peer, bound on the
//! configured address, driven on one tokio task that feeds str0m what arrives, sends what
//! it produces, keeps its timers, paces the outbound packets 20 ms apart and takes the
//! daemon's commands (a renegotiation, a trickled candidate, close).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{InboundPath, OutboundPath, Shared};
use crate::MediaError;
use crate::frame::{FRAME_MS, WIRE_RATE, frame_samples};
use crate::media::{CallStats, Frames, Link, Media, MediaConfig, Messages, Sink};
use crate::rtvi::Outbound;
use crate::sdp::{self, Fingerprint};

/// How many packets str0m holds back to reorder audio before releasing a gap (its default
/// is 15, 300 ms on a loss; the pipeline would rather conceal one frame than wait).
const REORDERING_SIZE_AUDIO: usize = 2;
const STATS_INTERVAL: Duration = Duration::from_secs(1);
const FRAME: Duration = Duration::from_millis(FRAME_MS as u64);

static CRYPTO: Once = Once::new();

fn install_crypto() {
    CRYPTO.call_once(|| {
        str0m::crypto::from_feature_flags().install_process_default();
    });
}

enum Cmd {
    Offer {
        sdp: String,
        reply: oneshot::Sender<Result<String, MediaError>>,
    },
    Candidate(String),
    Flush,
    Close {
        restart: bool,
        done: oneshot::Sender<()>,
    },
}

/// The str0m stack behind [`Media`].
pub struct Str0mMedia {
    shared: Arc<Shared>,
    config: MediaConfig,
    cmd: Mutex<Option<mpsc::Sender<Cmd>>>,
    task: AsyncMutex<Option<JoinHandle<()>>>,
}

impl Str0mMedia {
    pub fn new(config: MediaConfig) -> Result<Self, MediaError> {
        install_crypto();
        Ok(Str0mMedia {
            shared: Arc::new(Shared::new()),
            config,
            cmd: Mutex::new(None),
            task: AsyncMutex::new(None),
        })
    }

    fn cmd_sender(&self) -> Option<mpsc::Sender<Cmd>> {
        self.cmd.lock().expect("cmd lock").clone()
    }

    async fn stop_peer(&self, restart: bool) {
        let sender = self.cmd.lock().expect("cmd lock").take();
        if let Some(sender) = sender {
            let (done_tx, done_rx) = oneshot::channel();
            if sender
                .send(Cmd::Close {
                    restart,
                    done: done_tx,
                })
                .await
                .is_ok()
            {
                let _ = tokio::time::timeout(Duration::from_secs(2), done_rx).await;
            }
        }
        if let Some(task) = self.task.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
    }

    async fn start_peer(&self, offer_sdp: &str) -> Result<String, MediaError> {
        let offer = SdpOffer::from_sdp_string(offer_sdp)
            .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))?;
        let socket = UdpSocket::bind((self.config.bind, 0)).await?;
        let local = socket.local_addr()?;
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .set_reordering_size_audio(REORDERING_SIZE_AUDIO)
            .set_stats_interval(Some(STATS_INTERVAL))
            .build(Instant::now());
        let candidate = Candidate::host(local, "udp")
            .map_err(|e| MediaError::Rtc(format!("host candidate: {e}")))?;
        rtc.add_local_candidate(candidate);
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))?;
        let sdp = answer.to_sdp_string();
        sdp::check_answer(&sdp).map_err(|e| MediaError::Rtc(format!("our answer: {e}")))?;
        *self.shared.fingerprint.lock().expect("fingerprint lock") = sdp::fingerprint_of(&sdp);
        self.shared.reset_for_peer();
        self.shared.set_link(Link::Waiting);

        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let shared = self.shared.clone();
        let connect_timeout = self.config.connect_timeout;
        let task = tokio::spawn(async move {
            run(rtc, socket, local, shared, cmd_rx, connect_timeout).await;
        });
        *self.cmd.lock().expect("cmd lock") = Some(cmd_tx);
        *self.task.lock().await = Some(task);
        Ok(sdp)
    }
}

#[async_trait]
impl Media for Str0mMedia {
    async fn answer(&self, offer_sdp: &str, restart: bool) -> Result<String, MediaError> {
        sdp::parse_offer(offer_sdp).map_err(|e| MediaError::BadOffer(e.to_string()))?;
        if restart {
            info!("rtc: restarting the peer (restart_pc)");
            self.stop_peer(true).await;
        } else if let Some(sender) = self.cmd_sender() {
            let (reply_tx, reply_rx) = oneshot::channel();
            let sent = sender
                .send(Cmd::Offer {
                    sdp: offer_sdp.to_string(),
                    reply: reply_tx,
                })
                .await
                .is_ok();
            if let Ok(result) = reply_rx.await
                && sent
            {
                let sdp = result?;
                *self.shared.fingerprint.lock().expect("fingerprint lock") =
                    sdp::fingerprint_of(&sdp);
                return Ok(sdp);
            }
            // The peer's task is gone: start over.
            self.stop_peer(true).await;
        }
        self.start_peer(offer_sdp).await
    }

    async fn add_remote_candidate(&self, candidate: &str) -> Result<(), MediaError> {
        let Some(bare) = sdp::bare_candidate(candidate) else {
            return Ok(());
        };
        Candidate::from_sdp_string(bare).map_err(|_| MediaError::BadCandidate)?;
        let sender = self.cmd_sender().ok_or(MediaError::NoPeer)?;
        sender
            .send(Cmd::Candidate(bare.to_string()))
            .await
            .map_err(|_| MediaError::NoPeer)
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
        if let Some(sender) = self.cmd_sender() {
            let _ = sender.try_send(Cmd::Flush);
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

/// One peer's life: the run loop.
async fn run(
    mut rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    shared: Arc<Shared>,
    mut cmd_rx: mpsc::Receiver<Cmd>,
    connect_timeout: Duration,
) {
    let (mut inbound, mut outbound) = match (InboundPath::new(), OutboundPath::new()) {
        (Ok(i), Ok(o)) => (i, o),
        _ => {
            warn!("rtc: Opus is not available; the peer cannot start");
            shared.finish();
            return;
        }
    };
    let mut buf = vec![0u8; 2000];
    let mut audio_mid: Option<Mid> = None;
    let mut opus_pt: Option<Pt> = None;
    let mut channel: Option<ChannelId> = None;
    let mut rtp_time: u64 = 0;
    let started = Instant::now();
    let deadline = started + connect_timeout;
    let mut next_frame = started + FRAME;
    let mut connected = false;
    let mut gone = false; // the link is already settled (restart or close handled it)

    'peer: loop {
        if !rtc.is_alive() {
            break;
        }
        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Transmit(t)) => {
                    if let Err(e) = socket.send_to(&t.contents, t.destination).await {
                        debug!("rtc: send failed: {e}");
                    }
                }
                Ok(Output::Timeout(t)) => break t,
                Ok(Output::Event(event)) => match event {
                    Event::Connected => {
                        info!("rtc: peer connected");
                        connected = true;
                        next_frame = Instant::now() + FRAME;
                        if opus_pt.is_none() {
                            opus_pt = find_opus_pt(&rtc);
                        }
                        shared.set_link(Link::Connected);
                    }
                    Event::IceConnectionStateChange(state) => {
                        debug!("rtc: ice {state:?}");
                        if state == IceConnectionState::Disconnected && connected {
                            // The daemon's ping watchdog decides; str0m may come back.
                            debug!("rtc: ice disconnected; waiting on the keepalive");
                        }
                    }
                    Event::MediaAdded(m) => {
                        debug!("rtc: media added {:?} {:?}", m.kind, m.mid);
                        if m.kind == MediaKind::Audio {
                            audio_mid = Some(m.mid);
                            opus_pt = find_opus_pt(&rtc);
                        }
                    }
                    Event::MediaData(d) => {
                        let seq = d.seq_range.end().as_u16();
                        let timestamp = d.time.rebase(Frequency::FORTY_EIGHT_KHZ).numer() as u32;
                        inbound.packet(&shared, &d.data, seq, timestamp, d.network_time);
                    }
                    Event::ChannelOpen(id, label) => {
                        debug!("rtc: channel open {label:?}");
                        channel = Some(id);
                        flush(&mut rtc, id, &shared);
                    }
                    Event::ChannelData(d) => {
                        if !d.binary
                            && let Ok(text) = std::str::from_utf8(&d.data)
                        {
                            shared.on_channel_text(text);
                        }
                    }
                    Event::ChannelClose(_) => channel = None,
                    Event::MediaEgressStats(s) => {
                        let mut stats = shared.stats.lock().expect("stats lock");
                        stats.rtt_ms = s.rtt.map(|d| d.as_secs_f64() * 1000.0);
                        stats.remote_loss = s.loss;
                    }
                    _ => {}
                },
                Err(e) => {
                    warn!("rtc: poll_output failed: {e}");
                    rtc.disconnect();
                    break Instant::now();
                }
            }
        };
        if !rtc.is_alive() {
            break;
        }
        if !connected && Instant::now() > deadline {
            info!("rtc: the peer never connected");
            rtc.disconnect();
            break;
        }

        tokio::select! {
            received = socket.recv_from(&mut buf) => match received {
                Ok((n, source)) => {
                    if let Ok(contents) = buf[..n].try_into() {
                        let input = Input::Receive(
                            Instant::now(),
                            Receive { proto: Protocol::Udp, source, destination: local, contents },
                        );
                        if let Err(e) = rtc.handle_input(input) {
                            debug!("rtc: input rejected: {e}");
                        }
                        if connected {
                            let mut stats = shared.stats.lock().expect("stats lock");
                            if stats.path.is_none() {
                                stats.path = Some(format!("{local} -> {source}"));
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("rtc: socket read failed: {e}");
                    rtc.disconnect();
                }
            },
            _ = tokio::time::sleep_until(timeout.into()) => {
                if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
                    debug!("rtc: timeout rejected: {e}");
                }
            },
            _ = tokio::time::sleep_until(next_frame.into()), if connected => {
                next_frame += FRAME;
                if let (Some(mid), Some(pt)) = (audio_mid, opus_pt) {
                    match outbound.next_packet(&shared) {
                        Ok(packet) => {
                            let time = MediaTime::new(rtp_time, Frequency::FORTY_EIGHT_KHZ);
                            if let Some(writer) = rtc.writer(mid)
                                && let Err(e) = writer.write(pt, Instant::now(), time, packet)
                            {
                                debug!("rtc: write failed: {e}");
                            }
                            rtp_time += frame_samples(WIRE_RATE) as u64;
                        }
                        Err(e) => debug!("rtc: encode failed: {e}"),
                    }
                }
            },
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Offer { sdp, reply }) => {
                    let result = SdpOffer::from_sdp_string(&sdp)
                        .map_err(|_| MediaError::BadOffer("the stack could not parse it".into()))
                        .and_then(|offer| {
                            rtc.sdp_api()
                                .accept_offer(offer)
                                .map_err(|_| MediaError::BadOffer("the stack rejected it".into()))
                        })
                        .map(|answer| answer.to_sdp_string());
                    if result.is_ok() {
                        inbound.reset();
                        opus_pt = find_opus_pt(&rtc);
                    }
                    let _ = reply.send(result);
                }
                Some(Cmd::Candidate(c)) => {
                    if let Ok(candidate) = Candidate::from_sdp_string(&c) {
                        rtc.add_remote_candidate(candidate);
                    }
                }
                Some(Cmd::Flush) => {
                    if let Some(id) = channel {
                        flush(&mut rtc, id, &shared);
                    }
                }
                Some(Cmd::Close { restart, done }) => {
                    gone = restart;
                    rtc.disconnect();
                    // Let str0m say goodbye (DTLS close) before the socket goes.
                    while let Ok(Output::Transmit(t)) = rtc.poll_output() {
                        let _ = socket.send_to(&t.contents, t.destination).await;
                    }
                    let _ = done.send(());
                    break 'peer;
                }
                None => {
                    rtc.disconnect();
                    break 'peer;
                }
            },
        }
    }
    if !gone {
        shared.set_link(Link::Gone);
    }
    debug!("rtc: peer task ended");
}

/// Send what waits in the outbox over an open channel.
fn flush(rtc: &mut Rtc, id: ChannelId, shared: &Shared) {
    let texts = shared.drain_outbox();
    if texts.is_empty() {
        return;
    }
    let Some(mut channel) = rtc.channel(id) else {
        // Not open after all: keep them for the next open.
        let mut outbox = shared.outbox.lock().expect("outbox lock");
        for t in texts.into_iter().rev() {
            outbox.push_front(t);
        }
        return;
    };
    for text in texts {
        if let Err(e) = channel.write(false, text.as_bytes()) {
            debug!("rtc: channel write failed: {e}");
        }
    }
}

/// Opus's payload type as negotiated.
fn find_opus_pt(rtc: &Rtc) -> Option<Pt> {
    rtc.codec_config()
        .params()
        .iter()
        .find(|p| p.spec().codec == Codec::Opus)
        .map(|p| p.pt())
}
