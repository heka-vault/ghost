//! Questions for the operator (ADR 0051): an item on the waiting list with an answer.
//!
//! An agent, a schedule's run, an outside tool or a connected app may ask the operator
//! something. The question is an item on the waiting list (ADR 0029), so it alerts,
//! escalates, keeps quiet hours and do-not-disturb, and is delivered by the routes exactly
//! as any item is (ADR 0031). What an item does not have is kept here, in the `questions`
//! table: who asked, the choices, when it expires, and what came of it. Answering is the
//! item's acknowledgment and its resolution, in one step.
//!
//! **The answer is the operator's.** It arrives from the operator's own surfaces only: the
//! desktop (a click or a typed answer, route `desktop`), the paired phone (a signed
//! request, route `phone`), or their voice in a session, where the words must be in their
//! own last turn and are recorded as they said them (route `voice`; the session's
//! dispatcher checks that before the call reaches Ghost's app). Nothing here takes an
//! answer from a model, an outside tool or an app, and a choice is matched by fixed rules,
//! never by a model ([`match_choice`]).
//!
//! A question asked from a run with nobody there (ADR 0047) is followed up: once it is
//! answered, one more run of the same agent starts with the question and the answer
//! ([`Questions::with_jobs`]). One per answer, recorded with the question, so a restart
//! never starts a second; none for a decline or an expiry.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::confirmation::{Verdict, classify_confirmation, has_negation, normalize};
use crate::models::Proposal;
use crate::redact::redact_text;
use crate::routes::Urgency;
use crate::schedules::{Jobs, RunSpec};
use crate::store::{Item, Store, StoreError};
use crate::time::{iso, parse_iso, utcnow};
use crate::waiting::{self, ItemDraft, WaitingList};

/// How a question's item key starts.
pub const KEY_PREFIX: &str = "question:";
/// The source a connected app's question is kept under: `app:<name>`.
pub const APP_SOURCE: &str = "app:";
/// The run source of a follow-up run (`schedule_runs.source`).
pub const FOLLOW_UP_SOURCE: &str = "answer";
pub const QUESTION_MAX: usize = 300;
pub const DETAIL_MAX: usize = 1000;
pub const CHOICES_MAX: usize = 4;
pub const CHOICE_MAX: usize = 60;
pub const ANSWER_MAX: usize = 2000;
/// A question nobody said how long to keep waits a day.
pub const DEFAULT_EXPIRY: Duration = Duration::hours(24);
pub const MAX_EXPIRY: Duration = Duration::days(7);
/// The longest an asker may hold its request for the answer.
pub const WAIT_MAX_SECS: u64 = 120;
/// How long a settled question is still listed for whoever asked it.
pub const RECENT: Duration = Duration::hours(24);
/// Follow-up runs an hour, at the most, across every question.
pub const FOLLOW_UPS_PER_HOUR: u32 = 30;

/// Where the answer came from.
pub mod via {
    /// Spoken in a session, the operator's own words from their last turn.
    pub const VOICE: &str = "voice";
    /// A click or a typed answer in the desktop app, or the desktop notification's button.
    pub const DESKTOP: &str = "desktop";
    /// A signed request from the paired phone.
    pub const PHONE: &str = "phone";
    /// Ghost itself: an expiry, or a restart that lost the asker.
    pub const GHOST: &str = "ghost";
}

#[derive(Debug, thiserror::Error)]
pub enum QuestionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Waiting(#[from] waiting::WaitingError),
    #[error("there is no question {0}")]
    NotFound(String),
    #[error("the question was {0} already")]
    NotOpen(&'static str),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, QuestionError>;

/// Where a question is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QState {
    Open,
    Answered,
    Declined,
    Expired,
    Cancelled,
}

impl QState {
    pub fn as_str(self) -> &'static str {
        match self {
            QState::Open => "open",
            QState::Answered => "answered",
            QState::Declined => "declined",
            QState::Expired => "expired",
            QState::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => QState::Open,
            "answered" => QState::Answered,
            "declined" => QState::Declined,
            "expired" => QState::Expired,
            "cancelled" => QState::Cancelled,
            _ => return None,
        })
    }
}

