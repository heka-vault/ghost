//! The audio barrier through the daemon's own passive loop (ADR 0077): a room the test
//! speaks into, a wake that hears it only while the loop has it waiting (as the real
//! listener does: a fresh pipeline and capture each wait, closed at `stop`), the kept takes'
//! real array tape fed by a raw capture that never pauses (the worst case: a late frame),
//! and dictation's own capture on the same room.

use std::sync::atomic::{AtomicBool, AtomicI16, AtomicUsize, Ordering};

use ghost_core::wake_takes::{NewTake, Takes};
use ghost_voice::pipeline::Models;
use ghost_voice::stt::Recognizer;
use ghost_voice::wakeword::TakeKeeper;

use ghost_voice::frames::Audio;

use super::*;
use crate::daemon::tests::Recorder;
use crate::daemon::{Daemon, SessionStart};
use crate::wake::WakeDetector;
use crate::wake_takes::{ArrayTape, Keeper, TapedWake};

const BEFORE: i16 = 100;
const DICTATED: i16 = 900;

/// What is being said in the room now. A clock room numbers every read: each frame anyone
/// captures has its own value, in the order they were heard.
struct Room {
    value: AtomicI16,
    clock: bool,
}

impl Room {
    fn fixed(v: i16) -> Self {
        Self {
            value: AtomicI16::new(v),
            clock: false,
        }
    }
    fn hear(&self) -> i16 {
        if self.clock {
            self.value.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.value.load(Ordering::SeqCst)
        }
    }
}

/// Hears the room only while it is waiting; a hit keeps a take of what it heard this wait.
struct RoomWake {
    room: Arc<Room>,
    keeper: Arc<Keeper>,
    waiting: AtomicBool,
    heard: Mutex<Vec<i16>>,
    /// Everything it ever heard, across waits.
    ever: Mutex<Vec<i16>>,
    hit: Notify,
    waits: AtomicUsize,
}

#[async_trait]
impl WakeDetector for RoomWake {
    async fn wait(&self) -> Option<String> {
        self.waits.fetch_add(1, Ordering::SeqCst);
        lock(&self.heard).clear(); // a fresh pipeline each wait
        self.waiting.store(true, Ordering::SeqCst);
        loop {
            tokio::select! {
                _ = self.hit.notified() => break,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    let v = self.room.hear();
                    lock(&self.heard).extend(std::iter::repeat_n(v, 160));
                    lock(&self.ever).extend(std::iter::repeat_n(v, 160));
                }
            }
        }
        self.waiting.store(false, Ordering::SeqCst);
        // A take reaches 3 s back, as a real one reaches 12 s before an accepted hit; the
        // detector's own window is fresh, so what it did not hear this wait is silence.
        let heard = lock(&self.heard).clone();
        let mut audio = vec![0i16; (3 * RATE as usize).saturating_sub(heard.len())];
        audio.extend(heard);
        self.keeper.keep(
            NewTake {
                hit: audio.len(),
                audio,
                accepted: true,
                model: "hey_ghost".into(),
                score: 0.9,
                threshold: 0.6,
                ..NewTake::default()
            },
            0.0,
        );
        Some("what's next".into())
    }
    async fn stop(&self) {
        self.waiting.store(false, Ordering::SeqCst);
    }
    fn close(&self) {}
}

/// Dictation's capture on the same room.
struct RoomMic(Arc<Room>);

#[async_trait]
impl MediaIn for RoomMic {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Some(Audio::new(vec![self.0.hear(); 160], RATE))
    }
}

/// Reads the marked sentence when it hears dictated audio.
struct Reads;

impl Recognizer for Reads {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        // Anything louder than the dither is a room speaking.
        let heard = audio.iter().any(|&x| x.abs() > 0.01);
        Ok(if heard { "Pick up milk." } else { "" }.into())
    }
}

struct Complete;

