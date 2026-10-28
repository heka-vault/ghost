//! SQLite (WAL) persistence for proposals, the audit log (ADR 0009), and what Ghost
//! writes about agents: the registry mirror, the transcript, every compaction (ADR 0014).
//! Transcripts and compactions are keyed by the agent's stable id, so an agent folder
//! moved by hand keeps its history. The schema and the rows are the Python daemon's, so
//! the operator's database opens as it is.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde_json::{Map, Value};
use tracing::warn;

use crate::codec::{dump_proposal, load_proposal};
use crate::conversations::models::{Compaction, CompactionSource, Profile, Role, TurnRecord};
use crate::models::{Proposal, ProposalStatus};
use crate::redact::{redact_json, redact_text};
use crate::time::{iso, parse_iso, utcnow};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS proposals (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    body        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS proposals_session ON proposals(session_id, status);

CREATE TABLE IF NOT EXISTS operations (
    proposal_id     TEXT PRIMARY KEY REFERENCES proposals(id),
    idempotency_key TEXT NOT NULL UNIQUE,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    status          TEXT,
    readback_version TEXT
);

CREATE TABLE IF NOT EXISTS operation_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal_id TEXT NOT NULL REFERENCES proposals(id),
    at          TEXT NOT NULL,
    event       TEXT NOT NULL,
    detail      TEXT
);

CREATE TABLE IF NOT EXISTS checkpoints (
    session_id  TEXT NOT NULL,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (session_id, key)
);

-- Agents (ADR 0014, ADR 0035): a mirror of each agent folder plus what only Ghost
-- writes (timestamps, retired), the transcript, every compaction version. ``conversation``
-- columns hold the agent's id. ``department`` is no longer read (ADR 0035); new rows
-- say 'ghost'.
CREATE TABLE IF NOT EXISTS conversations (
    id              TEXT PRIMARY KEY,
    slug            TEXT NOT NULL UNIQUE,
    department      TEXT NOT NULL,
    name            TEXT NOT NULL,
    aliases         TEXT NOT NULL DEFAULT '[]',
    job             TEXT NOT NULL DEFAULT '',
    voice           TEXT,
    speed           REAL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    last_active_at  TEXT,
    retired_at      TEXT,
    deleted_at      TEXT
);

CREATE TABLE IF NOT EXISTS turns (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation    TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    role            TEXT NOT NULL,
    text            TEXT NOT NULL,
    at              TEXT NOT NULL,
    compaction_id   TEXT,
    hidden_by       TEXT  -- the proposal marker of the reset/delete that hid it
);
CREATE INDEX IF NOT EXISTS turns_conversation ON turns(conversation, id);

CREATE TABLE IF NOT EXISTS compactions (
    id              TEXT PRIMARY KEY,
    conversation    TEXT NOT NULL,
    summary         TEXT NOT NULL,
    through_turn_id INTEGER,
    at              TEXT NOT NULL,
    source          TEXT NOT NULL DEFAULT 'auto'
);
CREATE INDEX IF NOT EXISTS compactions_conversation ON compactions(conversation, at);

-- What Ghost is tracking for the operator (ADR 0029): one row per stable key, from a
-- watch or from Ghost itself (a change waiting for a yes, a server down). ``fields`` are
-- the item's typed fields as JSON; untrusted data, never instructions.
CREATE TABLE IF NOT EXISTS items (
    key             TEXT PRIMARY KEY,
    source          TEXT NOT NULL,
    department      TEXT,
    title           TEXT NOT NULL,
    link            TEXT,
    fields          TEXT NOT NULL DEFAULT '{}',
    response        TEXT NOT NULL DEFAULT 'tell',
    first_seen      TEXT NOT NULL,
    last_seen       TEXT NOT NULL,
    changed_at      TEXT NOT NULL,
    resolved_at     TEXT,
    delivered_at    TEXT,
    delivered_via   TEXT,
    acknowledged_at TEXT,
    snoozed_until   TEXT
);
CREATE INDEX IF NOT EXISTS items_open ON items(resolved_at, source);

-- Webhook deliveries (ADR 0027): the id, so a replay is refused, and what came of it.
-- Never the body.
CREATE TABLE IF NOT EXISTS deliveries (
    source      TEXT NOT NULL,
    id          TEXT NOT NULL,
    at          TEXT NOT NULL,
    outcome     TEXT,
    item        TEXT,
    checked     INTEGER,
    PRIMARY KEY (source, id)
);
"#;

/// The three tables the lines (ADR 0014 to 0023) lived in. ADR 0024 started over: they
/// are moved aside under these names, never read again, for the operator to drop.
const LINE_TABLES: [&str; 3] = ["conversations", "turns", "compactions"];

const ITEM_COLS: &str = "key, source, title, link, fields, response, first_seen, last_seen, \
    changed_at, resolved_at, delivered_at, delivered_via, acknowledged_at, snoozed_until";
const PROFILE_COLS: &str = "id, slug, name, aliases, job, voice, speed, created_at, updated_at, \
    last_active_at, retired_at, deleted_at";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("a stored row does not parse: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// An item on the waiting list (ADR 0029), as the row holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub source: String,
    pub title: String,
    pub link: Option<String>,
    pub fields: Map<String, Value>,
    pub response: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub changed_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub delivered_via: Option<String>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub snoozed_until: Option<DateTime<Utc>>,
}

/// What `upsert_item` is told about an item seen now.
pub struct ItemSeen<'a> {
    pub key: &'a str,
    pub source: &'a str,
    pub title: &'a str,
    pub link: Option<&'a str>,
    pub fields: &'a Map<String, Value>,
    pub response: &'a str,
}

