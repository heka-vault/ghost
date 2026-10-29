//! The routes. Local only, over a Unix socket; the daemon owns the state and implements
//! `Controller`; this module only translates HTTP into calls on it.
//!
//! What never passes through here: passive audio or passive transcripts (the idle-audio
//! rule), a password's value (write-only, ADR 0036), or a way for the model to reach it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Weak};

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path as PathArg, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Utc};
use ghost_core::gate::{Request, WriteGate};
use ghost_core::models::{
    ChangeSet, Outcome, OutcomeStatus as CoreOutcomeStatus, Proposal, Target, py_repr,
};
use ghost_core::outputs::list_outputs;
use ghost_core::policy::{AutonomyPolicy, write_routes};
use ghost_core::routes::{Carry as CoreCarry, RouteName as CoreRouteName, RoutePolicy};
use ghost_core::skills::SkillsError;
use ghost_core::store::{ProposalQuery, Store};
use ghost_core::waiting::State as ItemState;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;
use utoipa::{IntoParams, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::{Deps, SharedDeps, UI_SESSION};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, OptionalBody, check_len};
use crate::models::{
    API_VERSION, Ack, AuditEntry, AuditEventView, AuditPage, Carry, DevicesView, HookInfo, ItemKey,
    Level, Mode, ModelsView, OperationInfo, OutcomeStatus, OutcomeView, OutputInfo,
    PendingProposal, PermissionsView, PhonePairing, PhoneView, PolicyResult, PolicyUpdate,
    PolicyView, ReadToolInfo, Response as WatchResponse, RouteName, RoutesView, RuleAdd, RulesView,
    SayRequest, SecretState, SettingsResult, SettingsUpdate, SettingsView, SkillAdd, SkillBody,
    SkillInfo, SnoozeRequest, SourceInfo, Status, Turn, Urgency, Verify, WaitingItemView,
    WaitingState, WaitingView, WakeRequest, WatchInfo, WatchingView,
};
use crate::passwords::{
    SECRET_KEYS_SET, password_check, referenced, revive_apps_using, save_passwords, secret_states,
};
use crate::skill_catalog::skill_view;
use crate::state::now;

/// The document's name, version and words, as FastAPI wrote them.
pub const TITLE: &str = "Ghost control API";
pub const DESCRIPTION: &str = "Local control surface for ghostd over a Unix socket. Desktop and mobile apps talk to this; the model never does. Events stream over /events.";

// --- views ------------------------------------------------------------------------------

pub fn level_of(level: ghost_core::levels::Autonomy) -> Level {
    match level {
        ghost_core::levels::Autonomy::Deny => Level::Block,
        ghost_core::levels::Autonomy::Confirm => Level::Ask,
        ghost_core::levels::Autonomy::Quiet => Level::Quiet,
        ghost_core::levels::Autonomy::Announce => Level::Allow,
    }
}

pub fn mode_of(mode: ghost_core::permissions::Mode) -> Mode {
    match mode {
        ghost_core::permissions::Mode::Open => Mode::Open,
        ghost_core::permissions::Mode::Ask => Mode::Ask,
        ghost_core::permissions::Mode::ReadOnly => Mode::ReadOnly,
    }
}

pub fn core_mode(mode: Mode) -> ghost_core::permissions::Mode {
    match mode {
        Mode::Open => ghost_core::permissions::Mode::Open,
        Mode::Ask => ghost_core::permissions::Mode::Ask,
        Mode::ReadOnly => ghost_core::permissions::Mode::ReadOnly,
    }
}

pub fn route_name(r: CoreRouteName) -> RouteName {
    match r {
        CoreRouteName::Digest => RouteName::Digest,
        CoreRouteName::Desk => RouteName::Desk,
        CoreRouteName::Desktop => RouteName::Desktop,
        CoreRouteName::Phone => RouteName::Phone,
        CoreRouteName::Call => RouteName::Call,
        CoreRouteName::Sms => RouteName::Sms,
        CoreRouteName::Whatsapp => RouteName::Whatsapp,
    }
}

fn route_str(r: RouteName) -> &'static str {
    match r {
        RouteName::Digest => "digest",
        RouteName::Desk => "desk",
        RouteName::Desktop => "desktop",
        RouteName::Phone => "phone",
        RouteName::Call => "call",
        RouteName::Sms => "sms",
        RouteName::Whatsapp => "whatsapp",
    }
}

fn urgency_of(s: &str) -> Urgency {
    match s {
        "notify" => Urgency::Notify,
        "call" => Urgency::Call,
        _ => Urgency::Digest,
    }
}

fn urgency_str(u: Urgency) -> &'static str {
    match u {
        Urgency::Digest => "digest",
        Urgency::Notify => "notify",
        Urgency::Call => "call",
    }
}

/// `[permissions]` as the desktop edits it (ADR 0034).
pub fn permissions_view(p: &AutonomyPolicy, path: &Path) -> PermissionsView {
    let mut lists: BTreeMap<&str, Vec<String>> = p.permissions.rules().into_iter().collect();
    PermissionsView {
        path: path.display().to_string(),
        mode: mode_of(p.permissions.mode),
        block: lists.remove("block").unwrap_or_default(),
        ask: lists.remove("ask").unwrap_or_default(),
        quiet: lists.remove("quiet").unwrap_or_default(),
        allow: lists.remove("allow").unwrap_or_default(),
    }
}

pub fn pending_view(p: &Proposal, at: Option<DateTime<Utc>>) -> PendingProposal {
    let at = at.unwrap_or_else(now);
    let left = (p.expires_at - at).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
    PendingProposal {
        id: p.id.clone(),
        source: p.ability.clone(), // the store's column keeps its older name
        operation: p.operation.clone(),
        target: p.target.spoken(),
        summary: p.preview.clone(),
        rationale: p.rationale.clone(),
        expires_at: p.expires_at,
        expires_in_secs: left.max(0.0),
    }
}

pub fn outcome_view(o: &Outcome) -> OutcomeView {
    OutcomeView {
        proposal_id: o.proposal_id.clone(),
        target: o.target.spoken(),
        status: match o.status {
            CoreOutcomeStatus::Success => OutcomeStatus::Success,
            CoreOutcomeStatus::Partial => OutcomeStatus::Partial,
            CoreOutcomeStatus::Failed => OutcomeStatus::Failed,
        },
        applied: o.applied.clone(),
        failed_step: o.failed_step.clone(),
        error: o.error.clone(),
        state_now: o.readback.as_ref().map(|s| s.spoken()),
        summary: o.summary(),
    }
}

