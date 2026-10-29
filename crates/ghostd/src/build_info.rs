//! What this ghostd is: its version, and which kind of build (docs/releasing.md).
//!
//! - `dev`: a checkout (`make dev`, `cargo run`): the commit, whether the tree had
//!   uncommitted changes when the daemon started, and the make target that started it
//!   when it says so (`GHOST_LAUNCHER`), so the desktop names the right restart.
//! - `deploy`: a `make deploy` runtime: the deploy records the commit beside the binary.
//! - `release`: the bundle (ADR 0038): its build records it the same way.
//!
//! A recorded build is `ghost-build.json` beside the executable (a `make deploy` build
//! writes it there, as the venv's root held it for the Python daemon), else in the bundle's
//! resources beside it by the same lexical rule as the runtime's (`/usr/bin/ghostd` reads
//! `/usr/lib/heka-ghost-desktop/ghost-build.json`, which `scripts/sidecars.sh` writes;
//! `ghost_infer::runtime::resources_beside`); a binary with neither is a checkout's. The
//! record names the version it was built at too; one that differs from this binary's is
//! said in the log and believed. Read once, at start: two `git` calls for a checkout,
//! nothing for the others, and never on a turn.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use ghost_api::models::{Build, BuildKind, Launcher};
use ghost_core::redact::child_env;
use ghost_infer::runtime::resources_beside;
use regex::Regex;
use tracing::warn;

pub const BUILD_FILE: &str = "ghost-build.json";
pub const GIT_TIMEOUT: Duration = Duration::from_secs(2);
/// Set by scripts/dev.sh (make dev, make daemon) and make start; any other value is ignored.
pub const LAUNCHERS: [&str; 2] = ["make-dev", "make-start"];

static COMMIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{7,40}$").unwrap());

/// The version ghostd was built at: the workspace's, which is the root `pyproject.toml`'s.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The build recorded beside the executable, else in the bundle's resources beside it,
/// else the checkout `source` (this crate's folder when it was built) is in, else a dev
/// build with no commit. A checkout's says which make target started it, when
/// `GHOST_LAUNCHER` names one.
pub fn detect(
    prefix: Option<&Path>,
    source: Option<&Path>,
    environ: Option<&HashMap<String, String>>,
) -> Build {
    let prefix = prefix.map_or_else(default_prefix, Path::to_path_buf);
    if let Some(recorded) = recorded(&prefix.join(BUILD_FILE)) {
        return recorded;
    }
    if let Some(resources) = resources_beside(&prefix)
        && let Some(recorded) = recorded(&resources.join(BUILD_FILE))
    {
        return recorded;
    }
    let source = source.map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Path::to_path_buf,
    );
    let mut found = checkout(&source);
    let launcher = match environ {
        Some(env) => env.get("GHOST_LAUNCHER").cloned().unwrap_or_default(),
        None => std::env::var("GHOST_LAUNCHER").unwrap_or_default(),
    };
    found.launcher = match launcher.as_str() {
        "make-dev" => Some(Launcher::MakeDev),
        "make-start" => Some(Launcher::MakeStart),
        _ => None,
    };
    found
}

/// The executable's folder, where a deploy records its build (a release's is in the
/// resources beside it).
fn default_prefix() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn recorded(path: &Path) -> Option<Build> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(
                "{} is unreadable ({:?}); reporting a dev build",
                path.display(),
                e.kind()
            );
            return None;
        }
    };
    let data: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            warn!(
                "{} is unreadable (JSONDecodeError); reporting a dev build",
                path.display()
            );
            return None;
        }
    };
    match validate(&data) {
        Some(build) => {
            if let Some(recorded) = data.get("version").and_then(|v| v.as_str())
                && recorded != version()
            {
                warn!(
                    "{} says version {recorded}; this ghostd is {}",
                    path.display(),
                    version()
                );
            }
            Some(build)
        }
        None => {
            warn!(
                "{} isn't a build record; reporting a dev build",
                path.display()
            );
            None
        }
    }
}

/// `Build` as the Python model validated it: a kind it knows, a commit that is a hash.
/// The record's `version` is read apart (`recorded`), not part of the `Build`.
fn validate(data: &serde_json::Value) -> Option<Build> {
    if !data.is_object() {
        return None;
    }
    let build: Build = serde_json::from_value(data.clone()).ok()?;
    if let Some(commit) = &build.commit
        && !COMMIT.is_match(commit)
    {
        return None;
    }
    Some(build)
}

