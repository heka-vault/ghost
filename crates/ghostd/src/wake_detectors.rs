//! Retrained wake detectors, installed beside the shipped one (ADR 0064).
//!
//! `make wake-train` writes each into `<data>/wake/detectors/<name>/`: `hey_ghost.onnx` and
//! `card.json`, its SHA-256, the threshold and floor chosen from its own curve, and every
//! number before and after. `GHOST_WAKE_DETECTOR=<name>` in `settings.env` selects one
//! (`ghostd detector use <name>`; `ghostd detector use shipped` goes back), and the daemon
//! loads it only when [`load`] passes: the file matches its card's hash, the threshold and
//! floor are sane, and no set the card reports has more false wakes after than before
//! (issue #13: false wakes are not traded away). `ghostd check` runs the same check.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The setting that selects a retrained detector; unset or `shipped`: the shipped one.
pub const KEY: &str = "GHOST_WAKE_DETECTOR";
pub const SHIPPED: &str = "shipped";

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Count {
    #[serde(default)]
    pub takes: u32,
    #[serde(default)]
    pub woke: u32,
    #[serde(default)]
    pub checked: u32,
    #[serde(default)]
    pub false_wakes: u32,
    #[serde(default)]
    pub false_checked: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SetRow {
    pub set: String,
    pub stream: String,
    #[serde(default)]
    pub seen_in_training: String,
    pub before: Count,
    pub after: Count,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Card {
    pub name: String,
    #[serde(default = "default_file")]
    pub file: String,
    pub sha256: String,
    pub threshold: f64,
    pub floor: f64,
    #[serde(default)]
    pub sets: Vec<SetRow>,
}

fn default_file() -> String {
    "hey_ghost.onnx".into()
}

pub struct Installed {
    pub path: PathBuf,
    pub card: Card,
}

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("wake").join("detectors")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != SHIPPED
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.starts_with('.')
}

/// The card's own checks against its file.
pub fn verify(path: &Path, card: &Card) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sha = hex(&Sha256::digest(&bytes));
    if sha != card.sha256 {
        return Err(format!(
            "{} does not match its card (sha256 {sha}, the card says {})",
            path.display(),
            card.sha256
        ));
    }
    if !(0.05..=0.99).contains(&card.threshold) || !(0.0..=card.threshold).contains(&card.floor) {
        return Err(format!(
            "its threshold {} and floor {} are out of bounds",
            card.threshold, card.floor
        ));
    }
    for r in &card.sets {
        if r.after.false_wakes > r.before.false_wakes {
            return Err(format!(
                "it wakes more often than the shipped detector on {} ({}): {} false wakes, against {}",
                r.set, r.stream, r.after.false_wakes, r.before.false_wakes
            ));
        }
    }
    if card.sets.is_empty() {
        return Err("its card has no measurements".into());
    }
    Ok(())
}

/// A retrained detector by name, checked.
pub fn load(data_dir: &Path, name: &str) -> Result<Installed, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} is not a detector's name"));
    }
    let dir = folder(data_dir).join(name);
    let card: Card = serde_json::from_slice(
        &std::fs::read(dir.join("card.json"))
            .map_err(|e| format!("no card for detector {name}: {e}"))?,
    )
    .map_err(|e| format!("detector {name}'s card does not read: {e}"))?;
    if card.file.contains('/') {
        return Err(format!("detector {name}'s card names a file elsewhere"));
    }
    let path = dir.join(&card.file);
    verify(&path, &card).map_err(|e| format!("detector {name}: {e}"))?;
    Ok(Installed { path, card })
}

/// Every detector in the folder, by name, with whether it passes.
pub fn list(data_dir: &Path) -> Vec<(String, Result<Installed, String>)> {
    let mut names: Vec<String> = std::fs::read_dir(folder(data_dir))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let r = load(data_dir, &n);
            (n, r)
        })
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn takes_line(c: &Count) -> String {
    if c.takes > 0 {
        format!("{}+{}/{} heard", c.woke, c.checked, c.takes)
    } else {
        format!("{} false wakes", c.false_wakes)
    }
}

