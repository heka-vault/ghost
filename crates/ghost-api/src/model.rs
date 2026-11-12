//! Model connection routes (ADR 0037): the providers, the connection now, its model list,
//! Test connection, and signing in to OpenRouter.
//!
//! The connection itself is saved like any setting, through `PUT /settings`: its
//! `GHOST_LLM_*` values in settings.env. Its key is a password in Ghost's vault (ADR 0036),
//! saved on its own like any other (`PUT /passwords/{name}`). These routes read, list and
//! test, and keep the key OpenRouter's sign-in returns. A key goes in and never comes out:
//! views say whether the password is set, by name. The model has no route here.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use ghost_core::apps::AppError;
use ghost_core::connection::{
    ENV_KEYS, ModelConnection, PROVIDERS, Protocol as CoreProtocol, format_headers, new_pkce,
    openrouter_auth_url, provider,
};
use tracing::info;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, check_len};
use crate::models::{
    ModelCheckLatest, ModelCheckModel, ModelCheckState, ModelCheckTiming, ModelCheckVerdict,
    ModelCheckView, ModelConnectionView, ModelDraft, ModelListView, ModelOptionView,
    ModelProviderView, ModelSignInResult, Protocol,
};
use crate::passwords::save_passwords;

/// The sign-in's name: no app can be called this.
pub const SIGN_IN: &str = "model:openrouter";
/// OpenRouter's codes last ten minutes.
pub const SIGN_IN_SECS: u64 = 600;
/// Stands for a Password's value where only set-or-not is asked.
pub const HELD: &str = "(in the vault)";

/// What a connection is read from: its own settings, and the older ones it falls back on.
fn connection_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = ENV_KEYS.to_vec();
    keys.extend([
        "GHOST_LLM_ENDPOINT",
        "ANTHROPIC_BASE_URL",
        "CLIPROXY_BASE_URL",
    ]);
    keys
}

fn protocol_of(p: CoreProtocol) -> Protocol {
    match p {
        CoreProtocol::Anthropic => Protocol::Anthropic,
        CoreProtocol::Openai => Protocol::Openai,
    }
}

/// What a connection is resolved from: the daemon's environment (where Ghost's own
/// passwords are, ADR 0036), the saved settings over it, then the draft. Its Password
/// comes from the vault by name: its value only when `use_key` (a list or a test, noted
/// as a use for "last used"); otherwise only whether it is there. A key typed into a
/// draft is used for that one request and never kept.
pub fn model_env(
    deps: &Deps,
    draft: Option<&ModelDraft>,
    use_key: bool,
) -> ApiResult<HashMap<String, String>> {
    let mut env = deps.environ();
    let saved = deps.settings();
    let keys = connection_keys();
    for k in &keys {
        if let Some(v) = saved.get(*k) {
            env.insert((*k).to_string(), v.clone());
        }
    }
    if let Some(draft) = draft {
        for (k, v) in &draft.values {
            if !keys.contains(&k.as_str()) {
                return Err(ApiError::unprocessable(format!(
                    "{k} is not a model connection setting"
                )));
            }
            match v.as_deref() {
                None | Some("") => {
                    env.remove(k);
                }
                Some(v) => {
                    env.insert(k.clone(), v.to_string());
                }
            }
        }
    }
    let name = ModelConnection::from_env(&env).password;
    if name.is_empty() {
        return Ok(env);
    }
    if let Some(typed) = draft.and_then(|d| d.password_value.as_ref()) {
        env.insert(name, typed.expose().to_string());
        return Ok(env);
    }
    if let Some(vault) = &deps.passwords
        && vault.is_open()
        && vault.contains(&name)
    {
        if use_key {
            let value = vault
                .value(&name)
                .map(|v| v.to_string())
                .unwrap_or_default();
            env.insert(name, value);
        } else if env.get(&name).is_none_or(String::is_empty) {
            env.insert(name, HELD.to_string()); // set-or-not only; never sent anywhere
        }
    }
    Ok(env)
}

/// The GHOST_LLM_* values that say `conn` without the older settings.
pub fn explicit_values(conn: &ModelConnection) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("GHOST_LLM_PROVIDER".into(), conn.provider.clone());
    if !conn.model.is_empty() {
        out.insert("GHOST_LLM_MODEL".into(), conn.model.clone());
    }
    if !conn.fast_model.is_empty() {
        out.insert("GHOST_LLM_FAST_MODEL".into(), conn.fast_model.clone());
    }
    if conn.provider == "custom" {
        out.insert("GHOST_LLM_PROTOCOL".into(), conn.protocol.to_string());
    }
    if !conn.base_url_override.is_empty() {
        out.insert("GHOST_LLM_BASE_URL".into(), conn.base_url_override.clone());
    }
    if !conn.password.is_empty() && conn.password != conn.spec().password {
        out.insert("GHOST_LLM_PASSWORD".into(), conn.password.clone());
    }
    if !conn.headers.is_empty() {
        out.insert(
            "GHOST_LLM_HEADERS".into(),
            format_headers(conn.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .unwrap_or_default(),
        );
    }
    if let Some(t) = conn.timeout_secs
        && t != 0.0
    {
        out.insert("GHOST_LLM_TIMEOUT_SECS".into(), py_g(t));
    }
    out
}

