//! The operator's own AI subscriptions (ADR 0078): Claude Code, OpenAI Codex and Google
//! Antigravity, each the vendor's own program. `GET /subscriptions` asks each program (never
//! its files) whether it is here and signed in; the operator's click installs one with the
//! vendor's own command, starts its own sign-in, pastes back the code its page shows, cancels
//! either, or makes it Ghost's model.
//!
//! Every route is the operator's, on the control API only: no tool of Ghost's own app, no
//! outside client, nothing on the phone, no run and no model reaches them. A pasted code goes
//! in and never comes back out: not in a response, a log line, the audit or the store.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path as PathArg, State};
use chrono::{DateTime, Utc};
use ghost_core::subscriptions::{self as subs, JobKind, JobSnapshot, JobState, Refused, Sub};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zeroize::Zeroizing;

use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};

/// The settings a connection is made of (Settings › Model's keys): choosing a program clears
/// the rest, as the desktop's "Use Claude Code" always has.
pub const CONNECTION_KEYS: [&str; 7] = [
    "GHOST_LLM_PROVIDER",
    "GHOST_LLM_MODEL",
    "GHOST_LLM_PROTOCOL",
    "GHOST_LLM_BASE_URL",
    "GHOST_LLM_PASSWORD",
    "GHOST_LLM_HEADERS",
    "GHOST_LLM_TIMEOUT_SECS",
];

/// What a job is doing, as shown. Never the pasted code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubscriptionJobView {
    /// install or sign_in.
    pub kind: String,
    /// running, needs_code (the program waits for the code its page showed), done, failed or
    /// cancelled.
    pub state: String,
    /// The command as shown: the vendor's install command, or the sign-in's.
    pub command: String,
    /// What the program printed: escapes out, each line bounded and redacted, an address
    /// shown only to its path. Someone else's text, shown as data.
    pub lines: Vec<String>,
    /// The sign-in address, on the vendor's own domains only, for a link in case the browser
    /// didn't open.
    #[schema(required)]
    pub url: Option<String>,
    /// A code the program shows for you to type on its page.
    #[schema(required)]
    pub device_code: Option<String>,
    /// How it ended, or what it waits for, in a sentence.
    #[schema(required)]
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    #[schema(required)]
    pub ended_at: Option<DateTime<Utc>>,
    /// Asked of the program when a sign-in ended.
    #[schema(required)]
    pub signed_in: Option<bool>,
}

fn job_view(j: JobSnapshot) -> SubscriptionJobView {
    SubscriptionJobView {
        kind: match j.kind {
            JobKind::Install => "install",
            JobKind::SignIn => "sign_in",
        }
        .into(),
        state: match j.state {
            JobState::Running => "running",
            JobState::NeedsCode => "needs_code",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
        .into(),
        command: j.command,
        lines: j.lines,
        url: j.url,
        device_code: j.device_code,
        message: j.message,
        started_at: j.started_at,
        ended_at: j.ended_at,
        signed_in: j.signed_in,
    }
}

/// One subscription, as its program says it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubscriptionView {
    /// claude-code, codex or antigravity.
    pub id: String,
    pub title: String,
    /// Its program's name (`claude`, `codex`, `agy`).
    pub program: String,
    /// The program is on this computer.
    pub installed: bool,
    #[schema(required)]
    pub version: Option<String>,
    /// The program says it is signed in.
    pub signed_in: bool,
    /// Ghost can run it as its model now (signed in, and for Claude Code a version with every
    /// switch Ghost needs).
    pub usable: bool,
    /// It is Ghost's model now.
    pub in_use: bool,
    /// Ghost can run it as its model at all; false: "Coming next".
    pub ghost_can_use: bool,
    /// The vendor's own install command, exactly as Ghost would run it.
    pub install_command: String,
    /// Where the vendor documents it.
    pub install_source: String,
    /// The program's own sign-in command, as shown.
    pub sign_in_command: String,
    /// Ghost runs the sign-in (true), or the operator runs it in a terminal: the program's
    /// sign-in has choices that are theirs (Antigravity).
    pub sign_in_here: bool,
    /// What to say about it in a sentence: how it is signed in, or what stops Ghost.
    #[schema(required)]
    pub line: Option<String>,
    /// A version Ghost was not measured with, said once.
    #[schema(required)]
    pub note: Option<String>,
    /// Its install or sign-in, running or the last one.
    #[schema(required)]
    pub job: Option<SubscriptionJobView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubscriptionsView {
    pub subscriptions: Vec<SubscriptionView>,
    /// The one Ghost uses as its model now, by id; None when its model is none of them.
    #[schema(required)]
    pub in_use: Option<String>,
}

/// The code the program's page showed, pasted. Written to the program and never kept.
#[derive(Clone, Deserialize, ToSchema)]
pub struct SubscriptionCode {
    pub code: String,
}

impl std::fmt::Debug for SubscriptionCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionCode(…)")
    }
}

