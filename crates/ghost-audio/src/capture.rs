//! The microphone: a PipeWire capture stream on the node named `ghost.mic` (the output of
//! the echo canceller, `infra/pipewire`), 16 kHz mono s16, asking for a 20 ms quantum,
//! running on PipeWire's own thread. The process callback copies samples into a lock-free
//! ring and allocates nothing; the consumer side is an async stream of 20 ms frames for
//! tokio. What does not fit in the ring is dropped and counted as an overrun; nothing is
//! logged per frame, and nothing from the microphone is written anywhere.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::Stream;
use pipewire::keys;
use pipewire::properties::properties;
use pipewire::spa::pod::Pod;
use pipewire::spa::utils::Direction;
use pipewire::stream::{Stream as PwStream, StreamFlags, StreamRc, StreamState};
use rtrb::{Consumer, Producer};
use tracing::{info, warn};

use crate::frame::{CAPTURE_RATE, Frame, frame_samples};
use crate::pw::{Session, s16_format_pod};
use crate::wake::{Waiter, Wake};
use crate::{AudioError, ring};

/// Where and how to listen.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// The node to capture from, by `node.name`; `GHOST_INPUT_DEVICE`. `None`: the
    /// default source.
    pub node: Option<String>,
    /// When the named node is absent: `true` warns and captures from the default source,
    /// `false` fails with [`AudioError::NoSuchSource`]. Off by default: the raw
    /// microphone has no echo cancellation, and Ghost would hear itself.
    pub fallback_to_default: bool,
    pub rate: u32,
    /// Channels to capture: 1 (the default), or the node's own for a recording of every
    /// channel (the Hearing test's raw array, ADR 0042). Frames are interleaved.
    pub channels: u16,
    /// Take this one channel of the node alone (ADR 0064): every channel is captured and
    /// the others are dropped here, so nothing is mixed in (an array's raw microphone
    /// beside its processed beam). Frames are mono; `channels` is ignored.
    pub take_channel: Option<u16>,
    /// How much audio the ring holds before the oldest is dropped.
    pub ring_seconds: f32,
    /// The stream's `node.name`.
    pub name: String,
    /// The stream's `node.description`, as PipeWire's tools show it.
    pub description: String,
}

/// The echo canceller's output (`infra/pipewire`), used when no device is named and it is
/// there.
pub const ECHO_CANCELLED_SOURCE: &str = "ghost.mic";

impl CaptureConfig {
    /// The capture for `GHOST_INPUT_DEVICE`. Named, that node and no other (missing is an
    /// error). Unset, `ghost.mic` when the echo canceller made one, else the default
    /// source: Ghost starts on any machine, and on open speakers without the canceller it
    /// may hear itself (README.md, "Sound").
    /// The capture for a resolved input (ADR 0064): its node and the channel taken alone.
    /// Nothing named (the sound devices could not be read): `for_input(None)`'s rule.
    pub fn for_spec(spec: &crate::InputSpec) -> Self {
        let mut c = match &spec.node {
            Some(n) => Self::for_input(Some(n.clone())),
            None if spec.channel.is_some() => Self {
                node: None,
                ..Self::default()
            },
            None => Self::for_input(None),
        };
        c.take_channel = spec.channel;
        c
    }

    pub fn for_input(input_device: Option<String>) -> Self {
        match input_device {
            Some(name) => Self {
                node: Some(name),
                fallback_to_default: false,
                ..Self::default()
            },
            None => Self {
                node: Some(ECHO_CANCELLED_SOURCE.into()),
                fallback_to_default: true,
                ..Self::default()
            },
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            node: Some(ECHO_CANCELLED_SOURCE.into()),
            fallback_to_default: false,
            rate: CAPTURE_RATE,
            channels: 1,
            take_channel: None,
            ring_seconds: 1.0,
            name: "ghost-capture".into(),
            description: "Ghost (listening)".into(),
        }
    }
}

