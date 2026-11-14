//! #147: every file a real `ghostd` writes in a home folder is one `ghost_core::leftovers`
//! names, so `ghostd uninstall` can take it away again; and `ghostd uninstall --all --yes`
//! leaves that home as it found it.
//!
//! Isolated from the operator: a cleared environment, a temp home and runtime folder, no
//! session bus (so no keyring and no systemd), and no listener for outside tools.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ghost_core::leftovers::{self, FilesOnly, Places};

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");

fn ghostd(home: &Path) -> Command {
    let mut cmd = Command::new(GHOSTD);
    cmd.env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_RUNTIME_DIR", home.join("rt"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/ghost-test-bus",
        )
        .env("GHOST_MCP_LISTEN", "")
        .env("GHOST_REPORTS_URL", "http://127.0.0.1:9");
    cmd
}

fn files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn places(home: &Path) -> Places {
    let mut env = BTreeMap::new();
    env.insert(
        "XDG_RUNTIME_DIR".into(),
        home.join("rt").display().to_string(),
    );
    Places::standard(home, &env)
}

#[test]
fn every_file_the_daemon_writes_is_in_the_inventory_and_uninstall_takes_it_away() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join("rt")).unwrap();
    std::fs::write(home.join("notes.txt"), "a file the person made").unwrap();
    let before = files(&home);

    // The daemon, as the app starts it, until its control socket answers; then Ghost's own
    // commands a newcomer runs.
    let mut run = ghostd(&home)
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut up = false;
    while start.elapsed() < Duration::from_secs(60) {
        if ghostd(&home)
            .arg("status")
            .output()
            .unwrap()
            .status
            .success()
        {
            up = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let added = ghostd(&home)
        .args(["mcp", "add", "example"])
        .output()
        .unwrap();
    let _ = ghostd(&home).arg("check").output().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(run.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    );
    let _ = run.wait();
    assert!(up, "the daemon never answered on its socket");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );

    let p = places(&home);
    let items = leftovers::inventory(&p, &FilesOnly);
    let covered: Vec<&PathBuf> = items.iter().flat_map(|i| &i.paths).collect();
    let written: Vec<PathBuf> = files(&home)
        .into_iter()
        .filter(|f| !before.contains(f))
        .collect();
    assert!(
        written.iter().any(|f| f.ends_with("ghost.db")),
        "the daemon wrote no database: {written:?}"
    );
    for f in &written {
        assert!(
            covered.iter().any(|c| f.starts_with(c)),
            "{} was written by Ghost but is not in ghost_core::leftovers::inventory",
            f.display()
        );
    }

    // And uninstall takes all of it away, leaving what the person made.
    let out = ghostd(&home)
        .args(["uninstall", "--all", "--yes"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("To finish removing Ghost"), "{said}");
    let left: Vec<PathBuf> = files(&home);
    assert_eq!(left, before, "{said}");
}
