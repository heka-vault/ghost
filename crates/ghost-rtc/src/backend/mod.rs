//! The three stacks, each behind a feature, and what they share: the channels the daemon
//! reads and writes, the link state, the ping rule, the inbound path (packet to 16 kHz
//! frame) and the outbound path (24 kHz frame to packet).
#![cfg_attr(
    not(any(feature = "str0m", feature = "webrtc", feature = "libwebrtc")),
    allow(dead_code)
)]

#[cfg(feature = "libwebrtc")]
pub mod libwebrtc;
#[cfg(feature = "str0m")]
pub mod str0m;
#[cfg(feature = "webrtc")]
pub mod webrtc;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::MediaError;
use crate::codec::{OpusDecoder, OpusEncoder};
use crate::frame::{IN_RATE, PcmFrame, WIRE_RATE};
use crate::media::{CallStats, Frames, Link, Messages, Sink, pinged_recently};
use crate::resample::Resampler;
use crate::rtvi::{ClientMessage, Inbound, OUTBOX_CAPACITY, Outbound, parse_inbound};
use crate::sdp::Fingerprint;
use crate::stats::RtpCounters;

/// Inbound frames the daemon has not read yet: 2 s before frames are dropped.
pub(crate) const FRAMES_CAPACITY: usize = 100;
/// Client messages not yet read.
pub(crate) const MESSAGES_CAPACITY: usize = 16;
/// The most packets concealed in a row; past that a gap is a silence, not a guess.
pub(crate) const MAX_CONCEAL: u16 = 5;

/// What every stack keeps for the daemon, outside the stack's own peer.
pub(crate) struct Shared {
    pub frames_tx: mpsc::Sender<PcmFrame>,
    frames_rx: Mutex<Option<Frames>>,
    pub sink: Sink,
    pub msgs_tx: mpsc::Sender<ClientMessage>,
    msgs_rx: Mutex<Option<Messages>>,
    /// Outbound data-channel texts waiting for the channel to open.
    pub outbox: Mutex<VecDeque<String>>,
    pub link: watch::Sender<Link>,
    pub last_ping: Mutex<Option<Instant>>,
    pub stats: Mutex<CallStats>,
    pub counters: Mutex<RtpCounters>,
    pub fingerprint: Mutex<Option<Fingerprint>>,
    pub epoch: Instant,
}

impl Shared {
    pub(crate) fn new() -> Shared {
        let (frames_tx, frames_rx) = mpsc::channel(FRAMES_CAPACITY);
        let (msgs_tx, msgs_rx) = mpsc::channel(MESSAGES_CAPACITY);
        let (link, _) = watch::channel(Link::Waiting);
        Shared {
            frames_tx,
            frames_rx: Mutex::new(Some(Frames(frames_rx))),
            sink: Sink::default(),
            msgs_tx,
            msgs_rx: Mutex::new(Some(Messages(msgs_rx))),
            outbox: Mutex::new(VecDeque::new()),
            link,
            last_ping: Mutex::new(None),
            stats: Mutex::new(CallStats::default()),
            counters: Mutex::new(RtpCounters::new(WIRE_RATE)),
            fingerprint: Mutex::new(None),
            epoch: Instant::now(),
        }
    }

    pub(crate) fn take_frames(&self) -> Option<Frames> {
        self.frames_rx.lock().expect("frames lock").take()
    }

    pub(crate) fn take_messages(&self) -> Option<Messages> {
        self.msgs_rx.lock().expect("messages lock").take()
    }

    /// A text from the data channel: the ping, a client message, or nothing kept.
    pub(crate) fn on_channel_text(&self, text: &str) {
        match parse_inbound(text) {
            Inbound::Ping => *self.last_ping.lock().expect("ping lock") = Some(Instant::now()),
            Inbound::Client(m) => {
                // A daemon that is not reading loses the message rather than the call.
                let _ = self.msgs_tx.try_send(m);
            }
            Inbound::Dropped => {}
        }
    }

    /// Queue a message for the channel, fifty at most.
    pub(crate) fn queue_outbound(&self, message: &Outbound) -> Result<(), MediaError> {
        let mut outbox = self.outbox.lock().expect("outbox lock");
        if outbox.len() >= OUTBOX_CAPACITY {
            return Err(MediaError::OutboxFull);
        }
        outbox.push_back(message.to_json());
        Ok(())
    }

