//! A stack, as PostHog's error tracking takes it: `platform: "custom"` frames with the
//! language, the function, the file, the line and column, and whether the frame is Ghost's
//! own code (`in_app`). Oldest call first, the frame that failed last.
//!
//! A Rust backtrace is read from its printed form (`std::backtrace::Backtrace`'s `Display`,
//! the stable way to its frames): the release profile keeps line tables (`debug = 1`), so a
//! frame has its function and `file:line`. A file is kept relative to the workspace for
//! Ghost's own crates, as `<crate>-<version>/...` for a dependency, as `rustc/...` for the
//! standard library, and as its last part otherwise: never a build machine's or the
//! operator's path.
//!
//! A page's stack (WebKit's `fn@url:line:col`, or V8's `at fn (url:line:col)`) keeps the
//! path of the URL and nothing else of it. The release bundle is minified. A frame in a
//! script the build gave a chunk id ([`PageChunks`]) goes the way posthog-js sends one, so
//! PostHog resolves it against the source map uploaded under that id: `platform:
//! "web:javascript"`, `filename`, `function`, `lineno`, `colno`, `in_app` and `chunk_id`
//! (cymbal's `RawJSFrame`; a filename that is not `http(s)` is never fetched, the id alone
//! names the map). Any other frame goes as `custom`, minified, with the version.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::chunks::PageChunks;
use crate::scrub::{FRAME_CHARS, Scrubber};

/// At most this many frames: the ones nearest the failure.
pub const MAX_FRAMES: usize = 40;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub platform: String,
    /// `custom` frames say their language; a `web:javascript` frame's platform says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineno: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colno: Option<u32>,
    pub in_app: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<bool>,
    /// The uploaded source map this frame resolves against (`web:javascript` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
}

static RUST_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+:\s+(.+?)\s*$").unwrap());
static RUST_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*at\s+(.+?):(\d+)(?::(\d+))?\s*$").unwrap());
static RUST_HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"::h[0-9a-f]{16}$").unwrap());
static REGISTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/registry/src/[^/]+/([^/]+/.+)$").unwrap());
static GIT_DEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/git/checkouts/([^/]+)/[^/]+/(.+)$").unwrap());
static RUSTC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/rustc/[0-9a-f]+/(.+)$").unwrap());
static OWN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|/)((?:crates|apps/desktop/src-tauri)/.+)$").unwrap());

/// Machinery between the failure and the panic hook: not where anything went wrong.
const PANIC_MACHINERY: [&str; 9] = [
    "std::backtrace",
    "std::panicking",
    "core::panicking",
    "std::panic::",
    "rust_begin_unwind",
    "ghost_reports::",
    "__rustc::",
    "<alloc::boxed::Box<F,A> as core::ops::function::Fn",
    "std::sys::backtrace",
];

/// A Rust file path → (what is kept of it, whether it is Ghost's own).
pub fn rust_file(path: &str) -> (String, bool) {
    let path = path.trim_start_matches("./");
    if let Some(c) = REGISTRY.captures(path) {
        return (c[1].to_string(), false);
    }
    if let Some(c) = GIT_DEP.captures(path) {
        return (format!("{}/{}", &c[1], &c[2]), false);
    }
    if let Some(c) = RUSTC.captures(path) {
        return (format!("rustc/{}", &c[1]), false);
    }
    if !path.starts_with('/') && (path.starts_with("crates/") || path.starts_with("apps/")) {
        return (path.to_string(), true);
    }
    if let Some(c) = OWN.captures(path) {
        return (c[1].to_string(), true);
    }
    // Relative: the backtrace printed it from the process's own folder (a checkout's).
    if !path.starts_with('/') {
        return (path.to_string(), true);
    }
    (path.rsplit('/').next().unwrap_or(path).to_string(), false)
}