/// The result of making one Ghost's model.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubscriptionUseResult {
    /// What was done, in a sentence.
    pub done: String,
    /// Ghost picks it up at its next start, not now.
    pub restart_required: bool,
    pub subscriptions: SubscriptionsView,
}

fn jobs(deps: &Deps) -> ApiResult<&subs::Jobs> {
    deps.subscriptions
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("subscriptions are not available in this daemon"))
}

fn sub_of(id: &str) -> ApiResult<Sub> {
    Sub::parse(id).ok_or_else(|| ApiError::not_found(format!("no subscription called {id}")))
}

fn refused(sub: Sub, r: Refused) -> ApiError {
    let title = sub.spec().title;
    match r {
        Refused::NoJob | Refused::NotInstalled => ApiError::not_found(r.sentence(title)),
        Refused::Code(_) => ApiError::unprocessable(r.sentence(title)),
        Refused::Start(_) => ApiError::unavailable(r.sentence(title)),
        _ => ApiError::conflict(r.sentence(title)),
    }
}

/// The provider Ghost's model connection names now.
fn provider_now(deps: &Deps) -> String {
    ghost_core::connection::ModelConnection::from_env(&deps.ghost_env()).provider
}

async fn view(deps: &Deps) -> ApiResult<SubscriptionsView> {
    let j = jobs(deps)?;
    let provider = provider_now(deps);
    let mut out = Vec::new();
    let mut in_use = None;
    for spec in subs::TABLE {
        let seen = subs::look(spec.sub, j.started()).await;
        let using = spec.provider.is_some_and(|p| p == provider);
        if using {
            in_use = Some(spec.id.to_string());
        }
        let (usable, line, note) = match &seen.claude {
            Some(found) => (
                found.usable(),
                found.problem().or_else(|| found.sign_in_line()),
                found.version_note(),
            ),
            None => (
                seen.signed_in && spec.provider.is_some(),
                (seen.binary.is_some() && !seen.signed_in)
                    .then(|| format!("{} is installed but not signed in.", spec.title)),
                None,
            ),
        };
        out.push(SubscriptionView {
            id: spec.id.into(),
            title: spec.title.into(),
            program: spec.binary.into(),
            installed: seen.binary.is_some(),
            version: seen.version.clone(),
            signed_in: seen.signed_in,
            usable,
            in_use: using,
            ghost_can_use: spec.provider.is_some(),
            install_command: spec.install.into(),
            install_source: spec.install_source.into(),
            sign_in_command: spec.sign_in_shown.into(),
            sign_in_here: matches!(spec.sign_in, subs::SignInHow::Pty(_)),
            line,
            note,
            job: j.job(spec.sub).map(job_view),
        });
    }
    Ok(SubscriptionsView {
        subscriptions: out,
        in_use,
    })
}

/// Each subscription as its program says it is: installed, signed in, Ghost's model now, and
/// its install or sign-in. Asks the programs; reads none of their files.
#[utoipa::path(get, path = "/subscriptions", operation_id = "subscriptions_get", tag = "subscriptions",
    summary = "Subscriptions",
    description = "Claude Code, OpenAI Codex and Google Antigravity, each as its own program says it is:\ninstalled, its version, signed in, and whether it is Ghost's model. Asks the programs, in an\nenvironment with no keys; reads none of their files.",
    responses((status = 200, description = "Successful Response", body = SubscriptionsView)))]
async fn subscriptions_get(State(deps): State<SharedDeps>) -> ApiResult<Json<SubscriptionsView>> {
    Ok(Json(view(&deps).await?))
}

/// One subscription's install or sign-in, running or the last one: for a screen that follows
/// it without asking every program again.
#[utoipa::path(get, path = "/subscriptions/{id}/job", operation_id = "subscription_job_get", tag = "subscriptions",
    summary = "Subscription job",
    params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Option<SubscriptionJobView>),
              (status = 404, description = "No such subscription")))]
async fn subscription_job(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<Option<SubscriptionJobView>>> {
    let sub = sub_of(&id)?;
    Ok(Json(jobs(&deps)?.job(sub).map(job_view)))
}

/// Install the program with the vendor's own command, on the operator's click.
#[utoipa::path(post, path = "/subscriptions/{id}/install", operation_id = "subscription_install", tag = "subscriptions",
    summary = "Install a subscription's program",
    description = "Run the vendor's own install command (`install_command`), exactly as given, as you and\nnever as root. Its output is the job's lines.",
    params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SubscriptionJobView),
              (status = 409, description = "Already installed, or busy")))]
