//! How loud Ghost's own voice is at the speaker, for the desktop's waveform.
//!
//! The output stage hands every chunk it has written here with how far ahead of the
//! speaker it was written (the device's queue). The meter folds the samples into 50 ms
//! windows, stamps each with the moment it starts to play, and a small task hands the
//! windows out at those moments: about twenty a second, in step with what the operator
//! hears, even though the output stage writes up to the device's queue ahead (half a
//! second at the desk, two at the phone). When the voice stops (played out, cut by a
//! barge-in, the session gone, or the queue ran dry) one `0.0` follows the last window,
//! and nothing more until Ghost speaks again.
//!
//! Only Ghost's output passes through here. The microphone never does, and nothing is
//! sent while Ghost is silent (CLAUDE.md: the control API never carries passive audio).
//!
//! The cost is a multiply-add per sample and one channel send per window; the callback
//! must not block (the daemon's is a broadcast send that drops for a slow client).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::frames::Spoken;

/// Called with each level, 0.0 to 1.0, from the meter's task. Must not block.
pub type OnLevel = Arc<dyn Fn(f32) + Send + Sync>;

/// Called with the words of the reply being heard (`Spoken`) each time they change, from
/// the meter's task, in step with the levels (ADR 0058). Must not block.
pub type OnSaid = Arc<dyn Fn(Spoken) + Send + Sync>;

/// One level per this much audio.
pub const WINDOW: Duration = Duration::from_millis(50);

/// The quietest level shown: this many dBFS and below is 0.0, 0 dBFS is 1.0.
pub const FLOOR_DBFS: f32 = -50.0;

/// With nothing new this long after the last window finished playing, the voice stopped
/// (the device ran dry between clauses): one `0.0`. Longer than the write jitter between
/// two windows, short enough that the waveform does not hang on a stale level.
const GRACE: Duration = Duration::from_millis(100);

/// dBFS to the perceptual level: linear from [`FLOOR_DBFS`] (0.0) to 0 dBFS (1.0), clamped.
pub fn level_from_dbfs(dbfs: f32) -> f32 {
    if dbfs.is_nan() {
        return 0.0;
    }
    ((dbfs - FLOOR_DBFS) / -FLOOR_DBFS).clamp(0.0, 1.0)
}

/// The level of a window from its sum of squared samples (full scale 1.0) and its length.
pub fn level_from_power(sum_sq: f64, n: usize) -> f32 {
    if n == 0 || sum_sq <= 0.0 {
        return 0.0;
    }
    let ms = sum_sq / n as f64;
    level_from_dbfs((10.0 * ms.log10()) as f32)
}

/// The level of `pcm` taken as one window.
pub fn level_of(pcm: &[i16]) -> f32 {
    level_from_power(power(pcm), pcm.len())
}

fn power(pcm: &[i16]) -> f64 {
    pcm.iter()
        .map(|s| {
            let v = f64::from(*s) / 32768.0;
            v * v
        })
        .sum()
}

enum Cmd {
    /// A window's level, when it starts to play, and the words it says.
    Window(Instant, f32, Option<Spoken>),
    /// The voice stopped now: drop what is pending, one `0.0` if anything was sent.
    Silence,
}

/// The output stage's side: fold written audio into windows, say when the voice stopped.
pub struct VoiceMeter {
    on: OnLevel,
    on_said: Option<OnSaid>,
    /// The words of the audio being written now.
    spoken: Option<Spoken>,
    tx: Option<mpsc::UnboundedSender<Cmd>>,
    rate: u32,
    sum_sq: f64,
    n: usize,
}

impl VoiceMeter {
    pub fn new(on: OnLevel) -> Self {
        Self {
            on,
            on_said: None,
            spoken: None,
            tx: None,
            rate: 0,
            sum_sq: 0.0,
            n: 0,
        }
    }

    /// Also say which words of the reply are heard, as they are (ADR 0058).
    pub fn with_said(mut self, on: OnSaid) -> Self {
        self.on_said = Some(on);
        self
    }

    /// The audio written next says these words (None: not a reply's, or not known).
    pub fn saying(&mut self, spoken: Option<Spoken>) {
        self.spoken = spoken;
    }

    /// `pcm` (at `rate`) was just written; its last sample is heard `lead` from now.
    pub fn written(&mut self, pcm: &[i16], rate: u32, lead: Duration) {
        if pcm.is_empty() || rate == 0 {
            return;
        }
        if rate != self.rate {
            self.rate = rate;
            self.sum_sq = 0.0;
            self.n = 0;
        }
        let window = (u64::from(rate) * WINDOW.as_millis() as u64 / 1000) as usize;
        let end = Instant::now() + lead; // when pcm's last sample plays
        let mut i = 0;
        while i < pcm.len() {
            let take = (window - self.n).min(pcm.len() - i);
            self.sum_sq += power(&pcm[i..i + take]);
            self.n += take;
            i += take;
            if self.n == window {
                let level = level_from_power(self.sum_sq, self.n);
                self.sum_sq = 0.0;
                self.n = 0;
                // this window's last sample is pcm[i - 1]; it starts a window earlier
                let after = (pcm.len() - i) as u64 + window as u64;
                let back = Duration::from_micros(after * 1_000_000 / u64::from(rate));
                let spoken = self.spoken;
                self.send(Cmd::Window(
                    end.checked_sub(back).unwrap_or(end),
                    level,
                    spoken,
                ));
            }
        }
    }

