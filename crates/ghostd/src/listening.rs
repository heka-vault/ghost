//! Not listening, the daemon's side (ADR 0055): the passive loop with the microphone
//! closed, the switch from the control API, and the status that says so.
//!
//! While the switch is off the passive loop opens no capture: the wake listener is
//! stopped (its pipeline, and with it the PipeWire stream, is dropped) and is not built
//! again until the switch is on. It still answers what needs no microphone: a typed turn
//! (a session whose input is silent), the phone's own call (its audio is the phone's), and
//! it turns a raise at the desk away so the routes reach the operator another way.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};
use ghost_api::models::NotListening;
use ghost_core::listening::Listening;
use ghost_core::routes::Delivery;
use ghost_core::time::iso;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::daemon::{Daemon, Won, lock};
use ghost_api::HearPort;

/// What the status says for the switch at `now`.
pub fn view(l: Listening, now: DateTime<Utc>) -> Option<NotListening> {
    match l.at(now) {
        Listening::On => None,
        Listening::Off { until } => Some(NotListening {
            until: until.as_ref().map(iso),
        }),
    }
}

/// Until `until` (forever without one).
async fn until(t: Option<DateTime<Utc>>) {
    match t.and_then(|t| (t - Utc::now()).to_std().ok()) {
        Some(d) => tokio::time::sleep(d).await,
        None if t.is_some() => {} // already past
        None => std::future::pending().await,
    }
}

impl Daemon {
    /// Keeps the status's `not_listening` true to the switch, an end that passes included.
    pub(crate) fn follow_mic(self: &Arc<Self>) {
        let d = self.clone();
        tokio::spawn(async move {
            let mut rx = d.mic.watch();
            loop {
                let now = *rx.borrow_and_update();
                d.state.set_not_listening(view(now, Utc::now()));
                let ends = match now.at(Utc::now()) {
                    Listening::Off { until: Some(t) } => Some(t),
                    _ => None,
                };
                tokio::select! {
                    _ = d.stop.cancelled() => break,
                    r = rx.changed() => if r.is_err() { break },
                    _ = until(ends), if ends.is_some() => {
                        info!("listening: back on, as set");
                        d.rearm.set(); // the passive loop opens the microphone again
                    }
                }
            }
        });
    }

    /// The operator's switch (the desktop, the tray, the phone, a shortcut). Turning it off
    /// ends a session at the desk and a Hearing test, so nothing keeps the microphone; the
    /// phone's own call goes on, its audio being the phone's.
    pub(crate) async fn set_listening(
        &self,
        listening: bool,
        minutes: Option<u32>,
    ) -> Result<Option<NotListening>, String> {
        let set = if listening {
            self.mic.on()
        } else {
            self.mic.off(minutes)
        }
        .map_err(|e| format!("could not keep the switch: {e}"))?;
        let view = view(set, Utc::now());
        // Now, not when the follower runs: the answer and the status agree.
        self.state.set_not_listening(view.clone());
        if listening {
            info!("listening: on");
            self.rearm.set();
            return Ok(view);
        }
        info!("listening: off; the microphone closes");
        if let Some(session) = self.session()
            && *lock(&self.origin) != Some(ghost_api::models::SessionOrigin::Phone)
        {
            self.state.set_ending();
            session.end().await;
        }
        if self.state.phase() == ghost_api::Phase::Recording
            && let Some(hear) = self.hear()
        {
            HearPort::close(&*hear).await;
        }
        Ok(view)
    }

