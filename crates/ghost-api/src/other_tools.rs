//! The AI tools already on this computer (ADR 0068): what Claude Code, Cursor and Codex
//! have connected, letting them use Ghost, and a model connection that is already there.
//!
//! `GET /other-tools` reads a fixed list of their files and asks two loopback ports, and
//! answers with names and kinds: never a secret, an address without its query or
//! user-info, a command exactly as it would run. `POST /other-tools/{tool}/import`
//! connects what the operator chose through the same path the desktop's connect takes
//! (`apps::connect_specs`): each secret into the vault, `${NAME}` in `apps.json`.
//! `POST /other-tools/{tool}/link` makes a client as `ghostd mcp add` does and adds Ghost
//! to that tool, by its own command or into its file after a backup; `DELETE` takes it
//! back out. Every route is the operator's: no tool of Ghost's own app, nothing for
//! outside clients, nothing on the phone, and no model anywhere.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path as PathArg, State};
use ghost_core::other_tools::link::{Bridge, How, plan_link, remove_link, unlink_argv, write_link};
use ghost_core::other_tools::models::{LOCAL_SERVERS, keys_present, model_names};
use ghost_core::other_tools::{
    FileState, Ghost, Shown, Tool, ToolFound, Where, detect, noticed_line, shown_path,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::apps::connect_specs;
use crate::clients::{ClientView, clients_view};
use crate::deps::{Deps, OtherToolsPort, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};
use crate::models::AppView;

/// A file another tool keeps its apps in, as read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OtherFileView {
    /// The file, from the home folder (`~/.claude.json`).
    pub path: String,
    /// read, missing, too_large, outside (a link out of the home folder) or unreadable.
    pub state: String,
    /// Why it was not read, in a sentence.
    #[schema(required)]
    pub problem: Option<String>,
}

/// An app another tool uses, as Ghost would connect it. Never a secret's value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FoundAppView {
    /// Which entry: what an import names.
    pub key: String,
    /// Its name in the other tool.
    pub name: String,
    /// "every project", or "~/code/site only".
    pub scope: String,
    pub file: String,
    /// The name it would have in Ghost.
    pub suggested: String,
    /// address or command.
    pub kind: String,
    /// The address, without its query or user-info.
    #[schema(required)]
    pub address: Option<String>,
    /// The command Ghost would run, word by word, exactly as given: someone else's text.
    pub command: Vec<String>,
    /// The same command as one line, quoted where a shell would read it. Shown only.
    #[schema(required)]
    pub command_line: Option<String>,
    /// It signs in through Ghost's own sign-in; the other tool's is never copied.
    pub sign_in: bool,
    /// What goes into Ghost's vault, by where it was ("header Authorization").
    pub moved: Vec<String>,
    /// Passwords it refers to that Ghost does not have yet, by name.
    pub needs: Vec<String>,
    pub notes: Vec<String>,
    /// Why it is not offered, in a sentence; None when it can be connected.
    #[schema(required)]
    pub skip: Option<String>,
}

/// What letting the tool use Ghost would do, shown before it is done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LinkView {
    /// The client it would be (`ghostd mcp add`'s name).
    pub client: String,
    /// command (the tool's own) or file (Ghost writes it, after a backup).
    pub by: String,
    /// The command, or the file and what is added to it; `<token>` stands for the token.
    pub shown: String,
    /// It connects by address, with a token in its config.
    pub by_address: bool,
}

/// Ghost already in the tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UsesGhostView {
    pub entry: String,
    /// The client it names, when it names one.
    #[schema(required)]
    pub client: Option<String>,
    pub file: String,
    /// Ghost can take it back out (it is in the tool's own user file).
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OtherToolView {
    /// claude-code, cursor or codex.
    pub id: String,
    pub title: String,
    /// Its folder or its command is on this computer.
    pub installed: bool,
    pub files: Vec<OtherFileView>,
    pub apps: Vec<FoundAppView>,
    #[schema(required)]
    pub uses_ghost: Option<UsesGhostView>,
    #[schema(required)]
    pub link: Option<LinkView>,
    /// Why Ghost can't add itself to the tool, in a sentence.
    #[schema(required)]
    pub link_problem: Option<String>,
}

