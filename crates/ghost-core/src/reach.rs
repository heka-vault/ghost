//! How much Ghost read and did (ADR 0060): one row per tool call an agent made, in a
//! session or a run, as counts and sizes. Never contents: no arguments, no result, no
//! words, only the app, the tool's name, whether it was a read, whether it worked, how
//! many items came back, how many bytes and how long it took. Today says "read 412 items
//! across 4 apps, 3.7 MB" from these, and the timeline shows a run's reach.
//!
//! The size is the app's answer as it arrived, redacted, before it was fitted to the
//! model's budget (ADR 0046): what Ghost read, not what the model was shown. The app
//! measures it inside the call (`note`, from `apps::App::call`) and the dispatcher that
//! made the call picks it up (`measured`) and keeps it with the session and the agent.
//! A call that ran outside `measured` (a change the gate made on another task) is
//! measured from the result the model got instead.

use std::cell::RefCell;
use std::future::Future;

use chrono::{DateTime, Utc};
use serde_json::Value;

/// How long the rows are kept.
pub const REACH_KEPT_DAYS: i64 = 90;

/// What one call brought back, measured where it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measure {
    /// The answer's size as JSON text, in bytes.
    pub bytes: u64,
    /// How many items: the length of the list the answer is, or 1 for one thing. None
    /// when nothing came back.
    pub items: Option<u64>,
}

/// One call, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct ReachRow {
    pub at: DateTime<Utc>,
    /// The session's id, or `run-<id>` for a run with nobody there.
    pub session_id: String,
    /// The agent's slug.
    pub agent: String,
    /// The app's name (`github`, `ghost` for Ghost's own).
    pub app: String,
    /// The tool's name as the app calls it.
    pub tool: String,
    /// A read; else a change through the gate.
    pub read: bool,
    /// It came back without an error.
    pub ok: bool,
    pub items: Option<u64>,
    pub bytes: u64,
    pub ms: u64,
}

/// Totals over some rows, grouped by one key.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReachTotal {
    pub key: String,
    /// The agent for a session's total (the first that called in it).
    pub agent: Option<String>,
    pub calls: u64,
    pub reads: u64,
    pub changes: u64,
    pub failed: u64,
    pub items: u64,
    pub bytes: u64,
    pub ms: u64,
    /// The apps called, by name, sorted.
    pub apps: Vec<String>,
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
}

impl ReachTotal {
    pub fn add(&mut self, r: &ReachRow) {
        self.calls += 1;
        if r.read {
            self.reads += 1;
        } else {
            self.changes += 1;
        }
        if !r.ok {
            self.failed += 1;
        }
        self.items += r.items.unwrap_or(0);
        self.bytes += r.bytes;
        self.ms += r.ms;
        if !self.apps.contains(&r.app) {
            self.apps.push(r.app.clone());
            self.apps.sort();
        }
        if self.agent.is_none() {
            self.agent = Some(r.agent.clone());
        }
        self.first = Some(self.first.map_or(r.at, |f| f.min(r.at)));
        self.last = Some(self.last.map_or(r.at, |l| l.max(r.at)));
    }
}

/// The rows of a stretch of time, totalled: everything, by app, by agent and by session
/// (or run), each list largest first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReachSummary {
    pub total: ReachTotal,
    pub by_app: Vec<ReachTotal>,
    pub by_agent: Vec<ReachTotal>,
    pub by_session: Vec<ReachTotal>,
}

pub fn summarize(rows: &[ReachRow]) -> ReachSummary {
    use std::collections::BTreeMap;
    let mut total = ReachTotal {
        key: "all".into(),
        ..Default::default()
    };
    let mut apps: BTreeMap<&str, ReachTotal> = BTreeMap::new();
    let mut agents: BTreeMap<&str, ReachTotal> = BTreeMap::new();
    let mut sessions: BTreeMap<&str, ReachTotal> = BTreeMap::new();
    for r in rows {
        total.add(r);
        for (map, key) in [
            (&mut apps, r.app.as_str()),
            (&mut agents, r.agent.as_str()),
            (&mut sessions, r.session_id.as_str()),
        ] {
            map.entry(key)
                .or_insert_with(|| ReachTotal {
                    key: key.to_string(),
                    ..Default::default()
                })
                .add(r);
        }
    }
    total.agent = None;
    let sorted = |m: BTreeMap<&str, ReachTotal>| {
        let mut v: Vec<ReachTotal> = m.into_values().collect();
        v.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.key.cmp(&b.key)));
        v
    };
    let mut by_session = sorted(sessions);
    // Sessions read best in time order, newest first.
    by_session.sort_by(|a, b| b.last.cmp(&a.last).then(a.key.cmp(&b.key)));
    ReachSummary {
        total,
        by_app: sorted(apps)
            .into_iter()
            .map(|mut t| {
                t.agent = None;
                t
            })
            .collect(),
        by_agent: sorted(agents),
        by_session,
    }
}

/// The keys of an object that wraps one list (`{"items": [...], "total_count": 3}`): a
/// wrapper has few keys; a thing with many fields (an issue with its labels) is one item.
const WRAPPER_KEYS: usize = 4;

