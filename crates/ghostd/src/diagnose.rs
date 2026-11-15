//! The diagnostic report (ADR 0073): one file the person saves and hands to a developer,
//! built from what Ghost already knows. Nothing is uploaded: it is a file in their Downloads
//! folder (or one they name), 0600. `docs/diagnostic-report.md` lists every field.
//!
//! What it never holds unless the person ticks a box for it: what was said or typed (a
//! transcript), audio, debug mode's log. What it never holds at all: a password (the vault's
//! redaction first), an app's content (apps by name and state only; a log line keeps the
//! code's own words, `ghost_reports::scrub::head`, so an app's error text after the first
//! `": "` is dropped), a persona or memory, a home path, an address with credentials. Every
//! string outside the person's own additions goes through the error reports' scrubber
//! (`ghost_reports::Scrubber`, with the vault's `redact` first); the kept error reports go in
//! exactly as they were sent, scrubbed already. Tests hold each line with hostile fixtures.
//!
//! The operator's only: `POST /diagnose` on the control API and `ghostd diagnose`. No tool of
//! Ghost's own app, run, outside client, phone route, app or model can start or read one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use ghost_api::developer::{
    DiagnosePart, DiagnosePort, DiagnosePreview, DiagnoseRequest, DiagnoseSaved,
};
use ghost_reports::Scrubber;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;

/// The report's format; a reader checks it.
pub const FORMAT: u32 = 1;
/// Turns' timings kept.
pub const LATENCY_TURNS: usize = 30;
/// Turns of the conversation with Ghost the transcripts box adds.
pub const TRANSCRIPT_TURNS: i64 = 40;
/// Wake recordings the box adds, newest first.
pub const WAKE_RECORDINGS: usize = 8;
/// The longest log line kept.
pub const LOG_CHARS: usize = 240;
/// The longest other string kept.
pub const TEXT_CHARS: usize = 400;

/// What the daemon knows, gathered and not yet scrubbed. Every field optional: the command
/// line with no daemon running fills what the files give.
#[derive(Debug, Default, Clone)]
pub struct Inputs {
    pub at: Option<DateTime<Utc>>,
    pub version: String,
    pub build: Value,
    /// `ghost_reports::Context`: OS, session type, architecture, how it was installed.
    pub computer: Value,
    /// The desktop shell's own lines (floating, updater, the window's version).
    pub shell: BTreeMap<String, String>,
    /// Facts about the display that decide floating (ADR 0059).
    pub display: Value,
    pub sound: Value,
    pub model: Value,
    pub speech_models: Value,
    pub latency: Vec<Value>,
    pub wake: Value,
    /// Exactly as sent (scrubbed when they were made).
    pub error_reports: Vec<Value>,
    pub counts: Value,
    /// `time LEVEL message`, redacted against the vault.
    pub log: Vec<String>,
}

/// What the person chose to add, each behind a box with a warning.
#[derive(Debug, Default, Clone)]
pub struct Added {
    /// (role, at, text)
    pub transcripts: Option<Vec<(String, String, String)>>,
    pub debug_log: Option<String>,
    /// (id, outcome, wav bytes)
    pub wake_recordings: Option<Vec<(String, String, Vec<u8>)>>,
}

/// Fields whose words may carry someone else's text after the code's own (an error, a
/// problem): only the code's own words are kept (`scrub::head`).
const HEAD_ONLY: [&str; 4] = ["error", "problem", "why_not", "advice"];

/// Every string in `v` through the scrubber.
fn scrub_all(v: &mut Value, s: &Scrubber) {
    match v {
        Value::String(t) => *t = s.text(t, TEXT_CHARS),
        Value::Array(a) => a.iter_mut().for_each(|x| scrub_all(x, s)),
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if HEAD_ONLY.contains(&k.as_str())
                    && let Value::String(t) = x
                {
                    *t = ghost_reports::scrub::head(t);
                }
                scrub_all(x, s);
            }
        }
        _ => {}
    }
}