/// Numbers only, never audio. Read any time from any thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureCounters {
    /// Process callbacks run.
    pub callbacks: u64,
    /// Samples delivered into the ring.
    pub samples: u64,
    /// Samples dropped because the ring was full (the consumer fell behind).
    pub overruns: u64,
    /// Callbacks that found no buffer to dequeue.
    pub empty_callbacks: u64,
    /// PipeWire's reported delay from the device, in samples, at the last callback.
    pub delay_samples: i64,
}

struct Shared {
    epoch: Instant,
    wake: Wake,
    stopped: AtomicBool,
    callbacks: AtomicU64,
    samples: AtomicU64,
    overruns: AtomicU64,
    empty: AtomicU64,
    delay: AtomicI64,
    last_ns: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn new(wake: Wake) -> Self {
        Self {
            epoch: Instant::now(),
            wake,
            stopped: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            empty: AtomicU64::new(0),
            delay: AtomicI64::new(0),
            last_ns: AtomicU64::new(0),
            error: Mutex::new(None),
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Relaxed);
        self.wake.signal();
    }
}

struct CaptureData {
    producer: Producer<i16>,
    shared: Arc<Shared>,
}

/// The realtime side. No allocation, no lock, no log line.
fn process(stream: &PwStream, data: &mut CaptureData) {
    let shared = &*data.shared;
    shared.callbacks.fetch_add(1, Relaxed);
    let Some(mut buffer) = stream.dequeue_buffer() else {
        shared.empty.fetch_add(1, Relaxed);
        return;
    };
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else {
        return;
    };
    let offset = d.chunk().offset() as usize;
    let size = d.chunk().size() as usize;
    let Some(bytes) = d.data() else {
        return;
    };
    let end = offset.saturating_add(size).min(bytes.len());
    let start = offset.min(end);
    let (written, dropped) = ring::push_le_bytes(&mut data.producer, &bytes[start..end]);
    shared.samples.fetch_add(written as u64, Relaxed);
    if dropped > 0 {
        shared.overruns.fetch_add(dropped as u64, Relaxed);
    }
    if let Ok(t) = stream.time() {
        shared.delay.store(t.delay(), Relaxed);
    }
    shared
        .last_ns
        .store(shared.epoch.elapsed().as_nanos() as u64, Relaxed);
    shared.wake.signal();
}

fn run(
    config: CaptureConfig,
    producer: Producer<i16>,
    shared: Arc<Shared>,
    ready: mpsc::Sender<Result<(String, u16), AudioError>>,
    quit: pipewire::channel::Receiver<()>,
) {
    let session = match Session::connect("ghost") {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.into()));
            return;
        }
    };
    let mut node_channels = None;
    let (target, source) = match &config.node {
        Some(name) => match session.find_node(name) {
            Ok(Some(node)) => {
                node_channels = node
                    .props
                    .get("audio.channels")
                    .and_then(|c| c.trim().parse::<u16>().ok());
                (Some(node.serial_or_id().to_string()), name.clone())
            }
            Ok(None) if config.fallback_to_default => {
                info!(
                    "audio in: no PipeWire node named {name:?} (no echo canceller set up); \
                     capturing from the default source"
                );
                (None, "default source".to_string())
            }
            Ok(None) => {
                let _ = ready.send(Err(AudioError::NoSuchSource { name: name.clone() }));
                return;
            }
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                return;
            }
        },
        None => (None, "default source".to_string()),
    };
    // One channel alone: capture all of them (at least up to it), never a mixdown.
    let channels = match config.take_channel {
        Some(c) => node_channels.unwrap_or(2).max(c + 1).min(MAX_CHANNELS),
        None => config.channels.max(1),
    };
    let latency = format!("{}/{}", frame_samples(config.rate), config.rate);
    let rate = format!("1/{}", config.rate);
    let mut props = properties! {
        *keys::MEDIA_TYPE => "Audio",
        *keys::MEDIA_CATEGORY => "Capture",
        *keys::MEDIA_ROLE => "Production",
        *keys::APP_NAME => "ghost",
        *keys::NODE_NAME => config.name.as_str(),
        *keys::NODE_DESCRIPTION => config.description.as_str(),
        *keys::NODE_LATENCY => latency.as_str(),
        *keys::NODE_RATE => rate.as_str(),
    };
    if let Some(target) = target {
        props.insert(*keys::TARGET_OBJECT, target);
    }
    let stream = match StreamRc::new(session.core().clone(), &config.name, props) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(AudioError::Stream(e.to_string())));
            return;
        }
    };
    let data = CaptureData {
        producer,
        shared: shared.clone(),
    };
    let listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(|_stream, data, _old, new| match new {
            StreamState::Error(e) => {
                warn!("audio in: stream error: {e}");
                *data.shared.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
            }
            StreamState::Streaming => info!("audio in: streaming"),
            _ => {}
        })
        .process(process)
        .register();
    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(AudioError::Stream(e.to_string())));
            return;
        }
    };
    let format = s16_format_pod(config.rate, channels);
    let mut params = [Pod::from_bytes(&format).expect("a format pod")];
    if let Err(e) = stream.connect(
        Direction::Input,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        let _ = ready.send(Err(AudioError::Stream(e.to_string())));
        return;
    }
    let mainloop = session.mainloop().clone();
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    match config.take_channel {
        Some(c) => info!(
            "audio in: {source} @ {}, channel {c} of {channels} alone (20 ms frames)",
            config.rate
        ),
        None => info!(
            "audio in: {source} @ {}, {channels} channel(s) (20 ms frames)",
            config.rate
        ),
    }
    let _ = ready.send(Ok((source, channels)));
    mainloop.run();
    let _ = stream.disconnect();
    drop(listener);
    shared.stop();
}

