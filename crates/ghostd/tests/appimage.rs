//! A `ghostd` run from a shell inside another program's AppImage is not that AppImage.
//!
//! An editor or a terminal that is itself an AppImage (Cursor, T3 Code) hands `APPIMAGE` and
//! `APPDIR` to every command run from it. Ghost's setup guide is followed by a coding agent in
//! exactly such a shell, and there `ghostd uninstall` ended with `rm` of the editor,
//! `ghostd mcp add --command` refused the bridge, and `ghostd echo-cancel on` pointed the
//! canceller's `installed` link at the editor. The built `ghostd` is run here with an editor's
//! two variables, and from a mount of its own, to hold `ghost_core::appimage` to every
//! command that reads it.
//!
//! Isolated from the operator: a cleared environment, a temp home and runtime folder, no
//! session bus (so no keyring and no systemd), and no listener for outside tools.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const GHOSTD: &str = env!("CARGO_BIN_EXE_ghostd");

/// What a shell inside the editor's AppImage sets for everything it starts.
const EDITOR_FILE: &str = "/home/p/Applications/T3-Code-0.1.9-x86_64.AppImage";
const EDITOR_MOUNT: &str = "/tmp/.mount_T3-CodAbCdEf";

fn ghostd_at(bin: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(bin);
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

fn home() -> (tempfile::TempDir, PathBuf) {
    // Under the target folder, so a hard link to the built ghostd is on its file system.
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join("rt")).unwrap();
    (tmp, home)
}

fn said(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_editors_appimage_is_never_taken_for_ghosts() {
    let (_tmp, home) = home();
    let editor = |args: &[&str]| {
        said(
            ghostd_at(Path::new(GHOSTD), &home)
                .env("APPIMAGE", EDITOR_FILE)
                .env("APPDIR", EDITOR_MOUNT)
                .args(args),
        )
    };

    // The line that finishes an uninstall never names the editor's file.
    let dry = editor(&["uninstall", "--dry-run"]);
    assert!(dry.contains("Nothing was removed (--dry-run)"), "{dry}");
    assert!(!dry.contains("T3-Code"), "{dry}");
    assert!(!dry.contains("rm "), "{dry}");

    // Nor does the `ghostd` command's status, and there is no AppImage to install it for.
    let status = editor(&["command", "status", "--json"]);
    assert!(status.contains("\"appimage\":false"), "{status}");
    let install = editor(&["command", "install"]);
    assert!(
        install.contains("not running from an AppImage"),
        "{install}"
    );
    assert!(!home.join(".local/bin/ghostd").exists());

    // A tool that starts commands is given this ghostd as its bridge, as outside any
    // AppImage, and its token in a file: not the note about a mount that moves.
    let added = editor(&["mcp", "add", "codex", "--command"]);
    assert!(added.contains("mcp serve --client codex"), "{added}");
    assert!(!added.contains("running from an AppImage"), "{added}");
    assert!(!added.contains("T3-Code"), "{added}");
}

#[test]
fn ghosts_own_appimage_is_the_one_its_binary_runs_from() {
    // The built ghostd inside a folder that stands for the AppImage's mount.
    let (tmp, home) = home();
    let mount = tmp.path().join(".mount_GhostAb12Cd");
    std::fs::create_dir_all(mount.join("usr/bin")).unwrap();
    let bin = mount.join("usr/bin/ghostd");
    if std::fs::hard_link(GHOSTD, &bin).is_err() {
        std::fs::copy(GHOSTD, &bin).unwrap();
    }
    let image = tmp.path().join("Ghost.AppImage");
    std::fs::write(&image, b"ELF").unwrap();
    let own = |args: &[&str]| {
        said(
            ghostd_at(&bin, &home)
                .env("APPIMAGE", &image)
                .env("APPDIR", &mount)
                .args(args),
        )
    };

    let status = own(&["command", "status", "--json"]);
    assert!(status.contains("\"appimage\":true"), "{status}");
    let dry = own(&["uninstall", "--dry-run"]);
    assert!(dry.contains(&format!("rm {}", image.display())), "{dry}");
    // The same binary and the same file named, with another program's mount: not Ghost's.
    let other = said(
        ghostd_at(&bin, &home)
            .env("APPIMAGE", &image)
            .env("APPDIR", EDITOR_MOUNT)
            .args(["command", "status", "--json"]),
    );
    assert!(other.contains("\"appimage\":false"), "{other}");
}

#[test]
fn the_app_tells_its_copy_of_ghostd_which_appimage_it_ran_from() {
    // "Remove Ghost from this computer" runs a copy of ghostd from the runtime folder, outside
    // the mount, after the app has quit: it is told the file, and nothing else may tell it.
    let (tmp, home) = home();
    let image = tmp.path().join("Ghost.AppImage");
    std::fs::write(&image, b"ELF").unwrap();
    let image = image.display().to_string();
    let run = |args: &[&str]| said(ghostd_at(Path::new(GHOSTD), &home).args(args));

    let told = run(&[
        "uninstall",
        "--only",
        "app-storage",
        "--yes",
        "--after",
        "1999999999",
        "--appimage",
        &image,
    ]);
    assert!(told.contains(&format!("rm {image}")), "{told}");
    // Without the app's `--after` it is not an option a person is offered.
    let alone = run(&["uninstall", "--dry-run", "--appimage", &image]);
    assert!(alone.contains("--appimage is the app's own"), "{alone}");
    assert!(!alone.contains(&format!("rm {image}")), "{alone}");
}

#[test]
fn an_error_report_and_the_rest_of_ghost_agree_on_what_an_appimage_is() {
    // `ghost_reports` cannot use `ghost_core`, so it keeps the rule in its own words; the
    // two are held together here, case by case.
    let cases: [(Option<&str>, Option<&str>, Option<&str>); 9] = [
        (
            Some(EDITOR_FILE),
            Some(EDITOR_MOUNT),
            Some("/usr/bin/ghostd"),
        ),
        (
            Some(EDITOR_FILE),
            Some(EDITOR_MOUNT),
            Some("/home/p/ghost/target/debug/ghostd"),
        ),
        (
            Some("/home/p/Ghost.AppImage"),
            Some("/tmp/.mount_GhostAb12Cd"),
            Some("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"),
        ),
        (
            Some("/home/p/Ghost.AppImage"),
            Some("/tmp/.mount_Ghost"),
            Some("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"),
        ),
        (
            Some("/home/p/Ghost.AppImage"),
            None,
            Some("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"),
        ),
        (
            None,
            Some("/tmp/.mount_GhostAb12Cd"),
            Some("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"),
        ),
        (
            Some(""),
            Some("/tmp/.mount_GhostAb12Cd"),
            Some("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"),
        ),
        (
            Some("/home/p/Ghost.AppImage"),
            Some("/"),
            Some("/usr/bin/ghostd"),
        ),
        (
            Some("/home/p/Ghost.AppImage"),
            Some("/tmp/.mount_GhostAb12Cd"),
            None,
        ),
    ];
    for (file, dir, exe) in cases {
        let env = |k: &str| -> Option<OsString> {
            match k {
                "APPIMAGE" => file.map(OsString::from),
                "APPDIR" => dir.map(OsString::from),
                _ => None,
            }
        };
        let exe = exe.map(Path::new);
        assert_eq!(
            ghost_core::appimage::own("APPIMAGE", env, exe).is_some(),
            ghost_reports::report::runs_from_appimage(
                file.map(OsStr::new),
                dir.map(OsStr::new),
                exe
            ),
            "{file:?} {dir:?} {exe:?}"
        );
    }
}
