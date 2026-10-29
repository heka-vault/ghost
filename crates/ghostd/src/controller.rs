//! `ghost_api::Controller` for the desktop app (`Daemon`'s controller methods): what the
//! control API may ask of the running daemon. Every method is safe to call at any time;
//! before `run` has built the parts, each answers as a daemon with nothing behind it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use ghost_api::models::{OutcomeVia, SessionOrigin, TurnRole, TurnVia};
use ghost_api::{
    Controller, Phase, PhoneConnection, PhonePairing, PhoneView, Probe, Refusal, outcome_view,
};
use ghost_core::conversations::GHOST;
use ghost_core::models::Outcome;
use ghost_core::policy::{AutonomyPolicy, PolicyError};
use ghost_core::routes::{RouteName, RoutePolicy};
use ghost_infer::wake::parse_phrases;
use tracing::{info, warn};

use crate::config::GhostdSettings;
use crate::daemon::{Daemon, lock};
use crate::phone;

/// The phone listener's refusal, as the control API says it.
pub fn api_refusal(r: phone::Refusal) -> Refusal {
    match r {
        phone::Refusal::Busy => Refusal::Busy,
        phone::Refusal::NotReady => Refusal::NotReady,
        phone::Refusal::NoSuchLine => Refusal::NoSuchLine,
    }
}

#[async_trait]
impl Controller for Daemon {
    /// `conversation`: the line the session opens in (the desktop's Call button); Ghost
    /// when unset. A retired or unknown line is refused.
    async fn wake(&self, first_utterance: Option<String>, conversation: Option<String>) -> bool {
        if self.state.phase() != Phase::Passive {
            return false;
        }
        let Some(manual) = lock(&self.manual).clone() else {
            return false;
        };
        if !self.phone_q_empty() {
            return false; // a phone session is about to start
        }
        if let Some(slug) = conversation.as_deref().filter(|c| *c != GHOST) {
            let target = self
                .conversations()
                .and_then(|c| c.get(slug).ok().flatten());
            match target {
                Some(p) if !p.retired() => {}
                _ => return false,
            }
        }
        *lock(&self.start_in) = conversation.unwrap_or_else(|| GHOST.to_string());
        manual.trigger(first_utterance);
        true
    }

