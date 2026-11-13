//! The music in a typed session (2026-11-13): lowered, and paused where it is a player,
//! only while Ghost's spoken reply plays.
//!
//! A voice session holds the duck from its start to its end: the operator is talking to
//! Ghost, and the canceller needs the music off to hear them (`players`). A typed session
//! has no microphone, so the only reason to lower the music is the reply Ghost says aloud.
//! It stays open `GHOST_TYPED_IDLE_SECS` (20 s) after that reply in case the operator types
//! again, and holding the duck through that wait left music down for 20 s after every
//! typed question. Here the duck is taken when the session opens (a reply is coming) and
//! when a further reply starts to play, and given back once a reply has finished: Ghost
//! stopped speaking, `RELEASE_AFTER` went by without it speaking again, and no call to a
//! tool is out (an acknowledgment, "One sec.", is followed by the answer). The session's
//! end gives it back if it is still held.
//!
//! The duck is never taken twice without a release between: `ghost_audio::Ducker` saves
//! the volumes it found, and a second duck would save the lowered ones.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_voice::activity::Activity;
use ghost_voice::tap::SessionPhase;

use crate::daemon::Duck;
use crate::players::PlayersPort;

/// How long after Ghost stops speaking the music comes back, unless it speaks again: the
/// gap between an acknowledgment or a sentence and the next.
pub const RELEASE_AFTER: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct State {
    held: bool,
    speaking: bool,
    /// Calls to tools started and not yet returned.
    calls: usize,
    /// Bumped by everything that cancels a pending release.
    generation: u64,
}

/// The duck of one typed session.
pub struct ReplyDuck {
    ducker: Option<Arc<dyn Duck>>,
    players: Arc<dyn PlayersPort>,
    pause_players: bool,
    state: Mutex<State>,
}