/// The one place a `Status` is built: the control API, its event stream and the phone
/// listener's `/hello` all use it.
pub fn status_snapshot(deps: &Deps) -> Status {
    let st = &deps.state;
    let pending = st
        .session_id()
        .and_then(|id| deps.gate.pending(&id, None).ok().flatten())
        .map(|p| pending_view(&p, None));
    let mut apps: Vec<String> = match &deps.apps {
        Some(apps) => apps.names().into_iter().filter(|n| apps.is_up(n)).collect(),
        None => Vec::new(),
    };
    apps.sort();
    Status {
        phase: st.phase(),
        session_id: st.session_id(),
        models_ready: st.models_ready(),
        uptime_secs: st.uptime_secs(),
        version: st.version(),
        build: st.build(),
        api_version: API_VERSION.to_string(),
        wake_mode: st.wake_mode(),
        apps,
        pending,
        conversation: st.conversation(),
        session_origin: st.session_origin(),
        ending: st.ending(),
        waiting: st.waiting(),
    }
}

/// The waiting list (ADR 0029), re-read now: the control API and the phone listener both
/// serve this.
pub fn waiting_view(deps: &Deps) -> ApiResult<WaitingView> {
    let Some(waiting) = &deps.waiting else {
        return Ok(WaitingView {
            items: Vec::new(),
            count: 0,
        });
    };
    let at = now();
    let items = waiting
        .refresh(Some(at))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let changes: BTreeMap<String, Proposal> = deps
        .gate
        .waiting(Some(at))
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();
    let mut out = Vec::new();
    for i in &items {
        let change = if i.source == ghost_core::waiting::GATE {
            i.fields
                .get("proposal")
                .and_then(Value::as_str)
                .and_then(|id| changes.get(id))
        } else {
            None
        };
        out.push(WaitingItemView {
            key: i.key.clone(),
            source: i.source.clone(),
            title: i.title.clone(),
            link: i.link.clone(),
            state: match i.state(Some(at)) {
                ItemState::New => WaitingState::New,
                ItemState::Delivered => WaitingState::Delivered,
                ItemState::Acknowledged => WaitingState::Acknowledged,
                ItemState::Snoozed => WaitingState::Snoozed,
            },
            first_seen: i.first_seen,
            changed_at: i.changed_at,
            age_secs: i.age_seconds(Some(at)),
            delivered_at: i.delivered_at,
            delivered_via: i.delivered_via.clone(),
            acknowledged_at: i.acknowledged_at,
            snoozed_until: i.snoozed_until,
            change: change.map(|c| pending_view(c, Some(at))),
        });
    }
    Ok(WaitingView {
        count: items.iter().filter(|i| i.needs_alerting(Some(at))).count() as u64,
        items: out,
    })
}

/// A page of the audit log, newest first. Shared by the control API and the phone.
pub fn audit_page(
    store: &Store,
    limit: i64,
    offset: i64,
    source: Option<&str>,
    status: Option<&str>,
    session_id: Option<&str>,
) -> ApiResult<AuditPage> {
    let q = ProposalQuery {
        limit,
        offset,
        ability: source,
        status,
        session_id,
        ..Default::default()
    };
    let (rows, total) = store
        .list_proposals(&q)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut items = Vec::with_capacity(rows.len());
    for p in rows {
        let events = store
            .events(&p.id)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .into_iter()
            .map(|ev| AuditEventView {
                at: ghost_core::time::parse_iso(&ev.at).unwrap_or_else(now),
                event: ev.event,
                detail: event_detail(ev.detail),
            })
            .collect();
        items.push(AuditEntry {
            id: p.id.clone(),
            session_id: p.session_id.clone(),
            source: p.ability.clone(),
            operation: p.operation.clone(),
            target: p.target.spoken(),
            summary: p.preview.clone(),
            autonomy: p.autonomy.clone(),
            status: p.status.as_str().to_string(),
            created_at: p.created_at,
            events,
            conversation: p.conversation.clone(),
        });
    }
    Ok(AuditPage {
        items,
        total,
        limit,
        offset,
    })
}

/// An audit event's detail as the API says it: the source, where the store's rows keep
/// the older word (ability).
fn event_detail(detail: Option<Value>) -> Option<Map<String, Value>> {
    let Value::Object(m) = detail? else {
        return None;
    };
    if !m.contains_key("ability") {
        return Some(m);
    }
    Some(
        m.into_iter()
            .map(|(k, v)| (if k == "ability" { "source".into() } else { k }, v))
            .collect(),
    )
}

/// A change of Ghost's own from the desktop: through the gate's internal path, audited
/// under the desktop's own session. A refusal is a 409 the operator can read.
pub async fn apply_internal(
    gate: &WriteGate,
    ability: &str,
    operation: &str,
    target: Target,
    changes: ChangeSet,
    rationale: &str,
) -> ApiResult<Outcome> {
    let req = Request::new(UI_SESSION, ability, operation, target, changes, rationale);
    let out = gate
        .apply_internal(req)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    if out.status != CoreOutcomeStatus::Success {
        return Err(ApiError::conflict(
            out.error.clone().unwrap_or_else(|| "not applied".into()),
        ));
    }
    Ok(out)
}

// --- query strings, as FastAPI refused them ------------------------------------------------

/// A query string read with a 422 for what doesn't parse, as the body is.
pub struct Qs<T>(pub T);

impl<S, T> FromRequestParts<S> for Qs<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(q)| Qs(q))
            .map_err(|e| {
                ApiError::invalid(
                    &["query"],
                    crate::error::scrub(&e.body_text()),
                    "value_error",
                )
            })
    }
}

// --- status + control ------------------------------------------------------------------

#[utoipa::path(get, path = "/status", operation_id = "get_status_status_get", tag = "session",
    summary = "Get Status", responses((status = 200, description = "Successful Response", body = Status)))]
async fn get_status(State(deps): State<SharedDeps>) -> Json<Status> {
    Json(status_snapshot(&deps))
}

