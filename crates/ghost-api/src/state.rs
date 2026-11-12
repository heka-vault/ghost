//! What the API reads: the daemon's phase, the current session, recent turns, and a
//! fan-out of events to every connected client.
//!
//! Nothing passive is ever put here: a turn is a session's, a log line is the daemon's
//! own words. The bus carries `models::Event` as JSON text.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Instant;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::models::{
    Build, Event, NotListening, OutcomeVia, OutcomeView, PendingProposal, Phase, SessionOrigin,
    Status, Turn, TurnRole, TurnVia,
};

/// How many events a slow client may fall behind before its oldest are dropped.
pub const BUS_CAPACITY: usize = 256;
/// How many calls' results are kept for `GET /calls/{id}` (ADR 0058).
pub const CALLS_KEPT: usize = 64;
/// How many of a session's turns are kept for `GET /session/turns`.
pub const TURNS_KEPT: usize = 200;

pub fn now() -> DateTime<Utc> {
    ghost_core::time::utcnow()
}

/// Each subscriber gets its own bounded queue; a slow client drops its oldest events
/// rather than stalling the daemon (tokio's `broadcast` does exactly that).
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<String>,
    /// The same events for the daemon's own parts (ADR 0048's feed). Apart from `tx`, so
    /// a part of the daemon listening is not counted as the desktop being open.
    taps: broadcast::Sender<String>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(BUS_CAPACITY)
    }
}

impl EventBus {
    pub fn new(maxsize: usize) -> Self {
        let (tx, _) = broadcast::channel(maxsize.max(1));
        let (taps, _) = broadcast::channel(maxsize.max(1));
        Self { tx, taps }
    }

    /// The events, for a part of the daemon itself. Not a subscriber: `subscribers`
    /// counts the operator's apps only.
    pub fn tap(&self) -> broadcast::Receiver<String> {
        self.taps.subscribe()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: &Event) {
        if let Ok(payload) = serde_json::to_string(event) {
            if self.taps.receiver_count() > 0 {
                let _ = self.taps.send(payload.clone());
            }
            // No subscriber is not an error: the daemon runs with the desktop closed.
            let _ = self.tx.send(payload);
        }
    }

    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }
}

/// A function that builds the full `Status`: the app installs one that reads this state
/// plus the gate's pending proposal, so one function owns the shape.
pub type StatusProvider = Arc<dyn Fn() -> Status + Send + Sync>;

#[derive(Debug, Clone)]
struct Fields {
    version: String,
    build: Build,
    wake_mode: String,
    phase: Phase,
    session_id: Option<String>,
    models_ready: bool,
    /// The apps connected now (ADR 0034).
    apps: Vec<String>,
    pending: Option<PendingProposal>,
    conversation: Option<String>,
    /// "desk" | "phone" while a session runs.
    session_origin: Option<SessionOrigin>,
    /// Going to sleep; the goodbye may still be playing.
    ending: bool,
    /// The microphone is closed (ADR 0057).
    not_listening: Option<NotListening>,
    /// Items on the waiting list still to be heard (ADR 0029).
    waiting: u64,
    turns: VecDeque<Turn>,
}

/// What the daemon knows about itself. Every mutation publishes an event; the fields are
/// read through the getters, so the lock is never held across an `await`.
pub struct DaemonState {
    fields: Mutex<Fields>,
    /// What the last calls returned, newest last (ADR 0058): memory only, never stored.
    calls: Mutex<VecDeque<crate::models::CallPreview>>,
    started: Instant,
    pub bus: EventBus,
    provider: RwLock<Option<StatusProvider>>,
}

impl std::fmt::Debug for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fields = self.lock();
        f.debug_struct("DaemonState")
            .field("phase", &fields.phase)
            .field("session_id", &fields.session_id)
            .field("conversation", &fields.conversation)
            .finish_non_exhaustive()
    }
}

impl DaemonState {
    pub fn new(version: impl Into<String>, wake_mode: impl Into<String>) -> Self {
        Self::with_build(version, wake_mode, Build::default())
    }

