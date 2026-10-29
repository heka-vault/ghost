//! Per-stream music ducking, a port of `ghostd.audio.Ducker`. Rules from the workstation
//! handoff: lower only the matching output streams (Spotify), restore their exact previous
//! volumes, and never touch the sink mute that F8/F9 own. The saved volumes live in a state
//! file (`duck.json` under the runtime dir) so `ghostd restore-volumes` can undo a crash
//! from a systemd `ExecStopPost`, and the daemon restores it at start.
//!
//! Through PipeWire's registry and the node `Props` param (what `wpctl set-volume` does),
//! not the pulse protocol. The file keeps the Python's shape and meaning: keyed by the
//! stream's `object.serial` (the number `pactl` shows as the sink-input index), values are
//! pulse-scale fractions (the cube root of PipeWire's linear `channelVolumes`, what
//! `pactl` shows as a percentage), so a file either daemon wrote restores under the other.
//! The cube root and cube round-trip every f32 exactly (tested), so the restore is to the
//! bit.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::pw::{BoundNode, GlobalNode, NodeState, PwError, Session};

/// The state file's name under the runtime dir.
pub const DUCK_STATE: &str = "duck.json";

/// Stream serial (as a string, the Python's `str(si.index)`) to its channel volumes as
/// pulse-scale fractions.
pub type DuckState = BTreeMap<String, Vec<f64>>;

#[derive(Debug, thiserror::Error)]
pub enum DuckError {
    #[error(transparent)]
    PipeWire(#[from] PwError),
    #[error("duck state {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// `$XDG_RUNTIME_DIR/ghost` (0700), or `/tmp/ghost`: the Python's `ghostd.config.runtime_dir`.
pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = base.join("ghost");
    if std::fs::create_dir_all(&dir).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

/// Where the daemon keeps the ducked volumes.
pub fn default_state_file() -> PathBuf {
    runtime_dir().join(DUCK_STATE)
}

/// PipeWire's linear channel volume as the pulse-scale fraction `pactl` shows.
pub fn to_pulse(linear: f32) -> f64 {
    f64::from(linear.max(0.0)).cbrt()
}

/// A pulse-scale fraction back to PipeWire's linear volume.
pub fn to_linear(pulse: f64) -> f32 {
    let p = pulse.max(0.0);
    (p * p * p) as f32
}

/// What a ducked stream's channels go to: no louder than `level`, quieter left alone.
pub fn duck_target(before: &[f64], level: f64) -> Vec<f64> {
    before.iter().map(|&v| v.min(level)).collect()
}

/// One stream the ducker lowered.
#[derive(Debug, Clone, PartialEq)]
pub struct DuckedStream {
    pub serial: u64,
    pub name: String,
    /// Pulse-scale fractions, one per channel.
    pub before: Vec<f64>,
    pub after: Vec<f64>,
}

/// Lowers the configured music streams for a session and puts them back.
#[derive(Debug, Clone)]
pub struct Ducker {
    apps: Vec<String>,
    level: f64,
    state: PathBuf,
}

impl Ducker {
    /// `apps`: substrings matched against the streams' names (`GHOST_DUCK_APPS`);
    /// `level`: the pulse-scale ceiling while ducked (`GHOST_DUCK_LEVEL`).
    pub fn new<I, S>(apps: I, level: f64, state_file: PathBuf) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            apps: apps
                .into_iter()
                .map(|a| a.as_ref().to_lowercase())
                .collect(),
            level,
            state: state_file,
        }
    }

    pub fn state_file(&self) -> &Path {
        &self.state
    }

    /// Does a stream with these properties count as music? The Python's `_matches`: any
    /// app substring in `application.name`, `application.process.binary` or `media.name`,
    /// case-insensitively.
    pub fn matches(&self, props: &BTreeMap<String, String>) -> bool {
        [
            "application.name",
            "application.process.binary",
            "media.name",
        ]
        .iter()
        .map(|k| props.get(*k).map(|s| s.to_lowercase()).unwrap_or_default())
        .any(|name| self.apps.iter().any(|a| name.contains(a.as_str())))
    }