#[utoipa::path(post, path = "/wake", operation_id = "wake_wake_post", tag = "session", summary = "Wake",
    request_body = Option<WakeRequest>,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn wake(
    State(deps): State<SharedDeps>,
    OptionalBody(req): OptionalBody<WakeRequest>,
) -> ApiResult<Json<Ack>> {
    let (text, conversation) = req.map_or((None, None), |r| (r.text, r.conversation));
    let ok = deps.control.wake(text, conversation).await;
    Ok(Json(if ok {
        Ack::ok()
    } else {
        Ack::no("not passive, or no such line")
    }))
}

#[utoipa::path(post, path = "/sleep", operation_id = "sleep_sleep_post", tag = "session", summary = "Sleep",
    responses((status = 200, description = "Successful Response", body = Ack)))]
async fn sleep(State(deps): State<SharedDeps>) -> Json<Ack> {
    let ok = deps.control.sleep().await;
    Json(if ok { Ack::ok() } else { Ack::no("no session") })
}

#[utoipa::path(post, path = "/session/say", operation_id = "say_session_say_post", tag = "session",
    summary = "Say", request_body = SayRequest,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn say(State(deps): State<SharedDeps>, Body(req): Body<SayRequest>) -> ApiResult<Json<Ack>> {
    check_len(&["body", "text"], &req.text, 1, 4000)?;
    let ok = deps.control.say(req.text, req.conversation).await;
    Ok(Json(if ok {
        Ack::ok()
    } else {
        Ack::no("daemon is still starting")
    }))
}

#[utoipa::path(get, path = "/session/turns", operation_id = "turns_session_turns_get", tag = "session",
    summary = "Turns", responses((status = 200, description = "Successful Response", body = Vec<Value>)))]
async fn turns(State(deps): State<SharedDeps>) -> Json<Vec<Turn>> {
    Json(deps.state.turns())
}

// --- pending proposal --------------------------------------------------------------------

#[utoipa::path(get, path = "/proposals/pending", operation_id = "pending_proposals_pending_get",
    tag = "writes", summary = "Pending",
    responses((status = 200, description = "Successful Response", body = Option<PendingProposal>)))]
async fn pending(State(deps): State<SharedDeps>) -> Json<Option<PendingProposal>> {
    Json(status_snapshot(&deps).pending)
}

#[utoipa::path(post, path = "/proposals/pending/confirm",
    operation_id = "confirm_proposals_pending_confirm_post", tag = "writes", summary = "Confirm",
    responses((status = 200, description = "Successful Response", body = OutcomeView)))]
async fn confirm(State(deps): State<SharedDeps>) -> ApiResult<Json<OutcomeView>> {
    let outcome = deps
        .control
        .confirm_pending()
        .await
        .ok_or_else(|| ApiError::conflict("nothing pending"))?;
    Ok(Json(outcome_view(&outcome)))
}

#[utoipa::path(post, path = "/proposals/pending/cancel",
    operation_id = "cancel_proposals_pending_cancel_post", tag = "writes", summary = "Cancel",
    responses((status = 200, description = "Successful Response", body = Ack)))]
async fn cancel(State(deps): State<SharedDeps>) -> Json<Ack> {
    Json(Ack {
        ok: deps.control.cancel_pending().await,
        detail: None,
    })
}

/// A Confirm on a change on the waiting list (ADR 0030): the stale check runs.
#[utoipa::path(post, path = "/proposals/{proposal_id}/confirm",
    operation_id = "confirm_one_proposals__proposal_id__confirm_post", tag = "writes",
    summary = "Confirm One",
    description = "A Confirm on a change on the waiting list (ADR 0030): the stale check runs.",
    params(("proposal_id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = OutcomeView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn confirm_one(
    State(deps): State<SharedDeps>,
    PathArg(proposal_id): PathArg<String>,
) -> ApiResult<Json<OutcomeView>> {
    let outcome = deps
        .control
        .confirm_proposal(&proposal_id)
        .await
        .ok_or_else(|| ApiError::conflict("that change isn't waiting"))?;
    Ok(Json(outcome_view(&outcome)))
}

#[utoipa::path(post, path = "/proposals/{proposal_id}/cancel",
    operation_id = "cancel_one_proposals__proposal_id__cancel_post", tag = "writes",
    summary = "Cancel One", params(("proposal_id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn cancel_one(
    State(deps): State<SharedDeps>,
    PathArg(proposal_id): PathArg<String>,
) -> Json<Ack> {
    Json(Ack {
        ok: deps.control.cancel_proposal(&proposal_id).await,
        detail: None,
    })
}

// --- the waiting list (ADR 0029) ----------------------------------------------------------

#[utoipa::path(get, path = "/waiting", operation_id = "waiting_waiting_get", tag = "waiting",
    summary = "Waiting", responses((status = 200, description = "Successful Response", body = WaitingView)))]
async fn waiting(State(deps): State<SharedDeps>) -> ApiResult<Json<WaitingView>> {
    Ok(Json(waiting_view(&deps)?))
}

/// The operator heard it: it stops alerting until it changes. Not a gate write; it
/// changes Ghost's own bookkeeping only.
#[utoipa::path(post, path = "/waiting/ack", operation_id = "acknowledge_waiting_ack_post", tag = "waiting",
    summary = "Acknowledge",
    description = "The operator heard it: it stops alerting until it changes. Not a gate write;\nit changes Ghost's own bookkeeping only.",
    request_body = ItemKey,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn acknowledge(
    State(deps): State<SharedDeps>,
    Body(body): Body<ItemKey>,
) -> ApiResult<Json<Ack>> {
    let ok = match &deps.waiting {
        Some(w) => w
            .acknowledge(&body.key, None, None)
            .map_err(|e| ApiError::internal(e.to_string()))?,
        None => false,
    };
    Ok(Json(Ack { ok, detail: None }))
}

#[utoipa::path(post, path = "/waiting/snooze", operation_id = "snooze_waiting_snooze_post", tag = "waiting",
    summary = "Snooze", request_body = SnoozeRequest,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn snooze(
    State(deps): State<SharedDeps>,
    Body(body): Body<SnoozeRequest>,
) -> ApiResult<Json<Ack>> {
    crate::error::check_range(&["body", "minutes"], body.minutes, 1, 7 * 24 * 60)?;
    let until = now() + chrono::Duration::minutes(body.minutes);
    let ok = match &deps.waiting {
        Some(w) => w
            .snooze(&body.key, until)
            .map_err(|e| ApiError::internal(e.to_string()))?,
        None => false,
    };
    Ok(Json(Ack { ok, detail: None }))
}