/// A question, or a change waiting for a yes shown as one (ADR 0030; `kind` says which).
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    /// The item's key: `question:<id>`, or `change:<proposal>` for a change.
    pub key: String,
    /// Its source: `agent:<slug>`, `client:<name>`, `app:<name>`, or `gate` for a change.
    pub asker: String,
    /// Who asked, as the operator reads it: `Ledger`, `Claude Code`, `GitHub`.
    pub who: String,
    /// The agent to follow up with, when an agent asked.
    pub agent: Option<String>,
    /// The session (or `run-<id>`) it was asked in.
    pub asked_in: Option<String>,
    /// Asked from a run with nobody there: its answer starts one more run.
    pub follow_up: bool,
    pub text: String,
    pub detail: Option<String>,
    pub choices: Vec<String>,
    /// Only one of the choices will do; otherwise the operator may answer in their words.
    pub choices_only: bool,
    pub urgency: Urgency,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: QState,
    /// The operator's words, as they gave them.
    pub answer: Option<String>,
    /// The choice the answer clearly is, when it is one.
    pub choice: Option<String>,
    /// `voice`, `desktop` or `phone`; `ghost` for an expiry.
    pub answered_via: Option<String>,
    pub answered_at: Option<DateTime<Utc>>,
    pub follow_up_run: Option<String>,
    /// A change waiting for a yes, shown as a question (never stored here).
    pub change: Option<String>,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.state == QState::Open
    }

    pub fn is_change(&self) -> bool {
        self.change.is_some()
    }

    /// What Ghost says to put it to the operator: `Ledger asks: should the October
    /// invoice go out today? Yes or no?` The words are the asker's: data, read as they are.
    pub fn spoken(&self) -> String {
        spoken_line(&self.who, &self.text, &self.choices)
    }

    /// The typed fields an asker reads back (`question`, `questions_list`): never more
    /// than the asker wrote and the operator answered.
    pub fn fields(&self) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert("key".into(), json!(self.key));
        out.insert("question".into(), json!(self.text));
        if let Some(d) = &self.detail {
            out.insert("detail".into(), json!(d));
        }
        if !self.choices.is_empty() {
            out.insert("choices".into(), json!(self.choices));
            out.insert("choices_only".into(), json!(self.choices_only));
        }
        out.insert("asked_by".into(), json!(self.who));
        out.insert("state".into(), json!(self.state.as_str()));
        out.insert("asked_at".into(), json!(iso(&self.created_at)));
        out.insert("expires_at".into(), json!(iso(&self.expires_at)));
        if let Some(a) = &self.answer {
            out.insert("answer".into(), json!(a));
        }
        if let Some(c) = &self.choice {
            out.insert("choice".into(), json!(c));
        }
        if let Some(v) = &self.answered_via {
            out.insert("answered_via".into(), json!(v));
        }
        if let Some(t) = &self.answered_at {
            out.insert("answered_at".into(), json!(iso(t)));
        }
        out
    }
}

/// What an asker hands over.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub asker: String,
    pub who: String,
    pub agent: Option<String>,
    pub asked_in: Option<String>,
    pub follow_up: bool,
    pub text: String,
    pub detail: Option<String>,
    pub choices: Vec<String>,
    pub choices_only: bool,
    pub urgency: Urgency,
    /// None: [`DEFAULT_EXPIRY`].
    pub expires_in: Option<Duration>,
}

impl Ask {
    pub fn new(asker: impl Into<String>, who: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            asker: asker.into(),
            who: who.into(),
            agent: None,
            asked_in: None,
            follow_up: false,
            text: text.into(),
            detail: None,
            choices: Vec::new(),
            choices_only: false,
            urgency: Urgency::Notify,
            expires_in: None,
        }
    }
}

/// Runs whenever a question is asked or settled.
pub type OnQuestion = Arc<dyn Fn(&Question) + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    let t = flat(text);
    if t.chars().count() <= max {
        t
    } else {
        let mut cut: String = t.chars().take(max - 1).collect();
        cut.push('…');
        cut
    }
}

/// `Ledger asks: <question>` and the choices as a spoken question: `Yes or no?`,
/// `Staging, production or both?`.
pub fn spoken_line(who: &str, text: &str, choices: &[String]) -> String {
    let mut q = flat(text);
    if !q.ends_with(['?', '.', '!']) {
        q.push('?');
    }
    let mut out = format!("{who} asks: {q}");
    if !choices.is_empty() {
        let mut words: Vec<String> = choices.iter().map(|c| flat(c)).collect();
        for w in words.iter_mut().skip(1) {
            // "Yes or no?", not "Yes or No?": only a word in capitals (CI) keeps them.
            let capitals = w.chars().filter(|c| c.is_alphabetic()).count() > 1
                && !w.chars().any(char::is_lowercase);
            if !capitals {
                *w = lower_first(w);
            }
        }
        let said = match words.len() {
            1 => words[0].clone(),
            n => format!("{} or {}", words[..n - 1].join(", "), words[n - 1]),
        };
        out.push(' ');
        out.push_str(&upper_first(&said));
        out.push('?');
    }
    out
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The choice an answer clearly is, by fixed rules and no model: the answer is the choice
/// (case, punctuation and a leading "ghost" or "okay" aside), or starts with it and no
/// other choice; for exactly the choices yes and no, the confirmation allowlist decides,
/// negation guard included ("yeah, go ahead" is yes, "no, not today" is no, "yes but not
/// today" is neither).
pub fn match_choice(answer: &str, choices: &[String]) -> Option<String> {
    if choices.is_empty() {
        return None;
    }
    let said = normalize(answer);
    let said = said
        .strip_prefix("ghost ")
        .or_else(|| said.strip_prefix("okay "))
        .or_else(|| said.strip_prefix("ok "))
        .unwrap_or(&said)
        .to_string();
    if said.is_empty() {
        return None;
    }
    let norm: Vec<String> = choices.iter().map(|c| normalize(c)).collect();
    if let Some(i) = norm.iter().position(|c| *c == said) {
        return Some(choices[i].clone());
    }
    let yes_no =
        norm.len() == 2 && norm.contains(&"yes".to_string()) && norm.contains(&"no".to_string());
    if yes_no {
        let first = said.split(' ').next().unwrap_or("");
        let pick = if classify_confirmation(answer) == Verdict::Affirmative
            || (matches!(first, "yes" | "yeah" | "yep" | "yup") && !has_negation(&said))
        {
            "yes"
        } else if matches!(first, "no" | "nope")
            || classify_confirmation(answer) == Verdict::Negative && !said.contains("yes")
        {
            "no"
        } else {
            return None;
        };
        let i = norm.iter().position(|c| c == pick)?;
        return Some(choices[i].clone());
    }
    let starts: Vec<usize> = norm
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty() && (said.starts_with(&format!("{c} ")) || said == **c))
        .map(|(i, _)| i)
        .collect();
    match starts.as_slice() {
        [one] => Some(choices[*one].clone()),
        _ => None,
    }
}