/// A turn's timing record or a wake attempt with numbers, yes/no and times only: model ids,
/// tool names and anything read or heard go.
pub fn numbers_only(v: &Value) -> Option<Value> {
    match v {
        Value::Number(_) | Value::Bool(_) => Some(v.clone()),
        Value::String(s) if DateTime::parse_from_rfc3339(s).is_ok() => Some(v.clone()),
        Value::Array(a) => {
            let kept: Vec<Value> = a.iter().filter_map(numbers_only).collect();
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        Value::Object(o) => {
            let kept: Map<String, Value> = o
                .iter()
                .filter_map(|(k, x)| numbers_only(x).map(|y| (k.clone(), y)))
                .collect();
            (!kept.is_empty()).then_some(Value::Object(kept))
        }
        _ => None,
    }
}

/// A log line, `time LEVEL message`, cut to the code's own words and scrubbed.
pub fn log_line(line: &str, s: &Scrubber) -> String {
    let mut parts = line.splitn(3, ' ');
    let (time, level, msg) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let msg = s.text(&ghost_reports::scrub::head(msg), LOG_CHARS);
    format!("{time} {level} {msg}")
}

const ABOUT: &str = "A diagnostic report from Ghost, saved by the person who uses it to hand to a developer. Nothing in it was sent anywhere by Ghost. It holds no password, no home path, no app's content and nothing that was said or typed, unless the person added a part under added_by_you. docs/diagnostic-report.md lists every field.";

/// The report: the parts the person always gets, scrubbed, then what they added.
pub fn build(inputs: &Inputs, added: &Added, s: &Scrubber) -> (Value, Vec<DiagnosePart>) {
    let mut base = json!({
        "version": inputs.version,
        "build": inputs.build,
        "computer": inputs.computer,
        "desktop": inputs.shell,
        "display": inputs.display,
        "sound": inputs.sound,
        "model": inputs.model,
        "speech_models": inputs.speech_models,
        "wake_attempts": numbers_only(&inputs.wake).unwrap_or(Value::Null),
        "counts": inputs.counts,
    });
    scrub_all(&mut base, s);
    let latency: Vec<Value> = inputs.latency.iter().filter_map(numbers_only).collect();
    let log: Vec<String> = inputs.log.iter().map(|l| log_line(l, s)).collect();

    let mut parts = Vec::new();
    let size = |v: &Value| serde_json::to_vec(v).map(|b| b.len() as u64).unwrap_or(0);
    let mut part = |key: &str, title: &str, summary: String, personal: bool, v: &Value| {
        parts.push(DiagnosePart {
            key: key.into(),
            title: title.into(),
            summary,
            personal,
            bytes: size(v),
        });
    };
    let obj = base.as_object().cloned().unwrap_or_default();
    let get = |k: &str| obj.get(k).cloned().unwrap_or(Value::Null);
    part(
        "about",
        "This Ghost and this computer",
        "version, build, how it was installed, the OS and the session type".into(),
        false,
        &json!([
            get("version"),
            get("build"),
            get("computer"),
            get("desktop"),
            get("display")
        ]),
    );
    part(
        "sound",
        "Sound",
        "the microphone and speakers in use, how Ghost hears you, the echo canceller".into(),
        false,
        &get("sound"),
    );
    part(
        "model",
        "Model",
        "the provider and model names and the last check's timings; never a key or an address"
            .into(),
        false,
        &get("model"),
    );
    part(
        "speech_models",
        "Speech models",
        "whether Parakeet and Kokoro are here".into(),
        false,
        &get("speech_models"),
    );
    let latency_v = Value::Array(latency.clone());
    part(
        "latency",
        "Turn timings",
        format!("{} recent turns, numbers only", latency.len()),
        false,
        &latency_v,
    );
    part(
        "wake_attempts",
        "Wake attempts",
        "counts and scores only, no sound and no words".into(),
        false,
        &get("wake_attempts"),
    );
    let reports_v = Value::Array(inputs.error_reports.clone());
    part(
        "error_reports",
        "Error reports",
        format!(
            "{} kept, exactly as they were sent",
            inputs.error_reports.len()
        ),
        false,
        &reports_v,
    );
    part(
        "counts",
        "Apps, agents and schedules",
        "how many, and the apps' names and states; never what they hold".into(),
        false,
        &get("counts"),
    );
    let log_v = json!(log);
    part(
        "log",
        "Ghost's log",
        format!(
            "the last {} lines, Ghost's own words only, scrubbed",
            log.len()
        ),
        false,
        &log_v,
    );

    let mut mine = Map::new();
    if let Some(t) = &added.transcripts {
        let v: Vec<Value> = t
            .iter()
            .map(|(role, at, text)| json!({"role": role, "at": at, "text": text}))
            .collect();
        let v = Value::Array(v);
        part(
            "transcripts",
            "Recent transcripts",
            format!(
                "{} turns of your conversation with Ghost, word for word",
                t.len()
            ),
            true,
            &v,
        );
        mine.insert("transcripts".into(), v);
    }
    if let Some(log) = &added.debug_log {
        let v = json!(log.lines().collect::<Vec<_>>());
        part(
            "debug_log",
            "Debug log",
            format!(
                "{} lines of what Ghost heard and decided while debug mode was on",
                log.lines().count()
            ),
            true,
            &v,
        );
        mine.insert("debug_log".into(), v);
    }
    if let Some(takes) = &added.wake_recordings {
        let v: Vec<Value> = takes
            .iter()
            .map(|(id, outcome, wav)| {
                json!({
                    "id": id,
                    "outcome": outcome,
                    "wav_base64": base64::engine::general_purpose::STANDARD.encode(wav),
                })
            })
            .collect();
        let v = Value::Array(v);
        part(
            "wake_recordings",
            "Wake recordings",
            format!(
                "{} recordings of your voice saying Ghost's name",
                takes.len()
            ),
            true,
            &v,
        );
        mine.insert("wake_recordings".into(), v);
    }

    let mut report = Map::new();
    report.insert("ghost_diagnostic_report".into(), json!(FORMAT));
    report.insert("about_this_file".into(), json!(ABOUT));
    report.insert(
        "saved_at".into(),
        json!(inputs.at.unwrap_or_else(Utc::now).to_rfc3339()),
    );
    for (k, v) in obj {
        report.insert(k, v);
    }
    report.insert("turn_timings".into(), latency_v);
    report.insert("error_reports".into(), reports_v);
    report.insert("log".into(), log_v);
    if !mine.is_empty() {
        report.insert("added_by_you".into(), Value::Object(mine));
    }
    (Value::Object(report), parts)
}

/// The report's bytes as written.
pub fn render(report: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec_pretty(report).unwrap_or_default();
    b.push(b'\n');
    b
}

/// The person's Downloads folder: `XDG_DOWNLOAD_DIR` from `user-dirs.dirs`, else
/// `~/Downloads` where it exists, else home.
pub fn downloads(home: &Path, config_home: Option<&Path>) -> PathBuf {
    let dirs = config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("user-dirs.dirs");
    if let Ok(text) = std::fs::read_to_string(dirs) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let v = v.trim().trim_matches('"');
                let p = match v.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => PathBuf::from(v),
                };
                if p.is_absolute() && p.is_dir() {
                    return p;
                }
            }
        }
    }
    let d = home.join("Downloads");
    if d.is_dir() { d } else { home.to_path_buf() }
}

