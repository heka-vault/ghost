//! Pause wake detection while nerd-dictation (F9) is active: the operator's own outside
//! dictation tool, which types through `ydotool`. Not Ghost's own dictation (ADR 0077,
//! `ghostd::dictation`), which takes the microphone from the passive loop itself; this only
//! notices the other tool so its words never wake Ghost.

use std::path::Path;
use std::process::{Command, Stdio};

use ghost_core::redact::child_env;

/// The toggle script's flag file, or a `nerd-dictation begin` process.
pub fn running(flag: &Path) -> bool {
    if flag.exists() {
        return true;
    }
    // `[/]` so the pattern never matches a pgrep's own command line: two of these at the
    // same moment (the passive loop and a test, or two daemons under test) would each see
    // the other and say yes.
    Command::new("pgrep")
        .args(["-f", "[/]nerd-dictation begin"])
        .env_clear()
        .envs(child_env(std::env::vars()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nerd_dictation_flag() {
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("dictation-audio-duck.state");
        assert!(!running(&flag));
        std::fs::write(&flag, "sink\tno\n").unwrap();
        assert!(running(&flag));
    }
}