impl ReplyDuck {
    pub fn new(
        ducker: Option<Arc<dyn Duck>>,
        players: Arc<dyn PlayersPort>,
        pause_players: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            ducker,
            players,
            pause_players,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lower the music and pause the players, unless that is already done.
    pub async fn take(&self) {
        {
            let mut s = self.state();
            s.generation += 1;
            if s.held {
                return;
            }
            s.held = true;
        }
        if let Some(d) = &self.ducker {
            d.duck();
        }
        if self.pause_players {
            self.players.pause_playing().await;
        }
    }

    /// Put the music back and resume what was paused, if the duck is held.
    pub async fn release(&self) {
        {
            let mut s = self.state();
            if !s.held {
                return;
            }
            s.held = false;
        }
        if let Some(d) = &self.ducker {
            d.restore();
        }
        self.players.resume().await;
    }

    pub fn held(&self) -> bool {
        self.state().held
    }

    /// The session's phase: a reply starting to play takes the duck, one that stopped
    /// gives it back after `RELEASE_AFTER` if nothing else happened meanwhile.
    pub async fn on_phase(self: &Arc<Self>, phase: SessionPhase) {
        match phase {
            SessionPhase::Speaking => {
                self.state().speaking = true;
                self.take().await;
            }
            SessionPhase::Listening | SessionPhase::Hold => {
                let generation = {
                    let mut s = self.state();
                    s.speaking = false;
                    s.generation += 1;
                    s.generation
                };
                let me = Arc::downgrade(self);
                tokio::spawn(async move {
                    tokio::time::sleep(RELEASE_AFTER).await;
                    let Some(me) = me.upgrade() else { return };
                    let done = {
                        let s = me.state();
                        s.generation == generation && !s.speaking && s.calls == 0
                    };
                    if done {
                        me.release().await;
                    }
                });
            }
            SessionPhase::Thinking => {
                self.state().generation += 1;
            }
        }
    }

    /// What the session is doing: a call out to a tool means the answer is still coming.
    pub fn on_activity(&self, a: &Activity) {
        let mut s = self.state();
        match a {
            Activity::Call { .. } => {
                s.calls += 1;
                s.generation += 1;
            }
            Activity::Result { .. } => s.calls = s.calls.saturating_sub(1),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;

    #[derive(Default)]
    struct Log(Mutex<Vec<&'static str>>);

    impl Log {
        fn take(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    struct FakeDuck(Arc<Log>);
    impl Duck for FakeDuck {
        fn duck(&self) {
            self.0.0.lock().unwrap().push("duck");
        }
        fn restore(&self) {
            self.0.0.lock().unwrap().push("restore");
        }
    }

    struct FakePlayers(Arc<Log>);
    impl PlayersPort for FakePlayers {
        fn pause_playing(&self) -> BoxFuture<'_, ()> {
            self.0.0.lock().unwrap().push("pause");
            Box::pin(async {})
        }
        fn resume(&self) -> BoxFuture<'_, ()> {
            self.0.0.lock().unwrap().push("resume");
            Box::pin(async {})
        }
    }

    fn duck() -> (Arc<ReplyDuck>, Arc<Log>) {
        let log = Arc::new(Log::default());
        let d = ReplyDuck::new(
            Some(Arc::new(FakeDuck(log.clone()))),
            Arc::new(FakePlayers(log.clone())),
            true,
        );
        (d, log)
    }

    async fn after(d: Duration) {
        tokio::time::sleep(d).await;
        tokio::task::yield_now().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_music_comes_back_once_the_typed_reply_has_played() {
        let (d, log) = duck();
        d.take().await; // the session opens: a reply is coming
        d.on_phase(SessionPhase::Speaking).await;
        assert_eq!(log.take(), vec!["duck", "pause"], "taken once");
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER / 2).await;
        assert!(d.held(), "not between two sentences");
        after(RELEASE_AFTER).await;
        assert_eq!(log.take(), vec!["restore", "resume"]);
        assert!(!d.held());
        // The session stays open for the idle wait; nothing more happens to the music.
        after(Duration::from_secs(20)).await;
        d.release().await; // the session's end
        assert!(log.take().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_further_reply_in_the_same_session_takes_it_again() {
        let (d, log) = duck();
        d.take().await;
        d.on_phase(SessionPhase::Speaking).await;
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER * 2).await;
        log.take();
        d.on_phase(SessionPhase::Thinking).await; // typed again: nothing said yet
        assert!(
            log.take().is_empty(),
            "typing alone does not lower the music"
        );
        d.on_phase(SessionPhase::Speaking).await;
        assert_eq!(log.take(), vec!["duck", "pause"]);
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER * 2).await;
        assert_eq!(log.take(), vec!["restore", "resume"]);
    }

    #[tokio::test(start_paused = true)]
    async fn speaking_again_within_the_gap_keeps_it() {
        let (d, log) = duck();
        d.take().await;
        d.on_phase(SessionPhase::Speaking).await; // "One sec."
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER / 2).await;
        d.on_phase(SessionPhase::Speaking).await; // the answer
        after(RELEASE_AFTER * 2).await;
        assert_eq!(log.take(), vec!["duck", "pause"], "held while it speaks");
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER * 2).await;
        assert_eq!(log.take(), vec!["restore", "resume"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_out_keeps_it_until_the_answer_has_played() {
        let (d, log) = duck();
        d.take().await;
        d.on_phase(SessionPhase::Speaking).await; // "One sec."
        d.on_activity(&Activity::Call {
            name: "github_issue_read".into(),
            id: "1".into(),
        });
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER * 4).await;
        assert!(d.held(), "the answer is still coming");
        d.on_activity(&Activity::Result {
            name: "github_issue_read".into(),
            id: "1".into(),
        });
        d.on_phase(SessionPhase::Speaking).await;
        d.on_phase(SessionPhase::Listening).await;
        after(RELEASE_AFTER * 2).await;
        assert_eq!(log.take(), vec!["duck", "pause", "restore", "resume"]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_gives_back_a_duck_still_held_once() {
        let (d, log) = duck();
        d.take().await;
        d.take().await;
        d.release().await;
        d.release().await;
        assert_eq!(log.take(), vec!["duck", "pause", "restore", "resume"]);
    }
}