/// One of the columns `mark_item` may set on an open item.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemMark {
    DeliveredAt(Option<DateTime<Utc>>),
    DeliveredVia(Option<String>),
    AcknowledgedAt(Option<DateTime<Utc>>),
    SnoozedUntil(Option<DateTime<Utc>>),
}

/// Filters for the audit view.
#[derive(Debug, Clone, Default)]
pub struct ProposalQuery<'a> {
    pub limit: i64,
    pub offset: i64,
    pub ability: Option<&'a str>,
    pub status: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub conversation: Option<&'a str>,
    pub executed_only: bool,
}

impl ProposalQuery<'_> {
    pub fn page(limit: i64, offset: i64) -> Self {
        Self {
            limit,
            offset,
            ..Default::default()
        }
    }
}

/// An audit event on a proposal.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEvent {
    pub at: String,
    pub event: String,
    pub detail: Option<Value>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn opt_iso(dt: &Option<DateTime<Utc>>) -> Option<String> {
    dt.as_ref().map(iso)
}

fn col_dt(raw: Option<String>) -> Option<DateTime<Utc>> {
    raw.as_deref().and_then(parse_iso)
}

fn must_dt(raw: String) -> Result<DateTime<Utc>> {
    parse_iso(&raw).ok_or_else(|| StoreError::Invalid(format!("not a timestamp: {raw}")))
}