/// An open capture stream. Dropping it stops the stream and joins PipeWire's thread.
pub struct Capture {
    shared: Arc<Shared>,
    consumer: Consumer<i16>,
    waiter: Waiter,
    quit: pipewire::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    frame: usize,
    rate: u32,
    channels: u16,
    /// The channel kept when one is taken alone.
    take: Option<u16>,
    source: String,
}

/// The most channels a capture opens to take one of them alone.
pub const MAX_CHANNELS: u16 = 16;

/// Channel `take` of interleaved `samples` with `channels` channels.
pub fn take_channel(samples: &[i16], channels: u16, take: u16) -> Vec<i16> {
    let n = usize::from(channels.max(1));
    let t = usize::from(take).min(n - 1);
    samples.iter().skip(t).step_by(n).copied().collect()
}

impl Capture {
    /// Open the stream; returns once it is connected, or with the error that stopped it.
    pub fn open(config: CaptureConfig) -> Result<Capture, AudioError> {
        let take = config.take_channel;
        // With one channel taken alone the node's count is learnt once connected; the ring
        // holds the most there can be.
        let ring_channels = match take {
            Some(_) => MAX_CHANNELS,
            None => config.channels.max(1),
        };
        let (producer, consumer) =
            ring::ring(config.rate * u32::from(ring_channels), config.ring_seconds);
        let (wake, waiter) =
            Wake::pair().map_err(|e| AudioError::Stream(format!("cannot make an eventfd: {e}")))?;
        let shared = Arc::new(Shared::new(wake));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (quit_tx, quit_rx) = pipewire::channel::channel();
        let rate = config.rate;
        let thread = std::thread::Builder::new()
            .name("ghost-capture".into())
            .spawn({
                let shared = shared.clone();
                move || run(config, producer, shared, ready_tx, quit_rx)
            })
            .map_err(|e| AudioError::Stream(format!("cannot start the capture thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok((source, channels))) => Ok(Capture {
                shared,
                consumer,
                waiter,
                quit: quit_tx,
                thread: Some(thread),
                frame: frame_samples(rate) * usize::from(channels),
                rate,
                channels,
                take,
                source,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Closed)
            }
        }
    }

    /// The node being captured: the configured name, or "default source".
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Samples per frame as captured, every channel's (320 at 16 kHz mono).
    pub fn frame_samples(&self) -> usize {
        self.frame
    }

    /// The channels of each frame `next_frame` returns: 1 when one is taken alone.
    pub fn channels(&self) -> u16 {
        if self.take.is_some() {
            1
        } else {
            self.channels
        }
    }

