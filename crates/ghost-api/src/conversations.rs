//! Agent routes (ADR 0014, ADR 0035), still `/conversations` on the wire. Edits made here
//! are the operator's own, applied through the gate's internal path so they land in the
//! same files Ghost writes, with the same snapshot and audit row. Switching and compacting
//! need a running session and are delegated to the daemon.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as PathArg, State};
use ghost_core::conversations::models::{CompactionSource, Profile, Role, slugify};
use ghost_core::conversations::{Conversations, SOURCE};
use ghost_core::models::{
    AppendNote, Change, ChangeSet, ConversationTarget, Create, Rename, SetMemory, SetNotes,
    SetPersona, SetVoice, Target, py_repr,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::{Qs, apply_internal};
use crate::deps::{Deps, SharedDeps};
use crate::error::{
    ApiError, ApiResult, Body, HTTPValidationError, OptionalBody, check_len, check_range,
};
use crate::models::{
    Ack, CompactionView, ConversationCreate, ConversationDetail, ConversationUpdate,
    ConversationView, SampleRequest, TurnView, TurnViewRole, VoiceInfo,
};

fn lang(c: char) -> Option<&'static str> {
    Some(match c {
        'a' => "American",
        'b' => "British",
        'e' => "Spanish",
        'f' => "French",
        'h' => "Hindi",
        'i' => "Italian",
        'j' => "Japanese",
        'p' => "Portuguese",
        'z' => "Chinese",
        _ => return None,
    })
}

fn sex(c: char) -> &'static str {
    match c {
        'f' => "female",
        'm' => "male",
        _ => "",
    }
}

pub fn voice_label(voice_id: &str) -> String {
    let (head, name) = voice_id.split_once('_').unwrap_or((voice_id, ""));
    let mut chars = head.chars();
    let language = match chars.next() {
        Some(c) => lang(c).map_or_else(|| c.to_string(), String::from),
        None => String::new(),
    };
    let s = chars.next().map_or("", sex);
    let mut name_chars = name.chars();
    let capitalized = match name_chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &name_chars.as_str().to_lowercase(),
        None => String::new(),
    };
    format!("{language} {s}: {capitalized}").trim().to_string()
}

/// The control API's view of an agent; the phone listener serves the same one.
pub fn conversation_view(
    convs: &Conversations,
    p: &Profile,
    active: Option<&str>,
) -> ApiResult<ConversationView> {
    let (voice, speed) = convs
        .voice_for(p)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(ConversationView {
        id: p.id.clone(),
        slug: p.slug.clone(),
        name: p.name.clone(),
        aliases: p.aliases.clone(),
        job: p.job.clone(),
        voice,
        speed,
        own_voice: p.voice.is_some(),
        created_at: p.created_at,
        last_active_at: p.last_active_at,
        retired_at: p.retired_at,
        active: active == Some(p.slug.as_str()),
        is_ghost: p.is_ghost(),
    })
}

/// The full view of an agent: the profile plus its files and counts.
pub fn conversation_detail(
    convs: &Conversations,
    p: &Profile,
    active: Option<&str>,
) -> ApiResult<ConversationDetail> {
    let io = |e: std::io::Error| ApiError::internal(e.to_string());
    let core = |e: ghost_core::conversations::service::ConversationError| {
        ApiError::internal(e.to_string())
    };
    Ok(ConversationDetail {
        view: conversation_view(convs, p, active)?,
        persona: convs.files.persona(&p.slug).map_err(io)?,
        memory: convs.files.memory(&p.slug).map_err(io)?,
        notes: convs.files.note_lines(&p.slug).map_err(io)?,
        turn_count: convs.turns(&p.slug, 100_000).map_err(core)?.len() as i64,
        compactions: convs.compactions(&p.slug).map_err(core)?.len() as i64,
        dir: convs.files.dir(&p.slug).display().to_string(),
    })
}

fn need(deps: &Deps) -> ApiResult<&Arc<Conversations>> {
    deps.conversations
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("agents are not available"))
}

fn get(convs: &Conversations, slug: &str) -> ApiResult<Profile> {
    convs
        .get(slug)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("no agent {}", py_repr(slug))))
}