    pub fn with_build(
        version: impl Into<String>,
        wake_mode: impl Into<String>,
        build: Build,
    ) -> Self {
        Self {
            fields: Mutex::new(Fields {
                version: version.into(),
                build,
                wake_mode: wake_mode.into(),
                phase: Phase::Starting,
                session_id: None,
                models_ready: false,
                apps: Vec::new(),
                pending: None,
                conversation: None,
                session_origin: None,
                ending: false,
                not_listening: None,
                waiting: 0,
                turns: VecDeque::with_capacity(TURNS_KEPT),
            }),
            calls: Mutex::new(VecDeque::new()),
            started: Instant::now(),
            bus: EventBus::default(),
            provider: RwLock::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Fields> {
        self.fields.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- reads -----------------------------------------------------------------------

    pub fn uptime_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn version(&self) -> String {
        self.lock().version.clone()
    }

    pub fn build(&self) -> Build {
        self.lock().build.clone()
    }

    pub fn wake_mode(&self) -> String {
        self.lock().wake_mode.clone()
    }

    pub fn phase(&self) -> Phase {
        self.lock().phase
    }

    pub fn session_id(&self) -> Option<String> {
        self.lock().session_id.clone()
    }

    pub fn models_ready(&self) -> bool {
        self.lock().models_ready
    }

    pub fn apps(&self) -> Vec<String> {
        self.lock().apps.clone()
    }

    pub fn pending(&self) -> Option<PendingProposal> {
        self.lock().pending.clone()
    }

    pub fn conversation(&self) -> Option<String> {
        self.lock().conversation.clone()
    }

    pub fn session_origin(&self) -> Option<SessionOrigin> {
        self.lock().session_origin
    }

    pub fn ending(&self) -> bool {
        self.lock().ending
    }

    pub fn not_listening(&self) -> Option<NotListening> {
        self.lock().not_listening.clone()
    }

    pub fn waiting(&self) -> u64 {
        self.lock().waiting
    }

    pub fn turns(&self) -> Vec<Turn> {
        self.lock().turns.iter().cloned().collect()
    }

    // --- mutations, each publishing an event -------------------------------------------

    pub fn set_build(&self, build: Build) {
        self.lock().build = build;
    }

    pub fn set_apps(&self, apps: Vec<String>) {
        self.lock().apps = apps;
    }

    pub fn set_phase(&self, phase: Phase, session_id: Option<&str>, origin: Option<SessionOrigin>) {
        {
            let mut f = self.lock();
            f.phase = phase;
            if matches!(phase, Phase::Passive | Phase::Starting) {
                f.session_id = None;
                f.session_origin = None;
                f.ending = false;
                f.pending = None;
                f.conversation = None;
                f.turns.clear();
            } else {
                if let Some(id) = session_id {
                    f.session_id = Some(id.to_string());
                }
                if let Some(origin) = origin {
                    f.session_origin = Some(origin);
                }
            }
        }
        self.publish_status();
    }

    /// The session is hanging up; it ends once the goodbye has played.
    pub fn set_ending(&self) {
        let changed = {
            let mut f = self.lock();
            if f.session_id.is_some() && !f.ending {
                f.ending = true;
                true
            } else {
                false
            }
        };
        if changed {
            self.publish_status();
        }
    }

    pub fn set_models_ready(&self, ready: bool) {
        self.lock().models_ready = ready;
        self.publish_status();
    }

    pub fn add_turn(&self, role: TurnRole, text: &str, via: TurnVia) -> Option<Turn> {
        let turn = {
            let mut f = self.lock();
            let session_id = f.session_id.clone()?;
            if text.trim().is_empty() {
                return None;
            }
            let turn = Turn {
                role,
                text: text.trim().to_string(),
                session_id,
                at: now(),
                via,
            };
            if f.turns.len() >= TURNS_KEPT {
                f.turns.pop_front();
            }
            f.turns.push_back(turn.clone());
            turn
        };
        self.bus.publish(&Event::Turn { turn: turn.clone() });
        Some(turn)
    }

    pub fn set_conversation(&self, slug: Option<&str>, changed: Option<&str>) {
        let active = {
            let mut f = self.lock();
            f.conversation = if f.session_id.is_some() {
                slug.map(String::from)
            } else {
                None
            };
            f.conversation.clone()
        };
        self.bus.publish(&Event::Conversation {
            active,
            changed: changed.map(String::from),
        });
        self.publish_status();
    }

    /// An app changed (ADR 0034); with `sign_in_url` it waits for the browser.
    pub fn publish_apps(&self, changed: Option<&str>, sign_in_url: Option<&str>) {
        self.bus.publish(&Event::Apps {
            changed: changed.map(String::from),
            sign_in_url: sign_in_url.map(String::from),
        });
    }

    /// The Hearing test's recorder (`Event::Hear`, ADR 0042): only while the operator has it
    /// open.
    pub fn publish_hear(&self, hear: crate::hear::HearView, level: f32, speaking: bool) {
        self.bus.publish(&Event::Hear {
            hear,
            level,
            speaking,
        });
    }

    /// The loudness of Ghost's own voice as it plays (`Event::Voice`); never the microphone.
    pub fn publish_voice(&self, level: f32) {
        self.bus.publish(&Event::Voice { level });
    }

    /// The operator's spoken turn closed, or was taken back (`Event::Heard`, ADR 0058);
    /// dropped outside a session.
    pub fn publish_heard(&self, text: Option<&str>) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let heard = crate::models::HeardView {
            session_id,
            at: now(),
            text: text.map(|t| t.trim().to_string()).unwrap_or_default(),
            reopened: text.is_none(),
        };
        if !heard.reopened && heard.text.is_empty() {
            return;
        }
        self.bus.publish(&Event::Heard { heard });
    }

    /// Which words of the reply are heard now (`Event::Saying`, ADR 0058); dropped outside
    /// a session.
    pub fn publish_saying(&self, reply: u32, from: u32, to: u32) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        self.bus.publish(&Event::Saying {
            saying: crate::models::SayingView {
                session_id,
                reply,
                from,
                to,
            },
        });
    }

    /// Keep what a call of the live session returned (ADR 0058): `result` is what the
    /// model was given, already bounded and redacted; it is redacted again and cut here.
    pub fn keep_call(&self, id: &str, name: &str, read: bool, result: &serde_json::Value) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let text = serde_json::to_string_pretty(result).unwrap_or_default();
        let text = ghost_core::redact::redact_text(&text);
        let total = text.chars().count();
        let keep = crate::models::CALL_PREVIEW_CHARS;
        let preview: String = text.chars().take(keep).collect();
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.retain(|c| c.id != id);
        if calls.len() >= CALLS_KEPT {
            calls.pop_front();
        }
        calls.push_back(crate::models::CallPreview {
            id: id.to_string(),
            session_id,
            name: name.to_string(),
            read,
            at: now(),
            preview,
            dropped: total.saturating_sub(keep) as u64,
        });
    }