fn checkout(where_: &Path) -> Build {
    let dev = Build {
        kind: BuildKind::Dev,
        ..Default::default()
    };
    let Some(commit) = git(where_, &["rev-parse", "HEAD"]) else {
        return dev;
    };
    if !COMMIT.is_match(&commit) {
        return dev;
    }
    // Tracked files only, as scripts/deploy.sh judges a tree.
    let Some(status) = git(where_, &["status", "--porcelain", "--untracked-files=no"]) else {
        return dev;
    };
    Build {
        kind: BuildKind::Dev,
        commit: Some(commit),
        dirty: !status.is_empty(),
        launcher: None,
    }
}

/// One `git -C where …`, its stdout trimmed; None when git is missing, fails, or takes
/// longer than `GIT_TIMEOUT`. Started without any password in its environment.
fn git(where_: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(where_)
        .args(args)
        .env_clear()
        .envs(child_env(std::env::vars()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {}
            Err(_) => return None,
        }
        if started.elapsed() > GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().ok().map(|s| s.trim().to_string())
}

/// One line: "Ghost 2026.10.0 · dev a1b2c3d", the version, the kind and the commit when
/// known, as Settings › General says it. A release names its commit too, since the bundle
/// records it (the release workflow's install test reads it from `ghostd version`); the
/// desktop's sidebar footer shows a release's version alone, since its version is its tag.
pub fn describe(ver: &str, build: &Build) -> String {
    let mut line = format!("Ghost {ver}");
    let kind = match build.kind {
        BuildKind::Dev => "dev",
        BuildKind::Deploy => "deploy",
        BuildKind::Release => "release",
    };
    line.push_str(&format!(" · {kind}"));
    if let Some(commit) = &build.commit {
        let short: String = commit.chars().take(7).collect();
        line.push_str(&format!(" {short}"));
    }
    if build.dirty {
        line.push_str(", uncommitted changes");
    }
    line
}

#[cfg(test)]
mod tests {
    //! What the daemon says it is (#33): its version and its build, in the status.

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn build(kind: BuildKind, commit: Option<&str>, dirty: bool) -> Build {
        Build {
            kind,
            commit: commit.map(String::from),
            dirty,
            launcher: None,
        }
    }

    fn detect_in(dir: &Path) -> Build {
        detect(Some(dir), Some(dir), Some(&HashMap::new()))
    }

    #[test]
    fn the_version_is_the_installed_one() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(version().starts_with("20"), "{}", version());
    }

    #[test]
    fn a_recorded_build_is_believed() {
        for (record, want) in [
            (
                serde_json::json!({"kind": "deploy", "commit": SHA}),
                build(BuildKind::Deploy, Some(SHA), false),
            ),
            (
                serde_json::json!({"kind": "release", "commit": SHA, "dirty": false}),
                build(BuildKind::Release, Some(SHA), false),
            ),
            (
                serde_json::json!({"kind": "release", "dirty": false}),
                build(BuildKind::Release, None, false),
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(BUILD_FILE), record.to_string()).unwrap();
            assert_eq!(detect_in(dir.path()), want);
        }
    }

    #[test]
    fn a_build_recorded_in_the_bundles_resources_is_read() {
        // `usr/bin/ghostd` and `usr/lib/heka-ghost-desktop/ghost-build.json`, as the .deb
        // and the AppImage lay them out (scripts/sidecars.sh writes the record there);
        // nothing beside the binary.
        const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("usr/bin");
        let resources = tmp
            .path()
            .join("usr/lib")
            .join(ghost_infer::runtime::BUNDLE_NAME);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            resources.join(BUILD_FILE),
            serde_json::json!({"kind": "release", "commit": SHA, "version": version()}).to_string(),
        )
        .unwrap();
        assert_eq!(detect_in(&bin), build(BuildKind::Release, Some(SHA), false));
        // A record whose version isn't this binary's is still believed (and said in the log).
        std::fs::write(
            resources.join(BUILD_FILE),
            serde_json::json!({"kind": "release", "commit": SHA, "version": "1999.1.0"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(detect_in(&bin), build(BuildKind::Release, Some(SHA), false));
        // A record beside the executable (a make deploy build) wins over the resources'.
        std::fs::write(
            bin.join(BUILD_FILE),
            serde_json::json!({"kind": "deploy", "commit": OTHER}).to_string(),
        )
        .unwrap();
        assert_eq!(
            detect_in(&bin),
            build(BuildKind::Deploy, Some(OTHER), false)
        );
        // A folder with no record and no bundle resources beside it is a checkout's or
        // nothing: dev.
        let alone = tmp.path().join("alone");
        std::fs::create_dir_all(&alone).unwrap();
        assert_eq!(detect_in(&alone), build(BuildKind::Dev, None, false));
    }

    #[test]
    fn a_bad_record_is_a_dev_build() {
        for text in [
            "{".to_string(),
            serde_json::json!({"kind": "nightly"}).to_string(),
            serde_json::json!({"kind": "deploy", "commit": "; rm"}).to_string(),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(BUILD_FILE), text).unwrap();
            assert_eq!(detect_in(dir.path()), build(BuildKind::Dev, None, false));
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn a_checkout_is_dev_with_its_commit_and_whether_it_is_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.py"), "x = 1\n").unwrap();
        run_git(&repo, &["add", "a.py"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=T",
                "-c",
                "commit.gpgSign=false",
                "commit",
                "-q",
                "-m",
                "a",
            ],
        );
        let head = run_git(&repo, &["rev-parse", "HEAD"]);
        let prefix = tmp.path().join("venv");
        std::fs::create_dir(&prefix).unwrap();
        let env = HashMap::new();
        assert_eq!(
            detect(Some(&prefix), Some(&repo), Some(&env)),
            build(BuildKind::Dev, Some(&head), false)
        );
        std::fs::write(repo.join("untracked.txt"), "not counted").unwrap();
        assert!(!detect(Some(&prefix), Some(&repo), Some(&env)).dirty);
        std::fs::write(repo.join("a.py"), "x = 2\n").unwrap();
        assert_eq!(
            detect(Some(&prefix), Some(&repo), Some(&env)),
            build(BuildKind::Dev, Some(&head), true)
        );
    }

    #[test]
    fn a_checkout_says_which_make_target_started_it() {
        for (launcher, want) in [
            ("make-dev", Some(Launcher::MakeDev)),
            ("make-start", Some(Launcher::MakeStart)),
            ("systemd", None),
            ("", None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let env: HashMap<String, String> =
                [("GHOST_LAUNCHER".to_string(), launcher.to_string())].into();
            assert_eq!(
                detect(Some(dir.path()), Some(dir.path()), Some(&env)).launcher,
                want
            );
        }
    }

    #[test]
    fn a_recorded_build_has_no_launcher() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(BUILD_FILE),
            serde_json::json!({"kind": "deploy", "commit": SHA}).to_string(),
        )
        .unwrap();
        let env: HashMap<String, String> =
            [("GHOST_LAUNCHER".to_string(), "make-dev".to_string())].into();
        assert_eq!(
            detect(Some(dir.path()), Some(dir.path()), Some(&env)).launcher,
            None
        );
    }

    #[test]
    fn outside_a_checkout_it_is_dev_without_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_in(dir.path()), build(BuildKind::Dev, None, false));
    }

    #[test]
    fn describe_() {
        const EDITED: &str = ", uncommitted changes";
        for (b, want) in [
            (
                build(BuildKind::Dev, Some(SHA), false),
                "Ghost 2026.10.0 · dev 0123456".to_string(),
            ),
            (
                build(BuildKind::Dev, Some(SHA), true),
                format!("Ghost 2026.10.0 · dev 0123456{EDITED}"),
            ),
            (
                build(BuildKind::Dev, None, false),
                "Ghost 2026.10.0 · dev".to_string(),
            ),
            (
                build(BuildKind::Deploy, Some(SHA), false),
                "Ghost 2026.10.0 · deploy 0123456".to_string(),
            ),
            (
                build(BuildKind::Release, Some(SHA), false),
                "Ghost 2026.10.0 · release 0123456".to_string(),
            ),
            (
                build(BuildKind::Release, None, false),
                "Ghost 2026.10.0 · release".to_string(),
            ),
            (
                build(BuildKind::Release, Some(SHA), true),
                format!("Ghost 2026.10.0 · release 0123456{EDITED}"),
            ),
        ] {
            assert_eq!(describe("2026.10.0", &b), want);
        }
    }
}