/// The frames of a printed Rust backtrace.
pub fn rust(backtrace: &str, scrub: &Scrubber) -> Vec<Frame> {
    let mut frames: Vec<Frame> = Vec::new();
    for line in backtrace.lines() {
        if let Some(c) = RUST_AT.captures(line) {
            if let Some(f) = frames.last_mut()
                && f.filename.is_none()
            {
                let (file, own) = rust_file(&c[1]);
                f.filename = Some(scrub.text(&file, FRAME_CHARS));
                f.lineno = c[2].parse().ok();
                f.colno = c.get(3).and_then(|m| m.as_str().parse().ok());
                f.in_app = own;
            }
        } else if let Some(c) = RUST_FN.captures(line) {
            let function = RUST_HASH.replace(&c[1], "").into_owned();
            frames.push(Frame {
                platform: "custom".into(),
                lang: Some("rust".into()),
                function: scrub.text(&function, FRAME_CHARS),
                filename: None,
                lineno: None,
                colno: None,
                in_app: false,
                resolved: Some(true),
                chunk_id: None,
            });
        }
    }
    // Printed newest first: drop the hook's own frames, keep the nearest, oldest first.
    // The machinery is the run at the top; `std::panicking::try` far below (a thread's or
    // a task's catch_unwind) is the caller's, and stays.
    let machinery = |f: &Frame| PANIC_MACHINERY.iter().any(|m| f.function.starts_with(m));
    let leading = frames.iter().take_while(|f| machinery(f)).count();
    let start = frames
        .iter()
        .take(24)
        .rposition(|f| {
            [
                "core::panicking",
                "rust_begin_unwind",
                "__rustc::rust_begin_unwind",
                "std::panicking::begin_panic",
            ]
            .iter()
            .any(|m| f.function.starts_with(m))
        })
        .map_or(leading, |i| (i + 1).max(leading));
    let mut kept: Vec<Frame> = frames.into_iter().skip(start).take(MAX_FRAMES).collect();
    kept.reverse();
    kept
}

static JS_WEBKIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:(.*?)@)?(\S+?):(\d+):(\d+)\s*$").unwrap());
static JS_V8: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*at\s+(?:(.+?)\s+\()?(\S+?):(\d+):(\d+)\)?\s*$").unwrap());

/// A page's script URL → (its path, whether it is the app's own bundle).
fn js_file(url: &str) -> (String, bool) {
    let rest = url.split(['?', '#']).next().unwrap_or(url);
    let path = match rest.split_once("://") {
        Some((_, after)) => after.find('/').map_or("", |i| &after[i..]),
        None => rest,
    };
    let path = path.trim_start_matches('/');
    let own = path.starts_with("assets/") || path.starts_with("src/");
    if own {
        (path.to_string(), true)
    } else {
        (path.rsplit('/').next().unwrap_or(path).to_string(), false)
    }
}

