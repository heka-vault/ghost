//! Whether this Ghost runs from its own AppImage (ADR 0075).
//!
//! An AppImage's runtime sets `APPIMAGE` (the file) and `APPDIR` (where it is mounted) for
//! the program it starts, and everything that program starts inherits them. A person's
//! editor or terminal can itself be an AppImage (Cursor, T3 Code), so a `ghostd` run from a
//! shell inside one, the .deb's or a checkout's, sees that program's two variables. Read as
//! Ghost's own they made `ghostd uninstall` end with `rm` of the editor, pointed the echo
//! canceller's `installed` link at it, said "appimage" in an error report and refused
//! `ghostd mcp add --command` its bridge.
//!
//! The two are Ghost's only when the running binary is inside `$APPDIR`: true for a mounted
//! AppImage and for `--appimage-extract-and-run`, and for nothing a parent left in the
//! environment. Every reader of either variable goes through this module
//! (`only_this_module_reads_the_appimages_variables`). `ghost_reports` sits beside this
//! crate, not above it, and keeps the same rule in `report::runs_from_appimage`. A copy of
//! `ghostd` the app runs from outside its mount (`ghostd uninstall --after`) is told the
//! file by the app (`--appimage`), since no environment can say it there.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The two variables an AppImage's runtime sets.
pub const VARIABLES: [&str; 2] = ["APPIMAGE", "APPDIR"];

/// Whether the binary at `exe` is inside the folder `appdir`, links resolved where the
/// paths are there. A relative folder and the root are no AppImage's mount.
fn inside(appdir: &Path, exe: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = real(appdir);
    dir.is_absolute() && dir.parent().is_some() && real(exe).starts_with(&dir)
}

/// `key` (`APPIMAGE` or `APPDIR`) from `env`, when it is this program's own: the binary at
/// `exe` runs from inside `env`'s `$APPDIR`. None for another program's, and for an empty
/// value.
pub fn own(
    key: &str,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
) -> Option<OsString> {
    let dir = env("APPDIR").filter(|d| !d.is_empty())?;
    if !inside(Path::new(&dir), exe?) {
        return None;
    }
    env(key).filter(|v| !v.is_empty())
}

/// `key` from `env` as the functions that take a `var` read it: `APPIMAGE` and `APPDIR`
/// only when they are this program's own ([`own`]), every other name as it is.
pub fn var_in(
    key: &str,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
) -> Option<String> {
    if VARIABLES.contains(&key) {
        own(key, env, exe)
    } else {
        env(key)
    }
    .and_then(|v| v.into_string().ok())
}

/// This process's environment, with another program's `APPIMAGE` and `APPDIR` left out.
pub fn var(key: &str) -> Option<String> {
    var_in(
        key,
        |k| std::env::var_os(k),
        std::env::current_exe().ok().as_deref(),
    )
}

/// Which of the two variables `env` holds for another program: set, and not this program's
/// own ([`own`]). The desktop app removes these from its environment before Tauri reads it,
/// since Tauri's `Env` takes `APPIMAGE` as the app's own file for the login item and the
/// updater (#178).
pub fn foreign_in(env: impl Fn(&str) -> Option<OsString>, exe: Option<&Path>) -> Vec<&'static str> {
    VARIABLES
        .into_iter()
        .filter(|key| env(key).is_some() && own(key, &env, exe).is_none())
        .collect()
}

/// [`foreign_in`] for this process.
pub fn foreign() -> Vec<&'static str> {
    foreign_in(
        |k| std::env::var_os(k),
        std::env::current_exe().ok().as_deref(),
    )
}

