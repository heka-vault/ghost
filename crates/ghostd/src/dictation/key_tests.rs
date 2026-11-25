//! Dictation from the key (ADR 0077 as amended 2026-11-25): typed instead of the clipboard,
//! the model's tidy, the pieces read at pauses and shown live, the sound muted and put back,
//! and the key's exception to Not listening. A fake host whose microphone the test speaks
//! into and goes quiet on, a recognizer that reads one text per piece, and fakes for the
//! typing, the model, the live view and the sound.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ghost_voice::frames::Audio;
use ghost_voice::stt::Recognizer;

use super::*;

/// 20 ms frames, loud while `talking` is set and silent otherwise.
struct Mic {
    talking: Arc<AtomicBool>,
}

#[async_trait]
impl MediaIn for Mic {
    async fn next_frame(&mut self) -> Option<Audio> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let v = if self.talking.load(Ordering::SeqCst) {
            4000
        } else {
            0
        };
        Some(Audio::new(vec![v; 320], RATE))
    }
}

/// Speech while the frame is loud; the pause ends a piece after three quiet frames.
struct Loud {
    quiet: usize,
}

impl Pauses for Loud {
    fn speaking(&mut self, frame: &[i16]) -> bool {
        if frame.iter().any(|&s| s != 0) {
            self.quiet = 0;
            true
        } else {
            self.quiet += 1;
            self.quiet < 3
        }
    }
}

/// Reads the next of `texts` each time, then empty.
struct Reader {
    texts: Mutex<Vec<&'static str>>,
    reads: Arc<AtomicUsize>,
}

impl Recognizer for Reader {
    fn transcribe(&mut self, _audio: &[f32]) -> ghost_infer::Result<String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut t = self.texts.lock().unwrap();
        Ok(if t.is_empty() {
            String::new()
        } else {
            t.remove(0).to_string()
        })
    }
}

struct Host {
    off: AtomicBool,
    model: SharedRecognizer,
    talking: Arc<AtomicBool>,
    pieces: bool,
}

#[async_trait]
impl super::Host for Host {
    // The daemon's rule (`DaemonHost`): not listening refuses a command, not the key.
    fn refusal(&self, origin: Origin) -> Option<DictationReason> {
        (self.off.load(Ordering::SeqCst) && origin == Origin::Command)
            .then_some(DictationReason::NotListening)
    }
    fn still_allowed(&self, origin: Origin) -> Option<DictationReason> {
        self.refusal(origin)
    }
    async fn take_mic(&self) -> Result<MicHold, DictationReason> {
        Ok(MicHold::free())
    }
    async fn open(&self) -> Result<Box<dyn MediaIn>, DictationReason> {
        Ok(Box::new(Mic {
            talking: self.talking.clone(),
        }))
    }
    fn recognizer(&self) -> Option<SharedRecognizer> {
        Some(self.model.clone())
    }
    fn pauses(&self) -> Option<Box<dyn Pauses>> {
        self.pieces
            .then(|| Box::new(Loud { quiet: 0 }) as Box<dyn Pauses>)
    }
}

#[derive(Default)]
struct Typed {
    typed: Mutex<Vec<String>>,
    fail: bool,
}

