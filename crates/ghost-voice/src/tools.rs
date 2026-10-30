//! Bridge tool sources to the model's function calling.
//!
//! Three kinds of tool reach the model: a source's typed read tools; each connected app's
//! changes, under the app's name like its reads (ADR 0034); and one shared `change` for
//! Ghost's own operations (the agents, the rules, the outputs), plus the session's tools
//! (switch, hang up, hold). A change of either kind goes to the gate. What happens then is
//! the operator's permissions (ADR 0011, ADR 0034): it happens now and the result comes
//! back, or it waits for a spoken yes. There is no undo tool: to take a change back the
//! model asks for the reversing change. The model never applies a change itself.
//!
//! `ghost_llm::Offering` decides what is offered and under which name; this module runs
//! the calls.

use std::sync::Arc;
use std::time::Instant;

use ghost_core::conversations::{GHOST, SOURCE as CONVERSATIONS};
use ghost_core::gate::Request;
use ghost_core::rules::SOURCE as RULES;
use ghost_core::source::EventKind;
use ghost_core::{Autonomy, Events, Operation, Outcome, OutcomeStatus, ToolSource, WriteGate};
pub use ghost_llm::{BRING_UP_TOOL, CHANGE_TOOL, HANG_UP_TOOL, HOLD_TOOL, SWITCH_TOOL};
use ghost_llm::{Offered, Offering, SessionTools, Tools};
use serde_json::{Map, Value, json};
use tracing::{info, warn};

use crate::confirmation::status_str;
use crate::conversation::ConversationRuntime;
use crate::frames::Frame;
use crate::intercept::SessionAction;
use crate::latency::{LatencyProbe, null_probe};

pub type OnCall = Arc<dyn Fn(&str, &Map<String, Value>) + Send + Sync>;
/// The operator's words since Ghost last spoke, and that reply.
pub type LastRequest = Arc<dyn Fn() -> (String, String) + Send + Sync>;

/// What a call hands back to the model, and what follows it.
#[derive(Debug, Clone)]
pub struct CallResult {
    pub result: Map<String, Value>,
    /// Run the model on the result (false after `hang_up`, `hold` and a transfer).
    pub run_llm: bool,
    /// Frames to queue at the pipeline source once the result has landed in the context:
    /// a transfer's.
    pub after: Vec<Frame>,
}

impl CallResult {
    fn plain(result: Map<String, Value>) -> Self {
        Self {
            result,
            run_llm: true,
            after: Vec::new(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        let mut m = Map::new();
        m.insert("error".into(), Value::String(message.into()));
        Self::plain(m)
    }

    pub fn error_text(&self) -> Option<&str> {
        self.result.get("error").and_then(Value::as_str)
    }

    pub fn status(&self) -> Option<&str> {
        self.result.get("status").and_then(Value::as_str)
    }
}

fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => {
            let mut m = Map::new();
            m.insert("result".into(), other);
            m
        }
    }
}

/// The session's tools: what is offered and how a call runs.
pub struct ToolDispatcher {
    sources: Vec<ToolSource>,
    gate: Arc<WriteGate>,
    session_id: String,
    on_hang_up: SessionAction,
    on_hold: Option<SessionAction>,
    /// The turn's state: a `hold` asked for in a turn that cut Ghost off is declined.
    turns: Option<Arc<crate::turns::TurnControl>>,
    events: Arc<Events>,
    conversations: Option<Arc<ConversationRuntime>>,
    record: OnCall,
    last_request: LastRequest,
    probe: Arc<dyn LatencyProbe>,
    offering: Offering,
}

pub struct ToolsParams {
    pub sources: Vec<ToolSource>,
    pub gate: Arc<WriteGate>,
    pub session_id: String,
    pub on_hang_up: SessionAction,
    pub on_hold: Option<SessionAction>,
    pub turns: Option<Arc<crate::turns::TurnControl>>,
    pub events: Option<Arc<Events>>,
    pub conversations: Option<Arc<ConversationRuntime>>,
    pub on_call: Option<OnCall>,
    pub last_request: Option<LastRequest>,
    pub probe: Option<Arc<dyn LatencyProbe>>,
}

