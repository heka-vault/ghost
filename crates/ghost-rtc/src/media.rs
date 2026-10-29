//! The `Media` trait: what the daemon's phone session uses, whichever stack is compiled in.
//!
//! One `Media` is one phone call's media: the daemon makes one per accepted offer (the
//! listener's `POST /offer` without a `pc_id`), hands later offers with that `pc_id` to
//! [`Media::answer`] (`restart` for the protocol's `restart_pc`), reads the inbound frames
//! and the client messages, writes speech into the [`Sink`] and events through
//! [`Media::send`], watches the [`Link`], and closes it when the session ends.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::{Notify, mpsc, watch};

use crate::MediaError;
use crate::frame::{OUT_RATE, PcmFrame, frame_samples};
use crate::rtvi::{ClientMessage, Outbound};
use crate::sdp::Fingerprint;

/// How the stack is set up for one call.
#[derive(Debug, Clone)]
pub struct MediaConfig {
    /// The address the media socket binds; the phone listener's own address, so the
    /// candidates in the answer are on the network the offer came in on (ADR 0020).
    /// Loopback for the tests and the measurements.
    pub bind: IpAddr,
    /// How long a peer may stay unconnected after its offer was answered (the Python's
    /// `PHONE_CONNECT_SECS` is the daemon's; this is the stack's own give-up).
    pub connect_timeout: Duration,
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            connect_timeout: Duration::from_secs(60),
        }
    }
}

/// Where the call stands. `Waiting` before the peer connects and again after a restart;
/// `Gone` once it has left (ICE failed, DTLS closed, or [`Media::close`]). `Gone` is not
/// final on the stack's say-so: a phone that lost its call closes its peer and sends a
/// `restart_pc` offer (ADR 0020), and that answer brings the link back through `Waiting`.
/// The daemon's keepalive grace (`PHONE_PING_GRACE_SECS`) decides when gone is for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Waiting,
    Connected,
    Gone,
}

/// Inbound 20 ms frames at 16 kHz mono s16, in order, a `Stream`. One per call; the daemon
/// takes it once.
pub struct Frames(pub(crate) mpsc::Receiver<PcmFrame>);

impl Frames {
    pub async fn next(&mut self) -> Option<PcmFrame> {
        self.0.recv().await
    }
}

impl Stream for Frames {
    type Item = PcmFrame;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<PcmFrame>> {
        self.0.poll_recv(cx)
    }
}

/// The client messages the data channel let through (`client-ready`, `disconnect-bot`).
pub struct Messages(pub(crate) mpsc::Receiver<ClientMessage>);

impl Messages {
    pub async fn next(&mut self) -> Option<ClientMessage> {
        self.0.recv().await
    }
}

impl Stream for Messages {
    type Item = ClientMessage;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ClientMessage>> {
        self.0.poll_recv(cx)
    }
}

/// Two seconds of 24 kHz audio: what the sink holds before `send` waits. Speech arrives in
/// bursts from the TTS; the stack drains it at 20 ms per packet.
pub const SINK_CAPACITY: usize = OUT_RATE as usize * 2;

pub(crate) struct OutQueue {
    buf: Mutex<VecDeque<i16>>,
    capacity: usize,
    space: Notify,
    closed: AtomicBool,
}

/// The outbound sink: 24 kHz mono s16, any length per write. The stack paces it onto the
/// wire 20 ms at a time and sends silence when it runs dry, as the Python's `RawAudioTrack`
/// did. `clear` is barge-in: what is queued is dropped. Cloning shares the queue.
#[derive(Clone)]
pub struct Sink(pub(crate) Arc<OutQueue>);

impl Default for Sink {
    fn default() -> Self {
        Sink::new(SINK_CAPACITY)
    }
}

impl Sink {
    pub fn new(capacity: usize) -> Self {
        Sink(Arc::new(OutQueue {
            buf: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            space: Notify::new(),
            closed: AtomicBool::new(false),
        }))
    }

    /// Queue samples, waiting for room when the queue is full. Errors once the media is
    /// closed.
    pub async fn send(&self, samples: &[i16]) -> Result<(), MediaError> {
        let mut offset = 0;
        while offset < samples.len() {
            if self.0.closed.load(Ordering::Acquire) {
                return Err(MediaError::SinkClosed);
            }
            let taken = {
                let mut buf = self.0.buf.lock().expect("sink lock");
                let room = self.0.capacity.saturating_sub(buf.len());
                let n = room.min(samples.len() - offset);
                buf.extend(&samples[offset..offset + n]);
                n
            };
            offset += taken;
            if offset < samples.len() {
                self.0.space.notified().await;
            }
        }
        Ok(())
    }

    /// Queue without waiting: what does not fit is dropped and its count returned.
    pub fn try_send(&self, samples: &[i16]) -> usize {
        let mut buf = self.0.buf.lock().expect("sink lock");
        let room = self.0.capacity.saturating_sub(buf.len());
        let n = room.min(samples.len());
        buf.extend(&samples[..n]);
        samples.len() - n
    }

    /// Barge-in: drop what is queued.
    pub fn clear(&self) {
        self.0.buf.lock().expect("sink lock").clear();
        self.0.space.notify_waiters();
    }

    /// Samples waiting to go out.
    pub fn queued(&self) -> usize {
        self.0.buf.lock().expect("sink lock").len()
    }

    /// Milliseconds of audio waiting to go out.
    pub fn queued_ms(&self) -> u32 {
        (self.queued() * 1000 / OUT_RATE as usize) as u32
    }