    async fn sleep(&self) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        self.state.set_ending();
        session.end().await;
        true
    }

    async fn say(&self, text: String, conversation: Option<String>) -> bool {
        if self.state.phase() == Phase::Starting {
            return false;
        }
        let Some(session) = self.session() else {
            *lock(&self.first_via) = TurnVia::Text;
            return self.wake(Some(text), conversation).await;
        };
        self.state.add_turn(TurnRole::User, &text, TurnVia::Text);
        let heard = lock(&self.heard).clone();
        if let Some(heard) = heard {
            heard("user", text.clone()).await;
        }
        session.say_text(&text).await;
        true
    }

    async fn confirm_pending(&self) -> Option<Outcome> {
        let gate = self.gate()?;
        let session = self.session()?;
        let session_id = self.state.session_id()?;
        let outcome = gate
            .confirm_pending(&session_id, "ui", None)
            .await
            .ok()
            .flatten()?;
        self.state.set_pending(None);
        self.state.outcome(outcome_view(&outcome), OutcomeVia::Ui);
        session
            .system_note(&format!(
                "[system] The operator confirmed from the desktop app. Verified result: {} \
                 Say what happened in a few words.",
                outcome.summary()
            ))
            .await;
        Some(outcome)
    }

    async fn cancel_pending(&self) -> bool {
        let (Some(gate), Some(session_id)) = (self.gate(), self.state.session_id()) else {
            return false;
        };
        if gate
            .cancel(&session_id, "cancelled from the desktop app")
            .ok()
            .flatten()
            .is_none()
        {
            return false;
        }
        self.state.set_pending(None);
        if let Some(session) = self.session() {
            session
                .system_note(
                    "[system] The operator cancelled the pending change from the desktop app. \
                     Nothing was changed. Acknowledge briefly.",
                )
                .await;
        }
        true
    }

    /// A Confirm on a change on the waiting list (ADR 0030). When it is the one the live
    /// session is waiting on, the session hears the result too.
    async fn confirm_proposal(&self, proposal_id: &str) -> Option<Outcome> {
        let gate = self.gate()?;
        let live = self
            .state
            .session_id()
            .and_then(|id| gate.pending(&id, None).ok().flatten());
        let outcome = gate
            .confirm_id(proposal_id, "ui", None)
            .await
            .ok()
            .flatten()?;
        self.state.outcome(outcome_view(&outcome), OutcomeVia::Ui);
        if live.is_some_and(|p| p.id == proposal_id) {
            self.state.set_pending(None);
            if let Some(session) = self.session() {
                session
                    .system_note(&format!(
                        "[system] The operator confirmed from the desktop app. Verified result: \
                         {} Say what happened in a few words.",
                        outcome.summary()
                    ))
                    .await;
            }
        }
        self.refresh_waiting_now();
        Some(outcome)
    }

    async fn cancel_proposal(&self, proposal_id: &str) -> bool {
        let Some(gate) = self.gate() else {
            return false;
        };
        let live = self
            .state
            .session_id()
            .and_then(|id| gate.pending(&id, None).ok().flatten());
        if gate
            .cancel_id(proposal_id, "cancelled from the waiting list")
            .ok()
            .flatten()
            .is_none()
        {
            return false;
        }
        if live.is_some_and(|p| p.id == proposal_id) {
            self.state.set_pending(None);
        }
        self.refresh_waiting_now();
        true
    }

    /// The file, and Ghost's folder with it (ADR 0035): watches.json and hooks.json
    /// edited by hand apply too.
    fn reload_policy(&self) -> Result<AutonomyPolicy, PolicyError> {
        let cfg = self.cfg();
        let policy = AutonomyPolicy::load(&cfg.policy_path)?;
        if let Some(gate) = self.gate() {
            gate.set_autonomy(policy.clone());
        }
        if let Some(routes) = self.routes() {
            match RoutePolicy::parse(policy.routes.as_ref(), &policy.source) {
                Ok(p) => routes.set_policy(p),
                Err(e) => warn!("routes: {e}"),
            }
        }
        if let Some(folder) = self.late.folder.get() {
            lock(folder).refresh(); // the watches and hooks follow
        }
        info!("policy reloaded from {}", policy.source);
        Ok(policy)
    }

    /// Apply what can change without a restart; return those keys. The wake phrases
    /// and the near-miss threshold are picked up by rebuilding the passive listener; the
    /// crossing-streams toggles (ADR 0019) by the next session, which reads them; the
    /// model connection (ADR 0037) by the next session and the next model step.
    fn apply_settings(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        let mut applied = self.apply_model(values);
        let Some(listener) = lock(&self.listener).clone() else {
            return applied;
        };
        let mut wake = Vec::new();
        if let Some(v) = values.get("GHOST_WAKE_PHRASES") {
            let spec = v.clone().unwrap_or_default();
            let phrases = parse_phrases(Some(&spec));
            lock(&self.cfg).wake_phrases = spec;
            info!(
                "wake phrases: {}",
                phrases
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            listener.set_phrases(phrases, None);
            wake.push("GHOST_WAKE_PHRASES".to_string());
        }
        if let Some(v) = values.get("GHOST_WAKE_FUZZY") {
            let fuzzy = match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                None => Some(GhostdSettings::default().wake_fuzzy),
                Some(s) => s.parse::<f64>().ok(),
            };
            if let Some(fuzzy) = fuzzy {
                lock(&self.cfg).wake_fuzzy = fuzzy;
                listener.set_phrases(listener.phrases(), Some(fuzzy));
                info!("wake fuzzy threshold: {fuzzy}");
                wake.push("GHOST_WAKE_FUZZY".to_string());
            }
        }
        if !wake.is_empty() {
            self.rearm.set();
        }
        applied.extend(wake);
        applied
    }

    fn apply_model(&self, values: &BTreeMap<String, Option<String>>) -> Vec<String> {
        Daemon::apply_model(self, values)
    }

    async fn probe_model(
        &self,
        env: ghost_api::controller::ModelEnv,
        prompt: &str,
        max_tokens: u32,
    ) -> Option<Probe> {
        Some(Daemon::probe_model(self, env, prompt, max_tokens).await)
    }

    fn routes_available(&self) -> Vec<RouteName> {
        Daemon::routes_available(self)
    }

    // --- conversations (ADR 0014) ----------------------------------------------------

    /// A click in the desktop app: the same transfer the model performs, fed in at the
    /// pipeline source so the announce, tone, voice and context frames arrive in order.
    async fn switch_conversation(&self, slug: &str) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        let Some(runtime) = &session.conversation else {
            return false;
        };
        if !runtime.switch_allowed() {
            return false;
        }
        session.switch(slug).await
    }

    async fn compact_conversation(&self, slug: &str) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        let Some(runtime) = &session.conversation else {
            return false;
        };
        if runtime.active().slug != slug {
            return false;
        }
        session
            .system_note(
                "[system] The operator asked, from the desktop app, to compact this \
                 conversation's memory now. Call change with conversations.compact and a \
                 fresh summary, then say in a few words that it is done.",
            )
            .await;
        true
    }

    fn voices(&self) -> Vec<String> {
        let Some(models) = self.models() else {
            return Vec::new();
        };
        let mut voices = models.voices.clone();
        voices.sort();
        voices
    }

    async fn sample_voice(&self, voice: &str, speed: f64, text: Option<String>) -> bool {
        let Some(models) = self.models() else {
            return false;
        };
        let line = text.unwrap_or_else(|| {
            let name = voice.split_once('_').map(|(_, n)| n).unwrap_or("");
            let mut chars = name.chars();
            let cap = match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            };
            format!("Hi, this is {cap}. This is how I sound.")
        });
        let synth = models.tts.clone();
        let voice_id = voice.to_string();
        let rendered = tokio::task::spawn_blocking(move || {
            lock(&synth).synthesize(&line, &voice_id, speed as f32)
        })
        .await;
        let samples = match rendered {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("voice sample failed: {e}");
                return false;
            }
            Err(e) => {
                warn!("voice sample failed: {e}");
                return false;
            }
        };
        let device = self.voice().output_device.clone();
        if let Err(e) =
            (self.ports.play)(samples, ghost_voice::pipeline::TTS_SAMPLE_RATE, device).await
        {
            warn!("voice sample failed: {e}");
            return false;
        }
        true
    }

    // --- the phone (ADR 0016): the control API's side ---------------------------------

    fn phone_pairing(&self) -> Option<PhonePairing> {
        lock(&self.phone_listener).as_ref().map(|l| l.new_pairing())
    }

    fn phone_view(&self) -> PhoneView {
        match lock(&self.phone_listener).as_ref() {
            Some(l) => l.view(),
            None => PhoneView {
                address: None,
                addresses: Vec::new(),
                listen: Vec::new(),
                device: None,
            },
        }
    }

    async fn forget_phone(&self) -> bool {
        let listener = lock(&self.phone_listener).clone();
        match listener {
            Some(l) => l.forget().await,
            None => false,
        }
    }

    fn admit_phone(&self) -> Option<Refusal> {
        Daemon::admit_phone(self).map(api_refusal)
    }

    /// A phone's media, as a test hands one in: an `Arc<dyn ghost_rtc::Media>`.
    async fn start_phone(
        &self,
        connection: PhoneConnection,
        conversation: Option<String>,
    ) -> Option<Refusal> {
        let Ok(media) = connection.downcast::<Arc<dyn ghost_rtc::Media>>() else {
            return Some(Refusal::NotReady);
        };
        self.queue_phone(*media, conversation.as_deref())
            .map(api_refusal)
    }

    async fn end_phone(&self) -> bool {
        Daemon::end_phone(self).await
    }

    async fn passwords_opened(&self) {
        Daemon::passwords_opened(self).await;
    }
}

