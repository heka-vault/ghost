//! A yes or a no that sticks (ADR 0070), on the control API only: the operator's desktop.
//!
//! `POST /proposals/{id}/always` is the card's "Yes, and don't ask again" or "No, and never":
//! the rule is taken from the waiting change the gate holds, never from the request, so a
//! click can only ever name that change's own tool. The rule is written through
//! `ghost_core::permission_choices`, the gate reads the file again (`reload_policy`), and the
//! change is then confirmed or dropped exactly as a plain Confirm or Cancel would.
//! `GET /permissions/choices` lists what was set this way and `DELETE` takes one back out.
//!
//! The phone listener, the listener for outside tools and Ghost's own app have no route
//! here: a standing rule is the desktop's alone for now.

use std::path::Path;

use axum::Json;
use axum::extract::{Path as PathArg, State};
use ghost_core::permission_choices::{self as choices, Choice, ChoiceError, ChoiceRow, Surface};
use ghost_core::permissions::Rule;
use ghost_core::policy::AutonomyPolicy;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::outcome_view;
use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};
use crate::models::{Ack, AlwaysChoice, AlwaysRequest, AlwaysResult, PermissionChoiceView};

fn core(c: AlwaysChoice) -> Choice {
    match c {
        AlwaysChoice::Allow => Choice::Allow,
        AlwaysChoice::Block => Choice::Block,
    }
}

fn view(row: &ChoiceRow, policy: Option<&AutonomyPolicy>) -> PermissionChoiceView {
    let in_file = match (policy, Rule::parse(&row.rule)) {
        (Some(p), Ok(rule)) => p.permissions.list(row.choice.list()).contains(&rule),
        _ => false,
    };
    PermissionChoiceView {
        id: row.id,
        rule: row.rule.clone(),
        choice: match row.choice {
            Choice::Allow => AlwaysChoice::Allow,
            Choice::Block => AlwaysChoice::Block,
        },
        sentence: row.sentence(),
        set_at: row.set_at,
        proposal_id: row.proposal_id.clone(),
        in_file,
    }
}

fn refusal(e: ChoiceError) -> ApiError {
    match e {
        ChoiceError::Refused(m) => ApiError::conflict(m),
        ChoiceError::Io(m) => ApiError::internal(m),
    }
}

fn load(path: &Path) -> Option<AutonomyPolicy> {
    AutonomyPolicy::load(path).ok()
}

/// The card's sticky choice on a waiting change (ADR 0070): the rule for that change's own
/// tool goes into `[permissions]` of policy.toml, then the change runs (allow) or is
/// dropped (block). Refused when a more careful rule of the operator's would overrule it.
#[utoipa::path(post, path = "/proposals/{proposal_id}/always",
    operation_id = "always_proposals__proposal_id__always_post", tag = "writes",
    summary = "Decide And Remember",
    description = "Yes, and don't ask again (allow), or No, and never (block), on a change waiting for the operator (ADR 0070). The rule names exactly that change's tool, taken from the waiting change, never from the request; it is written to `[permissions]` in policy.toml, the gate reads it at once, and the change is then confirmed or dropped as a click would. A click in the operator's desktop session only (`classifier: ui`).",
    params(("proposal_id" = String, Path)),
    request_body = AlwaysRequest,
    responses((status = 200, description = "Successful Response", body = AlwaysResult),
              (status = 409, description = "Not waiting, or the choice would not take effect"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn always(
    State(deps): State<SharedDeps>,
    PathArg(proposal_id): PathArg<String>,
    Body(req): Body<AlwaysRequest>,
) -> ApiResult<Json<AlwaysResult>> {
    let proposal = deps
        .gate
        .waiting_one(&proposal_id, None)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::conflict("that change isn't waiting"))?;
    let choice = core(req.choice);
    let row = {
        let store = deps.store.clone();
        let path = deps.policy_path.clone();
        let p = proposal.clone();
        tokio::task::spawn_blocking(move || {
            choices::set_from_decision(
                &store,
                &path,
                &p,
                &ghost_core::gate::WriteGate::args_of(&p),
                choice,
                Surface::Desktop,
            )
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(refusal)?
    };
    // The gate reads the file again before the change is decided, so the very next call
    // (and this one's own check in `execute`) follows the new rule. No restart.
    deps.control
        .reload_policy()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let outcome = match choice {
        Choice::Allow => Some(
            deps.control
                .confirm_proposal(&proposal_id)
                .await
                .ok_or_else(|| ApiError::conflict("that change isn't waiting any more"))?,
        ),
        Choice::Block => {
            deps.control.cancel_proposal(&proposal_id).await;
            None
        }
    };
    let policy = load(&deps.policy_path);
    Ok(Json(AlwaysResult {
        choice: view(&row, policy.as_ref()),
        outcome: outcome.as_ref().map(outcome_view),
    }))
}

/// The rules the operator set from a decision, newest first (ADR 0070).
#[utoipa::path(get, path = "/permissions/choices",
    operation_id = "choices_permissions_choices_get", tag = "policy",
    summary = "Permission Choices",
    description = "The rules the operator set from a decision on the desktop's card (ADR 0070), newest first, each in a sentence with when it was set and from which change.",
    responses((status = 200, description = "Successful Response", body = Vec<PermissionChoiceView>)))]
async fn list(State(deps): State<SharedDeps>) -> ApiResult<Json<Vec<PermissionChoiceView>>> {
    let rows = deps
        .store
        .permission_choices()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let policy = load(&deps.policy_path);
    Ok(Json(
        rows.iter().map(|r| view(r, policy.as_ref())).collect(),
    ))
}

/// Take a choice back out: its rule leaves policy.toml through the same writer.
#[utoipa::path(delete, path = "/permissions/choices/{choice_id}",
    operation_id = "remove_choice_permissions_choices__choice_id__delete", tag = "policy",
    summary = "Remove Permission Choice",
    description = "Take a rule set from a decision back out (ADR 0070): it leaves policy.toml through the same writer, which puts the file back as it was when that rule was the last thing added, and the gate reads it at once.",
    params(("choice_id" = i64, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 404, description = "No such choice"),
              (status = 409, description = "The file doesn't read"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn remove(
    State(deps): State<SharedDeps>,
    PathArg(choice_id): PathArg<i64>,
) -> ApiResult<Json<Ack>> {
    let store = deps.store.clone();
    let path = deps.policy_path.clone();
    let removed =
        tokio::task::spawn_blocking(move || choices::remove_choice(&store, &path, choice_id))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .map_err(refusal)?;
    if !removed {
        return Err(ApiError::not_found("no such choice"));
    }
    deps.control
        .reload_policy()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(Ack {
        ok: true,
        detail: None,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(always))
        .routes(routes!(list))
        .routes(routes!(remove))
}
