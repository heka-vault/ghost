//! Outside tools (ADR 0048): the tools on this computer the operator let use Ghost, as
//! Settings sees them.
//!
//! `GET /clients` lists them with when each was last used; `POST /clients` makes one and
//! answers with its token and what to paste into the other tool, once; `DELETE` takes one
//! away, and its token stops working at once. The token is a password in Ghost's sense: no
//! other route returns it, it is not logged, and only its hash is kept. The model has no
//! route here: no tool of Ghost's own app makes, lists or removes a client.

use axum::Json;
use axum::extract::{Path as PathArg, State};
use chrono::{DateTime, Utc};
use ghost_core::ghost_app::clients::{
    APPIMAGE_NOTE, ClientInfo, Clients, ClientsError, http_snippets, shown, stable_ghostd,
    stdio_snippets,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};

/// A tool the operator connected to Ghost. Never its token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ClientView {
    /// What it was connected as: lowercase letters, digits and hyphens.
    pub name: String,
    /// The name as Ghost says it: "Claude Code" for `claude-code`.
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// The last time it called Ghost; None when it never has.
    #[schema(required)]
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ClientsView {
    pub clients: Vec<ClientView>,
    /// Where Ghost listens for them (`host:port`); empty when that is turned off.
    pub listening: Vec<String>,
    /// Why Ghost is not listening though it should be, in a sentence.
    #[schema(required)]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ClientCreate {
    pub name: String,
    /// For a tool that can only start a command: Ghost keeps the token in a private file
    /// and the tool runs `ghostd mcp serve`.
    #[serde(default)]
    pub command: bool,
}

/// What `POST /clients` answers, once: the token is never returned again.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClientCreated {
    pub client: ClientView,
    /// The client's token. Shown once.
    #[schema(value_type = String, format = Password, read_only)]
    pub token: String,
    /// The command that connects Claude Code.
    pub claude_code: String,
    /// The config other tools read, as JSON text.
    pub config_json: String,
    /// Why the snippets are not what was asked for, in a sentence: a command was asked for
    /// and Ghost runs from an AppImage, so they connect by address.
    #[serde(default)]
    #[schema(required)]
    pub note: Option<String>,
    pub clients: ClientsView,
}

fn view(c: ClientInfo) -> ClientView {
    ClientView {
        title: shown(&c.name),
        name: c.name,
        created_at: c.created_at,
        last_used_at: c.last_used_at,
    }
}

fn clients(deps: &Deps) -> ApiResult<&Clients> {
    deps.clients
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("outside tools are not available"))
}

fn failed(e: ClientsError) -> ApiError {
    match e {
        ClientsError::Name(m) => ApiError::invalid(&["body", "name"], m, "value_error"),
        ClientsError::Taken(_) => ApiError::conflict(e.to_string()),
        ClientsError::NotFound(_) => ApiError::not_found(e.to_string()),
        ClientsError::Store(_) | ClientsError::Io(_) => ApiError::internal(e.to_string()),
    }
}

pub fn clients_view(deps: &Deps) -> ApiResult<ClientsView> {
    Ok(ClientsView {
        clients: clients(deps)?
            .list()
            .map_err(failed)?
            .into_iter()
            .map(view)
            .collect(),
        listening: deps.clients_listening.clone(),
        problem: deps.control.outside_problem(),
    })
}

#[utoipa::path(get, path = "/clients", operation_id = "get_clients_clients_get", tag = "clients",
    summary = "Get Clients", description = "The tools the operator connected to Ghost, and where Ghost listens for them. Never a token.",
    responses((status = 200, description = "Successful Response", body = ClientsView)))]
async fn get_clients(State(deps): State<SharedDeps>) -> ApiResult<Json<ClientsView>> {
    Ok(Json(clients_view(&deps)?))
}

#[utoipa::path(post, path = "/clients", operation_id = "create_client_clients_post", tag = "clients",
    summary = "Create Client", description = "Connect a tool: its token and what to paste into it, returned once and never again.",
    request_body = ClientCreate,
    responses((status = 200, description = "Successful Response", body = ClientCreated),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn create_client(
    State(deps): State<SharedDeps>,
    Body(body): Body<ClientCreate>,
) -> ApiResult<Json<ClientCreated>> {
    // This process is ghostd: the command the other tool runs is this binary, unless it
    // runs from an AppImage, whose mount goes away; then the tool connects by address.
    let ghostd = if deps.in_appimage {
        None
    } else {
        stable_ghostd(|_| None)
    };
    let command = body.command && ghostd.is_some();
    let note = (body.command && !command).then(|| APPIMAGE_NOTE.to_string());
    let made = clients(&deps)?
        .create(&body.name, command)
        .map_err(failed)?;
    let snippets = if let (true, Some(ghostd)) = (command, ghostd.as_deref()) {
        stdio_snippets(&made.client.name, ghostd)
    } else {
        let address = deps.clients_listening.first().map_or(
            ghost_core::ghost_app::clients::DEFAULT_LISTEN,
            String::as_str,
        );
        http_snippets(&made.token, address)
    };
    Ok(Json(ClientCreated {
        client: view(made.client),
        token: made.token.to_string(),
        claude_code: snippets.claude_code,
        config_json: snippets.config_json,
        note,
        clients: clients_view(&deps)?,
    }))
}

#[utoipa::path(delete, path = "/clients/{name}", operation_id = "delete_client_clients__name__delete",
    tag = "clients", summary = "Delete Client", description = "Take a tool away: its token stops working at once.",
    params(("name" = String, Path)),
    responses((status = 200, description = "Successful Response", body = ClientsView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn delete_client(
    State(deps): State<SharedDeps>,
    PathArg(name): PathArg<String>,
) -> ApiResult<Json<ClientsView>> {
    clients(&deps)?.remove(&name).map_err(failed)?;
    Ok(Json(clients_view(&deps)?))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_clients, create_client))
        .routes(routes!(delete_client))
}