/// A change waiting for a yes, as a question with the choices Yes and No (ADR 0030). The
/// answer is the gate's: a yes is a confirmation by id with the route as its classifier.
pub fn change_question(p: &Proposal) -> Question {
    Question {
        key: waiting::change_key(&p.id),
        asker: waiting::GATE.into(),
        who: p
            .conversation
            .as_deref()
            .map(upper_first)
            .unwrap_or_else(|| "Ghost".into()),
        agent: p.conversation.clone(),
        asked_in: Some(p.session_id.clone()),
        follow_up: false,
        text: format!("{}?", p.preview.trim_end_matches(['.', '?'])),
        detail: (!p.rationale.trim().is_empty()).then(|| clip(&p.rationale, DETAIL_MAX)),
        choices: vec!["Yes".into(), "No".into()],
        choices_only: true,
        urgency: Urgency::Notify,
        created_at: p.created_at,
        expires_at: p.expires_at,
        state: QState::Open,
        answer: None,
        choice: None,
        answered_via: None,
        answered_at: None,
        follow_up_run: None,
        change: Some(p.id.clone()),
    }
}

/// The questions, over the store and the waiting list. Cheap to share: one per daemon.
pub struct Questions {
    store: Arc<Store>,
    waiting: Arc<WaitingList>,
    jobs: Mutex<Option<Arc<Jobs>>>,
    hooks: Mutex<Vec<OnQuestion>>,
    /// One watch per question someone is waiting on (`wait`), told when it settles.
    waiters: Mutex<HashMap<String, watch::Sender<QState>>>,
}

const COLS: &str = "key, asker, who, agent, asked_in, follow_up, question, detail, choices, \
    choices_only, urgency, created_at, expires_at, state, answer, choice, answered_via, \
    answered_at, follow_up_run";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Question> {
    let choices: String = r.get(8)?;
    let urgency: String = r.get(10)?;
    let state: String = r.get(13)?;
    let dt = |i: usize| -> rusqlite::Result<Option<DateTime<Utc>>> {
        Ok(r.get::<_, Option<String>>(i)?
            .as_deref()
            .and_then(parse_iso))
    };
    Ok(Question {
        key: r.get(0)?,
        asker: r.get(1)?,
        who: r.get(2)?,
        agent: r.get(3)?,
        asked_in: r.get(4)?,
        follow_up: r.get::<_, i64>(5)? != 0,
        text: r.get(6)?,
        detail: r.get(7)?,
        choices: serde_json::from_str(&choices).unwrap_or_default(),
        choices_only: r.get::<_, i64>(9)? != 0,
        urgency: Urgency::parse(&urgency).unwrap_or(Urgency::Notify),
        created_at: dt(11)?.unwrap_or_else(utcnow),
        expires_at: dt(12)?.unwrap_or_else(utcnow),
        state: QState::parse(&state).unwrap_or(QState::Expired),
        answer: r.get(14)?,
        choice: r.get(15)?,
        answered_via: r.get(16)?,
        answered_at: dt(17)?,
        follow_up_run: r.get(18)?,
        change: None,
    })
}

