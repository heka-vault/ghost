//! Password routes (ADR 0036): Ghost's vault, as the Passwords page sees it.
//!
//! A password goes in and never comes back out. `GET /passwords` lists names and what may
//! be said about them (set or not, what uses it, when it was last used); `PUT` saves one,
//! `DELETE` forgets one, `POST /passwords/import` moves the plaintext copies into the
//! vault once, and `POST /passwords/unlock` asks the keyring to unlock. No route returns a
//! value, and none is logged. The model has no route here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use axum::Json;
use axum::extract::{Path as PathArg, State};
use ghost_core::connection::{ModelConnection, PASSWORD_NAMES};
use ghost_core::envfile::EnvFile;
use ghost_core::password_import::{
    ImportOptions, SourceKind, fetch_heka, import_passwords, sources,
};
use ghost_core::passwords::{
    GHOST_USES, PasswordsError, check_name, check_value, is_password_name,
};
use indexmap::IndexMap;
use tracing::info;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};
use crate::models::{
    ImportSourceKind, ImportSourceView, PasswordImportResult, PasswordInfo, PasswordResult,
    PasswordSet, PasswordsView, VaultState, VaultView,
};

/// GHOST_USES's word for a key only the model reads.
pub const MODEL_USE: &str = "The model connection";
/// Keys only one model provider uses: not listed until set, unless that provider is chosen.
pub const PROVIDER_ONLY: [&str; 3] = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LLM_API_KEY"];
/// Ghost's own passwords the Settings view lists set-or-not for (the rest is `/passwords`).
pub const SECRET_KEYS_SET: [&str; 6] = crate::models::SECRET_KEYS;

pub fn tilde(p: &Path) -> String {
    let s = p.display().to_string();
    let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()) else {
        return s;
    };
    if s == home {
        "~".into()
    } else if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        s
    }
}

/// Names the operator listed in settings.env as passwords their own config uses
/// (`GHOST_APP_SECRETS`; `GHOST_MCP_SECRETS` is its older name).
pub fn listed_names(settings: &BTreeMap<String, String>) -> BTreeSet<String> {
    let raw = format!(
        "{},{}",
        settings.get("GHOST_APP_SECRETS").map_or("", String::as_str),
        settings.get("GHOST_MCP_SECRETS").map_or("", String::as_str)
    );
    raw.split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().all(|c| !c.is_lowercase()))
        .filter(|n| n.chars().any(|c| c.is_ascii_uppercase()))
        .map(String::from)
        .collect()
}

fn ghost_use(name: &str) -> Option<&'static str> {
    GHOST_USES.iter().find(|(n, _)| *n == name).map(|(_, u)| *u)
}

/// What uses each password, in the operator's words: Ghost itself, an app, a webhook.
pub fn used_by(deps: &Deps) -> IndexMap<String, Vec<String>> {
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    for (name, what) in GHOST_USES {
        if name != "GITHUB_TOKEN" {
            // an app's, when one names it
            out.entry(name.to_string())
                .or_default()
                .push(what.to_string());
        }
    }
    if let Some(apps) = &deps.apps {
        for spec in apps.specs() {
            for name in spec.secrets() {
                out.entry(name).or_default().push(spec.shown());
            }
        }
    }
    if let Some(folder) = &deps.folder {
        let folder = folder.lock().unwrap_or_else(|e| e.into_inner());
        for hook in folder.hooks.values() {
            out.entry(hook.secret_name())
                .or_default()
                .push(format!("Webhook {}", hook.name));
        }
    }
    let settings = deps.settings();
    for name in listed_names(&settings) {
        out.entry(name).or_default();
    }
    // A model connection may name a password of its own (ADR 0037).
    let named = model_password(deps, &settings);
    if !named.is_empty() && ghost_use(&named).is_none() {
        out.entry(named).or_default().push(MODEL_USE.to_string());
    }
    out
}