// --- audit ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    #[param(minimum = 1, maximum = 500, default = 50)]
    pub limit: Option<i64>,
    #[param(minimum = 0, default = 0)]
    pub offset: Option<i64>,
    pub source: Option<String>,
    pub status: Option<String>,
    pub session_id: Option<String>,
}

/// Proposals, newest first; source narrows to one app or one of Ghost's own.
#[utoipa::path(get, path = "/audit", operation_id = "audit_audit_get", tag = "writes", summary = "Audit",
    description = "Proposals, newest first; source narrows to one app or one of Ghost's own.",
    params(AuditQuery),
    responses((status = 200, description = "Successful Response", body = AuditPage),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn audit(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<AuditQuery>,
) -> ApiResult<Json<AuditPage>> {
    let limit = q.limit.unwrap_or(50);
    let offset = q.offset.unwrap_or(0);
    crate::error::check_range(&["query", "limit"], limit, 1, 500)?;
    crate::error::check_range(&["query", "offset"], offset, 0, i64::MAX)?;
    Ok(Json(audit_page(
        &deps.store,
        limit,
        offset,
        q.source.as_deref(),
        q.status.as_deref(),
        q.session_id.as_deref(),
    )?))
}

// --- the paired phone (ADR 0016) ---------------------------------------------------------

/// A new six-digit code for the phone, good for two minutes and five tries. It replaces
/// any earlier code. 409 when GHOST_PHONE_LISTEN is off.
#[utoipa::path(post, path = "/phone/pairing", operation_id = "phone_pairing_phone_pairing_post", tag = "phone",
    summary = "Phone Pairing",
    description = "A new six-digit code for the phone, good for two minutes and five tries. It\nreplaces any earlier code. 409 when GHOST_PHONE_LISTEN is off.",
    responses((status = 200, description = "Successful Response", body = PhonePairing)))]
async fn phone_pairing(State(deps): State<SharedDeps>) -> ApiResult<Json<PhonePairing>> {
    deps.control
        .phone_pairing()
        .map(Json)
        .ok_or_else(|| ApiError::conflict("the phone listener is off (set GHOST_PHONE_LISTEN)"))
}

#[utoipa::path(get, path = "/phone", operation_id = "phone_phone_get", tag = "phone", summary = "Phone",
    responses((status = 200, description = "Successful Response", body = PhoneView)))]
async fn phone(State(deps): State<SharedDeps>) -> Json<PhoneView> {
    Json(deps.control.phone_view())
}

/// Forget the paired phone; its signatures stop working at once.
#[utoipa::path(delete, path = "/phone", operation_id = "forget_phone_phone_delete", tag = "phone",
    summary = "Forget Phone", description = "Forget the paired phone; its signatures stop working at once.",
    responses((status = 200, description = "Successful Response", body = Ack)))]
async fn forget_phone(State(deps): State<SharedDeps>) -> Json<Ack> {
    let ok = deps.control.forget_phone().await;
    Json(if ok {
        Ack::ok()
    } else {
        Ack::no("no phone paired")
    })
}

// --- what Ghost watches, its rules and what it made (ADR 0035) ----------------------------

fn hook_info(deps: &Deps, h: &ghost_core::hooks::Hook) -> HookInfo {
    let path = h.path();
    let intake = deps.hooks.as_deref();
    let st = intake.and_then(|i| i.state_of(&path));
    let counts = |m: &BTreeMap<String, u64>| -> Map<String, Value> {
        m.iter().map(|(k, v)| (k.clone(), json!(v))).collect()
    };
    HookInfo {
        name: h.name.clone(),
        path: path.clone(),
        verify: match h.verify {
            ghost_core::hooks::Scheme::Github => Verify::Github,
            ghost_core::hooks::Scheme::Hmac => Verify::Hmac,
        },
        events: h.events.keys().cloned().collect(),
        on: intake.is_some_and(|i| i.knows(&path)),
        off_reason: match intake {
            Some(i) => i
                .off
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&path)
                .cloned(),
            None => Some("the listener is off".into()),
        },
        accepted: st.as_ref().map(|s| counts(&s.accepted)).unwrap_or_default(),
        rejected: st.as_ref().map(|s| counts(&s.rejected)).unwrap_or_default(),
        last: st.and_then(|s| s.last),
    }
}

fn watch_info(deps: &Deps, w: &ghost_core::watches::Watch) -> WatchInfo {
    let st = deps.watches.as_ref().and_then(|r| r.state_of(&w.source()));
    let mut steps = vec!["check".to_string()];
    if w.jev.is_some() {
        steps.push("jev".into());
    }
    if w.model.is_some() {
        steps.push("model".into());
    }
    WatchInfo {
        name: w.name.clone(),
        app: w.app.clone(),
        read: w.read.clone(),
        every_secs: w.every as i64,
        steps,
        response: match w.response.response() {
            ghost_core::waiting::Response::Tell => WatchResponse::Tell,
            ghost_core::waiting::Response::Record => WatchResponse::Record,
        },
        urgency: urgency_of(&w.urgency),
        last_run: st.as_ref().and_then(|s| s.last_run),
        last_ok: st.as_ref().and_then(|s| s.last_ok),
        error: st.as_ref().and_then(|s| s.error.clone()),
        seen: st.as_ref().map_or(0, |s| s.seen as i64),
        open: st.as_ref().map_or(0, |s| s.open as i64),
    }
}

/// The watches in watches.json and the webhooks in hooks.json, each with its last look
/// and its counts, re-read now; where the webhook listener is; and what is wrong in those
/// files or left over in Ghost's folder (ADR 0035).
#[utoipa::path(get, path = "/watches", operation_id = "watching_watches_get", tag = "watching",
    summary = "Watching",
    description = "The watches in watches.json and the webhooks in hooks.json, each with its last\nlook and its counts, re-read now; where the webhook listener is; and what is wrong\nin those files or left over in Ghost's folder (ADR 0035).",
    responses((status = 200, description = "Successful Response", body = WatchingView)))]
async fn watching(State(deps): State<SharedDeps>) -> Json<WatchingView> {
    let Some(folder) = &deps.folder else {
        return Json(WatchingView {
            listening: deps.hooks_listening.clone(),
            ..Default::default()
        });
    };
    let mut folder = folder.lock().unwrap_or_else(|e| e.into_inner());
    folder.refresh(); // the daemon follows: changed watches restart
    Json(WatchingView {
        watches: folder
            .watches
            .values()
            .map(|w| watch_info(&deps, w))
            .collect(),
        hooks: folder.hooks.values().map(|h| hook_info(&deps, h)).collect(),
        listening: deps.hooks_listening.clone(),
        problems: folder.problems.clone(),
    })
}

