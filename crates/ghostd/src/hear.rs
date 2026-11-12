//! The Hearing test's recorder (ADR 0042): the daemon side of `ghost_api::hear`.
//!
//! Opening it takes the microphone from the passive loop the way a phone call does: the loop
//! lets its wake listener go and waits, in the `recording` phase, until the recorder gives
//! the mic back. Ghost is not listening for its name meanwhile, and a session in progress
//! refuses it. While open, the recorder captures `ghost.mic` and the raw array beneath it
//! (`~/.config/ghost/echo-cancel.conf`'s capture target) into tapes that keep half a second
//! before a take starts, runs the session's VAD live for the level meter and the takes that
//! end on silence, and publishes a `hear` event about twenty times a second.
//!
//! A take ends on the operator's release (held), two seconds after the last word (across the
//! room), or after the plan's seconds (silent). It is then checked (one Parakeet read of the
//! whole take, lined up against the line) and saved with its manifest line. The check goes
//! back to the screen and is never written. Score runs use the daemon's loaded models while
//! the recorder holds the mic, so they never compete with a session.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ghost_api::hear::{
    HearCheck, HearError, HearFact, HearKind, HearPart, HearPlan, HearPlanView, HearPort,
    HearReport, HearResult, HearRow, HearRun, HearRunInfo, HearRunWer, HearScoring, HearSection,
    HearSession, HearSessionSummary, HearState, HearTable, HearTake, HearTakeState, HearVerdict,
    HearView, HearWarning, HearWord, HearWordOp, TakeEnding,
};
use ghost_api::models::Phase;
use ghost_audio::{Capture, CaptureConfig};
use ghost_voice::perf::hear::plan::{self, Block, Channel, Kind, Plan, PlanTake};
use ghost_voice::perf::hear::report::{self, Part, Report};
use ghost_voice::perf::hear::score::Verdict;
use ghost_voice::perf::hear::store::{Recorded, RunInfo, Store};
use ghost_voice::perf::hear::take::{
    self, Check, Ending, POST_SECS, QuietEnd, Tape, Vad, Warning, WordOp,
};
use ghost_voice::perf::hear::{MODES, Results, check_take, score_session};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::daemon::{Daemon, lock};

/// A recorder left open with no take for this long gives the mic back.
pub const IDLE: Duration = Duration::from_secs(600);
/// How long the passive loop has to let the mic go.
const LEASE_WAIT: Duration = Duration::from_secs(5);
/// Countdowns: a take that ends on silence, a silent take, and the walk across the room.
const COUNTDOWN_SECS: u32 = 3;
const SILENT_COUNTDOWN_SECS: u32 = 2;
const WALK_SECS: u32 = 5;
/// The `hear` event's period, and the meter's window.
const EVENT_SAMPLES: usize = 800; // 50 ms at 16 kHz
const RAW_CHANNELS: u16 = 2;
/// The pass rule's close budget, for a report in the app (`make perf-hear`'s default).
const CLOSE_BUDGET_SECS: f64 = 0.15;

/// The passive loop's side of a lease: say the mic is free, then wait to get it back.
pub struct Lease {
    pub granted: oneshot::Sender<()>,
    pub release: oneshot::Receiver<()>,
}