async fn subscription_install(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<SubscriptionJobView>> {
    let sub = sub_of(&id)?;
    let j = jobs(&deps)?
        .start_install(sub)
        .map_err(|r| refused(sub, r))?;
    Ok(Json(job_view(j)))
}

/// Start the program's own sign-in, on the operator's click.
#[utoipa::path(post, path = "/subscriptions/{id}/sign-in", operation_id = "subscription_sign_in", tag = "subscriptions",
    summary = "Sign in to a subscription",
    description = "Run the program's own sign-in. It opens your browser itself; the job carries the address in\ncase it didn't, and says when the program waits for the code its page shows.",
    params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SubscriptionJobView),
              (status = 409, description = "Busy, or signed in in a terminal")))]
async fn subscription_sign_in(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<SubscriptionJobView>> {
    let sub = sub_of(&id)?;
    let j = jobs(&deps)?
        .start_sign_in(sub)
        .map_err(|r| refused(sub, r))?;
    Ok(Json(job_view(j)))
}

/// Give the waiting sign-in the code its page showed. The code is typed with Enter, held in
/// memory until the program ends, and never returned, logged, stored or audited.
#[utoipa::path(post, path = "/subscriptions/{id}/sign-in/code", operation_id = "subscription_sign_in_code", tag = "subscriptions",
    summary = "Paste a sign-in code",
    params(("id" = String, Path)),
    request_body = SubscriptionCode,
    responses((status = 200, description = "Successful Response", body = SubscriptionJobView),
              (status = 409, description = "Not waiting for a code"),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn subscription_sign_in_code(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
    Body(body): Body<SubscriptionCode>,
) -> ApiResult<Json<SubscriptionJobView>> {
    let sub = sub_of(&id)?;
    let code = Zeroizing::new(body.code);
    let j = jobs(&deps)?
        .send_code(sub, code)
        .await
        .map_err(|r| refused(sub, r))?;
    Ok(Json(job_view(j)))
}

/// Stop the install or the sign-in: the program and everything it started.
#[utoipa::path(post, path = "/subscriptions/{id}/cancel", operation_id = "subscription_cancel", tag = "subscriptions",
    summary = "Cancel a subscription's job",
    params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SubscriptionJobView),
              (status = 404, description = "Nothing running")))]
async fn subscription_cancel(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<SubscriptionJobView>> {
    let sub = sub_of(&id)?;
    let j = jobs(&deps)?.cancel(sub).map_err(|r| refused(sub, r))?;
    Ok(Json(job_view(j)))
}

/// Make it Ghost's model: the provider Settings › Model chooses, the rest of the connection
/// cleared. Only Claude Code for now.
#[utoipa::path(put, path = "/subscriptions/{id}/use", operation_id = "subscription_use", tag = "subscriptions",
    summary = "Use a subscription as Ghost's model",
    params(("id" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SubscriptionUseResult),
              (status = 409, description = "Ghost can't use it as its model yet, or it isn't ready")))]
async fn subscription_use(
    State(deps): State<SharedDeps>,
    PathArg(id): PathArg<String>,
) -> ApiResult<Json<SubscriptionUseResult>> {
    let sub = sub_of(&id)?;
    let spec = sub.spec();
    let Some(provider) = spec.provider else {
        return Err(ApiError::conflict(format!(
            "Ghost can't use {} as its model yet.",
            spec.title
        )));
    };
    let seen = subs::look(sub, jobs(&deps)?.started()).await;
    match &seen.claude {
        None if seen.binary.is_none() => {
            return Err(refused(sub, Refused::NotInstalled));
        }
        Some(found) if !found.usable() => {
            return Err(ApiError::conflict(
                found
                    .problem()
                    .unwrap_or_else(|| format!("{} isn't ready.", spec.title)),
            ));
        }
        _ => {}
    }
    let values: BTreeMap<String, Option<String>> = CONNECTION_KEYS
        .iter()
        .map(|k| {
            let v = (*k == "GHOST_LLM_PROVIDER").then(|| provider.to_string());
            ((*k).to_string(), v)
        })
        .collect();
    ghost_core::envfile::update_env(&deps.settings_path, &values, 0o600)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let applied = deps.control.apply_settings(&values);
    let restart_required = values.keys().any(|k| !applied.contains(k));
    tracing::info!("subscriptions: {} is Ghost's model now", spec.title);
    Ok(Json(SubscriptionUseResult {
        done: format!("Ghost uses {} now.", spec.title),
        restart_required,
        subscriptions: view(&deps).await?,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(subscriptions_get))
        .routes(routes!(subscription_job))
        .routes(routes!(subscription_install))
        .routes(routes!(subscription_sign_in))
        .routes(routes!(subscription_sign_in_code))
        .routes(routes!(subscription_cancel))
        .routes(routes!(subscription_use))
}
