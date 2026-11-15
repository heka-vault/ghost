//! Ghost's echo canceller, installed or removed (`infra/pipewire/install.sh`, ADR 0064):
//! the one runner behind `POST /setup/echo-cancel`, `ghostd echo-cancel` and the microphone's
//! Repair. The installer comes from the checkout's `infra/` or, installed from a release,
//! from the bundle's resources (ADR 0066); it writes a user unit and a config under the
//! operator's home and runs the system's own `pipewire` on them, so nothing it leaves points
//! into the bundle. No root.
//!
//! Before running it, the sound server is asked what it is (`pactl info`), so a machine that
//! cannot have the canceller hears why in a sentence instead of a script's error.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// What `pactl info` says the sound server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundServer {
    /// PipeWire, through its PulseAudio service: the canceller works.
    PipeWire,
    /// PulseAudio itself: the canceller is a PipeWire module.
    PulseAudio,
    /// `pactl` answered nothing: no sound server is running for this user.
    None,
    /// `pactl` is not installed.
    NoPactl,
    /// Not asked: a daemon with no sound stack of its own (`NoDevices`). The installer
    /// speaks for itself.
    Unknown,
}

/// The live answer: `pactl info`, run now. Only the daemon's own devices port and the
/// command line call it; a test's port answers for itself.
pub fn probe_sound_server() -> SoundServer {
    sound_server(tool("pactl", &["info"]).as_deref())
}

/// From `pactl info`'s output, or None when `pactl` could not be run.
pub fn sound_server(info: Option<&str>) -> SoundServer {
    let Some(info) = info else {
        return SoundServer::NoPactl;
    };
    match info
        .lines()
        .find_map(|l| l.strip_prefix("Server Name:"))
        .map(str::trim)
    {
        Some(name) if name.contains("PipeWire") => SoundServer::PipeWire,
        Some(_) => SoundServer::PulseAudio,
        None => SoundServer::None,
    }
}

/// Why the canceller can't be installed on this sound server, in the operator's words.
pub fn refusal(server: &SoundServer) -> Option<&'static str> {
    match server {
        SoundServer::PipeWire | SoundServer::Unknown => None,
        SoundServer::PulseAudio => Some(
            "This computer's sound runs on PulseAudio, and Ghost's echo canceller is a PipeWire \
             module, so it can't be set up here. Use headphones, or a microphone that cancels \
             echo itself; Ubuntu 22.10 and later run PipeWire.",
        ),
        SoundServer::None => Some(
            "Ghost found no sound server for your login (pactl info answered nothing), so there \
             is nothing to set the echo canceller up on. Is PipeWire running? \
             (systemctl --user status pipewire pipewire-pulse)",
        ),
        SoundServer::NoPactl => Some(
            "Ghost needs pactl to set up its echo canceller, and it isn't installed: \
             sudo apt install pulseaudio-utils",
        ),
    }
}

/// What `ghostd echo-cancel status` prints: whether the canceller runs, where its
/// installer is, and, when it does not run, either that `on` sets it up or why it cannot be
/// set up on this sound server, the check `on` makes (#138).
pub fn status_lines(running: bool, installer: &Path, server: &SoundServer) -> Vec<String> {
    let mut lines = vec![
        format!(
            "echo canceller: {}",
            if running {
                "running (ghost.mic)"
            } else {
                "not running"
            }
        ),
        format!(
            "installer: {}{}",
            installer.display(),
            if installer.is_file() {
                ""
            } else {
                " (missing)"
            }
        ),
    ];
    if !running {
        lines.push(match refusal(server) {
            Some(why) => format!("It can't be set up here. {why}"),
            None => "ghostd echo-cancel on sets it up".to_string(),
        });
    }
    lines
}