/// The raw array under `ghost.mic`: the `node.target` inside `capture.props` of the
/// echo-cancel config (the playback block has one too: the speakers).
pub fn capture_target(conf: &str) -> Option<String> {
    let text: String = conf
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let start = text.find("capture.props")?;
    let rest = &text[start..];
    let open = rest.find('{')?;
    let close = rest[open..].find('}')? + open;
    let block = &rest[open + 1..close];
    let at = block.find("node.target")?;
    let value = block[at + "node.target".len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let value = value.lines().next()?.trim();
    let value = value.trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A take going: what it is and how it ends.
struct Run {
    session: String,
    block: Block,
    take: PlanTake,
    ending: Ending,
    quiet: QuietEnd,
}

/// What the capture loops share: the tapes and the take going.
struct Rec {
    aec: Tape,
    raw: Option<Tape>,
    run: Option<Run>,
    state: HearState,
    countdown_until: Option<Instant>,
    level: f32,
    speaking: bool,
    last_take: Instant,
    /// Waiting for the take's check (`stop_take`).
    waiters: Vec<oneshot::Sender<HearResult<HearCheck>>>,
}

impl Rec {
    fn new(raw: bool) -> Self {
        Self {
            aec: Tape::new(1),
            raw: raw.then(|| Tape::new(usize::from(RAW_CHANNELS))),
            run: None,
            state: HearState::Ready,
            countdown_until: None,
            level: 0.0,
            speaking: false,
            last_take: Instant::now(),
            waiters: Vec::new(),
        }
    }

    fn start_tapes(&mut self) {
        self.aec.start();
        if let Some(r) = &mut self.raw {
            r.start();
        }
        self.state = HearState::Recording;
        self.countdown_until = None;
    }

    fn stop_tapes(&mut self, more: f64) {
        self.aec.stop(more);
        if let Some(r) = &mut self.raw {
            r.stop(more);
        }
    }

    fn cancel_tapes(&mut self) {
        self.aec.cancel();
        if let Some(r) = &mut self.raw {
            r.cancel();
        }
    }

    fn busy(&self) -> bool {
        !matches!(self.state, HearState::Ready)
    }
}

/// A finished take on its way to its check.
struct Finished {
    run: Run,
    aec: Vec<i16>,
    raw: Option<Vec<i16>>,
}

struct Open {
    session: String,
    raw_device: Option<String>,
    rec: Arc<Mutex<Rec>>,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    open: Option<Open>,
    opening: bool,
    /// Gives the mic back to the passive loop.
    release: Option<oneshot::Sender<()>>,
    last_check: Option<HearCheck>,
    scoring: Option<HearScoring>,
    playing: Option<JoinHandle<()>>,
    problem: Option<String>,
}

pub struct HearDesk {
    d: Weak<Daemon>,
    store: Store,
    /// The echo-cancel config the raw array is read from.
    echo_conf: PathBuf,
    inner: Mutex<Inner>,
    lease_q: Mutex<Option<Lease>>,
    lease_notify: Notify,
}

fn conflict(m: impl Into<String>) -> HearError {
    HearError::Conflict(m.into())
}

fn failed(e: impl std::fmt::Display) -> HearError {
    HearError::Failed(e.to_string())
}

impl HearDesk {
    pub fn new(d: Weak<Daemon>, data_dir: &Path, config_dir: &Path) -> Self {
        Self {
            d,
            store: Store::under(data_dir),
            echo_conf: config_dir.join("echo-cancel.conf"),
            inner: Mutex::new(Inner::default()),
            lease_q: Mutex::new(None),
            lease_notify: Notify::new(),
        }
    }

    fn daemon(&self) -> HearResult<Arc<Daemon>> {
        self.d
            .upgrade()
            .ok_or_else(|| HearError::Unavailable("Ghost is stopping".into()))
    }

    /// The passive loop waits on this beside the wake: the recorder asking for the mic.
    pub async fn next_lease(&self) -> Lease {
        loop {
            let notified = self.lease_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(l) = lock(&self.lease_q).take() {
                return l;
            }
            notified.await;
        }
    }

    fn publish(&self) {
        let Some(d) = self.d.upgrade() else {
            return;
        };
        let (level, speaking) = {
            let inner = lock(&self.inner);
            inner.open.as_ref().map_or((0.0, false), |o| {
                let r = lock(&o.rec);
                (r.level, r.speaking)
            })
        };
        d.state.publish_hear(self.view(), level, speaking);
    }

    fn set_problem(&self, p: Option<String>) {
        lock(&self.inner).problem = p;
    }

    fn plan_take(&self, id: &str) -> HearResult<(Block, PlanTake)> {
        let (p, _) = self.store.plan().map_err(failed)?;
        plan::takes(&p)
            .find(|(_, t)| t.id == id)
            .map(|(b, t)| (b.clone(), t.clone()))
            .ok_or_else(|| HearError::NotFound(format!("no take {id} in the take list")))
    }

    fn rec(&self) -> HearResult<(String, Arc<Mutex<Rec>>)> {
        let inner = lock(&self.inner);
        let open = inner
            .open
            .as_ref()
            .ok_or_else(|| conflict("the recorder is not open"))?;
        Ok((open.session.clone(), open.rec.clone()))
    }

    /// The raw array, if the echo canceller names one.
    fn raw_node(&self) -> Option<String> {
        std::fs::read_to_string(&self.echo_conf)
            .ok()
            .and_then(|c| capture_target(&c))
    }

    async fn open_capture(node: String, channels: u16) -> Result<Capture, String> {
        tokio::task::spawn_blocking(move || {
            Capture::open(CaptureConfig {
                node: Some(node),
                channels,
                ring_seconds: 2.0,
                name: "ghost-hear".into(),
                description: "Ghost (Hearing test)".into(),
                ..CaptureConfig::default()
            })
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// The `ghost.mic` loop: tapes, meter, VAD, countdowns and endings.
    async fn aec_loop(self: Arc<Self>, mut cap: Capture, rec: Arc<Mutex<Rec>>, mut vad: Vad) {
        let mut window: Vec<i16> = Vec::with_capacity(EVENT_SAMPLES * 2);
        let mut speaking = false;
        while let Some(frame) = cap.next_frame().await {
            let secs = frame.samples.len() as f64 / f64::from(take::RATE);
            window.extend_from_slice(&frame.samples);
            if window.len() >= 512 {
                speaking = vad.speaking(&window[..window.len() / 512 * 512]);
            }
            let publish = window.len() >= EVENT_SAMPLES;
            let mut finished = None;
            let mut idle = false;
            {
                let mut r = lock(&rec);
                r.aec.push(&frame.samples);
                r.speaking = speaking;
                if publish {
                    r.level = ghost_voice::level::level_of(&window);
                }
                if r.state == HearState::Countdown
                    && r.countdown_until.is_some_and(|t| Instant::now() >= t)
                {
                    r.start_tapes();
                }
                if matches!(r.state, HearState::Recording | HearState::Stopping) {
                    let mut stop = false;
                    let elapsed = r.aec.secs();
                    if let Some(run) = &mut r.run {
                        match run.ending {
                            Ending::Quiet => stop = run.quiet.feed(speaking, secs),
                            Ending::Timed { seconds } => {
                                stop = elapsed >= seconds + take::PRE_SECS;
                            }
                            Ending::Hold => {}
                        }
                    }
                    if stop {
                        r.stop_tapes(0.0);
                    }
                    if r.aec.done() {
                        let aec = r.aec.finish();
                        let raw = r.raw.as_mut().map(Tape::finish);
                        r.state = HearState::Checking;
                        if let Some(run) = r.run.take() {
                            finished = Some(Finished { run, aec, raw });
                        }
                    }
                }
                if r.state == HearState::Ready && r.last_take.elapsed() >= IDLE {
                    idle = true;
                }
            }
            if publish {
                window.clear();
                self.publish();
            }
            if let Some(f) = finished {
                tokio::spawn(self.clone().finish(f, rec.clone()));
            }
            if idle {
                info!(
                    "hear: no take for {} min; the mic goes back to Ghost",
                    IDLE.as_secs() / 60
                );
                let me = self.clone();
                tokio::spawn(async move {
                    me.close_now().await;
                });
                return;
            }
        }
        warn!("hear: ghost.mic stopped; the recorder closes");
        self.set_problem(Some("ghost.mic stopped while the recorder was open".into()));
        let me = self.clone();
        tokio::spawn(async move {
            me.close_now().await;
        });
    }

    async fn raw_loop(mut cap: Capture, rec: Arc<Mutex<Rec>>) {
        while let Some(frame) = cap.next_frame().await {
            if let Some(raw) = &mut lock(&rec).raw {
                raw.push(&frame.samples);
            }
        }
    }

    /// Check a finished take, keep it, and hand the check to whoever waits.
    async fn finish(self: Arc<Self>, f: Finished, rec: Arc<Mutex<Rec>>) {
        let result = self.check_and_save(&f).await;
        match &result {
            Ok(c) => {
                lock(&self.inner).last_check = Some(c.clone());
                self.set_problem(None);
            }
            Err(e) => {
                warn!("hear: take {} not kept: {e:?}", f.run.take.id);
                self.set_problem(Some(format!("take {} could not be kept", f.run.take.id)));
            }
        }
        let waiters = {
            let mut r = lock(&rec);
            r.state = HearState::Ready;
            r.last_take = Instant::now();
            std::mem::take(&mut r.waiters)
        };
        for w in waiters {
            let _ = w.send(result.clone());
        }
        self.publish();
    }

    async fn check_and_save(&self, f: &Finished) -> HearResult<HearCheck> {
        let d = self.daemon()?;
        let models = d
            .models()
            .ok_or_else(|| HearError::Unavailable("the models are not loaded".into()))?;
        let settings = d.voice();
        let check = check_take(
            &models,
            &settings,
            f.run.take.kind,
            &f.run.take.text,
            &f.aec,
        )
        .await
        .map_err(failed)?;
        self.store
            .save_take(
                &f.run.session,
                &Recorded {
                    block: &f.run.block,
                    take: &f.run.take,
                    aec: &f.aec,
                    raw: f.raw.as_deref().map(|r| (r, RAW_CHANNELS)),
                    check: &check,
                    ending: f.run.ending,
                },
            )
            .map_err(failed)?;
        Ok(check_view(&f.run.session, &f.run.take.id, &check))
    }

    /// Close, from inside: the idle timer or a stopped stream.
    async fn close_now(self: Arc<Self>) {
        HearPort::close(&*self).await;
    }

    fn drop_empty_session(&self, name: &str) {
        if self.store.entries(name).is_ok_and(|e| e.is_empty())
            && let Ok(dir) = self.store.session_dir(name)
        {
            let _ = std::fs::remove_dir(dir); // only if empty
        }
    }

    fn give_mic_back(&self) {
        if let Some(r) = lock(&self.inner).release.take() {
            let _ = r.send(());
        }
    }
}

// --- conversions to the API's types ---------------------------------------------------------

fn warning_kind(w: &Warning) -> String {
    serde_json::to_value(w)
        .ok()
        .and_then(|v| v["kind"].as_str().map(String::from))
        .unwrap_or_default()
}

fn check_view(session: &str, id: &str, c: &Check) -> HearCheck {
    HearCheck {
        session: session.to_string(),
        take: id.to_string(),
        seconds: c.seconds,
        rms: c.rms,
        peak: c.peak,
        speech_start: c.speech.map(|s| s.0),
        speech_end: c.speech.map(|s| s.1),
        heard: c.heard.clone(),
        words: c
            .words
            .iter()
            .map(|w| HearWord {
                op: match w.op {
                    WordOp::Same => HearWordOp::Same,
                    WordOp::Misheard => HearWordOp::Misheard,
                    WordOp::Missed => HearWordOp::Missed,
                    WordOp::Extra => HearWordOp::Extra,
                },
                said: w.said.clone(),
                heard: w.heard.clone(),
            })
            .collect(),
        matched: c.matched,
        said: c.said,
        warnings: c
            .warnings
            .iter()
            .map(|w| HearWarning {
                kind: warning_kind(w),
                text: w.text(),
                redo: w.redo(),
            })
            .collect(),
    }
}

fn plan_view(p: &Plan) -> HearResult<HearPlan> {
    serde_json::to_value(p)
        .and_then(serde_json::from_value)
        .map_err(failed)
}

fn plan_of(p: HearPlan) -> HearResult<Plan> {
    serde_json::to_value(p)
        .and_then(serde_json::from_value)
        .map_err(|e| HearError::Invalid(e.to_string()))
}

fn kind_view(k: Kind) -> HearKind {
    match k {
        Kind::Speech => HearKind::Speech,
        Kind::Reply => HearKind::Reply,
        Kind::Silence => HearKind::Silence,
    }
}

fn run_view(r: RunInfo) -> HearRunInfo {
    let w = |m: &str| r.wer.get(m).copied().flatten();
    HearRunInfo {
        wer: HearRunWer {
            stage: w("stage"),
            turn: w("turn"),
            file: w("file"),
        },
        id: r.id,
        session: r.session,
        channel: r.channel,
        created: r.created,
        commit: r.commit,
        takes: r.takes,
        note: r.note,
    }
}

fn facts(f: &[(String, String)]) -> Vec<HearFact> {
    f.iter()
        .map(|(name, value)| HearFact {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

fn report_view(r: Report) -> HearReport {
    HearReport {
        meta: facts(&r.meta),
        intro: r.intro,
        pass: r.pass,
        sections: r
            .sections
            .into_iter()
            .map(|s| HearSection {
                level: s.level,
                title: s.title,
                parts: s
                    .parts
                    .into_iter()
                    .map(|p| match p {
                        Part::Text { text } => HearPart::Text { text },
                        Part::Facts { facts: f } => HearPart::Facts { facts: facts(&f) },
                        Part::Pass { pass, text } => HearPart::Pass { pass, text },
                        Part::Table(t) => HearPart::Table(HearTable {
                            columns: t.columns,
                            rows: t
                                .rows
                                .into_iter()
                                .map(|row| HearRow {
                                    cells: row.cells,
                                    take: row.take,
                                    verdict: row.verdict.map(|v| match v {
                                        Verdict::Better => HearVerdict::Better,
                                        Verdict::Same => HearVerdict::Same,
                                        Verdict::Worse => HearVerdict::Worse,
                                    }),
                                })
                                .collect(),
                        }),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn channel_of(c: &str) -> HearResult<Channel> {
    Channel::parse(c).map_err(HearError::Invalid)
}

// --- the port -------------------------------------------------------------------------------

#[async_trait]
impl HearPort for HearDesk {
    fn view(&self) -> HearView {
        let inner = lock(&self.inner);
        let unavailable = self.d.upgrade().and_then(|d| {
            if !d.state.models_ready() {
                Some("Ghost is still loading its models.".to_string())
            } else if inner.open.is_none()
                && !inner.opening
                && !matches!(d.state.phase(), Phase::Passive)
            {
                Some("Ghost is in a session: end it first.".to_string())
            } else {
                None
            }
        });
        let mut v = HearView::closed(unavailable);
        v.scoring = inner.scoring.clone();
        v.playing = inner.playing.as_ref().is_some_and(|h| !h.is_finished());
        v.problem = inner.problem.clone();
        v.checked = inner.last_check.as_ref().map(|c| c.take.clone());
        if inner.opening {
            v.state = HearState::Opening;
        }
        if let Some(open) = &inner.open {
            let r = lock(&open.rec);
            v.state = r.state;
            v.session = Some(open.session.clone());
            v.raw_device = open.raw_device.clone();
            v.take = r.run.as_ref().map(|run| run.take.id.clone());
            v.ending = r.run.as_ref().map(|run| match run.ending {
                Ending::Hold => TakeEnding::Hold,
                Ending::Quiet => TakeEnding::Quiet,
                Ending::Timed { .. } => TakeEnding::Timed,
            });
            v.countdown = r.countdown_until.map(|t| {
                t.saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    .ceil() as u32
            });
            v.elapsed = r.aec.secs();
        }
        v
    }

    async fn open(&self, session: Option<String>, new: bool) -> HearResult<HearView> {
        let d = self.daemon()?;
        if let Some(s) = &session {
            let dir = self
                .store
                .session_dir(s)
                .map_err(|e| HearError::Invalid(e.to_string()))?;
            if !dir.is_dir() {
                return Err(HearError::NotFound(format!("no session {s}")));
            }
        }
        {
            let mut inner = lock(&self.inner);
            if let Some(open) = &mut inner.open {
                // open already: carry another session on, or start one, between takes
                let switch = match &session {
                    Some(s) => s != &open.session,
                    None => new,
                };
                let mut old = None;
                if switch {
                    if lock(&open.rec).busy() {
                        return Err(conflict("a take is going"));
                    }
                    let next = match session {
                        Some(s) => s,
                        None => self
                            .store
                            .new_session(chrono::Local::now().date_naive())
                            .map_err(failed)?,
                    };
                    old = Some(std::mem::replace(&mut open.session, next));
                    inner.last_check = None;
                }
                drop(inner); // before anything below locks it again
                if let Some(old) = old {
                    self.drop_empty_session(&old);
                }
                self.publish();
                return Ok(self.view());
            }
            if inner.opening {
                return Err(conflict("the recorder is opening"));
            }
            let Some(models) = d.models().filter(|_| d.state.models_ready()) else {
                return Err(HearError::Unavailable(
                    "Ghost is still loading its models.".into(),
                ));
            };
            drop(models);
            if d.state.phase() != Phase::Passive {
                return Err(HearError::Unavailable(
                    "Ghost is in a session: end it first.".into(),
                ));
            }
            inner.opening = true;
        }
        self.publish();
        let result = self.take_mic_and_open(&d, session, new).await;
        lock(&self.inner).opening = false;
        if let Err(e) = &result {
            self.give_mic_back();
            warn!("hear: the recorder did not open: {e:?}");
        }
        self.publish();
        result.map(|()| self.view())
    }

    async fn close(&self) -> HearView {
        let (open, playing, scoring) = {
            let mut inner = lock(&self.inner);
            (
                inner.open.take(),
                inner.playing.take(),
                inner.scoring.is_some(),
            )
        };
        if let Some(h) = playing {
            h.abort();
        }
        if let Some(open) = open {
            for t in &open.tasks {
                t.abort();
            }
            let waiters = std::mem::take(&mut lock(&open.rec).waiters);
            for w in waiters {
                let _ = w.send(Err(conflict("the recorder closed")));
            }
            self.drop_empty_session(&open.session);
            info!("hear: the recorder closed");
        }
        lock(&self.inner).last_check = None;
        if !scoring {
            self.give_mic_back(); // a run going keeps it until it ends
        }
        self.publish();
        self.view()
    }

    async fn start_take(&self, id: &str, ending: TakeEnding) -> HearResult<HearView> {
        let (session, rec) = self.rec()?;
        if lock(&self.inner).scoring.is_some() {
            return Err(conflict("a score run is going: wait for it to finish"));
        }
        if let Some(h) = lock(&self.inner).playing.take() {
            h.abort(); // a take never records Ghost playing one back
        }
        let (block, take) = self.plan_take(id)?;
        let silent = take.kind == Kind::Silence;
        let ending = match (ending, silent) {
            (TakeEnding::Timed, true) => Ending::Timed {
                seconds: take.seconds.unwrap_or(6.0),
            },
            (_, true) => {
                return Err(HearError::Invalid(format!(
                    "take {id} is silent: it is timed"
                )));
            }
            (TakeEnding::Timed, false) => {
                return Err(HearError::Invalid(format!(
                    "take {id} has a line: hold the key, or let it end on silence"
                )));
            }
            (TakeEnding::Hold, false) => Ending::Hold,
            (TakeEnding::Quiet, false) => Ending::Quiet,
        };
        {
            let mut r = lock(&rec);
            if r.busy() {
                return Err(conflict("a take is going"));
            }
            r.run = Some(Run {
                session,
                block: block.clone(),
                take,
                ending,
                quiet: QuietEnd::default(),
            });
            match ending {
                Ending::Hold => r.start_tapes(),
                Ending::Quiet => {
                    let secs = COUNTDOWN_SECS + if block.away { WALK_SECS } else { 0 };
                    r.state = HearState::Countdown;
                    r.countdown_until = Some(Instant::now() + Duration::from_secs(secs.into()));
                }
                Ending::Timed { .. } => {
                    r.state = HearState::Countdown;
                    r.countdown_until =
                        Some(Instant::now() + Duration::from_secs(SILENT_COUNTDOWN_SECS.into()));
                }
            }
        }
        self.publish();
        Ok(self.view())
    }

    async fn stop_take(&self) -> HearResult<HearCheck> {
        let (_, rec) = self.rec()?;
        let (tx, rx) = oneshot::channel();
        {
            let mut r = lock(&rec);
            match r.state {
                HearState::Recording => {
                    r.stop_tapes(POST_SECS);
                    r.state = HearState::Stopping;
                }
                HearState::Stopping | HearState::Checking => {}
                HearState::Countdown => {
                    return Err(conflict("the take has not started: cancel it instead"));
                }
                _ => return Err(conflict("no take is going")),
            }
            r.waiters.push(tx);
        }
        self.publish();
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(result)) => result,
            _ => Err(HearError::Failed(
                "the take's check did not come back".into(),
            )),
        }
    }

    async fn cancel_take(&self) -> HearResult<HearView> {
        let (_, rec) = self.rec()?;
        {
            let mut r = lock(&rec);
            match r.state {
                HearState::Countdown | HearState::Recording | HearState::Stopping => {
                    r.cancel_tapes();
                    r.run = None;
                    r.countdown_until = None;
                    r.state = HearState::Ready;
                    r.last_take = Instant::now();
                }
                HearState::Checking => return Err(conflict("the take is being checked")),
                _ => return Err(conflict("no take is going")),
            }
        }
        self.publish();
        Ok(self.view())
    }

    fn last_check(&self) -> Option<HearCheck> {
        lock(&self.inner).last_check.clone()
    }

    async fn skip_take(&self, id: &str) -> HearResult<HearSession> {
        let (session, rec) = self.rec()?;
        if lock(&rec).run.as_ref().is_some_and(|r| r.take.id == id) {
            return Err(conflict("that take is going"));
        }
        let (block, take) = self.plan_take(id)?;
        self.store.skip(&session, &block, &take).map_err(failed)?;
        self.session(&session)
    }

    fn plan(&self) -> HearResult<HearPlanView> {
        let (p, edited) = self.store.plan().map_err(failed)?;
        Ok(HearPlanView {
            plan: plan_view(&p)?,
            edited,
        })
    }

    fn save_plan(&self, p: HearPlan) -> HearResult<HearPlanView> {
        let p = plan_of(p)?;
        self.store.save_plan(&p).map_err(HearError::Invalid)?;
        self.plan()
    }

    fn reset_plan(&self) -> HearResult<HearPlanView> {
        self.store.reset_plan().map_err(failed)?;
        self.plan()
    }

    fn sessions(&self) -> Vec<HearSessionSummary> {
        self.store
            .sessions()
            .into_iter()
            .map(|s| HearSessionSummary {
                name: s.name,
                recorded: s.recorded,
                skipped: s.skipped,
                raw: s.raw,
                flagged: s.flagged,
                seconds: s.seconds,
                updated: s.updated,
            })
            .collect()
    }

    fn session(&self, name: &str) -> HearResult<HearSession> {
        let entries = self
            .store
            .entries(name)
            .map_err(|e| HearError::NotFound(e.to_string()))?;
        Ok(HearSession {
            name: name.to_string(),
            takes: entries
                .into_iter()
                .map(|e| HearTake {
                    state: if e.skipped {
                        HearTakeState::Skipped
                    } else {
                        HearTakeState::Recorded
                    },
                    kind: kind_view(e.kind),
                    raw: e.raw.is_some(),
                    speech_start: e.speech.map(|s| s.0),
                    speech_end: e.speech.map(|s| s.1),
                    id: e.id,
                    block: e.block,
                    text: e.text,
                    seconds: e.seconds,
                    peak: e.peak,
                    ending: e.ending,
                    warnings: e.warnings,
                    time: e.time,
                })
                .collect(),
        })
    }

    fn delete_session(&self, name: &str) -> HearResult<()> {
        {
            let inner = lock(&self.inner);
            if inner.open.as_ref().is_some_and(|o| o.session == name) {
                return Err(conflict("the session is open in the recorder"));
            }
            if inner.scoring.as_ref().is_some_and(|s| s.session == name) {
                return Err(conflict("the session is being scored"));
            }
        }
        self.store
            .delete_session(name)
            .map_err(|e| HearError::NotFound(e.to_string()))
    }

    async fn play(&self, session: &str, id: &str, channel: &str) -> HearResult<()> {
        let d = self.daemon()?;
        if let Ok((_, rec)) = self.rec()
            && lock(&rec).busy()
        {
            return Err(conflict("not while a take is going"));
        }
        let ch = channel_of(channel)?;
        let path = self
            .store
            .take_file(session, id, ch)
            .map_err(|e| HearError::NotFound(e.to_string()))?
            .ok_or_else(|| HearError::NotFound(format!("take {id} has no {channel} recording")))?;
        let pcm = plan::read_mono(&path).map_err(failed)?;
        let play = d.ports.play.clone();
        let device = d.voice().output_device.clone();
        let weak = self.d.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = play(pcm, take::RATE, device).await {
                warn!("hear: playing a take failed: {e}");
            }
            if let Some(d) = weak.upgrade()
                && let Some(desk) = d.hear()
            {
                lock(&desk.inner).playing = None;
                desk.publish();
            }
        });
        if let Some(old) = lock(&self.inner).playing.replace(handle) {
            old.abort();
        }
        self.publish();
        Ok(())
    }

    async fn stop_playing(&self) {
        if let Some(h) = lock(&self.inner).playing.take() {
            h.abort();
        }
        self.publish();
    }

    async fn start_run(&self, session: &str, channel: &str) -> HearResult<HearRunInfo> {
        let d = self.daemon()?;
        let ch = channel_of(channel)?;
        let dir = self
            .store
            .session_dir(session)
            .map_err(|e| HearError::Invalid(e.to_string()))?;
        if !dir.join(plan::MANIFEST).is_file() {
            return Err(HearError::NotFound(format!(
                "session {session} has no takes"
            )));
        }
        let models = d
            .models()
            .ok_or_else(|| HearError::Unavailable("the models are not loaded".into()))?;
        let build = d.state.build();
        let commit = match build.commit {
            Some(c) if build.dirty => format!("{c} (with changes)"),
            Some(c) => c,
            None => "unknown".into(),
        };
        let now = chrono::Local::now();
        let id = self.store.new_run_id(now.naive_local()).map_err(failed)?;
        let mut info = RunInfo {
            id: id.clone(),
            session: session.to_string(),
            channel: channel.to_string(),
            created: now.format("%Y-%m-%d %H:%M").to_string(),
            commit: commit.clone(),
            takes: 0,
            wer: Default::default(),
            note: String::new(),
        };
        {
            let mut inner = lock(&self.inner);
            let open = inner
                .open
                .as_ref()
                .ok_or_else(|| conflict("open the recorder first: a run holds the mic"))?;
            if lock(&open.rec).busy() {
                return Err(conflict("a take is going"));
            }
            if inner.scoring.is_some() {
                return Err(conflict("a run is going"));
            }
            inner.scoring = Some(HearScoring {
                id: id.clone(),
                session: session.to_string(),
                channel: channel.to_string(),
                done: 0,
                total: 0,
            });
        }
        self.publish();
        let settings = d.voice();
        let weak = self.d.clone();
        let started = info.clone();
        tokio::spawn(async move {
            let progress_weak = weak.clone();
            let progress = move |done: usize, total: usize| {
                if let Some(d) = progress_weak.upgrade()
                    && let Some(desk) = d.hear()
                {
                    if let Some(s) = &mut lock(&desk.inner).scoring {
                        s.done = done;
                        s.total = total;
                    }
                    desk.publish();
                }
            };
            let result = score_session(&models, &settings, &dir, ch, &commit, progress).await;
            let Some(desk) = weak.upgrade().and_then(|d| d.hear()) else {
                return;
            };
            let problem = match result {
                Ok(results) => {
                    info.takes = results.takes.len();
                    info.wer = MODES
                        .iter()
                        .map(|m| ((*m).to_string(), report::wer(&results, m)))
                        .collect();
                    let (text, _) = report::render(&results, None, CLOSE_BUDGET_SECS);
                    match desk.store.save_run(&info, &results, &text) {
                        Ok(()) => {
                            info!("hear: run {} scored {} takes", info.id, info.takes);
                            None
                        }
                        Err(e) => Some(format!("the run could not be saved: {e}")),
                    }
                }
                Err(e) => Some(format!("the run failed: {e}")),
            };
            if let Some(p) = &problem {
                warn!("hear: {p}");
            }
            let closed = {
                let mut inner = lock(&desk.inner);
                inner.scoring = None;
                inner.problem = problem;
                inner.open.is_none()
            };
            if closed {
                desk.give_mic_back(); // the recorder closed while the run went on
            }
            desk.publish();
        });
        Ok(run_view(started))
    }

    fn runs(&self) -> Vec<HearRunInfo> {
        self.store.runs().into_iter().map(run_view).collect()
    }

    fn run(&self, id: &str, baseline: Option<&str>) -> HearResult<HearRun> {
        let (info, results) = self
            .store
            .run(id)
            .map_err(|e| HearError::NotFound(e.to_string()))?;
        let base = match baseline {
            Some(b) => Some(
                self.store
                    .run(b)
                    .map_err(|e| HearError::NotFound(e.to_string()))?,
            ),
            None => None,
        };
        let r = report::build(
            &results,
            base.as_ref().map(|(_, r): &(RunInfo, Results)| r),
            CLOSE_BUDGET_SECS,
        );
        Ok(HearRun {
            info: run_view(info),
            baseline: base.map(|(i, _)| run_view(i)),
            report: report_view(r),
        })
    }

    fn note_run(&self, id: &str, note: &str) -> HearResult<HearRunInfo> {
        self.store
            .set_run_note(id, note)
            .map(run_view)
            .map_err(|e| HearError::NotFound(e.to_string()))
    }

    fn delete_run(&self, id: &str) -> HearResult<()> {
        if lock(&self.inner)
            .scoring
            .as_ref()
            .is_some_and(|s| s.id == id)
        {
            return Err(conflict("the run is going"));
        }
        self.store
            .delete_run(id)
            .map_err(|e| HearError::NotFound(e.to_string()))
    }
}

impl HearDesk {
    /// Ask the passive loop for the mic, then open the captures on it.
    async fn take_mic_and_open(
        &self,
        d: &Arc<Daemon>,
        session: Option<String>,
        new: bool,
    ) -> HearResult<()> {
        if d.mic.is_off() {
            return Err(HearError::Unavailable(
                "Ghost is not listening. Turn listening on to run the Hearing test.".into(),
            ));
        }
        let (granted_tx, granted_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *lock(&self.lease_q) = Some(Lease {
            granted: granted_tx,
            release: release_rx,
        });
        lock(&self.inner).release = Some(release_tx);
        self.lease_notify.notify_one();
        match tokio::time::timeout(LEASE_WAIT, granted_rx).await {
            Ok(Ok(())) => {}
            _ => {
                lock(&self.lease_q).take();
                return Err(HearError::Unavailable(
                    "Ghost didn't let go of the mic: is it in a session?".into(),
                ));
            }
        }
        let models = d
            .models()
            .ok_or_else(|| HearError::Unavailable("the models are not loaded".into()))?;
        let settings = d.voice();
        let vad = Vad::load(&models.silero, settings.vad_params()).map_err(failed)?;
        let node = settings
            .input_device
            .clone()
            .unwrap_or_else(|| "ghost.mic".to_string());
        let aec = Self::open_capture(node.clone(), 1)
            .await
            .map_err(|e| HearError::Unavailable(format!("{node} could not be opened: {e}")))?;
        let raw_node = self.raw_node();
        let raw = match &raw_node {
            Some(n) => match Self::open_capture(n.clone(), RAW_CHANNELS).await {
                Ok(c) => Some(c),
                Err(e) => {
                    warn!(
                        "hear: the raw array {n} could not be opened, takes have ghost.mic only: {e}"
                    );
                    None
                }
            },
            None => None,
        };
        let today = chrono::Local::now().date_naive();
        let todays = (!new).then(|| self.store.todays_session(today)).flatten();
        let session = match session.or(todays) {
            Some(s) => s,
            None => self.store.new_session(today).map_err(failed)?,
        };
        let rec = Arc::new(Mutex::new(Rec::new(raw.is_some())));
        let me = d
            .hear()
            .ok_or_else(|| HearError::Unavailable("the recorder is not set up".into()))?;
        let mut tasks = vec![tokio::spawn(me.clone().aec_loop(aec, rec.clone(), vad))];
        let raw_device = raw.as_ref().and(raw_node);
        if let Some(raw) = raw {
            tasks.push(tokio::spawn(Self::raw_loop(raw, rec.clone())));
        }
        info!(
            "hear: the recorder is open on session {session} ({node}{})",
            raw_device
                .as_ref()
                .map_or_else(String::new, |r| format!(" and {r}"))
        );
        lock(&self.inner).open = Some(Open {
            session,
            raw_device,
            rec,
            tasks,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raw_array_is_the_capture_blocks_target_not_the_playbacks() {
        let conf = r#"
# capture.props = { node.target = "commented" }
context.modules = [
  { name = libpipewire-module-echo-cancel
    args = {
      capture.props = {
        node.name = ghost.mic.capture
        node.target = "alsa_input.usb-Seeed_reSpeaker.iec958-stereo"
        node.passive = true
      }
      playback.props = {
        node.target = "alsa_output.speakers"
      }
    }
  }
]"#;
        assert_eq!(
            capture_target(conf).as_deref(),
            Some("alsa_input.usb-Seeed_reSpeaker.iec958-stereo")
        );
        assert_eq!(
            capture_target("playback.props = { node.target = \"x\" }"),
            None
        );
        assert_eq!(capture_target(""), None);
    }

    #[test]
    fn a_report_keeps_its_verdicts_and_takes_through_the_api() {
        let mut t = ghost_voice::perf::hear::TakeResult {
            id: "A01".into(),
            block: "A".into(),
            condition: "desk".into(),
            kind: Kind::Speech,
            tags: vec![],
            source: "take".into(),
            seconds: 3.0,
            ref_words: 5,
            modes: Default::default(),
        };
        for m in MODES {
            t.modes.insert(
                m.to_string(),
                ghost_voice::perf::hear::Read {
                    words: 5,
                    sub: 1,
                    classes: vec![[5, 1, 0, 0], [0; 4], [0; 4], [0; 4]],
                    segments: 1,
                    close_secs: Some(0.1),
                    ..Default::default()
                },
            );
        }
        let results = Results {
            meta: Default::default(),
            takes: vec![t],
        };
        let r = report_view(report::build(&results, Some(&results), 0.15));
        assert_eq!(r.pass, Some(true));
        let rows: Vec<&HearRow> = r
            .sections
            .iter()
            .flat_map(|s| &s.parts)
            .filter_map(|p| match p {
                HearPart::Table(t) => Some(&t.rows),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(rows.iter().any(|r| r.verdict == Some(HearVerdict::Same)));
        assert!(rows.iter().any(|r| r.take.as_deref() == Some("A01|take")));
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["sections"][0]["parts"][0]["kind"] == "table", "{v}");
    }
}
