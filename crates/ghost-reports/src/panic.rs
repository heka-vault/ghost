//! The panic hook: a panic anywhere in the process (any thread, any task) is written to the
//! spool, and the hook that was there before runs as ever (the message and the location in
//! the log). No network here: the running sender, when the process survives the panic (a
//! task's), or the next start sends it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Reporter, What};

/// One report at a time: a panic inside the hook (or two threads at once) is left to the
/// previous hook alone.
static IN_HOOK: AtomicBool = AtomicBool::new(false);

/// Put the hook in front of the one already there.
pub fn install(reporter: Arc<Reporter>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if reporter.on() && !IN_HOOK.swap(true, Ordering::SeqCst) {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            let location = info.location();
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            reporter.report(&What::Panic {
                message,
                file: location.map(|l| l.file().to_string()),
                line: location.map(|l| l.line()),
                col: location.map(|l| l.column()),
                thread: std::thread::current().name().map(str::to_string),
                backtrace,
            });
            IN_HOOK.store(false, Ordering::SeqCst);
        }
        previous(info);
    }));
}
