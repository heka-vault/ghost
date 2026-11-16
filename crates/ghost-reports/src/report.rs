//! One report, built in one place: what failed (a panic, a page's uncaught error, or one of a
//! closed list of failures), where (the component), and a few facts about this copy of Ghost
//! and the computer it runs on, none of which names the computer or the operator.
//!
//! The event is PostHog's `$exception` (error tracking's capture API, `POST /i/v0/e/`): one
//! entry in `$exception_list` with its `type`, `value`, `mechanism` and `stacktrace` of `raw`
//! frames, a `$exception_fingerprint` that groups it, and `$process_person_profile: false`
//! and `$geoip_disable: true`, so no person profile is made and the address the request came
//! from is not looked up for a place.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::frames::{self, Frame};
use crate::scrub::{self, MESSAGE_CHARS, Scrubber};

/// Which of Ghost's processes or pages it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Daemon,
    Shell,
    Floating,
    Page,
}

impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Shell => "shell",
            Self::Floating => "floating",
            Self::Page => "page",
        }
    }
}

/// The closed list of times Ghost did not work, reported by kind (ADR 0067). Each carries
/// a fixed sentence of Ghost's own and a few coarse facts, never anyone else's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A turn's model call failed and Ghost said so (`ghost_llm::spoken_failure`).
    TurnFailed,
    /// The speech models would not load (ADR 0038's wait, first failure of a start).
    SpeechModelsLoad,
    /// The speech models' download failed (ADR 0066).
    SpeechModelsDownload,
    /// Ghost's sound output stopped playing (`output:unavailable`, #117).
    OutputUnavailable,
    /// The microphone stopped hearing well (ADR 0064).
    InputUnhealthy,
    /// "Update and restart" failed (ADR 0066).
    UpdateFailed,
    /// The shell could not start its daemon.
    DaemonStartFailed,
    /// The shell's daemon exited while it ran.
    DaemonLost,
}

impl FailureKind {
    pub const ALL: [FailureKind; 8] = [
        Self::TurnFailed,
        Self::SpeechModelsLoad,
        Self::SpeechModelsDownload,
        Self::OutputUnavailable,
        Self::InputUnhealthy,
        Self::UpdateFailed,
        Self::DaemonStartFailed,
        Self::DaemonLost,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnFailed => "turn_failed",
            Self::SpeechModelsLoad => "speech_models_load",
            Self::SpeechModelsDownload => "speech_models_download",
            Self::OutputUnavailable => "output_unavailable",
            Self::InputUnhealthy => "input_unhealthy",
            Self::UpdateFailed => "update_failed",
            Self::DaemonStartFailed => "daemon_start_failed",
            Self::DaemonLost => "daemon_lost",
        }
    }

    /// The report's message: Ghost's own words, fixed.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::TurnFailed => "A turn's model call failed",
            Self::SpeechModelsLoad => "The speech models did not load",
            Self::SpeechModelsDownload => "The speech models' download failed",
            Self::OutputUnavailable => "Ghost's sound output stopped playing",
            Self::InputUnhealthy => "The microphone stopped hearing well",
            Self::UpdateFailed => "An update did not install",
            Self::DaemonStartFailed => "The app could not start the daemon",
            Self::DaemonLost => "The daemon exited while the app ran it",
        }
    }
}

/// Which part of Ghost's voice did not load (`FailureKind::SpeechModelsLoad`, #168): a
/// closed set, so a report says which without the loader's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPart {
    /// Kokoro's tokenizer, espeak-ng, and its data.
    Espeak,
    Kokoro,
    Parakeet,
    Onnxruntime,
    Silero,
    SmartTurn,
    Wake,
}

impl SpeechPart {
    pub const ALL: [SpeechPart; 7] = [
        Self::Espeak,
        Self::Kokoro,
        Self::Parakeet,
        Self::Onnxruntime,
        Self::Silero,
        Self::SmartTurn,
        Self::Wake,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Espeak => "espeak",
            Self::Kokoro => "kokoro",
            Self::Parakeet => "parakeet",
            Self::Onnxruntime => "onnxruntime",
            Self::Silero => "silero",
            Self::SmartTurn => "smart_turn",
            Self::Wake => "wake",
        }
    }
}