fn seed_memory(convs: &Conversations, seed: Option<&str>) -> ApiResult<String> {
    match seed {
        None | Some("") => Ok(String::new()),
        Some(seed) => convs
            .handoff_memory(seed)
            .map_err(|e| ApiError::unprocessable(e.to_string())),
    }
}

async fn apply(
    deps: &Deps,
    slug: &str,
    op: &str,
    change: Change,
    rationale: Option<&str>,
) -> ApiResult<()> {
    let rationale = rationale.map_or_else(|| format!("{op} from the desktop app"), String::from);
    apply_internal(
        &deps.gate,
        SOURCE,
        op,
        Target::Conversation(ConversationTarget {
            slug: slug.to_string(),
        }),
        ChangeSet::one(change),
        &rationale,
    )
    .await?;
    let active = deps.state.conversation();
    deps.state.set_conversation(active.as_deref(), Some(slug));
    Ok(())
}

fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn aliases_of(aliases: &[String]) -> Vec<String> {
    aliases
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect()
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub retired: Option<bool>,
}

/// Every agent, Ghost first, retired ones included (`retired_at` set) so the app can show
/// its Retired group. `retired=true` or `false` filters.
#[utoipa::path(get, path = "/conversations", operation_id = "list_conversations_conversations_get",
    tag = "conversations", summary = "List Conversations",
    description = "Every agent, Ghost first, retired ones included (``retired_at`` set) so the app\ncan show its Retired group. ``retired=true`` or ``false`` filters.",
    params(ListQuery),
    responses((status = 200, description = "Successful Response", body = Vec<ConversationView>),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn list_conversations(
    State(deps): State<SharedDeps>,
    Qs(q): Qs<ListQuery>,
) -> ApiResult<Json<Vec<ConversationView>>> {
    let convs = need(&deps)?;
    let active = deps.state.conversation();
    let mut out = Vec::new();
    for p in convs
        .list(true)
        .map_err(|e| ApiError::internal(e.to_string()))?
    {
        out.push(conversation_view(convs, &p, active.as_deref())?);
    }
    if let Some(retired) = q.retired {
        out.retain(|c| c.retired_at.is_some() == retired);
    }
    Ok(Json(out))
}

/// Hire an agent.
#[utoipa::path(post, path = "/conversations", operation_id = "create_conversation_conversations_post",
    tag = "conversations", summary = "Create Conversation", description = "Hire an agent.",
    request_body = ConversationCreate,
    responses((status = 200, description = "Successful Response", body = ConversationDetail),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn create_conversation(
    State(deps): State<SharedDeps>,
    Body(req): Body<ConversationCreate>,
) -> ApiResult<Json<ConversationDetail>> {
    check_len(&["body", "name"], &req.name, 1, 80)?;
    check_len(&["body", "job"], &req.job, 0, 200)?;
    check_len(&["body", "persona"], &req.persona, 0, 4000)?;
    if let Some(speed) = req.speed {
        check_range(&["body", "speed"], speed, 0.5, 2.0)?;
    }
    let convs = need(&deps)?;
    let slug = slugify(&req.name);
    let voices = convs.voices();
    if let Some(voice) = req.voice.as_deref().filter(|v| !v.is_empty())
        && !voices.is_empty()
        && !voices.iter().any(|v| v == voice)
    {
        return Err(ApiError::unprocessable(format!(
            "{} is not an installed voice",
            py_repr(voice)
        )));
    }
    let memory = seed_memory(convs, req.seed_from.as_deref())?;
    apply(
        &deps,
        &slug,
        "create",
        Change::Create(Create {
            name: req.name.trim().to_string(),
            aliases: aliases_of(&req.aliases),
            job: squeeze(&req.job),
            voice: req.voice.filter(|v| !v.is_empty()),
            speed: req.speed,
            persona: req.persona,
            memory,
        }),
        None,
    )
    .await?;
    let active = deps.state.conversation();
    Ok(Json(conversation_detail(
        convs,
        &get(convs, &slug)?,
        active.as_deref(),
    )?))
}

#[utoipa::path(get, path = "/conversations/{slug}", operation_id = "get_conversation_conversations__slug__get",
    tag = "conversations", summary = "Get Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = ConversationDetail),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn get_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<ConversationDetail>> {
    let convs = need(&deps)?;
    let active = deps.state.conversation();
    Ok(Json(conversation_detail(
        convs,
        &get(convs, &slug)?,
        active.as_deref(),
    )?))
}

#[utoipa::path(patch, path = "/conversations/{slug}", operation_id = "update_conversation_conversations__slug__patch",
    tag = "conversations", summary = "Update Conversation", params(("slug" = String, Path)),
    request_body = ConversationUpdate,
    responses((status = 200, description = "Successful Response", body = ConversationDetail),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn update_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
    Body(req): Body<ConversationUpdate>,
) -> ApiResult<Json<ConversationDetail>> {
    if let Some(name) = &req.name {
        check_len(&["body", "name"], name, 1, 80)?;
    }
    if let Some(job) = &req.job {
        check_len(&["body", "job"], job, 0, 200)?;
    }
    if let Some(speed) = req.speed {
        check_range(&["body", "speed"], speed, 0.5, 2.0)?;
    }
    if let Some(persona) = &req.persona {
        check_len(&["body", "persona"], persona, 0, 4000)?;
    }
    if let Some(memory) = &req.memory {
        check_len(&["body", "memory"], memory, 0, 20000)?;
    }
    let convs = need(&deps)?;
    let p = get(convs, &slug)?;
    if req.name.is_some() || req.aliases.is_some() || req.job.is_some() {
        apply(
            &deps,
            &slug,
            "rename",
            Change::Rename(Rename {
                name: req.name.as_deref().unwrap_or(&p.name).trim().to_string(),
                aliases: aliases_of(req.aliases.as_deref().unwrap_or(&p.aliases)),
                job: req.job.as_deref().map(squeeze),
            }),
            None,
        )
        .await?;
    }
    if req.voice.is_some() || req.speed.is_some() {
        let voice = req.voice.as_deref().filter(|v| !v.is_empty()); // "" means Ghost's
        let voices = convs.voices();
        if let Some(v) = voice
            && !voices.is_empty()
            && !voices.iter().any(|x| x == v)
        {
            return Err(ApiError::unprocessable(format!(
                "{} is not an installed voice",
                py_repr(v)
            )));
        }
        apply(
            &deps,
            &slug,
            "set_voice",
            Change::SetVoice(SetVoice {
                voice: if req.voice.is_some() {
                    voice.map(String::from)
                } else {
                    p.voice.clone()
                },
                speed: req.speed.or(p.speed),
            }),
            None,
        )
        .await?;
    }
    if let Some(text) = req.persona {
        apply(
            &deps,
            &slug,
            "set_persona",
            Change::SetPersona(SetPersona { text }),
            None,
        )
        .await?;
    }
    if let Some(seed) = &req.seed_from {
        let text = seed_memory(convs, Some(seed))?;
        apply(
            &deps,
            &slug,
            "compact",
            Change::SetMemory(SetMemory {
                text,
                through_turn_id: None,
                source: CompactionSource::Edit,
            }),
            Some(&format!("memory seeded from {seed} in the desktop app")),
        )
        .await?;
    }
    if let Some(text) = req.memory {
        apply(
            &deps,
            &slug,
            "compact",
            Change::SetMemory(SetMemory {
                text,
                through_turn_id: None,
                source: CompactionSource::Edit,
            }),
            Some("memory edited in the desktop app"),
        )
        .await?;
    }
    if let Some(notes) = req.notes {
        let text = notes
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        apply(
            &deps,
            &slug,
            "forget",
            Change::SetNotes(SetNotes { text }),
            Some("notes edited in the desktop app"),
        )
        .await?;
    }
    let active = deps.state.conversation();
    Ok(Json(conversation_detail(
        convs,
        &get(convs, &slug)?,
        active.as_deref(),
    )?))
}

#[utoipa::path(post, path = "/conversations/{slug}/notes", operation_id = "add_note_conversations__slug__notes_post",
    tag = "conversations", summary = "Add Note", params(("slug" = String, Path)),
    request_body = inline(crate::openapi::NoteBody),
    responses((status = 200, description = "Successful Response", body = ConversationDetail),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn add_note(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
    Body(body): Body<Map<String, Value>>,
) -> ApiResult<Json<ConversationDetail>> {
    let convs = need(&deps)?;
    get(convs, &slug)?;
    let text = squeeze(&match body.get("text") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => ghost_core::models::py_str(other),
    });
    if text.is_empty() {
        return Err(ApiError::unprocessable("text is required"));
    }
    apply(
        &deps,
        &slug,
        "remember",
        Change::AppendNote(AppendNote { text }),
        None,
    )
    .await?;
    let active = deps.state.conversation();
    Ok(Json(conversation_detail(
        convs,
        &get(convs, &slug)?,
        active.as_deref(),
    )?))
}

#[utoipa::path(delete, path = "/conversations/{slug}", operation_id = "delete_conversation_conversations__slug__delete",
    tag = "conversations", summary = "Delete Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn delete_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    let p = get(convs, &slug)?;
    if p.is_ghost() {
        return Err(ApiError::conflict("Ghost stays"));
    }
    if deps.state.conversation().as_deref() == Some(slug.as_str()) {
        return Err(ApiError::conflict(
            "the session is with this agent; switch first",
        ));
    }
    apply(&deps, &slug, "delete", Change::Delete, None).await?;
    Ok(Json(Ack::ok()))
}