    /// The passive loop's wait with the microphone closed: no wake listener runs. A typed
    /// turn, the phone's call, a change of the switch or a settings change ends it; a raise
    /// at the desk is turned away and a Hearing test's lease is refused.
    pub(crate) async fn wait_not_listening(&self, ends: Option<DateTime<Utc>>) -> Won {
        info!("passive: not listening; the microphone is closed");
        let mut mic: watch::Receiver<Listening> = self.mic.watch();
        mic.borrow_and_update();
        let manual = lock(&self.manual).clone();
        loop {
            let typed = async {
                match &manual {
                    Some(m) => m.wait().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = self.stop.cancelled() => return Won::Stop,
                _ = mic.changed() => return Won::Rearm,
                _ = until(ends) => return Won::Rearm,
                _ = self.rearm.wait() => return Won::Rearm,
                p = self.next_phone() => return Won::Phone(p),
                text = typed => match text {
                    // A typed turn (the desktop's box, `say`): a session with a silent input.
                    Some(text) => return Won::Wake(Some(text)),
                    None => {
                        info!("wake ignored: not listening");
                        if let Some(m) = &manual {
                            m.arm();
                        }
                    }
                },
                r = self.next_raised() => if let Some(r) = r {
                    let _ = r.reply.send(Delivery::Unavailable);
                },
                lease = self.next_hear() => {
                    warn!("the Hearing test asked for the mic while Ghost is not listening");
                    drop(lease); // the recorder is told no
                }
            }
            self.waiting_flag.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    //! A fake microphone stands in for `ghost.mic`: it counts the streams open and the
    //! frames it hands out, and a stage where Parakeet would be counts the frames that
    //! reach it. Never the real microphone.

    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use async_trait::async_trait;
    use ghost_api::Controller;
    use ghost_core::store::Store;
    use ghost_core::waiting::Items;
    use ghost_voice::VoiceSettings;
    use ghost_voice::frames::{Audio, Direction, Frame, Out, Stage};
    use ghost_voice::transport::MediaIn;

    use super::*;
    use crate::daemon::tests::with_recorder;
    use crate::wake::{
        FileWakeDetector, MediaFactory, SpokenWakeDetector, WakeDetector, WakeListener,
    };

    #[derive(Default)]
    struct Counts {
        open: AtomicUsize,
        opened: AtomicUsize,
        frames_out: AtomicUsize,
        frames_heard: AtomicUsize,
    }

    /// One capture stream: a frame every 20 ms, closed when dropped.
    struct FakeMic(Arc<Counts>);

    #[async_trait]
    impl MediaIn for FakeMic {
        async fn next_frame(&mut self) -> Option<Audio> {
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.0.frames_out.fetch_add(1, Ordering::SeqCst);
            Some(Audio::new(vec![100; 320], 16_000))
        }
    }

    impl Drop for FakeMic {
        fn drop(&mut self) {
            self.0.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Where Parakeet would be: counts what reaches it, hears no wake phrase.
    struct Ear(Arc<Counts>);

    #[async_trait]
    impl Stage for Ear {
        fn name(&self) -> &'static str {
            "stt"
        }
        async fn process(&mut self, frame: Frame, direction: Direction, out: &Out) {
            if matches!(frame, Frame::AudioIn(_)) {
                self.0.frames_heard.fetch_add(1, Ordering::SeqCst);
                return;
            }
            out.forward(frame, direction).await;
        }
    }

    async fn until_true(what: &str, f: impl Fn() -> bool) {
        for _ in 0..250 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{what}");
    }

    #[tokio::test]
    async fn not_listening_closes_the_capture_and_listening_opens_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (d, recorder) = with_recorder(tmp.path());
        d.mic
            .attach(Items::new(Arc::new(Store::in_memory().unwrap())))
            .unwrap();
        d.follow_mic();
        let counts = Arc::new(Counts::default());
        let media: MediaFactory = {
            let c = counts.clone();
            Arc::new(move || {
                c.open.fetch_add(1, Ordering::SeqCst);
                c.opened.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(FakeMic(c.clone())) as Box<dyn MediaIn>)
            })
        };
        let stt: crate::wake::SttFactory = {
            let c = counts.clone();
            Arc::new(move |_| Box::new(Ear(c.clone())) as Box<dyn Stage>)
        };
        let voice = Arc::new(VoiceSettings::from_vars(|_| None).unwrap());
        let listener = Arc::new(
            WakeListener::new(voice, stt, vec!["hey ghost".into()], media, 0.0, None)
                .with_vad_factory(Arc::new(|_| None))
                .with_backoff(Duration::ZERO),
        );
        let manual = FileWakeDetector::new(tmp.path().join("wake"));
        *lock(&d.manual) = Some(manual.clone());
        let wake: Arc<dyn WakeDetector> = Arc::new(SpokenWakeDetector::new(listener, manual, None));
        let passive = {
            let d = d.clone();
            tokio::spawn(async move { d.passive(wake).await })
        };

        // listening: one stream open, its frames reach the ear
        until_true("the capture opens", || {
            counts.open.load(Ordering::SeqCst) == 1
                && counts.frames_heard.load(Ordering::SeqCst) > 2
        })
        .await;
        assert_eq!(d.state.snapshot().not_listening, None);

        // not listening: the stream is closed, not ignored
        let set = Controller::set_listening(&*d, false, None).await.unwrap();
        assert_eq!(set, Some(NotListening { until: None }));
        until_true("the capture closes", || {
            counts.open.load(Ordering::SeqCst) == 0
        })
        .await;
        let (out, heard, opened) = (
            counts.frames_out.load(Ordering::SeqCst),
            counts.frames_heard.load(Ordering::SeqCst),
            counts.opened.load(Ordering::SeqCst),
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(counts.open.load(Ordering::SeqCst), 0);
        assert_eq!(counts.frames_out.load(Ordering::SeqCst), out);
        assert_eq!(counts.frames_heard.load(Ordering::SeqCst), heard);
        assert_eq!(counts.opened.load(Ordering::SeqCst), opened, "not rebuilt");
        assert_eq!(
            d.state.snapshot().not_listening,
            Some(NotListening { until: None })
        );
        assert!(!d.can_raise(), "no voice at the desk while not listening");

        // a voice session is refused; a typed turn opens one (its media has no mic)
        assert!(!Controller::wake(&*d, None, None).await);
        assert!(Controller::wake(&*d, Some("what's waiting?".into()), None).await);
        until_true("the typed turn's session runs", || {
            recorder.0.lock().unwrap().len() == 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(counts.open.load(Ordering::SeqCst), 0);
        assert_eq!(counts.opened.load(Ordering::SeqCst), opened);

        // listening again: a new stream
        Controller::set_listening(&*d, true, None).await.unwrap();
        until_true("the capture opens again", || {
            counts.open.load(Ordering::SeqCst) == 1
                && counts.frames_heard.load(Ordering::SeqCst) > heard
        })
        .await;
        assert_eq!(d.state.snapshot().not_listening, None);

        d.stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), passive)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts.open.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_timed_off_comes_back_by_itself_and_the_status_follows() {
        let tmp = tempfile::tempdir().unwrap();
        let (d, _recorder) = with_recorder(tmp.path());
        d.follow_mic();
        d.rearm.clear();
        d.mic
            .off_until(Utc::now() + chrono::Duration::milliseconds(300))
            .unwrap();
        until_true("the status says not listening", || {
            d.state.snapshot().not_listening.is_some()
        })
        .await;
        assert!(d.mic.is_off());
        until_true("listening comes back", || {
            d.state.snapshot().not_listening.is_none()
        })
        .await;
        assert!(!d.mic.is_off());
        assert!(d.rearm.is_set(), "the passive loop is told to open the mic");
        d.stop.cancel();
    }
}