    /// Everything queued, in order, for a channel that is open.
    pub(crate) fn drain_outbox(&self) -> Vec<String> {
        self.outbox.lock().expect("outbox lock").drain(..).collect()
    }

    pub(crate) fn set_link(&self, link: Link) {
        self.link.send_if_modified(|l| {
            if *l == link {
                false
            } else {
                *l = link;
                true
            }
        });
    }

    pub(crate) fn link_now(&self) -> Link {
        *self.link.borrow()
    }

    pub(crate) fn is_connected(&self) -> bool {
        match pinged_recently(*self.last_ping.lock().expect("ping lock")) {
            Some(recent) => recent,
            None => self.link_now() == Link::Connected,
        }
    }

    pub(crate) fn stats(&self) -> CallStats {
        let mut s = self.stats.lock().expect("stats lock").clone();
        let c = self.counters.lock().expect("counters lock");
        s.packets_received = c.received();
        s.bytes_received = c.bytes();
        s.packets_lost = c.lost();
        s.jitter_ms = c.jitter_ms();
        s
    }

    /// A fresh peer: the counters and the ping start over; the daemon's streams stay.
    pub(crate) fn reset_for_peer(&self) {
        *self.counters.lock().expect("counters lock") = RtpCounters::new(WIRE_RATE);
        *self.last_ping.lock().expect("ping lock") = None;
        let mut s = self.stats.lock().expect("stats lock");
        s.path = None;
        s.remote_loss = None;
        s.rtt_ms = None;
    }

    /// The call is over: the sink refuses writes and the streams end once drained.
    pub(crate) fn finish(&self) {
        self.sink.close();
        self.set_link(Link::Gone);
    }
}

/// The inbound path: one Opus packet to 20 ms frames at 16 kHz, with loss concealment.
pub(crate) struct InboundPath {
    decoder: OpusDecoder,
    resampler: Resampler,
}

impl InboundPath {
    pub(crate) fn new() -> Result<Self, MediaError> {
        Ok(InboundPath {
            decoder: OpusDecoder::new()?,
            resampler: Resampler::downsampler()?,
        })
    }