/// The unit as this version writes it (`infra/pipewire/ghost-echo-cancel.service`).
pub const UNIT_TEXT: &str = include_str!("../../../infra/pipewire/ghost-echo-cancel.service");
/// The first line of a unit Ghost wrote, from form 2 on.
pub const UNIT_MARKER: &str = "# Written by Ghost (ghost-echo-cancel";
/// The unit 2026.11.0 and 2026.11.1 installed, byte for byte: Ghost's, though it carries no
/// marker, and without the condition that stops it once Ghost is gone (#147).
pub const UNIT_FORM_1: &str = "[Unit]
Description=Ghost echo-cancelled microphone (ghost.mic)
After=pipewire.service wireplumber.service
Requires=pipewire.service

[Service]
Type=simple
ExecStart=/usr/bin/pipewire -c %h/.config/ghost/echo-cancel.conf
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
";

/// What `refresh_unit` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitRefresh {
    /// No unit: the canceller is not installed.
    Absent,
    /// The unit is this version's.
    Current,
    /// Ghost's own unit of an earlier form, rewritten to this one.
    Rewritten,
    /// A unit Ghost did not write, or one the person edited and took the marker out of:
    /// left as it is.
    NotGhosts,
}

/// Keep the canceller's unit at this version's form: a unit Ghost wrote (its marker line,
/// or the exact text of form 1) is rewritten when it differs; any other is left as it is.
/// The caller reloads systemd when it was rewritten.
pub fn refresh_unit(unit: &Path) -> std::io::Result<UnitRefresh> {
    let text = match std::fs::read_to_string(unit) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UnitRefresh::Absent),
        Err(e) => return Err(e),
    };
    if text == UNIT_TEXT {
        return Ok(UnitRefresh::Current);
    }
    if !(text.starts_with(UNIT_MARKER) || text == UNIT_FORM_1) {
        return Ok(UnitRefresh::NotGhosts);
    }
    // One rename, so systemd never reads half a unit.
    let tmp = unit.with_extension("service.ghost-new");
    std::fs::write(&tmp, UNIT_TEXT)?;
    std::fs::rename(&tmp, unit)?;
    Ok(UnitRefresh::Rewritten)
}

/// What the canceller's `installed` link points at for this install: the AppImage itself
/// when Ghost runs from one (its mount moves at every start; the file does not), else the
/// running `ghostd` (`/usr/bin/ghostd` from the .deb). Removing the package or deleting the
/// AppImage leaves the link dangling, and the unit's `ConditionPathExists` then fails.
pub fn installed_target(appimage: Option<&Path>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(a) = appimage.filter(|a| a.is_absolute() && a.is_file()) {
        return Some(a.to_path_buf());
    }
    exe.map(|e| std::fs::canonicalize(e).unwrap_or_else(|_| e.to_path_buf()))
}

/// This process's `installed_target`.
pub fn installed_target_now() -> Option<PathBuf> {
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    installed_target(appimage.as_deref(), std::env::current_exe().ok().as_deref())
}

/// Point `link` at `target`, by one rename. True when it changed.
pub fn keep_installed_link(link: &Path, target: &Path) -> std::io::Result<bool> {
    if std::fs::read_link(link).is_ok_and(|t| t == target) {
        return Ok(false);
    }
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = link.with_extension("ghost-new");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    std::fs::rename(&tmp, link)?;
    Ok(true)
}

/// The installer under `infra_dir`.
pub fn installer(infra_dir: &Path) -> PathBuf {
    infra_dir.join("pipewire").join("install.sh")
}

