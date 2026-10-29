//! Pause wake detection while nerd-dictation (F9) is active.

use std::path::Path;
use std::process::{Command, Stdio};

use ghost_core::redact::child_env;

/// The toggle script's flag file, or a `nerd-dictation begin` process.
pub fn is_dictating(flag: &Path) -> bool {
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
    fn dictation_flag() {
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("dictation-audio-duck.state");
        assert!(!is_dictating(&flag));
        std::fs::write(&flag, "sink\tno\n").unwrap();
        assert!(is_dictating(&flag));
    }
}
