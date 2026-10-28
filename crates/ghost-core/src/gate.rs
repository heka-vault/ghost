//! The write gate (ADR 0007, ADR 0011, ADR 0034, ADR 0035).
//!
//! Every change from every source comes through here. Sources (the agents, each connected
//! app's changes) register their operations; the operator's permissions decide how each
//! call runs (everything runs and is announced unless a rule blocks it or asks first); the
//! gate does the same work at every level: the source's own limit, the proposal, the
//! steps, the read-back where the source has one, the audit row. At `confirm` a spoken
//! yes sits between proposal and execution. At `announce` and `quiet` the change happens
//! at once. Reversing a change is a new change through `submit` like any other.
//!
//! A change at `confirm` waits on the waiting list (ADR 0030), not in the session: while
//! attached, the session's next turn answers it; set aside (a turn about something else,
//! a hang-up, a transfer), it waits for a yes by id until its time is up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};

use crate::confirmation::{ConfirmationClassifier, ConfirmationContext, RegexClassifier, Verdict};
use crate::levels::Autonomy;
use crate::models::{
    ChangeSet, Draft, ModelError, Outcome, OutcomeStatus, PENDING_TTL_SECS, Proposal,
    ProposalStatus, Target,
};
use crate::policy::{AutonomyPolicy, PolicyError};
use crate::request::asked_for;
use crate::source::Operation;
use crate::store::{Store, StoreError};
use crate::time::utcnow;

#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// A rule of the gate was violated. Safe to read aloud.
    #[error("unknown operation {0}")]
    UnknownOperation(String),
    #[error("That proposal is no longer pending.")]
    NotPending,
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("{0}")]
    Writer(String),
}

pub type Result<T> = std::result::Result<T, GateError>;

/// What `submit` hands back: the level that applied, the stored proposal, and the
/// verified outcome when the write already happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    pub autonomy: Autonomy,
    pub proposal: Proposal,
    pub outcome: Option<Outcome>,
}

impl Submission {
    pub fn executed(&self) -> bool {
        self.outcome.is_some()
    }
}

/// Which classifier judged the last confirmation turn and how long it took. Timing only;
/// the words are never kept here.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub classifier: String,
    pub seconds: f64,
}

/// What a change is submitted with.
#[derive(Debug, Clone)]
pub struct Request {
    pub session_id: String,
    pub ability: String,
    pub operation: String,
    pub target: Target,
    pub changes: ChangeSet,
    pub rationale: String,
    /// The agent the session is in, kept on the proposal for the audit.
    pub conversation: Option<String>,
    /// The words that ask for this call, when the source reads them off it (ADR 0034).
    pub asked_by: Vec<String>,
}

impl Request {
    pub fn new(
        session_id: &str,
        ability: &str,
        operation: &str,
        target: Target,
        changes: ChangeSet,
        rationale: &str,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            ability: ability.into(),
            operation: operation.into(),
            target,
            changes,
            rationale: rationale.into(),
            conversation: None,
            asked_by: Vec::new(),
        }
    }
}

pub type DecisionLog = Arc<dyn Fn(&str) + Send + Sync>;

/// At most one pending proposal is attached to a session; any number wait set aside.
///
/// * `submit` is callable from an LLM tool. Depending on policy it either writes now or
///   stores a pending proposal.
/// * `on_user_turn` is called by the pipeline with the operator's transcribed turn;
///   `confirm_by_request` with the request that led to the proposal; `confirm_pending`
///   and `confirm_id` with a click in the operator's own app. Those are the only paths
///   from a pending proposal to `execute`.
pub struct WriteGate {
    store: Arc<Store>,
    autonomy: RwLock<AutonomyPolicy>,
    classifier: RwLock<Arc<dyn ConfirmationClassifier>>,
    log: DecisionLog,
    ops: RwLock<HashMap<(String, String), Arc<Operation>>>,
    last_classification: Mutex<Option<Classification>>,
}