impl ghost_voice::turns::TurnModel for Complete {
    fn predict(&mut self, _audio: &[f32]) -> ghost_infer::Result<ghost_infer::TurnVerdict> {
        Ok(ghost_infer::TurnVerdict {
            probability: 0.99,
            complete: true,
        })
    }
}

#[derive(Default)]
struct Board(Mutex<Vec<String>>);

impl Clipboard for Board {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        lock(&self.0).push(text.to_string());
        Ok(())
    }
}

async fn until(what: impl Fn() -> bool) {
    for _ in 0..600 {
        if what() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened");
}

struct World {
    d: Arc<Daemon>,
    room: Arc<Room>,
    wake: Arc<RoomWake>,
    takes: Arc<Takes>,
    board: Arc<Board>,
    recorder: Arc<Recorder>,
    _tmp: tempfile::TempDir,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let room = Arc::new(Room::fixed(BEFORE));
    let board = Arc::new(Board::default());
    let recorder = Arc::new(Recorder(Mutex::new(Vec::new())));
    let mut d = Arc::try_unwrap(crate::daemon::tests::daemon(tmp.path()))
        .ok()
        .expect("one reference");
    d.ports.sessions = recorder.clone();
    d.ports.clipboard = board.clone();
    d.ports.dictation_media = Some({
        let room = room.clone();
        Arc::new(move || Ok(Box::new(RoomMic(room.clone())) as Box<dyn MediaIn>))
    });
    let d = Arc::new(d);
    let _ = d.late.models.set(Arc::new(Models {
        stt: Arc::new(Mutex::new(Reads)),
        tts: Arc::new(Mutex::new(ghost_voice::tts::NoSynth)),
        turn: Arc::new(Mutex::new(Complete)),
        silero: tmp.path().join("no-silero.onnx"),
        voices: vec![],
        acks: ghost_voice::acks::Acks::new(None),
        problem: None,
    }));
    let takes = Arc::new(Takes::new(&tmp.path().join("data")));
    // A node that is not there: the tape's own capture fails to open, and the test feeds it.
    let tape = ArrayTape::new("ghost-test-no-such-array".into(), None);
    let wake = Arc::new(RoomWake {
        room: room.clone(),
        keeper: Arc::new(Keeper::new(
            takes.clone(),
            "ghost.mic".into(),
            Some(tape.clone()),
        )),
        waiting: AtomicBool::new(false),
        heard: Mutex::new(Vec::new()),
        ever: Mutex::new(Vec::new()),
        hit: Notify::new(),
        waits: AtomicUsize::new(0),
    });
    World {
        d,
        room,
        wake,
        takes,
        board,
        recorder,
        _tmp: tmp,
    }
}

/// A room whose every read is numbered: no two captured frames share a value, so the first
/// and the last of dictation's are its own too.
fn clock_room() -> Arc<Room> {
    Arc::new(Room {
        value: AtomicI16::new(1000),
        clock: true,
    })
}

/// The array's raw capture on the clock room: slow to open and slow to close (a real one
/// joins its thread when dropped), counted while open.
struct TapeMic {
    room: Arc<Room>,
    /// Dictation's capture is open; a tape capture alive meanwhile is a clash.
    dictating: Arc<AtomicBool>,
    clash: Arc<AtomicBool>,
}

#[async_trait]
impl MediaIn for TapeMic {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Some(Audio::new(vec![self.room.hear(); 160], RATE))
    }
}

impl Drop for TapeMic {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(30));
        if self.dictating.load(Ordering::SeqCst) {
            self.clash.store(true, Ordering::SeqCst); // it was open beside dictation's
        }
    }
}

/// Dictation's capture on the clock room: what it heard, and whether anything else was
/// capturing when it opened.
struct WindowMic {
    room: Arc<Room>,
    seen: Arc<Mutex<Vec<i16>>>,
    dictating: Arc<AtomicBool>,
}

