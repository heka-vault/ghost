//! The speaker: a PipeWire playback stream to the default sink (or the one
//! `GHOST_OUTPUT_DEVICE` names), 24 kHz mono s16, fed from a ring the daemon writes. The
//! process callback pulls what is queued, pads with silence, mixes the thinking sound under
//! it when the mixer is on, and allocates nothing. `clear()` is barge-in: what is queued is
//! dropped within one quantum. `drain()` resolves when what was queued has played.

use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU64,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire::keys;
use pipewire::properties::properties;
use pipewire::spa::pod::Pod;
use pipewire::spa::utils::Direction;
use pipewire::stream::{Stream as PwStream, StreamFlags, StreamRc, StreamState};
use rtrb::{Consumer, Producer, RingBuffer};
use tracing::{info, warn};

use crate::frame::{FRAME_MS, PLAYBACK_RATE, frame_samples};
use crate::mixer::Mixer;
use crate::pw::{Session, s16_mono_format_pod};
use crate::wake::{Waiter, Wake};
use crate::{AudioError, ring};

/// Where and how to play.
#[derive(Debug, Clone)]
pub struct PlaybackConfig {
    /// The sink, by `node.name`; `GHOST_OUTPUT_DEVICE`. `None`: the default sink.
    pub sink: Option<String>,
    pub rate: u32,
    /// How far ahead the daemon may write; `write` waits for room beyond it.
    pub ring_seconds: f32,
    /// The stream's `node.name`.
    pub name: String,
    /// The sound the mixer plays under speech (the thinking loop), at `rate`.
    pub sound: Option<Arc<Vec<i16>>>,
    /// Its level as a fraction of full scale; `GHOST_THINKING_VOLUME`.
    pub sound_volume: f64,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            sink: None,
            rate: PLAYBACK_RATE,
            ring_seconds: 0.5,
            name: "ghost-playback".into(),
            sound: None,
            sound_volume: 0.12,
        }
    }
}

/// Numbers only. Read any time from any thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackCounters {
    /// Process callbacks run.
    pub callbacks: u64,
    /// Samples taken from the ring and handed to PipeWire.
    pub played: u64,
    /// Cycles where speech was in flight and the ring ran short: the daemon fed it too
    /// slowly. The end of an utterance counts when `drain` was not called for it.
    pub underruns: u64,
    /// Cycles with nothing queued at all (idle, or the thinking sound alone).
    pub silent_cycles: u64,
    /// Callbacks that found no buffer to dequeue.
    pub empty_callbacks: u64,
    /// PipeWire's reported delay to the device, in samples, at the last callback.
    pub delay_samples: i64,
}

struct Shared {
    wake: Wake,
    stopped: AtomicBool,
    callbacks: AtomicU64,
    played: AtomicU64,
    underruns: AtomicU64,
    silent: AtomicU64,
    empty: AtomicU64,
    delay: AtomicI64,
    clear_requested: AtomicU64,
    cleared: AtomicU64,
    draining: AtomicBool,
    in_flight: AtomicBool,
    mixer_on: AtomicBool,
    mixer_volume_bits: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn new(wake: Wake, volume: f64) -> Self {
        Self {
            wake,
            stopped: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            played: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            silent: AtomicU64::new(0),
            empty: AtomicU64::new(0),
            delay: AtomicI64::new(0),
            clear_requested: AtomicU64::new(0),
            cleared: AtomicU64::new(0),
            draining: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            mixer_on: AtomicBool::new(false),
            mixer_volume_bits: AtomicU64::new(volume.to_bits()),
            error: Mutex::new(None),
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Relaxed);
        self.wake.signal();
    }
}

/// The mixer's switch and level, shared with the stream. `Clone` to hand to the pipeline
/// stage that turns the thinking sound on and off.
#[derive(Clone)]
pub struct MixerHandle {
    shared: Arc<Shared>,
}