    /// The next `n` samples for the wire, padded with silence when fewer are queued.
    pub(crate) fn pull(&self, n: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(n);
        {
            let mut buf = self.0.buf.lock().expect("sink lock");
            let take = n.min(buf.len());
            out.extend(buf.drain(..take));
        }
        if out.len() < n {
            out.resize(n, 0);
        }
        self.0.space.notify_waiters();
        out
    }

    /// One 20 ms frame for the wire.
    #[cfg_attr(not(any(feature = "str0m", feature = "webrtc")), allow(dead_code))]
    pub(crate) fn pull_frame(&self) -> Vec<i16> {
        self.pull(frame_samples(OUT_RATE))
    }

    pub(crate) fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.space.notify_waiters();
    }
}

/// Counts for the log and the measurements. Numbers only, never audio.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct CallStats {
    /// RTP packets received from the phone.
    pub packets_received: u64,
    /// Packets the sequence numbers say never arrived.
    pub packets_lost: u64,
    /// Frames the decoder concealed for lost packets.
    pub concealed: u64,
    /// Inbound frames dropped because the daemon was not reading.
    pub frames_dropped: u64,
    /// RTP packets sent to the phone.
    pub packets_sent: u64,
    pub bytes_received: u64,
    pub bytes_sent: u64,
    /// RFC 3550 interarrival jitter of the inbound stream, in milliseconds.
    pub jitter_ms: f64,
    /// What the phone reported about our stream in its last receiver report, if the stack
    /// surfaces it: its loss fraction and round trip.
    pub remote_loss: Option<f32>,
    pub rtt_ms: Option<f64>,
    /// The ICE pair in use, `local -> remote`, for the log (which network the phone came
    /// in on, ADR 0020).
    pub path: Option<String>,
}

/// One phone call's media. `Send + Sync` so the listener's handler and the session share it.
#[async_trait]
pub trait Media: Send + Sync {
    /// Answer the phone's offer. Without a live peer it starts one; with one and
    /// `restart` false it renegotiates in place; with `restart` true (the protocol's
    /// `restart_pc`) it drops the live peer and starts fresh, the link going back to
    /// `Waiting`, never `Gone`. Returns the answer SDP, candidates included.
    async fn answer(&self, offer_sdp: &str, restart: bool) -> Result<String, MediaError>;

    /// A trickled remote candidate (`PATCH /offer`). The `candidate:` prefix is optional;
    /// an empty one is end of candidates and does nothing.
    async fn add_remote_candidate(&self, candidate: &str) -> Result<(), MediaError>;

    /// The DTLS fingerprint the latest answer carries: the one the phone pins through the
    /// signed answer. `None` before the first answer.
    fn fingerprint(&self) -> Option<Fingerprint>;

    /// The inbound frames. Once; `None` after.
    fn take_frames(&self) -> Option<Frames>;

    /// The outbound sink.
    fn sink(&self) -> Sink;

    /// The client messages that passed the RTVI subset. Once; `None` after.
    fn take_messages(&self) -> Option<Messages>;

    /// Send to the phone over the data channel. Held until the channel opens (fifty at
    /// most, as the Python did), then in order.
    fn send(&self, message: &Outbound) -> Result<(), MediaError>;

    /// The link state, for `changed().await`.
    fn link(&self) -> watch::Receiver<Link>;

    /// The Python's `is_connected`: the phone's `ping` seen in the last 3 s, or, if it has
    /// never pinged, the link itself. The daemon's keepalive watchdog polls this.
    fn is_connected(&self) -> bool;

    fn stats(&self) -> CallStats;

    /// End the call: the peer is told, the sockets close, the link goes `Gone`, the sink
    /// and the streams end.
    async fn close(&self);
}

/// The ping rule: how long after the phone's last `ping` it still counts as connected.
pub const PING_WINDOW: Duration = Duration::from_secs(3);

/// Where the phone's last ping landed, for [`Media::is_connected`].
pub(crate) fn pinged_recently(last: Option<Instant>) -> Option<bool> {
    last.map(|t| t.elapsed() < PING_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sink_queues_pulls_and_pads() {
        let sink = Sink::new(1000);
        sink.send(&[1, 2, 3]).await.unwrap();
        assert_eq!(sink.queued(), 3);
        assert_eq!(sink.pull(5), vec![1, 2, 3, 0, 0]);
        assert_eq!(sink.queued(), 0);
        assert_eq!(sink.try_send(&[7; 1200]), 200);
        assert_eq!(sink.queued(), 1000);
        sink.clear();
        assert_eq!(sink.queued(), 0);
        assert_eq!(sink.pull_frame().len(), 480);
    }

    #[tokio::test]
    async fn sink_send_waits_for_room_and_errors_when_closed() {
        let sink = Sink::new(4);
        sink.send(&[1, 2, 3, 4]).await.unwrap();
        let writer = {
            let s = sink.clone();
            tokio::spawn(async move { s.send(&[5, 6]).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!writer.is_finished());
        assert_eq!(sink.pull(2), vec![1, 2]);
        writer.await.unwrap().unwrap();
        assert_eq!(sink.pull(4), vec![3, 4, 5, 6]);
        sink.close();
        assert!(matches!(sink.send(&[1]).await, Err(MediaError::SinkClosed)));
    }

    #[test]
    fn ping_rule() {
        assert_eq!(pinged_recently(None), None);
        assert_eq!(pinged_recently(Some(Instant::now())), Some(true));
        assert_eq!(
            pinged_recently(Some(Instant::now() - Duration::from_secs(4))),
            Some(false)
        );
    }
}