    /// One RTP packet's payload with its sequence number, timestamp and arrival time.
    pub(crate) fn packet(
        &mut self,
        shared: &Shared,
        payload: &[u8],
        seq: u16,
        timestamp: u32,
        arrival: Instant,
    ) {
        let arrived = shared.counters.lock().expect("counters lock").packet(
            seq,
            timestamp,
            arrival,
            payload.len(),
            shared.epoch,
        );
        if arrived.late {
            return;
        }
        for _ in 0..arrived.gap.min(MAX_CONCEAL) {
            if let Ok(pcm) = self.decoder.conceal() {
                self.resampler.push(&pcm);
                shared.stats.lock().expect("stats lock").concealed += 1;
            }
        }
        match self.decoder.decode(payload) {
            Ok(pcm) => self.resampler.push(&pcm),
            Err(_) => return, // a packet Opus will not take: skipped, not logged
        }
        while let Some(samples) = self.resampler.pull_frame() {
            let frame = PcmFrame::new(samples, IN_RATE);
            if shared.frames_tx.try_send(frame).is_err() {
                shared.stats.lock().expect("stats lock").frames_dropped += 1;
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.resampler.reset();
    }
}

/// The outbound path: 20 ms of the sink at 24 kHz to one Opus packet at 48 kHz.
pub(crate) struct OutboundPath {
    encoder: OpusEncoder,
    resampler: Resampler,
}

impl OutboundPath {
    pub(crate) fn new() -> Result<Self, MediaError> {
        Ok(OutboundPath {
            encoder: OpusEncoder::new()?,
            resampler: Resampler::upsampler()?,
        })
    }

    /// The next packet: the sink's next 20 ms (silence when it is dry), resampled and
    /// encoded. The resampler's delay means the first call encodes silence.
    pub(crate) fn next_packet(&mut self, shared: &Shared) -> Result<Vec<u8>, MediaError> {
        let pcm24 = shared.sink.pull_frame();
        self.resampler.push(&pcm24);
        let pcm48 = self
            .resampler
            .pull_frame()
            .unwrap_or_else(|| vec![0; crate::frame::frame_samples(WIRE_RATE)]);
        let packet = self.encoder.encode(&pcm48)?;
        let mut s = shared.stats.lock().expect("stats lock");
        s.packets_sent += 1;
        s.bytes_sent += packet.len() as u64;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marker::{MarkerTone, OnsetDetector};
    use std::time::Duration;

    #[tokio::test]
    async fn the_inbound_path_turns_packets_into_16k_frames_and_conceals_gaps() {
        let shared = Shared::new();
        let mut frames = shared.take_frames().unwrap();
        assert!(shared.take_frames().is_none());
        let mut path = InboundPath::new().unwrap();
        let mut enc = OpusEncoder::new().unwrap();
        let mut tone = MarkerTone::new(WIRE_RATE);
        let t0 = Instant::now();
        let mut seq = 0u16;
        for i in 0..100u32 {
            let (pcm, _) = tone.frame(960);
            let packet = enc.encode(&pcm).unwrap();
            if i == 40 {
                seq = seq.wrapping_add(1); // one packet lost
            }
            path.packet(
                &shared,
                &packet,
                seq,
                i * 960,
                t0 + Duration::from_millis(i as u64 * 20),
            );
            seq = seq.wrapping_add(1);
        }
        let stats = shared.stats();
        assert_eq!(stats.packets_received, 100);
        assert_eq!(stats.packets_lost, 1);
        assert_eq!(stats.concealed, 1);
        assert_eq!(stats.frames_dropped, 0);
        assert!(stats.jitter_ms < 0.1);
        let mut det = OnsetDetector::new(IN_RATE);
        let mut got = 0;
        let mut onsets = 0;
        while let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(10), frames.next()).await
        {
            assert_eq!(f.samples.len(), 320);
            assert_eq!(f.rate, IN_RATE);
            got += 1;
            if det.feed(&f.samples).is_some() {
                onsets += 1;
            }
        }
        // 100 packets in, a frame per packet less the resampler's delay, bursts at 0 and 1 s.
        assert!((97..=100).contains(&got), "{got} frames");
        assert_eq!(onsets, 2);
    }

    #[tokio::test]
    async fn the_outbound_path_paces_the_sink_and_sends_silence_when_dry() {
        let shared = Shared::new();
        let mut path = OutboundPath::new().unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let dry = path.next_packet(&shared).unwrap();
        assert!(dry.len() < 100, "a dry packet is small, got {}", dry.len());
        let mut tone = MarkerTone::new(crate::frame::OUT_RATE);
        let (pcm, _) = tone.frame(480 * 10);
        shared.sink.send(&pcm).await.unwrap();
        let mut loud = 0;
        for _ in 0..12 {
            let packet = path.next_packet(&shared).unwrap();
            let pcm = dec.decode(&packet).unwrap();
            assert_eq!(pcm.len(), 960);
            if crate::frame::peak(&pcm) > 4000 {
                loud += 1;
            }
        }
        // 200 ms of burst = 10 frames, minus what the resampler's delay slides.
        assert!((9..=11).contains(&loud), "{loud} loud frames");
        assert_eq!(shared.sink.queued(), 0);
        assert_eq!(shared.stats().packets_sent, 13);
    }

    #[test]
    fn channel_texts_ping_or_pass_or_vanish_and_the_outbox_is_bounded() {
        let shared = Shared::new();
        assert!(!shared.is_connected());
        shared.set_link(Link::Connected);
        assert!(shared.is_connected());
        shared.on_channel_text("ping");
        assert!(shared.is_connected());
        *shared.last_ping.lock().unwrap() = Some(Instant::now() - Duration::from_secs(4));
        assert!(!shared.is_connected(), "a stale ping outranks the link");
        let mut msgs = shared.take_messages().unwrap();
        shared.on_channel_text(
            r#"{"label":"rtvi-ai","type":"send-text","id":"1","data":{"content":"x"}}"#,
        );
        shared.on_channel_text(r#"{"label":"rtvi-ai","type":"disconnect-bot","id":"9","data":{}}"#);
        assert_eq!(
            msgs.0.try_recv().unwrap(),
            ClientMessage::DisconnectBot { id: "9".into() }
        );
        assert!(msgs.0.try_recv().is_err());
        for _ in 0..OUTBOX_CAPACITY {
            shared
                .queue_outbound(&Outbound::BotStartedSpeaking)
                .unwrap();
        }
        assert!(matches!(
            shared.queue_outbound(&Outbound::BotStoppedSpeaking),
            Err(MediaError::OutboxFull)
        ));
        assert_eq!(shared.drain_outbox().len(), OUTBOX_CAPACITY);
        assert!(shared.drain_outbox().is_empty());
        shared.finish();
        assert_eq!(shared.link_now(), Link::Gone);
    }
}