impl Daemon {
    /// Whether a phone session may start now (the listener's cheap check).
    pub fn admit_phone(&self) -> Option<phone::Refusal> {
        if self.state.phase() == Phase::Starting || !self.state.models_ready() {
            return Some(phone::Refusal::NotReady);
        }
        if !self.waiting_flag.load(Ordering::SeqCst)
            || self.session().is_some()
            || self.state.phase() != Phase::Passive
            || !self.phone_q_empty()
        {
            return Some(phone::Refusal::Busy);
        }
        None
    }

    /// Hand an accepted peer to the passive loop, in `conversation` or in Ghost.
    pub fn queue_phone(
        &self,
        media: Arc<dyn ghost_rtc::Media>,
        conversation: Option<&str>,
    ) -> Option<phone::Refusal> {
        let line = conversation.unwrap_or(GHOST).to_string();
        if line != GHOST {
            let profile = self
                .conversations()
                .and_then(|c| c.get(&line).ok().flatten());
            match profile {
                Some(p) if !p.retired() => {}
                _ => return Some(phone::Refusal::NoSuchLine),
            }
        }
        if let Some(r) = self.admit_phone() {
            return Some(r);
        }
        self.phone_end.clear();
        self.put_phone(crate::session::pending_phone(media, &line));
        None
    }

