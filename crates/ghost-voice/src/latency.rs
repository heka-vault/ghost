//! Where a turn's time goes: one timeline per turn, timings only, never text.
//! (The observer and the record follow in this module; the probe is what the stages
//! report through.)

use std::sync::Arc;

/// Durations Ghost's stages measure themselves. Every method is a no-op by default, so a
/// stage built without a session (tests, the wake listener) needs no checks.
pub trait LatencyProbe: Send + Sync {
    /// The turn model said done and the grace hold ran `secs` before the turn ended.
    fn held(&self, _secs: f64) {}
    /// The operator went on during the grace hold; the turn stayed open.
    fn kept_open(&self) {}
    /// An answer started during the grace hold: `started`, then `kept` (it was the
    /// answer), `withdrawn` (the operator went on), `missed` (the words changed) or
    /// `tool` (it reached a tool call and the confirmed turn ran again).
    fn speculation(&self, _outcome: &str) {}
    /// An interceptor (`name` is its code name) handled the turn in `secs`.
    fn intercepted(&self, _name: &str, _secs: f64) {}
    /// The interceptors looked at a transcript and let it through, taking `secs`.
    fn interceptors(&self, _secs: f64) {}
    /// A pending change met a transcript: which classifier decided, how long it took,
    /// and the whole call including any execution.
    fn confirmation(
        &self,
        _classifier: &str,
        _classify_secs: f64,
        _verdict: &str,
        _total_secs: f64,
    ) {
    }
    /// A step inside a function call (`prepare`, `submit`, `request`, `read`).
    fn tool_step(&self, _call_id: &str, _step: &str, _secs: f64) {}
    /// How a function call ended: `done`, `needs_confirmation`, `error`, `ok`.
    fn tool_status(&self, _call_id: &str, _status: &str) {}
    /// The operation a `change` call named, as `ability.operation`.
    fn tool_operation(&self, _call_id: &str, _operation: &str) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NullProbe;

impl LatencyProbe for NullProbe {}

pub fn null_probe() -> Arc<dyn LatencyProbe> {
    Arc::new(NullProbe)
}