impl ToolDispatcher {
    /// `probe` gets the time each step inside a call takes (`latency`): `read` for a read
    /// tool (the app behind it), and for a change the source's `prepare`, the gate's
    /// `submit` (the call itself below `confirm`) and the `request` check that may
    /// execute it.
    pub fn new(p: ToolsParams) -> Arc<Self> {
        let session = SessionTools {
            conversations: p.conversations.is_some(),
            hold: p.on_hold.is_some(),
        };
        let offering = Offering::new(p.gate.clone(), session);
        let this = Arc::new(Self {
            sources: p.sources,
            gate: p.gate,
            session_id: p.session_id,
            on_hang_up: p.on_hang_up,
            on_hold: p.on_hold,
            turns: p.turns,
            events: p.events.unwrap_or_default(),
            conversations: p.conversations,
            record: p.on_call.unwrap_or_else(|| Arc::new(|_, _| {})),
            last_request: p
                .last_request
                .unwrap_or_else(|| Arc::new(|| (String::new(), String::new()))),
            probe: p.probe.unwrap_or_else(null_probe),
            offering,
        });
        if let Some(c) = &this.conversations {
            let me = this.clone();
            c.set_tool_schemas(Arc::new(move |sources| me.schemas_for(sources)));
        }
        this
    }

    /// What the active agent reaches (ADR 0035): every connected app and Ghost's own
    /// sources, or every source given when the session was built without a reach.
    pub fn active_sources(&self) -> Vec<ToolSource> {
        match &self.conversations {
            Some(c) if c.has_reach() => c.sources_for(None),
            _ => self.sources.clone(),
        }
    }

    /// The tools advertised for a list of sources: their reads, each app's changes under
    /// their own names, `change` for Ghost's own operations, and the session's own tools.
    pub fn schemas_for(&self, sources: &[ToolSource]) -> Tools {
        self.offering.schemas(sources)
    }

    /// The tools the session starts with.
    /// Whether the tool the model called by `name` goes to a connected app, as a read or a
    /// change: what Ghost acknowledges aloud (`crate::acks`). Ghost's own tools (the agents,
    /// the rules, the outputs, the skills, the waiting list, `change`, the session's) are
    /// None.
    pub fn outside(&self, name: &str) -> Option<crate::acks::AckKind> {
        let sources = self.active_sources();
        match self.offering.lookup(&sources, name)? {
            Offered::Read { source, .. } => source.app.then_some(crate::acks::AckKind::Read),
            Offered::Change { source, .. } => sources
                .iter()
                .any(|s| s.name == source && s.app)
                .then_some(crate::acks::AckKind::Change),
        }
    }

    pub fn schemas(&self) -> Tools {
        self.schemas_for(&self.active_sources())
    }

    /// One call by the model: `name` with `args`; `call_id` labels its steps in the
    /// latency record.
    pub async fn call(&self, name: &str, args: Map<String, Value>, call_id: &str) -> CallResult {
        match name {
            CHANGE_TOOL => self.change(args, call_id).await,
            SWITCH_TOOL if self.conversations.is_some() => self.switch(args).await,
            BRING_UP_TOOL => self.bring_up(args).await,
            HANG_UP_TOOL => {
                // The model's goodbye in this same response is spoken (`narration` lets
                // hang_up's text through); no follow-up completion runs, and the call ends
                // as the goodbye finishes.
                (self.on_hang_up)().await;
                let mut m = Map::new();
                m.insert("ok".into(), json!(true));
                CallResult {
                    result: m,
                    run_llm: false,
                    after: Vec::new(),
                }
            }
            HOLD_TOOL if self.on_hold.is_some() => {
                let mut m = Map::new();
                if self.turns.as_ref().is_some_and(|t| t.barged_in()) {
                    // The operator cut Ghost off to speak, not to turn away (ADR 0021):
                    // Ghost has stopped and keeps listening; the narration cut drops the
                    // words that came with the call.
                    m.insert("ok".into(), json!(false));
                    m.insert(
                        "note".into(),
                        json!("the operator interrupted you to speak: stopped and listening, not on hold"),
                    );
                    return CallResult {
                        result: m,
                        run_llm: false,
                        after: Vec::new(),
                    };
                }
                // No follow-up completion: the words in this response are all Ghost says
                // before it goes quiet.
                if let Some(h) = &self.on_hold {
                    h().await;
                }
                m.insert("ok".into(), json!(true));
                CallResult {
                    result: m,
                    run_llm: false,
                    after: Vec::new(),
                }
            }
            _ => self.dispatch(name, args, call_id).await,
        }
    }