/// The frames of a page's `Error.stack`; `chunks`, the build's manifest, when it has one.
pub fn javascript(stack: &str, scrub: &Scrubber, chunks: Option<&PageChunks>) -> Vec<Frame> {
    let mut frames: Vec<Frame> = stack
        .lines()
        .filter_map(|line| {
            let c = JS_V8.captures(line).or_else(|| JS_WEBKIT.captures(line))?;
            let (file, own) = js_file(&c[2]);
            let function = c
                .get(1)
                .map(|m| m.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("?");
            let chunk_id = chunks.and_then(|m| m.get(&file)).map(str::to_string);
            let known = chunk_id.is_some();
            Some(Frame {
                platform: if known { "web:javascript" } else { "custom" }.into(),
                lang: (!known).then(|| "javascript".into()),
                function: scrub.text(function, FRAME_CHARS),
                filename: Some(scrub.text(&file, FRAME_CHARS)),
                lineno: c[3].parse().ok(),
                colno: c[4].parse().ok(),
                in_app: own,
                resolved: (!known).then_some(false),
                chunk_id,
            })
        })
        .take(MAX_FRAMES)
        .collect();
    frames.reverse();
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> Scrubber {
        Scrubber::new(Some("/home/alice".into()), Some("alice".into()), None)
    }

    const BACKTRACE: &str = "   0: std::backtrace_rs::backtrace::libunwind::trace
             at /rustc/abc123/library/std/src/../../backtrace/src/backtrace/libunwind.rs:116:5
   1: std::panicking::begin_panic_handler
             at /rustc/abc123/library/std/src/panicking.rs:665:5
   2: ghost_reports::panic::hook::{{closure}}::h0123456789abcdef
             at ./crates/ghost-reports/src/panic.rs:20:9
   3: core::panicking::panic_fmt
             at /rustc/abc123/library/core/src/panicking.rs:76:14
   4: ghostd::daemon::Daemon::run::{{closure}}::h0123456789abcdef
             at ./crates/ghostd/src/daemon.rs:123:45
   5: tokio::runtime::task::harness::poll_future
             at /home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.48.0/src/runtime/task/harness.rs:473:19
   6: main
             at /home/alice/build/crates/ghostd/src/main.rs:10:1
";

    #[test]
    fn a_backtrace_keeps_ghosts_frames_and_drops_the_hooks() {
        let f = rust(BACKTRACE, &s());
        let names: Vec<&str> = f.iter().map(|f| f.function.as_str()).collect();
        assert_eq!(
            names,
            [
                "main",
                "tokio::runtime::task::harness::poll_future",
                "ghostd::daemon::Daemon::run::{{closure}}"
            ]
        );
        let last = f.last().unwrap();
        assert_eq!(
            last.filename.as_deref(),
            Some("crates/ghostd/src/daemon.rs")
        );
        assert_eq!((last.lineno, last.colno), (Some(123), Some(45)));
        assert!(last.in_app);
        assert_eq!(
            f[1].filename.as_deref(),
            Some("tokio-1.48.0/src/runtime/task/harness.rs")
        );
        assert!(!f[1].in_app);
        assert_eq!(f[0].filename.as_deref(), Some("crates/ghostd/src/main.rs"));
        let all = serde_json::to_string(&f).unwrap();
        assert!(!all.contains("alice") && !all.contains("/home/"), "{all}");
    }

    #[test]
    fn a_webkit_stack_keeps_the_path_only() {
        let stack = "render@tauri://localhost/assets/index-AbC12.js:1:2345\n\
                     dispatch@https://cdn.example.com/lib/vendor.js?token=secret123:4:5\n\
                     global code@tauri://localhost/assets/index-AbC12.js:9:1";
        let f = javascript(stack, &s(), None);
        assert_eq!(f.len(), 3);
        assert_eq!(f[2].function, "render");
        assert_eq!(f[2].filename.as_deref(), Some("assets/index-AbC12.js"));
        assert!(f[2].in_app);
        assert_eq!(f[1].filename.as_deref(), Some("vendor.js"));
        assert!(!serde_json::to_string(&f).unwrap().contains("secret"));
    }

    #[test]
    fn a_v8_stack_is_read_too() {
        let f = javascript(
            "TypeError: x\n    at Foo (http://localhost:1430/src/app/App.tsx:10:3)\n    at http://localhost:1430/src/main.tsx:2:1",
            &s(),
            None,
        );
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].function, "Foo");
        assert_eq!(f[1].filename.as_deref(), Some("src/app/App.tsx"));
        assert_eq!(f[0].function, "?");
    }

    const CHUNK: &str = "1b7a3c6e-2f1d-5e8a-9b0c-4d3e2f1a0b9c";

    fn chunks() -> PageChunks {
        PageChunks::parse(format!(r#"{{"assets/index-AbC12.js": "{CHUNK}"}}"#).as_bytes()).unwrap()
    }

    #[test]
    fn a_frame_in_a_script_with_a_chunk_id_is_sent_as_posthog_js_sends_it() {
        let stack = "render@tauri://localhost/assets/index-AbC12.js:1:2345\n\
                     helper@tauri://localhost/assets/vendor-Q1.js:4:5";
        let f = javascript(stack, &s(), Some(&chunks()));
        let resolved = serde_json::to_value(&f[1]).unwrap();
        // cymbal's RawJSFrame: platform, filename, function, lineno and colno, in_app, chunk_id.
        assert_eq!(
            resolved,
            serde_json::json!({
                "platform": "web:javascript",
                "filename": "assets/index-AbC12.js",
                "function": "render",
                "lineno": 1,
                "colno": 2345,
                "in_app": true,
                "chunk_id": CHUNK,
            })
        );
        // A script the build gave no id: as before.
        let other = serde_json::to_value(&f[0]).unwrap();
        assert_eq!(other["platform"], "custom");
        assert_eq!(other["lang"], "javascript");
        assert_eq!(other["resolved"], false);
        assert!(other.get("chunk_id").is_none());
    }

    #[test]
    fn without_a_manifest_page_frames_are_minified_as_before() {
        let stack = "render@tauri://localhost/assets/index-AbC12.js:1:2345";
        let f = javascript(stack, &s(), None);
        assert_eq!(
            serde_json::to_value(&f[0]).unwrap(),
            serde_json::json!({
                "platform": "custom",
                "lang": "javascript",
                "function": "render",
                "filename": "assets/index-AbC12.js",
                "lineno": 1,
                "colno": 2345,
                "in_app": true,
                "resolved": false,
            })
        );
    }

    #[test]
    fn a_resolved_frame_is_scrubbed_like_any_other() {
        // A function name a page could make up still passes the scrubber, and the filename
        // stays the bundle's relative path: the origin and the query never go.
        let stack = "/home/alice/x@tauri://localhost/assets/index-AbC12.js?u=alice@example.com:3:4";
        let f = javascript(stack, &s(), Some(&chunks()));
        let all = serde_json::to_string(&f).unwrap();
        assert!(!all.contains("alice") && !all.contains("tauri://"), "{all}");
        assert_eq!(f[0].filename.as_deref(), Some("assets/index-AbC12.js"));
        assert_eq!(f[0].chunk_id.as_deref(), Some(CHUNK));
    }
}