/// The AppImage this process runs from: its own file, or None.
pub fn file() -> Option<PathBuf> {
    own(
        "APPIMAGE",
        |k| std::env::var_os(k),
        std::env::current_exe().ok().as_deref(),
    )
    .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    /// What a shell inside T3 Code's AppImage hands every command it starts.
    const EDITOR: [(&str, &str); 3] = [
        (
            "APPIMAGE",
            "/home/p/Applications/T3-Code-0.1.9-x86_64.AppImage",
        ),
        ("APPDIR", "/tmp/.mount_T3-CodAbCdEf"),
        ("HOME", "/home/p"),
    ];

    #[test]
    fn another_programs_appimage_is_not_ghosts() {
        // The .deb's ghostd, and a checkout's, run from a shell inside an editor that is an
        // AppImage: the editor's file and mount are not Ghost's.
        for exe in ["/usr/bin/ghostd", "/home/p/ghost/target/release/ghostd"] {
            let exe = Some(Path::new(exe));
            for key in VARIABLES {
                assert_eq!(own(key, env(&EDITOR), exe), None, "{key}");
                assert_eq!(var_in(key, env(&EDITOR), exe), None, "{key}");
            }
            // Everything else in the environment is read as it is.
            assert_eq!(
                var_in("HOME", env(&EDITOR), exe),
                Some("/home/p".to_string())
            );
        }
    }

    #[test]
    fn another_programs_variables_are_named_to_be_dropped_and_ghosts_kept() {
        // #178: the .deb's app started from a shell inside T3 Code wrote T3 Code's AppImage
        // as Ghost's login item.
        let deb = Some(Path::new("/usr/bin/heka-ghost-desktop"));
        assert_eq!(foreign_in(env(&EDITOR), deb), ["APPIMAGE", "APPDIR"]);
        let only_file = [("APPIMAGE", "/home/p/Applications/T3-Code.AppImage")];
        assert_eq!(foreign_in(env(&only_file), deb), ["APPIMAGE"]);
        assert!(foreign_in(env(&[("HOME", "/home/p")]), deb).is_empty());
        // Ghost's own AppImage keeps both.
        let ghost = [
            ("APPIMAGE", "/home/p/Applications/Ghost.AppImage"),
            ("APPDIR", "/tmp/.mount_GhostAb12Cd"),
        ];
        let exe = Some(Path::new(
            "/tmp/.mount_GhostAb12Cd/usr/bin/heka-ghost-desktop",
        ));
        assert!(foreign_in(env(&ghost), exe).is_empty());
    }

    #[test]
    fn ghosts_own_appimage_is_the_one_its_binary_runs_from() {
        let ghost = [
            ("APPIMAGE", "/home/p/Applications/Ghost.AppImage"),
            ("APPDIR", "/tmp/.mount_GhostAb12Cd"),
        ];
        for exe in [
            "/tmp/.mount_GhostAb12Cd/usr/bin/ghostd",
            "/tmp/.mount_GhostAb12Cd/usr/bin/heka-ghost-desktop",
        ] {
            assert_eq!(
                own("APPIMAGE", env(&ghost), Some(Path::new(exe))),
                Some(OsString::from("/home/p/Applications/Ghost.AppImage"))
            );
            assert_eq!(
                var_in("APPDIR", env(&ghost), Some(Path::new(exe))),
                Some("/tmp/.mount_GhostAb12Cd".to_string())
            );
        }
        // `--appimage-extract-and-run`: the folder it was extracted to is the mount.
        let extracted = [
            ("APPIMAGE", "/home/p/Ghost.AppImage"),
            ("APPDIR", "/tmp/appimage_extracted_0a1b2c"),
        ];
        assert!(
            own(
                "APPIMAGE",
                env(&extracted),
                Some(Path::new("/tmp/appimage_extracted_0a1b2c/usr/bin/ghostd"))
            )
            .is_some()
        );
    }

    #[test]
    fn a_mount_is_matched_by_folder_and_never_guessed() {
        let exe = Some(Path::new("/tmp/.mount_GhostAb12Cd/usr/bin/ghostd"));
        // A folder whose name only begins the same is another mount.
        let near = [
            ("APPIMAGE", "/home/p/Other.AppImage"),
            ("APPDIR", "/tmp/.mount_Ghost"),
        ];
        assert_eq!(own("APPIMAGE", env(&near), exe), None);
        // No mount named, an empty one, a relative one, the root: nothing to hold the file to.
        for dir in [None, Some(""), Some("tmp/.mount_GhostAb12Cd"), Some("/")] {
            let mut pairs = vec![("APPIMAGE", "/home/p/Ghost.AppImage")];
            pairs.extend(dir.map(|d| ("APPDIR", d)));
            assert_eq!(own("APPIMAGE", env(&pairs), exe), None, "{dir:?}");
        }
        // The binary's own path unknown: not an AppImage.
        let ghost = [
            ("APPIMAGE", "/home/p/Ghost.AppImage"),
            ("APPDIR", "/tmp/.mount_GhostAb12Cd"),
        ];
        assert_eq!(own("APPIMAGE", env(&ghost), None), None);
        // The mount without the file's name is still the mount; an empty name is none.
        let no_file = [("APPIMAGE", ""), ("APPDIR", "/tmp/.mount_GhostAb12Cd")];
        assert_eq!(own("APPIMAGE", env(&no_file), exe), None);
        assert!(own("APPDIR", env(&no_file), exe).is_some());
    }

    #[test]
    fn a_mount_reached_through_a_link_is_the_same_mount() {
        // $TMPDIR is a link on some systems; the binary's own path is always the real one.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real/.mount_GhostAb12Cd");
        std::fs::create_dir_all(real.join("usr/bin")).unwrap();
        let exe = real.join("usr/bin/ghostd");
        std::fs::write(&exe, b"").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let dir = tmp.path().join("link/.mount_GhostAb12Cd");
        let dir = dir.to_str().unwrap();
        let pairs = [("APPIMAGE", "/home/p/Ghost.AppImage"), ("APPDIR", dir)];
        let exe = std::fs::canonicalize(&exe).unwrap();
        assert!(own("APPIMAGE", env(&pairs), Some(&exe)).is_some());
    }

    #[test]
    fn only_this_module_reads_the_appimages_variables() {
        // A new reader of `APPIMAGE` or `APPDIR` from the process is a new place another
        // program's AppImage is taken for Ghost's. Every source file of the workspace that
        // names either variable is listed here with why it may.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let mut named = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("apps/desktop/src-tauri/src")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    if !p.ends_with("target") {
                        stack.push(p);
                    }
                    continue;
                }
                if p.extension().is_some_and(|x| x == "rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    if t.contains("\"APPIMAGE\"") || t.contains("\"APPDIR\"") {
                        named.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
                    }
                }
            }
        }
        named.sort();
        assert_eq!(
            named,
            [
                // This module: the one reader of the process's two variables.
                "crates/ghost-core/src/appimage.rs",
                // Functions over a `var` they are given; the process's is `appimage::var`.
                "crates/ghost-core/src/ghost_app/clients.rs",
                // The same rule, for the crate that cannot use this one.
                "crates/ghost-reports/src/report.rs",
                // A test's own `var`.
                "crates/ghostd/src/outside_tests.rs",
                // Tests that run the built ghostd with an editor's variables.
                "crates/ghostd/tests/appimage.rs",
            ]
        );
    }
}