    /// Every read tool and every app's change is one catch-all: a call goes to the
    /// version the active agent reaches now (an app connected since the session was built
    /// is there), or is refused when the permissions block it.
    async fn dispatch(&self, name: &str, args: Map<String, Value>, call_id: &str) -> CallResult {
        let reach_now = self.active_sources();
        let Some(hit) = self.offering.lookup(&reach_now, name) else {
            (self.record)(name, &args);
            return CallResult::error(format!("{name} is not available here"));
        };
        match hit {
            Offered::Change { operation, source } => {
                let mut args = args;
                let rationale = args
                    .remove("rationale")
                    .map(|v| py_str(&v))
                    .unwrap_or_default();
                let op = operation.clone();
                let source = source.to_string();
                self.run_change(&source, &op, args, &rationale, call_id)
                    .await
            }
            Offered::Read { tool, source } => {
                let operation = if tool.operation.is_empty() {
                    tool.name.as_str()
                } else {
                    tool.operation.as_str()
                };
                if self.gate.blocks_read(&source.name, operation, &args) {
                    (self.record)(name, &args);
                    return CallResult::error(format!(
                        "{name} is blocked by the operator's permissions for that"
                    ));
                }
                (self.record)(name, &args);
                let began = Instant::now();
                let (result, status) = match (tool.handler)(args).await {
                    Ok(r) => (r, "ok"),
                    Err(e) => {
                        let mut m = Map::new();
                        m.insert("error".into(), Value::String(e.to_string()));
                        (m, "error")
                    }
                };
                self.probe
                    .tool_step(call_id, "read", began.elapsed().as_secs_f64());
                self.probe.tool_status(call_id, status);
                CallResult::plain(result)
            }
        }
    }

    async fn change(&self, a: Map<String, Value>, call_id: &str) -> CallResult {
        let reach_now = self.active_sources();
        let ops = self.offering.operations(&reach_now);
        let mut aname = a
            .get("ability")
            .or_else(|| a.get("skill")) // "skill" for older contexts
            .map(py_str)
            .unwrap_or_default();
        let mut op_name = a.get("operation").map(py_str).unwrap_or_default();
        if let Some((head, tail)) = op_name.rsplit_once('.') {
            // the model echoed "ability.operation" from the listing
            if ops.iter().any(|(n, _)| *n == head) {
                aname = head.to_string();
                op_name = tail.to_string();
            }
        }
        if aname.is_empty() && ops.len() == 1 {
            aname = ops[0].0.to_string();
        }
        let op = ops
            .iter()
            .find(|(n, _)| *n == aname)
            .and_then(|(_, list)| list.iter().find(|op| op.name == op_name))
            .map(|op| (*op).clone());
        let Some(op) = op else {
            self.probe.tool_status(call_id, "error");
            return CallResult::error(format!("unknown operation '{aname}'.'{op_name}'"));
        };
        let args = a
            .get("args")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let rationale = a.get("rationale").map(py_str).unwrap_or_default();
        self.run_change(&aname, &op, args, &rationale, call_id)
            .await
    }

