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
        SoundServer::PipeWire => None,
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

/// Install (or, `uninstall`, remove) the canceller. (ok, what happened in words, at most
/// 2,000 characters of the installer's own output).
pub async fn run(infra_dir: &Path, uninstall: bool) -> (bool, String) {
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
    if !uninstall {
        let info = tokio::task::spawn_blocking(|| tool("pactl", &["info"]))
            .await
            .ok()
            .flatten();
        if let Some(why) = refusal(&sound_server(info.as_deref())) {
            return (false, why.to_string());
        }
    }
    // Through bash, so a bundle that lost the file's executable bit still runs it.
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg(&script);
    if uninstall {
        cmd.arg("--uninstall");
    }
    cmd.env_clear()
        .envs(ghost_core::redact::child_env(std::env::vars())) // never a password (ADR 0036)
        .stdin(std::process::Stdio::null())
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
        let (ok, why) = run(tmp.path(), false).await;
        assert!(!ok);
        assert!(
            why.starts_with("Ghost's echo canceller installer is missing"),
            "{why}"
        );
    }
}
