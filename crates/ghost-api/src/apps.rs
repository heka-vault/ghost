//! App routes (ADR 0034): what is connected, the catalog, connecting and disconnecting, the
//! sign-in, and the permissions.
//!
//! Connecting is the operator's own action, from the desktop: it writes `apps.json` and puts
//! any password into Ghost's vault (ADR 0036), where the app finds it when it connects, at
//! once. A password goes in and never comes back out; `apps.json` holds only `${NAME}`.
//! The model has no route here.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path as PathArg, State};
use chrono::{DateTime, Utc};
use ghost_core::apps::{
    APPS_FILE, AppSpec, RESERVED_NAMES, app_name_problem, edit_apps, extract_secrets,
    parse_apps_json, secret_name,
};
use ghost_core::catalog::{self, CatalogEntry, Found, Kind, Registry as CoreRegistry};
use ghost_core::models::{py_repr, py_str};
use ghost_core::permissions::{Rule, parse_rules};
use ghost_core::policy::{AutonomyPolicy, write_permissions};
use ghost_core::provenance::{self, host_owner};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::{Qs, core_mode, level_of, permissions_view};
use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, check_len, check_range};
use crate::models::{
    Ack, AppAction, AppConnect, AppConnectResult, AppKind, AppState, AppView, CatalogEntryView,
    CatalogMaker, CatalogOption, CatalogOptionKind, CatalogPage, CatalogPublisher, CatalogSetting,
    HostOwner, Level, MakerEvidence, PermissionsUpdate, PermissionsView, Registry, Verified,
};
use crate::passwords::{check_passwords, save_passwords};

/// How long POST /apps waits for a connection or a sign-in address.
pub const CONNECT_WAIT_SECS: f64 = 10.0;
/// The catalog's first copy, so the screen has something to show.
pub const FIRST_COPY_WAIT_SECS: f64 = 15.0;

fn apps_path(deps: &Deps) -> PathBuf {
    deps.policy_path
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(APPS_FILE)
}

/// The registry's timestamp, or None when it isn't one.
fn when(rfc3339: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = rfc3339?;
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| ghost_core::time::parse_iso(raw))
}

fn kind_of(spec: &AppSpec) -> AppKind {
    if spec.command.is_some() {
        AppKind::Local
    } else if spec.sign_in {
        AppKind::SignIn
    } else if !spec.secrets().is_empty() {
        AppKind::Key
    } else {
        AppKind::Custom
    }
}

fn state_of(s: &str) -> AppState {
    match s {
        "connected" => AppState::Connected,
        "connecting" => AppState::Connecting,
        "needs_sign_in" => AppState::NeedsSignIn,
        "stopped" => AppState::Stopped,
        _ => AppState::Failed,
    }
}

fn action_views(deps: &Deps, name: &str) -> Vec<AppAction> {
    let Some(tools) = deps.apps.as_ref().and_then(|a| a.live_tools(name)) else {
        return Vec::new();
    };
    let policy = deps.gate.autonomy();
    let perms = &policy.permissions;
    let rules: Vec<&Rule> = perms
        .block
        .iter()
        .chain(&perms.ask)
        .chain(&perms.quiet)
        .chain(&perms.allow)
        .collect();
    tools
        .iter()
        .map(|t| {
            let conditional = rules
                .iter()
                .any(|r| r.conditional() && r.names(name, &t.name));
            let level = if t.read_only {
                if perms.blocks(name, &t.name, None) {
                    Level::Block
                } else {
                    Level::Allow
                }
            } else {
                level_of(perms.resolve(name, &t.name, None))
            };
            AppAction {
                name: t.name.clone(),
                description: t.brief(),
                reads: t.read_only,
                level,
                rule: format!("{name}.{}", t.name),
                conditional,
            }
        })
        .collect()
}