impl Questions {
    pub fn new(store: Arc<Store>, waiting: Arc<WaitingList>) -> Self {
        Self {
            store,
            waiting,
            jobs: Mutex::new(None),
            hooks: Mutex::new(Vec::new()),
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// The schedules, so a question asked from a run is followed up by another (ADR 0047).
    pub fn with_jobs(self, jobs: Arc<Jobs>) -> Self {
        *lock(&self.jobs) = Some(jobs);
        self
    }

    pub fn set_jobs(&self, jobs: Arc<Jobs>) {
        *lock(&self.jobs) = Some(jobs);
    }

    /// Told of every question asked and settled: the event bus, the phone's stream.
    pub fn on_change(&self, hook: OnQuestion) {
        lock(&self.hooks).push(hook);
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn waiting(&self) -> &Arc<WaitingList> {
        &self.waiting
    }

    fn told(&self, q: &Question) {
        let hooks: Vec<OnQuestion> = lock(&self.hooks).clone();
        for h in hooks {
            h(q);
        }
        if !q.is_open()
            && let Some(tx) = lock(&self.waiters).remove(&q.key)
        {
            let _ = tx.send(q.state);
        }
    }

    // --- asking --------------------------------------------------------------------------

    /// Put a question to the operator: a row here and an item on the waiting list.
    pub fn ask(&self, a: Ask, now: Option<DateTime<Utc>>) -> Result<Question> {
        let now = now.unwrap_or_else(utcnow);
        let text = clip(&redact_text(&a.text), QUESTION_MAX);
        if text.is_empty() {
            return Err(QuestionError::Invalid("a question needs its words".into()));
        }
        let mut choices: Vec<String> = Vec::new();
        for c in &a.choices {
            let c = clip(&redact_text(c), CHOICE_MAX);
            if !c.is_empty() && !choices.iter().any(|k| k.to_lowercase() == c.to_lowercase()) {
                choices.push(c);
            }
        }
        if choices.len() > CHOICES_MAX {
            return Err(QuestionError::Invalid(format!(
                "a question has {CHOICES_MAX} choices at most"
            )));
        }
        if a.choices_only && choices.len() < 2 {
            return Err(QuestionError::Invalid(
                "choices only needs at least two choices".into(),
            ));
        }
        let expires_in = a
            .expires_in
            .unwrap_or(DEFAULT_EXPIRY)
            .clamp(Duration::minutes(1), MAX_EXPIRY);
        let detail = a
            .detail
            .as_deref()
            .map(|d| clip(&redact_text(d), DETAIL_MAX))
            .filter(|d| !d.is_empty());
        let who = clip(&a.who, 60);
        let key = format!(
            "{KEY_PREFIX}{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let q = Question {
            key: key.clone(),
            asker: a.asker.clone(),
            who: if who.is_empty() { "Ghost".into() } else { who },
            agent: a.agent.clone(),
            asked_in: a.asked_in.clone(),
            follow_up: a.follow_up,
            text,
            detail,
            choices,
            choices_only: a.choices_only,
            urgency: a.urgency,
            created_at: now,
            expires_at: now + expires_in,
            state: QState::Open,
            answer: None,
            choice: None,
            answered_via: None,
            answered_at: None,
            follow_up_run: None,
            change: None,
        };
        self.store.conn().execute(
            &format!("INSERT INTO questions ({COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL, NULL)"),
            params![
                q.key,
                q.asker,
                q.who,
                q.agent,
                q.asked_in,
                q.follow_up as i64,
                q.text,
                q.detail,
                serde_json::to_string(&q.choices).unwrap_or_else(|_| "[]".into()),
                q.choices_only as i64,
                q.urgency.as_str(),
                iso(&q.created_at),
                iso(&q.expires_at),
                QState::Open.as_str(),
            ],
        )?;
        let mut fields = Map::new();
        fields.insert("urgency".into(), json!(q.urgency.as_str()));
        fields.insert("kind".into(), json!("question"));
        fields.insert("said".into(), json!(clip(&q.spoken(), 400)));
        if let Some(d) = &q.detail {
            fields.insert("detail".into(), json!(clip(d, 500)));
        }
        let title = clip(&format!("{} asks: {}", q.who, q.text), waiting::TITLE_MAX);
        self.waiting
            .observe(&q.asker, &ItemDraft::new(key.clone(), title).fields(fields))?;
        info!("questions: {} asked {}", q.asker, q.key);
        self.told(&q);
        Ok(q)
    }

    // --- reading -------------------------------------------------------------------------

    pub fn get(&self, key: &str) -> Result<Option<Question>> {
        let conn = self.store.conn();
        Ok(conn
            .query_row(
                &format!("SELECT {COLS} FROM questions WHERE key = ?"),
                [key],
                row,
            )
            .optional()?)
    }

    fn select(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Question>> {
        let conn = self.store.conn();
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM questions WHERE {clause}"))?;
        let rows = st.query_map(args, row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The open questions, oldest first, after the ones past their time are expired.
    pub fn open(&self, now: Option<DateTime<Utc>>) -> Result<Vec<Question>> {
        self.expire(now)?;
        self.select("state = 'open' ORDER BY created_at, rowid", &[])
    }

    /// Open and recently settled, newest first; one asker's when `asker` is given.
    pub fn recent(
        &self,
        asker: Option<&str>,
        now: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<Question>> {
        let now = now.unwrap_or_else(utcnow);
        self.expire(Some(now))?;
        let since = iso(&(now - RECENT));
        let limit = limit.clamp(1, 200);
        match asker {
            Some(a) => self.select(
                "asker = ? AND (state = 'open' OR answered_at >= ? OR created_at >= ?) \
                 ORDER BY created_at DESC, rowid DESC LIMIT ?",
                &[&a, &since, &since, &limit],
            ),
            None => self.select(
                "(state = 'open' OR answered_at >= ? OR created_at >= ?) \
                 ORDER BY created_at DESC, rowid DESC LIMIT ?",
                &[&since, &since, &limit],
            ),
        }
    }

    // --- settling ------------------------------------------------------------------------

    /// One open question settled: the row, the item resolved, the waiters and the hooks
    /// told. `Err(NotOpen)` when it was settled already.
    fn settle(
        &self,
        key: &str,
        state: QState,
        answer: Option<&str>,
        choice: Option<&str>,
        via: &str,
        now: DateTime<Utc>,
    ) -> Result<Question> {
        let n = self.store.conn().execute(
            "UPDATE questions SET state = ?, answer = ?, choice = ?, answered_via = ?, answered_at = ? \
             WHERE key = ? AND state = 'open'",
            params![state.as_str(), answer, choice, via, iso(&now), key],
        )?;
        let q = self
            .get(key)?
            .ok_or_else(|| QuestionError::NotFound(key.to_string()))?;
        if n == 0 {
            return Err(QuestionError::NotOpen(q.state.as_str()));
        }
        // Answering is the acknowledgment and the resolution (ADR 0051).
        if let Err(e) = self.waiting.resolve(&q.asker, key) {
            warn!("questions: {key} stayed on the list: {e}");
        }
        self.told(&q);
        Ok(q)
    }

    fn open_one(&self, key: &str, now: DateTime<Utc>) -> Result<Question> {
        let q = self
            .get(key)?
            .ok_or_else(|| QuestionError::NotFound(key.to_string()))?;
        if q.is_open() && q.expires_at <= now {
            let _ = self.settle(key, QState::Expired, None, None, via::GHOST, now);
            return Err(QuestionError::NotOpen(QState::Expired.as_str()));
        }
        if !q.is_open() {
            return Err(QuestionError::NotOpen(q.state.as_str()));
        }
        Ok(q)
    }

    /// The operator's answer, as they gave it, from `via`. With choices, the choice it
    /// clearly is is noted; with choices only, an answer that is none of them is refused.
    /// The same answer given again (a phone that retried) is the answer it already was.
    pub fn answer(
        &self,
        key: &str,
        text: &str,
        via: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Question> {
        let now = now.unwrap_or_else(utcnow);
        let words = redact_text(text.trim());
        if words.is_empty() {
            return Err(QuestionError::Invalid("an answer needs its words".into()));
        }
        if words.chars().count() > ANSWER_MAX {
            return Err(QuestionError::Invalid(format!(
                "an answer is {ANSWER_MAX} characters at most"
            )));
        }
        let q = match self.open_one(key, now) {
            Ok(q) => q,
            Err(QuestionError::NotOpen(_))
                if self.get(key)?.is_some_and(|q| {
                    q.state == QState::Answered
                        && q.answer.as_deref() == Some(words.as_str())
                        && q.answered_via.as_deref() == Some(via)
                }) =>
            {
                return self
                    .get(key)?
                    .ok_or_else(|| QuestionError::NotFound(key.to_string()));
            }
            Err(e) => return Err(e),
        };
        let choice = match_choice(&words, &q.choices);
        if q.choices_only && choice.is_none() {
            return Err(QuestionError::Invalid(format!(
                "answer with one of: {}",
                q.choices.join(", ")
            )));
        }
        let q = self.settle(
            key,
            QState::Answered,
            Some(&words),
            choice.as_deref(),
            via,
            now,
        )?;
        info!("questions: {key} answered from {via}");
        Ok(self.follow_up(q, now))
    }

    /// The operator will not answer it.
    pub fn decline(&self, key: &str, via: &str, now: Option<DateTime<Utc>>) -> Result<Question> {
        let now = now.unwrap_or_else(utcnow);
        match self.open_one(key, now) {
            Ok(_) => {}
            Err(QuestionError::NotOpen(_))
                if self.get(key)?.is_some_and(|q| q.state == QState::Declined) =>
            {
                return self
                    .get(key)?
                    .ok_or_else(|| QuestionError::NotFound(key.to_string()));
            }
            Err(e) => return Err(e),
        }
        let q = self.settle(key, QState::Declined, None, None, via, now)?;
        info!("questions: {key} declined from {via}");
        Ok(q)
    }

    /// Whoever asked takes it back: only its own.
    pub fn cancel(&self, key: &str, asker: &str, now: Option<DateTime<Utc>>) -> Result<Question> {
        let now = now.unwrap_or_else(utcnow);
        let q = self
            .get(key)?
            .ok_or_else(|| QuestionError::NotFound(key.to_string()))?;
        if q.asker != asker {
            return Err(QuestionError::NotFound(key.to_string()));
        }
        self.open_one(key, now)?;
        self.settle(key, QState::Cancelled, None, None, asker, now)
    }

    /// The open questions past their time: expired, their items resolved.
    pub fn expire(&self, now: Option<DateTime<Utc>>) -> Result<Vec<Question>> {
        let now = now.unwrap_or_else(utcnow);
        let due = self.select(
            "state = 'open' AND expires_at <= ? ORDER BY created_at",
            &[&iso(&now)],
        )?;
        let mut out = Vec::new();
        for q in due {
            if let Ok(q) = self.settle(&q.key, QState::Expired, None, None, via::GHOST, now) {
                info!("questions: {} expired", q.key);
                out.push(q);
            }
        }
        Ok(out)
    }

    /// At start: an app's question that was open when Ghost stopped has nobody waiting
    /// for it any more (the app's call is gone), so it is declined.
    pub fn recover(&self, now: Option<DateTime<Utc>>) -> Result<usize> {
        let now = now.unwrap_or_else(utcnow);
        let lost = self.select(
            "state = 'open' AND asker LIKE 'app:%' ORDER BY created_at",
            &[],
        )?;
        let mut n = 0;
        for q in lost {
            if self
                .settle(&q.key, QState::Declined, None, None, via::GHOST, now)
                .is_ok()
            {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Wait up to `timeout` for it to be settled; what it is then.
    pub async fn wait(&self, key: &str, timeout: StdDuration) -> Result<Question> {
        let mut rx = {
            let mut waiters = lock(&self.waiters);
            let tx = waiters
                .entry(key.to_string())
                .or_insert_with(|| watch::channel(QState::Open).0);
            tx.subscribe()
        };
        let q = self
            .get(key)?
            .ok_or_else(|| QuestionError::NotFound(key.to_string()))?;
        if !q.is_open() || timeout.is_zero() {
            return Ok(q);
        }
        let _ = tokio::time::timeout(timeout, rx.wait_for(|s| *s != QState::Open)).await;
        self.get(key)?
            .ok_or_else(|| QuestionError::NotFound(key.to_string()))
    }

    /// A run waited for the answer and none came: from now its answer starts a run of its
    /// own. False when it was settled meanwhile (the caller reads the answer instead).
    pub fn arm_follow_up(&self, key: &str) -> Result<bool> {
        let n = self.store.conn().execute(
            "UPDATE questions SET follow_up = 1 WHERE key = ? AND state = 'open'",
            [key],
        )?;
        Ok(n == 1)
    }

    /// A question from a run is followed up by one more run of the same agent, with the
    /// question and the answer as what it is asked. One per answer: the run is queued once
    /// for this key, and that is recorded, so neither a retry nor a restart starts another.
    fn follow_up(&self, q: Question, now: DateTime<Utc>) -> Question {
        if !q.follow_up || q.state != QState::Answered {
            return q;
        }
        let (Some(agent), Some(jobs)) = (q.agent.clone(), lock(&self.jobs).clone()) else {
            return q;
        };
        let answer = q.answer.clone().unwrap_or_default();
        let mut instruction = format!(
            "You asked the operator: \"{}\"\nThe operator answered ({}): \"{}\"",
            q.text,
            q.answered_via.as_deref().unwrap_or("the waiting list"),
            answer
        );
        if let Some(c) = &q.choice {
            instruction.push_str(&format!("\nThat is the choice \"{c}\"."));
        }
        instruction.push_str(
            "\nCarry on with what you asked it for. Their answer is not a yes to any change: \
             a change that needs one still waits for it.",
        );
        let spec = RunSpec {
            instruction,
            data: Some(json!({"question": q.key, "answered_via": q.answered_via})),
            skills: Vec::new(),
            max_turns: crate::schedules::DEFAULT_MAX_TURNS,
            max_runtime_secs: crate::schedules::DEFAULT_MAX_RUNTIME_SECS,
            urgency: q.urgency.as_str().into(),
            notify_failure: true,
        };
        match jobs.enqueue_act(
            FOLLOW_UP_SOURCE,
            &q.key,
            &agent,
            "answer from the operator",
            &spec,
            FOLLOW_UPS_PER_HOUR,
            true,
            now,
        ) {
            Ok(Ok(run)) => {
                let _ = self.store.conn().execute(
                    "UPDATE questions SET follow_up_run = ? WHERE key = ?",
                    params![run.id, q.key],
                );
                info!(
                    "questions: {} answered; {agent} follows up in run {}",
                    q.key, run.id
                );
                self.get(&q.key).ok().flatten().unwrap_or(q)
            }
            Ok(Err(why)) => {
                info!("questions: {} not followed up: {why}", q.key);
                q
            }
            Err(e) => {
                warn!("questions: {} not followed up: {e}", q.key);
                q
            }
        }
    }
}

// --- a connected app's question (ADR 0046's elicitation) ------------------------------------

/// What an app's form can be asked as: one free-text field, or one field with a fixed list
/// (an `enum`, or a yes or no). Anything else is declined, with why.
#[derive(Debug, Clone, PartialEq)]
pub enum AppForm {
    Text { field: String },
    Choice { field: String, choices: Vec<String> },
    YesNo { field: String },
}

/// The form an app asked with, as one question; `Err` says why Ghost declines it.
pub fn app_form(q: &crate::apps::Question) -> std::result::Result<AppForm, String> {
    let props = q
        .schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if props.len() != 1 {
        return Err(format!(
            "it asks for {} fields; Ghost asks the operator for one answer at a time",
            props.len()
        ));
    }
    let (field, spec) = props.into_iter().next().expect("one field");
    if !q.choices.is_empty() {
        if q.choices.len() > CHOICES_MAX {
            return Err(format!(
                "it offers {} choices; Ghost offers {CHOICES_MAX} at most",
                q.choices.len()
            ));
        }
        return Ok(AppForm::Choice {
            field,
            choices: q.choices.clone(),
        });
    }
    match spec.get("type").and_then(Value::as_str) {
        Some("string") | None => Ok(AppForm::Text { field }),
        Some("boolean") => Ok(AppForm::YesNo { field }),
        Some(other) => Err(format!("it wants a {other}, which Ghost does not ask for")),
    }
}

/// The asker a connected app's questions go to (ADR 0051): each becomes a question from
/// the app (`app:<name>`), on the waiting list like any other, held until the app's own
/// time limit and declined when it runs out. The app's words are its own: shown and said
/// as "GitHub asks: …", never as Ghost's, and only the operator answers them.
pub struct AppAsker {
    questions: Arc<std::sync::OnceLock<Arc<Questions>>>,
}

impl AppAsker {
    /// Questions come later than the apps connect: until they are set, every question is
    /// declined.
    pub fn new(questions: Arc<std::sync::OnceLock<Arc<Questions>>>) -> Self {
        Self { questions }
    }
}

#[async_trait::async_trait]
impl crate::apps::Asker for AppAsker {
    async fn ask(&self, q: crate::apps::Question) -> crate::apps::Answer {
        use crate::apps::Answer;
        let Some(questions) = self.questions.get().cloned() else {
            return Answer::Declined;
        };
        let form = match app_form(&q) {
            Ok(f) => f,
            Err(why) => {
                info!("app {}: a question declined: {why}", q.app);
                return Answer::Declined;
            }
        };
        let mut ask = Ask::new(
            format!("{APP_SOURCE}{}", q.app),
            q.title.clone(),
            q.message.clone(),
        );
        ask.expires_in = Duration::from_std(q.timeout).ok();
        match &form {
            AppForm::Choice { choices, .. } => {
                ask.choices = choices.clone();
                ask.choices_only = choices.len() >= 2;
            }
            AppForm::YesNo { .. } => {
                ask.choices = vec!["Yes".into(), "No".into()];
                ask.choices_only = true;
            }
            AppForm::Text { .. } => {}
        }
        let asked = match questions.ask(ask, None) {
            Ok(a) => a,
            Err(e) => {
                warn!("app {}: its question was not asked: {e}", q.app);
                return Answer::Declined;
            }
        };
        // A little short of the app's own limit, so the decline is Ghost's and on record.
        let wait = q.timeout.saturating_sub(StdDuration::from_secs(1));
        let got = match questions.wait(&asked.key, wait).await {
            Ok(got) => got,
            Err(_) => return Answer::Declined,
        };
        if got.is_open() {
            let _ = questions.decline(&asked.key, via::GHOST, None);
            info!(
                "app {}: its question was not answered in time; declined",
                q.app
            );
            return Answer::Declined;
        }
        if got.state != QState::Answered {
            return Answer::Declined;
        }
        let words = got.answer.clone().unwrap_or_default();
        let (field, value) = match form {
            AppForm::Text { field } => (field, json!(words)),
            AppForm::Choice { field, .. } => match got.choice {
                Some(c) => (field, json!(c)),
                None => return Answer::Declined,
            },
            AppForm::YesNo { field } => match got.choice.as_deref() {
                Some("Yes") => (field, json!(true)),
                Some("No") => (field, json!(false)),
                _ => return Answer::Declined,
            },
        };
        Answer::Answered(Map::from_iter([(field, value)]))
    }
}

/// An item that is a question (its key says so).
pub fn is_question(item: &Item) -> bool {
    item.key.starts_with(KEY_PREFIX)
}

/// The words Ghost puts a question item to the operator with, when it is one.
pub fn said_of(item: &Item) -> Option<String> {
    if !is_question(item) {
        return None;
    }
    item.fields
        .get("said")
        .and_then(Value::as_str)
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedules::RunStatus;
    use crate::waiting::{Items, State};

    fn world() -> (Arc<Store>, Arc<WaitingList>, Questions) {
        let store = Arc::new(Store::in_memory().unwrap());
        let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
        let q = Questions::new(store.clone(), waiting.clone());
        (store, waiting, q)
    }

    fn invoice() -> Ask {
        let mut a = Ask::new(
            "agent:ledger",
            "Ledger",
            "Should the October invoice go out today",
        );
        a.agent = Some("ledger".into());
        a.choices = vec!["Yes".into(), "No".into()];
        a
    }

    #[test]
    fn a_question_is_an_item_that_alerts_until_it_is_answered() {
        let (_, waiting, qs) = world();
        let q = qs.ask(invoice(), None).unwrap();
        assert!(q.key.starts_with(KEY_PREFIX));
        assert_eq!(
            q.spoken(),
            "Ledger asks: Should the October invoice go out today? Yes or no?"
        );
        let item = waiting.items.get(&q.key).unwrap().unwrap();
        assert_eq!(item.source, "agent:ledger");
        assert_eq!(
            item.title,
            "Ledger asks: Should the October invoice go out today"
        );
        assert!(item.needs_alerting(None));
        assert_eq!(said_of(&item).as_deref(), Some(q.spoken().as_str()));

        let done = qs
            .answer(&q.key, "yeah, send it", via::PHONE, None)
            .unwrap();
        assert_eq!(done.state, QState::Answered);
        assert_eq!(done.answer.as_deref(), Some("yeah, send it"));
        assert_eq!(done.choice.as_deref(), Some("Yes"));
        assert_eq!(done.answered_via.as_deref(), Some("phone"));
        // Answering is the acknowledgment and the resolution.
        assert!(
            waiting
                .items
                .get(&q.key)
                .unwrap()
                .unwrap()
                .resolved_at
                .is_some()
        );
        assert!(qs.open(None).unwrap().is_empty());
        // Asked again with the same words from the same place, it is what it was.
        assert_eq!(
            qs.answer(&q.key, "yeah, send it", via::PHONE, None)
                .unwrap()
                .state,
            QState::Answered
        );
        // Anything else is refused: it was answered.
        assert!(matches!(
            qs.answer(&q.key, "no", via::DESKTOP, None),
            Err(QuestionError::NotOpen("answered"))
        ));
        assert!(matches!(
            qs.decline(&q.key, via::DESKTOP, None),
            Err(QuestionError::NotOpen("answered"))
        ));
    }

    #[test]
    fn a_question_survives_a_restart_open_and_answered() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("ghost.db");
        let key = {
            let store = Arc::new(Store::open(&db).unwrap());
            let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
            Questions::new(store, waiting)
                .ask(invoice(), None)
                .unwrap()
                .key
        };
        let store = Arc::new(Store::open(&db).unwrap());
        let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
        let qs = Questions::new(store, waiting.clone());
        let open = qs.open(None).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].key, key);
        assert_eq!(open[0].choices, vec!["Yes", "No"]);
        assert_eq!(
            waiting.items.get(&key).unwrap().unwrap().state(None),
            State::New
        );
        qs.answer(&key, "No", via::DESKTOP, None).unwrap();
        drop(qs);
        let store = Arc::new(Store::open(&db).unwrap());
        let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
        let again = Questions::new(store, waiting).get(&key).unwrap().unwrap();
        assert_eq!(again.state, QState::Answered);
        assert_eq!(again.choice.as_deref(), Some("No"));
    }

    #[test]
    fn declined_expired_and_cancelled_each_end_it_and_resolve_the_item() {
        let (_, waiting, qs) = world();
        let a = qs.ask(invoice(), None).unwrap();
        assert_eq!(
            qs.decline(&a.key, via::DESKTOP, None).unwrap().state,
            QState::Declined
        );
        assert!(
            waiting
                .items
                .get(&a.key)
                .unwrap()
                .unwrap()
                .resolved_at
                .is_some()
        );

        let now = utcnow();
        let mut short = invoice();
        short.expires_in = Some(Duration::minutes(5));
        let b = qs.ask(short, Some(now)).unwrap();
        assert!(
            qs.expire(Some(now + Duration::minutes(4)))
                .unwrap()
                .is_empty()
        );
        let gone = qs.expire(Some(now + Duration::minutes(6))).unwrap();
        assert_eq!(gone[0].key, b.key);
        assert_eq!(qs.get(&b.key).unwrap().unwrap().state, QState::Expired);
        assert!(matches!(
            qs.answer(&b.key, "yes", via::PHONE, None),
            Err(QuestionError::NotOpen("expired"))
        ));

        let c = qs.ask(invoice(), None).unwrap();
        // Only who asked may take it back.
        assert!(qs.cancel(&c.key, "agent:brand", None).is_err());
        assert_eq!(
            qs.cancel(&c.key, "agent:ledger", None).unwrap().state,
            QState::Cancelled
        );
    }

    #[test]
    fn choices_are_matched_by_fixed_rules() {
        let yn = vec!["Yes".to_string(), "No".to_string()];
        assert_eq!(match_choice("yes", &yn).as_deref(), Some("Yes"));
        assert_eq!(match_choice("Ghost, no.", &yn).as_deref(), Some("No"));
        assert_eq!(match_choice("yeah go ahead", &yn).as_deref(), Some("Yes"));
        assert_eq!(match_choice("no, not today", &yn).as_deref(), Some("No"));
        assert_eq!(match_choice("yes but not today", &yn), None);
        let envs = vec!["Staging".to_string(), "Production".to_string()];
        assert_eq!(
            match_choice("production please", &envs).as_deref(),
            Some("Production")
        );
        assert_eq!(match_choice("the other one", &envs), None);
        assert_eq!(match_choice("anything", &[]), None);
    }

    #[test]
    fn choices_only_refuses_an_answer_that_is_none_of_them() {
        let (_, _, qs) = world();
        let mut a = invoice();
        a.choices_only = true;
        let q = qs.ask(a, None).unwrap();
        assert!(matches!(
            qs.answer(&q.key, "send half of it", via::PHONE, None),
            Err(QuestionError::Invalid(_))
        ));
        assert!(qs.get(&q.key).unwrap().unwrap().is_open());
        // More than four choices, or choices only with fewer than two, is not a question.
        let mut many = invoice();
        many.choices = (0..5).map(|n| n.to_string()).collect();
        assert!(qs.ask(many, None).is_err());
    }

    #[test]
    fn the_spoken_line_reads_its_choices() {
        let three = vec!["Staging".into(), "Production".into(), "Both".into()];
        assert_eq!(
            spoken_line("GitHub", "Which environment", &three),
            "GitHub asks: Which environment? Staging, production or both?"
        );
        assert_eq!(
            spoken_line("Claude Code", "Ship it now?", &[]),
            "Claude Code asks: Ship it now?"
        );
    }

    #[tokio::test]
    async fn a_waiter_hears_the_answer_at_once_and_a_timeout_returns_it_open() {
        let (_, _, qs) = world();
        let qs = Arc::new(qs);
        let q = qs.ask(invoice(), None).unwrap();
        let open = qs.wait(&q.key, StdDuration::from_millis(30)).await.unwrap();
        assert!(open.is_open());
        let waiter = tokio::spawn({
            let (qs, key) = (qs.clone(), q.key.clone());
            async move { qs.wait(&key, StdDuration::from_secs(10)).await.unwrap() }
        });
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        qs.answer(&q.key, "yes", via::DESKTOP, None).unwrap();
        let got = tokio::time::timeout(StdDuration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.state, QState::Answered);
    }

    #[test]
    fn an_answer_to_a_question_from_a_run_queues_exactly_one_follow_up() {
        let (store, _, qs) = world();
        let jobs = Arc::new(Jobs::new(store.clone()));
        qs.set_jobs(jobs.clone());
        let mut a = invoice();
        a.follow_up = true;
        a.asked_in = Some("run-r1".into());
        let q = qs.ask(a, None).unwrap();
        let done = qs.answer(&q.key, "yes", via::PHONE, None).unwrap();
        let run = done.follow_up_run.clone().expect("a follow-up");
        let queued = jobs.queued().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, run);
        assert_eq!(queued[0].agent, "ledger");
        assert_eq!(queued[0].status, RunStatus::Queued);
        assert!(
            queued[0]
                .spec
                .instruction
                .contains("You asked the operator: \"Should the October invoice go out today\"")
        );
        assert!(
            queued[0]
                .spec
                .instruction
                .contains("answered (phone): \"yes\"")
        );
        // The same answer again (a retry) starts nothing more.
        qs.answer(&q.key, "yes", via::PHONE, None).unwrap();
        assert_eq!(jobs.queued().unwrap().len(), 1);
        // A decline or an expiry is never followed up.
        let mut b = invoice();
        b.follow_up = true;
        let b = qs.ask(b, None).unwrap();
        qs.decline(&b.key, via::PHONE, None).unwrap();
        assert_eq!(jobs.queued().unwrap().len(), 1);
    }

    #[test]
    fn an_apps_question_open_at_a_restart_is_declined() {
        let (_, _, qs) = world();
        let app = qs
            .ask(Ask::new("app:github", "GitHub", "Which branch?"), None)
            .unwrap();
        let agent = qs.ask(invoice(), None).unwrap();
        assert_eq!(qs.recover(None).unwrap(), 1);
        assert_eq!(qs.get(&app.key).unwrap().unwrap().state, QState::Declined);
        assert!(qs.get(&agent.key).unwrap().unwrap().is_open());
    }
}
