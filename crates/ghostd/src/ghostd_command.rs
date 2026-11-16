//! `ghostd command status|install|remove` (ADR 0075): the `ghostd` command for a person who
//! runs Ghost from the AppImage, written by `ghost_core::ghostd_command`. Settings › General's
//! "Install the ghostd command" runs `install` through the shell; from a terminal it is
//! `./Ghost.AppImage ghostd command install`. The operator's only: no tool, model, outside
//! client or the phone reaches it.

use std::io::Write;
use std::path::{Path, PathBuf};

use ghost_core::ghostd_command::{self, State};

/// What the command line reads from the process.
pub struct Env {
    pub home: Option<PathBuf>,
    /// The AppImage this `ghostd` runs from (`ghost_core::appimage::file`), never one a
    /// parent program left in the environment.
    pub appimage: Option<PathBuf>,
    pub path: Option<std::ffi::OsString>,
}

impl Env {
    pub fn process() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            appimage: ghost_core::appimage::file(),
            path: std::env::var_os("PATH"),
        }
    }
}

fn status_json(home: &Path, env: &Env) -> serde_json::Value {
    let p = ghostd_command::path(home);
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    serde_json::json!({
        "appimage": env.appimage.is_some(),
        "path": p,
        "on_path": ghostd_command::on_path(&dir, env.path.as_deref()),
        "command": ghostd_command::state(home, env.appimage.as_deref()),
    })
}

pub fn cli(args: &[String], env: &Env, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(home) = env.home.as_deref() else {
        let _ = writeln!(err, "ghostd command: HOME is not set");
        return 2;
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let v = status_json(home, env);
            if args.iter().any(|a| a == "--json") {
                let _ = writeln!(out, "{v}");
                return 0;
            }
            let p = ghostd_command::path(home);
            let line = match ghostd_command::state(home, env.appimage.as_deref()) {
                State::Absent => format!("not installed ({} is not there)", p.display()),
                State::Installed { appimage } => {
                    format!("{} runs {}", p.display(), appimage.display())
                }
                State::Stale { appimage } => format!(
                    "{} runs {}, not this AppImage; `ghostd command install` points it here",
                    p.display(),
                    appimage.display()
                ),
                State::Other => format!("{} is there and is not Ghost's", p.display()),
            };
            let _ = writeln!(out, "ghostd command: {line}");
            0
        }
        Some("install") => {
            let Some(app) = env.appimage.as_deref() else {
                let _ = writeln!(
                    err,
                    "This Ghost is not running from an AppImage, so there is nothing to install: \
                     the .deb already puts ghostd on the PATH."
                );
                return 1;
            };
            match ghostd_command::install(home, app) {
                Ok(p) => {
                    let _ = writeln!(out, "installed {} (runs {})", p.display(), app.display());
                    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                    if !ghostd_command::on_path(&dir, env.path.as_deref()) {
                        let _ = writeln!(
                            out,
                            "{} is not on your PATH in this shell; open a new terminal, or add \
                             it to your shell's PATH",
                            dir.display()
                        );
                    }
                    0
                }
                Err(e) => {
                    let _ = writeln!(err, "ghostd command: {e}");
                    1
                }
            }
        }
        Some("remove") => match ghostd_command::remove(home) {
            Ok(true) => {
                let _ = writeln!(out, "removed {}", ghostd_command::path(home).display());
                0
            }
            Ok(false) => {
                let _ = writeln!(out, "nothing of Ghost's to remove");
                0
            }
            Err(e) => {
                let _ = writeln!(err, "ghostd command: {e}");
                1
            }
        },
        Some(other) => {
            let _ = writeln!(
                err,
                "unknown: ghostd command {other}; ghostd command [status [--json]|install|remove]"
            );
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], env: &Env) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = cli(&args, env, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn install_status_and_remove_from_an_appimage() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("Ghost.AppImage");
        std::fs::write(&app, b"ELF").unwrap();
        let env = Env {
            home: Some(t.path().to_path_buf()),
            appimage: Some(app.clone()),
            path: Some("/usr/bin".into()),
        };
        let (code, out, _) = run(&["install"], &env);
        assert_eq!(code, 0);
        assert!(
            out.contains("installed") && out.contains("not on your PATH"),
            "{out}"
        );
        let (_, out, _) = run(&["status", "--json"], &env);
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["appimage"], true);
        assert_eq!(v["command"]["state"], "installed");
        assert_eq!(v["on_path"], false);
        let (code, out, _) = run(&["remove"], &env);
        assert_eq!((code, out.contains("removed")), (0, true));
    }

    #[test]
    fn a_deb_has_nothing_to_install() {
        let t = tempfile::tempdir().unwrap();
        let env = Env {
            home: Some(t.path().to_path_buf()),
            appimage: None,
            path: None,
        };
        let (code, _, err) = run(&["install"], &env);
        assert_eq!(code, 1);
        assert!(err.contains("not running from an AppImage"), "{err}");
        assert!(!ghostd_command::path(t.path()).exists());
    }
}