pub fn file_name(at: DateTime<Utc>) -> String {
    format!(
        "ghost-diagnostic-{}.json",
        at.with_timezone(&chrono::Local).format("%Y%m%d-%H%M%S")
    )
}

/// Write `bytes` to `path`, 0600, through a file beside it, never over an existing file.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("json.part");
    {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(&tmp);
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "a file of that name is there",
        ));
    }
    std::fs::rename(&tmp, path)
}

fn tilde(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The folder a request names, checked: absolute, a folder, no `..`.
fn folder_of(req: &DiagnoseRequest, home: &Path) -> Result<PathBuf, String> {
    match &req.folder {
        None => Ok(downloads(
            home,
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .as_deref(),
        )),
        Some(f) => {
            let p = PathBuf::from(f);
            if !p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("the folder must be an absolute path".into());
            }
            if !p.is_dir() {
                return Err("that folder isn't there".into());
            }
            Ok(p)
        }
    }
}

// --- what the files give, with or without a daemon -----------------------------------------

/// The parts the files on this computer give: version, build, computer, display, the turn
/// timings, the wake attempts, the kept error reports, the model's names.
pub fn from_files(cfg: &crate::GhostdSettings, env: &BTreeMap<String, String>) -> Inputs {
    let build = crate::build_info::detect(None, None, None);
    let version = env!("CARGO_PKG_VERSION").to_string();
    let ctx = ghost_reports::Context::detect(&version, ghost_reports::Component::Daemon);
    let latency = crate::latency::tail_records(&cfg.latency_path(), 512 * 1024);
    let latency = latency[latency.len().saturating_sub(LATENCY_TURNS)..].to_vec();
    let takes = ghost_core::wake_takes::Takes::new(&cfg.data_dir);
    let reporter = crate::reports::reporter(&cfg.data_dir, |k| std::env::var(k).ok());
    let conn = ghost_core::connection::ModelConnection::from_env(
        &env.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::HashMap<_, _>>(),
    );
    Inputs {
        at: Some(Utc::now()),
        version,
        build: serde_json::to_value(&build).unwrap_or(Value::Null),
        computer: json!({
            "os": ctx.os_name,
            "os_version": ctx.os_version,
            "session_type": ctx.session_type,
            "arch": ctx.arch,
            "installed_as": ctx.installed_as,
        }),
        display: display(env),
        model: json!({
            "provider": conn.provider,
            "model": conn.wire_model(),
            "fast_model": conn.fast_wire_model(),
        }),
        latency,
        wake: wake_attempts(&takes),
        error_reports: reporter.kept().into_iter().map(|k| k.event).collect(),
        ..Inputs::default()
    }
}