fn tool(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars()))
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// Install (or, `uninstall`, remove) the canceller on `server` (what the sound server is,
/// asked by the caller's devices port). (ok, what happened in words, at most 2,000
/// characters of the installer's own output).
pub async fn run(infra_dir: &Path, uninstall: bool, server: SoundServer) -> (bool, String) {
    let script = installer(infra_dir);
    if !script.is_file() {
        return (
            false,
            format!(
                "Ghost's echo canceller installer is missing ({}). Reinstall Ghost, or set \
                 GHOST_INFRA_DIR to a checkout's infra/ folder.",
                script.display()
            ),
        );
    }
    if !uninstall && let Some(why) = refusal(&server) {
        return (false, why.to_string());
    }
    // Through bash, so a bundle that lost the file's executable bit still runs it.
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg(&script);
    if uninstall {
        cmd.arg("--uninstall");
    }
    cmd.env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars())); // never a password (ADR 0036)
    // What the unit's condition follows (#147): this install's ghostd, or its AppImage.
    if let Some(target) = installed_target_now() {
        cmd.env("GHOST_INSTALLED", target);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(60), cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return (false, format!("Ghost could not run the installer: {e}")),
        Err(_) => {
            return (
                false,
                "The echo canceller's installer took more than a minute and was stopped.".into(),
            );
        }
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let detail: String = text
        .trim()
        .chars()
        .rev()
        .take(2000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (out.status.success(), detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_does_not_start_once_ghost_is_gone() {
        // #147: the unit is conditioned on a link Ghost keeps pointing at itself.
        assert!(UNIT_TEXT.starts_with(UNIT_MARKER));
        assert!(UNIT_TEXT.contains("\nConditionPathExists=%S/ghost/installed\n"));
        assert!(
            UNIT_TEXT.contains("ExecStart=/usr/bin/pipewire -c %h/.config/ghost/echo-cancel.conf")
        );
        assert!(!UNIT_FORM_1.contains("Condition"));
    }

    #[test]
    fn ghosts_own_unit_is_rewritten_and_no_one_elses() {
        let tmp = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("ghost-echo-cancel.service");
        assert_eq!(refresh_unit(&unit).unwrap(), UnitRefresh::Absent);
        // 2026.11.1's unit, which has no marker: Ghost's, rewritten.
        std::fs::write(&unit, UNIT_FORM_1).unwrap();
        assert_eq!(refresh_unit(&unit).unwrap(), UnitRefresh::Rewritten);
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), UNIT_TEXT);
        assert_eq!(refresh_unit(&unit).unwrap(), UnitRefresh::Current);
        // A later form with the marker: rewritten.
        std::fs::write(&unit, format!("{UNIT_MARKER}, form 9)\n[Unit]\n")).unwrap();
        assert_eq!(refresh_unit(&unit).unwrap(), UnitRefresh::Rewritten);
        // The person edited it and took the marker out: theirs, left.
        let theirs = UNIT_FORM_1.replace("RestartSec=2", "RestartSec=5");
        std::fs::write(&unit, &theirs).unwrap();
        assert_eq!(refresh_unit(&unit).unwrap(), UnitRefresh::NotGhosts);
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), theirs);
    }

    #[test]
    fn the_link_follows_the_appimage_or_the_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let image = tmp.path().join("Ghost.AppImage");
        std::fs::write(&image, b"").unwrap();
        let exe = tmp.path().join("ghostd");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(
            installed_target(Some(&image), Some(&exe)),
            Some(image.clone())
        );
        // An AppImage that is not there (moved) is not followed; the binary is.
        let gone = tmp.path().join("gone.AppImage");
        assert_eq!(
            installed_target(Some(&gone), Some(&exe)),
            Some(std::fs::canonicalize(&exe).unwrap())
        );
        let link = tmp.path().join("state/ghost/installed");
        assert!(keep_installed_link(&link, &image).unwrap());
        assert!(!keep_installed_link(&link, &image).unwrap());
        assert!(link.exists());
        // Removed (the package gone, the AppImage deleted): the link dangles.
        std::fs::remove_file(&image).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_ok() && !link.exists());
        assert!(keep_installed_link(&link, &exe).unwrap());
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
    }

    #[test]
    fn the_shipped_unit_is_the_one_in_infra() {
        let file = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../infra/pipewire/ghost-echo-cancel.service"),
        )
        .unwrap();
        assert_eq!(file, UNIT_TEXT);
        let installer = include_str!("../../../infra/pipewire/install.sh");
        assert!(
            installer.contains(r#"ln -sfn "${GHOST_INSTALLED:-$HERE/install.sh}" "$INSTALLED""#)
        );
        assert!(
            installer
                .contains(r#"INSTALLED="${XDG_STATE_HOME:-$HOME/.local/state}/ghost/installed""#)
        );
    }

    #[test]
    fn the_sound_server_is_read_from_pactl() {
        let pw = "Server String: /run/user/1000/pulse/native\nServer Name: PulseAudio (on PipeWire 1.0.5)\n";
        assert_eq!(sound_server(Some(pw)), SoundServer::PipeWire);
        assert_eq!(refusal(&sound_server(Some(pw))), None);
        let pa = "Server Name: pulseaudio\nServer Version: 16.1\n";
        assert_eq!(sound_server(Some(pa)), SoundServer::PulseAudio);
        assert!(
            refusal(&SoundServer::PulseAudio)
                .unwrap()
                .contains("PulseAudio")
        );
        let none = "Connection failure: Connection refused\npa_context_connect() failed: Connection refused\n";
        assert_eq!(sound_server(Some(none)), SoundServer::None);
        assert!(
            refusal(&SoundServer::None)
                .unwrap()
                .contains("no sound server")
        );
        assert_eq!(sound_server(None), SoundServer::NoPactl);
        assert!(
            refusal(&SoundServer::NoPactl)
                .unwrap()
                .contains("pulseaudio-utils")
        );
    }

    #[tokio::test]
    async fn a_missing_installer_is_said_in_words() {
        let tmp = tempfile::tempdir().unwrap();
        let (ok, why) = run(tmp.path(), false, SoundServer::PipeWire).await;
        assert!(!ok);
        assert!(
            why.starts_with("Ghost's echo canceller installer is missing"),
            "{why}"
        );
    }

    #[test]
    fn a_daemon_with_no_sound_stack_of_its_own_lets_the_installer_speak() {
        assert_eq!(refusal(&SoundServer::Unknown), None);
    }

    /// No host involved: the installer is a script in a temp folder, and the sound server
    /// is what the caller says.
    #[tokio::test]
    async fn a_pulseaudio_machine_is_told_why_and_the_installer_never_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let pw = tmp.path().join("pipewire");
        std::fs::create_dir_all(&pw).unwrap();
        let ran = tmp.path().join("ran");
        std::fs::write(
            pw.join("install.sh"),
            format!("touch {}\necho installed\n", ran.display()),
        )
        .unwrap();
        let (ok, why) = run(tmp.path(), false, SoundServer::PulseAudio).await;
        assert!(!ok);
        assert!(why.contains("PulseAudio"), "{why}");
        assert!(!ran.exists(), "the installer must not run");
        let (ok, why) = run(tmp.path(), false, SoundServer::PipeWire).await;
        assert!(ok && why == "installed", "{why}");
        assert!(ran.exists());
        // Removing never asks the sound server.
        std::fs::remove_file(&ran).unwrap();
        let (ok, _) = run(tmp.path(), true, SoundServer::NoPactl).await;
        assert!(ok && ran.exists());
    }

    /// #138: `status` makes the check `on` makes, and does not suggest `on` where it
    /// cannot work.
    #[test]
    fn status_says_why_the_canceller_cannot_be_set_up_here() {
        let tmp = tempfile::tempdir().unwrap();
        let script = installer(tmp.path());
        let joined = |r: bool, s: SoundServer| status_lines(r, &script, &s).join("\n");
        let here = joined(false, SoundServer::PipeWire);
        assert!(here.starts_with("echo canceller: not running"), "{here}");
        assert!(here.ends_with("ghostd echo-cancel on sets it up"), "{here}");
        for server in [
            SoundServer::None,
            SoundServer::PulseAudio,
            SoundServer::NoPactl,
        ] {
            let text = joined(false, server.clone());
            assert!(!text.contains("echo-cancel on"), "{text}");
            assert!(
                text.contains(&format!(
                    "It can't be set up here. {}",
                    refusal(&server).unwrap()
                )),
                "{text}"
            );
        }
        let running = joined(true, SoundServer::PipeWire);
        assert!(running.starts_with("echo canceller: running"), "{running}");
        assert_eq!(running.lines().count(), 2);
    }
}