impl MixerHandle {
    pub fn set_enabled(&self, on: bool) {
        self.shared.mixer_on.store(on, Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.shared.mixer_on.load(Relaxed)
    }

    pub fn set_volume(&self, volume: f64) {
        self.shared
            .mixer_volume_bits
            .store(volume.to_bits(), Relaxed);
    }

    pub fn volume(&self) -> f64 {
        f64::from_bits(self.shared.mixer_volume_bits.load(Relaxed))
    }
}

impl std::fmt::Debug for MixerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixerHandle")
            .field("enabled", &self.enabled())
            .field("volume", &self.volume())
            .finish()
    }
}

/// A mark the daemon leaves on the queue: when the sample at `total_after` has been
/// handed to PipeWire, the first sample written at `at` has left the ring.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    total_after: u64,
    at: Instant,
}

struct PlaybackData {
    consumer: Consumer<i16>,
    stamps: Consumer<Stamp>,
    latencies: Producer<Duration>,
    mixer: Option<Mixer>,
    shared: Arc<Shared>,
    frame: usize,
}

/// The realtime side. No allocation, no lock, no log line.
fn process(stream: &PwStream, data: &mut PlaybackData) {
    let shared = &*data.shared;
    shared.callbacks.fetch_add(1, Relaxed);
    let Some(mut buffer) = stream.dequeue_buffer() else {
        shared.empty.fetch_add(1, Relaxed);
        return;
    };
    // Barge-in: drop what is queued, now.
    let requested_clear = shared.clear_requested.load(Relaxed);
    if requested_clear != shared.cleared.load(Relaxed) {
        let dropped = ring::drop_all(&mut data.consumer);
        ring::drop_all(&mut data.stamps);
        shared.played.fetch_add(dropped as u64, Relaxed);
        shared.in_flight.store(false, Relaxed);
        shared.cleared.store(requested_clear, Release); // after the ring's own release
    }
    let requested = buffer.requested() as usize;
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else {
        return;
    };
    let Some(bytes) = d.data() else {
        return;
    };
    let max = bytes.len() / 2;
    let n = if requested > 0 {
        requested.min(max)
    } else {
        data.frame.min(max)
    };
    let out = &mut bytes[..n * 2];
    let got = ring::pop_into_le_bytes(&mut data.consumer, out);
    if got < n {
        out[got * 2..].fill(0);
        let was_in_flight = shared.in_flight.swap(false, Relaxed);
        if was_in_flight && !shared.draining.load(Relaxed) {
            shared.underruns.fetch_add(1, Relaxed);
        }
        if got == 0 {
            shared.silent.fetch_add(1, Relaxed);
        }
    }
    let played = shared.played.fetch_add(got as u64, Release) + got as u64;
    if shared.mixer_on.load(Relaxed)
        && let Some(mixer) = data.mixer.as_mut()
    {
        let volume = f64::from_bits(shared.mixer_volume_bits.load(Relaxed));
        mixer.mix_into_le_bytes(out, volume);
    }
    let now = Instant::now();
    while let Ok(stamp) = data.stamps.peek().copied() {
        if stamp.total_after > played {
            break;
        }
        let _ = data.stamps.pop();
        let _ = data.latencies.push(now.saturating_duration_since(stamp.at));
    }
    let chunk = d.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = 2;
    *chunk.size_mut() = (n * 2) as u32;
    if let Ok(t) = stream.time() {
        shared.delay.store(t.delay(), Relaxed);
    }
    shared.wake.signal();
}