/// What was wrong with espeak-ng, in a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspeakKind {
    DataMissing,
    DataVersionMismatch,
    DataUnreadable,
    /// The library itself is not there. The daemon links it, so that one stops the dynamic
    /// loader before `main`; only the app sees it, as the daemon's exit (code 127).
    LibraryMissing,
}

impl EspeakKind {
    pub const ALL: [EspeakKind; 4] = [
        Self::DataMissing,
        Self::DataVersionMismatch,
        Self::DataUnreadable,
        Self::LibraryMissing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DataMissing => "data_missing",
            Self::DataVersionMismatch => "data_version_mismatch",
            Self::DataUnreadable => "data_unreadable",
            Self::LibraryMissing => "library_missing",
        }
    }
}

/// The facts that tell one failure of a kind from another in error tracking: they join the
/// fingerprint, so an espeak failure and a Parakeet one are two issues, not one.
const GROUPING_FACTS: [&str; 2] = ["part", "espeak"];

/// A failure as the code reports it: the kind, the Rust error's type where it is Ghost's
/// own (`ghost_llm::LlmError::Status`), and coarse facts.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub error: Option<&'static str>,
    pub facts: BTreeMap<&'static str, String>,
}

static FACT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:/\-]{1,48}$").unwrap());

impl Failure {
    pub fn new(kind: FailureKind) -> Self {
        Self {
            kind,
            error: None,
            facts: BTreeMap::new(),
        }
    }

    pub fn error(mut self, type_name: &'static str) -> Self {
        self.error = Some(type_name);
        self
    }

    /// Which part of the voice failed: a closed word.
    pub fn speech_part(mut self, part: SpeechPart) -> Self {
        self.facts.insert("part", part.as_str().to_string());
        self
    }

    /// What was wrong with espeak-ng: a closed word.
    pub fn espeak(mut self, kind: EspeakKind) -> Self {
        self.facts.insert("espeak", kind.as_str().to_string());
        self
    }

    /// A fact: a short word (a provider's name, `5xx`, `signal`). Anything else is `other`,
    /// so no sentence from anywhere rides in a fact.
    pub fn fact(mut self, key: &'static str, value: impl AsRef<str>) -> Self {
        let v = value.as_ref();
        // A path is never a fact: a model's id may have a `/`, a folder starts with one.
        let path = v.starts_with(['/', '~', '.']);
        let v = if FACT.is_match(v) && !path {
            v
        } else {
            "other"
        };
        self.facts.insert(key, v.to_string());
        self
    }
}

/// A word as a fact keeps it: itself when it passes the fact filter, else `other`.
pub fn fact_word(v: &str) -> String {
    if FACT.is_match(v) && !v.starts_with(['/', '~', '.']) {
        v.to_string()
    } else {
        "other".to_string()
    }
}

/// An HTTP status as its class: `4xx`, `5xx`.
pub fn status_class(status: u16) -> String {
    format!("{}xx", status / 100)
}

/// Where a page's error was caught.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSource {
    /// React's error boundary.
    Boundary,
    /// `window.onerror`.
    Error,
    /// `unhandledrejection`.
    Rejection,
}

/// What a page hands the shell, and nothing more: the shell builds the rest.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageError {
    /// `main` or `floating`.
    pub page: String,
    pub source: PageSource,
    /// The error's constructor name (`TypeError`), or `string`/`object` for a thrown value
    /// that is not an Error.
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub stack: String,
}

pub const PAGE_TYPE_CHARS: usize = 64;
pub const PAGE_MESSAGE_CHARS: usize = 2_000;
pub const PAGE_STACK_CHARS: usize = 16_000;