/// The daemon's environment with the saved settings over it.
fn env_with(deps: &Deps, settings: &BTreeMap<String, String>) -> HashMap<String, String> {
    let mut env = deps.environ();
    for (k, v) in settings {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// The password the model connection reads, as settings.env names it.
pub fn model_password(deps: &Deps, settings: &BTreeMap<String, String>) -> String {
    ModelConnection::from_env(&env_with(deps, settings)).password
}

/// Passwords nothing but the model connection reads: the next session reads them by name
/// from the daemon's environment, which the vault updates, so no restart.
pub fn model_only(deps: &Deps, settings: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = PASSWORD_NAMES
        .iter()
        .filter(|n| ghost_use(n).unwrap_or(MODEL_USE) == MODEL_USE)
        .map(|n| (*n).to_string())
        .collect();
    let named = model_password(deps, settings);
    if !named.is_empty() && ghost_use(&named).is_none() {
        names.insert(named);
    }
    names
}

/// Every name something in Ghost refers to (its own, the apps', the webhooks').
pub fn referenced(deps: &Deps) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = used_by(deps).into_keys().collect();
    out.insert("GITHUB_TOKEN".into());
    out
}

/// Whether a settings.env key is a password, so `/settings` neither shows nor writes it
/// (ADR 0036): one Ghost, an app or a webhook names, any model provider's, one shaped
/// like a password, or one the vault or a plaintext file not imported yet holds. Names
/// compare without case.
///
/// `pending`: an update about to be written. The settings it would leave behind count
/// too, so one update can't name a password (`GHOST_LLM_PASSWORD`, `GHOST_APP_SECRETS`)
/// and write its value in plain text at once.
pub fn password_check(
    deps: &Deps,
    pending: Option<&BTreeMap<String, Option<String>>>,
) -> impl Fn(&str) -> bool {
    let mut settings = deps.settings();
    if let Some(pending) = pending {
        for (k, v) in pending {
            match v {
                None => {
                    settings.remove(k);
                }
                Some(v) => {
                    settings.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let listed = format!(
        "{},{}",
        settings.get("GHOST_APP_SECRETS").map_or("", String::as_str),
        settings.get("GHOST_MCP_SECRETS").map_or("", String::as_str)
    );
    let mut named: BTreeSet<String> = referenced(deps);
    named.extend(PASSWORD_NAMES.iter().map(|n| (*n).to_string()));
    named.extend(ghost_core::redact::password_names());
    named.extend(listed.split(',').map(|n| n.trim().to_string())); // any case: the check folds it
    named.insert(model_password(deps, &settings));
    named.insert(
        settings
            .get("GHOST_LLM_PASSWORD")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
    );
    named.remove("");
    if let Some(vault) = &deps.passwords
        && vault.is_open()
    {
        named.extend(vault.names());
    }
    move |k: &str| {
        let refs: Vec<&str> = named.iter().map(String::as_str).collect();
        is_password_name(k, &refs)
    }
}

pub fn passwords_view(deps: &Deps) -> ApiResult<PasswordsView> {
    let vault = deps
        .passwords
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    let uses = used_by(deps);
    let is_open = vault.is_open();
    let meta: BTreeMap<String, ghost_core::passwords::PasswordMeta> = if is_open {
        vault
            .info()
            .into_iter()
            .map(|m| (m.name.clone(), m))
            .collect()
    } else {
        BTreeMap::new()
    };
    // The keys of model providers not in use are listed once they're set (ADR 0037).
    let settings = deps.settings();
    let in_use = model_password(deps, &settings);
    let idle: BTreeSet<&str> = PROVIDER_ONLY
        .iter()
        .copied()
        .filter(|n| *n != in_use && !meta.contains_key(*n))
        .collect();
    let mut names: Vec<String> = GHOST_USES
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !idle.contains(n))
        .map(String::from)
        .collect();
    let mut rest: BTreeSet<String> = uses.keys().cloned().collect();
    rest.extend(meta.keys().cloned());
    for (n, _) in GHOST_USES {
        rest.remove(n);
    }
    for n in &idle {
        rest.remove(*n);
    }
    names.extend(rest);
    let rows = names
        .into_iter()
        .map(|name| {
            let m = meta.get(&name);
            PasswordInfo {
                set: if is_open { Some(m.is_some()) } else { None }, // unknown while it's closed
                used_by: uses.get(&name).cloned().unwrap_or_default(),
                ghost: ghost_use(&name).is_some() && name != "GITHUB_TOKEN",
                added: m.map(|m| m.added),
                updated: m.map(|m| m.updated),
                last_used: m.and_then(|m| m.last_used),
                name,
            }
        })
        .collect();
    let config_dir = deps
        .env_path
        .parent()
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    let importable = sources(&config_dir, Some(&deps.vault_env_path), &EnvFile)
        .unwrap_or_default()
        .into_iter()
        .map(|s| ImportSourceView {
            kind: match s.kind {
                SourceKind::HekaVault => ImportSourceKind::HekaVault,
                SourceKind::File => ImportSourceKind::File,
            },
            path: tilde(&s.path),
        })
        .collect();
    Ok(PasswordsView {
        vault: VaultView {
            state: match vault.state() {
                ghost_core::passwords::State::Open => VaultState::Open,
                ghost_core::passwords::State::Locked => VaultState::Locked,
                ghost_core::passwords::State::Unavailable => VaultState::Unavailable,
                ghost_core::passwords::State::Broken => VaultState::Broken,
            },
            problem: vault.problem(),
            path: tilde(vault.path()),
        },
        passwords: rows,
        importable,
    })
}

/// Set or not, for the Settings view's list.
pub fn secret_states(deps: &Deps, names: &[String]) -> Vec<(String, bool)> {
    let held: BTreeSet<String> = match &deps.passwords {
        Some(v) if v.is_open() => v.names().into_iter().collect(),
        _ => BTreeSet::new(),
    };
    names
        .iter()
        .map(|n| (n.clone(), held.contains(n)))
        .collect()
}

/// An app that couldn't connect for want of one of these tries again now.
pub async fn revive_apps_using(deps: &Deps, names: &[String]) {
    let Some(apps) = &deps.apps else {
        return;
    };
    for spec in apps.specs() {
        let wants = spec.secrets().iter().any(|s| names.contains(s));
        if wants && !apps.is_up(&spec.name) && apps.revive(&spec.name).await {
            deps.state.publish_apps(Some(&spec.name), None);
        }
    }
}

/// The values to save, stripped, each checked as the vault will: a 422 that never echoes
/// a value, before anything else changes.
pub fn check_passwords(
    values: &BTreeMap<String, Option<String>>,
) -> ApiResult<BTreeMap<String, String>> {
    let put: BTreeMap<String, String> = values
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| (k.clone(), v.trim().to_string())))
        .collect();
    for (k, v) in &put {
        check_name(k).map_err(|e| ApiError::unprocessable(e.to_string()))?;
        check_value(v).map_err(|e| ApiError::unprocessable(e.to_string()))?;
    }
    Ok(put)
}

fn vault_error(e: PasswordsError) -> ApiError {
    match e {
        PasswordsError::Invalid(m) => ApiError::unprocessable(m),
        PasswordsError::VaultClosed(m)
        | PasswordsError::KeyringLocked(m)
        | PasswordsError::KeyringUnavailable(m)
        | PasswordsError::VaultBroken(m) => {
            ApiError::locked(format!("{m}. Unlock it, then try again."))
        }
        PasswordsError::Io(e) => ApiError::internal(e.to_string()),
    }
}

/// Into the vault (None forgets one). True when one of them is Ghost's own, which it
/// picks up at its next start; the model connection's own keys are used from the next
/// session (ADR 0037).
pub fn save_passwords(deps: &Deps, values: &BTreeMap<String, Option<String>>) -> ApiResult<bool> {
    let vault = deps
        .passwords
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    let put = check_passwords(values)?;
    let gone: Vec<&str> = values
        .iter()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| k.as_str())
        .collect();
    if !put.is_empty() {
        vault
            .set_many(put.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .map_err(vault_error)?;
    }
    if !gone.is_empty() {
        vault.delete(&gone).map_err(vault_error)?;
    }
    let saved: Vec<&String> = put.keys().collect();
    info!("passwords: saved {saved:?}, forgot {gone:?}");
    let live = model_only(deps, &deps.settings());
    Ok(values
        .keys()
        .any(|k| ghost_use(k).is_some() && k != "GITHUB_TOKEN" && !live.contains(k)))
}

#[utoipa::path(get, path = "/passwords", operation_id = "get_passwords_passwords_get", tag = "passwords",
    summary = "Get Passwords", responses((status = 200, description = "Successful Response", body = PasswordsView)))]