fn run(
    config: PlaybackConfig,
    data: PlaybackData,
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
    let (target, sink) = match &config.sink {
        Some(name) => match session.find_node(name) {
            Ok(Some(node)) if node.media_class() == "Audio/Sink" => {
                (Some(node.serial_or_id().to_string()), name.clone())
            }
            Ok(_) => {
                let _ = ready.send(Err(AudioError::NoSuchSink { name: name.clone() }));
                return;
            }
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                return;
            }
        },
        None => (None, "default sink".to_string()),
    };
    let latency = format!("{}/{}", frame_samples(config.rate), config.rate);
    let rate = format!("1/{}", config.rate);
    let mut props = properties! {
        *keys::MEDIA_TYPE => "Audio",
        *keys::MEDIA_CATEGORY => "Playback",
        *keys::MEDIA_ROLE => "Production",
        *keys::APP_NAME => "ghost",
        *keys::NODE_NAME => config.name.as_str(),
        *keys::NODE_DESCRIPTION => "Ghost (speaking)",
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
    let listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(|_stream, data, _old, new| match new {
            StreamState::Error(e) => {
                warn!("audio out: stream error: {e}");
                *data.shared.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
            }
            StreamState::Streaming => info!("audio out: streaming"),
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
        Direction::Output,
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
    info!("audio out: {sink} @ {}", config.rate);
    let _ = ready.send(Ok(sink));
    mainloop.run();
    let _ = stream.disconnect();
    drop(listener);
    shared.stop();
}

/// An open playback stream. Dropping it stops the stream and joins PipeWire's thread.
pub struct Playback {
    shared: Arc<Shared>,
    producer: Producer<i16>,
    waiter: Waiter,
    stamps: Producer<Stamp>,
    latencies: Consumer<Duration>,
    written: u64,
    quit: pipewire::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    frame: usize,
    rate: u32,
    ring_seconds: f32,
    sink: String,
}

impl Playback {
    /// Open the stream; returns once it is connected, or with the error that stopped it.
    pub fn open(config: PlaybackConfig) -> Result<Playback, AudioError> {
        let (producer, consumer) = ring::ring(config.rate, config.ring_seconds);
        let (stamps_tx, stamps_rx) = RingBuffer::<Stamp>::new(512);
        let (latencies_tx, latencies_rx) = RingBuffer::<Duration>::new(4096);
        let (wake, waiter) =
            Wake::pair().map_err(|e| AudioError::Stream(format!("cannot make an eventfd: {e}")))?;
        let shared = Arc::new(Shared::new(wake, config.sound_volume));
        let frame = frame_samples(config.rate);
        let data = PlaybackData {
            consumer,
            stamps: stamps_rx,
            latencies: latencies_tx,
            mixer: config.sound.clone().map(Mixer::new),
            shared: shared.clone(),
            frame,
        };
        let (ready_tx, ready_rx) = mpsc::channel();
        let (quit_tx, quit_rx) = pipewire::channel::channel();
        let rate = config.rate;
        let ring_seconds = config.ring_seconds;
        let thread = std::thread::Builder::new()
            .name("ghost-playback".into())
            .spawn({
                let shared = shared.clone();
                move || run(config, data, shared, ready_tx, quit_rx)
            })
            .map_err(|e| AudioError::Stream(format!("cannot start the playback thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(sink)) => Ok(Playback {
                shared,
                producer,
                waiter,
                stamps: stamps_tx,
                latencies: latencies_rx,
                written: 0,
                quit: quit_tx,
                thread: Some(thread),
                frame,
                rate,
                ring_seconds,
                sink,
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

    /// The sink being played to: the configured name, or "default sink".
    pub fn sink(&self) -> &str {
        &self.sink
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Samples per 20 ms frame (480 at 24 kHz).
    pub fn frame_samples(&self) -> usize {
        self.frame
    }

    /// The thinking sound's switch and level.
    pub fn mixer(&self) -> MixerHandle {
        MixerHandle {
            shared: self.shared.clone(),
        }
    }

    pub fn counters(&self) -> PlaybackCounters {
        PlaybackCounters {
            callbacks: self.shared.callbacks.load(Relaxed),
            played: self.shared.played.load(Relaxed),
            underruns: self.shared.underruns.load(Relaxed),
            silent_cycles: self.shared.silent.load(Relaxed),
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

    /// Samples written so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Samples queued and not yet handed to PipeWire.
    pub fn queued(&self) -> usize {
        self.producer.buffer().capacity() - self.producer.slots()
    }

    /// Write what fits now; returns how many samples were taken.
    pub fn try_write(&mut self, samples: &[i16]) -> usize {
        let (written, _) = ring::push_samples(&mut self.producer, samples);
        if written > 0 {
            self.written += written as u64;
            self.shared.in_flight.store(true, Relaxed);
        }
        written
    }

    /// Write all of `samples`, waiting for room as the stream plays.
    pub async fn write(&mut self, mut samples: &[i16]) -> Result<(), AudioError> {
        while !samples.is_empty() {
            let n = self.try_write(samples);
            samples = &samples[n..];
            if samples.is_empty() {
                break;
            }
            if self.shared.stopped.load(Relaxed) {
                return Err(AudioError::Closed);
            }
            self.waiter.wait().await.map_err(|_| AudioError::Closed)?;
        }
        Ok(())
    }

    /// `write`, and note when these samples were produced, so [`take_latencies`] can say
    /// how long the first of them waited before PipeWire took it (the loopback
    /// measurement; the rest follow at the device's rate).
    ///
    /// [`take_latencies`]: Playback::take_latencies
    pub async fn write_stamped(&mut self, samples: &[i16], at: Instant) -> Result<(), AudioError> {
        if samples.is_empty() {
            return Ok(());
        }
        let _ = self.stamps.push(Stamp {
            total_after: self.written + 1,
            at,
        });
        self.write(samples).await
    }

    /// Barge-in: drop what is queued. Resolves once the stream's callback has done it,
    /// within one quantum, or after 250 ms when the stream is not running.
    pub async fn clear(&mut self) {
        let generation = self.shared.clear_requested.fetch_add(1, Relaxed) + 1;
        let deadline = tokio::time::sleep(Duration::from_millis(250));
        tokio::pin!(deadline);
        loop {
            if self.shared.cleared.load(Acquire) >= generation || self.shared.stopped.load(Relaxed)
            {
                return;
            }
            tokio::select! {
                woken = self.waiter.wait() => {
                    if woken.is_err() {
                        return;
                    }
                }
                _ = &mut deadline => return,
            }
        }
    }

    /// Resolves when everything written so far has been handed to PipeWire and the
    /// device's reported delay has passed. Gives up after the ring's length plus a second
    /// when the stream is not running.
    pub async fn drain(&mut self) -> Result<(), AudioError> {
        let target = self.written;
        self.shared.draining.store(true, Relaxed);
        let limit = Duration::from_secs_f32(self.ring_seconds + 1.0);
        let shared = self.shared.clone();
        let waiter = &mut self.waiter;
        let handed_over = tokio::time::timeout(limit, async {
            loop {
                if shared.played.load(Acquire) >= target || shared.stopped.load(Relaxed) {
                    return;
                }
                if waiter.wait().await.is_err() {
                    return;
                }
            }
        })
        .await;
        if handed_over.is_ok() && !self.shared.stopped.load(Relaxed) {
            let delay = self.shared.delay.load(Relaxed).max(0) as f64 / self.rate as f64;
            tokio::time::sleep(
                Duration::from_secs_f64(delay) + Duration::from_millis(u64::from(FRAME_MS)),
            )
            .await;
        }
        self.shared.draining.store(false, Relaxed);
        self.shared.in_flight.store(false, Relaxed);
        if self.shared.stopped.load(Relaxed) {
            Err(AudioError::Closed)
        } else {
            Ok(())
        }
    }

    /// How long each stamped write waited in the ring before the callback took its first
    /// sample, one entry per [`write_stamped`] call that has started playing.
    ///
    /// [`write_stamped`]: Playback::write_stamped
    pub fn take_latencies(&mut self, out: &mut Vec<Duration>) {
        while let Ok(d) = self.latencies.pop() {
            out.push(d);
        }
    }

    /// Stop the stream and join its thread. What is queued is dropped.
    pub fn close(self) {
        drop(self);
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