/// Ghost's AGENTS.md: the rules in every agent's prompt (ADR 0035).
#[utoipa::path(get, path = "/rules", operation_id = "get_rules_rules_get", tag = "rules", summary = "Get Rules",
    description = "Ghost's AGENTS.md: the rules in every agent's prompt (ADR 0035).",
    responses((status = 200, description = "Successful Response", body = RulesView)))]
async fn get_rules(State(deps): State<SharedDeps>) -> ApiResult<Json<RulesView>> {
    let rules = deps
        .rules
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("rules are not available"))?;
    Ok(Json(RulesView {
        path: rules.path.display().to_string(),
        text: rules.text(),
    }))
}

/// Append a rule to Ghost's AGENTS.md, verbatim, through the gate: the operator's own
/// click, audited like the spoken `rules.add` (ADR 0035). Never written here.
#[utoipa::path(post, path = "/rules", operation_id = "add_rule_rules_post", tag = "rules", summary = "Add Rule",
    description = "Append a rule to Ghost's AGENTS.md, verbatim, through the gate: the operator's\nown click, audited like the spoken ``rules.add`` (ADR 0035). Never written here.",
    request_body = RuleAdd,
    responses((status = 200, description = "Successful Response", body = OutcomeView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn add_rule(
    State(deps): State<SharedDeps>,
    Body(req): Body<RuleAdd>,
) -> ApiResult<Json<OutcomeView>> {
    check_len(&["body", "text"], &req.text, 1, 1000)?;
    let rules = deps
        .rules
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("rules are not available"))?;
    let prepared = rules
        .prepare_add(&req.text, None)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let out = apply_internal(
        &deps.gate,
        ghost_core::rules::SOURCE,
        "add",
        prepared.target,
        prepared.changes,
        "a rule added in the desktop app",
    )
    .await?;
    Ok(Json(outcome_view(&out)))
}

/// The files Ghost made in its out/ folder (ADR 0033), by name.
#[utoipa::path(get, path = "/outputs", operation_id = "outputs_outputs_get", tag = "outputs", summary = "Outputs",
    description = "The files Ghost made in its out/ folder (ADR 0033), by name.",
    responses((status = 200, description = "Successful Response", body = Vec<OutputInfo>)))]
async fn outputs(State(deps): State<SharedDeps>) -> Json<Vec<OutputInfo>> {
    let Some(outputs) = &deps.outputs else {
        return Json(Vec::new());
    };
    Json(
        list_outputs(&outputs.config_dir)
            .into_iter()
            .map(|f| OutputInfo {
                name: f.name,
                path: f.path.display().to_string(),
                bytes: f.bytes,
                modified: DateTime::from_timestamp(
                    f.modified.trunc() as i64,
                    ((f.modified.fract()) * 1e9) as u32,
                )
                .unwrap_or_else(now),
            })
            .collect(),
    )
}

// --- skills (ADR 0023) ---------------------------------------------------------------------

#[utoipa::path(get, path = "/skills", operation_id = "skills_skills_get", tag = "skills", summary = "Skills",
    responses((status = 200, description = "Successful Response", body = Vec<SkillInfo>)))]
async fn skills(State(deps): State<SharedDeps>) -> Json<Vec<SkillInfo>> {
    let Some(skills) = &deps.skills else {
        return Json(Vec::new());
    };
    skills.refresh();
    let origins = skills.origins();
    Json(
        skills
            .all()
            .iter()
            .map(|s| skill_view(&deps, s, Some(&origins)))
            .collect(),
    )
}

#[utoipa::path(get, path = "/skills/{name}", operation_id = "skill_body_skills__name__get", tag = "skills",
    summary = "Skill Body", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SkillBody),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn skill_body(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<SkillBody>> {
    let skill = deps
        .skills
        .as_ref()
        .and_then(|s| s.get(&name))
        .ok_or_else(|| ApiError::not_found(format!("no skill {}", py_repr(&name))))?;
    let body = skill
        .body()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(SkillBody {
        name: skill.name.clone(),
        body: body.chars().take(20000).collect(),
        files: skill.files(),
    }))
}

/// Add or remove through the gate at quiet: the operator's own click, audited.
async fn skill_change(
    deps: &Deps,
    op: &str,
    args: Map<String, Value>,
) -> ApiResult<Option<SkillInfo>> {
    let skills = deps
        .skills
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("skills are not available"))?;
    let operation = skills
        .operations()
        .into_iter()
        .find(|o| o.name == op)
        .ok_or_else(|| ApiError::internal(format!("no skills operation {op}")))?;
    let prepared =
        (operation.prepare)(args)
            .await
            .map_err(|e| match e.downcast_ref::<SkillsError>() {
                Some(SkillsError::NotFound(_)) => ApiError::not_found(e.to_string()),
                _ => ApiError::conflict(e.to_string()),
            })?;
    let name = match &prepared.target {
        Target::Skill(t) => t.name.clone(),
        other => other.spoken(),
    };
    apply_internal(
        &deps.gate,
        ghost_core::skills::SOURCE,
        op,
        prepared.target,
        prepared.changes,
        &format!("{op} from the desktop app"),
    )
    .await?;
    skills.refresh();
    Ok(skills.get(&name).map(|s| skill_view(deps, &s, None)))
}

#[utoipa::path(post, path = "/skills", operation_id = "add_skill_skills_post", tag = "skills", summary = "Add Skill",
    request_body = SkillAdd,
    responses((status = 200, description = "Successful Response", body = SkillInfo),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn add_skill(
    State(deps): State<SharedDeps>,
    Body(req): Body<SkillAdd>,
) -> ApiResult<Json<SkillInfo>> {
    check_len(&["body", "path"], &req.path, 1, usize::MAX)?;
    let mut args = Map::new();
    args.insert("path".into(), json!(req.path));
    args.insert("name".into(), json!(req.name));
    args.insert("replace".into(), json!(req.replace));
    skill_change(&deps, "add", args)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::internal("the skill was copied but could not be read back"))
}