/// A model connection that is already there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FoundModelView {
    /// The provider id Settings › Model takes.
    pub provider: String,
    pub title: String,
    /// How Ghost knows, in a sentence.
    pub how: String,
    /// What a local server lists; empty for a key.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OtherToolsView {
    /// "Cursor and Claude Code are set up on this computer.", or None.
    #[schema(required)]
    pub line: Option<String>,
    pub tools: Vec<OtherToolView>,
    pub models: Vec<FoundModelView>,
    /// Tools here whose own sign-in Ghost may not use (Claude Code, Codex).
    pub own_sign_in: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct OtherImport {
    /// The keys of the apps to connect.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Every app offered.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SkippedView {
    pub key: String,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OtherImportResult {
    pub apps: Vec<AppView>,
    pub skipped: Vec<SkippedView>,
    /// The first app that wants the browser for its sign-in.
    #[schema(required)]
    pub sign_in_url: Option<String>,
    pub tools: OtherToolsView,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OtherLinkResult {
    /// What was done, in a sentence.
    pub done: String,
    /// The client made; None when Ghost was taken out.
    #[schema(required)]
    pub client: Option<ClientView>,
    /// The backup Ghost made of the tool's file.
    #[schema(required)]
    pub backup: Option<String>,
    pub tools: OtherToolsView,
}

fn port(deps: &Deps) -> ApiResult<&dyn OtherToolsPort> {
    deps.other_tools
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("other tools are not available"))
}

fn tool_of(id: &str) -> ApiResult<Tool> {
    Tool::parse(id).ok_or_else(|| ApiError::not_found(format!("no tool called {id}")))
}

/// Ghost as detection needs it: its apps, the passwords it has by name, its ports.
fn ghost_of(deps: &Deps) -> Ghost {
    let mut passwords: std::collections::BTreeSet<String> = deps.ghost_env().into_keys().collect();
    if let Some(v) = &deps.passwords {
        passwords.extend(v.names());
    }
    let mut ports: Vec<u16> = deps
        .clients_listening
        .iter()
        .filter_map(|a| a.rsplit_once(':').and_then(|(_, p)| p.parse().ok()))
        .collect();
    ports.push(47824);
    Ghost {
        connected: deps.apps.as_ref().map(|a| a.specs()).unwrap_or_default(),
        passwords,
        ports,
    }
}

fn bridge(deps: &Deps) -> Result<Bridge, String> {
    if !deps.in_appimage
        && let Some(g) = ghost_core::ghost_app::clients::stable_ghostd(|_| None)
    {
        return Ok(Bridge::Command(g));
    }
    match deps.clients_listening.first() {
        Some(a) => Ok(Bridge::Address(a.clone())),
        None => {
            Err("Ghost is not listening for outside tools, so another tool cannot reach it.".into())
        }
    }
}

fn client_names(deps: &Deps) -> Vec<String> {
    deps.clients
        .as_ref()
        .and_then(|c| c.list().ok())
        .map(|l| l.into_iter().map(|c| c.name).collect())
        .unwrap_or_default()
}

fn found_now(deps: &Deps) -> ApiResult<(Where, Vec<ToolFound>)> {
    let w = port(deps)?.place();
    let mut opened = Vec::new();
    let found = detect(&w, &ghost_of(deps), &mut opened);
    Ok((w, found))
}

fn app_view_of(a: &ghost_core::other_tools::FoundApp) -> FoundAppView {
    let (kind, address, command) = match &a.shown {
        Shown::Address(u) => ("address", Some(u.clone()), Vec::new()),
        Shown::Command(argv) => ("command", None, argv.clone()),
    };
    FoundAppView {
        key: a.key.clone(),
        name: a.name.clone(),
        scope: a.scope.clone(),
        file: a.file.clone(),
        suggested: a.suggested.clone(),
        kind: kind.into(),
        address,
        command_line: (!command.is_empty())
            .then(|| ghost_core::other_tools::link::shell_words(&command)),
        command,
        sign_in: a.sign_in,
        moved: a.moved.clone(),
        needs: a.needs.clone(),
        notes: a.notes.clone(),
        skip: a.skip.as_ref().map(|s| s.sentence()),
    }
}

fn tool_view(deps: &Deps, w: &Where, t: &ToolFound) -> OtherToolView {
    let link = bridge(deps).and_then(|b| plan_link(w, t, &client_names(deps), &b));
    let user_file = shown_path(&w.home, &w.user_file(t.tool));
    OtherToolView {
        id: t.tool.id().into(),
        title: t.tool.title().into(),
        installed: t.installed,
        files: t
            .files
            .iter()
            .filter(|f| f.state != FileState::Missing)
            .map(|f| OtherFileView {
                path: f.shown.clone(),
                state: match f.state {
                    FileState::Read => "read",
                    FileState::Missing => "missing",
                    FileState::TooLarge(_) => "too_large",
                    FileState::Outside => "outside",
                    FileState::Unreadable(_) => "unreadable",
                }
                .into(),
                problem: f
                    .problem_line(t.tool)
                    .or_else(|| f.problems.first().cloned()),
            })
            .collect(),
        apps: t.apps.iter().map(app_view_of).collect(),
        uses_ghost: t.linked.as_ref().map(|l| UsesGhostView {
            entry: l.entry.clone(),
            client: l.client.clone(),
            removable: l.file == user_file,
            file: l.file.clone(),
        }),
        link_problem: match (&link, t.installed, &t.linked) {
            (Err(e), true, None) => Some(e.clone()),
            _ => None,
        },
        link: link.ok().filter(|_| t.installed).map(|p| LinkView {
            client: p.client.clone(),
            by: match p.how {
                How::Command(_) => "command",
                How::File { .. } => "file",
            }
            .into(),
            shown: p.shown(),
            by_address: p.by_address,
        }),
    }
}

async fn view(deps: &Deps) -> ApiResult<OtherToolsView> {
    let (w, found) = found_now(deps)?;
    let p = port(deps)?;
    let mut models = Vec::new();
    for s in &LOCAL_SERVERS {
        if let Some(body) = p.get_local(s.list_url).await {
            let names = model_names(&body);
            let how = match names.len() {
                0 => format!(
                    "{} is running on this computer, with no model yet.",
                    s.title
                ),
                1 => format!("{} is running on this computer, with 1 model.", s.title),
                n => format!("{} is running on this computer, with {n} models.", s.title),
            };
            models.push(FoundModelView {
                provider: s.provider.into(),
                title: s.title.into(),
                how,
                models: names,
            });
        }
    }
    for (var, provider) in keys_present(&deps.started_env) {
        let title = ghost_core::connection::provider(provider).map_or(provider, |p| p.title);
        models.push(FoundModelView {
            provider: provider.into(),
            title: title.into(),
            how: format!("{var} is set in the environment Ghost was started in."),
            models: Vec::new(),
        });
    }
    Ok(OtherToolsView {
        line: noticed_line(&found),
        tools: found
            .iter()
            .filter(|t| {
                t.installed
                    || !t.apps.is_empty()
                    || t.files.iter().any(|f| f.state != FileState::Missing)
            })
            .map(|t| tool_view(deps, &w, t))
            .collect(),
        models,
        own_sign_in: found
            .iter()
            .filter(|t| t.installed && t.tool.has_own_sign_in())
            .map(|t| t.tool.title().to_string())
            .collect(),
    })
}

/// What the AI tools on this computer have set up: their apps, whether they use Ghost,
/// and a model connection already there. Read-only, local, names and kinds only.
#[utoipa::path(get, path = "/other-tools", operation_id = "other_tools_other_tools_get", tag = "other-tools",
    summary = "Other Tools",
    description = "What the AI tools on this computer (Claude Code, Cursor, Codex) have set up, read\nfrom their own files: their apps, whether they use Ghost, and a model connection that is\nalready there (a local server, a key in Ghost's environment). Names and kinds only.",
    responses((status = 200, description = "Successful Response", body = OtherToolsView)))]
