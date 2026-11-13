//! The provider check, kept (ADR 0056): what the operator's connection does with each
//! thing a spoken turn needs, run by the desktop's Test or `ghostd llm test`, kept in
//! SQLite with a time, and used three ways:
//!
//! - every route's latest check is seeded into `ghost_llm::capabilities` at start, so a
//!   refused thinking setting or cache marker is never sent again;
//! - a connection too slow for a good spoken reply is a note on the waiting list, with
//!   what would help (a fast model, or which of the operator's tried options measured
//!   faster), from the latest check of the saved connection or the operator's own recent
//!   turns in `latency.jsonl`. It is for the screen (`Response::Show`): no route and no
//!   wake ever says it. One item under one key, raised over 1.5 s, cleared only under
//!   1.2 s, and its words kept while it holds unless the figure moves by half a second
//!   or the advice changes ([`settle`]);
//! - the desktop shows the latest check of the saved connection.
//!
//! The password is read from the vault by the daemon at the point of use (the env the
//! control API resolves) and never shown. Nothing here changes the connection.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use ghost_api::ModelCheckRun;
use ghost_core::connection::{ModelConnection, Target};
use ghost_core::store::ModelCheckRow;
use ghost_llm::capabilities::{self, route_key};
use ghost_llm::check::{
    self, CheckOptions, ModelCheck, QUICK_AGAIN_SECS, SLOW_FIRST_WORD_SECS, Slow, Tried,
};
use ghost_llm::model::Model;
use serde_json::Value;
use tracing::{info, warn};

use crate::daemon::{Daemon, lock};

/// The waiting list's key for a connection too slow for a good spoken reply: one item
/// however its figure moves.
pub const SLOW_NOTE_KEY: &str = "model:slow";
/// How far the figure must move before a note that holds is reworded.
const REWORD_BY_SECS: f64 = 0.5;
/// Checks kept per route.
const KEEP: i64 = 20;
/// Recent spoken turns read for the live measure, and how many it needs.
const LIVE_TURNS: usize = 10;
const LIVE_LEAST: usize = 5;
/// The tail of `latency.jsonl` a session reads for its first sound: a spoken turn's record
/// is about 1 to 2 KB, so this holds well over the last ten.
const FIRST_WORD_TAIL_BYTES: u64 = 64 * 1024;

/// "claude-haiku-5-5 via cliproxy".
fn label(row: &ModelCheckRow) -> String {
    format!("{} via {}", row.model, row.provider)
}

fn parsed(row: &ModelCheckRow) -> Option<ModelCheck> {
    serde_json::from_value(row.report.clone()).ok()
}

/// The slow note after one more look: raised over [`SLOW_FIRST_WORD_SECS`], and once
/// raised held until the measure is under [`QUICK_AGAIN_SECS`]; while it holds its words
/// stay as they are unless the figure moved by [`REWORD_BY_SECS`] or the advice changed,
/// so a connection near the line is neither raised and cleared nor reworded turn by turn.
pub fn settle(
    held: Option<&Slow>,
    models: &[ModelCheck],
    has_fast: bool,
    live: Option<(f64, usize)>,
    others: &[Tried],
) -> Option<Slow> {
    let over = if held.is_some() {
        QUICK_AGAIN_SECS
    } else {
        SLOW_FIRST_WORD_SECS
    };
    let found = check::advice(models, has_fast, live, others, over)?;
    match held {
        Some(h) if h.help == found.help && (found.secs - h.secs).abs() < REWORD_BY_SECS => {
            Some(h.clone())
        }
        _ => Some(found),
    }
}

/// The first model call's time to its first byte in the operator's recent spoken turns:
/// (median seconds, turns), when there are enough. Timings only; `latency.jsonl` holds
/// no words.
pub fn live_first_token(records: &[Value]) -> Option<(f64, usize)> {
    let xs: Vec<f64> = records
        .iter()
        .rev()
        .filter(|r| r["kind"] == "voice")
        .filter_map(|r| r["llm"]["calls"][0]["ttfb"].as_f64())
        .take(LIVE_TURNS)
        .collect();
    if xs.len() < LIVE_LEAST {
        return None;
    }
    check::Timing::of(&xs).median.map(|m| (m, xs.len()))
}

impl Daemon {
    fn connection_targets(
        env: &HashMap<String, String>,
    ) -> (ModelConnection, Target, Option<Target>) {
        let conn = ModelConnection::from_env(env);
        let main = conn.target(env);
        let fast = conn.fast_target(env);
        (conn, main, fast)
    }