#[utoipa::path(delete, path = "/skills/{name}", operation_id = "remove_skill_skills__name__delete",
    tag = "skills", summary = "Remove Skill", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn remove_skill(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let mut args = Map::new();
    args.insert("name".into(), json!(name));
    skill_change(&deps, "remove", args).await?;
    Ok(Json(Ack::ok()))
}

// --- sources: what the gate knows (ADR 0034) ----------------------------------------------

fn operation_infos(
    autonomy: &AutonomyPolicy,
    name: &str,
    operations: &[ghost_core::source::Operation],
) -> Vec<OperationInfo> {
    operations
        .iter()
        .map(|op| OperationInfo {
            name: op.name.clone(),
            description: op.description.clone(),
            risk: op.risk.as_str().to_string(),
            autonomy: autonomy.resolve(name, &op.name, None).as_str().to_string(),
            properties: op.properties.as_object().cloned().unwrap_or_default(),
            required: op.required.clone(),
        })
        .collect()
}

fn read_tool_infos(tools: &[ghost_core::source::ReadTool]) -> Vec<ReadToolInfo> {
    tools
        .iter()
        .map(|t| ReadToolInfo {
            name: t.name.clone(),
            description: t.description.clone(),
        })
        .collect()
}

#[utoipa::path(get, path = "/sources", operation_id = "sources_sources_get", tag = "sources", summary = "Sources",
    responses((status = 200, description = "Successful Response", body = Vec<SourceInfo>)))]
async fn sources(State(deps): State<SharedDeps>) -> Json<Vec<SourceInfo>> {
    let autonomy = deps.gate.autonomy();
    let mut out: Vec<SourceInfo> = Vec::new();
    if let Some(convs) = &deps.conversations {
        // The agents' own operations (ADR 0014): built in, always on, and named in the
        // permissions like any app's.
        let name = ghost_core::conversations::SOURCE;
        let mut operations = operation_infos(&autonomy, name, &convs.operations());
        let mut props = Map::new();
        props.insert("name".into(), json!({"type": "string"}));
        operations.push(OperationInfo {
            name: "switch".into(),
            description: "Transfer the operator to one of their agents.".into(),
            risk: "low".into(),
            autonomy: autonomy.resolve(name, "switch", None).as_str().to_string(),
            properties: props,
            required: vec!["name".into()],
        });
        out.push(SourceInfo {
            name: name.into(),
            kind: "built-in".into(),
            description: "Your agents: hire, retire, persona, voice, memory (ADR 0014)".into(),
            operations,
            read_tools: read_tool_infos(&convs.read_tools()),
        });
    }
    if let Some(rules) = &deps.rules {
        let name = ghost_core::rules::SOURCE;
        out.push(SourceInfo {
            name: name.into(),
            kind: "built-in".into(),
            description: "Your rules for every agent: a rule added (ADR 0035)".into(),
            operations: operation_infos(&autonomy, name, &rules.operations()),
            read_tools: Vec::new(),
        });
    }
    let apps = match &deps.apps {
        Some(apps) => apps.sources(),
        None => deps.sources.clone(),
    };
    for src in apps {
        if out.iter().any(|s| s.name == src.name) {
            continue;
        }
        out.push(SourceInfo {
            name: src.name.clone(),
            kind: "app".into(),
            description: if src.title.is_empty() {
                src.name.clone()
            } else {
                src.title.clone()
            },
            operations: operation_infos(&autonomy, &src.name, &src.operations),
            read_tools: read_tool_infos(&src.read_tools),
        });
    }
    Json(out)
}

// --- policy ----------------------------------------------------------------------------------

fn policy_text(deps: &Deps) -> (bool, String) {
    let exists = deps.policy_path.exists();
    let text = if exists {
        std::fs::read_to_string(&deps.policy_path).unwrap_or_default()
    } else {
        String::new()
    };
    (exists, text)
}

/// The file parsed; `AutonomyPolicy()` for an empty one, a 422 for one that doesn't read.
fn parse_policy(deps: &Deps, text: &str) -> ApiResult<AutonomyPolicy> {
    if text.is_empty() {
        return Ok(AutonomyPolicy::default());
    }
    AutonomyPolicy::parse(text, &deps.policy_path.display().to_string())
        .map_err(|e| ApiError::unprocessable(e.to_string()))
}

fn routes_view(deps: &Deps, raw: Option<&toml::Table>) -> ApiResult<RoutesView> {
    let r =
        RoutePolicy::parse(raw, "routes").map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let mut available = vec![RouteName::Digest];
    for name in deps.control.routes_available() {
        let name = route_name(name);
        if !available.contains(&name) {
            available.push(name);
        }
    }
    Ok(RoutesView {
        enabled: r.enabled.iter().map(|n| route_name(*n)).collect(),
        escalate_after: r.escalate_after,
        quiet_hours: r
            .quiet_hours
            .map(|(a, b)| format!("{}-{}", a.format("%H:%M"), b.format("%H:%M"))),
        batch_secs: r.batch_secs,
        calls_per_hour: r.calls_per_hour,
        calls_per_day: r.calls_per_day,
        carry: match r.carry {
            CoreCarry::Count => Carry::Count,
            CoreCarry::Title => Carry::Title,
        },
        greeting: r.greeting.clone(),
        urgency: r
            .urgency
            .iter()
            .map(|(k, v)| (k.clone(), urgency_of(v.as_str())))
            .collect(),
        available,
    })
}

fn policy_view(deps: &Deps) -> ApiResult<PolicyView> {
    let (exists, text) = policy_text(deps);
    let p = parse_policy(deps, &text)?;
    Ok(PolicyView {
        path: deps.policy_path.display().to_string(),
        exists,
        permissions: permissions_view(&p, &deps.policy_path),
        routes: Some(routes_view(deps, p.routes.as_ref())?),
        text,
    })
}

