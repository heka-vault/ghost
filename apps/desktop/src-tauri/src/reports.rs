//! Error reports in the desktop app (ADR 0067): the shell's and the floating room's panics,
//! the pages' uncaught errors, and the app's own failures by kind (the daemon would not
//! start, the daemon exited, an update did not install).
//!
//! A page never builds a report: it hands the shell the error's type, message and stack
//! through `report_page_error`, the shell refuses what is not that shape, and
//! `ghost_reports` scrubs, builds, spools and sends, as it does for the daemon. The switch
//! is the same file the daemon reads; the window turns it on and off through the daemon's
//! control API (`PUT /reports/choice`), like every other setting.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghost_reports::{Component, Failure, FailureKind, PageError, Reporter, Scrubber, What};

/// At most this many page errors a minute reach the reporter from one process: a page in
/// a loop of errors costs nothing past it.
const PAGE_PER_MINUTE: usize = 10;

static PAGE_TIMES: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// This process's reporter, installed with its panic hook. The sender runs on Tauri's
/// runtime when there is a key; with none (a build from the checkout) nothing is made.
pub fn start(component: Component) -> Arc<Reporter> {
    let r = Arc::new(Reporter::from_env(
        env!("CARGO_PKG_VERSION"),
        component,
        Scrubber::from_env(),
    ));
    ghost_reports::install(r.clone());
    ghost_reports::panic::install(r.clone());
    if r.available() {
        tauri::async_runtime::spawn(r.clone().run());
    }
    r
}

/// The bundle's manifest of page scripts and their chunk ids (`report-chunks.json`, written by
/// the release build: `tools/report_chunks.py`), so a page's frames resolve to TypeScript in
/// PostHog. Read from the app's own embedded assets, never from a page; a build without one
/// (`make dev`, `make bundle`) sends minified frames, as before. A manifest that is not the
/// shape is refused whole and said once in the log.
pub fn load_page_chunks<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(r) = ghost_reports::global() else {
        return;
    };
    if !r.available() {
        return;
    }
    let Some(asset) = app
        .asset_resolver()
        .get(ghost_reports::chunks::FILE.to_string())
    else {
        return;
    };
    match ghost_reports::chunks::PageChunks::parse(asset.bytes()) {
        Ok(c) => r.set_page_chunks(c),
        Err(e) => eprintln!("ghost: error reports: {e}"),
    }
}

/// The page's uncaught error, from the main window or the floating room.
#[tauri::command]
pub fn report_page_error(error: PageError) -> Result<(), String> {
    accept(&error, Instant::now())?;
    if let Some(r) = ghost_reports::global() {
        r.report(&What::Page(error));
    }
    Ok(())
}

fn accept(error: &PageError, now: Instant) -> Result<(), String> {
    error.check()?;
    let mut times = PAGE_TIMES.lock().unwrap_or_else(|e| e.into_inner());
    times.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if times.len() >= PAGE_PER_MINUTE {
        return Err("too many errors this minute".into());
    }
    times.push(now);
    Ok(())
}

/// The daemon's exit, as the supervisor words it (`exit status: 101`, `signal: 6 (SIGABRT)`),
/// as two facts: how, and the number.
pub fn exit_failure(exit: &str) -> Failure {
    let how = if exit.contains("signal") {
        "signal"
    } else if exit.contains("status") || exit.contains("code") {
        "code"
    } else {
        "other"
    };
    let number: String = exit
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .chars()
        .take(4)
        .collect();
    let f = Failure::new(FailureKind::DaemonLost).fact("exit", how);
    if number.is_empty() {
        f
    } else {
        f.fact("number", number)
    }
}

/// What the supervisor said, as a failure when it is one.
pub fn on_daemon_event(e: &crate::supervisor::DaemonEvent) {
    use crate::supervisor::DaemonEvent;
    match e {
        DaemonEvent::Failed { .. } => {
            ghost_reports::failure(Failure::new(FailureKind::DaemonStartFailed))
        }
        DaemonEvent::Restarting { exit, .. } => ghost_reports::failure(exit_failure(exit)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_reports::PageSource;

    fn page(message: &str) -> PageError {
        PageError {
            page: "main".into(),
            source: PageSource::Error,
            error_type: "TypeError".into(),
            message: message.into(),
            stack: String::new(),
        }
    }

    #[test]
    fn the_command_refuses_what_is_not_an_error_and_a_flood() {
        let now = Instant::now();
        let mut big = page("");
        big.stack = "x".repeat(ghost_reports::report::PAGE_STACK_CHARS + 1);
        assert!(accept(&big, now).is_err());
        let mut odd = page("");
        odd.page = "settings".into();
        assert!(accept(&odd, now).is_err());
        let ok = (0..PAGE_PER_MINUTE + 5)
            .filter(|_| accept(&page("x"), now).is_ok())
            .count();
        assert_eq!(ok, PAGE_PER_MINUTE);
        assert!(accept(&page("x"), now + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn the_command_takes_only_the_fields_a_page_may_send() {
        let from_page: Result<PageError, _> = serde_json::from_value(serde_json::json!({
            "page": "floating", "source": "rejection", "type": "Error",
            "message": "m", "stack": "s", "properties": {"$set": {"email": "x"}}
        }));
        assert!(from_page.is_err());
    }

    #[test]
    fn a_daemon_exit_is_two_coarse_facts() {
        let f = exit_failure("signal: 6 (SIGABRT) (core dumped)");
        assert_eq!(f.facts["exit"], "signal");
        assert_eq!(f.facts["number"], "6");
        let f = exit_failure("exit status: 101");
        assert_eq!(f.facts["exit"], "code");
        assert_eq!(f.facts["number"], "101");
    }
}