    /// End the phone's session if one is running (or is about to).
    pub async fn end_phone(&self) -> bool {
        if *lock(&self.origin) == Some(SessionOrigin::Phone) || !self.phone_q_empty() {
            self.phone_end.set();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    //! `test_desk_voice.py`'s typed reply, and `test_phone.py`'s daemon tests, by name.

    use std::sync::Mutex;

    use ghost_api::models::Phase;

    use super::*;
    use crate::daemon::tests::daemon;
    use crate::phone_host::tests::FakeMedia;

    #[tokio::test]
    async fn test_a_typed_reply_answers_the_session_like_a_spoken_one() {
        // The desktop's chat input is a reply too: it reaches the digest and desk voice.
        // Without a session a typed line is the wake with those words, as the first turn.
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        let heard: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        *lock(&d.heard) = Some({
            let heard = heard.clone();
            Arc::new(move |role: &'static str, text: String| {
                let heard = heard.clone();
                Box::pin(async move {
                    heard.lock().unwrap().push((role.to_string(), text));
                })
            })
        });
        // no session yet: starting refuses, passive wakes with the words
        assert!(!d.say("yeah, go on".into(), None).await);
        let manual = crate::wake::FileWakeDetector::new(tmp.path().join("rt/wake"));
        *lock(&d.manual) = Some(manual.clone());
        d.state.set_phase(Phase::Passive, None, None);
        let waiting = {
            let m = manual.clone();
            tokio::spawn(async move { m.wait().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(d.say("yeah, go on".into(), None).await);
        assert_eq!(waiting.await.unwrap(), Some("yeah, go on".into()));
        assert_eq!(*lock(&d.first_via), TurnVia::Text);
        assert!(heard.lock().unwrap().is_empty()); // the session's turn handler hears it later
    }

    #[tokio::test]
    async fn test_daemon_admits_a_phone_only_while_passive() {
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        assert_eq!(d.admit_phone(), Some(phone::Refusal::NotReady)); // starting, models loading
        d.state.set_models_ready(true);
        d.state.set_phase(Phase::Passive, None, None);
        assert_eq!(d.admit_phone(), Some(phone::Refusal::Busy)); // the loop isn't waiting yet
        d.waiting_flag.store(true, Ordering::SeqCst);
        assert_eq!(d.admit_phone(), None);

        let conn: Arc<dyn ghost_rtc::Media> = Arc::new(FakeMedia::default());
        assert_eq!(d.queue_phone(conn.clone(), None), None);
        assert_eq!(d.admit_phone(), Some(phone::Refusal::Busy)); // one phone at a time
        let another: Arc<dyn ghost_rtc::Media> = Arc::new(FakeMedia::default());
        assert_eq!(d.queue_phone(another, None), Some(phone::Refusal::Busy));
        assert!(!d.wake(None, None).await); // the desk can't take it now
        assert!(Daemon::end_phone(&d).await && d.phone_end.is_set());
        let pending = d.take_phone().unwrap();
        assert_eq!(pending.line, "ghost"); // Ghost when the offer names no line
        assert!(std::ptr::addr_eq(
            Arc::as_ptr(&pending.media),
            Arc::as_ptr(&conn)
        ));
        let another: Arc<dyn ghost_rtc::Media> = Arc::new(FakeMedia::default());
        assert_eq!(
            d.queue_phone(another, Some("no-such-thread")),
            Some(phone::Refusal::NoSuchLine)
        );
    }

    #[tokio::test]
    async fn test_daemon_refuses_a_phone_during_a_desk_session() {
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        d.state.set_models_ready(true);
        d.waiting_flag.store(true, Ordering::SeqCst);
        d.state
            .set_phase(Phase::Listening, Some("s1"), Some(SessionOrigin::Desk));
        assert_eq!(d.admit_phone(), Some(phone::Refusal::Busy));
        assert_eq!(d.state.session_origin(), Some(SessionOrigin::Desk));
        assert!(!Daemon::end_phone(&d).await); // /end never ends a desk session
    }

    #[tokio::test]
    async fn test_daemon_phone_controller_when_listener_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        assert!(d.phone_pairing().is_none());
        assert!(d.phone_view().address.is_none());
        assert!(!d.forget_phone().await);
    }

    #[tokio::test]
    async fn the_controller_answers_before_run_as_a_daemon_with_nothing_behind_it() {
        let tmp = tempfile::tempdir().unwrap();
        let d = daemon(tmp.path());
        assert!(!d.sleep().await);
        assert!(d.confirm_pending().await.is_none());
        assert!(!d.cancel_pending().await);
        assert!(d.confirm_proposal("p1").await.is_none());
        assert!(!d.cancel_proposal("p1").await);
        assert!(!d.switch_conversation("legal").await);
        assert!(!d.compact_conversation("ghost").await);
        assert!(d.voices().is_empty());
        assert!(!d.sample_voice("af_heart", 1.0, None).await);
        assert_eq!(d.routes_available(), vec![RouteName::Digest]);
        assert!(d.apply_settings(&BTreeMap::new()).is_empty());
    }
}