/// `[routes]` as the file would hold it, from the fields the desktop edited (`available`
/// is the daemon's, not the file's).
fn routes_table(v: &RoutesView) -> toml::Table {
    let mut t = toml::Table::new();
    t.insert(
        "enabled".into(),
        toml::Value::Array(
            v.enabled
                .iter()
                .map(|r| toml::Value::String(route_str(*r).into()))
                .collect(),
        ),
    );
    t.insert(
        "escalate_after".into(),
        toml::Value::Integer(v.escalate_after),
    );
    if let Some(q) = &v.quiet_hours {
        t.insert("quiet_hours".into(), toml::Value::String(q.clone()));
    }
    t.insert("batch_secs".into(), toml::Value::Integer(v.batch_secs));
    t.insert(
        "calls_per_hour".into(),
        toml::Value::Integer(v.calls_per_hour),
    );
    t.insert(
        "calls_per_day".into(),
        toml::Value::Integer(v.calls_per_day),
    );
    t.insert(
        "carry".into(),
        toml::Value::String(
            match v.carry {
                Carry::Count => "count",
                Carry::Title => "title",
            }
            .into(),
        ),
    );
    t.insert("greeting".into(), toml::Value::String(v.greeting.clone()));
    let mut urgency = toml::Table::new();
    for (k, u) in &v.urgency {
        urgency.insert(k.clone(), toml::Value::String(urgency_str(*u).into()));
    }
    t.insert("urgency".into(), toml::Value::Table(urgency));
    t
}

fn write_policy(deps: &Deps, text: &str) -> ApiResult<()> {
    if let Some(parent) = deps.policy_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let text = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    std::fs::write(&deps.policy_path, text).map_err(|e| ApiError::internal(e.to_string()))
}

#[utoipa::path(get, path = "/policy", operation_id = "get_policy_policy_get", tag = "policy", summary = "Get Policy",
    responses((status = 200, description = "Successful Response", body = PolicyView)))]
async fn get_policy(State(deps): State<SharedDeps>) -> ApiResult<Json<PolicyView>> {
    Ok(Json(policy_view(&deps)?))
}

#[utoipa::path(put, path = "/policy", operation_id = "put_policy_policy_put", tag = "policy", summary = "Put Policy",
    request_body = PolicyUpdate,
    responses((status = 200, description = "Successful Response", body = PolicyResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_policy(
    State(deps): State<SharedDeps>,
    Body(update): Body<PolicyUpdate>,
) -> ApiResult<Json<PolicyResult>> {
    let text = match update.text {
        Some(text) => {
            // never a file that won't read
            AutonomyPolicy::parse(&text, "request")
                .map_err(|e| ApiError::unprocessable(e.to_string()))?;
            text
        }
        None => {
            let current = policy_view(&deps)?;
            let Some(routes) = update.routes else {
                return Ok(Json(PolicyResult {
                    policy: current,
                    applied: false,
                }));
            };
            let table = routes_table(&routes);
            let parsed = RoutePolicy::parse(Some(&table), "routes")
                .map_err(|e| ApiError::unprocessable(e.to_string()))?;
            // The routes editor doesn't know the rest of the file: only [routes] changes,
            // and everything else stays as written, comments included.
            write_routes(&current.text, &parsed.to_toml())
                .map_err(|e| ApiError::unprocessable(e.to_string()))?
        }
    };
    write_policy(&deps, &text)?;
    deps.control
        .reload_policy()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(PolicyResult {
        policy: policy_view(&deps)?,
        applied: true,
    }))
}

// --- settings --------------------------------------------------------------------------------

fn settings_view(deps: &Deps) -> SettingsView {
    // `values` is whatever non-secret keys settings.env holds; the desktop app owns the
    // list it offers. The passwords are the vault's (ADR 0036): set or not, never a value.
    // A password written into settings.env by hand is never shown; the waiting list asks
    // for its import.
    let is_password = password_check(deps, None);
    let values: BTreeMap<String, String> = deps
        .settings()
        .into_iter()
        .filter(|(k, _)| !is_password(k))
        .collect();
    let mut names: Vec<String> = SECRET_KEYS_SET.iter().map(|s| (*s).to_string()).collect();
    names.sort();
    let extra: BTreeSet<String> = referenced(deps)
        .into_iter()
        .filter(|n| !SECRET_KEYS_SET.contains(&n.as_str()))
        .collect();
    names.extend(extra);
    SettingsView {
        settings_path: deps.settings_path.display().to_string(),
        values,
        secrets: secret_states(deps, &names)
            .into_iter()
            .map(|(name, set)| SecretState { name, set })
            .collect(),
    }
}

#[utoipa::path(get, path = "/settings", operation_id = "get_settings_settings_get", tag = "settings",
    summary = "Get Settings", responses((status = 200, description = "Successful Response", body = SettingsView)))]
async fn get_settings(State(deps): State<SharedDeps>) -> Json<SettingsView> {
    Json(settings_view(&deps))
}

fn py_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| py_repr(s)).collect();
    format!("[{}]", inner.join(", "))
}

#[utoipa::path(put, path = "/settings", operation_id = "put_settings_settings_put", tag = "settings",
    summary = "Put Settings", request_body = SettingsUpdate,
    responses((status = 200, description = "Successful Response", body = SettingsResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_settings(
    State(deps): State<SharedDeps>,
    Body(update): Body<SettingsUpdate>,
) -> ApiResult<Json<SettingsResult>> {
    let mut applied: Vec<String> = Vec::new();
    let values = update.values.unwrap_or_default();
    let secrets = update.secrets.unwrap_or_default();
    if !values.is_empty() {
        // settings.env is plain text: a password never goes there (ADR 0036). One written
        // there by hand may be taken out (None).
        let is_password = password_check(&deps, Some(&values));
        let bad: Vec<String> = values
            .iter()
            .filter(|(k, v)| v.is_some() && is_password(k))
            .map(|(k, _)| k.clone())
            .collect();
        if !bad.is_empty() {
            return Err(ApiError::unprocessable(format!(
                "{} {}; save it on Settings › Passwords",
                bad.join(", "),
                if bad.len() > 1 {
                    "are passwords"
                } else {
                    "is a password"
                }
            )));
        }
        ghost_core::envfile::update_env(&deps.settings_path, &values, 0o600).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidInput {
                ApiError::unprocessable(e.to_string())
            } else {
                ApiError::internal(e.to_string())
            }
        })?;
        applied = deps.control.apply_settings(&values);
    }
    if !secrets.is_empty() {
        let mut allowed: BTreeSet<String> =
            SECRET_KEYS_SET.iter().map(|s| (*s).to_string()).collect();
        allowed.extend(referenced(&deps));
        let bad: Vec<String> = secrets
            .keys()
            .filter(|k| !allowed.contains(*k))
            .cloned()
            .collect();
        if !bad.is_empty() {
            return Err(ApiError::unprocessable(format!(
                "unknown secrets {}",
                py_list(&bad)
            )));
        }
        save_passwords(&deps, &secrets)?; // the vault (ADR 0036)
        let set: Vec<String> = secrets
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(k, _)| k.clone())
            .collect();
        revive_apps_using(&deps, &set).await;
    }
    let pending_values = values.keys().any(|k| !applied.contains(k));
    let pending_secrets = secrets.keys().any(|k| !applied.contains(k));
    Ok(Json(SettingsResult {
        settings: settings_view(&deps),
        applied,
        restart_required: pending_values || pending_secrets,
    }))
}