/// What decides whether Ghost can float (ADR 0059): an X server to reach, and the session.
pub fn display(env: &BTreeMap<String, String>) -> Value {
    let x11 = env.get("DISPLAY").is_some_and(|d| !d.is_empty());
    let wayland = env.get("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
    json!({
        "x11_reachable": x11,
        "wayland": wayland,
        "can_float": x11,
        "why_not": if x11 { Value::Null } else { json!("no X11 or XWayland: Ghost needs it to float") },
    })
}

/// The kept wake attempts as counts and scores: never their sound or what Parakeet read.
pub fn wake_attempts(takes: &ghost_core::wake_takes::Takes) -> Value {
    let list = takes.list();
    let woke = list
        .iter()
        .filter(|t| matches!(t.outcome, ghost_core::wake_takes::Outcome::Accepted))
        .count();
    let recent: Vec<Value> = list
        .iter()
        .rev()
        .take(50)
        .map(|t| {
            json!({
                "at": t.at.to_rfc3339(),
                "woke": matches!(t.outcome, ghost_core::wake_takes::Outcome::Accepted),
                "score": t.score,
                "threshold": t.threshold,
                "labelled": t.label.is_some(),
            })
        })
        .collect();
    json!({
        "kept": list.len(),
        "woke": woke,
        "refused": list.len() - woke,
        "keeping": takes.choice().keep,
        "recent": recent,
    })
}

/// The newest kept wake recordings, as files (the box's).
pub fn wake_recordings(takes: &ghost_core::wake_takes::Takes) -> Vec<(String, String, Vec<u8>)> {
    takes
        .list()
        .iter()
        .rev()
        .take(WAKE_RECORDINGS)
        .filter_map(|t| {
            let wav = std::fs::read(takes.dir().join(format!("{}.wav", t.id))).ok()?;
            let outcome = match t.outcome {
                ghost_core::wake_takes::Outcome::Accepted => "woke",
                ghost_core::wake_takes::Outcome::Refused => "refused",
            };
            Some((t.id.clone(), outcome.to_string(), wav))
        })
        .collect()
}

// --- the daemon's side ------------------------------------------------------------------------

pub struct DiagnoseDesk {
    pub daemon: Weak<Daemon>,
}

impl DiagnoseDesk {
    async fn gather(&self, req: &DiagnoseRequest) -> Result<(Inputs, Added), String> {
        let d = self.daemon.upgrade().ok_or("Ghost is stopping")?;
        let cfg = d.cfg();
        let env: BTreeMap<String, String> = d.env().into_iter().collect();
        let mut inputs = from_files(&cfg, &env);
        inputs.shell = req.shell.clone();
        let deps = d.late.deps.get().cloned();
        if let Some(deps) = &deps {
            let mic = deps.control.microphone().await;
            let devices = deps.devices.devices();
            inputs.sound = json!({
                "input_setting": deps.input_device,
                "output_device": deps.output_device,
                "case": mic.as_ref().map(|m| m.case.clone()),
                "tested_array": mic.as_ref().map(|m| m.case == "on-chip"),
                "session_input": mic.as_ref().map(|m| m.session_input.clone()),
                "wake_input": mic.as_ref().map(|m| m.wake_input.clone()),
                "problem": mic.as_ref().and_then(|m| m.problem.clone()),
                "fell_back": mic.as_ref().and_then(|m| m.fell_back.clone()),
                "repair": mic.as_ref().map(|m| m.repair.clone()),
                "echo_canceller_running": deps.devices.echo_cancel_active(),
                "echo_canceller_installed": devices.iter().any(|x| x.name == "ghost.mic"),
                "devices": devices.iter().map(|x| json!({
                    "name": x.name,
                    "inputs": x.max_input_channels,
                    "outputs": x.max_output_channels,
                    "rate": x.default_samplerate,
                })).collect::<Vec<_>>(),
            });
            if let Some(m) = inputs.model.as_object_mut()
                && let Some(run) = deps.control.last_model_check()
            {
                m.insert(
                    "last_check".into(),
                    json!({
                        "at": run.at.to_rfc3339(),
                        "advice": run.advice,
                        "models": run.models.iter().map(|c| json!({
                            "role": c.role,
                            "model": c.model,
                            "reachable": c.reachable,
                            "error": c.error.as_deref().map(ghost_reports::scrub::head),
                            "first_byte": c.first_byte,
                            "first_word": c.first_word,
                            "samples": c.samples,
                            "streaming": c.streaming,
                            "tools": c.tools,
                            "caching": c.caching,
                            "thinking": c.thinking,
                        })).collect::<Vec<_>>(),
                    }),
                );
            }
            if let Some(sm) = &deps.speech_models {
                let v = sm.view();
                inputs.speech_models = json!({
                    "ready": v.ready,
                    "missing": v.missing.len(),
                    "missing_bytes": v.missing_bytes,
                    "free_bytes": v.free_bytes,
                    "fetch": serde_json::to_value(&v.fetch).unwrap_or(Value::Null),
                });
            }
            let apps: Vec<Value> = deps
                .apps
                .as_ref()
                .map(|a| {
                    a.names()
                        .into_iter()
                        .map(|n| json!({"name": n, "state": a.state(&n)}))
                        .collect()
                })
                .unwrap_or_default();
            inputs.counts = json!({
                "apps": apps,
                "agents": deps.conversations.as_ref().and_then(|c| c.list(false).ok()).map(|l| l.len()),
                "schedules": deps.schedules.as_ref().and_then(|s| s.jobs().list().ok()).map(|l| l.len()),
            });
            inputs.version = deps.state.version();
            inputs.build = serde_json::to_value(deps.state.build()).unwrap_or(Value::Null);
        }
        inputs.log = d.log_tail();
        let mut added = Added::default();
        if req.transcripts
            && let Some(store) = d.late.store.get()
        {
            added.transcripts = Some(
                store
                    .recent_turns("ghost", TRANSCRIPT_TURNS, false)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|t| {
                        (
                            serde_json::to_value(t.role)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_default(),
                            t.at.to_rfc3339(),
                            t.text,
                        )
                    })
                    .collect(),
            );
        }
        if req.debug_log {
            added.debug_log = Some(d.debug_audio().captured().unwrap_or_default());
        }
        if req.wake_recordings {
            added.wake_recordings = Some(wake_recordings(&d.wake_takes()));
        }
        Ok((inputs, added))
    }

    fn scrubber() -> Scrubber {
        Scrubber::from_env().with_redact(Arc::new(ghost_core::redact::redact_text))
    }
}