pub fn app_view(deps: &Deps, name: &str) -> ApiResult<AppView> {
    let apps = need_apps(deps)?;
    let spec = apps
        .spec(name)
        .ok_or_else(|| ApiError::not_found(format!("no app called {}", py_repr(name))))?;
    let entry = match (&deps.catalog, &spec.catalog) {
        (Some(cat), Some(c)) => cat.get(c),
        _ => None,
    };
    let url = deps.signins.as_ref().and_then(|s| s.pending_url(name));
    Ok(AppView {
        name: name.to_string(),
        title: spec.shown(),
        kind: kind_of(&spec),
        state: state_of(apps.state(name)),
        problem: apps.problem(name),
        catalog: spec.catalog.clone(),
        icon: entry.and_then(|e| e.icon.clone()),
        actions: action_views(deps, name),
        sign_in_url: url,
        settings: spec.secrets(),
    })
}

fn need_apps(deps: &Deps) -> ApiResult<&Arc<dyn crate::deps::AppsPort>> {
    deps.apps
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("apps are not available"))
}

/// Every app in apps.json: connected or not, and what each tool may do.
#[utoipa::path(get, path = "/apps", operation_id = "list_apps_apps_get", tag = "apps", summary = "List Apps",
    description = "Every app in apps.json: connected or not, and what each tool may do.",
    responses((status = 200, description = "Successful Response", body = Vec<AppView>)))]
async fn list_apps(State(deps): State<SharedDeps>) -> ApiResult<Json<Vec<AppView>>> {
    let apps = need_apps(&deps)?;
    apps.specs()
        .iter()
        .map(|s| app_view(&deps, &s.name))
        .collect::<ApiResult<Vec<_>>>()
        .map(Json)
}

