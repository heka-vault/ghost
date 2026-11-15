//! `ghostd check --json` and the desktop's Run Ghost's checks (ADR 0073 as amended
//! 2026-11-14): the same checks as the terminal's, read as lines with problems first.
//!
//! Isolated from the operator: Ghost's folders in the temp dir and a session bus address
//! that points nowhere, so their files, their vault and their keyring are never reached.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use ghost_api::developer::ChecksPort;

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");
const NO_BUS: &str = "unix:path=/nonexistent/ghost-test-bus";

/// Ghost's folders in `dir`, the socket there, no bus to a keyring, and no Parakeet (the
/// full-precision model takes 2.4 GB and ten seconds to load, which this test doesn't need).
fn isolated(cmd: &mut Command, dir: &Path, socket: &Path) {
    std::fs::create_dir_all(dir.join("config")).unwrap();
    cmd.env("GHOST_API_SOCKET", socket)
        .env("GHOST_DATA_DIR", dir)
        .env("GHOST_STT_DIR", dir.join("no-stt"))
        .env("GHOST_CONFIG_DIR", dir.join("config"))
        // Each of these has a default of its own under the operator's home, whatever the
        // two folders above say: the database most of all, which the daemon opens and
        // migrates at start.
        .env("GHOST_DB_PATH", dir.join("ghost.db"))
        .env("GHOST_POLICY_PATH", dir.join("config/policy.toml"))
        .env("GHOST_SETTINGS_PATH", dir.join("config/settings.env"))
        .env("GHOST_ENV_PATH", dir.join("config/env"))
        .env("GHOST_VAULT_ENV_PATH", dir.join("no-heka"))
        .env("GHOST_CATALOG_PATH", dir.join("cache/catalog.json"))
        .env("GHOST_SKILLS_DIR", dir.join("config/skills"))
        .env("GHOST_HOME_SKILLS_DIR", dir.join("home-skills"))
        // No listener for outside tools on the operator's own port (ADR 0048).
        .env("GHOST_MCP_LISTEN", "")
        .env("DBUS_SESSION_BUS_ADDRESS", NO_BUS)
        .env_remove("GHOST_PARENT_PID");
}

fn env_of(cmd: &Command) -> Vec<(String, String)> {
    cmd.get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .chain(std::env::var("PATH").ok().map(|p| ("PATH".to_string(), p)))
        .chain(std::env::var("HOME").ok().map(|p| ("HOME".to_string(), p)))
        .collect()
}

#[test]
fn check_json_prints_its_lines_and_problems_on_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut check = Command::new(GHOSTD);
    isolated(&mut check, dir.path(), &dir.path().join("api.sock"));
    let out = check.args(["check", "--json"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert!(
        v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with("version:")),
        "{v}"
    );
    assert_eq!(v["code"].as_i64(), out.status.code().map(i64::from), "{v}");
    assert_eq!(
        v["problems"].as_array().unwrap().is_empty(),
        out.status.success(),
        "{v}"
    );
}

#[tokio::test]
async fn the_desktops_button_runs_the_same_checks() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(GHOSTD);
    isolated(&mut cmd, dir.path(), &dir.path().join("api.sock"));
    let env = env_of(&cmd);
    let desk = ghostd::check::ChecksDesk {
        exe: GHOSTD.into(),
        env: Arc::new(move || env.clone()),
    };
    let v = desk.run().await.unwrap();
    assert_eq!(v.ok, v.code == 0);
    assert!(
        v.lines.iter().any(|l| l.text.starts_with("version:")),
        "{v:?}"
    );
    // Problems first.
    let first_plain = v
        .lines
        .iter()
        .position(|l| !l.problem)
        .unwrap_or(v.lines.len());
    assert!(v.lines[first_plain..].iter().all(|l| !l.problem), "{v:?}");
    assert_eq!(v.problems, first_plain);
}