    /// The saved connection's targets, as a session builds them.
    fn saved_targets(&self) -> (Target, Option<Target>) {
        let env = self.env();
        let voice = self.voice();
        (
            voice.llm_target(Some(&env)),
            voice.llm_fast_target(Some(&env)),
        )
    }

    /// One model's client: the one sessions share when it is the saved connection (warm,
    /// as a session finds it), else one of its own that isn't kept.
    fn check_client(&self, target: &Target) -> Result<Arc<Model>, String> {
        let (saved, _) = self.saved_targets();
        let made = if target.fingerprint() == saved.fingerprint() {
            ghost_llm::model::shared(target)
        } else {
            Model::new(target.clone()).map(Arc::new)
        };
        made.map_err(|e| e.to_string())
    }

    /// The provider check through `env` (ADR 0056): the main model, then the fast one,
    /// one after the other so neither's timing is the other's. Kept, seeded, and the
    /// slow note brought up to date.
    pub(crate) async fn check_model(&self, env: HashMap<String, String>) -> ModelCheckRun {
        let (_, main, fast) = Self::connection_targets(&env);
        let voice = self.voice();
        let thinking = (!voice.llm_thinking.is_empty()).then(|| voice.llm_thinking.clone());
        let mut jobs = vec![(main.clone(), CheckOptions::main(thinking.clone()))];
        if let Some(f) = &fast {
            jobs.push((f.clone(), CheckOptions::fast(thinking.clone())));
        }
        let mut models = Vec::new();
        for (target, opts) in jobs {
            let found = match self.check_client(&target) {
                Ok(client) => check::check(&client, &opts).await,
                Err(e) => ModelCheck::failed(&opts.role, &target.model, e),
            };
            info!(
                "llm check: {} ({}): {}",
                target.describe(),
                found.role,
                match (&found.error, found.first_word.median) {
                    (Some(e), _) if !found.reachable => e.clone(),
                    (_, Some(m)) => format!("first word {m:.2}s"),
                    _ => "answered".into(),
                }
            );
            if found.reachable {
                capabilities::seed(&route_key(&target), found.capabilities());
            }
            self.keep_check(&target, &found);
            models.push(found);
        }
        let at = Utc::now();
        // The Test judges what it tested afresh (it may not be the saved connection yet);
        // the note on the waiting list is the saved connection's, with its hysteresis.
        let advice = self
            .advice_for(None, &models, fast.is_some(), &[route_key(&main)])
            .map(|s| s.text());
        self.refresh_slow_note();
        ModelCheckRun { at, models, advice }
    }

    fn keep_check(&self, target: &Target, found: &ModelCheck) {
        let Some(store) = self.late.store.get() else {
            return;
        };
        let row = ModelCheckRow {
            at: Utc::now(),
            route: route_key(target),
            role: found.role.clone(),
            provider: target.provider.clone(),
            model: target.model.clone(),
            report: serde_json::to_value(found).unwrap_or(Value::Null),
        };
        if let Err(e) = store
            .add_model_check(&row)
            .and_then(|_| store.prune_model_checks(KEEP))
        {
            warn!("llm check: not kept ({e})");
        }
    }

    /// Every route's latest check, newest first.
    fn latest_checks(&self) -> Vec<ModelCheckRow> {
        self.late
            .store
            .get()
            .and_then(|s| s.latest_model_checks().ok())
            .unwrap_or_default()
    }

    /// The note after one more look, from `models` and the other routes the operator has
    /// tried, given the one that holds.
    fn advice_for(
        &self,
        held: Option<&Slow>,
        models: &[ModelCheck],
        has_fast: bool,
        own: &[String],
    ) -> Option<Slow> {
        let others: Vec<Tried> = self
            .latest_checks()
            .iter()
            .filter(|r| !own.contains(&r.route))
            .filter_map(|r| {
                let c = parsed(r)?;
                Some(Tried {
                    label: label(r),
                    first_word: c.first_word.median.filter(|_| c.reachable)?,
                })
            })
            .collect();
        let live = live_first_token(&crate::latency::read_records(&self.cfg().latency_path()));
        settle(held, models, has_fast, live, &others)
    }

    /// The latest kept check of the saved connection.
    pub(crate) fn last_model_check(&self) -> Option<ModelCheckRun> {
        let (main, fast) = self.saved_targets();
        let rows = self.latest_checks();
        let find = |t: &Target| rows.iter().find(|r| r.route == route_key(t)).cloned();
        let main_row = find(&main)?;
        let mut models = vec![parsed(&main_row)?];
        if let Some(f) = fast.as_ref().and_then(find).and_then(|r| parsed(&r)) {
            models.push(f);
        }
        Some(ModelCheckRun {
            at: main_row.at,
            models,
            advice: self.slow_note(),
        })
    }