#[async_trait]
impl Typer for Typed {
    async fn type_text(&self, text: &str) -> Result<(), DictationReason> {
        if self.fail {
            return Err(DictationReason::TypingFailed);
        }
        self.typed.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

struct Tidy {
    answer: Option<&'static str>,
    asked: AtomicUsize,
}

#[async_trait]
impl Reviewer for Tidy {
    async fn review(&self, _text: &str) -> Option<Zeroizing<String>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer.map(|a| Zeroizing::new(a.to_string()))
    }
}

#[derive(Default)]
struct Views(Mutex<Vec<DictationLiveView>>);

impl Live for Views {
    fn show(&self, view: DictationLiveView) {
        self.0.lock().unwrap().push(view);
    }
}

#[derive(Default)]
struct Sound {
    muted: AtomicUsize,
    restored: AtomicUsize,
}

impl Muter for Sound {
    fn mute(&self) {
        self.muted.fetch_add(1, Ordering::SeqCst);
    }
    fn restore(&self) {
        self.restored.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Board(Mutex<Vec<String>>);

impl Clipboard for Board {
    fn set(&self, text: Zeroizing<String>) -> Result<(), DictationReason> {
        self.0.lock().unwrap().push(text.to_string());
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

struct World {
    d: Arc<Dictation>,
    host: Arc<Host>,
    talking: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
    typed: Arc<Typed>,
    tidy: Arc<Tidy>,
    views: Arc<Views>,
    sound: Arc<Sound>,
    board: Arc<Board>,
    notes: Arc<Notes>,
}

struct Setup {
    texts: Vec<&'static str>,
    pieces: bool,
    fail_typing: bool,
    tidy: Option<&'static str>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            texts: vec!["Hello there."],
            pieces: false,
            fail_typing: false,
            tidy: None,
        }
    }
}

fn world(s: Setup) -> World {
    let talking = Arc::new(AtomicBool::new(true));
    let reads = Arc::new(AtomicUsize::new(0));
    let model: SharedRecognizer = Arc::new(Mutex::new(Reader {
        texts: Mutex::new(s.texts),
        reads: reads.clone(),
    }));
    let host = Arc::new(Host {
        off: AtomicBool::new(false),
        model,
        talking: talking.clone(),
        pieces: s.pieces,
    });
    let typed = Arc::new(Typed {
        fail: s.fail_typing,
        ..Typed::default()
    });
    let tidy = Arc::new(Tidy {
        answer: s.tidy,
        asked: AtomicUsize::new(0),
    });
    let views = Arc::new(Views::default());
    let sound = Arc::new(Sound::default());
    let board = Arc::new(Board::default());
    let notes = Arc::new(Notes::default());
    let d = Dictation::with_parts(
        host.clone(),
        board.clone(),
        notes.clone(),
        Parts {
            typer: typed.clone(),
            reviewer: tidy.clone(),
            live: views.clone(),
            muter: sound.clone(),
        },
        LIMIT,
    );
    World {
        d,
        host,
        talking,
        reads,
        typed,
        tidy,
        views,
        sound,
        board,
        notes,
    }
}

fn outcome(v: &DictationView) -> (DictationOutcome, Option<DictationReason>) {
    let r = v.result.expect("a result");
    (r.outcome, r.reason)
}

fn scenes(w: &World) -> Vec<DictationScene> {
    let mut out: Vec<DictationScene> = Vec::new();
    for v in w.views.0.lock().unwrap().iter() {
        if out.last() != Some(&v.scene) {
            out.push(v.scene);
        }
    }
    out
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

#[tokio::test]
async fn the_key_types_what_was_said_and_the_clipboard_is_untouched() {
    let w = world(Setup::default());
    assert_eq!(w.d.start_key().await.state, DictationState::Recording);
    tokio::time::sleep(Duration::from_millis(80)).await;
    let v = w.d.stop().await;
    assert_eq!(outcome(&v), (DictationOutcome::Typed, None));
    assert_eq!(*w.typed.typed.lock().unwrap(), ["Hello there."]);
    assert!(
        w.board.0.lock().unwrap().is_empty(),
        "the clipboard is not touched"
    );
    // The scene says it all: no notification at the start or for the typed text.
    assert!(w.notes.0.lock().unwrap().is_empty());
    assert_eq!(
        scenes(&w),
        [
            DictationScene::Starting,
            DictationScene::Listening,
            DictationScene::Writing,
            DictationScene::Typed
        ]
    );
    let last = w.views.0.lock().unwrap().last().cloned().unwrap();
    assert!(last.key);
    assert_eq!(last.sentence.as_deref(), Some("Typed 12 characters."));
}

#[tokio::test]
async fn a_command_still_ends_on_the_clipboard_and_is_notified() {
    let w = world(Setup::default());
    w.d.start().await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let v = w.d.stop().await;
    assert_eq!(outcome(&v), (DictationOutcome::OnClipboard, None));
    assert!(
        w.typed.typed.lock().unwrap().is_empty(),
        "a command never types"
    );
    assert_eq!(*w.board.0.lock().unwrap(), ["Hello there."]);
    assert_eq!(
        w.tidy.asked.load(Ordering::SeqCst),
        0,
        "nor goes to the model"
    );
    let notes = w.notes.0.lock().unwrap();
    assert_eq!(notes.len(), 2, "Dictating, then how it ended: {notes:?}");
}

#[tokio::test]
async fn english_only_an_accent_comes_off_and_the_text_is_typed() {
    let w = world(Setup {
        texts: vec!["Meet me at the caf\u{e9}."],
        ..Setup::default()
    });
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let v = w.d.stop().await;
    assert_eq!(outcome(&v), (DictationOutcome::Typed, None));
    assert_eq!(*w.typed.typed.lock().unwrap(), ["Meet me at the cafe."]);
    assert!(w.board.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn nothing_typable_goes_to_the_clipboard_and_is_notified() {
    let w = world(Setup {
        texts: vec!["\u{6771}\u{4eac}"],
        ..Setup::default()
    });
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let v = w.d.stop().await;
    assert_eq!(
        outcome(&v),
        (DictationOutcome::OnClipboard, Some(DictationReason::NoKeys))
    );
    assert!(w.typed.typed.lock().unwrap().is_empty());
    // The clipboard is notified even from the key: "Press Ctrl+V".
    assert_eq!(w.notes.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn typing_that_fails_falls_back_to_the_clipboard() {
    let w = world(Setup {
        fail_typing: true,
        ..Setup::default()
    });
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let v = w.d.stop().await;
    assert_eq!(
        outcome(&v),
        (
            DictationOutcome::OnClipboard,
            Some(DictationReason::TypingFailed)
        )
    );
    assert_eq!(*w.board.0.lock().unwrap(), ["Hello there."]);
}

#[tokio::test]
async fn the_models_small_fix_is_typed_and_a_rewrite_is_not() {
    let w = world(Setup {
        texts: vec!["send it to the hole team"],
        tidy: Some("Send it to the whole team."),
        ..Setup::default()
    });
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    w.d.stop().await;
    assert_eq!(
        *w.typed.typed.lock().unwrap(),
        ["Send it to the whole team."]
    );

    let w = world(Setup {
        texts: vec!["can you check the build"],
        tidy: Some("Sure! I checked: the build passed on main, all green."),
        ..Setup::default()
    });
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    w.d.stop().await;
    assert_eq!(w.tidy.asked.load(Ordering::SeqCst), 1);
    assert_eq!(*w.typed.typed.lock().unwrap(), ["can you check the build"]);
}

#[tokio::test]
async fn pieces_are_read_at_pauses_while_speaking_and_shown_live() {
    let w = world(Setup {
        texts: vec!["First part.", "Second part.", "The end."],
        pieces: true,
        ..Setup::default()
    });
    w.d.start_key().await;
    // Speak, pause, speak, pause: two pieces read while recording goes on.
    tokio::time::sleep(Duration::from_millis(700)).await;
    w.talking.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    w.talking.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(700)).await;
    w.talking.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    settle(|| w.reads.load(Ordering::SeqCst) == 2).await;
    assert_eq!(
        w.d.state(),
        DictationState::Recording,
        "read while recording"
    );
    settle(|| {
        w.views
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|v| v.text.as_deref() == Some("First part. Second part."))
    })
    .await;
    // The rest is read at the end.
    w.talking.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let v = w.d.stop().await;
    assert_eq!(outcome(&v).0, DictationOutcome::Typed);
    assert_eq!(
        *w.typed.typed.lock().unwrap(),
        ["First part. Second part. The end."]
    );
    assert_eq!(w.reads.load(Ordering::SeqCst), 3);
    // Levels were shown while listening, and no view after the end carries words.
    let views = w.views.0.lock().unwrap();
    assert!(views.iter().any(|v| v.level.is_some_and(|l| l > 0.0)));
    assert!(views.last().unwrap().text.is_none());
}

#[tokio::test]
async fn the_sound_is_muted_while_recording_and_put_back_however_it_ends() {
    let w = world(Setup::default());
    w.d.start_key().await;
    assert_eq!(w.sound.muted.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(60)).await;
    w.d.stop().await;
    settle(|| w.sound.restored.load(Ordering::SeqCst) == 1).await;

    let w = world(Setup::default());
    w.d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    w.d.cancel(DictationReason::Operator).await;
    settle(|| w.sound.restored.load(Ordering::SeqCst) == 1).await;
    assert_eq!(w.sound.muted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_key_opens_the_microphone_while_not_listening_and_a_command_does_not() {
    let w = world(Setup::default());
    w.host.off.store(true, Ordering::SeqCst);
    assert_eq!(
        outcome(&w.d.start().await),
        (
            DictationOutcome::Refused,
            Some(DictationReason::NotListening)
        )
    );
    assert_eq!(w.d.start_key().await.state, DictationState::Recording);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(outcome(&w.d.stop().await).0, DictationOutcome::Typed);
}

#[tokio::test]
async fn a_cancel_while_the_model_tidies_types_nothing() {
    struct Slow;
    #[async_trait]
    impl Reviewer for Slow {
        async fn review(&self, text: &str) -> Option<Zeroizing<String>> {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Some(Zeroizing::new(text.to_string()))
        }
    }
    let w = world(Setup::default());
    let d = Dictation::with_parts(
        w.host.clone(),
        w.board.clone(),
        w.notes.clone(),
        Parts {
            typer: w.typed.clone(),
            reviewer: Arc::new(Slow),
            live: w.views.clone(),
            muter: w.sound.clone(),
        },
        LIMIT,
    );
    d.start_key().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let stopping = {
        let d = d.clone();
        tokio::spawn(async move { d.stop().await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(d.state(), DictationState::Reading, "the model is tidying");
    d.cancel(DictationReason::Operator).await;
    stopping.await.unwrap();
    assert!(w.typed.typed.lock().unwrap().is_empty());
    assert!(w.board.0.lock().unwrap().is_empty());
}

#[test]
fn the_level_is_dbfs_mapped_to_zero_to_one() {
    assert_eq!(level_of(&[]), 0.0);
    assert_eq!(level_of(&[0; 320]), 0.0);
    assert!((level_of(&[32767; 320]) - 1.0).abs() < 0.01);
    let quiet = level_of(&[100; 320]); // about -50 dBFS
    assert!(quiet < 0.05, "{quiet}");
}
