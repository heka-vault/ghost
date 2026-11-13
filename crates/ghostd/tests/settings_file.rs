//! `ghostd` reads `settings.env` itself (2026-11-13): a plain `ghostd run`, `check`, `llm`,
//! `passwords` or `listening` from a shell gets the settings the launchers give it, with
//! what the shell exported winning over the file, and a launcher that loaded the file says so
//! and is not loaded over (`ghost_core::environment::settings_file_under`).
//!
//! Isolated from the operator: a cleared environment, a temp home, and no bus.

use std::path::Path;
use std::process::{Command, Output};

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");

fn ghostd(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(GHOSTD);
    cmd.args(args)
        .env_clear()
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", dir.join("rt"))
        .env("GHOST_SETTINGS_PATH", dir.join("settings.env"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/ghost-test-bus",
        );
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn said(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn setup() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("rt")).unwrap();
    std::fs::write(
        tmp.path().join("settings.env"),
        format!(
            "# saved by Ghost\nGHOST_API_SOCKET={}\nGHOST_DATA_DIR=\"{}\"\nANTHROPIC_API_KEY=sk-not-a-setting-0123456789\n",
            tmp.path().join("from-file.sock").display(),
            tmp.path().join("data-from-file").display(),
        ),
    )
    .unwrap();
    tmp
}

#[test]
fn a_command_started_by_hand_reads_settings_env() {
    let tmp = setup();
    // The control socket's address, from the file: `listening` and `status` look there.
    for args in [&["listening", "on"][..], &["status"][..]] {
        let o = ghostd(tmp.path(), args, &[]);
        assert!(!o.status.success());
        let out = said(&o);
        assert!(out.contains("from-file.sock"), "{args:?}: {out}");
    }
    // The vault's folder, from the file: `passwords` reads the vault there.
    let o = ghostd(tmp.path(), &["passwords", "list"], &[]);
    let out = said(&o);
    assert!(out.contains("data-from-file/passwords.vault"), "{out}");
    // A password's name in the file is not a setting, and its value never printed.
    assert!(
        out.contains("ANTHROPIC_API_KEY") && out.contains("password"),
        "{out}"
    );
    assert!(!out.contains("sk-not-a-setting"), "{out}");
}

#[test]
fn what_the_shell_exported_wins_over_the_file() {
    let tmp = setup();
    let mine = tmp.path().join("exported.sock");
    let o = ghostd(
        tmp.path(),
        &["status"],
        &[("GHOST_API_SOCKET", mine.to_str().unwrap())],
    );
    let out = said(&o);
    assert!(out.contains("exported.sock"), "{out}");
    assert!(!out.contains("from-file.sock"), "{out}");
}

#[test]
fn a_launcher_that_loaded_the_file_is_not_loaded_over() {
    let tmp = setup();
    let o = ghostd(tmp.path(), &["status"], &[("GHOST_SETTINGS_LOADED", "1")]);
    let out = said(&o);
    assert!(!out.contains("from-file.sock"), "{out}");
    assert!(out.contains("rt/ghost/api.sock"), "{out}");
}