static JS_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_$][A-Za-z0-9_$.]{0,63}$").unwrap());
/// The engine's own errors: their message is the engine's about Ghost's code.
const JS_BUILTIN: [&str; 6] = [
    "TypeError",
    "ReferenceError",
    "RangeError",
    "SyntaxError",
    "EvalError",
    "URIError",
];

impl PageError {
    /// The webview is not trusted to build a report: refuse what is not the shape.
    pub fn check(&self) -> Result<(), String> {
        if !matches!(self.page.as_str(), "main" | "floating") {
            return Err("page is main or floating".into());
        }
        if self.error_type.len() > PAGE_TYPE_CHARS || !JS_TYPE.is_match(&self.error_type) {
            return Err("type is an error's name".into());
        }
        if self.message.len() > PAGE_MESSAGE_CHARS {
            return Err("message is too long".into());
        }
        if self.stack.len() > PAGE_STACK_CHARS {
            return Err("stack is too long".into());
        }
        Ok(())
    }
}

/// What fails, as the report carries it.
#[derive(Debug, Clone, PartialEq)]
pub enum What {
    Panic {
        message: String,
        /// `file:line:col` of `PanicHookInfo::location`.
        file: Option<String>,
        line: Option<u32>,
        col: Option<u32>,
        thread: Option<String>,
        backtrace: String,
    },
    Page(PageError),
    Failure(Failure),
    /// `ghostd reports test`.
    Test,
}

/// This copy of Ghost and the computer, in coarse words. Read once per process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub version: String,
    pub component: Component,
    pub os_name: String,
    pub os_version: String,
    /// `x11`, `wayland`, `tty` or `unknown`.
    pub session_type: String,
    pub arch: String,
    /// `deb`, `appimage` or `checkout`.
    pub installed_as: String,
    /// The page scripts' chunk ids, from the bundle's manifest: a release build's only.
    #[serde(skip)]
    pub page_chunks: Option<crate::chunks::PageChunks>,
}

impl Context {
    pub fn detect(version: &str, component: Component) -> Self {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let (name, ver) = os_release(&release);
        Self {
            version: version.to_string(),
            component,
            os_name: name,
            os_version: ver,
            session_type: session_type(std::env::var("XDG_SESSION_TYPE").ok().as_deref()),
            arch: std::env::consts::ARCH.to_string(),
            installed_as: {
                let exe = std::env::current_exe().ok();
                let appimage = runs_from_appimage(
                    std::env::var_os("APPIMAGE").as_deref(),
                    std::env::var_os("APPDIR").as_deref(),
                    exe.as_deref(),
                );
                installed_as(appimage, exe.as_deref()).to_string()
            },
            page_chunks: None,
        }
    }
}

fn word(s: &str, max: usize) -> String {
    let w: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || " ._-:".contains(*c))
        .take(max)
        .collect();
    if w.trim().is_empty() {
        "unknown".into()
    } else {
        w.trim().to_string()
    }
}

/// `NAME` and `VERSION_ID` of `/etc/os-release`.
pub fn os_release(text: &str) -> (String, String) {
    let get = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    (word(&get("NAME"), 32), word(&get("VERSION_ID"), 16))
}

pub fn session_type(v: Option<&str>) -> String {
    match v {
        Some(t @ ("x11" | "wayland" | "tty")) => t.to_string(),
        _ => "unknown".to_string(),
    }
}

/// Whether the binary at `exe` runs from its own AppImage: `$APPIMAGE` is set and the binary
/// is inside `$APPDIR`. A shell inside another program's AppImage (an editor's terminal)
/// hands both to everything it starts, and a .deb's or a checkout's Ghost run from it is not
/// an AppImage. The rule is `ghost_core::appimage::own`'s, kept here because this crate does
/// not use that one; `crates/ghostd/tests/appimage.rs` holds the two together.
pub fn runs_from_appimage(
    appimage: Option<&std::ffi::OsStr>,
    appdir: Option<&std::ffi::OsStr>,
    exe: Option<&std::path::Path>,
) -> bool {
    let (Some(_), Some(dir), Some(exe)) = (
        appimage.filter(|a| !a.is_empty()),
        appdir.filter(|d| !d.is_empty()),
        exe,
    ) else {
        return false;
    };
    let real = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = real(std::path::Path::new(dir));
    dir.is_absolute() && dir.parent().is_some() && real(exe).starts_with(&dir)
}

