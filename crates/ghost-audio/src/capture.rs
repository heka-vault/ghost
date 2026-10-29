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
use crate::pw::{Session, s16_mono_format_pod};
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
    /// How much audio the ring holds before the oldest is dropped.
    pub ring_seconds: f32,
    /// The stream's `node.name`.
    pub name: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            node: Some("ghost.mic".into()),
            fallback_to_default: false,
            rate: CAPTURE_RATE,
            ring_seconds: 1.0,
            name: "ghost-capture".into(),
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
    ready: mpsc::Sender<Result<String, AudioError>>,
    quit: pipewire::channel::Receiver<()>,
) {
    let session = match Session::connect("ghost") {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.into()));
            return;
        }
    };
    let (target, source) = match &config.node {
        Some(name) => match session.find_node(name) {
            Ok(Some(node)) => (Some(node.serial_or_id().to_string()), name.clone()),
            Ok(None) if config.fallback_to_default => {
                warn!(
                    "audio in: no PipeWire node named {name:?} (is ghost-echo-cancel running?); \
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
    let latency = format!("{}/{}", frame_samples(config.rate), config.rate);
    let rate = format!("1/{}", config.rate);
    let mut props = properties! {
        *keys::MEDIA_TYPE => "Audio",
        *keys::MEDIA_CATEGORY => "Capture",
        *keys::MEDIA_ROLE => "Production",
        *keys::APP_NAME => "ghost",
        *keys::NODE_NAME => config.name.as_str(),
        *keys::NODE_DESCRIPTION => "Ghost (listening)",
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
    let format = s16_mono_format_pod(config.rate);
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
    info!("audio in: {source} @ {} (20 ms frames)", config.rate);
    let _ = ready.send(Ok(source));
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
    source: String,
}

impl Capture {
    /// Open the stream; returns once it is connected, or with the error that stopped it.
    pub fn open(config: CaptureConfig) -> Result<Capture, AudioError> {
        let (producer, consumer) = ring::ring(config.rate, config.ring_seconds);
        let (wake, waiter) =
            Wake::pair().map_err(|e| AudioError::Stream(format!("cannot make an eventfd: {e}")))?;
        let shared = Arc::new(Shared::new(wake));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (quit_tx, quit_rx) = pipewire::channel::channel();
        let frame = frame_samples(config.rate);
        let rate = config.rate;
        let thread = std::thread::Builder::new()
            .name("ghost-capture".into())
            .spawn({
                let shared = shared.clone();
                move || run(config, producer, shared, ready_tx, quit_rx)
            })
            .map_err(|e| AudioError::Stream(format!("cannot start the capture thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(source)) => Ok(Capture {
                shared,
                consumer,
                waiter,
                quit: quit_tx,
                thread: Some(thread),
                frame,
                rate,
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

    /// Samples per frame (320 at 16 kHz).
    pub fn frame_samples(&self) -> usize {
        self.frame
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
