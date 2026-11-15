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

use super::*;
use crate::daemon::tests::Recorder;
use crate::daemon::{Daemon, SessionStart};
use crate::wake::WakeDetector;
use crate::wake_takes::{ArrayTape, Keeper, TapedWake};

const BEFORE: i16 = 100;
const DICTATED: i16 = 900;
const AFTER: i16 = 300;

/// What is being said in the room now.
struct Room(AtomicI16);

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
                    let v = self.room.0.load(Ordering::SeqCst);
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
        Some(Audio::new(vec![self.0.0.load(Ordering::SeqCst); 160], RATE))
    }
}

/// Reads the marked sentence when it hears dictated audio.
struct Reads;

impl Recognizer for Reads {
    fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
        let dictated = f32::from(DICTATED) / 32768.0;
        let heard = audio.iter().any(|&x| (x - dictated).abs() < 1e-3);
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
    tape: Arc<ArrayTape>,
    takes: Arc<Takes>,
    board: Arc<Board>,
    recorder: Arc<Recorder>,
    _tmp: tempfile::TempDir,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let room = Arc::new(Room(AtomicI16::new(BEFORE)));
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
    let tape = ArrayTape::new("ghost-test-no-such-array".into());
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
        tape,
        takes,
        board,
        recorder,
        _tmp: tmp,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dictation_never_reaches_the_wake_or_a_kept_take() {
    let w = world();
    // The array's raw capture, as if it never paused: the room into the tape every 10 ms.
    let feeding = {
        let (room, tape) = (w.room.clone(), w.tape.clone());
        tokio::spawn(async move {
            loop {
                let v = room.0.load(Ordering::SeqCst);
                tape.push(&vec![v; 320], Instant::now());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    let detector: Arc<dyn WakeDetector> = Arc::new(TapedWake::new(w.wake.clone(), w.tape.clone()));
    let passive = {
        let d = w.d.clone();
        tokio::spawn(async move { d.passive(detector).await })
    };
    // Before: the wake hears the room.
    until(|| w.wake.waiting.load(Ordering::SeqCst) && !lock(&w.wake.heard).is_empty()).await;
    // Dictation takes the microphone: the wake stops before its capture opens.
    let v = w.d.dictation().start().await;
    assert_eq!(v.state, DictationState::Recording, "{v:?}");
    assert!(
        !w.wake.waiting.load(Ordering::SeqCst),
        "the wake is stopped"
    );
    w.room.0.store(DICTATED, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!w.wake.waiting.load(Ordering::SeqCst), "and stays stopped");
    // The operator is already saying Ghost's name as they stop.
    w.room.0.store(AFTER, Ordering::SeqCst);
    let v = w.d.dictation().stop().await;
    let r = v.result.unwrap();
    assert_eq!(r.outcome, DictationOutcome::OnClipboard, "{r:?}");
    assert_eq!(*lock(&w.board.0), ["Pick up milk."]);
    // At once, the wake listens again and hits.
    until(|| w.wake.waits.load(Ordering::SeqCst) >= 2 && w.wake.waiting.load(Ordering::SeqCst))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.wake.hit.notify_one();
    until(|| !lock(&w.recorder.0).is_empty()).await;
    until(|| !w.takes.list().is_empty()).await;
    // The take that was kept holds the detector's stream and the array's, and neither has
    // a sample of the dictation.
    let take = &w.takes.list()[0];
    let detector_audio = w.takes.audio(&take.id, false).unwrap();
    let array_audio = w.takes.audio(&take.id, true).unwrap();
    assert!(!detector_audio.is_empty() && !array_audio.is_empty());
    assert!(
        !detector_audio.contains(&DICTATED),
        "the detector's stream holds dictated audio"
    );
    assert!(
        !array_audio.contains(&DICTATED),
        "the array's tape holds dictated audio"
    );
    assert!(
        array_audio.len() < 3 * RATE as usize * 2,
        "the tape began empty when the wake listened again: {} samples",
        array_audio.len()
    );
    // Nothing the wake ever heard, across every wait, was dictated.
    assert!(!lock(&w.wake.ever).contains(&DICTATED));
    w.d.stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), passive).await;
    feeding.abort();
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
    w.room.0.store(DICTATED, Ordering::SeqCst);
    assert_eq!(
        w.d.dictation().start().await.state,
        DictationState::Recording
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The room goes quiet before the cancel, so anything dictated the wake heard, it heard
    // while dictation recorded.
    w.room.0.store(BEFORE, Ordering::SeqCst);
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
    // Listening off: refused while off, and a dictation recording when it goes off is dropped.
    until(|| w.wake.waiting.load(Ordering::SeqCst)).await;
    assert_eq!(
        w.d.dictation().start().await.state,
        DictationState::Recording
    );
    w.room.0.store(DICTATED, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.room.0.store(BEFORE, Ordering::SeqCst);
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