    /// A call's preview, while it is still kept.
    pub fn call(&self, id: &str) -> Option<crate::models::CallPreview> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// What the live session is doing (`Event::Activity`, ADR 0055); dropped outside one.
    pub fn publish_activity(&self, mut activity: crate::models::ActivityView) {
        let Some(id) = self.session_id() else {
            return;
        };
        activity.session_id = id;
        self.bus.publish(&Event::Activity { activity });
    }

    /// A run started or has ended (`Event::Run`, ADR 0060).
    pub fn publish_run(
        &self,
        run: crate::schedules::ScheduleRunInfo,
        phase: crate::models::RunPhase,
    ) {
        self.bus.publish(&Event::Run { run, phase });
    }

    /// One call's counts and sizes (`Event::Reach`, ADR 0060). Only built when someone
    /// is listening.
    pub fn publish_reach(&self, row: &ghost_core::reach::ReachRow) {
        if self.bus.subscribers() > 0 {
            self.bus.publish(&Event::Reach { reach: row.into() });
        }
    }

    pub fn set_pending(&self, pending: Option<PendingProposal>) {
        self.lock().pending = pending.clone();
        self.bus.publish(&Event::Pending { pending });
        self.publish_status();
    }

    /// The microphone closed or opened (ADR 0057); the apps and the phone follow the status.
    pub fn set_not_listening(&self, not_listening: Option<NotListening>) {
        {
            let mut f = self.lock();
            if f.not_listening == not_listening {
                return;
            }
            f.not_listening = not_listening;
        }
        self.publish_status();
    }