#[utoipa::path(get, path = "/apps/{name}", operation_id = "get_app_apps__name__get", tag = "apps", summary = "Get App",
    params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = AppView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn get_app(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<AppView>> {
    need_apps(&deps)?;
    Ok(Json(app_view(&deps, &name)?))
}

// --- the catalog ------------------------------------------------------------------------

fn entry_view(
    deps: &Deps,
    e: &Arc<CatalogEntry>,
    others: &[Arc<CatalogEntry>],
) -> CatalogEntryView {
    let connected: Vec<String> = deps.apps.as_ref().map_or_else(Vec::new, |apps| {
        apps.specs()
            .into_iter()
            .filter(|s| s.catalog.as_deref() == Some(e.name.as_str()))
            .map(|s| s.name)
            .collect()
    });
    // The product's maker (#37): this entry when it is the maker's own, else the maker's
    // entry its title names. Who runs each remote is judged against it.
    let own = deps.catalog.as_ref().and_then(|c| c.product_maker(e));
    let maker = own.as_ref().and_then(|o| o.maker.as_ref());
    CatalogEntryView {
        name: e.name.clone(),
        title: e.title.clone(),
        description: e.description.clone(),
        version: e.version.clone(),
        website: e.website.clone(),
        repository: e.repository.clone(),
        icon: e.icon.clone(),
        featured: e.featured,
        options: e
            .options
            .iter()
            .map(|o| {
                let host = o.host();
                CatalogOption {
                    id: o.id.clone(),
                    kind: match o.kind {
                        Kind::SignIn => CatalogOptionKind::SignIn,
                        Kind::Key => CatalogOptionKind::Key,
                        Kind::Local => CatalogOptionKind::Local,
                    },
                    label: o.label.clone(),
                    settings: o
                        .settings
                        .iter()
                        .map(|s| CatalogSetting {
                            key: s.key.clone(),
                            label: s.label.clone(),
                            secret: s.secret,
                            required: s.required,
                            default: s.default.clone(),
                            choices: s.choices.clone(),
                        })
                        .collect(),
                    host_owner: host.as_deref().map(|h| {
                        match host_owner(h, maker, e.publisher.as_ref()) {
                            provenance::HostOwner::Maker => HostOwner::Maker,
                            provenance::HostOwner::Publisher => HostOwner::Publisher,
                            provenance::HostOwner::Other => HostOwner::Other,
                        }
                    }),
                    host,
                    registry: o.registry.map(|r| match r {
                        CoreRegistry::Npm => Registry::Npm,
                        CoreRegistry::Pypi => Registry::Pypi,
                        CoreRegistry::Oci => Registry::Oci,
                    }),
                    package: o.identifier.clone(),
                }
            })
            .collect(),
        suggested_name: free_name(deps, &e.app_name()),
        connected,
        updated_at: when(e.updated.as_deref()),
        publisher: e.publisher.as_ref().map(|p| CatalogPublisher {
            name: p.name.clone(),
            verified: match p.verified {
                provenance::Verified::Github => Verified::Github,
                provenance::Verified::Domain => Verified::Domain,
            },
        }),
        maker: e.maker.as_ref().map(|m| CatalogMaker {
            product: m.product.clone(),
            evidence: m
                .evidence
                .iter()
                .map(|ev| match ev {
                    provenance::Evidence::Publisher => MakerEvidence::Publisher,
                    provenance::Evidence::Listed => MakerEvidence::Listed,
                    provenance::Evidence::Remote => MakerEvidence::Remote,
                    provenance::Evidence::Repository => MakerEvidence::Repository,
                    provenance::Evidence::Website => MakerEvidence::Website,
                })
                .collect(),
        }),
        product: maker.map(|m| m.product.clone()),
        product_maker: own
            .as_ref()
            .filter(|o| o.name != e.name)
            .map(|o| o.name.clone()),
        others: others.iter().map(|o| entry_view(deps, o, &[])).collect(),
    }
}

/// `base`, or `base-2` and on when an app has it or Ghost's own tools do.
fn free_name(deps: &Deps, base: &str) -> String {
    let mut taken: Vec<String> = deps
        .apps
        .as_ref()
        .map(|a| a.specs().into_iter().map(|s| s.name).collect())
        .unwrap_or_default();
    taken.extend(RESERVED_NAMES.iter().map(|s| (*s).to_string()));
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    let mut n = 2;
    while taken.iter().any(|t| *t == format!("{base}-{n}")) {
        n += 1;
    }
    format!("{base}-{n}")
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CatalogQuery {
    #[param(max_length = 100, default = "")]
    pub search: Option<String>,
    #[param(minimum = 0, default = 0)]
    pub offset: Option<i64>,
    #[param(minimum = 1, maximum = 100, default = 40)]
    pub limit: Option<i64>,
}

/// Apps to connect, from Ghost's copy of the registry; first-party ones first when
/// nothing is searched. An old copy is refreshed in the background.
#[utoipa::path(get, path = "/catalog", operation_id = "catalog_catalog_get", tag = "apps", summary = "Catalog",
    description = "Apps to connect, from Ghost's copy of the registry; first-party ones first when\nnothing is searched. An old copy is refreshed in the background.",
    params(CatalogQuery),
    responses((status = 200, description = "Successful Response", body = CatalogPage),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn catalog_page(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<CatalogQuery>,
) -> ApiResult<Json<CatalogPage>> {
    let search = q.search.unwrap_or_default();
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(40);
    check_len(&["query", "search"], &search, 0, 100)?;
    check_range(&["query", "offset"], offset, 0, i64::MAX)?;
    check_range(&["query", "limit"], limit, 1, 100)?;
    let cat = deps
        .catalog
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("the catalog is not available"))?;
    cat.refresh_soon();
    if cat.fetched_at() <= 0.0 {
        cat.wait(FIRST_COPY_WAIT_SECS).await;
    }
    let (found, total): (Vec<Found>, usize) = cat.search(&search, limit as usize, offset as usize);
    let fetched_at = cat.fetched_at();
    Ok(Json(CatalogPage {
        entries: found
            .iter()
            .map(|f| entry_view(&deps, &f.entry, &f.others))
            .collect(),
        total: total as i64,
        fetched_at: (fetched_at > 0.0).then(|| {
            DateTime::from_timestamp(fetched_at.trunc() as i64, (fetched_at.fract() * 1e9) as u32)
                .unwrap_or_else(crate::state::now)
        }),
        refreshing: cat.refreshing(),
        problem: cat.problem(),
    }))
}

/// One entry, by its registry name (which has a slash in it: the route is a wildcard).
#[utoipa::path(get, path = "/catalog/{name}", operation_id = "catalog_entry_catalog__name__get", tag = "apps",
    summary = "Catalog Entry", params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = CatalogEntryView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
pub async fn catalog_entry(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<CatalogEntryView>> {
    let e = deps
        .catalog
        .as_ref()
        .and_then(|c| c.get(&name))
        .ok_or_else(|| ApiError::not_found(format!("{name} is not in the catalog")))?;
    Ok(Json(entry_view(&deps, &e, &[])))
}

// --- connecting ---------------------------------------------------------------------------

/// Into the vault (ADR 0036); never logged. A locked vault refuses the connection before
/// `apps.json` changes.
fn keep_secrets(deps: &Deps, secrets: &BTreeMap<String, String>) -> ApiResult<()> {
    if secrets.is_empty() {
        return Ok(());
    }
    let values: BTreeMap<String, Option<String>> = secrets
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    save_passwords(deps, &values).map(|_| ())
}

/// The passwords Ghost kept for this app: the `GHOST_APP_` names its own entry refers to
/// and no other app's does. A name the operator chose themselves stays, and so does every
/// other app's (`github-2`'s are not `github`'s).
fn own_secrets(deps: &Deps, spec: &AppSpec) -> Vec<String> {
    let others: Vec<String> = deps
        .apps
        .as_ref()
        .map(|a| {
            a.specs()
                .iter()
                .filter(|s| s.name != spec.name)
                .flat_map(AppSpec::secrets)
                .collect()
        })
        .unwrap_or_default();
    spec.secrets()
        .into_iter()
        .filter(|k| k.starts_with("GHOST_APP_") && !others.contains(k))
        .collect()
}

fn forget_secrets(deps: &Deps, spec: &AppSpec) -> ApiResult<()> {
    let Some(vault) = &deps.passwords else {
        return Ok(());
    };
    let gone: BTreeMap<String, Option<String>> = own_secrets(deps, spec)
        .into_iter()
        .filter(|k| vault.contains(k))
        .map(|k| (k, None))
        .collect();
    if !gone.is_empty() {
        save_passwords(deps, &gone)?;
    }
    Ok(())
}

/// Another app whose keys this one's would overwrite: `my-app` and `my_app` keep theirs
/// under the same names.
fn clash(deps: &Deps, spec: &AppSpec, secrets: &BTreeMap<String, String>) -> Option<String> {
    let prefix = secret_name(&spec.name, "");
    let specs = deps.apps.as_ref()?.specs();
    specs
        .iter()
        .filter(|other| other.name != spec.name)
        .find(|other| {
            secret_name(&other.name, "") == prefix
                || other.secrets().iter().any(|k| secrets.contains_key(k))
        })
        .map(|other| other.name.clone())
}

/// Only these entries change in apps.json (`edit_apps`); a file that isn't JSON is
/// refused, never overwritten.
fn save(deps: &Deps, specs: &[AppSpec], remove: Option<&str>) -> ApiResult<()> {
    edit_apps(&apps_path(deps), specs, remove)
        .map_err(|e| ApiError::conflict(format!("{APPS_FILE} can't be changed: {e}")))
}

/// Connect each now, in the background; wait a little for a connection or for a sign-in
/// to want the browser. Returns the sign-in address, if one does.
async fn start(deps: &SharedDeps, specs: &[AppSpec]) -> Option<String> {
    let mut tasks = Vec::new();
    for spec in specs {
        let deps = deps.clone();
        let spec = spec.clone();
        let name = spec.name.clone();
        tasks.push((
            name.clone(),
            tokio::spawn(async move {
                if let Some(apps) = &deps.apps {
                    apps.add(spec, true).await;
                }
                deps.state.publish_apps(Some(&name), None);
            }),
        ));
    }
    let deadline = Instant::now() + Duration::from_secs_f64(CONNECT_WAIT_SECS);
    while Instant::now() < deadline {
        if let Some(signins) = &deps.signins {
            for (name, _) in &tasks {
                if let Some(url) = signins.pending_url(name) {
                    return Some(url);
                }
            }
        }
        if tasks.iter().all(|(_, t)| t.is_finished()) {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

fn values_of(req: &AppConnect) -> HashMap<String, String> {
    req.values
        .as_ref()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        other => py_str(other),
                    };
                    (k.clone(), s)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The host as `re.sub(r"[^a-z0-9]+", "-", host.lower())`.
fn host_to_name(host: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in host.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out
}

/// What a connect request asks for: the apps to connect, the secrets to keep, and what
/// to tell the operator.
struct Asked {
    specs: Vec<AppSpec>,
    secrets: BTreeMap<String, String>,
    notes: Vec<String>,
}

/// The apps to connect, the secrets to keep, and what to tell the operator.
fn specs_from(deps: &Deps, req: &AppConnect) -> ApiResult<Asked> {
    let values = values_of(req);
    if let Some(config) = &req.config {
        check_len(&["body", "config"], config, 0, 20000)?;
        let parsed = parse_apps_json(config, "the pasted config")
            .map_err(|e| ApiError::unprocessable(e.to_string()))?;
        if parsed.apps.is_empty() || !parsed.left_out.is_empty() {
            // Every entry, or none: one left out silently is one the operator thinks is
            // connected.
            let why = parsed.problems.join("; ");
            return Err(ApiError::unprocessable(if why.is_empty() {
                "no apps in it".to_string()
            } else {
                why
            }));
        }
        let mut specs = Vec::new();
        let mut secrets = BTreeMap::new();
        for spec in &parsed.apps {
            let (moved, found) = extract_secrets(spec);
            specs.push(moved);
            secrets.extend(found);
        }
        // A key written into the paste is moved, not a problem left to fix.
        let notes = parsed
            .problems
            .iter()
            .filter(|p| !p.contains("looks like a key"))
            .cloned()
            .collect();
        return Ok(Asked {
            specs,
            secrets,
            notes,
        });
    }
    if let Some(url) = &req.url {
        let parsed = url::Url::parse(url).ok();
        let host = parsed
            .as_ref()
            .filter(|u| matches!(u.scheme(), "https" | "http"))
            .and_then(|u| u.host_str().map(String::from))
            .ok_or_else(|| ApiError::unprocessable("the address must start with https://"))?;
        if let Some(name) = &req.name {
            check_len(&["body", "name"], name, 0, 40)?;
        }
        let name = req.name.clone().unwrap_or_else(|| host_to_name(&host));
        let name = name.strip_prefix("mcp-").unwrap_or(&name).to_string();
        let name = name.strip_suffix("-com").unwrap_or(&name).to_string();
        let name: String = name.trim_matches('-').chars().take(40).collect();
        let name = if name.is_empty() {
            "app".to_string()
        } else {
            name
        };
        let mut headers = indexmap::IndexMap::new();
        let mut secrets = BTreeMap::new();
        let key = values.get("key").map(|k| k.trim()).unwrap_or("");
        if !key.is_empty() {
            let env = secret_name(&name, "key");
            secrets.insert(
                env.clone(),
                if key.contains(' ') {
                    key.to_string()
                } else {
                    format!("Bearer {key}")
                },
            );
            headers.insert("Authorization".to_string(), format!("${{{env}}}"));
        }
        let spec = AppSpec {
            name,
            title: values.get("title").cloned().unwrap_or_default(),
            url: Some(url.clone()),
            headers,
            sign_in: key.is_empty(),
            ..AppSpec::default()
        };
        return Ok(Asked {
            specs: vec![spec],
            secrets,
            notes: Vec::new(),
        });
    }
    let (Some(catalog_name), Some(option)) = (&req.catalog, &req.option) else {
        return Err(ApiError::unprocessable(
            "give catalog and option, a url, or a config",
        ));
    };
    let entry = deps
        .catalog
        .as_ref()
        .and_then(|c| c.get(catalog_name))
        .ok_or_else(|| ApiError::not_found(format!("{catalog_name} is not in the catalog")))?;
    let name = match &req.name {
        Some(n) => {
            check_len(&["body", "name"], n, 0, 40)?;
            n.clone()
        }
        None => free_name(deps, &entry.app_name()),
    };
    let (spec, secrets) = catalog::connect(&entry, option, &name, &values)
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok(Asked {
        specs: vec![spec],
        secrets,
        notes: Vec::new(),
    })
}

/// Connect an app now and keep it in apps.json. An app that signs in comes back with the
/// address the browser should open; the desktop opens it.
#[utoipa::path(post, path = "/apps", operation_id = "connect_app_apps_post", tag = "apps", summary = "Connect App",
    description = "Connect an app now and keep it in apps.json. An app that signs in comes back\nwith the address the browser should open; the desktop opens it.",
    request_body = AppConnect,
    responses((status = 200, description = "Successful Response", body = AppConnectResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn connect_app(
    State(deps): State<SharedDeps>,
    Body(req): Body<AppConnect>,
) -> ApiResult<Json<AppConnectResult>> {
    let apps = need_apps(&deps)?;
    let Asked {
        specs,
        secrets,
        notes,
    } = specs_from(&deps, &req)?;
    for spec in &specs {
        if let Some(why) = app_name_problem(&spec.name) {
            return Err(ApiError::unprocessable(format!(
                "{}: {why}",
                py_repr(&spec.name)
            )));
        }
        if apps.spec(&spec.name).is_some() && !req.replace.unwrap_or(false) {
            return Err(ApiError::conflict(format!(
                "an app is already connected as {}",
                spec.name
            )));
        }
        if let Some(other) = clash(&deps, spec, &secrets) {
            return Err(ApiError::conflict(format!(
                "{} would share its keys with {other}; give it another name",
                spec.name
            )));
        }
    }
    if !secrets.is_empty() && !deps.passwords.as_ref().is_some_and(|v| v.is_open()) {
        return Err(ApiError::locked(
            "Your keyring is locked. Unlock it, then connect again.",
        ));
    }
    let as_values: BTreeMap<String, Option<String>> = secrets
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    check_passwords(&as_values)?; // a refused password changes nothing either
    save(&deps, &specs, None)?; // first: a file that won't take the entry changes nothing
    keep_secrets(&deps, &secrets)?;
    let url = start(&deps, &specs).await;
    let views = specs
        .iter()
        .map(|s| app_view(&deps, &s.name))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(AppConnectResult {
        apps: views,
        sign_in_url: url,
        problems: notes,
    }))
}

/// Sign in again, or reconnect an app that isn't connected.
#[utoipa::path(post, path = "/apps/{name}/sign-in", operation_id = "sign_in_apps__name__sign_in_post", tag = "apps",
    summary = "Sign In", description = "Sign in again, or reconnect an app that isn't connected.",
    params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = AppConnectResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn sign_in(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<AppConnectResult>> {
    let apps = need_apps(&deps)?;
    let spec = apps
        .spec(&name)
        .ok_or_else(|| ApiError::not_found(format!("no app called {}", py_repr(&name))))?;
    if let Some(signins) = &deps.signins
        && spec.sign_in
    {
        signins.forget(&name); // a fresh registration and tokens
    }
    let url = start(&deps, std::slice::from_ref(&spec)).await;
    Ok(Json(AppConnectResult {
        apps: vec![app_view(&deps, &name)?],
        sign_in_url: url,
        problems: Vec::new(),
    }))
}

/// Disconnect: it leaves the tools at once and apps.json; its sign-in and the keys Ghost
/// kept for it are forgotten.
#[utoipa::path(delete, path = "/apps/{name}", operation_id = "disconnect_apps__name__delete", tag = "apps",
    summary = "Disconnect",
    description = "Disconnect: it leaves the tools at once and apps.json; its sign-in and the keys\nGhost kept for it are forgotten.",
    params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn disconnect(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let apps = need_apps(&deps)?;
    let spec = apps
        .spec(&name)
        .ok_or_else(|| ApiError::not_found(format!("no app called {}", py_repr(&name))))?;
    if let Some(vault) = &deps.passwords
        && !vault.is_open()
        && !own_secrets(&deps, &spec).is_empty()
    {
        // Its passwords would stay behind in a vault Ghost can't open now: nothing
        // changes until it can.
        return Err(ApiError::locked(
            "Your keyring is locked, so Ghost can't forget this app's passwords. Unlock it on \
             Settings › Passwords, then disconnect again.",
        ));
    }
    save(&deps, &[], Some(&name))?; // first: a file that won't take the change leaves it connected
    apps.remove(&name).await;
    if let Some(signins) = &deps.signins {
        signins.forget(&name);
    }
    forget_secrets(&deps, &spec)?;
    deps.state.publish_apps(Some(&name), None);
    Ok(Json(Ack::ok()))
}

// --- permissions ----------------------------------------------------------------------------

fn policy_file(deps: &Deps) -> ApiResult<(String, AutonomyPolicy)> {
    let text = if deps.policy_path.exists() {
        std::fs::read_to_string(&deps.policy_path).map_err(|e| ApiError::internal(e.to_string()))?
    } else {
        String::new()
    };
    let p = AutonomyPolicy::parse(&text, &deps.policy_path.display().to_string())
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok((text, p))
}

#[utoipa::path(get, path = "/permissions", operation_id = "get_permissions_permissions_get", tag = "permissions",
    summary = "Get Permissions", responses((status = 200, description = "Successful Response", body = PermissionsView)))]
async fn get_permissions(State(deps): State<SharedDeps>) -> ApiResult<Json<PermissionsView>> {
    let (_, p) = policy_file(&deps)?;
    Ok(Json(permissions_view(&p, &deps.policy_path)))
}

/// Change the mode, whole lists, or one rule; only `[permissions]` in the file is
/// rewritten, and the gate follows at once.
#[utoipa::path(put, path = "/permissions", operation_id = "put_permissions_permissions_put", tag = "permissions",
    summary = "Put Permissions",
    description = "Change the mode, whole lists, or one rule; only ``[permissions]`` in the file is\nrewritten, and the gate follows at once.",
    request_body = PermissionsUpdate,
    responses((status = 200, description = "Successful Response", body = PermissionsView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_permissions(
    State(deps): State<SharedDeps>,
    Body(update): Body<PermissionsUpdate>,
) -> ApiResult<Json<PermissionsView>> {
    let (text, policy) = policy_file(&deps)?;
    let mut perms = policy.permissions.clone();
    if let Some(mode) = update.mode {
        perms.mode = core_mode(mode);
    }
    let lists: [(&str, &Option<Vec<String>>); 4] = [
        ("block", &update.block),
        ("ask", &update.ask),
        ("quiet", &update.quiet),
        ("allow", &update.allow),
    ];
    for (key, value) in lists {
        let Some(value) = value else { continue };
        let raw = toml::Value::Array(
            value
                .iter()
                .map(|s| toml::Value::String(s.clone()))
                .collect(),
        );
        let rules = parse_rules(&raw, key).map_err(|e| ApiError::unprocessable(e.to_string()))?;
        match key {
            "block" => perms.block = rules,
            "ask" => perms.ask = rules,
            "quiet" => perms.quiet = rules,
            _ => perms.allow = rules,
        }
    }
    if let Some(set) = &update.set {
        check_len(&["body", "set", "rule"], &set.rule, 1, 300)?;
        let rule = Rule::parse(&set.rule).map_err(|e| ApiError::unprocessable(e.to_string()))?;
        perms = perms
            .with_rule(&rule, set.level.map(Level::as_str))
            .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    }
    let out =
        write_permissions(&text, &perms).map_err(|e| ApiError::unprocessable(e.to_string()))?;
    if let Some(parent) = deps.policy_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    std::fs::write(&deps.policy_path, out).map_err(|e| ApiError::internal(e.to_string()))?;
    let p = deps
        .control
        .reload_policy()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    deps.state.publish_apps(None, None);
    Ok(Json(permissions_view(&p, &deps.policy_path)))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(list_apps, connect_app))
        .routes(routes!(get_app, disconnect))
        .routes(routes!(sign_in))
        .routes(routes!(catalog_page))
        .routes(routes!(get_permissions, put_permissions))
}