    /// Lower every matching output stream that is running or idle and save what it was.
    /// The Python skipped `si.corked`; PipeWire does not expose pulse's cork (a paused
    /// player's node is `idle`, the same as a silent uncorked one, checked with mpv on
    /// 2026-10-28), so idle streams are ducked too, which is harmless: they come back
    /// exactly. A node in error, creating or suspended is left alone.
    pub fn duck(&self) -> Result<Vec<DuckedStream>, DuckError> {
        let session = Session::connect("ghostd")?;
        let nodes = session.nodes()?;
        let candidates: Vec<&GlobalNode> = nodes
            .iter()
            .filter(|n| n.media_class() == "Stream/Output/Audio" && self.matches(&n.props))
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let bound = session.bind(candidates)?;
        let mut saved = DuckState::new();
        let mut ducked = Vec::new();
        let mut sets = Vec::new();
        for node in &bound {
            let facts = node.facts();
            if !matches!(facts.state, Some(NodeState::Idle | NodeState::Running)) {
                continue;
            }
            let Some(linear) = facts.channel_volumes else {
                continue;
            };
            let before: Vec<f64> = linear.iter().map(|&v| to_pulse(v)).collect();
            let after = duck_target(&before, self.level);
            saved.insert(node.serial_or_id().to_string(), before.clone());
            sets.push((
                node,
                after.iter().map(|&v| to_linear(v)).collect::<Vec<f32>>(),
            ));
            ducked.push(DuckedStream {
                serial: node.serial_or_id(),
                name: stream_name(&facts.props, node),
                before,
                after,
            });
        }
        if saved.is_empty() {
            return Ok(Vec::new());
        }
        // The file first, so a crash between here and the volumes still restores.
        write_state(&self.state, &saved)?;
        for (node, volumes) in &sets {
            node.set_channel_volumes(volumes)?;
        }
        session.roundtrip()?;
        for e in session.take_errors() {
            warn!("duck: {e}");
        }
        for d in &ducked {
            info!("ducked {:?} {:?} -> {:?}", d.name, d.before, d.after);
        }
        Ok(ducked)
    }

    /// Put every saved stream back to its exact previous volume and forget the file.
    pub fn restore(&self) -> Result<Vec<(u64, Vec<f64>)>, DuckError> {
        restore_from_state(&self.state)
    }
}

/// Restore the volumes `state_file` holds (a stream that ended is skipped) and remove it.
/// Nothing to do when the file is missing; an unreadable file is removed.
pub fn restore_from_state(state_file: &Path) -> Result<Vec<(u64, Vec<f64>)>, DuckError> {
    let Some(saved) = read_state(state_file)? else {
        return Ok(Vec::new());
    };
    let session = Session::connect("ghostd")?;
    let nodes = session.nodes()?;
    let wanted: Vec<&GlobalNode> = nodes
        .iter()
        .filter(|n| saved.contains_key(&n.serial_or_id().to_string()))
        .collect();
    let bound = session.bind(wanted)?;
    let mut restored = Vec::new();
    for node in &bound {
        let Some(volumes) = saved.get(&node.serial_or_id().to_string()) else {
            continue;
        };
        let linear: Vec<f32> = volumes.iter().map(|&v| to_linear(v)).collect();
        node.set_channel_volumes(&linear)?;
        let name = stream_name(&node.facts().props, node);
        info!("restored {:?} -> {:?}", name, volumes);
        restored.push((node.serial_or_id(), volumes.clone()));
    }
    if !bound.is_empty() {
        session.roundtrip()?;
        for e in session.take_errors() {
            warn!("restore: {e}");
        }
    }
    remove_state(state_file)?;
    Ok(restored)
}

/// The stream's name as pulse shows it: its current `media.name`, else its `node.name`.
fn stream_name(current: &BTreeMap<String, String>, node: &BoundNode) -> String {
    current
        .get("media.name")
        .or_else(|| node.props.get("media.name"))
        .cloned()
        .unwrap_or_else(|| node.name().to_string())
}

/// The saved volumes, or `None` when there is no file. Invalid JSON removes the file.
pub fn read_state(path: &Path) -> Result<Option<DuckState>, DuckError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DuckError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    match serde_json::from_str::<DuckState>(&text) {
        Ok(state) => Ok(Some(state)),
        Err(_) => {
            remove_state(path)?;
            Ok(None)
        }
    }
}