impl Drop for WindowMic {
    fn drop(&mut self) {
        self.dictating.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl MediaIn for WindowMic {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let v = self.room.hear();
        lock(&self.seen).push(v);
        Some(Audio::new(vec![v; 160], RATE))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictation_never_reaches_the_wake_or_a_kept_take() {
    let mut w = world();
    let room = clock_room();
    let captures = w.d.captures.clone();
    let dictating = Arc::new(AtomicBool::new(false));
    let clash = Arc::new(AtomicBool::new(false));
    let tape = ArrayTape::with_opener(
        "ghost-test-array".into(),
        {
            let room = room.clone();
            let (dictating, clash) = (dictating.clone(), clash.clone());
            Arc::new(move |_node: &str| {
                std::thread::sleep(Duration::from_millis(100)); // a slow open
                if dictating.load(Ordering::SeqCst) {
                    clash.store(true, Ordering::SeqCst); // opened beside dictation's
                }
                Ok(Box::new(TapeMic {
                    room: room.clone(),
                    dictating: dictating.clone(),
                    clash: clash.clone(),
                }) as Box<dyn MediaIn>)
            })
        },
        Some(captures.clone()),
    );
    // The wake and dictation hear the clock room too.
    let wake = Arc::new(RoomWake {
        room: room.clone(),
        keeper: Arc::new(Keeper::new(
            w.takes.clone(),
            "ghost.mic".into(),
            Some(tape.clone()),
        )),
        waiting: AtomicBool::new(false),
        heard: Mutex::new(Vec::new()),
        ever: Mutex::new(Vec::new()),
        hit: Notify::new(),
        waits: AtomicUsize::new(0),
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let d = Arc::get_mut(&mut w.d).expect("one reference");
        let (room, seen, clash) = (room.clone(), seen.clone(), clash.clone());
        let dictating = dictating.clone();
        let (captures, wake) = (captures.clone(), wake.clone());
        d.ports.dictation_media = Some(Arc::new(move || {
            // The barrier, observed: when dictation's capture opens, the only desk capture
            // counted is its own (its token is taken just before this).
            if captures.alive() != 1 || wake.waiting.load(Ordering::SeqCst) {
                clash.store(true, Ordering::SeqCst);
            }
            dictating.store(true, Ordering::SeqCst);
            Ok(Box::new(WindowMic {
                room: room.clone(),
                seen: seen.clone(),
                dictating: dictating.clone(),
            }) as Box<dyn MediaIn>)
        }));
    }
    let detector: Arc<dyn WakeDetector> = Arc::new(TapedWake::new(wake.clone(), tape.clone()));
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    // Dictate as soon as the wake listens: the tape's open is likely still in flight.
    until(|| wake.waiting.load(Ordering::SeqCst)).await;
    let v = w.d.dictation().start().await;
    assert_eq!(v.state, DictationState::Recording, "{v:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Stopped while the room is still going: the operator talks straight on.
    let v = w.d.dictation().stop().await;
    let r = v.result.unwrap();
    assert_eq!(r.outcome, DictationOutcome::OnClipboard, "{r:?}");
    let window = {
        let seen = lock(&seen);
        (*seen.iter().min().unwrap(), *seen.iter().max().unwrap())
    };
    assert!(window.1 - window.0 > 20, "the dictation heard {window:?}");
    // At once, the wake listens again (the tape too) and hits.
    until(|| {
        wake.waits.load(Ordering::SeqCst) >= 2
            && wake.waiting.load(Ordering::SeqCst)
            && tape.slice(0.01, 0.0).is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    wake.hit.notify_one();
    until(|| !lock(&w.recorder.0).is_empty()).await;
    until(|| !w.takes.list().is_empty()).await;
    assert!(
        !clash.load(Ordering::SeqCst),
        "something captured beside dictation"
    );
    // Nothing heard inside the dictation's window, its first and last frames included, is
    // anywhere but its own read: every read of the room has its own number.
    let inside = |v: &i16| *v >= window.0 && *v <= window.1;
    let take = &w.takes.list()[0];
    let detector_audio = w.takes.audio(&take.id, false).unwrap();
    let array_audio = w.takes.audio(&take.id, true).unwrap();
    assert!(!array_audio.is_empty(), "the take has the array's stream");
    assert!(
        !detector_audio.iter().any(inside),
        "the detector's stream holds dictated audio"
    );
    assert!(
        !array_audio.iter().any(inside),
        "the array's tape holds dictated audio"
    );
    assert!(
        !lock(&wake.ever).iter().any(inside),
        "the wake heard dictated audio"
    );
    // The take reaches back 3 s: everything in it was heard after the dictation.
    assert!(
        array_audio.iter().all(|v| *v > window.1),
        "{:?}",
        &array_audio[..4]
    );
    w.d.stop.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), passive)
            .await
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_opening_or_listening_off_cancels_it_and_keeps_nothing() {
    let w = world();
    let manual = crate::wake::FileWakeDetector::new(w._tmp.path().join("rt/wake"));
    *lock(&w.d.manual) = Some(manual.clone());
    let detector: Arc<dyn WakeDetector> = w.wake.clone();
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    assert_eq!(
        w.d.dictation().start().await.state,
        DictationState::Recording
    );
    w.room.value.store(DICTATED, Ordering::SeqCst); // only once dictation records
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The room goes quiet before the cancel, so anything dictated the wake heard, it heard
    // while dictation recorded.
    w.room.value.store(BEFORE, Ordering::SeqCst);
    // Call (a typed turn or the desktop's button): the session opens, dictation is dropped.
    manual.trigger(Some("hello".into()));
    until(|| !lock(&w.recorder.0).is_empty()).await;
    let r = w.d.dictation().view().result.unwrap();
    assert_eq!(
        (r.outcome, r.reason),
        (
            DictationOutcome::Cancelled,
            Some(DictationReason::SessionOpened)
        )
    );
    assert!(lock(&w.board.0).is_empty());
    manual.arm(); // what the real wake's `arm` does at the top of each wait
    // Listening off: refused while off, and a dictation recording when it goes off is dropped.
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    let v = w.d.dictation().start().await;
    assert_eq!(v.state, DictationState::Recording, "{v:?}");
    w.room.value.store(DICTATED, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.room.value.store(BEFORE, Ordering::SeqCst);
    w.d.set_listening(false, None).await.unwrap();
    let r = w.d.dictation().view().result.unwrap();
    assert_eq!(
        (r.outcome, r.reason),
        (
            DictationOutcome::Cancelled,
            Some(DictationReason::ListeningTurnedOff)
        )
    );
    let r = w.d.dictation().start().await.result.unwrap();
    assert_eq!(r.reason, Some(DictationReason::NotListening));
    assert!(lock(&w.board.0).is_empty());
    assert!(!lock(&w.wake.ever).contains(&DICTATED));
    w.d.stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), passive).await;
}

#[tokio::test]
async fn refused_in_a_session_and_while_the_loop_is_not_waiting() {
    let w = world();
    // No passive loop running: nobody waits on the microphone.
    let r = w.d.dictation().start().await.result.unwrap();
    assert_eq!(r.reason, Some(DictationReason::Busy));
    let _ = SessionStart::new("ghost"); // the type a session starts from, for the reader
}

/// A gate a blocking thread waits on (an open or a close that stalls).
#[derive(Default)]
struct Gate(std::sync::Mutex<bool>, std::sync::Condvar);

impl Gate {
    fn wait(&self) {
        let mut open = self.0.lock().unwrap();
        while !*open {
            open = self.1.wait(open).unwrap();
        }
    }
    fn open(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }
}

/// Opens the gate when the test ends, however it ends, so no blocking thread outlives it.
struct OpenOnDrop(Arc<Gate>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// Dictation's capture: frames of the room; its close waits on `closing` when given.
struct GatedMic {
    room: Arc<Room>,
    closing: Option<Arc<Gate>>,
}

#[async_trait]
impl MediaIn for GatedMic {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Some(Audio::new(vec![self.room.hear(); 160], RATE))
    }
}

impl Drop for GatedMic {
    fn drop(&mut self) {
        if let Some(g) = &self.closing {
            g.wait(); // a capture whose thread will not join
        }
    }
}

fn with_dictation_media(w: &mut World, media: crate::wake::MediaFactory) {
    Arc::get_mut(&mut w.d)
        .expect("one reference")
        .ports
        .dictation_media = Some(media);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capture_that_will_not_close_keeps_the_wake_off_says_so_and_shutdown_still_ends() {
    let mut w = world();
    w.room.value.store(DICTATED, Ordering::SeqCst); // loud enough to be read
    let closing = Arc::new(Gate::default());
    let _release = OpenOnDrop(closing.clone());
    {
        let (room, closing) = (w.room.clone(), closing.clone());
        with_dictation_media(
            &mut w,
            Arc::new(move || {
                Ok(Box::new(GatedMic {
                    room: room.clone(),
                    closing: Some(closing.clone()),
                }) as Box<dyn MediaIn>)
            }),
        );
    }
    let detector: Arc<dyn WakeDetector> = w.wake.clone();
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    let waits = w.wake.waits.load(Ordering::SeqCst);
    assert_eq!(
        w.d.dictation().start().await.state,
        DictationState::Recording
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let r = w.d.dictation().stop().await.result.unwrap();
    assert_eq!(
        r.outcome,
        DictationOutcome::OnClipboard,
        "the read does not wait on the close"
    );
    // The capture does not close: counted, the wake stays off, the waiting list says so.
    assert_eq!(
        w.d.captures.alive_for(ghost_audio::live::Holder::Dictation),
        1
    );
    until(|| lock(&w.d.dictation_note).is_some()).await;
    assert_eq!(
        w.wake.waits.load(Ordering::SeqCst),
        waits,
        "the wake listened again"
    );
    assert!(!w.wake.waiting.load(Ordering::SeqCst));
    // Another dictation is refused at once, as busy.
    let began = Instant::now();
    let r = w.d.dictation().start().await.result.unwrap();
    assert_eq!(r.reason, Some(DictationReason::Busy));
    assert!(began.elapsed() < crate::daemon::GRANT_WAIT + Duration::from_secs(1));
    // Ghost stopping wins over the wait.
    w.d.stop.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), passive)
            .await
            .is_ok(),
        "shutdown blocked on the capture"
    );
    assert!(
        lock(&w.d.dictation_note).is_none(),
        "the note outlived the wait"
    );
    closing.open(); // let the stuck close finish
    assert!(
        w.d.captures
            .until(Duration::from_secs(2), |c| c.total() == 0)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_dropped_mid_open_settles_and_the_wake_waits_for_its_capture() {
    let mut w = world();
    let opening = Arc::new(Gate::default());
    let _release = OpenOnDrop(opening.clone());
    let entered = Arc::new(AtomicBool::new(false));
    {
        let (room, opening, entered) = (w.room.clone(), opening.clone(), entered.clone());
        with_dictation_media(
            &mut w,
            Arc::new(move || {
                entered.store(true, Ordering::SeqCst);
                opening.wait(); // a slow open
                Ok(Box::new(GatedMic {
                    room: room.clone(),
                    closing: None,
                }) as Box<dyn MediaIn>)
            }),
        );
    }
    // The wake records, at the start of each wait, whether dictation's capture was alive.
    let alive_at_wait = Arc::new(Mutex::new(Vec::new()));
    let detector: Arc<dyn WakeDetector> = Arc::new(Watching {
        inner: w.wake.clone(),
        captures: w.d.captures.clone(),
        seen: alive_at_wait.clone(),
    });
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    let waits = w.wake.waits.load(Ordering::SeqCst);
    let request = tokio::spawn({
        let d = w.d.dictation();
        async move { d.start().await }
    });
    until(|| entered.load(Ordering::SeqCst)).await;
    request.abort(); // the CLI killed, the HTTP client gone
    let _ = request.await;
    assert_eq!(
        w.d.dictation().state(),
        DictationState::Starting,
        "the start goes on"
    );
    // Cancelled while the open is still in flight: settled at once.
    w.d.dictation().cancel(DictationReason::Operator).await;
    assert_eq!(w.d.dictation().state(), DictationState::Idle);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        w.wake.waits.load(Ordering::SeqCst),
        waits,
        "the wake listened while dictation's open was in flight"
    );
    opening.open(); // the open finishes; what it opened is closed
    until(|| w.wake.waits.load(Ordering::SeqCst) > waits && w.wake.waiting.load(Ordering::SeqCst))
        .await;
    assert!(
        lock(&alive_at_wait).iter().all(|n| *n == 0),
        "the wake listened beside dictation's capture: {:?}",
        lock(&alive_at_wait)
    );
    // A later start works.
    let v = w.d.dictation().start().await;
    assert_eq!(v.state, DictationState::Recording, "{v:?}");
    w.d.dictation().cancel(DictationReason::Operator).await;
    w.d.stop.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), passive)
            .await
            .is_ok()
    );
}

/// A wake that notes how many of dictation's captures were alive when each wait began.
struct Watching {
    inner: Arc<RoomWake>,
    captures: Arc<ghost_audio::live::Registry>,
    seen: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl WakeDetector for Watching {
    async fn wait(&self) -> Option<String> {
        lock(&self.seen).push(
            self.captures
                .alive_for(ghost_audio::live::Holder::Dictation),
        );
        self.inner.wait().await
    }
    async fn stop(&self) {
        self.inner.stop().await;
    }
    fn close(&self) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dictation_waits_for_a_tape_still_opening_when_the_wake_stopped() {
    // The tape was retired while its capture was still opening: the wake's `stop` does not
    // wait for it (as before ADR 0077), the registry counts it, and dictation is granted only
    // once it has opened and closed.
    let mut w = world();
    let tape_closed_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let dictation_opened_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    struct ClosingMic(Arc<Mutex<Option<Instant>>>);
    #[async_trait]
    impl MediaIn for ClosingMic {
        async fn next_frame(&mut self) -> Option<Audio> {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some(Audio::new(vec![5; 320], RATE))
        }
    }
    impl Drop for ClosingMic {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(50));
            *lock(&self.0) = Some(Instant::now());
        }
    }
    let tape = ArrayTape::with_opener(
        "ghost-test-array".into(),
        {
            let closed = tape_closed_at.clone();
            Arc::new(move |_node: &str| {
                std::thread::sleep(Duration::from_millis(800)); // a slow open
                Ok(Box::new(ClosingMic(closed.clone())) as Box<dyn MediaIn>)
            })
        },
        Some(w.d.captures.clone()),
    );
    {
        let (room, opened_at) = (w.room.clone(), dictation_opened_at.clone());
        with_dictation_media(
            &mut w,
            Arc::new(move || {
                *lock(&opened_at) = Some(Instant::now());
                Ok(Box::new(GatedMic {
                    room: room.clone(),
                    closing: None,
                }) as Box<dyn MediaIn>)
            }),
        );
    }
    let detector: Arc<dyn WakeDetector> = Arc::new(TapedWake::new(w.wake.clone(), tape.clone()));
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    assert_eq!(
        w.d.captures.alive_for(ghost_audio::live::Holder::Tape),
        1,
        "opening"
    );
    let v = w.d.dictation().start().await;
    assert_eq!(v.state, DictationState::Recording, "{v:?}");
    let opened = lock(&dictation_opened_at).expect("dictation opened");
    let closed = lock(&tape_closed_at).expect("the tape's capture was opened and closed");
    assert!(
        opened > closed,
        "dictation opened beside the tape's capture"
    );
    w.d.dictation().cancel(DictationReason::Operator).await;
    w.d.stop.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), passive)
            .await
            .is_ok()
    );
}