/// JSON with its object keys sorted at every level, as `json.dumps(sort_keys=True)`.
fn sorted_json(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut pairs: Vec<(&String, &Value)> = m.iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sorted_json(v)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted_json).collect()),
        other => other.clone(),
    }
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        retire_line_tables(&conn)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- proposals -----------------------------------------------------------------

    pub fn insert_proposal(&self, p: &Proposal) -> Result<()> {
        self.conn().execute(
            "INSERT INTO proposals (id, session_id, status, created_at, expires_at, body) VALUES (?, ?, ?, ?, ?, ?)",
            params![p.id, p.session_id, p.status.as_str(), iso(&p.created_at), iso(&p.expires_at), sealed(p)?],
        )?;
        Ok(())
    }

    pub fn get_proposal(&self, proposal_id: &str) -> Result<Option<Proposal>> {
        let row: Option<(String, String)> = self
            .conn()
            .query_row(
                "SELECT body, status FROM proposals WHERE id = ?",
                [proposal_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((body, status)) = row else {
            return Ok(None);
        };
        let mut p = load_proposal(&body)?;
        p.status = ProposalStatus::from_db(&status).unwrap_or(p.status);
        Ok(Some(p))
    }

    /// The change this session's next turn answers: pending and still attached.
    pub fn pending_for_session(&self, session_id: &str) -> Result<Option<Proposal>> {
        let body: Option<String> = self
            .conn()
            .query_row(
                "SELECT body FROM proposals WHERE session_id = ? AND status = ? AND attached = 1 ORDER BY created_at DESC LIMIT 1",
                params![session_id, ProposalStatus::Pending.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        body.map(|b| load_proposal(&b).map_err(StoreError::from))
            .transpose()
    }

    /// Every change waiting for a yes, attached or set aside, oldest first. A row from an
    /// older schema that doesn't parse is skipped.
    pub fn pending_proposals(&self) -> Result<Vec<Proposal>> {
        let conn = self.conn();
        let mut st =
            conn.prepare("SELECT body FROM proposals WHERE status = ? ORDER BY created_at")?;
        let rows = st.query_map([ProposalStatus::Pending.as_str()], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for body in rows {
            if let Ok(p) = load_proposal(&body?) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Set the session's pending changes aside: they stay pending, on the waiting list,
    /// and the session's next turn no longer answers them.
    pub fn detach(&self, session_id: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE proposals SET attached = 0 WHERE session_id = ? AND status = ? AND attached = 1",
            params![session_id, ProposalStatus::Pending.as_str()],
        )?)
    }

    /// Bring a pending change back to a session, the one its next turn answers. The
    /// session's own pending change, if any, is set aside first.
    pub fn attach(&self, proposal_id: &str, session_id: &str) -> Result<bool> {
        self.detach(session_id)?;
        let n = self.conn().execute(
            "UPDATE proposals SET attached = 1, session_id = ? WHERE id = ? AND status = ?",
            params![session_id, proposal_id, ProposalStatus::Pending.as_str()],
        )?;
        Ok(n == 1)
    }

    /// Newest first, for the audit view, with the total that matches the filters. Rows
    /// from an older schema are skipped.
    pub fn list_proposals(&self, q: &ProposalQuery<'_>) -> Result<(Vec<Proposal>, i64)> {
        let mut wheres: Vec<String> = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        if let Some(status) = q.status {
            let mut raw = vec![status.to_string()];
            if status == "executed" {
                raw.push("undone".into());
            }
            wheres.push(format!("status IN ({})", vec!["?"; raw.len()].join(", ")));
            args.extend(raw.into_iter().map(Value::String));
        }
        if q.executed_only {
            wheres.push("status = ?".into());
            args.push(Value::String(ProposalStatus::Executed.as_str().into()));
        }
        if let Some(s) = q.session_id {
            wheres.push("session_id = ?".into());
            args.push(Value::String(s.into()));
        }
        if let Some(a) = q.ability {
            wheres.push("json_extract(body, '$.ability') = ?".into());
            args.push(Value::String(a.into()));
        }
        if let Some(c) = q.conversation {
            wheres.push("json_extract(body, '$.conversation') = ?".into());
            args.push(Value::String(c.into()));
        }
        let clause = if wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", wheres.join(" AND "))
        };
        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM proposals{clause}"),
            params_from_iter(args.iter().map(json_param)),
            |r| r.get(0),
        )?;
        let mut st = conn.prepare(&format!(
            "SELECT body, status FROM proposals{clause} ORDER BY created_at DESC LIMIT ? OFFSET ?"
        ))?;
        let mut all: Vec<Value> = args.clone();
        all.push(Value::from(q.limit));
        all.push(Value::from(q.offset));
        let rows = st.query_map(params_from_iter(all.iter().map(json_param)), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (body, status) = row?;
            if let Ok(mut p) = load_proposal(&body) {
                p.status = ProposalStatus::from_db(&status).unwrap_or(p.status);
                out.push(p);
            }
        }
        Ok((out, total))
    }

    /// A waiting proposal leaves the list (cancelled, expired). False when it was not
    /// waiting: one claimed by a yes belongs to the gate until it finishes, and one that
    /// finished keeps its status, so a late cancel never writes over an executed row.
    pub fn set_status(&self, proposal_id: &str, status: ProposalStatus) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE proposals SET status = ? WHERE id = ? AND status = ?",
            params![
                status.as_str(),
                proposal_id,
                ProposalStatus::Pending.as_str()
            ],
        )?;
        Ok(n == 1)
    }

    pub fn cancel_pending(&self, session_id: &str, status: ProposalStatus) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE proposals SET status = ? WHERE session_id = ? AND status = ?",
            params![
                status.as_str(),
                session_id,
                ProposalStatus::Pending.as_str()
            ],
        )?)
    }

    /// Atomically move PENDING -> CLAIMED and open an operation row. False if the
    /// proposal was not pending (already claimed, cancelled, or expired). The UNIQUE
    /// idempotency key makes a double claim impossible even across processes.
    pub fn claim(&self, proposal_id: &str, now: Option<DateTime<Utc>>) -> Result<bool> {
        let now = now.unwrap_or_else(utcnow);
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE proposals SET status = ? WHERE id = ? AND status = ? AND expires_at > ?",
            params![
                ProposalStatus::Claimed.as_str(),
                proposal_id,
                ProposalStatus::Pending.as_str(),
                iso(&now)
            ],
        )?;
        if n != 1 {
            tx.rollback()?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO operations (proposal_id, idempotency_key, started_at) VALUES (?, ?, ?)",
            params![proposal_id, format!("ghost:{proposal_id}"), iso(&now)],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Close the claimed proposal's operation row. `final_status` is where the proposal
    /// ends: executed, or stale or cancelled when the gate refused it after the claim.
    pub fn finish_operation(
        &self,
        proposal_id: &str,
        status: &str,
        readback_version: Option<&str>,
        final_status: ProposalStatus,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE operations SET finished_at = ?, status = ?, readback_version = ? WHERE proposal_id = ?",
            params![iso(&utcnow()), status, readback_version, proposal_id],
        )?;
        conn.execute(
            "UPDATE proposals SET status = ? WHERE id = ? AND status = ?",
            params![
                final_status.as_str(),
                proposal_id,
                ProposalStatus::Claimed.as_str()
            ],
        )?;
        Ok(())
    }

    pub fn readback_version(&self, proposal_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT readback_version FROM operations WHERE proposal_id = ?",
                [proposal_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    // --- audit ---------------------------------------------------------------------

    pub fn event(&self, proposal_id: &str, event: &str, detail: Option<&Value>) -> Result<()> {
        let detail = detail
            .filter(|d| !is_empty_detail(d))
            .map(|d| redact_json(d).to_string());
        self.conn().execute(
            "INSERT INTO operation_events (proposal_id, at, event, detail) VALUES (?, ?, ?, ?)",
            params![proposal_id, iso(&utcnow()), event, detail],
        )?;
        Ok(())
    }

    pub fn events(&self, proposal_id: &str) -> Result<Vec<AuditEvent>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT at, event, detail FROM operation_events WHERE proposal_id = ? ORDER BY id",
        )?;
        let rows = st.query_map([proposal_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (at, event, detail) = row?;
            let detail = match detail {
                Some(d) if !d.is_empty() => Some(serde_json::from_str(&d)?),
                _ => None,
            };
            out.push(AuditEvent { at, event, detail });
        }
        Ok(out)
    }

    // --- items (ADR 0029) ----------------------------------------------------------

    pub fn get_item(&self, key: &str) -> Result<Option<Item>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {ITEM_COLS} FROM items WHERE key = ?"))?;
        let mut rows = st.query([key])?;
        match rows.next()? {
            Some(r) => Ok(Some(item(r)?)),
            None => Ok(None),
        }
    }

    /// Unresolved items, oldest first; one source's when `source` is given.
    pub fn open_items(&self, source: Option<&str>) -> Result<Vec<Item>> {
        let conn = self.conn();
        let (sql, args): (String, Vec<String>) = match source {
            Some(s) => (
                format!(
                    "SELECT {ITEM_COLS} FROM items WHERE resolved_at IS NULL AND source = ? ORDER BY first_seen"
                ),
                vec![s.to_string()],
            ),
            None => (
                format!(
                    "SELECT {ITEM_COLS} FROM items WHERE resolved_at IS NULL ORDER BY first_seen"
                ),
                vec![],
            ),
        };
        let mut st = conn.prepare(&sql)?;
        let mut rows = st.query(params_from_iter(args.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(item(r)?);
        }
        Ok(out)
    }

    /// Seen now. A new key, or one resolved before, starts over; `changed` moves
    /// `changed_at` so an item the operator acknowledged alerts again. The `department`
    /// column is left empty (ADR 0035).
    pub fn upsert_item(&self, seen: ItemSeen<'_>, now: DateTime<Utc>, changed: bool) -> Result<()> {
        let at = iso(&now);
        let fields = sorted_json(&redact_json(&Value::Object(seen.fields.clone()))).to_string();
        self.conn().execute(
            "INSERT INTO items (key, source, title, link, fields, response, first_seen, last_seen, changed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET source = excluded.source, department = NULL, title = excluded.title, \
             link = excluded.link, fields = excluded.fields, response = excluded.response, last_seen = excluded.last_seen, \
             first_seen = CASE WHEN items.resolved_at IS NULL THEN items.first_seen ELSE excluded.first_seen END, \
             changed_at = CASE WHEN ? OR items.resolved_at IS NOT NULL THEN excluded.changed_at ELSE items.changed_at END, \
             delivered_at = CASE WHEN items.resolved_at IS NULL THEN items.delivered_at END, \
             delivered_via = CASE WHEN items.resolved_at IS NULL THEN items.delivered_via END, \
             acknowledged_at = CASE WHEN items.resolved_at IS NULL THEN items.acknowledged_at END, \
             snoozed_until = CASE WHEN items.resolved_at IS NULL THEN items.snoozed_until END, \
             resolved_at = NULL",
            params![seen.key, seen.source, redact_text(seen.title), seen.link, fields, seen.response, at, at, at, changed as i32],
        )?;
        Ok(())
    }

    pub fn resolve_item(&self, key: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET resolved_at = ? WHERE key = ? AND resolved_at IS NULL",
            params![iso(&now), key],
        )?;
        Ok(())
    }

    /// Set delivery, acknowledgment or snooze columns on an open item. False when there
    /// is no such open item.
    pub fn mark_item(&self, key: &str, marks: &[ItemMark]) -> Result<bool> {
        if marks.is_empty() {
            return Err(StoreError::Invalid(
                "mark_item takes at least one column".into(),
            ));
        }
        let mut sets = Vec::new();
        let mut args: Vec<Option<String>> = Vec::new();
        for m in marks {
            let (col, val) = match m {
                ItemMark::DeliveredAt(v) => ("delivered_at", opt_iso(v)),
                ItemMark::DeliveredVia(v) => ("delivered_via", v.clone()),
                ItemMark::AcknowledgedAt(v) => ("acknowledged_at", opt_iso(v)),
                ItemMark::SnoozedUntil(v) => ("snoozed_until", opt_iso(v)),
            };
            sets.push(format!("{col} = ?"));
            args.push(val);
        }
        args.push(Some(key.to_string()));
        let n = self.conn().execute(
            &format!(
                "UPDATE items SET {} WHERE key = ? AND resolved_at IS NULL",
                sets.join(", ")
            ),
            params_from_iter(args.iter()),
        )?;
        Ok(n == 1)
    }

    // --- webhook deliveries (ADR 0027) ---------------------------------------------

    /// Record a delivery id; false when it was seen before (a replay). Ids older than
    /// `keep` are forgotten.
    pub fn first_delivery(
        &self,
        source: &str,
        delivery: &str,
        now: DateTime<Utc>,
        keep: Duration,
    ) -> Result<bool> {
        let conn = self.conn();
        conn.execute("DELETE FROM deliveries WHERE at < ?", [iso(&(now - keep))])?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO deliveries (source, id, at) VALUES (?, ?, ?)",
            params![source, delivery, iso(&now)],
        )?;
        Ok(n == 1)
    }

    /// Give an id back: nothing was decided (the source couldn't be read).
    pub fn forget_delivery(&self, source: &str, delivery: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM deliveries WHERE source = ? AND id = ?",
            params![source, delivery],
        )?;
        Ok(())
    }

    pub fn note_delivery(
        &self,
        source: &str,
        delivery: &str,
        outcome: &str,
        item: &str,
        checked: bool,
    ) -> Result<()> {
        let item = if item.is_empty() { None } else { Some(item) };
        self.conn().execute(
            "UPDATE deliveries SET outcome = ?, item = ?, checked = ? WHERE source = ? AND id = ?",
            params![outcome, item, checked as i32, source, delivery],
        )?;
        Ok(())
    }

    // --- per-source key/value ------------------------------------------------------

    pub fn set_checkpoint(&self, session_id: &str, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO checkpoints (session_id, key, value, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(session_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![session_id, key, value, iso(&utcnow())],
        )?;
        Ok(())
    }

    pub fn get_checkpoint(&self, session_id: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT value FROM checkpoints WHERE session_id = ? AND key = ?",
                params![session_id, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    // --- agents (ADR 0014, ADR 0035) -----------------------------------------------

    pub fn upsert_conversation(&self, p: &Profile) -> Result<()> {
        self.conn().execute(
            "INSERT INTO conversations (id, slug, department, name, aliases, job, voice, speed, created_at, updated_at, \
             last_active_at, retired_at, deleted_at) VALUES (?, ?, 'ghost', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET slug = excluded.slug, department = 'ghost', name = excluded.name, \
             aliases = excluded.aliases, job = excluded.job, voice = excluded.voice, speed = excluded.speed, \
             updated_at = excluded.updated_at, last_active_at = excluded.last_active_at, \
             retired_at = excluded.retired_at, deleted_at = excluded.deleted_at",
            params![
                p.id,
                p.slug,
                p.name,
                serde_json::to_string(&p.aliases)?,
                p.job,
                p.voice,
                p.speed,
                iso(&p.created_at),
                iso(&p.updated_at),
                opt_iso(&p.last_active_at),
                opt_iso(&p.retired_at),
                opt_iso(&p.deleted_at),
            ],
        )?;
        Ok(())
    }

    pub fn get_conversation(&self, slug: &str, include_deleted: bool) -> Result<Option<Profile>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {PROFILE_COLS} FROM conversations WHERE slug = ?"
        ))?;
        let mut rows = st.query([slug])?;
        let Some(r) = rows.next()? else {
            return Ok(None);
        };
        let p = profile(r)?;
        if p.deleted_at.is_some() && !include_deleted {
            return Ok(None);
        }
        Ok(Some(p))
    }

    pub fn get_conversation_by_id(&self, id: &str) -> Result<Option<Profile>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {PROFILE_COLS} FROM conversations WHERE id = ?"
        ))?;
        let mut rows = st.query([id])?;
        match rows.next()? {
            Some(r) => Ok(Some(profile(r)?)),
            None => Ok(None),
        }
    }

    pub fn list_conversations(
        &self,
        include_retired: bool,
        include_deleted: bool,
    ) -> Result<Vec<Profile>> {
        let mut wheres = vec![if include_deleted {
            "1 = 1"
        } else {
            "deleted_at IS NULL"
        }];
        if !include_retired {
            wheres.push("retired_at IS NULL");
        }
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {PROFILE_COLS} FROM conversations WHERE {} ORDER BY CASE WHEN slug = 'ghost' THEN 0 ELSE 1 END, lower(name)",
            wheres.join(" AND ")
        ))?;
        let mut rows = st.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(profile(r)?);
        }
        Ok(out)
    }

    /// Forget a mirror row whose folder is gone and whose history is empty.
    pub fn delete_conversation_row(&self, id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM conversations WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn touch_conversation(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE conversations SET last_active_at = ? WHERE id = ?",
            params![iso(&utcnow()), id],
        )?;
        Ok(())
    }

    pub fn add_turn(
        &self,
        conversation: &str,
        session_id: &str,
        role: Role,
        text: &str,
    ) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO turns (conversation, session_id, role, text, at) VALUES (?, ?, ?, ?, ?)",
            params![
                conversation,
                session_id,
                role.as_str(),
                redact_text(text),
                iso(&utcnow())
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Oldest first. `uncompacted` restricts to turns no summary covers yet.
    pub fn recent_turns(
        &self,
        conversation: &str,
        limit: i64,
        uncompacted: bool,
    ) -> Result<Vec<TurnRecord>> {
        let extra = if uncompacted {
            " AND compaction_id IS NULL"
        } else {
            ""
        };
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT id, conversation, session_id, role, text, at, compaction_id FROM turns \
             WHERE conversation = ? AND hidden_by IS NULL{extra} ORDER BY id DESC LIMIT ?"
        ))?;
        let mut rows = st.query(params![conversation, limit])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(turn(r)?);
        }
        out.reverse();
        Ok(out)
    }

    /// Carry this session's turns from one agent to another (save-as, a split move).
    pub fn move_session_turns(&self, session_id: &str, src: &str, dst: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE turns SET conversation = ? WHERE session_id = ? AND conversation = ? AND hidden_by IS NULL AND compaction_id IS NULL",
            params![dst, session_id, src],
        )?)
    }

    pub fn turn_count(&self, conversation: &str) -> Result<i64> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) FROM turns WHERE conversation = ? AND hidden_by IS NULL",
            [conversation],
            |r| r.get(0),
        )?)
    }

    pub fn last_turn_id(&self, conversation: &str) -> Result<Option<i64>> {
        Ok(self.conn().query_row(
            "SELECT MAX(id) FROM turns WHERE conversation = ? AND hidden_by IS NULL",
            [conversation],
            |r| r.get::<_, Option<i64>>(0),
        )?)
    }

    pub fn mark_compacted(
        &self,
        conversation: &str,
        through_turn_id: i64,
        compaction_id: &str,
    ) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE turns SET compaction_id = ? WHERE conversation = ? AND id <= ? AND compaction_id IS NULL AND hidden_by IS NULL",
            params![compaction_id, conversation, through_turn_id],
        )?)
    }

    /// Hide every turn (a reset or delete). Hidden turns are pruned with the rest.
    pub fn tag_turns(&self, conversation: &str, marker: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE turns SET hidden_by = ? WHERE conversation = ? AND hidden_by IS NULL",
            params![marker, conversation],
        )?)
    }

    /// Summarized or hidden turns older than `before` are dropped; the summary stands in
    /// for them.
    pub fn prune_turns(&self, before: DateTime<Utc>) -> Result<usize> {
        Ok(self.conn().execute("DELETE FROM turns WHERE at < ? AND (compaction_id IS NOT NULL OR hidden_by IS NOT NULL)", [iso(&before)])?)
    }

    pub fn add_compaction(&self, c: &Compaction) -> Result<()> {
        self.conn().execute(
            "INSERT INTO compactions (id, conversation, summary, through_turn_id, at, source) VALUES (?, ?, ?, ?, ?, ?)",
            params![c.id, c.conversation, redact_text(&c.summary), c.through_turn_id, iso(&c.at), c.source.as_str()],
        )?;
        Ok(())
    }

    pub fn compactions(&self, conversation: &str, limit: i64) -> Result<Vec<Compaction>> {
        let conn = self.conn();
        let mut st = conn.prepare("SELECT id, conversation, summary, through_turn_id, at, source FROM compactions WHERE conversation = ? ORDER BY at DESC LIMIT ?")?;
        let rows = st.query_map(params![conversation, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, conversation, summary, through_turn_id, at, source) = row?;
            out.push(Compaction {
                id,
                conversation,
                summary,
                through_turn_id,
                at: must_dt(at)?,
                source: CompactionSource::parse(&source).unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

fn json_param(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Number(n) if n.is_i64() => {
            rusqlite::types::Value::Integer(n.as_i64().unwrap_or_default())
        }
        Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or_default()),
        Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
        Value::Null => rusqlite::types::Value::Null,
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

fn is_empty_detail(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.is_empty(),
        Value::Null => true,
        _ => false,
    }
}

/// A proposal's stored body with every password replaced (ADR 0036): the audit never
/// holds one, even where the operator dictated it into a change.
fn sealed(p: &Proposal) -> Result<String> {
    let body: Value = serde_json::from_str(&dump_proposal(p))?;
    Ok(redact_json(&body).to_string())
}

fn item(r: &rusqlite::Row<'_>) -> Result<Item> {
    let fields: String = r.get(4)?;
    let fields =
        match serde_json::from_str::<Value>(if fields.is_empty() { "{}" } else { &fields })? {
            Value::Object(m) => m,
            _ => Map::new(),
        };
    Ok(Item {
        key: r.get(0)?,
        source: r.get(1)?,
        title: r.get(2)?,
        link: r.get(3)?,
        fields,
        response: r.get(5)?,
        first_seen: must_dt(r.get(6)?)?,
        last_seen: must_dt(r.get(7)?)?,
        changed_at: must_dt(r.get(8)?)?,
        resolved_at: col_dt(r.get(9)?),
        delivered_at: col_dt(r.get(10)?),
        delivered_via: r.get(11)?,
        acknowledged_at: col_dt(r.get(12)?),
        snoozed_until: col_dt(r.get(13)?),
    })
}

fn profile(r: &rusqlite::Row<'_>) -> Result<Profile> {
    let aliases: Option<String> = r.get(3)?;
    let aliases: Vec<String> =
        serde_json::from_str(aliases.as_deref().filter(|s| !s.is_empty()).unwrap_or("[]"))?;
    Ok(Profile {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        aliases,
        job: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        voice: r.get(5)?,
        speed: r.get(6)?,
        created_at: must_dt(r.get(7)?)?,
        updated_at: must_dt(r.get(8)?)?,
        last_active_at: col_dt(r.get(9)?),
        retired_at: col_dt(r.get(10)?),
        deleted_at: col_dt(r.get(11)?),
    })
}

fn turn(r: &rusqlite::Row<'_>) -> Result<TurnRecord> {
    let role: String = r.get(3)?;
    Ok(TurnRecord {
        id: r.get(0)?,
        conversation: r.get(1)?,
        session_id: r.get(2)?,
        role: Role::parse(&role)
            .ok_or_else(|| StoreError::Invalid(format!("not a role: {role}")))?,
        text: r.get(4)?,
        at: must_dt(r.get(5)?)?,
        compaction_id: r.get(6)?,
    })
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = st.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// A database from before ADR 0024 keeps its lines in tables keyed by slug. They are
/// renamed `retired_*` so the new layout starts empty, as the ADR says, and nothing of
/// the operator's is deleted by code.
fn retire_line_tables(conn: &Connection) -> Result<()> {
    let cols = columns(conn, "conversations")?;
    if cols.is_empty() || cols.iter().any(|c| c == "id") {
        return Ok(());
    }
    let stamp = utcnow().format("%Y%m%d").to_string();
    for table in LINE_TABLES {
        conn.execute(
            &format!("ALTER TABLE {table} RENAME TO retired_{table}_{stamp}"),
            [],
        )?;
    }
    for index in ["turns_conversation", "compactions_conversation"] {
        conn.execute(&format!("DROP INDEX IF EXISTS {index}"), [])?;
    }
    warn!(
        "store: the lines of the earlier layout were moved aside as retired_* tables (ADR 0024); agents start fresh"
    );
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    if !columns(conn, "operations")?
        .iter()
        .any(|c| c == "readback_version")
    {
        conn.execute(
            "ALTER TABLE operations ADD COLUMN readback_version TEXT",
            [],
        )?;
    }
    // ADR 0030: a pending proposal answers the session's next turn only while attached;
    // set aside, it waits on the waiting list for a yes by id.
    if !columns(conn, "proposals")?.iter().any(|c| c == "attached") {
        conn.execute(
            "ALTER TABLE proposals ADD COLUMN attached INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;
    use serde_json::json;

    fn proposal(session: &str) -> Proposal {
        Proposal::new(Draft {
            session_id: session.into(),
            ability: "github_issues".into(),
            operation: "close".into(),
            target: Target::FakeIssue(FakeIssue {
                owner: "acme".into(),
                repo: "app".into(),
                number: 7,
            }),
            changes: ChangeSet::one(Change::SetState(SetState {
                state: "closed".into(),
            })),
            rationale: "asked".into(),
            snapshot: Snapshot::FakeIssue(FakeIssueSnap {
                version: "v1".into(),
                title: "T".into(),
                state: "open".into(),
                labels: vec![],
            }),
            preview: "close it".into(),
        })
    }

    #[test]
    fn a_proposal_round_trips_and_its_status_comes_from_the_column() {
        let store = Store::in_memory().unwrap();
        let p = proposal("s1");
        store.insert_proposal(&p).unwrap();
        assert_eq!(store.get_proposal(&p.id).unwrap().unwrap(), p);
        assert_eq!(store.pending_for_session("s1").unwrap().unwrap().id, p.id);
        assert!(store.set_status(&p.id, ProposalStatus::Cancelled).unwrap());
        assert_eq!(
            store.get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Cancelled
        );
        assert!(
            !store.set_status(&p.id, ProposalStatus::Expired).unwrap(),
            "only a pending row changes"
        );
        assert!(store.pending_for_session("s1").unwrap().is_none());
    }

    #[test]
    fn claim_is_atomic_and_finishes_once() {
        let store = Store::in_memory().unwrap();
        let p = proposal("s1");
        store.insert_proposal(&p).unwrap();
        assert!(store.claim(&p.id, None).unwrap());
        assert!(
            !store.claim(&p.id, None).unwrap(),
            "a second claim finds it claimed"
        );
        assert_eq!(
            store.get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Claimed
        );
        store
            .finish_operation(&p.id, "success", Some("v2"), ProposalStatus::Executed)
            .unwrap();
        assert_eq!(
            store.get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Executed
        );
        assert_eq!(
            store.readback_version(&p.id).unwrap().as_deref(),
            Some("v2")
        );
        let expired = Proposal {
            expires_at: utcnow() - Duration::seconds(1),
            ..proposal("s2")
        };
        store.insert_proposal(&expired).unwrap();
        assert!(
            !store.claim(&expired.id, None).unwrap(),
            "an expired proposal cannot be claimed"
        );
    }

    #[test]
    fn detach_and_attach_move_a_pending_change_between_the_session_and_the_list() {
        let store = Store::in_memory().unwrap();
        let p = proposal("s1");
        store.insert_proposal(&p).unwrap();
        assert_eq!(store.detach("s1").unwrap(), 1);
        assert!(store.pending_for_session("s1").unwrap().is_none());
        assert_eq!(store.pending_proposals().unwrap().len(), 1);
        assert!(store.attach(&p.id, "s2").unwrap());
        assert_eq!(store.pending_for_session("s2").unwrap().unwrap().id, p.id);
        assert_eq!(
            store
                .cancel_pending("s2", ProposalStatus::Cancelled)
                .unwrap(),
            1
        );
    }

    #[test]
    fn the_audit_lists_newest_first_with_filters_and_redacts_details() {
        let store = Store::in_memory().unwrap();
        let a = proposal("s1");
        let mut b = proposal("s2");
        b.conversation = Some("ledger".into());
        b.created_at = a.created_at + Duration::seconds(1);
        store.insert_proposal(&a).unwrap();
        store.insert_proposal(&b).unwrap();
        let (rows, total) = store.list_proposals(&ProposalQuery::page(10, 0)).unwrap();
        assert_eq!(total, 2);
        assert_eq!(rows[0].id, b.id);
        let (rows, total) = store
            .list_proposals(&ProposalQuery {
                conversation: Some("ledger"),
                ..ProposalQuery::page(10, 0)
            })
            .unwrap();
        assert_eq!((rows.len(), total), (1, 1));
        let (rows, _) = store
            .list_proposals(&ProposalQuery {
                ability: Some("nothing"),
                ..ProposalQuery::page(10, 0)
            })
            .unwrap();
        assert!(rows.is_empty());
        crate::redact::also_redact(
            "test-store",
            [("STORE_TOKEN".to_string(), "storetokenvalue".to_string())],
        );
        store
            .event(
                &a.id,
                "user_turn",
                Some(&json!({"text": "say storetokenvalue", "classifier": "jev"})),
            )
            .unwrap();
        store.event(&a.id, "empty", Some(&json!({}))).unwrap();
        let events = store.events(&a.id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "user_turn");
        assert_eq!(
            events[0].detail.as_ref().unwrap()["text"],
            "say [password: STORE_TOKEN]"
        );
        assert_eq!(events[1].detail, None);
    }

    #[test]
    fn legacy_rows_read_back() {
        let store = Store::in_memory().unwrap();
        let p = proposal("s1");
        store.insert_proposal(&p).unwrap();
        store
            .conn()
            .execute(
                "UPDATE proposals SET status = 'undone' WHERE id = ?",
                [&p.id],
            )
            .unwrap();
        assert_eq!(
            store.get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Executed
        );
        let (rows, _) = store
            .list_proposals(&ProposalQuery {
                status: Some("executed"),
                ..ProposalQuery::page(10, 0)
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn items_are_seen_resolved_marked_and_start_over() {
        let store = Store::in_memory().unwrap();
        let now = utcnow();
        let fields = Map::from_iter([
            ("state".to_string(), json!("red")),
            ("a".to_string(), json!(1)),
        ]);
        let seen = || ItemSeen {
            key: "ci:main",
            source: "ci",
            title: "main is red",
            link: None,
            fields: &fields,
            response: "tell",
        };
        store.upsert_item(seen(), now, false).unwrap();
        let it = store.get_item("ci:main").unwrap().unwrap();
        assert_eq!(it.fields["state"], "red");
        assert_eq!(it.first_seen, now);
        assert!(
            store
                .mark_item(
                    "ci:main",
                    &[
                        ItemMark::DeliveredAt(Some(now)),
                        ItemMark::DeliveredVia(Some("desk".into()))
                    ]
                )
                .unwrap()
        );
        assert_eq!(
            store
                .get_item("ci:main")
                .unwrap()
                .unwrap()
                .delivered_via
                .as_deref(),
            Some("desk")
        );
        let later = now + Duration::seconds(5);
        store.upsert_item(seen(), later, true).unwrap();
        let it = store.get_item("ci:main").unwrap().unwrap();
        assert_eq!(it.first_seen, now, "still the first sighting");
        assert_eq!(it.changed_at, later, "changed moves changed_at");
        assert_eq!(
            it.delivered_via.as_deref(),
            Some("desk"),
            "delivery kept while open"
        );
        store.resolve_item("ci:main", later).unwrap();
        assert!(store.open_items(None).unwrap().is_empty());
        assert!(
            !store
                .mark_item("ci:main", &[ItemMark::AcknowledgedAt(Some(later))])
                .unwrap()
        );
        let again = later + Duration::seconds(5);
        store.upsert_item(seen(), again, false).unwrap();
        let it = store.get_item("ci:main").unwrap().unwrap();
        assert_eq!(it.first_seen, again, "a resolved item starts over");
        assert_eq!(it.delivered_via, None);
        assert_eq!(store.open_items(Some("ci")).unwrap().len(), 1);
        assert!(store.mark_item("ci:main", &[]).is_err());
    }

    #[test]
    fn deliveries_refuse_replays_and_forget_old_ids() {
        let store = Store::in_memory().unwrap();
        let now = utcnow();
        assert!(
            store
                .first_delivery("github", "d1", now, Duration::hours(1))
                .unwrap()
        );
        assert!(
            !store
                .first_delivery("github", "d1", now, Duration::hours(1))
                .unwrap()
        );
        store
            .note_delivery("github", "d1", "item", "ci:main", true)
            .unwrap();
        store.forget_delivery("github", "d1").unwrap();
        assert!(
            store
                .first_delivery("github", "d1", now, Duration::hours(1))
                .unwrap()
        );
        assert!(
            store
                .first_delivery("github", "d1", now + Duration::hours(2), Duration::hours(1))
                .unwrap(),
            "an old id is forgotten"
        );
        store.set_checkpoint("s1", "k", "v").unwrap();
        store.set_checkpoint("s1", "k", "v2").unwrap();
        assert_eq!(
            store.get_checkpoint("s1", "k").unwrap().as_deref(),
            Some("v2")
        );
        assert_eq!(store.get_checkpoint("s1", "nope").unwrap(), None);
    }

    #[test]
    fn agents_turns_and_compactions_keep_their_history() {
        let store = Store::in_memory().unwrap();
        let mut ghost = Profile::new("ghost", "Ghost");
        let mut ledger = Profile::new("ledger", "Ledger");
        ledger.aliases = vec!["books".into()];
        ledger.speed = Some(1.1);
        store.upsert_conversation(&ledger).unwrap();
        store.upsert_conversation(&ghost).unwrap();
        let listed = store.list_conversations(false, false).unwrap();
        assert_eq!(
            listed.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            vec!["ghost", "ledger"],
            "ghost first"
        );
        assert_eq!(
            store.get_conversation("ledger", false).unwrap().unwrap(),
            ledger
        );
        ledger.retired_at = Some(utcnow());
        store.upsert_conversation(&ledger).unwrap();
        assert_eq!(store.list_conversations(false, false).unwrap().len(), 1);
        assert_eq!(store.list_conversations(true, false).unwrap().len(), 2);
        ghost.deleted_at = Some(utcnow());
        store.upsert_conversation(&ghost).unwrap();
        assert!(store.get_conversation("ghost", false).unwrap().is_none());
        assert!(store.get_conversation("ghost", true).unwrap().is_some());
        assert_eq!(
            store
                .get_conversation_by_id(&ledger.id)
                .unwrap()
                .unwrap()
                .slug,
            "ledger"
        );

        let t1 = store
            .add_turn(&ledger.id, "s1", Role::User, "hello")
            .unwrap();
        let t2 = store
            .add_turn(&ledger.id, "s1", Role::Assistant, "hi")
            .unwrap();
        assert_eq!(store.turn_count(&ledger.id).unwrap(), 2);
        assert_eq!(store.last_turn_id(&ledger.id).unwrap(), Some(t2));
        let turns = store.recent_turns(&ledger.id, 10, false).unwrap();
        assert_eq!(
            turns.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![t1, t2],
            "oldest first"
        );
        assert_eq!(turns[0].role, Role::User);
        let c = Compaction {
            id: "c1".into(),
            conversation: ledger.id.clone(),
            summary: "they said hello".into(),
            through_turn_id: Some(t1),
            at: utcnow(),
            source: CompactionSource::Auto,
        };
        store.add_compaction(&c).unwrap();
        assert_eq!(store.mark_compacted(&ledger.id, t1, "c1").unwrap(), 1);
        assert_eq!(
            store.recent_turns(&ledger.id, 10, true).unwrap().len(),
            1,
            "one turn is still uncompacted"
        );
        assert_eq!(
            store.compactions(&ledger.id, 10).unwrap()[0].summary,
            "they said hello"
        );
        assert_eq!(
            store.move_session_turns("s1", &ledger.id, "other").unwrap(),
            1,
            "only the uncompacted turn moves"
        );
        assert_eq!(store.tag_turns(&ledger.id, "ghost:reset").unwrap(), 1);
        assert_eq!(store.turn_count(&ledger.id).unwrap(), 0);
        assert_eq!(
            store.prune_turns(utcnow() + Duration::seconds(1)).unwrap(),
            1,
            "the hidden turn is pruned"
        );
        store.touch_conversation(&ledger.id).unwrap();
        assert!(
            store
                .get_conversation_by_id(&ledger.id)
                .unwrap()
                .unwrap()
                .last_active_at
                .is_some()
        );
        store.delete_conversation_row(&ghost.id).unwrap();
        assert!(store.get_conversation("ghost", true).unwrap().is_none());
    }

    #[test]
    fn a_database_from_before_the_agents_layout_is_moved_aside_and_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE conversations (slug TEXT PRIMARY KEY, name TEXT);
                 CREATE TABLE turns (id INTEGER PRIMARY KEY, conversation TEXT);
                 CREATE TABLE compactions (id TEXT PRIMARY KEY, conversation TEXT);
                 CREATE INDEX turns_conversation ON turns(conversation);
                 CREATE TABLE proposals (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, body TEXT NOT NULL);
                 CREATE TABLE operations (proposal_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, started_at TEXT NOT NULL, finished_at TEXT, status TEXT);
                 INSERT INTO conversations VALUES ('old', 'Old line');",
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(
            store.list_conversations(true, true).unwrap().is_empty(),
            "the new layout starts empty"
        );
        let conn = store.conn();
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'retired_conversations_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "nothing of the operator's is deleted");
        assert!(
            columns(&conn, "proposals")
                .unwrap()
                .iter()
                .any(|c| c == "attached")
        );
        assert!(
            columns(&conn, "operations")
                .unwrap()
                .iter()
                .any(|c| c == "readback_version")
        );
    }
}