/// How this copy was installed: an AppImage is one its binary runs from
/// ([`runs_from_appimage`]); the .deb puts the binaries under `/usr`; anything else is a
/// checkout's build.
pub fn installed_as(appimage: bool, exe: Option<&std::path::Path>) -> &'static str {
    if appimage {
        "appimage"
    } else if exe.is_some_and(|p| p.starts_with("/usr/")) {
        "deb"
    } else {
        "checkout"
    }
}

/// A report, ready to be spooled: the event without the project's key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Built {
    pub fingerprint: String,
    pub event: Value,
}

/// The report's exception: type, value, handled, frames, and the facts beside it.
struct Exception {
    kind: &'static str,
    error_type: String,
    value: String,
    handled: bool,
    frames: Vec<Frame>,
    facts: BTreeMap<String, String>,
    /// What else separates this report's group: a failure's error type and grouping facts.
    group: String,
}

fn exception(what: &What, ctx: &Context, scrub: &Scrubber) -> Exception {
    match what {
        What::Panic {
            message,
            file,
            line,
            col,
            thread,
            backtrace,
        } => {
            let (file, own) = file
                .as_deref()
                .map(frames::rust_file)
                .unwrap_or_else(|| (String::new(), false));
            // A panic outside Ghost's own code is reduced to its type: its message is a
            // dependency's, and may carry anything.
            let value = if own {
                scrub.text(&scrub::head(message), MESSAGE_CHARS)
            } else {
                String::new()
            };
            let mut frames = frames::rust(backtrace, scrub);
            // A binary without line tables: the panic's own location is the nearest frame's.
            if let Some(last) = frames.last_mut()
                && last.filename.is_none()
                && !file.is_empty()
            {
                last.filename = Some(scrub.text(&file, scrub::FRAME_CHARS));
                last.lineno = *line;
                last.colno = *col;
                last.in_app = own;
            }
            if frames.is_empty() && !file.is_empty() {
                frames.push(Frame {
                    platform: "custom".into(),
                    lang: Some("rust".into()),
                    function: "?".into(),
                    filename: Some(scrub.text(&file, scrub::FRAME_CHARS)),
                    lineno: *line,
                    colno: *col,
                    in_app: own,
                    resolved: Some(true),
                    chunk_id: None,
                });
            }
            let mut facts = BTreeMap::new();
            if !file.is_empty() {
                facts.insert(
                    "location".into(),
                    scrub.text(
                        &format!("{file}:{}:{}", line.unwrap_or(0), col.unwrap_or(0)),
                        scrub::FRAME_CHARS,
                    ),
                );
            }
            if let Some(t) = thread {
                facts.insert("thread".into(), scrub.text(&word(t, 32), 32));
            }
            Exception {
                kind: "panic",
                error_type: "panic".into(),
                value,
                handled: false,
                frames,
                facts,
                group: String::new(),
            }
        }
        What::Page(p) => {
            let value = if JS_BUILTIN.contains(&p.error_type.as_str()) {
                scrub.text(&scrub::head(&p.message), MESSAGE_CHARS)
            } else {
                String::new()
            };
            let mut facts = BTreeMap::new();
            facts.insert("page".into(), p.page.clone());
            facts.insert(
                "caught_by".into(),
                match p.source {
                    PageSource::Boundary => "boundary",
                    PageSource::Error => "onerror",
                    PageSource::Rejection => "unhandledrejection",
                }
                .into(),
            );
            Exception {
                kind: "page",
                error_type: p.error_type.clone(),
                value,
                handled: p.source == PageSource::Boundary,
                frames: frames::javascript(&p.stack, scrub, ctx.page_chunks.as_ref()),
                facts,
                group: String::new(),
            }
        }
        What::Failure(f) => {
            let mut facts: BTreeMap<String, String> = f
                .facts
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect();
            if let Some(e) = f.error {
                facts.insert("error".into(), word(e, 96));
            }
            let group = std::iter::once(facts.get("error").cloned().unwrap_or_default())
                .chain(
                    GROUPING_FACTS
                        .iter()
                        .map(|k| facts.get(*k).cloned().unwrap_or_default()),
                )
                .collect::<Vec<_>>()
                .join("|");
            Exception {
                kind: "failure",
                error_type: f.kind.as_str().into(),
                value: f.kind.sentence().into(),
                handled: true,
                frames: Vec::new(),
                facts,
                group,
            }
        }
        What::Test => Exception {
            kind: "test",
            error_type: "test".into(),
            value: "A test report from ghostd reports test".into(),
            handled: true,
            frames: Vec::new(),
            facts: BTreeMap::new(),
            group: String::new(),
        },
    }
}