#[async_trait]
impl DiagnosePort for DiagnoseDesk {
    async fn preview(&self, req: DiagnoseRequest) -> Result<DiagnosePreview, String> {
        let home = home();
        let folder = folder_of(&req, &home)?;
        let (inputs, added) = self.gather(&req).await?;
        let (report, parts) = build(&inputs, &added, &Self::scrubber());
        let d = self.daemon.upgrade().ok_or("Ghost is stopping")?;
        Ok(DiagnosePreview {
            parts,
            bytes: render(&report).len() as u64,
            path: tilde(&folder.join(file_name(Utc::now())), &home),
            debug_log_available: d.debug_audio().captured().is_some(),
            wake_recordings: d.wake_takes().list().len().min(WAKE_RECORDINGS),
        })
    }

    async fn save(&self, req: DiagnoseRequest) -> Result<DiagnoseSaved, String> {
        let home = home();
        let folder = folder_of(&req, &home)?;
        let (inputs, added) = self.gather(&req).await?;
        let (report, _) = build(&inputs, &added, &Self::scrubber());
        let bytes = render(&report);
        let path = folder.join(file_name(Utc::now()));
        write_private(&path, &bytes).map_err(|e| format!("couldn't save it: {e}"))?;
        tracing::info!(
            "diagnostic report saved ({} bytes{})",
            bytes.len(),
            if req.transcripts || req.debug_log || req.wake_recordings {
                ", with parts the person added"
            } else {
                ""
            }
        );
        Ok(DiagnoseSaved {
            path: tilde(&path, &home),
            bytes: bytes.len() as u64,
        })
    }
}

