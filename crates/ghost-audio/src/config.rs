//! The audio settings, under the names the Python daemon read (`ghost_voice.config` and
//! `ghostd.config`, prefix `GHOST_`). The daemon crate wires them; this is the list.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSettings {
    /// `GHOST_INPUT_DEVICE`: the capture node's `node.name` (`ghost.mic`); unset, the
    /// default source. The Python also took a PortAudio index; here it is a name.
    pub input_device: Option<String>,
    /// `GHOST_OUTPUT_DEVICE`: the sink's `node.name`; unset, the default sink.
    pub output_device: Option<String>,
    /// `GHOST_DUCK_APPS`: a JSON list of substrings matched against sink-input names.
    pub duck_apps: Vec<String>,
    /// `GHOST_DUCK_LEVEL`: the pulse-scale ceiling while ducked.
    pub duck_level: f64,
    /// `GHOST_CHIME`: play the chime on wake and sleep.
    pub chime: bool,
    /// `GHOST_THINKING_SOUND`: a soft loop while the model and tools are working.
    pub thinking_sound: bool,
    /// `GHOST_THINKING_VOLUME`: its level, mixed under speech.
    pub thinking_volume: f64,
    /// `GHOST_THINKING_DELAY_SECS`: how long a gap must last before the loop starts.
    pub thinking_delay_secs: f64,
    /// `GHOST_CACHE_DIR`: where the thinking sound's file is rendered.
    pub cache_dir: PathBuf,
}

impl Default for AudioSettings {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            input_device: None,
            output_device: None,
            duck_apps: vec!["spotify".into()],
            duck_level: 0.25,
            chime: true,
            thinking_sound: true,
            thinking_volume: 0.12,
            thinking_delay_secs: 0.5,
            cache_dir: home.join(".cache").join("ghost"),
        }
    }
}

impl AudioSettings {
    /// From the process environment.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// From any lookup of `GHOST_*` names. A value that does not parse keeps the default.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut s = Self::default();
        let text = |name: &str| get(name).filter(|v| !v.trim().is_empty());
        if let Some(v) = text("GHOST_INPUT_DEVICE") {
            s.input_device = Some(v);
        }
        if let Some(v) = text("GHOST_OUTPUT_DEVICE") {
            s.output_device = Some(v);
        }
        if let Some(apps) = get("GHOST_DUCK_APPS").and_then(|v| parse_list(&v)) {
            s.duck_apps = apps;
        }
        if let Some(v) = text("GHOST_DUCK_LEVEL").and_then(|v| v.trim().parse().ok()) {
            s.duck_level = v;
        }
        if let Some(v) = text("GHOST_CHIME").and_then(|v| parse_bool(&v)) {
            s.chime = v;
        }
        if let Some(v) = text("GHOST_THINKING_SOUND").and_then(|v| parse_bool(&v)) {
            s.thinking_sound = v;
        }
        if let Some(v) = text("GHOST_THINKING_VOLUME").and_then(|v| v.trim().parse().ok()) {
            s.thinking_volume = v;
        }
        if let Some(v) = text("GHOST_THINKING_DELAY_SECS").and_then(|v| v.trim().parse().ok()) {
            s.thinking_delay_secs = v;
        }
        if let Some(v) = text("GHOST_CACHE_DIR") {
            s.cache_dir = PathBuf::from(v);
        }
        s
    }
}

/// The spellings pydantic accepts for a bool.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        _ => None,
    }
}

/// A JSON list of strings, as pydantic-settings reads a `list[str]` from the environment.
pub fn parse_list(text: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Vec<String>>(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_are_the_pythons() {
        let s = AudioSettings::from_vars(|_| None);
        assert_eq!(s.input_device, None);
        assert_eq!(s.output_device, None);
        assert_eq!(s.duck_apps, vec!["spotify".to_string()]);
        assert_eq!(s.duck_level, 0.25);
        assert!(s.chime);
        assert!(s.thinking_sound);
        assert_eq!(s.thinking_volume, 0.12);
        assert_eq!(s.thinking_delay_secs, 0.5);
        assert!(s.cache_dir.ends_with(".cache/ghost"));
    }

    #[test]
    fn reads_every_name() {
        let s = AudioSettings::from_vars(vars(&[
            ("GHOST_INPUT_DEVICE", "ghost.mic"),
            ("GHOST_OUTPUT_DEVICE", "ghost_test_out"),
            ("GHOST_DUCK_APPS", r#"["spotify", "rhythmbox"]"#),
            ("GHOST_DUCK_LEVEL", "0.1"),
            ("GHOST_CHIME", "0"),
            ("GHOST_THINKING_SOUND", "no"),
            ("GHOST_THINKING_VOLUME", "0.2"),
            ("GHOST_THINKING_DELAY_SECS", "1"),
            ("GHOST_CACHE_DIR", "/tmp/ghost-cache"),
        ]));
        assert_eq!(s.input_device.as_deref(), Some("ghost.mic"));
        assert_eq!(s.output_device.as_deref(), Some("ghost_test_out"));
        assert_eq!(
            s.duck_apps,
            vec!["spotify".to_string(), "rhythmbox".to_string()]
        );
        assert_eq!(s.duck_level, 0.1);
        assert!(!s.chime);
        assert!(!s.thinking_sound);
        assert_eq!(s.thinking_volume, 0.2);
        assert_eq!(s.thinking_delay_secs, 1.0);
        assert_eq!(s.cache_dir, PathBuf::from("/tmp/ghost-cache"));
    }

    #[test]
    fn empty_and_bad_values_keep_the_default() {
        let s = AudioSettings::from_vars(vars(&[
            ("GHOST_INPUT_DEVICE", ""),
            ("GHOST_DUCK_APPS", "[]"),
            ("GHOST_DUCK_LEVEL", "loud"),
            ("GHOST_CHIME", "maybe"),
        ]));
        assert_eq!(s.input_device, None);
        assert_eq!(
            s.duck_apps,
            Vec::<String>::new(),
            "the live-test recipe's GHOST_DUCK_APPS='[]'"
        );
        assert_eq!(s.duck_level, 0.25);
        assert!(s.chime);
        let s = AudioSettings::from_vars(vars(&[("GHOST_DUCK_APPS", "spotify")]));
        assert_eq!(
            s.duck_apps,
            vec!["spotify".to_string()],
            "not JSON: the default stays"
        );
    }

    #[test]
    fn bool_spellings() {
        for yes in ["1", "on", "t", "true", "y", "yes", "TRUE", " Yes "] {
            assert_eq!(parse_bool(yes), Some(true), "{yes}");
        }
        for no in ["0", "off", "f", "false", "n", "no", "False"] {
            assert_eq!(parse_bool(no), Some(false), "{no}");
        }
        assert_eq!(parse_bool(""), None);
        assert_eq!(parse_bool("2"), None);
    }
}