impl WriteGate {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            autonomy: RwLock::new(AutonomyPolicy::default()),
            classifier: RwLock::new(Arc::new(RegexClassifier)),
            log: Arc::new(|_| {}),
            ops: RwLock::new(HashMap::new()),
            last_classification: Mutex::new(None),
        }
    }

    /// One policy for every agent (ADR 0035).
    pub fn with_autonomy(mut self, policy: AutonomyPolicy) -> Self {
        self.autonomy = RwLock::new(policy);
        self
    }

    pub fn with_classifier(mut self, classifier: Arc<dyn ConfirmationClassifier>) -> Self {
        self.classifier = RwLock::new(classifier);
        self
    }

    pub fn with_log(mut self, log: DecisionLog) -> Self {
        self.log = log;
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn last_classification(&self) -> Option<Classification> {
        self.last_classification
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    // --- registry --------------------------------------------------------------------

    /// A source's operations, replacing whatever it registered before (an app that
    /// reconnected may have other tools).
    pub fn register(&self, ability: &str, operations: Vec<Operation>) {
        self.unregister(ability);
        let mut ops = self.ops.write().unwrap_or_else(|e| e.into_inner());
        for op in operations {
            ops.insert((ability.to_string(), op.name.clone()), Arc::new(op));
        }
    }

    /// A source left (an app was disconnected). A change of it waiting for a yes stays
    /// on the list and fails as unknown if confirmed.
    pub fn unregister(&self, ability: &str) {
        self.ops
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(a, _), _| a != ability);
    }

    fn lookup(&self, ability: &str, name: &str) -> Option<Arc<Operation>> {
        self.ops
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(ability.to_string(), name.to_string()))
            .cloned()
    }

    pub fn operation(&self, ability: &str, name: &str) -> Result<Arc<Operation>> {
        self.lookup(ability, name)
            .ok_or_else(|| GateError::UnknownOperation(format!("{ability}.{name}")))
    }

    pub fn operations(&self, ability: &str) -> Vec<Arc<Operation>> {
        self.ops
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|((a, _), _)| a == ability)
            .map(|(_, op)| op.clone())
            .collect()
    }

    pub fn autonomy(&self) -> AutonomyPolicy {
        self.autonomy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Swap the operator's policy live (the file was edited through the UI).
    pub fn set_autonomy(&self, policy: AutonomyPolicy) {
        (self.log)(&format!(
            "gate: autonomy policy reloaded from {}",
            policy.source
        ));
        *self.autonomy.write().unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Swap how a yes is heard (Jev, once a locked vault opens and has its key).
    pub fn set_classifier(&self, classifier: Arc<dyn ConfirmationClassifier>) {
        *self.classifier.write().unwrap_or_else(|e| e.into_inner()) = classifier;
    }

    fn classifier(&self) -> Arc<dyn ConfirmationClassifier> {
        self.classifier
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// How long a change at `confirm` waits (ADR 0030), from the policy file.
    pub fn pending_ttl(&self) -> Duration {
        let secs = self
            .autonomy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .pending_ttl;
        Duration::seconds(secs.filter(|s| *s > 0).unwrap_or(PENDING_TTL_SECS))
    }

    /// The level a call runs at. `args` (else the target's) are what a rule's argument
    /// patterns match; with neither, the name alone decides.
    pub fn level(
        &self,
        ability: &str,
        operation: &str,
        target: Option<&Target>,
        args: Option<&Map<String, Value>>,
    ) -> Result<Autonomy> {
        self.operation(ability, operation)?;
        let from_target;
        let args = match (args, target) {
            (Some(a), _) => Some(a),
            (None, Some(t)) => {
                from_target = t.rule_args();
                Some(&from_target)
            }
            (None, None) => None,
        };
        Ok(self
            .autonomy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(ability, operation, args))
    }

    /// Whether the model is offered this tool at all: a rule with argument patterns never
    /// takes a tool away, it refuses the call.
    pub fn offers(&self, ability: &str, operation: &str, read: bool) -> bool {
        let policy = self.autonomy.read().unwrap_or_else(|e| e.into_inner());
        if read {
            policy.permissions.offers_read(ability, operation)
        } else {
            policy.permissions.offers_change(ability, operation)
        }
    }

    pub fn blocks_read(&self, ability: &str, operation: &str, args: &Map<String, Value>) -> bool {
        self.autonomy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .permissions
            .blocks(ability, operation, Some(args))
    }

    fn check_allowed(&self, req: &Request) -> Result<Arc<Operation>> {
        let op = self.operation(&req.ability, &req.operation)?;
        if !(op.allowed)(&req.target) {
            return Err(PolicyError::Refused(format!(
                "{} is not allowed for {}.{}",
                req.target.spoken(),
                req.ability,
                req.operation
            ))
            .into());
        }
        Ok(op)
    }

    fn call_args(target: &Target, changes: &ChangeSet) -> Map<String, Value> {
        let mut args = target.rule_args();
        args.extend(changes.rule_args());
        args
    }

    // --- submit ----------------------------------------------------------------------

    pub async fn submit(&self, req: Request) -> Result<Submission> {
        self.check_allowed(&req)?;
        let args = Self::call_args(&req.target, &req.changes);
        let level = self.level(&req.ability, &req.operation, None, Some(&args))?;
        if level == Autonomy::Deny {
            return Err(PolicyError::Refused(format!(
                "{}.{} is blocked by the operator's permissions",
                req.ability, req.operation
            ))
            .into());
        }
        let proposal = self.propose(&req, level).await?;
        (self.log)(&format!(
            "gate: {}.{} on {} -> {}",
            req.ability, req.operation, req.target, level
        ));
        if level == Autonomy::Confirm {
            return Ok(Submission {
                autonomy: level,
                proposal,
                outcome: None,
            });
        }
        let outcome = self.run_now(&proposal).await?;
        Ok(Submission {
            autonomy: level,
            proposal,
            outcome: Some(outcome),
        })
    }

    /// A change that needs no yes runs at once; nothing else can have claimed it.
    async fn run_now(&self, proposal: &Proposal) -> Result<Outcome> {
        self.execute(proposal, None)
            .await?
            .ok_or(GateError::NotPending)
    }

    /// Ghost's own housekeeping (an automatic compaction, an edit made in the desktop app
    /// by the operator): applied at once at `quiet` whatever the policy says, still
    /// bounded, snapshotted and audited. Never reachable from a model tool; the policy's
    /// `deny` is honoured.
    pub async fn apply_internal(&self, req: Request) -> Result<Outcome> {
        self.check_allowed(&req)?;
        let args = Self::call_args(&req.target, &req.changes);
        if self.level(&req.ability, &req.operation, None, Some(&args))? == Autonomy::Deny {
            return Err(PolicyError::Refused(format!(
                "{}.{} is blocked by the operator's permissions",
                req.ability, req.operation
            ))
            .into());
        }
        let proposal = self.propose(&req, Autonomy::Quiet).await?;
        (self.log)(&format!(
            "gate: internal {}.{} on {}",
            req.ability, req.operation, req.target
        ));
        self.run_now(&proposal).await
    }

    /// The confirm path regardless of policy. Writes nothing.
    pub async fn prepare(&self, req: Request) -> Result<Proposal> {
        self.check_allowed(&req)?;
        self.propose(&req, Autonomy::Confirm).await
    }

    async fn propose(&self, req: &Request, autonomy: Autonomy) -> Result<Proposal> {
        let writer = self.operation(&req.ability, &req.operation)?.writer.clone();
        // One attached at a time: a new one sets the old aside, on the waiting list.
        self.detach(&req.session_id, "a newer change")?;
        let snapshot = writer
            .fetch(&req.target)
            .await
            .map_err(|e| GateError::Writer(e.to_string()))?;
        let mut proposal = Proposal::new(Draft {
            session_id: req.session_id.clone(),
            ability: req.ability.clone(),
            operation: req.operation.clone(),
            target: req.target.clone(),
            changes: req.changes.clone(),
            rationale: req.rationale.clone(),
            snapshot,
            preview: format!("On {}: {}.", req.target.spoken(), req.changes.describe()),
        });
        proposal.autonomy = autonomy.as_str().to_string();
        proposal.conversation = req.conversation.clone();
        proposal.asked_by = req.asked_by.clone();
        proposal.expires_at = utcnow() + self.pending_ttl();
        self.store.insert_proposal(&proposal)?;
        self.store.event(
            &proposal.id,
            "prepared",
            Some(&json!({
                "ability": req.ability,
                "operation": req.operation,
                "target": req.target.key(),
                "autonomy": autonomy.as_str(),
            })),
        )?;
        Ok(proposal)
    }

    pub fn pending(
        &self,
        session_id: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Option<Proposal>> {
        let Some(p) = self.store.pending_for_session(session_id)? else {
            return Ok(None);
        };
        if p.is_expired(now) {
            self.store.set_status(&p.id, ProposalStatus::Expired)?;
            self.store.event(&p.id, "expired", None)?;
            return Ok(None);
        }
        Ok(Some(p))
    }

    /// The operator dropped the change the session is waiting on ("never mind").
    pub fn cancel(&self, session_id: &str, reason: &str) -> Result<Option<Proposal>> {
        let Some(p) = self.pending(session_id, None)? else {
            return Ok(None);
        };
        if !self.store.set_status(&p.id, ProposalStatus::Cancelled)? {
            return Ok(None);
        }
        self.store
            .event(&p.id, "cancelled", Some(&json!({"reason": reason})))?;
        Ok(Some(p))
    }

    /// The session moved on (a hang-up, a transfer, a turn about something else): the
    /// change it was waiting on stays pending, on the waiting list (ADR 0030).
    pub fn set_aside(&self, session_id: &str, reason: &str) -> Result<Option<Proposal>> {
        let Some(p) = self.pending(session_id, None)? else {
            return Ok(None);
        };
        self.detach(session_id, reason)?;
        Ok(Some(p))
    }

    fn detach(&self, session_id: &str, reason: &str) -> Result<()> {
        if let Some(p) = self.store.pending_for_session(session_id)?
            && self.store.detach(session_id)? > 0
        {
            self.store
                .event(&p.id, "set_aside", Some(&json!({"reason": reason})))?;
        }
        Ok(())
    }

    /// Every change waiting for a yes, oldest first; expired ones are marked so.
    pub fn waiting(&self, now: Option<DateTime<Utc>>) -> Result<Vec<Proposal>> {
        let mut out = Vec::new();
        for p in self.store.pending_proposals()? {
            if p.is_expired(now) {
                self.store.set_status(&p.id, ProposalStatus::Expired)?;
                self.store.event(&p.id, "expired", None)?;
                continue;
            }
            out.push(p);
        }
        Ok(out)
    }

    pub fn waiting_one(
        &self,
        proposal_id: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Option<Proposal>> {
        let Some(p) = self.store.get_proposal(proposal_id)? else {
            return Ok(None);
        };
        if p.status != ProposalStatus::Pending {
            return Ok(None);
        }
        if p.is_expired(now) {
            self.store.set_status(&p.id, ProposalStatus::Expired)?;
            self.store.event(&p.id, "expired", None)?;
            return Ok(None);
        }
        Ok(Some(p))
    }

    /// Attach a waiting change to this session, so its next turn answers it: Ghost reads
    /// it out and the operator says yes or no (ADR 0030). Writes nothing. Any session may
    /// take up any waiting change (ADR 0035).
    pub fn bring_back(&self, proposal_id: &str, session_id: &str) -> Result<Option<Proposal>> {
        let Some(p) = self.waiting_one(proposal_id, None)? else {
            return Ok(None);
        };
        if self.store.attach(&p.id, session_id)? {
            self.store
                .event(&p.id, "brought_back", Some(&json!({"session": session_id})))?;
            return Ok(self.store.pending_for_session(session_id)?);
        }
        Ok(None)
    }

    pub fn cancel_id(&self, proposal_id: &str, reason: &str) -> Result<Option<Proposal>> {
        let Some(p) = self.waiting_one(proposal_id, None)? else {
            return Ok(None);
        };
        if !self.store.set_status(&p.id, ProposalStatus::Cancelled)? {
            return Ok(None);
        }
        self.store
            .event(&p.id, "cancelled", Some(&json!({"reason": reason})))?;
        Ok(Some(p))
    }

    // --- confirm + execute -----------------------------------------------------------

    /// Called with the pipeline's own transcript of the operator's turn. Returns the
    /// verdict and, when the verdict was affirmative, the verified outcome. A no cancels
    /// the pending proposal; a turn about something else sets it aside (ADR 0030).
    pub async fn on_user_turn(
        &self,
        session_id: &str,
        transcript: &str,
        now: Option<DateTime<Utc>>,
        question: &str,
    ) -> Result<(Verdict, Option<Outcome>)> {
        let Some(p) = self.pending(session_id, now)? else {
            return Ok((Verdict::Unclear, None));
        };
        let ctx = ConfirmationContext {
            pending_action: p.preview.clone(),
            question: question.to_string(),
        };
        let classifier = self.classifier();
        let began = Instant::now();
        let verdict = classifier.classify(transcript, &ctx).await;
        *self
            .last_classification
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Classification {
            classifier: classifier
                .last_source()
                .unwrap_or_else(|| classifier.name().to_string()),
            seconds: began.elapsed().as_secs_f64(),
        });
        self.store.event(
            &p.id,
            "user_turn",
            Some(&json!({"verdict": verdict.as_str(), "text": transcript, "classifier": classifier.name()})),
        )?;
        match verdict {
            Verdict::Negative => {
                if self.store.set_status(&p.id, ProposalStatus::Cancelled)? {
                    self.store.event(
                        &p.id,
                        "cancelled",
                        Some(&json!({"reason": verdict.as_str()})),
                    )?;
                }
                Ok((verdict, None))
            }
            Verdict::Unclear => {
                // Not an answer: the operator moved on. The change waits on the list.
                self.detach(session_id, verdict.as_str())?;
                Ok((verdict, None))
            }
            Verdict::Affirmative => Ok((verdict, self.execute(&p, now).await?)),
        }
    }

    /// The operator's own words that led to the pending proposal, from the pipeline's
    /// transcript. When they plainly asked for exactly this operation on this target
    /// (`request`), that is the yes: execute now. Otherwise None, and the proposal waits
    /// for a yes as before. `question` is Ghost's reply before the request, so "close it"
    /// can point at the issue it just named.
    pub async fn confirm_by_request(
        &self,
        session_id: &str,
        request: &str,
        question: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Option<Outcome>> {
        let Some(p) = self.pending(session_id, now)? else {
            return Ok(None);
        };
        let Some(op) = self.lookup(&p.ability, &p.operation) else {
            return Ok(None);
        };
        let verbs: Vec<String> = if p.asked_by.is_empty() {
            op.asked_by.clone()
        } else {
            p.asked_by.clone()
        };
        if verbs.is_empty() {
            return Ok(None);
        }
        let mentions = p.target.mentions();
        if !asked_for(
            request,
            verbs.iter().map(String::as_str),
            mentions.iter().map(String::as_str),
            question,
        ) {
            return Ok(None);
        }
        self.store.event(
            &p.id,
            "user_turn",
            Some(&json!({"verdict": Verdict::Affirmative.as_str(), "text": request, "classifier": "request"})),
        )?;
        (self.log)(&format!("gate: the operator's request confirmed {}", p.id));
        self.execute(&p, now).await
    }

    /// An authenticated local action (a click in the operator's own desktop session)
    /// confirms the pending proposal (ADR 0007 as amended by ADR 0013). None when
    /// nothing is pending.
    pub async fn confirm_pending(
        &self,
        session_id: &str,
        source: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Option<Outcome>> {
        let Some(p) = self.pending(session_id, now)? else {
            return Ok(None);
        };
        self.store.event(
            &p.id,
            "user_turn",
            Some(&json!({"verdict": Verdict::Affirmative.as_str(), "classifier": source})),
        )?;
        (self.log)(&format!("gate: {source} confirmed {}", p.id));
        self.execute(&p, now).await
    }

    /// A click in the operator's own app on a change on the waiting list (ADR 0013, ADR
    /// 0030). The stale check in `execute` still runs. None when it isn't waiting.
    pub async fn confirm_id(
        &self,
        proposal_id: &str,
        source: &str,
        now: Option<DateTime<Utc>>,
    ) -> Result<Option<Outcome>> {
        let Some(p) = self.waiting_one(proposal_id, now)? else {
            return Ok(None);
        };
        self.store.event(
            &p.id,
            "user_turn",
            Some(&json!({"verdict": Verdict::Affirmative.as_str(), "classifier": source})),
        )?;
        (self.log)(&format!(
            "gate: {source} confirmed {} from the waiting list",
            p.id
        ));
        self.execute(&p, now).await
    }

    // --- execution -------------------------------------------------------------------

    /// None when the proposal was not waiting any more: another yes (a click and a spoken
    /// yes at the same moment) claimed it first, or it was dropped or expired.
    async fn execute(&self, p: &Proposal, now: Option<DateTime<Utc>>) -> Result<Option<Outcome>> {
        let now = now.unwrap_or_else(utcnow);
        let writer = self.operation(&p.ability, &p.operation)?.writer.clone();
        // The claim is the one gate between a yes and the call, and it comes first: of two
        // yeses at once exactly one wins, and the other changes nothing, not even the status.
        if !self.store.claim(&p.id, Some(now))? {
            return Ok(None);
        }
        self.store.event(&p.id, "claimed", None)?;
        // The permissions are read again: the operator may have blocked the change in the
        // time it waited for its yes (ADR 0030). Blocked now, it is refused; nothing ran.
        let args = Self::call_args(&p.target, &p.changes);
        if self.level(&p.ability, &p.operation, None, Some(&args))? == Autonomy::Deny {
            self.store
                .finish_operation(&p.id, "blocked", None, ProposalStatus::Cancelled)?;
            self.store.event(
                &p.id,
                "blocked",
                Some(&json!({"reason": "blocked after it was proposed"})),
            )?;
            return Ok(Some(Outcome {
                proposal_id: p.id.clone(),
                target: p.target.clone(),
                status: OutcomeStatus::Failed,
                applied: Vec::new(),
                failed_step: None,
                error: Some("Your permissions block that now, so I didn't do it.".into()),
                readback: None,
            }));
        }
        // Stale check: the target must not have changed since the snapshot.
        let current = writer
            .fetch(&p.target)
            .await
            .map_err(|e| GateError::Writer(e.to_string()))?;
        if current.version() != p.snapshot.version() {
            self.store
                .finish_operation(&p.id, "stale", None, ProposalStatus::Stale)?;
            self.store.event(
                &p.id,
                "stale",
                Some(&json!({"was": p.snapshot.version(), "now": current.version()})),
            )?;
            return Ok(Some(Outcome {
                proposal_id: p.id.clone(),
                target: p.target.clone(),
                status: OutcomeStatus::Failed,
                applied: Vec::new(),
                failed_step: None,
                error: Some("It changed since I read it. Ask me to look again.".into()),
                readback: Some(current),
            }));
        }

        let mut applied: Vec<String> = Vec::new();
        let mut failed_step: Option<String> = None;
        let mut error: Option<String> = None;
        let key = p.idempotency_key();
        for (i, step) in p.changes.steps().iter().enumerate() {
            let label = step.describe();
            match writer.apply(&p.target, step, &key).await {
                Ok(()) => {
                    self.store.event(
                        &p.id,
                        "step_applied",
                        Some(&json!({"index": i, "step": label})),
                    )?;
                    applied.push(label);
                }
                Err(e) => {
                    // Every failure is recorded, never hidden.
                    let msg = e.to_string();
                    self.store.event(
                        &p.id,
                        "step_failed",
                        Some(&json!({"index": i, "step": label, "error": msg})),
                    )?;
                    failed_step = Some(label);
                    error = Some(msg);
                    break;
                }
            }
        }

        let readback = match writer.fetch(&p.target).await {
            Ok(s) => Some(s),
            Err(e) => {
                self.store.event(
                    &p.id,
                    "readback_failed",
                    Some(&json!({"error": e.to_string()})),
                )?;
                None
            }
        };

        let status = match (&failed_step, applied.is_empty()) {
            (None, _) => OutcomeStatus::Success,
            (Some(_), false) => OutcomeStatus::Partial,
            (Some(_), true) => OutcomeStatus::Failed,
        };
        let status_text = match status {
            OutcomeStatus::Success => "success",
            OutcomeStatus::Partial => "partial",
            OutcomeStatus::Failed => "failed",
        };
        self.store.finish_operation(
            &p.id,
            status_text,
            readback.as_ref().map(Snapshot_version),
            ProposalStatus::Executed,
        )?;
        self.store
            .event(&p.id, "finished", Some(&json!({"status": status_text})))?;
        Ok(Some(Outcome {
            proposal_id: p.id.clone(),
            target: p.target.clone(),
            status,
            applied,
            failed_step,
            error,
            readback,
        }))
    }
}

#[allow(non_snake_case)]
fn Snapshot_version(s: &crate::models::Snapshot) -> &str {
    s.version()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{Call, FakeWriter, changeset, issue, register_fake};
    use crate::models::{AddComment, Change, FakeIssue, SetLabels, SetState, Snapshot};
    use async_trait::async_trait;
    use std::time::Duration as StdDuration;

    fn approve() -> ChangeSet {
        changeset(vec![
            Change::SetLabels(SetLabels {
                add: vec!["approved".into()],
                remove: vec!["needs-signoff".into()],
            }),
            Change::AddComment(AddComment {
                body: "Approved by voice.".into(),
            }),
        ])
    }

    fn target() -> FakeIssue {
        issue("acme", "app", 42)
    }

    fn setup() -> (WriteGate, Arc<FakeWriter>) {
        setup_with(FakeWriter::default(), "")
    }

    fn setup_with(writer: FakeWriter, policy: &str) -> (WriteGate, Arc<FakeWriter>) {
        let writer = Arc::new(writer);
        writer.seed(&target());
        let gate = WriteGate::new(Arc::new(Store::in_memory().unwrap()))
            .with_autonomy(AutonomyPolicy::parse(policy, "<test>").unwrap());
        register_fake(&gate, writer.clone(), "acme/app");
        (gate, writer)
    }

    fn req(session: &str, op: &str, target: FakeIssue, changes: ChangeSet) -> Request {
        Request::new(
            session,
            "github_issues",
            op,
            Target::FakeIssue(target),
            changes,
            "operator asked",
        )
    }

    async fn prepare(gate: &WriteGate, session: &str, changes: ChangeSet) -> Proposal {
        gate.prepare(req(session, "approve", target(), changes))
            .await
            .unwrap()
    }

    fn labels(s: &Snapshot) -> Vec<String> {
        match s {
            Snapshot::FakeIssue(i) => i.labels.clone(),
            _ => panic!("not an issue snapshot"),
        }
    }

    #[tokio::test]
    async fn prepare_writes_nothing() {
        let (gate, writer) = setup();
        let p = prepare(&gate, "s1", approve()).await;
        assert_eq!(p.status, ProposalStatus::Pending);
        assert!(p.preview.contains("add approved") && p.autonomy == "confirm");
        assert_eq!(writer.call_names(), vec!["fetch"]);
        assert_eq!(writer.issue(&target()).labels, vec!["needs-signoff"]);
    }

    #[tokio::test]
    async fn confirm_executes_and_reads_back() {
        let (gate, writer) = setup();
        let p = prepare(&gate, "s1", approve()).await;
        let (verdict, outcome) = gate.on_user_turn("s1", "Confirm.", None, "").await.unwrap();
        assert_eq!(verdict, Verdict::Affirmative);
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Success);
        assert_eq!(labels(outcome.readback.as_ref().unwrap()), vec!["approved"]);
        assert_eq!(
            writer.comments.lock().unwrap()[&target()][0].1,
            p.idempotency_key()
        );
        assert!(gate.pending("s1", None).unwrap().is_none());
        assert!(outcome.spoken().contains("Done"));
        assert_eq!(gate.last_classification().unwrap().classifier, "regex");
    }

    #[tokio::test]
    async fn a_no_cancels_and_an_unrelated_turn_sets_aside() {
        let (gate, writer) = setup();
        let p = prepare(&gate, "s1", approve()).await;
        let (verdict, outcome) = gate
            .on_user_turn("s1", "no, cancel", None, "")
            .await
            .unwrap();
        assert_eq!((verdict, outcome), (Verdict::Negative, None));
        assert!(gate.pending("s1", None).unwrap().is_none());
        assert_eq!(
            gate.store().get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Cancelled
        );
        assert!(writer.call_names().iter().all(|c| *c == "fetch"));

        let p2 = prepare(&gate, "s1", approve()).await;
        let (verdict, outcome) = gate
            .on_user_turn("s1", "what's the next issue", None, "")
            .await
            .unwrap();
        assert_eq!((verdict, outcome), (Verdict::Unclear, None));
        assert!(gate.pending("s1", None).unwrap().is_none());
        assert_eq!(
            gate.store().get_proposal(&p2.id).unwrap().unwrap().status,
            ProposalStatus::Pending,
            "set aside, not cancelled (ADR 0030)"
        );
        assert_eq!(gate.waiting(None).unwrap().len(), 1);
        assert_eq!(
            gate.on_user_turn("s1", "confirm", None, "").await.unwrap(),
            (Verdict::Unclear, None),
            "nothing pending is a no-op"
        );
    }

    #[tokio::test]
    async fn expired_and_stale_proposals_do_not_write() {
        let (gate, writer) = setup();
        let p = prepare(&gate, "s1", approve()).await;
        let later = p.expires_at + Duration::seconds(1);
        let (_, outcome) = gate
            .on_user_turn("s1", "confirm", Some(later), "")
            .await
            .unwrap();
        assert!(outcome.is_none());
        assert_eq!(
            gate.store().get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Expired
        );
        assert!(writer.call_names().iter().all(|c| *c == "fetch"));

        prepare(&gate, "s1", approve()).await;
        writer.set_issue(&target(), |i| i.updated_at = "2026-10-16T00:00:05Z".into()); // someone edited it
        let (_, outcome) = gate.on_user_turn("s1", "confirm", None, "").await.unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Failed);
        assert!(outcome.error.as_deref().unwrap().contains("changed since"));
        assert!(writer.call_names().iter().all(|c| *c == "fetch"));
    }

    #[tokio::test]
    async fn failures_are_reported_honestly() {
        let (gate, writer) = setup();
        prepare(&gate, "s1", approve()).await;
        writer.fail_on("add_comment");
        let (_, outcome) = gate.on_user_turn("s1", "confirm", None, "").await.unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Partial);
        assert_eq!(
            outcome.applied,
            vec!["labels: add approved; remove needs-signoff"]
        );
        assert!(
            outcome
                .failed_step
                .as_deref()
                .unwrap()
                .starts_with("comment:")
        );
        assert!(outcome.spoken().contains("Partly done"));

        let (gate, writer) = setup();
        prepare(&gate, "s1", approve()).await;
        writer.fail_on("set_labels");
        let (_, outcome) = gate.on_user_turn("s1", "confirm", None, "").await.unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Failed);
        assert!(outcome.applied.is_empty());
    }

    #[tokio::test]
    async fn a_new_prepare_sets_the_old_aside_and_sessions_are_isolated() {
        let (gate, _) = setup();
        let p1 = prepare(&gate, "s1", approve()).await;
        let p2 = prepare(
            &gate,
            "s1",
            changeset(vec![Change::SetState(SetState {
                state: "closed".into(),
            })]),
        )
        .await;
        assert_eq!(gate.pending("s1", None).unwrap().unwrap().id, p2.id);
        assert_eq!(
            gate.store().get_proposal(&p1.id).unwrap().unwrap().status,
            ProposalStatus::Pending
        );
        assert_eq!(
            gate.waiting(None)
                .unwrap()
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>(),
            vec![p1.id.clone(), p2.id.clone()]
        );
        assert_eq!(
            gate.on_user_turn("b", "confirm", None, "").await.unwrap(),
            (Verdict::Unclear, None)
        );
        assert!(gate.pending("s1", None).unwrap().is_some());
        let back = gate.bring_back(&p1.id, "s2").unwrap().unwrap();
        assert_eq!(back.id, p1.id);
        assert_eq!(gate.pending("s2", None).unwrap().unwrap().id, p1.id);
        assert!(gate.cancel_id(&p2.id, "never mind").unwrap().is_some());
        assert!(gate.cancel_id(&p2.id, "again").unwrap().is_none());
    }

    #[tokio::test]
    async fn the_sources_own_limit_and_unknown_operations_refuse() {
        let (gate, _) = setup();
        let err = gate
            .prepare(req("s1", "approve", issue("acme", "other", 1), approve()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, GateError::Policy(PolicyError::Refused(_))),
            "{err}"
        );
        let err = gate
            .submit(Request::new(
                "s",
                "nope",
                "x",
                Target::FakeIssue(target()),
                approve(),
                "",
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, GateError::UnknownOperation(_)), "{err}");
    }

    #[tokio::test]
    async fn audit_trail_and_sqlite_round_trip() {
        let (gate, _) = setup();
        let p = prepare(&gate, "s1", approve()).await;
        let back = gate.store().get_proposal(&p.id).unwrap().unwrap();
        assert_eq!(back.target, Target::FakeIssue(target()));
        assert_eq!(back.changes, approve());
        assert_eq!(labels(&back.snapshot), vec!["needs-signoff"]);
        gate.on_user_turn("s1", "confirm", None, "").await.unwrap();
        let events = gate.store().events(&p.id).unwrap();
        let names: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(&names[..3], ["prepared", "user_turn", "claimed"]);
        assert_eq!(names.last(), Some(&"finished"));
        assert_eq!(
            events[0].detail.as_ref().unwrap()["target"],
            "fake:acme/app#42"
        );
    }

    #[tokio::test]
    async fn two_confirms_at_once_run_the_change_once() {
        for unversioned in [false, true] {
            let (gate, writer) = setup_with(
                FakeWriter {
                    delay: Some(StdDuration::from_millis(10)),
                    unversioned,
                    ..Default::default()
                },
                "",
            );
            let p = prepare(&gate, "s1", approve()).await;
            gate.set_aside("s1", "hung up").unwrap();
            let later = async {
                tokio::time::sleep(StdDuration::from_millis(5)).await;
                gate.confirm_id(&p.id, "ui", None).await
            };
            let (first, second) = tokio::join!(gate.confirm_id(&p.id, "ui", None), later);
            let first = first.unwrap().unwrap();
            assert_eq!(first.status, OutcomeStatus::Success);
            assert!(second.unwrap().is_none());
            assert_eq!(
                gate.store().get_proposal(&p.id).unwrap().unwrap().status,
                ProposalStatus::Executed
            );
            assert_eq!(
                writer
                    .call_names()
                    .iter()
                    .filter(|c| **c == "set_labels")
                    .count(),
                1
            );
            assert!(
                !gate
                    .store()
                    .events(&p.id)
                    .unwrap()
                    .iter()
                    .any(|e| e.event == "stale")
            );
        }
    }

    #[tokio::test]
    async fn a_spoken_yes_and_a_click_at_once_run_the_change_once() {
        let (gate, writer) = setup_with(
            FakeWriter {
                delay: Some(StdDuration::from_millis(10)),
                ..Default::default()
            },
            "",
        );
        let p = prepare(&gate, "s1", approve()).await;
        let later = async {
            tokio::time::sleep(StdDuration::from_millis(5)).await;
            gate.confirm_id(&p.id, "ui", None).await
        };
        let (spoken, clicked) = tokio::join!(gate.on_user_turn("s1", "yes", None, ""), later);
        let ran = [spoken.unwrap().1.is_some(), clicked.unwrap().is_some()];
        assert_eq!(ran.iter().filter(|r| **r).count(), 1);
        assert_eq!(
            gate.store().get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Executed
        );
        assert_eq!(
            writer
                .call_names()
                .iter()
                .filter(|c| **c == "set_labels")
                .count(),
            1
        );
        for status in [
            ProposalStatus::Stale,
            ProposalStatus::Cancelled,
            ProposalStatus::Expired,
        ] {
            assert!(
                !gate.store().set_status(&p.id, status).unwrap(),
                "a finished proposal keeps its status"
            );
        }
        assert!(gate.cancel_id(&p.id, "late").unwrap().is_none());
    }

    // --- submit paths (ADR 0011, ADR 0034) ----------------------------------------------

    fn label() -> ChangeSet {
        changeset(vec![Change::SetLabels(SetLabels {
            add: vec!["bug".into()],
            remove: vec!["needs-signoff".into()],
        })])
    }

    fn close() -> ChangeSet {
        changeset(vec![
            Change::AddComment(AddComment { body: "dup".into() }),
            Change::SetState(SetState {
                state: "closed".into(),
            }),
        ])
    }

    #[tokio::test]
    async fn an_empty_policy_announces_and_executes_at_once() {
        let (gate, writer) = setup();
        let sub = gate
            .submit(req("s1", "set_labels", target(), label()))
            .await
            .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Announce);
        assert!(sub.executed());
        assert_eq!(sub.outcome.as_ref().unwrap().status, OutcomeStatus::Success);
        assert_eq!(writer.issue(&target()).labels, vec!["bug"]);
        assert!(gate.pending("s1", None).unwrap().is_none());
        assert!(
            sub.outcome
                .unwrap()
                .summary()
                .contains("Applied on acme/app#42")
        );
        let events = gate.store().events(&sub.proposal.id).unwrap();
        assert_eq!(events[0].detail.as_ref().unwrap()["autonomy"], "announce");
        assert_eq!(events.last().unwrap().event, "finished");
    }

    #[tokio::test]
    async fn even_a_high_risk_change_runs_unless_an_ask_rule_names_it() {
        let (gate, writer) = setup();
        let sub = gate
            .submit(req("s1", "close", target(), close()))
            .await
            .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Announce);
        assert!(sub.executed());
        let (gate, writer2) = setup_with(
            FakeWriter::default(),
            "[permissions]\nask = [\"github_issues.close\"]\n",
        );
        drop(writer);
        let sub = gate
            .submit(req("s1", "close", target(), close()))
            .await
            .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Confirm);
        assert!(!sub.executed());
        assert_eq!(
            gate.pending("s1", None).unwrap().unwrap().id,
            sub.proposal.id
        );
        assert_eq!(writer2.issue(&target()).state, "open");
        let (verdict, outcome) = gate
            .on_user_turn("s1", "yeah go ahead", None, "")
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Affirmative);
        assert_eq!(outcome.unwrap().status, OutcomeStatus::Success);
        assert_eq!(writer2.issue(&target()).state, "closed");
    }

    #[tokio::test]
    async fn the_policy_file_decides_and_deny_writes_nothing() {
        let (gate, _) = setup_with(
            FakeWriter::default(),
            "[github_issues]\nset_labels = \"confirm\"\nclose = \"quiet\"\n",
        );
        assert!(
            !gate
                .submit(req("s1", "set_labels", target(), label()))
                .await
                .unwrap()
                .executed()
        );
        let sub = gate
            .submit(req("s1", "close", target(), close()))
            .await
            .unwrap();
        assert_eq!(sub.autonomy, Autonomy::Quiet);
        assert!(sub.executed());

        let (gate, writer) = setup_with(
            FakeWriter::default(),
            "[github_issues.targets.\"acme/app\"]\nclose = \"deny\"\n",
        );
        let err = gate
            .submit(req("s1", "close", target(), close()))
            .await
            .unwrap_err();
        assert!(matches!(err, GateError::Policy(PolicyError::Refused(_))));
        assert_eq!(writer.issue(&target()).state, "open");
        assert_eq!(
            gate.level(
                "github_issues",
                "close",
                Some(&Target::FakeIssue(target())),
                None
            )
            .unwrap(),
            Autonomy::Deny
        );

        let (gate, writer) = setup_with(
            FakeWriter::default(),
            "[github_issues]\n\"*\" = \"quiet\"\n",
        );
        let other = issue("acme", "other", 1);
        writer.seed(&other);
        assert!(
            gate.submit(req("s1", "set_labels", other, label()))
                .await
                .is_err(),
            "the source's own limit still wins"
        );
    }

    #[tokio::test]
    async fn reversing_is_an_ordinary_change() {
        let (gate, writer) = setup_with(
            FakeWriter::default(),
            "[github_issues]\nclose = \"announce\"\nreopen = \"confirm\"\n",
        );
        let first = gate
            .submit(req("s1", "set_labels", target(), label()))
            .await
            .unwrap();
        let back = gate
            .submit(req(
                "s1",
                "set_labels",
                target(),
                changeset(vec![Change::SetLabels(SetLabels {
                    add: vec!["needs-signoff".into()],
                    remove: vec!["bug".into()],
                })]),
            ))
            .await
            .unwrap();
        assert!(back.executed());
        assert_eq!(writer.issue(&target()).labels, vec!["needs-signoff"]);
        assert_eq!(
            gate.store()
                .get_proposal(&first.proposal.id)
                .unwrap()
                .unwrap()
                .status,
            ProposalStatus::Executed
        );
        gate.submit(req("s1", "close", target(), close()))
            .await
            .unwrap();
        let reopen = gate
            .submit(req(
                "s1",
                "reopen",
                target(),
                changeset(vec![Change::SetState(SetState {
                    state: "open".into(),
                })]),
            ))
            .await
            .unwrap();
        assert_eq!(reopen.autonomy, Autonomy::Confirm);
        assert!(!reopen.executed());
        assert_eq!(writer.issue(&target()).state, "closed");
    }

    #[tokio::test]
    async fn a_blocked_change_is_refused_when_its_yes_arrives_late() {
        let (gate, writer) = setup();
        let p = prepare(&gate, "s1", close()).await;
        gate.set_autonomy(
            AutonomyPolicy::parse(
                "[permissions]\nblock = [\"github_issues.approve\"]\n",
                "<test>",
            )
            .unwrap(),
        );
        let (_, outcome) = gate.on_user_turn("s1", "yes", None, "").await.unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Failed);
        assert!(
            outcome
                .error
                .as_deref()
                .unwrap()
                .contains("permissions block that now")
        );
        assert_eq!(writer.issue(&target()).state, "open");
        assert_eq!(
            gate.store().get_proposal(&p.id).unwrap().unwrap().status,
            ProposalStatus::Cancelled
        );
    }

    struct Always(Mutex<Vec<(String, ConfirmationContext)>>);

    #[async_trait]
    impl ConfirmationClassifier for Always {
        fn name(&self) -> &str {
            "always"
        }
        async fn classify(&self, utterance: &str, ctx: &ConfirmationContext) -> Verdict {
            self.0
                .lock()
                .unwrap()
                .push((utterance.to_string(), ctx.clone()));
            Verdict::Affirmative
        }
    }

    #[tokio::test]
    async fn a_custom_classifier_is_used_with_context() {
        let seen = Arc::new(Always(Mutex::new(Vec::new())));
        let (gate, _) = setup();
        gate.set_classifier(seen.clone());
        gate.prepare(req("s1", "close", target(), close()))
            .await
            .unwrap();
        let (verdict, outcome) = gate
            .on_user_turn("s1", "mm hm", None, "Close it?")
            .await
            .unwrap();
        assert_eq!(verdict, Verdict::Affirmative);
        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Success);
        let calls = seen.0.lock().unwrap();
        assert_eq!(calls[0].1.question, "Close it?");
        assert!(calls[0].1.pending_action.contains("closed the issue"));
        assert_eq!(
            gate.store().events(&outcome.proposal_id).unwrap()[1]
                .detail
                .as_ref()
                .unwrap()["classifier"],
            "always"
        );
    }

    #[tokio::test]
    async fn the_operators_request_is_the_yes() {
        let (gate, writer) = setup();
        let p = gate
            .prepare(req(
                "s1",
                "close",
                target(),
                changeset(vec![Change::SetState(SetState {
                    state: "closed".into(),
                })]),
            ))
            .await
            .unwrap();
        let out = gate
            .confirm_by_request("s1", "close 42 please", "", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.status, OutcomeStatus::Success);
        assert_eq!(writer.issue(&target()).state, "closed");
        assert!(gate.pending("s1", None).unwrap().is_none());
        let turns: Vec<_> = gate
            .store()
            .events(&p.id)
            .unwrap()
            .into_iter()
            .filter(|e| e.event == "user_turn")
            .collect();
        assert_eq!(
            turns.last().unwrap().detail.as_ref().unwrap()["classifier"],
            "request"
        );

        let (gate, writer) = setup();
        gate.prepare(req(
            "s1",
            "close",
            target(),
            changeset(vec![Change::SetState(SetState {
                state: "closed".into(),
            })]),
        ))
        .await
        .unwrap();
        assert!(
            gate.confirm_by_request("s1", "we're not gonna do that one", "", None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            gate.pending("s1", None).unwrap().unwrap().status,
            ProposalStatus::Pending
        );
        assert_eq!(writer.issue(&target()).state, "open");
        // approve declares no words: it always asks
        gate.prepare(req("s1", "approve", target(), label()))
            .await
            .unwrap();
        assert!(
            gate.confirm_by_request("s1", "approve 42", "", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(gate.pending("s1", None).unwrap().is_some());
        assert!(matches!(
            writer.calls.lock().unwrap().last(),
            Some(Call::Fetch(_))
        ));
    }
}