/// Python's `f"{x:g}"`: the shortest form, no trailing zeros.
fn py_g(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

pub fn providers_view() -> Vec<ModelProviderView> {
    PROVIDERS
        .iter()
        .map(|p| ModelProviderView {
            id: p.id.into(),
            title: p.title.into(),
            blurb: p.blurb.into(),
            protocol: protocol_of(p.protocol),
            base_url: p.base_url.into(),
            password: (!p.password.is_empty()).then(|| p.password.to_string()),
            password_optional: p.password_optional,
            sign_in: p.sign_in,
            local: p.local,
            default_model: (!p.default_model.is_empty()).then(|| p.default_model.to_string()),
        })
        .collect()
}

/// Ghost's passwords can't be read now: a key the environment lacks may be set.
fn locked(deps: &Deps) -> bool {
    deps.passwords.as_ref().is_some_and(|v| !v.is_open())
}

fn view(deps: &Deps, env: &HashMap<String, String>) -> ModelConnectionView {
    let conn = ModelConnection::from_env(env);
    let problems = conn.problems(env, locked(deps));
    ModelConnectionView {
        provider: conn.provider.clone(),
        protocol: protocol_of(conn.protocol),
        base_url: conn.base_url.clone(),
        base_url_override: (!conn.base_url_override.is_empty())
            .then(|| conn.base_url_override.clone()),
        model: conn.model.clone(),
        wire_model: conn.wire_model(),
        fast_model: (!conn.fast_model.is_empty()).then(|| conn.fast_model.clone()),
        fast_wire_model: conn.fast_wire_model(),
        password: (!conn.password.is_empty()).then(|| conn.password.clone()),
        password_set: conn.password_set(env),
        headers: conn.headers.iter().cloned().collect(),
        timeout_secs: conn.timeout_secs,
        legacy: (!conn.legacy.is_empty()).then(|| conn.legacy.clone()),
        ready: problems.is_empty(),
        problems,
        values: explicit_values(&conn),
        providers: providers_view(),
        sign_in_pending: deps.model_sign_in.pending(),
        sign_in_error: deps.model_sign_in.error(),
    }
}

/// The model connection now, with every provider the page offers.
#[utoipa::path(get, path = "/model", operation_id = "get_model_model_get", tag = "model", summary = "Get Model",
    description = "The model connection now, with every provider the page offers.",
    responses((status = 200, description = "Successful Response", body = ModelConnectionView)))]
async fn get_model(State(deps): State<SharedDeps>) -> ApiResult<Json<ModelConnectionView>> {
    let env = model_env(&deps, None, false)?;
    Ok(Json(view(&deps, &env)))
}

fn check_draft(draft: &ModelDraft) -> ApiResult<()> {
    if let Some(v) = &draft.password_value {
        check_len(&["body", "password_value"], v.expose(), 0, 4096)?;
    }
    Ok(())
}