    /// One change, from `change` or from an app's own tool: prepare, the gate, and what
    /// the model is told.
    async fn run_change(
        &self,
        aname: &str,
        op: &Operation,
        mut args: Map<String, Value>,
        rationale: &str,
        call_id: &str,
    ) -> CallResult {
        let op_name = op.name.as_str();
        self.probe
            .tool_operation(call_id, &format!("{aname}.{op_name}"));
        (self.record)(&format!("{aname}.{op_name}"), &args);
        if aname == CONVERSATIONS
            && let Some(c) = &self.conversations
        {
            args.insert("_active".into(), Value::String(c.active().slug));
        }
        if aname == RULES {
            // a rule must be the operator's words
            args.insert("_request".into(), Value::String((self.last_request)().0));
        }
        let began = Instant::now();
        let prepared = match (op.prepare)(args.clone()).await {
            Ok(p) => p,
            Err(e) => {
                self.probe.tool_status(call_id, "error");
                return CallResult::error(e.to_string());
            }
        };
        self.probe
            .tool_step(call_id, "prepare", began.elapsed().as_secs_f64());
        let mut req = Request::new(
            &self.session_id,
            aname,
            op_name,
            prepared.target.clone(),
            prepared.changes.clone(),
            if rationale.is_empty() {
                &prepared.rationale
            } else {
                rationale
            },
        );
        req.conversation = self.conversations.as_ref().map(|c| c.active().slug);
        req.asked_by = prepared.asked_by.clone();
        let began = Instant::now();
        let sub = match self.gate.submit(req).await {
            Ok(s) => s,
            Err(e) => {
                warn!("change {aname}.{op_name} failed: {e}");
                self.probe.tool_status(call_id, "error");
                return CallResult::error(e.to_string());
            }
        };
        self.probe
            .tool_step(call_id, "submit", began.elapsed().as_secs_f64());
        if let Some(outcome) = &sub.outcome {
            self.probe
                .tool_status(call_id, &format!("done:{}", sub.autonomy.as_str()));
            info!(
                "{} {}: {} {}",
                sub.autonomy.as_str(),
                sub.proposal.id,
                status_str(outcome.status),
                outcome.summary()
            );
            let mut result = Map::new();
            result.insert("status".into(), json!("done"));
            result.insert("result".into(), json!(outcome.summary()));
            result.insert("state_now".into(), json!(state(outcome)));
            result.insert(
                "how_to_say_it".into(),
                json!(if sub.autonomy == Autonomy::Announce {
                    "a few words, in passing"
                } else {
                    "only if asked, or if it failed"
                }),
            );
            // "Save this as accounting": a hired agent is switched into at once, carrying
            // this session's turns along (ADR 0014).
            if aname == CONVERSATIONS
                && op_name == "create"
                && outcome.status == OutcomeStatus::Success
                && args.get("switch").is_none_or(truthy)
                && let Some(c) = &self.conversations
                && let Ok(Some(target)) = c.conversations().get(&sub.proposal.target.scope())
            {
                c.carry_session_turns(&target);
                let outcome_note = format!(
                    "The agent '{}' was just hired at their request and they are now with \
                     it; that is done, do not hire or switch again.",
                    target.name
                );
                let after = c.switch(&target, "", &outcome_note).await;
                self.emit_outcome(aname, op_name, outcome).await;
                return self.transfer(c, result, after);
            }
            // A retired agent leaves voice routing: retiring the one the operator is with
            // hands them back to Ghost.
            if aname == CONVERSATIONS
                && op_name == "retire"
                && outcome.status == OutcomeStatus::Success
                && let Some(c) = &self.conversations
                && sub.proposal.target.scope() == c.active().slug
                && let Ok(Some(home)) = c.conversations().get(GHOST)
            {
                let outcome_note = format!(
                    "They just retired the agent '{}' at their request; that is done, do \
                     not retire or switch again.",
                    c.active().name
                );
                let after = c.switch(&home, "", &outcome_note).await;
                self.emit_outcome(aname, op_name, outcome).await;
                return self.transfer(c, result, after);
            }
            self.emit_outcome(aname, op_name, outcome).await;
            return CallResult::plain(result);
        }
        info!("pending {}: {}", sub.proposal.id, sub.proposal.preview);
        // The operator may already have said yes by asking for it: "close infra 17".
        let (request, before) = (self.last_request)();
        let began = Instant::now();
        let outcome = self
            .gate
            .confirm_by_request(&self.session_id, &request, &before, None)
            .await
            .unwrap_or_else(|e| {
                warn!("gate: the request check failed: {e}");
                None
            });
        self.probe
            .tool_step(call_id, "request", began.elapsed().as_secs_f64());
        self.probe.tool_status(
            call_id,
            if outcome.is_some() {
                "done:request"
            } else {
                "needs_confirmation"
            },
        );
        if let Some(outcome) = outcome {
            info!(
                "executed {}: {} (as asked)",
                outcome.proposal_id,
                status_str(outcome.status)
            );
            let mut result = Map::new();
            result.insert("status".into(), json!("done"));
            result.insert("result".into(), json!(outcome.summary()));
            result.insert("state_now".into(), json!(state(&outcome)));
            result.insert(
                "how_to_say_it".into(),
                json!("a few words, in passing; they asked for exactly this"),
            );
            self.emit_outcome(aname, op_name, &outcome).await;
            return CallResult::plain(result);
        }
        let mut result = Map::new();
        result.insert("status".into(), json!("needs_confirmation"));
        result.insert("summary".into(), json!(sub.proposal.preview));
        // ADR 0030: unanswered, it waits on the operator's waiting list.
        result.insert(
            "waits_minutes".into(),
            json!(self.gate.pending_ttl().num_seconds() / 60),
        );
        CallResult::plain(result)
    }