/// The event for one report, with `distinct_id` the install's random id.
pub fn build(
    what: &What,
    ctx: &Context,
    install_id: &str,
    at: DateTime<Utc>,
    scrub: &Scrubber,
) -> Built {
    let e = exception(what, ctx, scrub);
    let top = e
        .frames
        .iter()
        .rev()
        .find(|f| f.in_app)
        .map(|f| f.function.clone())
        .unwrap_or_default();
    let fingerprint = {
        let mut h = Sha256::new();
        for part in [
            ctx.component.as_str(),
            e.kind,
            &e.error_type,
            &e.value,
            &top,
        ] {
            h.update(part.as_bytes());
            h.update([0]);
        }
        // Only where there is one, so the groups reports already have keep their prints.
        if !e.group.trim_matches('|').is_empty() {
            h.update(e.group.as_bytes());
            h.update([0]);
        }
        hex::encode(&h.finalize()[..8])
    };
    let mut stack = json!({ "type": "raw", "frames": e.frames });
    if e.frames.is_empty() {
        stack = Value::Null;
    }
    let mut entry = json!({
        "type": e.error_type,
        "value": e.value,
        "mechanism": { "handled": e.handled, "synthetic": false, "type": "generic" },
    });
    if !stack.is_null() {
        entry["stacktrace"] = stack;
    }
    let event = json!({
        "event": "$exception",
        "distinct_id": install_id,
        "timestamp": at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "properties": {
            "$exception_list": [entry],
            "$exception_fingerprint": fingerprint,
            "$process_person_profile": false,
            "$geoip_disable": true,
            "$lib": "ghost-reports",
            "$lib_version": ctx.version,
            "ghost_kind": e.kind,
            "ghost_version": ctx.version,
            "ghost_component": ctx.component.as_str(),
            "ghost_facts": e.facts,
            "os_name": ctx.os_name,
            "os_version": ctx.os_version,
            "session_type": ctx.session_type,
            "arch": ctx.arch,
            "installed_as": ctx.installed_as,
        }
    });
    Built { fingerprint, event }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            version: "2026.11.1".into(),
            component: Component::Daemon,
            os_name: "Ubuntu".into(),
            os_version: "24.04".into(),
            session_type: "wayland".into(),
            arch: "x86_64".into(),
            installed_as: "deb".into(),
            page_chunks: None,
        }
    }

    fn s() -> Scrubber {
        Scrubber::new(
            Some("/home/alice".into()),
            Some("alice".into()),
            Some("desk".into()),
        )
    }

    fn at() -> DateTime<Utc> {
        "2026-11-14T10:00:00Z".parse().unwrap()
    }

    fn panic(message: &str, file: &str) -> What {
        What::Panic {
            message: message.into(),
            file: Some(file.into()),
            line: Some(7),
            col: Some(3),
            thread: Some("tokio-runtime-worker".into()),
            backtrace: String::new(),
        }
    }

    #[test]
    fn the_event_has_posthogs_exception_shape() {
        let b = build(
            &panic("index out of bounds", "crates/ghostd/src/x.rs"),
            &ctx(),
            "id-1",
            at(),
            &s(),
        );
        let e = &b.event;
        assert_eq!(e["event"], "$exception");
        assert_eq!(e["distinct_id"], "id-1");
        assert_eq!(e["timestamp"], "2026-11-14T10:00:00.000Z");
        let p = &e["properties"];
        assert_eq!(p["$process_person_profile"], false);
        assert_eq!(p["$geoip_disable"], true);
        assert_eq!(p["$exception_fingerprint"], b.fingerprint.as_str());
        let x = &p["$exception_list"][0];
        assert_eq!(x["type"], "panic");
        assert_eq!(x["value"], "index out of bounds");
        assert_eq!(x["mechanism"]["handled"], false);
        assert_eq!(x["mechanism"]["synthetic"], false);
        assert_eq!(x["stacktrace"]["type"], "raw");
        let f = &x["stacktrace"]["frames"][0];
        assert_eq!(f["platform"], "custom");
        assert_eq!(f["lang"], "rust");
        assert_eq!(f["filename"], "crates/ghostd/src/x.rs");
        assert_eq!(f["lineno"], 7);
        for k in [
            "ghost_version",
            "ghost_component",
            "os_name",
            "os_version",
            "session_type",
            "arch",
            "installed_as",
        ] {
            assert!(p[k].is_string(), "{k}");
        }
        // No key in what is spooled or kept: the sender adds it.
        assert!(e.get("api_key").is_none());
    }

    #[test]
    fn a_panic_message_with_a_home_path_and_a_value_keeps_neither() {
        let b = build(
            &panic(
                "can't read /home/alice/.config/ghost/apps.json: the app said \"Billing notice\"",
                "crates/ghostd/src/x.rs",
            ),
            &ctx(),
            "id",
            at(),
            &s(),
        );
        let all = b.event.to_string();
        assert!(!all.contains("alice"), "{all}");
        assert!(!all.contains("Billing"), "{all}");
        assert_eq!(
            b.event["properties"]["$exception_list"][0]["value"],
            "can't read ~/.config/ghost/apps.json"
        );
    }

    #[test]
    fn a_panic_in_a_dependency_is_reduced_to_its_type() {
        let b = build(
            &panic(
                "the server said: your token xyz is wrong",
                "/home/runner/.cargo/registry/src/index.crates.io-1/reqwest-0.13.0/src/a.rs",
            ),
            &ctx(),
            "id",
            at(),
            &s(),
        );
        assert_eq!(b.event["properties"]["$exception_list"][0]["value"], "");
        assert!(!b.event.to_string().contains("server said"));
    }

    #[test]
    fn a_page_error_not_of_the_engines_keeps_only_its_type() {
        let p = PageError {
            page: "main".into(),
            source: PageSource::Rejection,
            error_type: "Error".into(),
            message: "github said: Bad credentials for alice@example.com".into(),
            stack: String::new(),
        };
        let b = build(&What::Page(p), &ctx(), "id", at(), &s());
        let x = &b.event["properties"]["$exception_list"][0];
        assert_eq!(x["type"], "Error");
        assert_eq!(x["value"], "");
        assert!(!b.event.to_string().contains("credentials"));
    }

    #[test]
    fn an_engine_error_keeps_its_head_scrubbed() {
        let p = PageError {
            page: "floating".into(),
            source: PageSource::Error,
            error_type: "SyntaxError".into(),
            message: "JSON Parse error: Unexpected identifier \"transcript words\"".into(),
            stack: String::new(),
        };
        let b = build(&What::Page(p), &ctx(), "id", at(), &s());
        assert_eq!(
            b.event["properties"]["$exception_list"][0]["value"],
            "JSON Parse error"
        );
        assert_eq!(b.event["properties"]["ghost_facts"]["page"], "floating");
    }

    #[test]
    fn a_page_error_of_the_wrong_shape_is_refused() {
        let ok = PageError {
            page: "main".into(),
            source: PageSource::Error,
            error_type: "TypeError".into(),
            message: String::new(),
            stack: String::new(),
        };
        assert!(ok.check().is_ok());
        let mut bad = ok.clone();
        bad.page = "phone".into();
        assert!(bad.check().is_err());
        let mut bad = ok.clone();
        bad.error_type = "Type Error <script>".into();
        assert!(bad.check().is_err());
        let mut bad = ok.clone();
        bad.message = "x".repeat(PAGE_MESSAGE_CHARS + 1);
        assert!(bad.check().is_err());
        let mut bad = ok.clone();
        bad.stack = "x".repeat(PAGE_STACK_CHARS + 1);
        assert!(bad.check().is_err());
        // A page cannot add fields of its own.
        let extra: Result<PageError, _> = serde_json::from_value(serde_json::json!({
            "page": "main", "source": "error", "type": "Error", "distinct_id": "x"
        }));
        assert!(extra.is_err());
    }

    #[test]
    fn a_failure_carries_its_kind_and_coarse_facts_only() {
        let f = Failure::new(FailureKind::TurnFailed)
            .error("ghost_llm::LlmError::Status")
            .fact("provider", "openrouter")
            .fact("status", status_class(503))
            .fact("detail", "Your account alice@example.com is over quota");
        let b = build(&What::Failure(f), &ctx(), "id", at(), &s());
        let p = &b.event["properties"];
        assert_eq!(p["$exception_list"][0]["type"], "turn_failed");
        assert_eq!(
            p["$exception_list"][0]["value"],
            "A turn's model call failed"
        );
        assert_eq!(p["ghost_facts"]["provider"], "openrouter");
        assert_eq!(p["ghost_facts"]["status"], "5xx");
        assert_eq!(p["ghost_facts"]["detail"], "other");
        assert_eq!(p["ghost_facts"]["error"], "ghost_llm::LlmError::Status");
    }

    #[test]
    fn the_same_failure_has_the_same_fingerprint() {
        let a = build(&What::Test, &ctx(), "a", at(), &s());
        let b = build(&What::Test, &ctx(), "b", at(), &s());
        assert_eq!(a.fingerprint, b.fingerprint);
        let c = build(
            &What::Failure(Failure::new(FailureKind::DaemonLost)),
            &ctx(),
            "a",
            at(),
            &s(),
        );
        assert_ne!(a.fingerprint, c.fingerprint);
    }

    #[test]
    fn a_speech_load_failure_is_grouped_by_its_part_and_kind() {
        // #168: an espeak failure, a Parakeet one and ONNX Runtime's are separate issues.
        let print = |f: Failure| build(&What::Failure(f), &ctx(), "id", at(), &s()).fingerprint;
        let load = || Failure::new(FailureKind::SpeechModelsLoad);
        let espeak_missing = print(
            load()
                .error("EspeakData")
                .speech_part(SpeechPart::Espeak)
                .espeak(EspeakKind::DataMissing),
        );
        let espeak_version = print(
            load()
                .error("EspeakData")
                .speech_part(SpeechPart::Espeak)
                .espeak(EspeakKind::DataVersionMismatch),
        );
        let parakeet = print(load().error("Model").speech_part(SpeechPart::Parakeet));
        let runtime = print(load().error("Runtime").speech_part(SpeechPart::Onnxruntime));
        let all = [&espeak_missing, &espeak_version, &parakeet, &runtime];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // The same failure again is the same issue.
        assert_eq!(
            parakeet,
            print(load().error("Model").speech_part(SpeechPart::Parakeet))
        );
        // A failure with no grouping facts keeps the print it always had.
        let plain = build(&What::Failure(load()), &ctx(), "id", at(), &s()).fingerprint;
        let mut h = Sha256::new();
        for part in [
            "daemon",
            "failure",
            "speech_models_load",
            load().kind.sentence(),
            "",
        ] {
            h.update(part.as_bytes());
            h.update([0]);
        }
        assert_eq!(plain, hex::encode(&h.finalize()[..8]));
    }

    #[test]
    fn a_speech_load_failure_carries_closed_words_only_whatever_the_loader_said() {
        // The loader's text names paths, a home folder and anything a file held; none of it
        // may ride in a fact, the error or the value.
        let hostile = "/home/alice/.cache/ghost/kokoro: Wrong version of espeak-ng-data at \
                       '/home/alice/espeak-ng-data' (call me at 555-0100, alice@example.com)";
        let f = Failure::new(FailureKind::SpeechModelsLoad)
            .error("EspeakData")
            .speech_part(SpeechPart::Espeak)
            .espeak(EspeakKind::DataVersionMismatch)
            .fact("detail", hostile)
            .fact("path", "/home/alice");
        let b = build(&What::Failure(f), &ctx(), "id", at(), &s());
        let p = &b.event["properties"];
        let text = b.event.to_string();
        for leak in [
            "alice",
            "/home",
            "espeak-ng-data'",
            "555-0100",
            "Wrong version",
        ] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        assert_eq!(p["ghost_facts"]["part"], "espeak");
        assert_eq!(p["ghost_facts"]["espeak"], "data_version_mismatch");
        assert_eq!(p["ghost_facts"]["error"], "EspeakData");
        assert_eq!(p["ghost_facts"]["detail"], "other");
        assert_eq!(
            p["$exception_list"][0]["value"],
            "The speech models did not load"
        );
        // Every closed word is a fact the filter would pass on its own.
        for part in SpeechPart::ALL {
            assert!(FACT.is_match(part.as_str()) && !part.as_str().contains('/'));
        }
        for kind in EspeakKind::ALL {
            assert!(FACT.is_match(kind.as_str()) && !kind.as_str().contains('/'));
        }
    }

    #[test]
    fn coarse_facts_about_the_computer() {
        assert_eq!(
            os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"x\""),
            ("Ubuntu".into(), "24.04".into())
        );
        assert_eq!(session_type(Some("wayland")), "wayland");
        assert_eq!(session_type(Some("mir")), "unknown");
        assert_eq!(
            installed_as(false, Some(std::path::Path::new("/usr/bin/ghostd"))),
            "deb"
        );
        assert_eq!(installed_as(true, None), "appimage");
        // The AppImage is this copy's own only when its binary runs from the mount: an
        // editor that is an AppImage hands its two variables to a .deb's ghostd too.
        use std::ffi::OsStr;
        use std::path::Path;
        let editor = (
            Some(OsStr::new("/home/a/Applications/T3-Code.AppImage")),
            Some(OsStr::new("/tmp/.mount_T3-CodAbCdEf")),
        );
        for exe in ["/usr/bin/ghostd", "/home/a/ghost/target/debug/ghostd"] {
            assert!(!runs_from_appimage(
                editor.0,
                editor.1,
                Some(Path::new(exe))
            ));
        }
        let ghost = Some(OsStr::new("/tmp/.mount_GhostAb12Cd"));
        let image = Some(OsStr::new("/home/a/Ghost.AppImage"));
        let inside = Path::new("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd");
        assert!(runs_from_appimage(image, ghost, Some(inside)));
        assert!(!runs_from_appimage(None, ghost, Some(inside)));
        assert!(!runs_from_appimage(image, None, Some(inside)));
        assert!(!runs_from_appimage(
            image,
            Some(OsStr::new("/")),
            Some(inside)
        ));
        assert!(!runs_from_appimage(
            image,
            Some(OsStr::new("/tmp/.mount_Ghost")),
            Some(inside)
        ));
        assert!(!runs_from_appimage(image, ghost, None));
        assert_eq!(
            installed_as(
                false,
                Some(std::path::Path::new("/home/a/ghost/target/debug/ghostd"))
            ),
            "checkout"
        );
    }
}
