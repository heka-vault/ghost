//! Skills catalog routes (#35): search skills.sh, preview a skill from GitHub, install it
//! through the gate, and check installed skills for updates.
//!
//! All of it is the operator's own doing, from the Skills page in Settings: the model has no
//! route here, and nothing here runs during a conversation. A search reaches skills.sh only
//! when the page asks (its results are cached an hour), a preview and an update check reach
//! GitHub only when the operator clicks. What the sources send is shown, never acted on.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path as PathArg, State};
use chrono::DateTime;
use ghost_core::models::Target;
use ghost_core::skill_catalog::{Preview, SkillCatalog, SkillCatalogError, slug};
use ghost_core::skills::{GHOST_SOURCE, SOURCE, Skill, SkillOrigin, Skills};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::{Qs, apply_internal};
use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, check_len};
use crate::models::{
    SkillFileView, SkillInfo, SkillInstall, SkillListing, SkillOriginView, SkillPlace,
    SkillPreview, SkillPreviewRequest, SkillSearchPage, SkillUpdateView,
};

pub fn origin_view(o: &SkillOrigin) -> SkillOriginView {
    SkillOriginView {
        source: o.source.clone(),
        source_url: o.source_url(),
        skill_path: o.skill_path.clone(),
        commit: o.commit.clone(),
        folder_hash: o.folder_hash.clone(),
        git_ref: o.git_ref.clone(),
        installed_at: (!o.installed_at.is_empty()).then(|| o.installed_at.clone()),
    }
}

/// One skill for the desktop, with where it came from when Ghost installed it.
pub fn skill_view(
    deps: &Deps,
    skill: &Skill,
    origins: Option<&BTreeMap<String, SkillOrigin>>,
) -> SkillInfo {
    let connected: BTreeSet<String> = deps
        .apps
        .as_ref()
        .map(|a| a.names().into_iter().filter(|n| a.is_up(n)).collect())
        .unwrap_or_default();
    let own;
    let origins = match origins {
        Some(o) => o,
        None => {
            own = deps
                .skills
                .as_ref()
                .map(|s| s.origins())
                .unwrap_or_default();
            &own
        }
    };
    let origin = if skill.source == GHOST_SOURCE {
        origins.get(&skill.name)
    } else {
        None
    };
    SkillInfo {
        name: skill.name.clone(),
        description: skill.description.clone(),
        source: skill.source.clone(),
        path: skill.path.display().to_string(),
        requires: skill.apps(),
        missing: skill.missing(&connected),
        files: skill.files(),
        problem: skill.problem.clone(),
        origin: origin.map(origin_view),
    }
}

fn catalog(deps: &Deps) -> ApiResult<(&SkillCatalog, &Skills)> {
    match (&deps.skill_catalog, &deps.skills) {
        (Some(c), Some(s)) => Ok((c, s)),
        _ => Err(ApiError::unavailable("the skills catalog is not available")),
    }
}

