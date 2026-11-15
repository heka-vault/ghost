//! The coordinator over a fake host: a microphone that plays what the test says, a
//! recognizer that reads a fixed sentence (or waits to be let go), and a clipboard that
//! records what it was given.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ghost_voice::stt::Recognizer;

use super::*;

/// Frames of `value`, 20 ms each; `fast` gives them as quickly as they are asked for.
struct Mic {
    value: i16,
    fast: bool,
    frames: Option<usize>,
    open: Arc<AtomicUsize>,
}

#[async_trait]
impl MediaIn for Mic {
    async fn next_frame(&mut self) -> Option<Audio> {
        if let Some(n) = self.frames.as_mut() {
            if *n == 0 {
                return None;
            }
            *n -= 1;
        }
        if self.fast {
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some(Audio::new(vec![self.value; 320], RATE))
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Reads `text`; with `gate` set, waits until it is let go.
struct Reader {
    text: String,
    gate: Option<Arc<AtomicBool>>,
    reads: Arc<AtomicUsize>,
}

impl Recognizer for Reader {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        if let Some(g) = &self.gate {
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.text.clone())
    }
}

#[derive(Default)]
struct Board {
    set: Mutex<Vec<String>>,
    fail: Option<DictationReason>,
}

impl Clipboard for Board {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        if let Some(f) = self.fail {
            return Err(f);
        }
        self.set.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

#[derive(Default)]
struct Notes(Mutex<Vec<Note>>);

impl Notifier for Notes {
    fn show(&self, note: Note) {
        self.0.lock().unwrap().push(note);
    }
}

struct Fake {
    refusal: Mutex<Option<DictationReason>>,
    model: SharedRecognizer,
    fast: bool,
    frames: Option<usize>,
    /// Captures open now.
    open: Arc<AtomicUsize>,
    /// Holds of the passive loop's microphone alive now.
    held: Arc<AtomicUsize>,
}

#[async_trait]
impl Host for Fake {
    fn refusal(&self) -> Option<DictationReason> {
        *self.refusal.lock().unwrap()
    }
    async fn take_mic(&self) -> Result<MicHold, DictationReason> {
        self.held.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let held = self.held.clone();
        tokio::spawn(async move {
            let _ = rx.await;
            held.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(MicHold(Some(tx)))
    }
    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason> {
        self.open.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Mic {
            value: 900,
            fast: self.fast,
            frames: self.frames,
            open: self.open.clone(),
        }))
    }
    fn recognizer(&self) -> Option<SharedRecognizer> {
        Some(self.model.clone())
    }
}

struct World {
    d: Arc<Dictation>,
    host: Arc<Fake>,
    board: Arc<Board>,
    notes: Arc<Notes>,
    gate: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
}

fn world_with(text: &str, gated: bool, fast: bool, limit: Duration, board: Board) -> World {
    let gate = Arc::new(AtomicBool::new(!gated));
    let reads = Arc::new(AtomicUsize::new(0));
    let model: SharedRecognizer = Arc::new(Mutex::new(Reader {
        text: text.into(),
        gate: Some(gate.clone()),
        reads: reads.clone(),
    }));
    let host = Arc::new(Fake {
        refusal: Mutex::new(None),
        model,
        fast,
        frames: None,
        open: Arc::new(AtomicUsize::new(0)),
        held: Arc::new(AtomicUsize::new(0)),
    });
    let board = Arc::new(board);
    let notes = Arc::new(Notes::default());
    let d = Dictation::new(host.clone(), board.clone(), notes.clone(), limit);
    World {
        d,
        host,
        board,
        notes,
        gate,
        reads,
    }
}

fn world(text: &str) -> World {
    world_with(text, false, false, LIMIT, Board::default())
}

async fn settle(what: impl Fn() -> bool) {
    for _ in 0..400 {
        if what() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never settled");
}

fn outcome(v: &DictationView) -> (DictationOutcome, Option<DictationReason>) {
    let r = v.result.expect("a result");
    (r.outcome, r.reason)
}

#[tokio::test]
async fn a_dictation_reads_once_cleans_up_and_lands_on_the_clipboard() {
    let w = world("Um, so pick up milk.\nAnd eggs.");
    let v = w.d.start().await;
    assert_eq!(v.state, DictationState::Recording);
    assert!(v.result.is_none());
    assert!(v.first_frame_ms.is_some());
    assert_eq!(w.host.open.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let v = w.d.stop().await;
    assert_eq!(v.state, DictationState::Idle);
    let r = v.result.unwrap();
    assert_eq!(r.outcome, DictationOutcome::OnClipboard);
    assert_eq!(r.reason, None);
    assert_eq!(*w.board.set.lock().unwrap(), ["So pick up milk. And eggs."]);
    assert_eq!(r.chars, "So pick up milk. And eggs.".len());
    assert!(r.seconds > 0.05, "{}", r.seconds);
    assert!(r.read_ms.is_some() && r.queue_ms.is_some());
    // the capture is closed and the passive loop has the microphone back
    assert_eq!(w.host.open.load(Ordering::SeqCst), 0);
    settle(|| w.host.held.load(Ordering::SeqCst) == 0).await;
    assert_eq!(w.reads.load(Ordering::SeqCst), 1, "one whole read");
    assert_eq!(
        *w.notes.0.lock().unwrap(),
        [Note::Recording, Note::Ended(r)]
    );
    // the last result is there to read, as kinds and numbers
    assert_eq!(w.d.view().result, Some(r));
}

#[tokio::test]
async fn start_stop_and_cancel_are_idempotent_and_a_second_start_is_refused() {
    let w = world_with("Hello there.", true, false, LIMIT, Board::default());
    assert_eq!(
        outcome(&w.d.stop().await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::NotDictating)
        )
    );
    assert_eq!(
        outcome(&w.d.cancel(DictationReason::Operator).await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::NotDictating)
        )
    );
    w.d.start().await;
    assert_eq!(
        outcome(&w.d.start().await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::AlreadyDictating)
        )
    );
    assert_eq!(w.host.open.load(Ordering::SeqCst), 1, "no second capture");
    // stop while the read is held: a start, a toggle and a second stop wait or refuse
    let d = w.d.clone();
    let stopping = tokio::spawn(async move { d.stop().await });
    settle(|| w.d.state() == DictationState::Reading).await;
    assert_eq!(
        outcome(&w.d.start().await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::StillReading)
        )
    );
    assert_eq!(
        outcome(&w.d.toggle().await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::StillReading)
        )
    );
    let d = w.d.clone();
    let again = tokio::spawn(async move { d.stop().await });
    w.gate.store(true, Ordering::SeqCst);
    let first = stopping.await.unwrap();
    let second = again.await.unwrap();
    assert_eq!(outcome(&first).0, DictationOutcome::OnClipboard);
    assert_eq!(
        first.result, second.result,
        "the second stop answers with the same read"
    );
    assert_eq!(w.reads.load(Ordering::SeqCst), 1);
    assert_eq!(w.board.set.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_cancelled_read_that_finishes_late_does_nothing() {
    let w = world_with("Do not paste me.", true, false, LIMIT, Board::default());
    w.d.start().await;
    let d = w.d.clone();
    let stopping = tokio::spawn(async move { d.stop().await });
    settle(|| w.d.state() == DictationState::Reading).await;
    let v = w.d.cancel(DictationReason::Operator).await;
    assert_eq!(v.state, DictationState::Idle);
    assert_eq!(
        outcome(&v),
        (DictationOutcome::Cancelled, Some(DictationReason::Operator))
    );
    // a new dictation starts while the old read is still running
    let started = w.d.start().await;
    assert_eq!(started.state, DictationState::Recording);
    w.gate.store(true, Ordering::SeqCst);
    let late = stopping.await.unwrap();
    assert_eq!(outcome(&late).0, DictationOutcome::Cancelled);
    assert!(
        w.board.set.lock().unwrap().is_empty(),
        "nothing reached the clipboard"
    );
    assert_eq!(
        w.d.state(),
        DictationState::Recording,
        "the new one goes on"
    );
    w.d.cancel(DictationReason::Operator).await;
}

#[tokio::test]
async fn cancelling_a_recording_closes_the_microphone_and_keeps_nothing() {
    let w = world("Never read.");
    w.d.start().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let v = w.d.cancel(DictationReason::ListeningTurnedOff).await;
    assert_eq!(
        outcome(&v),
        (
            DictationOutcome::Cancelled,
            Some(DictationReason::ListeningTurnedOff)
        )
    );
    assert_eq!(w.host.open.load(Ordering::SeqCst), 0);
    settle(|| w.host.held.load(Ordering::SeqCst) == 0).await;
    assert_eq!(w.reads.load(Ordering::SeqCst), 0, "nothing was read");
    assert!(w.board.set.lock().unwrap().is_empty());
    w.d.end(DictationReason::Stopping).await; // nothing going on: nothing to do
    assert_eq!(
        w.d.view().result.unwrap().reason,
        Some(DictationReason::ListeningTurnedOff)
    );
}

#[tokio::test]
async fn the_limit_stops_it_and_what_was_said_is_read() {
    let w = world_with(
        "A long thought.",
        false,
        true,
        Duration::from_millis(500),
        Board::default(),
    );
    w.d.start().await;
    settle(|| w.d.view().result.is_some()).await;
    let r = w.d.view().result.unwrap();
    assert_eq!(r.outcome, DictationOutcome::OnClipboard);
    assert_eq!(r.reason, Some(DictationReason::Limit));
    assert!((r.seconds - 0.5).abs() < 1e-9, "{}", r.seconds);
    assert_eq!(w.d.state(), DictationState::Idle);
    assert_eq!(w.host.open.load(Ordering::SeqCst), 0);
    // stopping after the limit has nothing to stop
    assert_eq!(
        outcome(&w.d.stop().await).1,
        Some(DictationReason::NotDictating)
    );
}

#[tokio::test]
async fn the_microphone_going_away_ends_it_and_reads_what_there_is() {
    let w = world("Short.");
    let host = Arc::new(Fake {
        refusal: Mutex::new(None),
        model: w.host.model.clone(),
        fast: false,
        frames: Some(3),
        open: Arc::new(AtomicUsize::new(0)),
        held: Arc::new(AtomicUsize::new(0)),
    });
    let d = Dictation::new(host.clone(), w.board.clone(), w.notes.clone(), LIMIT);
    d.start().await;
    settle(|| d.view().result.is_some()).await;
    let r = d.view().result.unwrap();
    assert_eq!(r.outcome, DictationOutcome::OnClipboard);
    assert_eq!(r.reason, Some(DictationReason::MicrophoneEnded));
}

#[tokio::test]
async fn a_refusal_opens_nothing_and_says_why() {
    for why in [
        DictationReason::NotListening,
        DictationReason::Busy,
        DictationReason::HearingTest,
        DictationReason::NoSpeechModel,
        DictationReason::NoMicrophone,
    ] {
        let w = world("Hello.");
        *w.host.refusal.lock().unwrap() = Some(why);
        let v = w.d.start().await;
        assert_eq!(v.state, DictationState::Idle);
        assert_eq!(outcome(&v), (DictationOutcome::Refused, Some(why)));
        assert_eq!(w.host.open.load(Ordering::SeqCst), 0);
        assert_eq!(w.host.held.load(Ordering::SeqCst), 0);
        assert_eq!(
            *w.notes.0.lock().unwrap(),
            [Note::Ended(DictationResult::refused(why))]
        );
    }
}

#[tokio::test]
async fn nothing_heard_and_a_clipboard_that_fails_are_said_as_such() {
    let w = world("Um. Uh.");
    w.d.start().await;
    let v = w.d.stop().await;
    assert_eq!(outcome(&v), (DictationOutcome::NothingHeard, None));
    assert!(w.board.set.lock().unwrap().is_empty());

    let w = world_with(
        "Hello.",
        false,
        false,
        LIMIT,
        Board {
            fail: Some(DictationReason::NoDisplay),
            ..Board::default()
        },
    );
    w.d.start().await;
    let v = w.d.stop().await;
    assert_eq!(
        outcome(&v),
        (
            DictationOutcome::NotOnClipboard,
            Some(DictationReason::NoDisplay)
        )
    );
    assert_eq!(v.result.unwrap().chars, 6);
}

#[tokio::test]
async fn toggle_starts_then_stops() {
    let w = world("Toggle me.");
    assert_eq!(w.d.toggle().await.state, DictationState::Recording);
    let v = w.d.toggle().await;
    assert_eq!(outcome(&v).0, DictationOutcome::OnClipboard);
    assert_eq!(*w.board.set.lock().unwrap(), ["Toggle me."]);
}

#[test]
fn the_read_is_the_recognize_path() {
    // Dictation times the wait for the model and the read apart, so it takes the lock
    // itself; what Parakeet is given must stay what `stt::recognize` gives it for a segment
    // with its trailing silence: the same samples, dithered the same way.
    struct Echo;
    impl Recognizer for Echo {
        fn transcribe(&mut self, audio: &[f32]) -> ghost_infer::Result<String> {
            let sum: f64 = audio.iter().map(|&x| f64::from(x) * 1e6).sum();
            Ok(format!(" {} {sum:.3} ", audio.len()))
        }
    }
    let model: SharedRecognizer = Arc::new(Mutex::new(Echo));
    let samples: Vec<i16> = (0..8000).map(|i| ((i * 37) % 2000) as i16 - 1000).collect();
    let (ours, _, _) = read(&model, &samples);
    let mut audio = ghost_infer::audio::to_f32(&samples);
    audio.extend(std::iter::repeat_n(
        0.0f32,
        (TRAILING_SECS * f64::from(RATE)) as usize,
    ));
    let theirs = ghost_voice::stt::recognize(&model, &audio, true).unwrap();
    assert_eq!(ours.as_str(), theirs);
}

#[tokio::test]
async fn a_lease_is_given_back_when_the_hold_drops_and_withdrawn_when_nobody_takes_it() {
    let leases = Arc::new(Leases::default());
    // nobody takes it: withdrawn, so a later loop does not grant a stale one
    assert!(leases.ask(Duration::from_millis(50)).await.is_none());
    let l2 = leases.clone();
    let taker = tokio::spawn(async move {
        let lease = l2.next().await;
        lease.granted.send(()).unwrap();
        lease.release.await.is_ok()
    });
    let hold = leases.ask(Duration::from_secs(2)).await.expect("granted");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !taker.is_finished(),
        "the loop waits while dictation holds it"
    );
    drop(hold);
    assert!(taker.await.unwrap(), "dropping the hold gives it back");
}

#[test]
fn the_command_line_says_kinds_and_numbers() {
    let recording = DictationView {
        state: DictationState::Recording,
        limit_secs: 30.0,
        recording_secs: Some(0.0),
        first_frame_ms: Some(41),
        result: None,
    };
    let (line, code) = describe(&recording);
    assert_eq!(code, 0);
    assert!(line.contains("41 ms"), "{line}");
    let done = DictationView {
        state: DictationState::Idle,
        recording_secs: None,
        first_frame_ms: None,
        result: Some(DictationResult {
            seconds: 4.2,
            chars: 17,
            first_frame_ms: Some(41),
            queue_ms: Some(0),
            read_ms: Some(160),
            ..DictationResult::of(DictationOutcome::OnClipboard, None)
        }),
        ..recording
    };
    let (line, code) = describe(&done);
    assert_eq!(code, 0);
    assert!(
        line.starts_with("On your clipboard: 17 characters."),
        "{line}"
    );
    assert!(line.contains("160 ms"), "{line}");
    let refused = DictationView {
        result: Some(DictationResult::refused(DictationReason::NotListening)),
        ..done
    };
    assert_eq!(
        describe(&refused),
        (
            "Ghost isn't listening. Turn listening on to dictate.".to_string(),
            1
        )
    );
}