/// The models a provider offers, for the saved connection or the one being edited. A
/// provider that can't be read says why; that is not an HTTP error.
#[utoipa::path(post, path = "/model/models", operation_id = "model_list_model_models_post", tag = "model",
    summary = "Model List",
    description = "The models a provider offers, for the saved connection or the one being edited.\nA provider that can't be read says why; that is not an HTTP error.",
    request_body = ModelDraft,
    responses((status = 200, description = "Successful Response", body = ModelListView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn model_list(
    State(deps): State<SharedDeps>,
    Body(draft): Body<ModelDraft>,
) -> ApiResult<Json<ModelListView>> {
    check_draft(&draft)?;
    let env = model_env(&deps, Some(&draft), true)?;
    let conn = ModelConnection::from_env(&env);
    let spec = conn.spec();
    let default = (!spec.default_model.is_empty()).then(|| spec.default_model.to_string());
    if conn.base_url.is_empty() {
        return Ok(Json(ModelListView {
            models: Vec::new(),
            default,
            error: Some("Give it an address first.".into()),
        }));
    }
    // OpenRouter lists its models to anyone; the others want the key, so don't ask
    // without one.
    if !conn.password.is_empty()
        && !spec.password_optional
        && !spec.sign_in
        && !conn.password_set(&env)
    {
        let error = if locked(&deps) {
            "Unlock your keyring on Settings › Passwords to see the models it offers."
        } else {
            "Save its password to see the models it offers."
        };
        return Ok(Json(ModelListView {
            models: Vec::new(),
            default,
            error: Some(error.into()),
        }));
    }
    let found = match deps.model_net.list_models(&conn, &env).await {
        Ok(found) => found,
        Err(e) => {
            return Ok(Json(ModelListView {
                models: Vec::new(),
                default,
                error: Some(e.to_string()),
            }));
        }
    };
    let chosen = match &default {
        Some(d) if found.iter().any(|m| m.id == *d) => Some(d.clone()),
        _ => found.first().map(|m| m.id.clone()).or(default),
    };
    Ok(Json(ModelListView {
        models: found
            .into_iter()
            .map(|m| ModelOptionView {
                id: m.id,
                name: (!m.name.is_empty()).then_some(m.name),
            })
            .collect(),
        default: chosen,
        error: None,
    }))
}

fn ms(s: Option<f64>) -> Option<i64> {
    s.map(|s| (s * 1000.0).round() as i64)
}

fn timing(t: &ghost_llm::check::Timing) -> ModelCheckTiming {
    ModelCheckTiming {
        median_ms: ms(t.median),
        worst_ms: ms(t.worst),
    }
}

fn verdict(v: &ghost_llm::check::Verdict) -> ModelCheckVerdict {
    use ghost_llm::check::State;
    ModelCheckVerdict {
        state: match v.state {
            State::Yes => ModelCheckState::Yes,
            State::No => ModelCheckState::No,
            State::NotReported => ModelCheckState::NotReported,
            State::NotApplicable => ModelCheckState::NotApplicable,
            State::Untested => ModelCheckState::Untested,
        },
        detail: v.detail.clone(),
    }
}

/// The API's shape of a check.
pub fn check_view(run: &crate::controller::ModelCheckRun, where_: String) -> ModelCheckView {
    ModelCheckView {
        checked_at: run.at,
        where_,
        models: run
            .models
            .iter()
            .map(|m| ModelCheckModel {
                role: m.role.clone(),
                model: m.model.clone(),
                answered_by: m.answered_by.clone(),
                reachable: m.reachable,
                error: m.error.clone(),
                first_byte: timing(&m.first_byte),
                first_word: timing(&m.first_word),
                samples: m.samples as u32,
                streaming: verdict(&m.streaming),
                tools: verdict(&m.tools),
                caching: verdict(&m.caching),
                thinking: verdict(&m.thinking),
                thinking_mode: m.thinking_mode.clone(),
            })
            .collect(),
        advice: run.advice.clone(),
    }
}

/// Provider, protocol and host of a connection; never a key.
fn where_of(conn: &ModelConnection) -> String {
    let described = conn.describe();
    described
        .split_once(" via ")
        .map_or(described.as_str(), |(_, w)| w)
        .to_string()
}

/// Check the connection being edited, or the saved one (ADR 0056): for the main model
/// and the fast one, whether it answers (or the error in plain words), the time to the
/// first byte and the first word over a few short calls, and whether streaming, tool
/// calls, prompt caching and the thinking setting work. A fixed synthetic prompt, never
/// anything of the operator's; the password is read from the vault by the daemon and
/// never shown. The result is kept, and Ghost shapes its requests to it.
#[utoipa::path(post, path = "/model/check", operation_id = "model_check_model_check_post", tag = "model",
    summary = "Model Check",
    description = "Check the connection being edited, or the saved one (ADR 0056): for the main\nmodel and the fast one, whether it answers (or the error in plain words), the time\nto the first byte and the first word over a few short calls, and whether streaming,\ntool calls, prompt caching and the thinking setting work. A fixed synthetic prompt,\nnever anything of the operator's; the password is read from the vault by the daemon\nand never shown. The result is kept, and Ghost shapes its requests to it.",
    request_body = ModelDraft,
    responses((status = 200, description = "Successful Response", body = ModelCheckView),
              (status = 422, description = "Validation Error", body = HTTPValidationError),
              (status = 503, description = "Ghost can't check a connection now")))]
async fn model_check(
    State(deps): State<SharedDeps>,
    Body(draft): Body<ModelDraft>,
) -> ApiResult<Json<ModelCheckView>> {
    check_draft(&draft)?;
    let env = model_env(&deps, Some(&draft), true)?;
    let conn = ModelConnection::from_env(&env);
    let where_ = where_of(&conn);
    if let Some(problem) = conn.problems_in_settings.first() {
        return Ok(Json(ModelCheckView {
            checked_at: chrono::Utc::now(),
            where_,
            models: Vec::new(),
            advice: Some(problem.clone()),
        }));
    }
    let run =
        deps.control.check_model(env).await.ok_or_else(|| {
            ApiError::unavailable("Ghost can't check a model connection right now")
        })?;
    Ok(Json(check_view(&run, where_)))
}