    /// At start: what every route has shown, so the first turn is already shaped to it.
    pub(crate) fn seed_capabilities(&self) {
        let rows = self.latest_checks();
        let mut n = 0;
        for row in &rows {
            if let Some(c) = parsed(row).filter(|c| c.reachable) {
                capabilities::seed(&row.route, c.capabilities());
                n += 1;
            }
        }
        if n > 0 {
            info!("llm: what {n} checked model route(s) take, from their last check");
        }
        self.refresh_slow_note();
    }

    /// The slow note, from the saved connection's latest check and the operator's own
    /// recent turns. On the waiting list while it holds (`slow_note`), never said.
    pub(crate) fn refresh_slow_note(&self) {
        let (main, fast) = self.saved_targets();
        let rows = self.latest_checks();
        let mine: Vec<ModelCheck> = [Some(&main), fast.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|t| rows.iter().find(|r| r.route == route_key(t)))
            .filter_map(parsed)
            .collect();
        let held = lock(&self.model_slow).clone();
        let note = self.advice_for(held.as_ref(), &mine, fast.is_some(), &[route_key(&main)]);
        let changed = {
            let mut slot = lock(&self.model_slow);
            let changed = *slot != note;
            *slot = note;
            changed
        };
        if changed {
            self.refresh_waiting_now();
        }
    }

    /// How soon the model that answers a spoken turn first usually writes (seconds after
    /// the call): the median of the operator's last spoken turns when there are enough,
    /// else that route's latest check. What the first sound waits on
    /// (`ghost_voice::acks::FirstSound::for_connection`). Reads the tail of
    /// `latency.jsonl` only, so a session opening pays the same however long it has grown.
    pub(crate) fn first_word_secs(&self) -> Option<f64> {
        let records =
            crate::latency::tail_records(&self.cfg().latency_path(), FIRST_WORD_TAIL_BYTES);
        if let Some((median, _)) = live_first_token(&records) {
            return Some(median);
        }
        let (main, fast) = self.saved_targets();
        let first = fast.unwrap_or(main);
        self.latest_checks()
            .iter()
            .find(|r| r.route == route_key(&first))
            .and_then(parsed)
            .filter(|c| c.reachable)
            .and_then(|c| c.first_word.median)
    }

    pub(crate) fn slow_note(&self) -> Option<String> {
        lock(&self.model_slow).as_ref().map(Slow::text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn live(secs: f64) -> Option<(f64, usize)> {
        Some((secs, 10))
    }

    /// One refresh of the note from the operator's own turns, as `refresh_slow_note` does.
    fn step(held: Option<Slow>, secs: f64) -> Option<Slow> {
        settle(held.as_ref(), &[], true, live(secs), &[])
    }

    #[test]
    fn the_note_is_one_item_that_holds_its_words_until_the_state_really_changes() {
        // 2026-11-12: 1.6 s then 1.7 s made the note "new" twice and both wakes said it.
        let first = step(None, 1.6).expect("over 1.5 s: raised");
        let again = step(Some(first.clone()), 1.7).expect("still slow");
        assert_eq!(
            again.text(),
            first.text(),
            "a tenth of a second is not news"
        );
        // Hysteresis: once raised it holds until replies are clearly quick again.
        let held = step(Some(again), 1.4).expect("1.4 s does not clear it");
        assert_eq!(held.text(), first.text());
        assert_eq!(step(Some(held), 1.1), None, "quick again: cleared");
        assert_eq!(step(None, 1.4), None, "not slow enough to raise");
        // A real move of the figure is reworded in place (the same key, new words).
        let worse = step(Some(first.clone()), 2.3).expect("much slower");
        assert!(worse.text().contains("2.3 s"), "{}", worse.text());
    }

    #[test]
    fn the_live_measure_reads_the_first_call_of_recent_spoken_turns() {
        let rec =
            |ttfb: f64| json!({"kind": "voice", "llm": {"calls": [{"ttfb": ttfb}, {"ttfb": 9.0}]}});
        let mut records = vec![json!({"kind": "typed", "llm": {"calls": [{"ttfb": 9.0}]}})];
        records.extend([1.1, 2.7, 1.6, 1.9, 1.2].map(rec));
        assert_eq!(live_first_token(&records), Some((1.6, 5)));
        assert_eq!(live_first_token(&records[..4]), None);
        records.push(json!({"kind": "voice", "llm": null}));
        assert_eq!(live_first_token(&records), Some((1.6, 5)));
    }
}