async fn get_passwords(State(deps): State<SharedDeps>) -> ApiResult<Json<PasswordsView>> {
    Ok(Json(passwords_view(&deps)?))
}

/// Save or replace one. The value is never returned, here or anywhere.
#[utoipa::path(put, path = "/passwords/{name}", operation_id = "put_password_passwords__name__put", tag = "passwords",
    summary = "Put Password", description = "Save or replace one. The value is never returned, here or anywhere.",
    params(("name" = String, Path)), request_body = PasswordSet,
    responses((status = 200, description = "Successful Response", body = PasswordResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_password(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
    Body(body): Body<PasswordSet>,
) -> ApiResult<Json<PasswordResult>> {
    let mut values = BTreeMap::new();
    values.insert(name.clone(), Some(body.value.expose().to_string()));
    let restart = save_passwords(&deps, &values)?;
    revive_apps_using(&deps, std::slice::from_ref(&name)).await;
    Ok(Json(PasswordResult {
        passwords: passwords_view(&deps)?,
        restart_required: restart,
    }))
}

#[utoipa::path(delete, path = "/passwords/{name}", operation_id = "delete_password_passwords__name__delete",
    tag = "passwords", summary = "Delete Password", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = PasswordResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn delete_password(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<PasswordResult>> {
    let vault = deps
        .passwords
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    if !vault.is_open() {
        // whether it is there can't be known yet
        return Err(ApiError::locked(format!(
            "{}. Unlock it, then try again.",
            vault
                .problem()
                .unwrap_or_else(|| "The vault is closed".into())
        )));
    }
    if !vault.contains(&name) {
        return Err(ApiError::not_found(format!("no password called {name}")));
    }
    let mut values = BTreeMap::new();
    values.insert(name, None);
    let restart = save_passwords(&deps, &values)?;
    Ok(Json(PasswordResult {
        passwords: passwords_view(&deps)?,
        restart_required: restart,
    }))
}

/// Ask the keyring to unlock (it shows its own prompt on the desktop), then open the
/// vault. The daemon follows: Ghost's own settings and the apps that waited.
#[utoipa::path(post, path = "/passwords/unlock", operation_id = "unlock_passwords_unlock_post", tag = "passwords",
    summary = "Unlock",
    description = "Ask the keyring to unlock (it shows its own prompt on the desktop), then open\nthe vault. The daemon follows: Ghost's own settings and the apps that waited.",
    responses((status = 200, description = "Successful Response", body = PasswordsView)))]
async fn unlock(State(deps): State<SharedDeps>) -> ApiResult<Json<PasswordsView>> {
    let vault = deps
        .passwords
        .clone()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    if !vault.is_open() {
        let opened = tokio::task::spawn_blocking(move || vault.unlock())
            .await
            .unwrap_or(false);
        if opened {
            deps.control.passwords_opened().await;
        }
    }
    Ok(Json(passwords_view(&deps)?))
}

/// Once: heka-vault and the plaintext env files into the vault, then the plaintext copies
/// removed. A password already in the vault is kept.
#[utoipa::path(post, path = "/passwords/import", operation_id = "import_all_passwords_import_post", tag = "passwords",
    summary = "Import All",
    description = "Once: heka-vault and the plaintext env files into the vault, then the plaintext\ncopies removed. A password already in the vault is kept.",
    responses((status = 200, description = "Successful Response", body = PasswordImportResult)))]
async fn import_all(State(deps): State<SharedDeps>) -> ApiResult<Json<PasswordImportResult>> {
    let vault = deps
        .passwords
        .clone()
        .ok_or_else(|| ApiError::unavailable("the vault is not available"))?;
    let config_dir = deps
        .env_path
        .parent()
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    let heka = deps.vault_env_path.clone();
    let referenced_names = referenced(&deps);
    let report = tokio::task::spawn_blocking(move || {
        import_passwords(
            &vault,
            &config_dir,
            &EnvFile,
            ImportOptions {
                heka_env: Some(&heka),
                referenced: referenced_names,
                fetch: &fetch_heka,
            },
        )
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(vault_error)?;
    revive_apps_using(&deps, &report.imported).await;
    Ok(Json(PasswordImportResult {
        restart_required: !report.imported.is_empty() || !report.settings.is_empty(),
        imported: report.imported,
        kept: report.kept,
        settings: report.settings,
        removed: report.removed.iter().map(|p| tilde(p)).collect(),
        notes: report.notes,
        passwords: passwords_view(&deps)?,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_passwords))
        .routes(routes!(import_all))
        .routes(routes!(unlock))
        .routes(routes!(put_password, delete_password))
}
