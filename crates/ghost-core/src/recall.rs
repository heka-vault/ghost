//! Recall (ADR 0041, ADR 0052): a full-text index over the stored turns, so an agent can
//! find what was said weeks ago and not only what its summary kept.
//!
//! The index is SQLite's FTS5 (compiled into the bundled build), an external-content table
//! over `turns.text` keyed on the turn's id and kept in step by triggers: a turn added,
//! changed or deleted (compacted turns stay until they are pruned, then go) is in the index
//! or out of it in the same statement. A database from before this opens with the index
//! built once from what is there. No model, no embeddings, no new dependency.
//!
//! What is in `turns` is a session's transcript (redacted before it is written), a
//! scheduled run's and an asked turn's. Passive audio and passive transcripts are never
//! stored, so they can never be found here. Hidden turns (a reset or a delete) are left out
//! of every result.
//!
//! The words of a query arrive from a model, often from speech: quotes, colons, hyphens and
//! `*` are FTS5's syntax, so a query is cut into plain words by code and each is matched as
//! a quoted term (a prefix of a word counts). Every word must match; when nothing does,
//! any word may.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params_from_iter};

use crate::conversations::models::{Role, TurnRecord};
use crate::store::{Result, Store, StoreError};
use crate::time::{iso, parse_iso};

/// The index and its triggers. `IF NOT EXISTS` throughout: run on every open.
pub(crate) const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
    text, content='turns', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS turns_fts_insert AFTER INSERT ON turns BEGIN
    INSERT INTO turns_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS turns_fts_delete AFTER DELETE ON turns BEGIN
    INSERT INTO turns_fts(turns_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS turns_fts_update AFTER UPDATE OF text ON turns BEGIN
    INSERT INTO turns_fts(turns_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO turns_fts(rowid, text) VALUES (new.id, new.text);
END;
"#;

/// The most words a query keeps.
pub const QUERY_WORDS: usize = 8;
/// The most hits one search returns.
pub const LIMIT_MAX: i64 = 20;

/// Make the index when the database has none, and fill it once from the turns already
/// there.
pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    let had: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'turns_fts'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    conn.execute_batch(FTS_SCHEMA)?;
    if !had {
        conn.execute("INSERT INTO turns_fts(turns_fts) VALUES ('rebuild')", [])?;
    }
    Ok(())
}

/// A query as FTS5 terms: plain words, each quoted, a prefix allowed. None when nothing
/// in it is a word.
pub fn fts_terms(query: &str) -> Option<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    for w in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !words.iter().any(|x| x == w) {
            words.push(w.to_string());
        }
        if words.len() == QUERY_WORDS {
            break;
        }
    }
    (!words.is_empty()).then(|| words.into_iter().map(|w| format!("\"{w}\"*")).collect())
}

/// Which turns a search may see: the agents (by id), and for an outside client only the
/// sessions of the runs it started.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// None: every agent's.
    pub conversations: Option<Vec<String>>,
    /// Session ids matched with GLOB; None: any session.
    pub sessions: Option<Vec<String>>,
    pub since: Option<DateTime<Utc>>,
    /// Only turns before this one (paging back through a transcript).
    pub before: Option<i64>,
}

/// One hit: the turn, and the snippet around what matched (`[` and `]` mark it).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub turn: TurnRecord,
    pub snippet: String,
}

fn turn_of(r: &rusqlite::Row<'_>) -> Result<TurnRecord> {
    let role: String = r.get(3)?;
    let at: String = r.get(5)?;
    Ok(TurnRecord {
        id: r.get(0)?,
        conversation: r.get(1)?,
        session_id: r.get(2)?,
        role: Role::parse(&role)
            .ok_or_else(|| StoreError::Invalid(format!("not a role: {role}")))?,
        text: r.get(4)?,
        at: parse_iso(&at).ok_or_else(|| StoreError::Invalid(format!("not a timestamp: {at}")))?,
        compaction_id: r.get(6)?,
    })
}

/// The `WHERE` of a scope over `turns` as `t`, and its values.
fn scoped(scope: &Scope) -> (String, Vec<String>) {
    let mut sql = String::from("t.hidden_by IS NULL");
    let mut values: Vec<String> = Vec::new();
    if let Some(ids) = &scope.conversations {
        if ids.is_empty() {
            sql.push_str(" AND 0");
        } else {
            sql.push_str(&format!(
                " AND t.conversation IN ({})",
                vec!["?"; ids.len()].join(", ")
            ));
            values.extend(ids.iter().cloned());
        }
    }
    if let Some(globs) = &scope.sessions {
        if globs.is_empty() {
            sql.push_str(" AND 0");
        } else {
            let any: Vec<&str> = globs.iter().map(|_| "t.session_id GLOB ?").collect();
            sql.push_str(&format!(" AND ({})", any.join(" OR ")));
            values.extend(globs.iter().cloned());
        }
    }
    if let Some(since) = &scope.since {
        sql.push_str(" AND t.at >= ?");
        values.push(iso(since));
    }
    if let Some(before) = scope.before {
        sql.push_str(" AND t.id < ?");
        values.push(before.to_string());
    }
    (sql, values)
}