// --- `ghostd diagnose` ------------------------------------------------------------------------

pub const USAGE: &str =
    "usage: ghostd diagnose [--out FILE] [--transcripts] [--debug-log] [--wake-recordings]";

/// `ghostd diagnose`: through the running daemon, which knows the most; with none running,
/// from the files alone (and none of the parts only a running Ghost has).
pub async fn cli(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut req = DiagnoseRequest::default();
    let mut file: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--transcripts" => req.transcripts = true,
            "--debug-log" => req.debug_log = true,
            "--wake-recordings" => req.wake_recordings = true,
            "--out" => match it.next() {
                Some(f) => file = Some(PathBuf::from(f)),
                None => {
                    let _ = writeln!(err, "{USAGE}");
                    return 2;
                }
            },
            "-h" | "--help" => {
                let _ = writeln!(out, "{USAGE}");
                return 0;
            }
            other => {
                let _ = writeln!(err, "unknown: {other}; {USAGE}");
                return 2;
            }
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let file = file.map(|f| if f.is_absolute() { f } else { cwd.join(f) });
    let socket = ghost_api::server::default_socket_path();
    if socket.exists() && file.is_none() {
        let client = ghost_api::GhostClient::new(&socket);
        let body = serde_json::to_value(&req).unwrap_or(Value::Null);
        return match client.diagnose(&body).await {
            Ok(v) => {
                let _ = writeln!(
                    out,
                    "saved {} ({} bytes). Nothing was sent anywhere.",
                    v["path"].as_str().unwrap_or("?"),
                    v["bytes"]
                );
                0
            }
            Err(e) => {
                let _ = writeln!(err, "ghostd diagnose: {e}");
                1
            }
        };
    }
    // A named file, or no daemon: built here. A running daemon is asked for what only it
    // knows when it is there, through the same route, written to a temp name and moved.
    let cfg = match crate::GhostdSettings::load() {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "ghostd diagnose: {e}");
            return 2;
        }
    };
    let home = home();
    let path = match &file {
        Some(f) => f.clone(),
        None => downloads(
            &home,
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .as_deref(),
        )
        .join(file_name(Utc::now())),
    };
    if socket.exists() {
        if let Some(dir) = path.parent() {
            req.folder = Some(dir.display().to_string());
        }
        let client = ghost_api::GhostClient::new(&socket);
        let body = serde_json::to_value(&req).unwrap_or(Value::Null);
        return match client.diagnose(&body).await {
            Ok(v) => {
                let saved = v["path"].as_str().unwrap_or_default().to_string();
                let saved = saved
                    .strip_prefix("~/")
                    .map(|r| home.join(r))
                    .unwrap_or_else(|| PathBuf::from(&saved));
                if saved != path
                    && let Err(e) = std::fs::rename(&saved, &path)
                {
                    let _ = writeln!(err, "saved {}, but couldn't name it: {e}", saved.display());
                    return 1;
                }
                let _ = writeln!(
                    out,
                    "saved {} ({} bytes). Nothing was sent anywhere.",
                    tilde(&path, &home),
                    v["bytes"]
                );
                0
            }
            Err(e) => {
                let _ = writeln!(err, "ghostd diagnose: {e}");
                1
            }
        };
    }
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let mut inputs = from_files(&cfg, &env);
    inputs.shell.insert(
        "note".into(),
        "Ghost wasn't running: only what its files say".into(),
    );
    let mut added = Added::default();
    if req.wake_recordings {
        added.wake_recordings = Some(wake_recordings(&ghost_core::wake_takes::Takes::new(
            &cfg.data_dir,
        )));
    }
    if req.transcripts || req.debug_log {
        let _ = writeln!(
            err,
            "transcripts and the debug log need Ghost running; left out"
        );
    }
    let (report, _) = build(
        &inputs,
        &added,
        &Scrubber::from_env().with_redact(Arc::new(ghost_core::redact::redact_text)),
    );
    let bytes = render(&report);
    match write_private(&path, &bytes) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "saved {} ({} bytes). Nothing was sent anywhere.",
                tilde(&path, &home),
                bytes.len()
            );
            0
        }
        Err(e) => {
            let _ = writeln!(err, "ghostd diagnose: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "hunter2-vault-Zq8xT2v9";
    const HOME: &str = "/home/rowan";
    const SPOKEN: &str = "remind me to call the dentist about the crown";
    const APP_ERROR: &str = "Your billing plan expired, visit our pricing page";
    const TOKEN_URL: &str =
        "https://ada:s3cret@api.example.com/v1/x?token=abc123def456ghi789jkl012";

    fn scrubber() -> Scrubber {
        Scrubber::new(
            Some(HOME.into()),
            Some("rowan".into()),
            Some("deskbox".into()),
        )
        .with_redact(Arc::new(|s: &str| {
            s.replace(PASSWORD, "[password: GITHUB_TOKEN]")
        }))
    }

    /// Every hostile thing a report must never carry, in every field Ghost fills.
    fn hostile() -> Inputs {
        Inputs {
            at: None,
            version: "0.9.0".into(),
            build: json!({"kind": "release", "commit": "abc1234"}),
            computer: json!({"os": "Ubuntu", "session_type": "wayland"}),
            shell: BTreeMap::from([(
                "floating".into(),
                format!("refused: no X11 at {HOME}/.Xauthority for rowan@deskbox"),
            )]),
            display: json!({"can_float": false}),
            sound: json!({
                "session_input": format!("{HOME}/mic"),
                "problem": format!("loud speech reads as nothing ({PASSWORD})"),
            }),
            model: json!({
                "provider": "custom",
                "model": "m",
                "base_url": TOKEN_URL,
                "last_check": {"models": [{"error": format!("401: {APP_ERROR}")}]},
            }),
            speech_models: json!({"folder": format!("{HOME}/.cache/ghost")}),
            latency: vec![json!({
                "reply": 1.2, "llm": {"ttfb": 0.4, "model": "claude-x"},
                "tools": ["github_issue_write"], "heard": SPOKEN, "origin": "desk",
            })],
            wake: json!({"kept": 3, "recent": [{"score": 0.8, "reading": SPOKEN}]}),
            error_reports: vec![
                json!({"event": "$exception", "properties": {"ghost_kind": "test"}}),
            ],
            counts: json!({"apps": [{"name": "github", "state": "connected"}], "agents": 2}),
            log: vec![
                format!("2026-11-14T10:00:00Z WARNING app github: call failed: {APP_ERROR}"),
                format!(
                    "2026-11-14T10:00:01Z INFO settings.env loaded from {HOME}/.config/ghost/settings.env"
                ),
                format!("2026-11-14T10:00:02Z ERROR model: {TOKEN_URL} refused {PASSWORD}"),
                format!("2026-11-14T10:00:03Z INFO vault opened, {PASSWORD} cached"),
                format!("2026-11-14T10:00:04Z INFO turn closed \"{SPOKEN}\""),
                "2026-11-14T10:00:05Z INFO api: listening on a socket".into(),
            ],
        }
    }

    #[test]
    fn a_report_holds_no_password_home_path_transcript_app_text_or_credential() {
        let (report, parts) = build(&hostile(), &Added::default(), &scrubber());
        let text = String::from_utf8(render(&report)).unwrap();
        for bad in [
            PASSWORD,
            HOME,
            "rowan",
            "deskbox",
            SPOKEN,
            APP_ERROR,
            "s3cret",
            "abc123def456ghi789jkl012",
            "ada:",
            "claude-x",
            "github_issue_write",
        ] {
            assert!(!text.contains(bad), "{bad} is in the report:\n{text}");
        }
        // What it is for is there.
        assert!(text.contains("\"reply\": 1.2"));
        assert!(text.contains("\"ttfb\": 0.4"));
        assert!(text.contains("app github"), "the code's own words stay");
        assert!(text.contains("api.example.com"), "a URL keeps its host");
        assert!(text.contains("\"ghost_kind\": \"test\""), "reports as sent");
        assert!(text.contains("\"github\""), "an app's name is fine");
        assert!(report["added_by_you"].is_null());
        assert!(parts.iter().all(|p| !p.personal));
    }

    #[test]
    fn the_wake_attempts_carry_counts_and_scores_never_words_or_sound() {
        let tmp = tempfile::tempdir().unwrap();
        let takes = ghost_core::wake_takes::Takes::new(tmp.path());
        let _ = takes.keep(ghost_core::wake_takes::NewTake {
            audio: vec![0; 1600],
            hit: 800,
            accepted: true,
            model: "hey_ghost".into(),
            score: 0.91,
            threshold: 0.6,
            input: "ghost.mic".into(),
            reading: Some(SPOKEN.into()),
            scores: vec![0.1, 0.9],
            scores_end: 2,
            array: None,
            cut_at_start: false,
        });
        let v = wake_attempts(&takes).to_string();
        assert!(!v.contains(SPOKEN), "{v}");
        assert!(!v.contains("wav"));
        if takes.list().len() == 1 {
            assert!(v.contains("0.91"), "{v}");
        }
    }

    #[test]
    fn each_box_adds_its_part_and_only_then() {
        let added = Added {
            transcripts: Some(vec![(
                "user".into(),
                "2026-11-14T10:00:00Z".into(),
                SPOKEN.into(),
            )]),
            debug_log: Some(format!("[audio] heard: {SPOKEN}\n")),
            wake_recordings: Some(vec![("t1".into(), "woke".into(), b"RIFF....WAVE".to_vec())]),
        };
        let (report, parts) = build(&hostile(), &added, &scrubber());
        let mine = &report["added_by_you"];
        assert_eq!(mine["transcripts"][0]["text"], SPOKEN);
        assert!(mine["debug_log"][0].as_str().unwrap().contains(SPOKEN));
        assert!(
            mine["wake_recordings"][0]["wav_base64"]
                .as_str()
                .unwrap()
                .len()
                > 8
        );
        let personal: Vec<_> = parts
            .iter()
            .filter(|p| p.personal)
            .map(|p| p.key.as_str())
            .collect();
        assert_eq!(personal, ["transcripts", "debug_log", "wake_recordings"]);
        // Nothing of theirs outside what they added.
        let mut rest = report.clone();
        rest.as_object_mut().unwrap().remove("added_by_you");
        assert!(!rest.to_string().contains(SPOKEN));
        // Only the transcripts box.
        let only = Added {
            transcripts: added.transcripts.clone(),
            ..Added::default()
        };
        let (r, _) = build(&hostile(), &only, &scrubber());
        assert!(r["added_by_you"]["debug_log"].is_null());
        assert!(r["added_by_you"]["wake_recordings"].is_null());
    }

    #[test]
    fn it_saves_one_0600_file_in_the_downloads_folder_never_over_another() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::create_dir_all(home.join("Telechargements")).unwrap();
        std::fs::write(
            home.join(".config/user-dirs.dirs"),
            "XDG_DOWNLOAD_DIR=\"$HOME/Telechargements\"\n",
        )
        .unwrap();
        let dir = downloads(home, None);
        assert_eq!(dir, home.join("Telechargements"));
        let path = dir.join("ghost-diagnostic-x.json");
        write_private(&path, b"{}\n").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_private(&path, b"{}\n").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // No user-dirs: ~/Downloads, else home.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(downloads(bare.path(), None), bare.path());
    }

    #[test]
    fn a_folder_must_be_absolute_and_there() {
        let home = PathBuf::from("/nonexistent-home");
        let mut req = DiagnoseRequest {
            folder: Some("relative/dir".into()),
            ..DiagnoseRequest::default()
        };
        assert!(folder_of(&req, &home).is_err());
        req.folder = Some("/tmp/../etc".into());
        assert!(folder_of(&req, &home).is_err());
        req.folder = Some("/definitely/not/here".into());
        assert!(folder_of(&req, &home).is_err());
    }

    #[test]
    fn numbers_only_keeps_numbers_and_yes_no() {
        let v = numbers_only(&json!({"a": 1, "b": "x", "t": "2026-11-14T10:00:00Z", "c": [1, "y", {"d": true, "e": "z"}]})).unwrap();
        assert_eq!(
            v,
            json!({"a": 1, "t": "2026-11-14T10:00:00Z", "c": [1, {"d": true}]})
        );
    }
}
