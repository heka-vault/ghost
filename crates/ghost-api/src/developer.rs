//! What Settings › Developer reads and writes (ADR 0072, ADR 0073): the experimental switches
//! (a closed list of settings that are environment variables, written to `settings.env` by the
//! same writer as every other setting), debug mode (`GHOST_DEBUG_AUDIO` for an hour, held in
//! the daemon's memory only), and the diagnostic report (one file the person saves and hands
//! to a developer; nothing is uploaded).
//!
//! The operator's only: the phone listener, the listener for outside tools and Ghost's own
//! app serve none of this, and no tool, model, app, run or outside client can read a switch,
//! turn debug mode on or start or read a report (`no_tool_reaches_debug_mode_or_the_report`
//! in ghostd, and the phone's and the outside listener's closed-door tests).
//!
//! Developer is not a security boundary: everything here the operator can already do with a
//! file (`settings.env`) or a terminal (`ghostd debug-audio`, `ghostd diagnose`).

use std::collections::BTreeMap;

use async_trait::async_trait;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::deps::SharedDeps;
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError};

// --- the experimental switches ---------------------------------------------------------------

/// What a switch's value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SwitchKind {
    /// `on` or `off` (an unset value is the default).
    Flag,
    /// One of `choices`.
    Choice,
    /// A number of seconds or a count, at least 0.
    Number,
}

/// One experimental switch: a setting the daemon reads from its environment. A closed list
/// in code ([`SWITCHES`]); `every_switch_names_a_variable_the_daemon_reads` holds it to the
/// daemon's own readers.
#[derive(Debug, Clone, Copy)]
pub struct Switch {
    pub name: &'static str,
    pub title: &'static str,
    /// What it does, in one sentence.
    pub sentence: &'static str,
    pub kind: SwitchKind,
    /// The value the daemon uses when nothing sets it, as it would be written.
    pub default: &'static str,
    pub choices: &'static [&'static str],
    /// The largest number taken (Number only).
    pub max: f64,
}

const fn flag(name: &'static str, title: &'static str, sentence: &'static str, on: bool) -> Switch {
    Switch {
        name,
        title,
        sentence,
        kind: SwitchKind::Flag,
        default: if on { "on" } else { "off" },
        choices: &[],
        max: 0.0,
    }
}

const fn number(
    name: &'static str,
    title: &'static str,
    sentence: &'static str,
    default: &'static str,
    max: f64,
) -> Switch {
    Switch {
        name,
        title,
        sentence,
        kind: SwitchKind::Number,
        default,
        choices: &[],
        max,
    }
}

const fn choice(
    name: &'static str,
    title: &'static str,
    sentence: &'static str,
    default: &'static str,
    choices: &'static [&'static str],
) -> Switch {
    Switch {
        name,
        title,
        sentence,
        kind: SwitchKind::Choice,
        default,
        choices,
        max: 0.0,
    }
}