#[utoipa::path(post, path = "/conversations/{slug}/reset", operation_id = "reset_conversation_conversations__slug__reset_post",
    tag = "conversations", summary = "Reset Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn reset_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    get(convs, &slug)?;
    if deps.state.conversation().as_deref() == Some(slug.as_str()) {
        return Err(ApiError::conflict(
            "the session is with this agent; switch or sleep first",
        ));
    }
    apply(&deps, &slug, "reset", Change::Reset, None).await?;
    Ok(Json(Ack::ok()))
}

/// Retire the agent: off the roster and voice routing, everything kept.
#[utoipa::path(post, path = "/conversations/{slug}/retire", operation_id = "retire_conversation_conversations__slug__retire_post",
    tag = "conversations", summary = "Retire Conversation",
    description = "Retire the agent: off the roster and voice routing, everything kept.",
    params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn retire_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    let p = get(convs, &slug)?;
    if p.is_ghost() {
        return Err(ApiError::conflict("Ghost stays"));
    }
    if deps.state.conversation().as_deref() == Some(slug.as_str()) {
        return Err(ApiError::conflict(
            "the session is with this agent; switch first",
        ));
    }
    if p.retired() {
        return Ok(Json(Ack {
            ok: true,
            detail: Some("already retired".into()),
        }));
    }
    apply(&deps, &slug, "retire", Change::Retire, None).await?;
    Ok(Json(Ack::ok()))
}