// --- devices + setup --------------------------------------------------------------------------

#[utoipa::path(get, path = "/devices", operation_id = "devices_devices_get", tag = "setup", summary = "Devices",
    responses((status = 200, description = "Successful Response", body = DevicesView)))]
async fn devices(State(deps): State<SharedDeps>) -> Json<DevicesView> {
    let devices = deps.devices.devices();
    Json(DevicesView {
        echo_cancel_source_present: devices.iter().any(|d| d.name == "ghost.mic"),
        echo_cancel_active: deps.devices.echo_cancel_active(),
        devices,
        input_device: deps.input_device.clone(),
        output_device: deps.output_device.clone(),
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EchoCancelQuery {
    #[param(default = false)]
    pub uninstall: Option<bool>,
}

#[utoipa::path(post, path = "/setup/echo-cancel", operation_id = "setup_echo_cancel_setup_echo_cancel_post",
    tag = "setup", summary = "Setup Echo Cancel", params(EchoCancelQuery),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn setup_echo_cancel(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<EchoCancelQuery>,
) -> ApiResult<Json<Ack>> {
    let script = deps.infra_dir.join("pipewire").join("install.sh");
    if !script.exists() {
        return Err(ApiError::internal(format!(
            "installer not found at {}",
            script.display()
        )));
    }
    let mut cmd = tokio::process::Command::new(&script);
    if q.uninstall.unwrap_or(false) {
        cmd.arg("--uninstall");
    }
    cmd.env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars())) // never a password (ADR 0036)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let detail: String = text
        .chars()
        .rev()
        .take(2000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Ok(Json(Ack {
        ok: out.status.success(),
        detail: Some(detail),
    }))
}

#[utoipa::path(get, path = "/setup/models", operation_id = "models_setup_models_get", tag = "setup", summary = "Models",
    responses((status = 200, description = "Successful Response", body = ModelsView)))]
async fn models(State(deps): State<SharedDeps>) -> Json<ModelsView> {
    Json(ModelsView {
        ready: deps.state.models_ready(),
        cache_dir: deps.cache_dir.display().to_string(),
        kokoro_present: deps
            .cache_dir
            .join("kokoro")
            .join("kokoro-v1.0.onnx")
            .exists(),
        stt_model: deps.stt_model.clone(),
    })
}

// --- events ------------------------------------------------------------------------------------

/// Documents the payloads sent on the /events WebSocket. Not meant to be called.
#[utoipa::path(get, path = "/events/schema", operation_id = "events_schema_events_schema_get", tag = "events",
    summary = "Events Schema",
    description = "Documents the payloads sent on the /events WebSocket. Not meant to be called.",
    responses((status = 200, description = "Successful Response", body = inline(crate::openapi::Event))))]
async fn events_schema() -> ApiError {
    ApiError::not_found("use the /events WebSocket")
}

async fn events(ws: WebSocketUpgrade, State(deps): State<SharedDeps>) -> Response {
    ws.on_upgrade(move |socket| event_stream(socket, deps))
}

async fn event_stream(mut socket: WebSocket, deps: SharedDeps) {
    let mut rx = deps.state.bus.subscribe();
    // first message: a plain Status
    let Ok(first) = serde_json::to_string(&status_snapshot(&deps)) else {
        return;
    };
    if socket.send(Message::Text(first.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(payload) => {
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue, // the oldest were dropped, as the queue did
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

// --- the catalog entry, whose name has slashes in it ------------------------------------------

/// `/catalog/{name:path}` in the document: the handler lives in `apps`, on a wildcard.
#[derive(OpenApi)]
#[openapi(paths(crate::apps::catalog_entry))]
struct CatalogEntryDoc;

// --- the app -------------------------------------------------------------------------------------

/// The routes and the document that describes them, over `deps`. The `/events`
/// WebSocket is served beside them and documented by `/events/schema`.
pub fn create_app(deps: SharedDeps) -> (axum::Router, utoipa::openapi::OpenApi) {
    // The status provider reads these very deps; a weak reference, so the daemon's state
    // outlives nothing it shouldn't.
    let weak: Weak<Deps> = Arc::downgrade(&deps);
    let state = deps.state.clone();
    deps.state
        .set_status_provider(Arc::new(move || match weak.upgrade() {
            Some(d) => status_snapshot(&d),
            None => state.bare_status(),
        }));

    let router: OpenApiRouter<SharedDeps> = OpenApiRouter::new()
        .merge(crate::apps::router())
        .merge(crate::passwords::router())
        .merge(crate::model::router())
        .merge(crate::skill_catalog::router())
        .merge(crate::conversations::router())
        .routes(routes!(get_status))
        .routes(routes!(wake))
        .routes(routes!(sleep))
        .routes(routes!(say))
        .routes(routes!(turns))
        .routes(routes!(pending))
        .routes(routes!(confirm))
        .routes(routes!(cancel))
        .routes(routes!(confirm_one))
        .routes(routes!(cancel_one))
        .routes(routes!(waiting))
        .routes(routes!(acknowledge))
        .routes(routes!(snooze))
        .routes(routes!(audit))
        .routes(routes!(phone_pairing))
        .routes(routes!(phone, forget_phone))
        .routes(routes!(watching))
        .routes(routes!(get_rules, add_rule))
        .routes(routes!(outputs))
        .routes(routes!(skills, add_skill))
        .routes(routes!(skill_body, remove_skill))
        .routes(routes!(sources))
        .routes(routes!(get_policy, put_policy))
        .routes(routes!(get_settings, put_settings))
        .routes(routes!(devices))
        .routes(routes!(setup_echo_cancel))
        .routes(routes!(models))
        .routes(routes!(events_schema));
    let (router, mut api) = router.split_for_parts();
    api.merge(CatalogEntryDoc::openapi());
    let router = router
        .route("/catalog/{*name}", get(crate::apps::catalog_entry))
        .route("/events", get(events))
        .with_state(deps);
    (router, crate::openapi::finish(api))
}