    /// Ghost's voice stopped: played out, or cut.
    pub fn silence(&mut self) {
        self.sum_sq = 0.0;
        self.n = 0;
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::Silence);
        }
    }

    fn send(&mut self, cmd: Cmd) {
        let tx = self.tx.get_or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(pace(rx, self.on.clone(), self.on_said.clone()));
            tx
        });
        let _ = tx.send(cmd);
    }
}

/// Hand each window out when it starts to play; a `0.0` when the voice stops. Ends, with a
/// last `0.0` if one is owed, when the meter is dropped.
async fn pace(mut rx: mpsc::UnboundedReceiver<Cmd>, on: OnLevel, on_said: Option<OnSaid>) {
    let mut pending: VecDeque<(Instant, f32, Option<Spoken>)> = VecDeque::new();
    let mut said: Option<Spoken> = None; // the words last reported
    let mut sent = false; // a level above the last 0.0 went out
    let mut heard_until = Instant::now(); // when the last window sent finishes playing
    loop {
        let deadline = match pending.front() {
            Some((at, _, _)) => Some(*at),
            None if sent => Some(heard_until + GRACE),
            None => None,
        };
        tokio::select! {
            biased;
            cmd = rx.recv() => match cmd {
                Some(Cmd::Window(at, level, spoken)) => pending.push_back((at, level, spoken)),
                Some(Cmd::Silence) => {
                    pending.clear();
                    if sent {
                        on(0.0);
                        sent = false;
                    }
                }
                None => {
                    if sent {
                        on(0.0);
                    }
                    return;
                }
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                match pending.pop_front() {
                    Some((at, level, spoken)) => {
                        if let (Some(f), Some(s)) = (&on_said, spoken)
                            && said != Some(s)
                        {
                            said = Some(s);
                            f(s);
                        }
                        on(level);
                        sent = true;
                        heard_until = at + WINDOW;
                    }
                    None => {
                        on(0.0);
                        sent = false;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const RATE: u32 = 24_000;

    fn sine(amplitude: f64, secs: f64) -> Vec<i16> {
        let n = (f64::from(RATE) * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                (amplitude * 32767.0 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()) as i16
            })
            .collect()
    }

    type Heard = Arc<Mutex<Vec<(Instant, f32)>>>;

    fn recorder() -> (OnLevel, Heard) {
        let heard: Heard = Arc::default();
        let h = heard.clone();
        (
            Arc::new(move |l| h.lock().unwrap().push((Instant::now(), l))),
            heard,
        )
    }

    #[test]
    fn the_mapping_is_linear_in_dbfs_from_minus_50_to_0() {
        assert_eq!(level_from_dbfs(0.0), 1.0);
        assert_eq!(level_from_dbfs(-50.0), 0.0);
        assert!((level_from_dbfs(-25.0) - 0.5).abs() < 1e-6);
        assert!((level_from_dbfs(-10.0) - 0.8).abs() < 1e-6);
        assert_eq!(level_from_dbfs(-80.0), 0.0, "below the floor is clamped");
        assert_eq!(level_from_dbfs(6.0), 1.0, "above full scale is clamped");
        assert_eq!(level_from_dbfs(f32::NEG_INFINITY), 0.0);
        assert_eq!(level_from_dbfs(f32::NAN), 0.0);
    }

    #[test]
    fn silence_is_zero_and_a_full_scale_sine_is_near_one() {
        assert_eq!(level_of(&[0; 1200]), 0.0);
        assert_eq!(level_of(&[]), 0.0);
        // a full-scale sine's RMS is -3 dBFS
        let full = level_of(&sine(1.0, 0.05));
        assert!((full - (1.0 - 3.01 / 50.0)).abs() < 0.01, "{full}");
        // a tenth of full scale is 20 dB down
        let tenth = level_of(&sine(0.1, 0.05));
        assert!((full - tenth - 0.4).abs() < 0.01, "{tenth}");
    }

    #[tokio::test(start_paused = true)]
    async fn windows_go_out_every_50_ms_as_they_play_then_one_zero() {
        let (on, heard) = recorder();
        let mut meter = VoiceMeter::new(on);
        let t0 = Instant::now();
        // 200 ms written at once, the device a further 300 ms behind: heard from 300 ms
        meter.written(&sine(0.5, 0.2), RATE, Duration::from_millis(500));
        tokio::time::sleep(Duration::from_secs(2)).await;
        let heard = heard.lock().unwrap().clone();
        assert_eq!(heard.len(), 5, "{heard:?}");
        for (k, (at, level)) in heard[..4].iter().enumerate() {
            let want = t0 + Duration::from_millis(300 + 50 * k as u64);
            assert_eq!(*at, want, "window {k}");
            assert!((level - level_from_dbfs(-9.03)).abs() < 0.01, "{level}");
        }
        let (at, zero) = heard[4];
        assert_eq!(zero, 0.0);
        assert_eq!(
            at,
            t0 + Duration::from_millis(500) + GRACE,
            "once it played out"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn windows_span_chunks_and_a_cut_drops_what_has_not_played() {
        let (on, heard) = recorder();
        let mut meter = VoiceMeter::new(on);
        let t0 = Instant::now();
        // a second of 20 ms chunks written into an empty queue at once: chunk k is heard
        // from 20k ms, so each one's lead is 20 ms longer than the last's
        let chunk = sine(0.5, 0.02);
        for k in 0..50 {
            meter.written(&chunk, RATE, Duration::from_millis(20 * (k + 1)));
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
        meter.silence(); // barge-in at 120 ms: the rest never plays
        tokio::time::sleep(Duration::from_secs(2)).await;
        let heard = heard.lock().unwrap().clone();
        let at: Vec<Duration> = heard.iter().map(|(t, _)| t.duration_since(t0)).collect();
        let ms = Duration::from_millis;
        assert_eq!(at, [ms(0), ms(50), ms(100), ms(120)], "{heard:?}");
        assert!(heard[..3].iter().all(|(_, l)| *l > 0.5));
        assert_eq!(heard[3].1, 0.0, "the 0 is sent at the cut");
    }

    #[tokio::test(start_paused = true)]
    async fn the_words_heard_are_said_when_they_play_once_each() {
        // Two clauses written at once, the device 300 ms behind: the second clause's words
        // are reported when its first window plays, not when it was written (ADR 0058).
        let (on, _) = recorder();
        let said: Arc<Mutex<Vec<(Instant, Spoken)>>> = Arc::default();
        let s = said.clone();
        let mut meter = VoiceMeter::new(on).with_said(Arc::new(move |w| {
            s.lock().unwrap().push((Instant::now(), w))
        }));
        let t0 = Instant::now();
        let first = Spoken {
            reply: 0,
            from: 0,
            to: 3,
        };
        let second = Spoken {
            reply: 0,
            from: 3,
            to: 9,
        };
        meter.saying(Some(first));
        meter.written(&sine(0.5, 0.1), RATE, Duration::from_millis(400));
        meter.saying(Some(second));
        meter.written(&sine(0.5, 0.1), RATE, Duration::from_millis(500));
        tokio::time::sleep(Duration::from_secs(2)).await;
        let said = said.lock().unwrap().clone();
        let ms = Duration::from_millis;
        assert_eq!(
            said.iter()
                .map(|(t, w)| (t.duration_since(t0), *w))
                .collect::<Vec<_>>(),
            vec![(ms(300), first), (ms(400), second)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_sent_while_ghost_is_silent() {
        let (on, heard) = recorder();
        let mut meter = VoiceMeter::new(on);
        meter.silence(); // a barge-in with nothing playing
        meter.written(&[0; 600], RATE, Duration::ZERO); // half a window: not a level yet
        meter.silence();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(heard.lock().unwrap().is_empty());
        // one window, then silence twice: one 0 only
        meter.written(&sine(0.5, 0.05), RATE, Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(10)).await;
        meter.silence();
        meter.silence();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let levels: Vec<f32> = heard.lock().unwrap().iter().map(|(_, l)| *l).collect();
        assert_eq!(levels.len(), 2, "{levels:?}");
        assert_eq!(levels[1], 0.0);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_meter_mid_word_sends_the_last_zero() {
        let (on, heard) = recorder();
        let mut meter = VoiceMeter::new(on);
        meter.written(&sine(0.5, 0.1), RATE, Duration::from_millis(100));
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(meter); // the session hung up
        tokio::time::sleep(Duration::from_secs(1)).await;
        let levels: Vec<f32> = heard.lock().unwrap().iter().map(|(_, l)| *l).collect();
        assert_eq!(levels.len(), 2, "{levels:?}");
        assert_eq!(levels[1], 0.0);
    }

    #[test]
    fn the_envelope_costs_next_to_nothing() {
        // One second of speech, metered in the 20 ms chunks the output stage writes.
        let speech = sine(0.5, 1.0);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = rt.enter();
        let (on, _) = recorder();
        let mut meter = VoiceMeter::new(on);
        let runs = 50;
        let began = std::time::Instant::now();
        for _ in 0..runs {
            for chunk in speech.chunks(480) {
                meter.written(std::hint::black_box(chunk), RATE, Duration::ZERO);
            }
        }
        let per_second = began.elapsed() / runs;
        eprintln!("voice level: {per_second:?} per second of speech");
        assert!(per_second < Duration::from_millis(5), "{per_second:?}");
    }
}