    /// The waiting list changed; the apps re-read it.
    pub fn set_waiting(&self, count: u64) {
        self.lock().waiting = count;
        self.bus.publish(&Event::Waiting { count });
        self.publish_status();
    }

    pub fn outcome(&self, view: OutcomeView, via: OutcomeVia) {
        self.bus.publish(&Event::Outcome { outcome: view, via });
    }

    /// The daemon's own words only, and only while someone listens.
    pub fn log(&self, level: &str, message: &str) {
        if self.bus.subscribers() > 0 {
            self.bus.publish(&Event::Log {
                level: level.to_string(),
                message: message.to_string(),
                at: now(),
            });
        }
    }

    // --- status snapshot ---------------------------------------------------------------

    /// The current `Status`, built by the provider the app installed; the bare fields
    /// when none is (a daemon with no routes yet).
    pub fn snapshot(&self) -> Status {
        let provider = self
            .provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match provider {
            Some(p) => p(),
            None => self.bare_status(),
        }
    }

    /// `Status` from this state alone: no pending proposal, the apps as last set.
    pub fn bare_status(&self) -> Status {
        let f = self.lock();
        Status {
            phase: f.phase,
            session_id: f.session_id.clone(),
            models_ready: f.models_ready,
            uptime_secs: self.uptime_secs(),
            version: f.version.clone(),
            build: f.build.clone(),
            api_version: crate::models::API_VERSION.to_string(),
            wake_mode: f.wake_mode.clone(),
            apps: f.apps.clone(),
            pending: f.pending.clone(),
            conversation: f.conversation.clone(),
            session_origin: f.session_origin,
            ending: f.ending,
            waiting: f.waiting,
            not_listening: f.not_listening.clone(),
        }
    }

    /// The app installs a provider that builds a full `Status` from this state plus the
    /// gate's pending proposal, so one function owns the shape.
    pub fn set_status_provider(&self, provider: StatusProvider) {
        *self.provider.write().unwrap_or_else(|e| e.into_inner()) = Some(provider);
    }

    fn publish_status(&self) {
        let provider = self
            .provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(p) = provider {
            self.bus.publish(&Event::Status { status: p() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ending_is_set_in_a_session_and_cleared_after() {
        let st = DaemonState::new("test", "file");
        st.set_ending();
        assert!(!st.ending()); // no session, nothing to hang up
        st.set_phase(Phase::Listening, Some("s1"), Some(SessionOrigin::Phone));
        st.set_ending();
        assert!(st.ending());
        st.set_phase(Phase::Passive, None, None);
        assert!(!st.ending());
    }

    #[test]
    fn a_slow_subscriber_loses_its_oldest_events_not_the_daemon() {
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();
        for n in 0..5 {
            bus.publish(&Event::Waiting { count: n });
        }
        // The first read reports the lag; the next ones are the newest events.
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(3))
        ));
        let v: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(v["count"], 3);
    }

    #[test]
    fn a_turn_needs_a_session_and_words() {
        let st = DaemonState::new("test", "file");
        assert!(st.add_turn(TurnRole::User, "hi", TurnVia::Text).is_none());
        st.set_phase(Phase::Listening, Some("s1"), None);
        assert!(st.add_turn(TurnRole::User, "   ", TurnVia::Text).is_none());
        let t = st.add_turn(TurnRole::User, " hi ", TurnVia::Text).unwrap();
        assert_eq!(t.text, "hi");
        assert_eq!(st.turns().len(), 1);
        st.set_phase(Phase::Passive, None, None);
        assert!(st.turns().is_empty());
    }
}