    pub fn counters(&self) -> CaptureCounters {
        CaptureCounters {
            callbacks: self.shared.callbacks.load(Relaxed),
            samples: self.shared.samples.load(Relaxed),
            overruns: self.shared.overruns.load(Relaxed),
            empty_callbacks: self.shared.empty.load(Relaxed),
            delay_samples: self.shared.delay.load(Relaxed),
        }
    }

    /// The stream's error, if it failed after opening.
    pub fn error(&self) -> Option<String> {
        self.shared
            .error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Samples waiting in the ring.
    pub fn queued(&self) -> usize {
        self.consumer.slots()
    }

    /// The next 20 ms frame, or `None` once the stream has stopped.
    pub async fn next_frame(&mut self) -> Option<Frame> {
        loop {
            if let Some(samples) = ring::pop_frame(&mut self.consumer, self.frame) {
                let samples = match self.take {
                    Some(t) => take_channel(&samples, self.channels, t),
                    None => samples,
                };
                let at =
                    self.shared.epoch + Duration::from_nanos(self.shared.last_ns.load(Relaxed));
                return Some(Frame {
                    samples,
                    rate: self.rate,
                    captured_at: at,
                });
            }
            if self.shared.stopped.load(Relaxed) {
                return None;
            }
            if self.waiter.wait().await.is_err() {
                return None;
            }
        }
    }

    /// The frames as a `Stream`; ends when the capture stops.
    pub fn frames(self) -> impl Stream<Item = Frame> {
        futures::stream::unfold(self, |mut capture| async move {
            capture.next_frame().await.map(|frame| (frame, capture))
        })
    }

    /// Stop the stream and join its thread.
    pub fn close(self) {
        drop(self);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine without Ghost's echo canceller still hears: with no
    /// `GHOST_INPUT_DEVICE`, `ghost.mic` is used when it is there and the default
    /// source otherwise, so a laptop's microphone or a headset works out of the box.
    #[test]
    fn no_device_named_prefers_ghost_mic_then_the_default_source() {
        let c = CaptureConfig::for_input(None);
        assert_eq!(c.node.as_deref(), Some(ECHO_CANCELLED_SOURCE));
        assert!(c.fallback_to_default);
    }

    /// A device the operator named is that device and no other: a missing one is an
    /// error that names it, never a silent switch to another microphone.
    #[test]
    fn a_named_device_does_not_fall_back() {
        let c = CaptureConfig::for_input(Some("alsa_input.usb-headset".into()));
        assert_eq!(c.node.as_deref(), Some("alsa_input.usb-headset"));
        assert!(!c.fallback_to_default);
        let c = CaptureConfig::for_input(Some("ghost.mic".into()));
        assert!(!c.fallback_to_default);
    }

    /// ADR 0064: an input with a channel names that device and channel; with nothing
    /// named, `for_input`'s rule.
    #[test]
    fn an_input_spec_names_its_device_and_channel() {
        let c = CaptureConfig::for_spec(&crate::InputSpec::parse(Some("alsa_input.array#0")));
        assert_eq!(c.node.as_deref(), Some("alsa_input.array"));
        assert_eq!(c.take_channel, Some(0));
        assert!(!c.fallback_to_default);
        let c = CaptureConfig::for_spec(&crate::InputSpec::default());
        assert_eq!(c.node.as_deref(), Some(ECHO_CANCELLED_SOURCE));
        assert!(c.fallback_to_default);
        assert_eq!(c.take_channel, None);
    }

    #[test]
    fn one_channel_is_taken_alone_never_mixed() {
        // An array's two channels: its processed beam (1s) and its raw microphone (9s).
        let interleaved: Vec<i16> = (0..6).flat_map(|_| [1i16, 9]).collect();
        assert_eq!(take_channel(&interleaved, 2, 0), vec![1; 6]);
        assert_eq!(take_channel(&interleaved, 2, 1), vec![9; 6]);
        // Past the last channel: the last one, never a panic.
        assert_eq!(take_channel(&interleaved, 2, 5), vec![9; 6]);
        assert_eq!(take_channel(&[3, 4, 5], 1, 0), vec![3, 4, 5]);
    }
}