#[utoipa::path(post, path = "/conversations/{slug}/rehire", operation_id = "rehire_conversation_conversations__slug__rehire_post",
    tag = "conversations", summary = "Rehire Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn rehire_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    let p = get(convs, &slug)?;
    if !p.retired() {
        return Ok(Json(Ack {
            ok: true,
            detail: Some("not retired".into()),
        }));
    }
    apply(&deps, &slug, "rehire", Change::Rehire, None).await?;
    Ok(Json(Ack::ok()))
}

#[utoipa::path(post, path = "/conversations/{slug}/switch", operation_id = "switch_conversation_conversations__slug__switch_post",
    tag = "conversations", summary = "Switch Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn switch_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    let p = get(convs, &slug)?;
    if p.retired() {
        return Err(ApiError::conflict("that agent is retired; rehire it first"));
    }
    if deps.state.session_id().is_none() {
        return Err(ApiError::conflict("no session; wake Ghost first"));
    }
    let ok = deps.control.switch_conversation(&slug).await;
    Ok(Json(if ok {
        Ack::ok()
    } else {
        Ack::no("could not switch")
    }))
}

#[utoipa::path(post, path = "/conversations/{slug}/compact", operation_id = "compact_conversation_conversations__slug__compact_post",
    tag = "conversations", summary = "Compact Conversation", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn compact_conversation(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Ack>> {
    let convs = need(&deps)?;
    get(convs, &slug)?;
    if deps.state.session_id().is_none()
        || deps.state.conversation().as_deref() != Some(slug.as_str())
    {
        return Err(ApiError::conflict(
            "compacting needs a session with this agent",
        ));
    }
    let ok = deps.control.compact_conversation(&slug).await;
    Ok(Json(Ack { ok, detail: None }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TurnsQuery {
    #[param(default = 200)]
    pub limit: Option<i64>,
}

#[utoipa::path(get, path = "/conversations/{slug}/turns", operation_id = "conversation_turns_conversations__slug__turns_get",
    tag = "conversations", summary = "Conversation Turns", params(("slug" = String, Path), TurnsQuery),
    responses((status = 200, description = "Successful Response", body = Vec<TurnView>),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn conversation_turns(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
    Qs(q): Qs<TurnsQuery>,
) -> ApiResult<Json<Vec<TurnView>>> {
    let convs = need(&deps)?;
    get(convs, &slug)?;
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let turns = convs
        .turns(&slug, limit)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        turns
            .into_iter()
            .map(|t| TurnView {
                id: t.id,
                session_id: t.session_id,
                role: match t.role {
                    Role::User => TurnViewRole::User,
                    Role::Assistant => TurnViewRole::Assistant,
                    Role::Call => TurnViewRole::Call,
                    Role::System => TurnViewRole::System,
                },
                text: t.text,
                at: t.at,
                compacted: t.compaction_id.is_some(),
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/conversations/{slug}/compactions",
    operation_id = "conversation_compactions_conversations__slug__compactions_get",
    tag = "conversations", summary = "Conversation Compactions", params(("slug" = String, Path)),
    responses((status = 200, description = "Successful Response", body = Vec<CompactionView>),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn conversation_compactions(
    State(deps): State<SharedDeps>,
    PathArg(slug): PathArg<String>,
) -> ApiResult<Json<Vec<CompactionView>>> {
    let convs = need(&deps)?;
    get(convs, &slug)?;
    let rows = convs
        .compactions(&slug)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .map(|c| CompactionView {
                id: c.id,
                at: c.at,
                source: c.source.as_str().to_string(),
                summary: c.summary,
                through_turn_id: c.through_turn_id,
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/voices", operation_id = "voices_voices_get", tag = "conversations", summary = "Voices",
    responses((status = 200, description = "Successful Response", body = Vec<VoiceInfo>)))]
async fn voices(State(deps): State<SharedDeps>) -> Json<Vec<VoiceInfo>> {
    Json(
        deps.control
            .voices()
            .into_iter()
            .map(|v| VoiceInfo {
                label: voice_label(&v),
                id: v,
            })
            .collect(),
    )
}

#[utoipa::path(post, path = "/voices/{voice}/sample", operation_id = "sample_voice_voices__voice__sample_post",
    tag = "conversations", summary = "Sample Voice", params(("voice" = String, Path)),
    request_body = Option<SampleRequest>,
    responses((status = 200, description = "Successful Response", body = Ack),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn sample_voice(
    State(deps): State<SharedDeps>,
    PathArg(voice): PathArg<String>,
    OptionalBody(req): OptionalBody<SampleRequest>,
) -> ApiResult<Json<Ack>> {
    let req = req.unwrap_or(SampleRequest {
        speed: 1.0,
        text: None,
    });
    check_range(&["body", "speed"], req.speed, 0.5, 2.0)?;
    if let Some(text) = &req.text {
        check_len(&["body", "text"], text, 0, 200)?;
    }
    let known = deps.control.voices();
    if !known.is_empty() && !known.contains(&voice) {
        return Err(ApiError::not_found(format!("no voice {}", py_repr(&voice))));
    }
    let ok = deps.control.sample_voice(&voice, req.speed, req.text).await;
    Ok(Json(if ok {
        Ack::ok()
    } else {
        Ack::no("could not play a sample now")
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(list_conversations, create_conversation))
        .routes(routes!(
            get_conversation,
            update_conversation,
            delete_conversation
        ))
        .routes(routes!(add_note))
        .routes(routes!(reset_conversation))
        .routes(routes!(retire_conversation))
        .routes(routes!(rehire_conversation))
        .routes(routes!(switch_conversation))
        .routes(routes!(compact_conversation))
        .routes(routes!(conversation_turns))
        .routes(routes!(conversation_compactions))
        .routes(routes!(voices))
        .routes(routes!(sample_voice))
}