/// The experimental switches, in the page's order. Never a password, never
/// `GHOST_DEBUG_AUDIO` (debug mode is its own switch, ADR 0073), never the voice mode (ADR
/// 0063: chosen on Settings › Sound after what changes is shown), never a listener's
/// addresses or a path. The fast model's thinking and effort (`GHOST_LLM_FAST_*`) are left
/// out: the values a route takes are the provider's, not a closed set. Every one is read by the voice settings at Ghost's start, so every one
/// takes effect after a restart; the route says which a running Ghost took at once.
pub const SWITCHES: &[Switch] = &[
    choice(
        "GHOST_TTS_STREAM",
        "Windowed voice",
        "Kokoro renders a piece whole, or a window of frames at a time and plays each as it is made (ADR 0053).",
        "whole",
        &["whole", "windowed"],
    ),
    choice(
        "GHOST_STT_EARLY_READ",
        "Speculative answers",
        "Reads a segment's words at its first quiet; speculate starts the held answer from them while the stop's read still makes the transcript (ADR 0054).",
        "off",
        &["off", "verify", "speculate"],
    ),
    flag(
        "GHOST_LLM_RACE",
        "Race the main model",
        "With a fast model set, asks the main model beside every fast call and uses its reply when the fast model hands over (ADR 0062).",
        true,
    ),
    number(
        "GHOST_FIRST_SOUND_SECS",
        "Wait before “One sec.”",
        "Seconds after a turn's reply begins before Ghost says “One sec.” when the model has written nothing; unset chooses it from the connection's measured first word, 0 never says it.",
        "",
        10.0,
    ),
    number(
        "GHOST_TURN_GRACE_SECS",
        "Turn grace",
        "Seconds Ghost waits after you stop before it takes your turn as finished.",
        "1.2",
        5.0,
    ),
    number(
        "GHOST_TURN_CONFIDENT_GRACE_SECS",
        "Confident turn grace",
        "The shorter wait when the turn model is sure you have finished.",
        "0.35",
        5.0,
    ),
    number(
        "GHOST_TURN_STOP_SECS",
        "Turn stop",
        "Seconds of quiet that end a turn whatever the turn model says.",
        "1.6",
        10.0,
    ),
    number(
        "GHOST_TURN_UNFINISHED_SECS",
        "Unfinished turn",
        "How long Ghost waits on a sentence that sounds unfinished.",
        "3.0",
        10.0,
    ),
    flag(
        "GHOST_TURN_SPECULATE",
        "Start the answer early",
        "Asks the model while the turn's close is still being decided, and keeps the answer only if the turn did end.",
        true,
    ),
    flag(
        "GHOST_TURN_REREAD",
        "Read the whole turn again",
        "Reads the whole turn once more at its close for the transcript the model gets.",
        true,
    ),
    number(
        "GHOST_BARGE_IN_MIN_WORDS",
        "Words to cut Ghost off",
        "How many words of yours stop Ghost while it speaks.",
        "2",
        10.0,
    ),
    number(
        "GHOST_BARGE_IN_INTERIM_SECS",
        "Barge-in read",
        "Seconds of your speech over Ghost before it is read for those words.",
        "0.3",
        3.0,
    ),
    number(
        "GHOST_VAD_STOP_SECS",
        "Speech stop",
        "Seconds of quiet that end a segment of speech.",
        "0.2",
        3.0,
    ),
    flag(
        "GHOST_ACK",
        "Acknowledgments",
        "Ghost says a short acknowledgment before a slow call.",
        true,
    ),
    flag(
        "GHOST_QUICK_INTENTS",
        "Quick intents",
        "Fixed phrases (“Ghost, stop listening”, “hang on, Ghost”) are taken without asking the model.",
        true,
    ),
    flag(
        "GHOST_THINKING_SOUND",
        "Thinking sound",
        "A soft pulse while the model works; off since it leaked through the echo canceller.",
        false,
    ),
    flag(
        "GHOST_LATENCY",
        "Timing records",
        "Ghost keeps each turn's timings (numbers only) in latency.jsonl, for ghostd latency and the diagnostic report.",
        true,
    ),
];

/// The switch named `name`, if it is one.
pub fn switch(name: &str) -> Option<&'static Switch> {
    SWITCHES.iter().find(|s| s.name == name)
}

/// A value for `s` as it would be written, or why it isn't one. Empty clears it.
pub fn check_value(s: &Switch, value: &str) -> Result<Option<String>, String> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(None);
    }
    match s.kind {
        SwitchKind::Flag => match v.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => Ok(Some("on".into())),
            "off" | "false" | "0" | "no" => Ok(Some("off".into())),
            _ => Err(format!("{} is on or off", s.name)),
        },
        SwitchKind::Choice => {
            let v = v.to_ascii_lowercase();
            if s.choices.contains(&v.as_str()) {
                Ok(Some(v))
            } else {
                Err(format!("{} is one of {}", s.name, s.choices.join(", ")))
            }
        }
        SwitchKind::Number => match v.parse::<f64>() {
            Ok(n) if n.is_finite() && (0.0..=s.max).contains(&n) => Ok(Some(v.to_string())),
            _ => Err(format!("{} is a number from 0 to {}", s.name, s.max)),
        },
    }
}