async fn get_other_tools(State(deps): State<SharedDeps>) -> ApiResult<Json<OtherToolsView>> {
    Ok(Json(view(&deps).await?))
}

/// Connect apps another tool uses, chosen by the operator.
#[utoipa::path(post, path = "/other-tools/{tool}/import", operation_id = "import_other_tool_apps_other_tools__tool__import_post",
    tag = "other-tools", summary = "Import Other Tool Apps",
    description = "Connect apps another tool uses: each one's secrets into Ghost's vault and\n`${NAME}` in apps.json, through the same path as connecting by hand. One already connected\nis skipped, so a second import changes nothing.",
    params(("tool" = String, Path)),
    request_body = OtherImport,
    responses((status = 200, description = "Successful Response", body = OtherImportResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn import_apps(
    State(deps): State<SharedDeps>,
    PathArg(tool): PathArg<String>,
    Body(req): Body<OtherImport>,
) -> ApiResult<Json<OtherImportResult>> {
    let tool = tool_of(&tool)?;
    if !req.all && req.apps.is_empty() {
        return Err(ApiError::unprocessable("name the apps, or ask for all"));
    }
    let (_, found) = found_now(&deps)?;
    let t = found
        .iter()
        .find(|t| t.tool == tool)
        .ok_or_else(|| ApiError::not_found("no such tool"))?;
    for key in &req.apps {
        if !t.apps.iter().any(|a| &a.key == key) {
            return Err(ApiError::not_found(format!(
                "{} has no app {key:?} now; look again",
                tool.title()
            )));
        }
    }
    let mut specs = Vec::new();
    let mut secrets: BTreeMap<String, String> = BTreeMap::new();
    let mut skipped = Vec::new();
    for a in t
        .apps
        .iter()
        .filter(|a| req.all || req.apps.contains(&a.key))
    {
        match a.planned() {
            Some(p) => {
                specs.push(p.spec.clone());
                secrets.extend(p.secrets.iter().map(|(k, v)| (k.clone(), v.to_string())));
            }
            None => skipped.push(SkippedView {
                key: a.key.clone(),
                name: a.name.clone(),
                why: a.skip.as_ref().map(|s| s.sentence()).unwrap_or_default(),
            }),
        }
    }
    let (apps, sign_in_url) = if specs.is_empty() {
        (Vec::new(), None)
    } else {
        connect_specs(&deps, &specs, &secrets, false).await?
    };
    tracing::info!(
        "other tools: {} app(s) connected from {}, {} skipped",
        apps.len(),
        tool.title(),
        skipped.len()
    );
    Ok(Json(OtherImportResult {
        apps,
        skipped,
        sign_in_url,
        tools: view(&deps).await?,
    }))
}

/// Let another tool use Ghost: a client, and Ghost added to the tool.
#[utoipa::path(post, path = "/other-tools/{tool}/link", operation_id = "link_other_tool_other_tools__tool__link_post",
    tag = "other-tools", summary = "Link Other Tool",
    description = "Let another tool use Ghost: a client made as `ghostd mcp add` makes one, and Ghost\nadded to the tool by its own command or into its file after a backup beside it. The\ntoken is never returned.",
    params(("tool" = String, Path)),
    responses((status = 200, description = "Successful Response", body = OtherLinkResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn link_tool(
    State(deps): State<SharedDeps>,
    PathArg(tool): PathArg<String>,
) -> ApiResult<Json<OtherLinkResult>> {
    let tool = tool_of(&tool)?;
    let clients = deps
        .clients
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("outside tools are not available"))?;
    let (w, found) = found_now(&deps)?;
    let t = found.iter().find(|t| t.tool == tool).unwrap();
    if !t.installed {
        return Err(ApiError::conflict(format!(
            "{} is not set up on this computer.",
            tool.title()
        )));
    }
    let b = bridge(&deps).map_err(ApiError::conflict)?;
    let plan = plan_link(&w, t, &client_names(&deps), &b).map_err(ApiError::conflict)?;
    let command = matches!(b, Bridge::Command(_));
    let made = clients
        .create(&plan.client, command)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let outcome = match plan.argv(&made.token) {
        Some(argv) => port(&deps)?.run(&argv).await.map(|()| None),
        None => write_link(&w, &plan, &b, &made.token, &port(&deps)?.stamp()),
    };
    let backup = match outcome {
        Ok(b) => b,
        Err(why) => {
            let _ = clients.remove(&plan.client);
            return Err(ApiError::unprocessable(format!(
                "{} could not be set up to use Ghost: {why}",
                tool.title()
            )));
        }
    };
    tracing::info!(
        "other tools: {} uses Ghost as {}",
        tool.title(),
        plan.client
    );
    let view_now = clients_view(&deps)?;
    let client = view_now.clients.into_iter().find(|c| c.name == plan.client);
    Ok(Json(OtherLinkResult {
        done: format!(
            "{} can use Ghost now; restart it if it is open.",
            tool.title()
        ),
        client,
        backup: backup.map(|p| shown_path(&w.home, &p)),
        tools: view(&deps).await?,
    }))
}

/// Take Ghost back out of another tool. The client stays until it is removed.
#[utoipa::path(delete, path = "/other-tools/{tool}/link", operation_id = "unlink_other_tool_other_tools__tool__link_delete",
    tag = "other-tools", summary = "Unlink Other Tool",
    description = "Take Ghost back out of another tool's own settings, by its command or from its\nfile after a backup. The client itself is removed on its own (`DELETE /clients/{name}`).",
    params(("tool" = String, Path)),
    responses((status = 200, description = "Successful Response", body = OtherLinkResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn unlink_tool(
    State(deps): State<SharedDeps>,
    PathArg(tool): PathArg<String>,
) -> ApiResult<Json<OtherLinkResult>> {
    let tool = tool_of(&tool)?;
    let (w, found) = found_now(&deps)?;
    let t = found.iter().find(|t| t.tool == tool).unwrap();
    let Some(l) = &t.linked else {
        return Err(ApiError::not_found(format!(
            "{} does not use Ghost.",
            tool.title()
        )));
    };
    let user_file = shown_path(&w.home, &w.user_file(tool));
    if l.file != user_file {
        return Err(ApiError::conflict(format!(
            "Ghost is in {}, which Ghost did not write; take it out there.",
            l.file
        )));
    }
    let backup = match unlink_argv(tool, t.binary.as_deref(), &l.entry) {
        Some(argv) => port(&deps)?.run(&argv).await.map(|()| None),
        None => remove_link(&w, tool, &l.entry, &port(&deps)?.stamp()),
    }
    .map_err(|why| {
        ApiError::unprocessable(format!(
            "Ghost could not be taken out of {}: {why}",
            tool.title()
        ))
    })?;
    Ok(Json(OtherLinkResult {
        done: format!("{} no longer uses Ghost.", tool.title()),
        client: None,
        backup: backup.map(|p| shown_path(&w.home, &p)),
        tools: view(&deps).await?,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_other_tools))
        .routes(routes!(import_apps))
        .routes(routes!(link_tool, unlink_tool))
}