/// The latest kept check of the saved connection, if there is one.
#[utoipa::path(get, path = "/model/check", operation_id = "model_check_latest_model_check_get", tag = "model",
    summary = "Model Check Latest",
    description = "The latest kept check of the saved connection, if there is one.",
    responses((status = 200, description = "Successful Response", body = ModelCheckLatest)))]
async fn model_check_latest(State(deps): State<SharedDeps>) -> ApiResult<Json<ModelCheckLatest>> {
    let env = model_env(&deps, None, false)?;
    let where_ = where_of(&ModelConnection::from_env(&env));
    Ok(Json(ModelCheckLatest {
        check: deps
            .control
            .last_model_check()
            .map(|run| check_view(&run, where_)),
    }))
}

/// Sign in to OpenRouter (OAuth with PKCE). The desktop opens the address; the browser
/// comes back to Ghost's loopback, and the key OpenRouter gives is kept as the
/// OPENROUTER_API_KEY password in Ghost's vault (ADR 0036). Nothing to copy.
#[utoipa::path(post, path = "/model/sign-in", operation_id = "model_sign_in_model_sign_in_post", tag = "model",
    summary = "Model Sign In",
    description = "Sign in to OpenRouter (OAuth with PKCE). The desktop opens the address; the\nbrowser comes back to Ghost's loopback, and the key OpenRouter gives is kept as\nthe OPENROUTER_API_KEY password in Ghost's vault (ADR 0036). Nothing to copy.",
    responses((status = 200, description = "Successful Response", body = ModelSignInResult)))]
async fn model_sign_in(State(deps): State<SharedDeps>) -> ApiResult<Json<ModelSignInResult>> {
    let signins = deps
        .signins
        .clone()
        .ok_or_else(|| ApiError::unavailable("Ghost can't take a sign-in right now"))?;
    let vault = deps
        .passwords
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    if !vault.is_open() {
        // Asked first, so nobody signs in for a key Ghost then can't keep.
        return Err(ApiError::locked(
            "Ghost's passwords are locked. Unlock them, then sign in.",
        ));
    }
    {
        let mut task = deps
            .model_sign_in
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(t) = task.take()
            && !t.is_finished()
        {
            t.abort();
        }
    }
    let pkce = new_pkce();
    let url = openrouter_auth_url(&signins.localhost_redirect_uri(), &pkce);
    deps.model_sign_in.set_error(None);
    let password = provider("openrouter").map_or("OPENROUTER_API_KEY", |p| p.password);
    let finish = {
        let deps = deps.clone();
        let url = url.clone();
        async move {
            let answer = signins
                .browser_return(SIGN_IN, &url, Duration::from_secs(SIGN_IN_SECS))
                .await;
            let q = match answer {
                Ok(q) => q,
                Err(AppError::NoAnswer(_)) => {
                    deps.model_sign_in
                        .set_error(Some("Nobody finished signing in. Try again.".into()));
                    return;
                }
                Err(e) => {
                    deps.model_sign_in
                        .set_error(Some(format!("The sign-in didn’t go through: {e}")));
                    info!("model sign-in: {}", e.reason());
                    return;
                }
            };
            if let Some(state) = q.get("state")
                && *state != pkce.state
            {
                deps.model_sign_in.set_error(Some(
                    "The sign-in didn’t go through: the sign-in came back for another request"
                        .into(),
                ));
                return;
            }
            let Some(code) = q.get("code").filter(|c| !c.is_empty()) else {
                deps.model_sign_in.set_error(Some(
                    "The sign-in didn’t go through: OpenRouter sent no code back".into(),
                ));
                return;
            };
            let key = match deps.model_net.openrouter_exchange(code, &pkce).await {
                Ok(key) => key,
                Err(e) => {
                    deps.model_sign_in
                        .set_error(Some(format!("The sign-in didn’t go through: {e}")));
                    info!("model sign-in: exchange failed");
                    return;
                }
            };
            // The vault, which puts it in the daemon's environment: the next session uses
            // it, no restart (ADR 0036).
            let mut values = BTreeMap::new();
            values.insert(password.to_string(), Some(key.to_string()));
            if let Err(e) = save_passwords(&deps, &values) {
                deps.model_sign_in
                    .set_error(Some(format!("Ghost couldn’t keep the key: {}", e.text())));
                return;
            }
            info!("model sign-in: OpenRouter's key kept as the {password} password");
        }
    };
    let handle = tokio::spawn(finish);
    *deps
        .model_sign_in
        .task
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(handle);
    tokio::task::yield_now().await; // starts listening; the browser comes back far later
    Ok(Json(ModelSignInResult { url }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_model))
        .routes(routes!(model_list))
        .routes(routes!(model_check, model_check_latest))
        .routes(routes!(model_sign_in))
}