/// Where the value Ghost reads comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SwitchSource {
    /// `settings.env`.
    File,
    /// The environment Ghost was started in, not the file (an export, a unit, a launcher).
    Environment,
    /// Nothing sets it: Ghost's own default.
    Default,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SwitchView {
    /// The variable's real name.
    pub name: String,
    pub title: String,
    pub sentence: String,
    #[schema(inline)]
    pub kind: SwitchKind,
    /// The value Ghost reads now ("" when nothing sets it).
    pub value: String,
    /// What `settings.env` holds for it, if anything.
    #[schema(required)]
    pub file_value: Option<String>,
    #[schema(inline)]
    pub source: SwitchSource,
    /// Ghost's own default ("" for one it works out itself).
    pub default: String,
    pub choices: Vec<String>,
    #[schema(required)]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SwitchesView {
    pub switches: Vec<SwitchView>,
    /// `settings.env`, home shown as `~`.
    pub settings_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SwitchesUpdate {
    /// Name to value; "" or null takes it out of `settings.env` (Ghost's default again).
    pub values: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SwitchesResult {
    pub switches: SwitchesView,
    /// The names a running Ghost took at once.
    pub applied: Vec<String>,
    /// Something saved waits for Ghost's restart.
    pub restart_required: bool,
}

fn tilde(path: &std::path::Path, env: &std::collections::HashMap<String, String>) -> String {
    let p = path.display().to_string();
    match env.get("HOME").filter(|h| h.len() > 1) {
        Some(h) if p.starts_with(h.as_str()) => format!("~{}", &p[h.len()..]),
        _ => p,
    }
}

fn switches_view(deps: &crate::deps::Deps) -> SwitchesView {
    let file = deps.settings();
    let live = deps.environ();
    let env = deps.ghost_env();
    let switches = SWITCHES
        .iter()
        .map(|s| {
            let now = env
                .get(s.name)
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            let file_value = file.get(s.name).cloned();
            let source = if now.is_empty() {
                SwitchSource::Default
            } else if ghost_core::environment::from_start(s.name, &deps.started_env, &live, &file) {
                SwitchSource::Environment
            } else {
                SwitchSource::File
            };
            SwitchView {
                name: s.name.into(),
                title: s.title.into(),
                sentence: s.sentence.into(),
                kind: s.kind,
                value: now,
                file_value,
                source,
                default: s.default.into(),
                choices: s.choices.iter().map(|c| (*c).to_string()).collect(),
                max: (s.kind == SwitchKind::Number).then_some(s.max),
            }
        })
        .collect();
    SwitchesView {
        switches,
        settings_path: tilde(&deps.settings_path, &deps.started_env),
    }
}

/// The experimental switches: each one's value now and where it comes from.
#[utoipa::path(get, path = "/developer/switches", operation_id = "developer_switches_get",
    tag = "developer", summary = "The experimental switches",
    responses((status = 200, description = "Successful Response", body = SwitchesView)))]
async fn get_switches(State(deps): State<SharedDeps>) -> Json<SwitchesView> {
    Json(switches_view(&deps))
}