/// `ghostd detector list|use <name>|use shipped`.
pub fn cli(
    args: &[String],
    data_dir: &Path,
    settings_path: &Path,
    selected: Option<&str>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let usage = "ghostd detector list | use <name> | use shipped";
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let sel = selected.unwrap_or(SHIPPED);
            let _ = writeln!(
                out,
                "{} shipped (~/.cache/ghost/wake/hey_ghost.onnx)",
                if sel == SHIPPED { "*" } else { " " }
            );
            for (name, r) in list(data_dir) {
                let mark = if sel == name { "*" } else { " " };
                match r {
                    Ok(i) => {
                        let _ = writeln!(
                            out,
                            "{mark} {name}: threshold {:.2}, floor {:.2}",
                            i.card.threshold, i.card.floor
                        );
                        for s in &i.card.sets {
                            let _ = writeln!(
                                out,
                                "      {} / {} ({}): {} -> {}",
                                s.set,
                                s.stream,
                                s.seen_in_training,
                                takes_line(&s.before),
                                takes_line(&s.after)
                            );
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(out, "{mark} {name}: refused: {e}");
                    }
                }
            }
            0
        }
        Some("use") => {
            let Some(name) = args.get(1) else {
                let _ = writeln!(err, "{usage}");
                return 2;
            };
            let value = if name == SHIPPED {
                None
            } else {
                if let Err(e) = load(data_dir, name) {
                    let _ = writeln!(err, "not switched: {e}");
                    return 1;
                }
                Some(name.clone())
            };
            let mut changes = BTreeMap::new();
            changes.insert(KEY.to_string(), value.clone());
            if let Err(e) = ghost_core::envfile::update_env(settings_path, &changes, 0o600) {
                let _ = writeln!(err, "could not write {}: {e}", settings_path.display());
                return 1;
            }
            let _ = writeln!(
                out,
                "{KEY}={} in {}; restart Ghost (make reload, or the app) to hear with it. \
                 `ghostd detector use shipped` goes back.",
                value.as_deref().unwrap_or(""),
                settings_path.display()
            );
            0
        }
        _ => {
            let _ = writeln!(err, "{usage}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(data: &Path, name: &str, bytes: &[u8], after_fw: u32) -> PathBuf {
        let dir = folder(data).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hey_ghost.onnx"), bytes).unwrap();
        let card = serde_json::json!({
            "name": name,
            "file": "hey_ghost.onnx",
            "sha256": hex(&Sha256::digest(bytes)),
            "threshold": 0.55,
            "floor": 0.25,
            "sets": [
                {"set": "operator takes, ghost.mic", "stream": "a.wav", "seen_in_training": "no",
                 "before": {"takes": 3, "woke": 1, "checked": 1, "false_wakes": 0, "false_checked": 0},
                 "after": {"takes": 3, "woke": 3, "checked": 0, "false_wakes": 0, "false_checked": 0}},
                {"set": "Kokoro, digital", "stream": "p.wav", "seen_in_training": "no",
                 "before": {"takes": 0, "false_wakes": 10},
                 "after": {"takes": 0, "false_wakes": after_fw}}
            ]
        });
        std::fs::write(dir.join("card.json"), card.to_string()).unwrap();
        dir
    }

    #[test]
    fn a_detector_that_matches_its_card_loads() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "op-1", b"model bytes", 9);
        let i = load(tmp.path(), "op-1").unwrap();
        assert_eq!(i.card.threshold, 0.55);
        assert!(i.path.ends_with("op-1/hey_ghost.onnx"));
    }

    #[test]
    fn a_changed_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install(tmp.path(), "op-1", b"model bytes", 9);
        std::fs::write(dir.join("hey_ghost.onnx"), b"other bytes").unwrap();
        let e = load(tmp.path(), "op-1").err().unwrap();
        assert!(e.contains("does not match its card"), "{e}");
    }

    #[test]
    fn more_false_wakes_than_the_shipped_detector_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "op-2", b"model", 11);
        let e = load(tmp.path(), "op-2").err().unwrap();
        assert!(e.contains("wakes more often"), "{e}");
    }

    #[test]
    fn names_stay_inside_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(tmp.path(), "../x").is_err());
        assert!(load(tmp.path(), "shipped").is_err());
        assert!(load(tmp.path(), "").is_err());
    }

    #[test]
    fn use_and_go_back_write_the_setting() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "op-1", b"model bytes", 9);
        install(tmp.path(), "op-bad", b"model", 12);
        let settings = tmp.path().join("settings.env");
        std::fs::write(&settings, "GHOST_WAKE_MODE=onnx\n").unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            cli(
                &a(&["use", "op-1"]),
                tmp.path(),
                &settings,
                None,
                &mut out,
                &mut err
            ),
            0
        );
        let env = ghost_core::envfile::read_env(&settings).unwrap();
        assert_eq!(env.get(KEY).map(String::as_str), Some("op-1"));
        assert_eq!(env.get("GHOST_WAKE_MODE").map(String::as_str), Some("onnx"));
        // a detector that fails its card is not switched to
        assert_eq!(
            cli(
                &a(&["use", "op-bad"]),
                tmp.path(),
                &settings,
                None,
                &mut out,
                &mut err
            ),
            1
        );
        assert_eq!(
            ghost_core::envfile::read_env(&settings)
                .unwrap()
                .get(KEY)
                .map(String::as_str),
            Some("op-1")
        );
        // the one-command rollback
        assert_eq!(
            cli(
                &a(&["use", "shipped"]),
                tmp.path(),
                &settings,
                None,
                &mut out,
                &mut err
            ),
            0
        );
        assert!(
            !ghost_core::envfile::read_env(&settings)
                .unwrap()
                .contains_key(KEY)
        );
        let mut listed = Vec::new();
        cli(
            &a(&["list"]),
            tmp.path(),
            &settings,
            None,
            &mut listed,
            &mut err,
        );
        let listed = String::from_utf8(listed).unwrap();
        assert!(listed.contains("* shipped"));
        assert!(listed.contains("op-bad: refused"));
    }
}