/// How many items a result holds: a list's length, the length of the one list a small
/// wrapper object carries, else 1. None for nothing at all.
pub fn items_of(v: &Value) -> Option<u64> {
    match v {
        Value::Null => None,
        Value::Array(a) => Some(a.len() as u64),
        Value::Object(m) if m.is_empty() => None,
        Value::Object(m) => {
            if m.len() <= WRAPPER_KEYS {
                let lists: Vec<usize> = m
                    .iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .filter_map(|(_, v)| v.as_array().map(Vec::len))
                    .collect();
                if let [n] = lists[..] {
                    return Some(n as u64);
                }
                // `{"result": "[...]"}` or `{"text": ...}` hold one thing each.
                if let Some(inner @ (Value::Array(_) | Value::Object(_))) = m.get("result") {
                    return items_of(inner);
                }
            }
            Some(1)
        }
        _ => Some(1),
    }
}

/// The measure of a result: its size as compact JSON and its items.
pub fn measure(v: &Value) -> Measure {
    Measure {
        bytes: serde_json::to_string(v).map_or(0, |s| s.len() as u64),
        items: items_of(v),
    }
}

tokio::task_local! {
    static SLOT: RefCell<Option<Measure>>;
}

/// Run `work`, and return what the app call inside it measured (`note`), if any.
pub async fn measured<F: Future>(work: F) -> (F::Output, Option<Measure>) {
    SLOT.scope(RefCell::new(None), async {
        let out = work.await;
        let m = SLOT.with(|s| s.borrow_mut().take());
        (out, m)
    })
    .await
}

/// Called by an app's call with what arrived; kept for `measured` when there is one.
pub fn note(m: Measure) {
    let _ = SLOT.try_with(|s| *s.borrow_mut() = Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_counts_its_items_and_one_thing_counts_one() {
        assert_eq!(items_of(&json!([1, 2, 3])), Some(3));
        assert_eq!(
            items_of(&json!({"total_count": 2, "incomplete_results": false, "items": [{}, {}]})),
            Some(2)
        );
        assert_eq!(items_of(&json!({"result": [{"a": 1}, {"a": 2}]})), Some(2));
        // An issue with a list of labels is one issue.
        let issue = json!({"number": 1, "title": "t", "state": "open", "body": "b",
                           "labels": [{"name": "a"}, {"name": "b"}]});
        assert_eq!(items_of(&issue), Some(1));
        assert_eq!(items_of(&json!({"text": "hello"})), Some(1));
        assert_eq!(items_of(&json!({})), None);
        // Two lists side by side: one thing with two lists, not either.
        assert_eq!(items_of(&json!({"a": [1], "b": [1, 2]})), Some(1));
    }

    #[test]
    fn the_size_is_compact_json_bytes() {
        let m = measure(&json!({"a": "é"}));
        assert_eq!(m.bytes, r#"{"a":"é"}"#.len() as u64);
        assert_eq!(m.items, Some(1));
    }

    #[tokio::test]
    async fn a_note_inside_reaches_the_measure_and_outside_goes_nowhere() {
        note(Measure {
            bytes: 1,
            items: None,
        }); // no scope: dropped, no panic
        let (out, m) = measured(async {
            tokio::task::yield_now().await;
            note(Measure {
                bytes: 10,
                items: Some(2),
            });
            7
        })
        .await;
        assert_eq!(out, 7);
        assert_eq!(
            m,
            Some(Measure {
                bytes: 10,
                items: Some(2)
            })
        );
        let (_, none) = measured(async {}).await;
        assert_eq!(none, None);
    }

    fn row(session: &str, agent: &str, app: &str, read: bool, items: u64, bytes: u64) -> ReachRow {
        ReachRow {
            at: Utc::now(),
            session_id: session.into(),
            agent: agent.into(),
            app: app.into(),
            tool: "t".into(),
            read,
            ok: true,
            items: Some(items),
            bytes,
            ms: 5,
        }
    }

    #[test]
    fn a_summary_totals_by_app_agent_and_session() {
        let rows = vec![
            row("s1", "ghost", "github", true, 30, 3000),
            row("s1", "ghost", "linear", true, 2, 200),
            row("run-1", "ledger", "github", false, 1, 50),
        ];
        let s = summarize(&rows);
        assert_eq!(s.total.calls, 3);
        assert_eq!(s.total.reads, 2);
        assert_eq!(s.total.changes, 1);
        assert_eq!(s.total.items, 33);
        assert_eq!(s.total.bytes, 3250);
        assert_eq!(s.total.apps, vec!["github", "linear"]);
        assert_eq!(s.by_app[0].key, "github");
        assert_eq!(s.by_app[0].calls, 2);
        assert_eq!(s.by_agent[0].key, "ghost");
        let run = s.by_session.iter().find(|t| t.key == "run-1").unwrap();
        assert_eq!(run.agent.as_deref(), Some("ledger"));
        assert_eq!(run.apps, vec!["github"]);
    }
}