const COLS: &str = "t.id, t.conversation, t.session_id, t.role, t.text, t.at, t.compaction_id";

impl Store {
    /// The best turns for `query` within `scope`, best first: every word, else any.
    pub fn search_turns(&self, query: &str, scope: &Scope, limit: i64) -> Result<Vec<Hit>> {
        let Some(terms) = fts_terms(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, LIMIT_MAX);
        let hits = self.search_with(&terms.join(" AND "), scope, limit)?;
        if !hits.is_empty() || terms.len() == 1 {
            return Ok(hits);
        }
        self.search_with(&terms.join(" OR "), scope, limit)
    }

    fn search_with(&self, matched: &str, scope: &Scope, limit: i64) -> Result<Vec<Hit>> {
        let (filter, values) = scoped(scope);
        let sql = format!(
            "SELECT {COLS}, snippet(turns_fts, 0, '[', ']', '…', 24) FROM turns_fts \
             JOIN turns t ON t.id = turns_fts.rowid \
             WHERE turns_fts MATCH ? AND {filter} ORDER BY bm25(turns_fts), t.id DESC LIMIT ?"
        );
        let conn = self.conn();
        let mut st = conn.prepare(&sql)?;
        let mut all: Vec<String> = vec![matched.to_string()];
        all.extend(values);
        all.push(limit.to_string());
        let mut rows = st.query(params_from_iter(all.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(Hit {
                turn: turn_of(r)?,
                snippet: r.get(7)?,
            });
        }
        Ok(out)
    }

    /// The turns either side of one, in the same session, oldest first.
    pub fn turns_around(&self, turn: &TurnRecord, each_side: i64) -> Result<Vec<TurnRecord>> {
        let conn = self.conn();
        let mut out = Vec::new();
        for (cmp, order) in [("<", "DESC"), (">", "ASC")] {
            let mut st = conn.prepare(&format!(
                "SELECT {COLS} FROM turns t WHERE t.conversation = ? AND t.session_id = ? \
                 AND t.hidden_by IS NULL AND t.id {cmp} ? ORDER BY t.id {order} LIMIT ?"
            ))?;
            let mut rows = st.query(rusqlite::params![
                turn.conversation,
                turn.session_id,
                turn.id,
                each_side
            ])?;
            let mut side = Vec::new();
            while let Some(r) = rows.next()? {
                side.push(turn_of(r)?);
            }
            if order == "DESC" {
                side.reverse();
            }
            out.extend(side);
        }
        Ok(out)
    }

    /// The newest turns within `scope`, newest first.
    pub fn page_turns(&self, scope: &Scope, limit: i64) -> Result<Vec<TurnRecord>> {
        let (filter, values) = scoped(scope);
        let sql = format!("SELECT {COLS} FROM turns t WHERE {filter} ORDER BY t.id DESC LIMIT ?");
        let conn = self.conn();
        let mut st = conn.prepare(&sql)?;
        let mut all = values;
        all.push(limit.clamp(1, 200).to_string());
        let mut rows = st.query(params_from_iter(all.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(turn_of(r)?);
        }
        Ok(out)
    }

    /// The sessions of the runs of the schedules `created_by` set up (`client:<name>`),
    /// newest first: what an outside client may recall besides its own asked turns.
    pub fn run_sessions_made_by(&self, created_by: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT r.id FROM schedule_runs r JOIN schedules s ON s.id = r.job \
             WHERE s.created_by = ? ORDER BY r.scheduled_for DESC LIMIT 500",
        )?;
        let ids = st.query_map([created_by], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for id in ids {
            let id = id?;
            // A GLOB pattern: an id is letters, digits, - and _, or it is left out.
            if id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                out.push(crate::schedules::run_session(&id));
            }
        }
        Ok(out)
    }

    /// How many turns the index holds: for the tests that keep it in step.
    pub fn indexed_turns(&self) -> Result<i64> {
        Ok(self
            .conn()
            .query_row("SELECT COUNT(*) FROM turns_fts_docsize", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::conversations::models::Profile;
    use crate::time::utcnow;

    fn store_with_agents() -> (Store, String, String) {
        let store = Store::in_memory().unwrap();
        let ledger = Profile::new("ledger", "Ledger");
        let brand = Profile::new("brand", "Brand");
        store.upsert_conversation(&ledger).unwrap();
        store.upsert_conversation(&brand).unwrap();
        (store, ledger.id, brand.id)
    }

    fn mine(id: &str) -> Scope {
        Scope {
            conversations: Some(vec![id.to_string()]),
            ..Scope::default()
        }
    }

    #[test]
    fn a_query_is_plain_words_and_never_fts_syntax() {
        assert_eq!(
            fts_terms("turn-grace: \"August\" OR *x*").unwrap(),
            vec![
                "\"turn\"*",
                "\"grace\"*",
                "\"august\"*",
                "\"or\"*",
                "\"x\"*"
            ]
        );
        assert!(fts_terms("  -- :: ").is_none());
        assert_eq!(fts_terms(&"w ".repeat(40)).unwrap().len(), 1);
    }

    #[test]
    fn the_index_follows_turns_added_compacted_and_pruned() {
        let (store, ledger, brand) = store_with_agents();
        let t1 = store
            .add_turn(
                &ledger,
                "s1",
                Role::User,
                "what about the turn grace in August",
            )
            .unwrap();
        store
            .add_turn(
                &ledger,
                "s1",
                Role::Assistant,
                "We set the grace to half a second.",
            )
            .unwrap();
        store
            .add_turn(&brand, "s2", Role::User, "the logo needs more grace")
            .unwrap();
        // Added: found at once, scoped to the agent.
        let hits = store.search_turns("turn grace", &mine(&ledger), 5).unwrap();
        assert_eq!(hits[0].turn.id, t1);
        assert!(hits[0].snippet.contains("[grace]"), "{}", hits[0].snippet);
        let hits = store.search_turns("grace", &mine(&ledger), 5).unwrap();
        assert_eq!(hits.len(), 2, "only Ledger's");
        assert_eq!(
            store
                .search_turns("grace", &Scope::default(), 5)
                .unwrap()
                .len(),
            3
        );
        // Every word, else any.
        assert_eq!(
            store
                .search_turns("logo zebra", &Scope::default(), 5)
                .unwrap()
                .len(),
            1
        );
        // Compacted: still there until pruned.
        store.mark_compacted(&ledger, t1, "c1").unwrap();
        assert_eq!(
            store
                .search_turns("august", &mine(&ledger), 5)
                .unwrap()
                .len(),
            1
        );
        // Hidden (a reset): never returned.
        store.tag_turns(&brand, "p1").unwrap();
        assert!(
            store
                .search_turns("logo", &Scope::default(), 5)
                .unwrap()
                .is_empty()
        );
        // Pruned with no window for summarized turns (the default): only the hidden one
        // goes, and a summarized turn is still found.
        let indexed = store.indexed_turns().unwrap();
        let soon = utcnow() + Duration::seconds(1);
        assert_eq!(store.prune_turns(None, soon).unwrap(), 1, "the hidden one");
        assert_eq!(store.indexed_turns().unwrap(), indexed - 1);
        assert_eq!(
            store
                .search_turns("august", &mine(&ledger), 5)
                .unwrap()
                .len(),
            1
        );
        // With a window: the summarized turn goes too, from the index as well. A turn no
        // summary covers is never pruned.
        let gone = store.prune_turns(Some(soon), soon).unwrap();
        assert_eq!(gone, 1, "the compacted turn");
        assert_eq!(store.indexed_turns().unwrap(), indexed - 2);
        assert!(
            store
                .search_turns("august", &mine(&ledger), 5)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search_turns("half second", &mine(&ledger), 5)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_database_from_before_the_index_is_filled_once_when_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.db");
        let ledger = Profile::new("ledger", "Ledger").id;
        {
            let store = Store::open(&path).unwrap();
            store
                .add_turn(&ledger, "s1", Role::User, "the invoice went out late")
                .unwrap();
            // As a database from before ADR 0052: no index, no triggers.
            store
                .conn()
                .execute_batch(
                    "DROP TRIGGER turns_fts_insert; DROP TRIGGER turns_fts_delete; \
                     DROP TRIGGER turns_fts_update; DROP TABLE turns_fts;",
                )
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .search_turns("invoice", &mine(&ledger), 5)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_page_goes_back_from_a_turn_and_context_stays_in_its_session() {
        let (store, ledger, _) = store_with_agents();
        let mut ids = Vec::new();
        for (s, text) in [
            ("s1", "one"),
            ("s1", "two"),
            ("s2", "three"),
            ("s1", "four"),
        ] {
            ids.push(store.add_turn(&ledger, s, Role::User, text).unwrap());
        }
        let page = store.page_turns(&mine(&ledger), 2).unwrap();
        assert_eq!(page[0].text, "four");
        let older = store
            .page_turns(
                &Scope {
                    before: Some(page[1].id),
                    ..mine(&ledger)
                },
                10,
            )
            .unwrap();
        assert_eq!(
            older.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["two", "one"]
        );
        let two = &older[0];
        let around: Vec<String> = store
            .turns_around(two, 1)
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect();
        assert_eq!(around, ["one", "four"], "s2's turn is another session");
        // A glob over sessions: an outside client's runs only.
        let runs = Scope {
            sessions: Some(vec!["s2".into()]),
            ..Scope::default()
        };
        assert_eq!(store.page_turns(&runs, 10).unwrap().len(), 1);
        let none = Scope {
            sessions: Some(Vec::new()),
            ..Scope::default()
        };
        assert!(store.page_turns(&none, 10).unwrap().is_empty());
    }
}