    async fn emit_outcome(&self, aname: &str, op_name: &str, outcome: &Outcome) {
        let mut data = Map::new();
        data.insert("ability".into(), json!(aname));
        data.insert("operation".into(), json!(op_name));
        data.insert("status".into(), json!(status_str(outcome.status)));
        self.events.emit(EventKind::Outcome, data).await;
    }

    /// Return the tool result without a model run, then, once the aggregator has appended
    /// it to the context, the transfer frames go in at the pipeline source. Not out of the
    /// LLM stage: a frame the model stage pushes never passes through itself (the system
    /// prompt would not change), and anything pushed downstream queues behind the audio
    /// being played.
    fn transfer(
        &self,
        c: &ConversationRuntime,
        result: Map<String, Value>,
        after: Vec<Frame>,
    ) -> CallResult {
        if after.is_empty() {
            c.transfer_done();
            return CallResult::plain(result);
        }
        CallResult {
            result,
            run_llm: false,
            after,
        }
    }

    async fn switch(&self, args: Map<String, Value>) -> CallResult {
        let Some(c) = &self.conversations else {
            return CallResult::error("switching conversations is not available here");
        };
        let name = args.get("name").map(py_str).unwrap_or_default();
        (self.record)(SWITCH_TOOL, &args);
        if !c.switch_allowed() {
            return CallResult::error("switching conversations is not allowed");
        }
        let Some(target) = c.resolve(&name) else {
            let names: Vec<String> = c
                .conversations()
                .list(false)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.name)
                .collect();
            return CallResult::plain(object(json!({
                "status": "no_match",
                "known": names,
                "hint": "ask whether to hire an agent with that name",
            })));
        };
        if target.slug == c.active().slug {
            return CallResult::plain(object(json!({
                "status": "already_here",
                "conversation": target.name,
            })));
        }
        let handoff = args.get("handoff").map(py_str).unwrap_or_default();
        let after = c.switch(&target, &handoff, "").await;
        self.transfer(
            c,
            object(json!({"status": "transferred", "to": target.name})),
            after,
        )
    }

    async fn bring_up(&self, args: Map<String, Value>) -> CallResult {
        let change_id = args.get("change").map(py_str).unwrap_or_default();
        (self.record)(BRING_UP_TOOL, &args);
        let p = match self.gate.bring_back(&change_id, &self.session_id) {
            Ok(Some(p)) => p,
            _ => return CallResult::error("That change isn't waiting any more."),
        };
        // "Yes, close seventeen": the operator's own words may already be the yes (ADR 0017).
        let (request, before) = (self.last_request)();
        let outcome = self
            .gate
            .confirm_by_request(&self.session_id, &request, &before, None)
            .await
            .ok()
            .flatten();
        if let Some(outcome) = outcome {
            let result = object(json!({
                "status": "done",
                "result": outcome.summary(),
                "state_now": state(&outcome),
            }));
            let mut data = Map::new();
            data.insert("operation".into(), json!("confirmed"));
            data.insert("status".into(), json!(status_str(outcome.status)));
            self.events.emit(EventKind::Outcome, data).await;
            return CallResult::plain(result);
        }
        CallResult::plain(object(json!({
            "status": "needs_confirmation",
            "summary": p.preview,
        })))
    }
}

fn state(outcome: &Outcome) -> String {
    outcome
        .readback
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |s| s.spoken())
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::String(s) => !s.is_empty() && s != "false" && s != "0",
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        _ => true,
    }
}

/// A value as Python's `str()` showed it.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