/// Write experimental switches to `settings.env`, through the settings' own writer. Only a
/// name on the closed list, and only a value of its kind.
#[utoipa::path(put, path = "/developer/switches", operation_id = "developer_switches_put",
    tag = "developer", summary = "Save experimental switches", request_body = SwitchesUpdate,
    responses((status = 200, description = "Successful Response", body = SwitchesResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_switches(
    State(deps): State<SharedDeps>,
    Body(update): Body<SwitchesUpdate>,
) -> ApiResult<Json<SwitchesResult>> {
    let mut values: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (name, value) in &update.values {
        let s = switch(name).ok_or_else(|| {
            ApiError::unprocessable(format!("{name} is not one of the experimental switches"))
        })?;
        let v = check_value(s, value.as_deref().unwrap_or("")).map_err(ApiError::unprocessable)?;
        values.insert(name.clone(), v);
    }
    if values.is_empty() {
        return Ok(Json(SwitchesResult {
            switches: switches_view(&deps),
            applied: Vec::new(),
            restart_required: false,
        }));
    }
    ghost_core::envfile::update_env(&deps.settings_path, &values, 0o600)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let applied = deps.control.apply_settings(&values);
    tracing::info!(
        "developer: saved {} to settings.env",
        values.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    let restart_required = values.keys().any(|k| !applied.contains(k));
    Ok(Json(SwitchesResult {
        switches: switches_view(&deps),
        applied,
        restart_required,
    }))
}

// --- debug mode (ADR 0073) --------------------------------------------------------------------

/// How long debug mode stays on once turned on.
pub const DEBUG_AUDIO_MINUTES: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DebugAudioView {
    /// Ghost is logging what it hears and its audio decisions now.
    pub on: bool,
    /// When the switch turns itself off (RFC 3339); None when it is off, or on only because
    /// Ghost was started with `GHOST_DEBUG_AUDIO=1`.
    #[schema(required)]
    pub until: Option<String>,
    /// Ghost was started with `GHOST_DEBUG_AUDIO=1`: on for the life of that process, and
    /// the switch can't turn that off.
    pub from_environment: bool,
    /// Bytes of the debug log kept for a diagnostic report while the switch is on (0600,
    /// deleted when it turns off).
    pub log_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DebugAudioRequest {
    pub on: bool,
}

/// The daemon's switch (`ghostd::debug_audio`).
pub trait DebugAudioPort: Send + Sync {
    fn view(&self) -> DebugAudioView;
    fn set(&self, on: bool) -> DebugAudioView;
}

fn debug_port(deps: &SharedDeps) -> ApiResult<std::sync::Arc<dyn DebugAudioPort>> {
    deps.debug_audio
        .clone()
        .ok_or_else(|| ApiError::unavailable("debug mode is not available in this daemon"))
}

/// Debug mode: on or off, and when it ends.
#[utoipa::path(get, path = "/debug-audio", operation_id = "debug_audio_get",
    tag = "developer", summary = "Debug mode",
    responses((status = 200, description = "Successful Response", body = DebugAudioView)))]
async fn get_debug_audio(State(deps): State<SharedDeps>) -> ApiResult<Json<DebugAudioView>> {
    Ok(Json(debug_port(&deps)?.view()))
}

/// Turn debug mode on for an hour, or off now. Held in the daemon's memory only: a restart
/// turns it off.
#[utoipa::path(put, path = "/debug-audio", operation_id = "debug_audio_put",
    tag = "developer", summary = "Turn debug mode on or off", request_body = DebugAudioRequest,
    responses((status = 200, description = "Successful Response", body = DebugAudioView),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn put_debug_audio(
    State(deps): State<SharedDeps>,
    Body(req): Body<DebugAudioRequest>,
) -> ApiResult<Json<DebugAudioView>> {
    Ok(Json(debug_port(&deps)?.set(req.on)))
}

// --- the diagnostic report --------------------------------------------------------------------

/// What the person chose to add, and what the desktop's shell knows that the daemon doesn't.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiagnoseRequest {
    /// Add the last turns of the conversation with Ghost: your words and Ghost's.
    #[serde(default)]
    pub transcripts: bool,
    /// Add the debug log debug mode is keeping now (what Ghost heard, word for word).
    #[serde(default)]
    pub debug_log: bool,
    /// Add the kept wake recordings (your voice).
    #[serde(default)]
    pub wake_recordings: bool,
    /// The folder to save in (absolute); the Downloads folder when unset.
    #[serde(default)]
    pub folder: Option<String>,
    /// The shell's own facts, each a short line: whether Ghost can float and why not, the
    /// updater's state, this window's version. Scrubbed like a log line.
    #[serde(default)]
    pub shell: BTreeMap<String, String>,
}

/// One part of the report, for the summary the person sees before saving.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DiagnosePart {
    pub key: String,
    pub title: String,
    /// What is in it, in a few words ("120 log lines", "18 turns' timings").
    pub summary: String,
    /// The person's own words or voice: added only when they tick its box.
    pub personal: bool,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DiagnosePreview {
    pub parts: Vec<DiagnosePart>,
    /// The file's size as it would be saved.
    pub bytes: u64,
    /// Where it would go, home shown as `~`.
    pub path: String,
    /// Debug mode is keeping a log now (the debug-log box can add something).
    pub debug_log_available: bool,
    /// Kept wake recordings there are.
    pub wake_recordings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DiagnoseSaved {
    /// The file written, home shown as `~`.
    pub path: String,
    pub bytes: u64,
}

/// The daemon's report builder (`ghostd::diagnose`).
#[async_trait]
pub trait DiagnosePort: Send + Sync {
    async fn preview(&self, req: DiagnoseRequest) -> Result<DiagnosePreview, String>;
    async fn save(&self, req: DiagnoseRequest) -> Result<DiagnoseSaved, String>;
}

fn diagnose_port(deps: &SharedDeps) -> ApiResult<std::sync::Arc<dyn DiagnosePort>> {
    deps.diagnose
        .clone()
        .ok_or_else(|| ApiError::unavailable("this daemon cannot write a diagnostic report"))
}

fn check_request(req: &DiagnoseRequest) -> ApiResult<()> {
    if req.shell.len() > 8 || req.shell.iter().any(|(k, v)| k.len() > 32 || v.len() > 400) {
        return Err(ApiError::unprocessable("shell: at most 8 short lines"));
    }
    if let Some(f) = &req.folder
        && (!f.starts_with('/') || f.len() > 1024)
    {
        return Err(ApiError::unprocessable("folder: an absolute path"));
    }
    Ok(())
}

/// What the report would hold, part by part, and its size. Nothing is written.
#[utoipa::path(post, path = "/diagnose/preview", operation_id = "diagnose_preview_post",
    tag = "developer", summary = "Preview a diagnostic report", request_body = DiagnoseRequest,
    responses((status = 200, description = "Successful Response", body = DiagnosePreview),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn diagnose_preview(
    State(deps): State<SharedDeps>,
    Body(req): Body<DiagnoseRequest>,
) -> ApiResult<Json<DiagnosePreview>> {
    check_request(&req)?;
    diagnose_port(&deps)?
        .preview(req)
        .await
        .map(Json)
        .map_err(ApiError::unprocessable)
}

/// Save the diagnostic report: one JSON file in the Downloads folder (or the one named),
/// 0600. Nothing is uploaded anywhere.
#[utoipa::path(post, path = "/diagnose", operation_id = "diagnose_post",
    tag = "developer", summary = "Save a diagnostic report", request_body = DiagnoseRequest,
    responses((status = 200, description = "Successful Response", body = DiagnoseSaved),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn diagnose_save(
    State(deps): State<SharedDeps>,
    Body(req): Body<DiagnoseRequest>,
) -> ApiResult<Json<DiagnoseSaved>> {
    check_request(&req)?;
    diagnose_port(&deps)?
        .save(req)
        .await
        .map(Json)
        .map_err(ApiError::unprocessable)
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(get_switches, put_switches))
        .routes(routes!(get_debug_audio, put_debug_audio))
        .routes(routes!(diagnose_preview))
        .routes(routes!(diagnose_save))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust sources of the crates whose readers take Ghost's settings, without this file.
    fn sources() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut text = String::new();
        for krate in ["ghost-voice", "ghostd", "ghost-llm", "ghost-core"] {
            let mut stack = vec![root.join(krate).join("src")];
            while let Some(dir) = stack.pop() {
                for e in std::fs::read_dir(&dir).unwrap().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("tests.rs") {
                        text.push_str(&std::fs::read_to_string(&p).unwrap());
                    }
                }
            }
        }
        text
    }

    #[test]
    fn every_switch_names_a_variable_the_daemon_reads() {
        let text = sources();
        for s in SWITCHES {
            assert!(
                text.contains(&format!("\"{}\"", s.name)),
                "{} is not read by the daemon",
                s.name
            );
        }
    }

    #[test]
    fn the_list_is_closed_to_passwords_debug_mode_the_voice_mode_listeners_and_paths() {
        for s in SWITCHES {
            assert!(
                !ghost_core::environment::looks_like_a_password(s.name),
                "{}",
                s.name
            );
            for refused in [
                "GHOST_DEBUG_AUDIO",
                "GHOST_VOICE_MODE",
                "GHOST_REALTIME_VOICE",
                "GHOST_LOG",
                "GHOST_LOG_LEVEL",
            ] {
                assert_ne!(s.name, refused);
            }
            for part in ["LISTEN", "_DIR", "_PATH", "PASSWORD", "KEY", "TOKEN", "URL"] {
                assert!(!s.name.contains(part), "{}", s.name);
            }
        }
        let mut names: Vec<_> = SWITCHES.iter().map(|s| s.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), SWITCHES.len(), "a name twice");
    }

    #[test]
    fn a_value_is_checked_by_its_kind() {
        let race = switch("GHOST_LLM_RACE").unwrap();
        assert_eq!(check_value(race, "true").unwrap().as_deref(), Some("on"));
        assert!(check_value(race, "maybe").is_err());
        let stream = switch("GHOST_TTS_STREAM").unwrap();
        assert_eq!(
            check_value(stream, "Windowed").unwrap().as_deref(),
            Some("windowed")
        );
        assert!(check_value(stream, "bogus").is_err());
        let grace = switch("GHOST_TURN_GRACE_SECS").unwrap();
        assert_eq!(check_value(grace, " ").unwrap(), None);
        assert!(check_value(grace, "-1").is_err());
        assert!(check_value(grace, "99").is_err());
        assert!(check_value(grace, "nan").is_err());
        assert_eq!(check_value(grace, "0.8").unwrap().as_deref(), Some("0.8"));
        assert!(switch("GHOST_DEBUG_AUDIO").is_none());
    }
}