fn preview_view(skills: &Skills, p: &Preview, update_of: Option<&str>) -> SkillPreview {
    let existing = skills.all().into_iter().find(|s| s.name == p.name);
    SkillPreview {
        token: p.token.clone(),
        name: p.name.clone(),
        description: p.description.clone(),
        source: p.origin.source.clone(),
        source_url: p.origin.source_url(),
        skill_path: p.origin.skill_path.clone(),
        commit: p.origin.commit.clone(),
        folder_hash: p.origin.folder_hash.clone(),
        git_ref: p.origin.git_ref.clone(),
        skill_md: p.skill_md.clone(),
        files: p
            .files
            .iter()
            .map(|f| SkillFileView {
                path: f.path.clone(),
                bytes: f.bytes as u64,
                script: f.script,
            })
            .collect(),
        scripts: p.scripts(),
        installed: existing.map(|s| {
            if s.source == GHOST_SOURCE {
                SkillPlace::Ghost
            } else {
                SkillPlace::Home
            }
        }),
        update_of: update_of.map(String::from),
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    #[param(max_length = 100, default = "")]
    pub search: Option<String>,
}

/// Skills on skills.sh for a search of two letters or more, from Ghost's cache when
/// recent. A failure is `problem`, with the last results for the search if any.
#[utoipa::path(get, path = "/skill-catalog", operation_id = "search_skills_skill_catalog_get", tag = "skills",
    summary = "Search Skills",
    description = "Skills on skills.sh for a search of two letters or more, from Ghost's cache when\nrecent. A failure is ``problem``, with the last results for the search if any.",
    params(SearchQuery),
    responses((status = 200, description = "Successful Response", body = SkillSearchPage),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn search_skills(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<SearchQuery>,
) -> ApiResult<Json<SkillSearchPage>> {
    let search = q.search.unwrap_or_default();
    check_len(&["query", "search"], &search, 0, 100)?;
    let (cat, skills) = catalog(&deps)?;
    let res = cat.search(&search).await;
    // A listing is installed when a skill came from its repository under its id: the
    // folder's name there, or the name Ghost installed it as.
    let mut installed: BTreeMap<(String, String), String> = BTreeMap::new();
    for (n, o) in skills.origins() {
        let folder = o.folder();
        let last = folder.rsplit('/').next().unwrap_or(&folder);
        installed.insert((o.source.clone(), slug(last)), n.clone());
        installed.entry((o.source.clone(), n.clone())).or_insert(n);
    }
    Ok(Json(SkillSearchPage {
        query: res.query,
        skills: res
            .skills
            .iter()
            .map(|s| SkillListing {
                id: s.id.clone(),
                source: s.source.clone(),
                skill: s.skill.clone(),
                name: s.name.clone(),
                installs: s.installs as i64,
                installed: installed.get(&(s.source.clone(), slug(&s.skill))).cloned(),
            })
            .collect(),
        problem: res.problem,
        fetched_at: res
            .fetched_at
            .and_then(|t| DateTime::from_timestamp(t.trunc() as i64, (t.fract() * 1e9) as u32)),
    }))
}

/// Fetch a skill's folder from GitHub at the commit its branch is on, and stage it: its
/// SKILL.md and its files, for the operator to read before installing.
#[utoipa::path(post, path = "/skill-catalog/preview", operation_id = "preview_skill_skill_catalog_preview_post",
    tag = "skills", summary = "Preview Skill",
    description = "Fetch a skill's folder from GitHub at the commit its branch is on, and stage it:\nits SKILL.md and its files, for the operator to read before installing.",
    request_body = SkillPreviewRequest,
    responses((status = 200, description = "Successful Response", body = SkillPreview),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn preview_skill(
    State(deps): State<SharedDeps>,
    Body(req): Body<SkillPreviewRequest>,
) -> ApiResult<Json<SkillPreview>> {
    check_len(&["body", "source"], &req.source, 1, 300)?;
    if let Some(skill) = &req.skill {
        check_len(&["body", "skill"], skill, 0, 128)?;
    }
    let (cat, skills) = catalog(&deps)?;
    let p = cat
        .preview(&req.source, req.skill.as_deref())
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(preview_view(skills, &p, None)))
}

/// Copy a staged skill into Ghost's skills through the gate (`skills.install`), audited,
/// with where it came from.
#[utoipa::path(post, path = "/skill-catalog/install", operation_id = "install_skill_skill_catalog_install_post",
    tag = "skills", summary = "Install Skill",
    description = "Copy a staged skill into Ghost's skills through the gate (``skills.install``),\naudited, with where it came from.",
    request_body = SkillInstall,
    responses((status = 200, description = "Successful Response", body = SkillInfo),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn install_skill(
    State(deps): State<SharedDeps>,
    Body(req): Body<SkillInstall>,
) -> ApiResult<Json<SkillInfo>> {
    check_len(&["body", "token"], &req.token, 1, 64)?;
    let (cat, skills) = catalog(&deps)?;
    let p = cat.staged(&req.token).map_err(|e| match e {
        SkillCatalogError::Gone => ApiError::not_found(e.to_string()),
        other => ApiError::conflict(other.to_string()),
    })?;
    let prepared = skills
        .prepare_install(&p.folder, &p.origin, req.name.as_deref(), req.replace)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let name = match &prepared.target {
        Target::Skill(t) => t.name.clone(),
        other => other.spoken(),
    };
    apply_internal(
        &deps.gate,
        SOURCE,
        "install",
        prepared.target,
        prepared.changes,
        &format!("install {} from the desktop app", p.origin.source),
    )
    .await
    .map_err(|e| {
        if e.text() == "not applied" {
            ApiError::conflict("not installed")
        } else {
            e
        }
    })?;
    cat.discard(&req.token);
    skills.refresh();
    let skill = skills
        .all()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| ApiError::internal("the skill was copied but could not be read back"))?;
    Ok(Json(skill_view(&deps, &skill, None)))
}

/// For each skill Ghost installed from GitHub, whether its folder has changed at the
/// source since. Asks GitHub now (a recent answer per repository is reused).
#[utoipa::path(get, path = "/skill-catalog/updates", operation_id = "skill_updates_skill_catalog_updates_get",
    tag = "skills", summary = "Skill Updates",
    description = "For each skill Ghost installed from GitHub, whether its folder has changed at the\nsource since. Asks GitHub now (a recent answer per repository is reused).",
    responses((status = 200, description = "Successful Response", body = Vec<SkillUpdateView>)))]
async fn skill_updates(State(deps): State<SharedDeps>) -> ApiResult<Json<Vec<SkillUpdateView>>> {
    let (cat, skills) = catalog(&deps)?;
    let origins = skills.origins();
    let found = cat.check(&origins).await;
    Ok(Json(
        found
            .into_iter()
            .filter_map(|(name, latest)| {
                let origin = origins.get(&name)?;
                Some(SkillUpdateView {
                    name: name.clone(),
                    source: origin.source.clone(),
                    commit: origin.commit.clone(),
                    latest_commit: latest.commit.clone(),
                    update: latest.moved_on(origin),
                    problem: latest.problem.clone(),
                })
            })
            .collect(),
    ))
}

/// The newer version of an installed skill, staged like any preview; installing it with
/// `replace` and this name updates the skill.
#[utoipa::path(post, path = "/skill-catalog/updates/{name}",
    operation_id = "preview_update_skill_catalog_updates__name__post", tag = "skills", summary = "Preview Update",
    description = "The newer version of an installed skill, staged like any preview; installing it\nwith ``replace`` and this name updates the skill.",
    params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = SkillPreview),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn preview_update(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<SkillPreview>> {
    let key = name.trim().to_lowercase();
    let origin = deps
        .skills
        .as_ref()
        .and_then(|s| s.origins().remove(&key))
        .ok_or_else(|| ApiError::not_found(format!("{name} wasn't installed from GitHub")))?;
    let (cat, skills) = catalog(&deps)?;
    let p = cat
        .preview_update(&origin)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(Json(preview_view(skills, &p, Some(&key))))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(search_skills))
        .routes(routes!(install_skill))
        .routes(routes!(preview_skill))
        .routes(routes!(skill_updates))
        .routes(routes!(preview_update))
}
