//! The provider check, kept (ADR 0056): what the operator's connection does with each
//! thing a spoken turn needs, run by the desktop's Test or `ghostd llm test`, kept in
//! SQLite with a time, and used three ways:
//!
//! - every route's latest check is seeded into `ghost_llm::capabilities` at start, so a
//!   refused thinking setting or cache marker is never sent again;
//! - a connection too slow for a good spoken reply is a note on the waiting list, with
//!   what would help (a fast model, or which of the operator's tried options measured
//!   faster), from the latest check of the saved connection or the operator's own recent
//!   turns in `latency.jsonl`;
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
use ghost_llm::check::{self, CheckOptions, ModelCheck, Tried};
use ghost_llm::model::Model;
use serde_json::Value;
use tracing::{info, warn};

use crate::daemon::{Daemon, lock};

/// The waiting list's key for a connection too slow for a good spoken reply.
pub const SLOW_PROBLEM_KEY: &str = "model:slow";
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
        let advice = self.advice_for(&models, fast.is_some(), &[route_key(&main)]);
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

    /// What would help, from `models` and the other routes the operator has tried.
    fn advice_for(&self, models: &[ModelCheck], has_fast: bool, own: &[String]) -> Option<String> {
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
        check::advice(models, has_fast, live, &others)
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
        let advice = self.advice_for(&models, fast.is_some(), &[route_key(&main)]);
        Some(ModelCheckRun {
            at: main_row.at,
            models,
            advice,
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
    /// recent turns. On the waiting list while it holds (`slow_problem`).
    pub(crate) fn refresh_slow_note(&self) {
        let (main, fast) = self.saved_targets();
        let rows = self.latest_checks();
        let mine: Vec<ModelCheck> = [Some(&main), fast.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|t| rows.iter().find(|r| r.route == route_key(t)))
            .filter_map(parsed)
            .collect();
        let note = self.advice_for(&mine, fast.is_some(), &[route_key(&main)]);
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

    pub(crate) fn slow_problem(&self) -> Option<String> {
        lock(&self.model_slow).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