pub fn write_state(path: &Path, state: &DuckState) -> Result<(), DuckError> {
    let text = serde_json::to_string(state).expect("a map of floats serializes");
    std::fs::write(path, text).map_err(|source| DuckError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_state(path: &Path) -> Result<(), DuckError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DuckError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn matches_the_three_names_case_insensitively() {
        let d = Ducker::new(["Spotify"], 0.25, PathBuf::from("/nonexistent"));
        assert!(d.matches(&props(&[("application.name", "spotify")])));
        assert!(d.matches(&props(&[(
            "application.process.binary",
            "/usr/bin/SPOTIFY"
        )])));
        assert!(d.matches(&props(&[("media.name", "Spotify")])));
        assert!(
            !d.matches(&props(&[("node.name", "spotify")])),
            "node.name is not a name the Python read"
        );
        assert!(!d.matches(&props(&[("application.name", "Firefox")])));
        assert!(!d.matches(&props(&[])));
        let two = Ducker::new(["spotify", "rhythm"], 0.25, PathBuf::from("/nonexistent"));
        assert!(two.matches(&props(&[("application.name", "Rhythmbox")])));
        let none = Ducker::new(Vec::<String>::new(), 0.25, PathBuf::from("/nonexistent"));
        assert!(!none.matches(&props(&[("application.name", "spotify")])));
    }

    #[test]
    fn targets_cap_at_the_level_and_leave_quieter_channels() {
        assert_eq!(duck_target(&[0.8, 0.1], 0.25), vec![0.25, 0.1]);
        assert_eq!(duck_target(&[], 0.25), Vec::<f64>::new());
        assert_eq!(duck_target(&[0.25], 0.25), vec![0.25]);
    }

    #[test]
    fn pulse_scale_round_trips_every_f32_exactly() {
        // The cube root and the cube in f64 recover the f32 PipeWire holds, bit for bit.
        let mut x: u32 = 0x9E37_79B9;
        let mut checked = 0;
        for _ in 0..2_000_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let linear = (x as f32) / (u32::MAX as f32) * 1.5; // 0 ..= 150 %
            assert_eq!(
                to_linear(to_pulse(linear)).to_bits(),
                linear.to_bits(),
                "{linear}"
            );
            checked += 1;
        }
        assert_eq!(checked, 2_000_000);
        for linear in [
            0.0f32,
            1.0,
            0.040734,
            0.0156_25,
            f32::MIN_POSITIVE,
            1e-10,
            4.0,
        ] {
            assert_eq!(to_linear(to_pulse(linear)).to_bits(), linear.to_bits());
        }
        assert_eq!(to_pulse(-1.0), 0.0, "a negative volume reads as silence");
        assert_eq!(to_linear(-1.0), 0.0);
        // and the meaning: 34 % in pactl is linear 0.0393
        assert!((to_linear(0.34) - 0.039304).abs() < 1e-6);
        assert!((to_pulse(0.040734) - 0.344).abs() < 1e-3);
    }

    #[test]
    fn state_file_round_trips_in_the_pythons_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DUCK_STATE);
        assert_eq!(read_state(&path).unwrap(), None);
        let mut state = DuckState::new();
        state.insert("981".into(), vec![0.344, 0.344]);
        write_state(&path, &state).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, r#"{"981":[0.344,0.344]}"#);
        assert_eq!(read_state(&path).unwrap(), Some(state));
        // what the Python wrote reads too
        std::fs::write(&path, r#"{"981": [0.34400001, 0.34400001], "12": [1.0]}"#).unwrap();
        let read = read_state(&path).unwrap().unwrap();
        assert_eq!(read["981"], vec![0.34400001, 0.34400001]);
        assert_eq!(read["12"], vec![1.0]);
    }

    #[test]
    fn an_unreadable_state_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DUCK_STATE);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read_state(&path).unwrap(), None);
        assert!(!path.exists());
        std::fs::write(&path, r#"{"981": "loud"}"#).unwrap();
        assert_eq!(
            read_state(&path).unwrap(),
            None,
            "the wrong shape is unreadable too"
        );
        assert!(!path.exists());
    }

    #[test]
    fn state_file_lives_under_the_runtime_dir() {
        let d = Ducker::new(
            ["spotify"],
            0.25,
            PathBuf::from("/run/user/1000/ghost/duck.json"),
        );
        assert_eq!(d.state_file(), Path::new("/run/user/1000/ghost/duck.json"));
        assert!(default_state_file().ends_with("ghost/duck.json"));
    }
}
