//! Which capture node Ghost listens on, and which of its channels (ADR 0064).
//!
//! A setting names an input as `node` or `node#N`: the PipeWire `node.name`, and with
//! `#N` the one channel N (from 0) taken alone, never mixed with the others. A mic array
//! that cancels echo on its own chip puts its processed beam on channel 0 and a raw
//! microphone beside it; mixed down to mono (what a one-channel capture of a two-channel
//! source gets by default) the raw microphone's uncancelled audio comes back in, so such a
//! device is listened to as `node#0`. Empty or unset: the system's default source, mixed.

use std::collections::BTreeMap;
use std::fmt;

/// An input: a node (None: the default source) and the channel taken alone, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct InputSpec {
    pub node: Option<String>,
    pub channel: Option<u16>,
}

impl InputSpec {
    /// `node`, `node#N`, `#N` (the default source's channel N), or empty for the default.
    /// A `#` suffix that is not a small number is part of the name.
    pub fn parse(text: Option<&str>) -> Self {
        let text = text.map(str::trim).unwrap_or("");
        if text.is_empty() {
            return Self::default();
        }
        if let Some((node, ch)) = text.rsplit_once('#')
            && let Ok(ch) = ch.trim().parse::<u16>()
            && ch < 64
        {
            let node = node.trim();
            return Self {
                node: (!node.is_empty()).then(|| node.to_string()),
                channel: Some(ch),
            };
        }
        Self {
            node: Some(text.to_string()),
            channel: None,
        }
    }

    pub fn node(node: impl Into<String>) -> Self {
        Self {
            node: Some(node.into()),
            channel: None,
        }
    }

    /// The setting's text for this input (`node`, `node#N`, `#N`), None for the default.
    pub fn to_setting(&self) -> Option<String> {
        match (&self.node, self.channel) {
            (None, None) => None,
            (None, Some(c)) => Some(format!("#{c}")),
            _ => Some(self.to_string()),
        }
    }

    /// Ghost's own echo-cancelled source (`infra/pipewire`), by name.
    pub fn is_canceller(&self) -> bool {
        self.node.as_deref() == Some(CANCELLER_SOURCE)
    }
}

impl fmt::Display for InputSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.node, self.channel) {
            (Some(n), Some(c)) => write!(f, "{n}#{c}"),
            (Some(n), None) => f.write_str(n),
            (None, Some(c)) => write!(f, "default source#{c}"),
            (None, None) => f.write_str("default source"),
        }
    }
}

/// The source `ghost-echo-cancel` makes.
pub const CANCELLER_SOURCE: &str = "ghost.mic";

/// Devices known to cancel echo on their own chip against what they play, by a word of
/// their PipeWire names. The XVF3800 is measured (ADR 0064: Ghost's voice at about −82
/// dBFS on its channel 0 while it read aloud through the array's own output); the others
/// are speakerphones sold on it. A match is a recommendation, never a switch made alone.
pub const ON_CHIP_CANCELLERS: &[&str] = &[
    "xvf3800",
    "xvf3000",
    "respeaker",
    "speakerphone",
    "jabra_speak",
    "powerconf",
];

/// A node's props name a device known to cancel echo on chip.
pub fn cancels_on_chip(props: &BTreeMap<String, String>) -> bool {
    let text = [
        "node.name",
        "node.description",
        "device.product.name",
        "alsa.card_name",
    ]
    .iter()
    .filter_map(|k| props.get(*k))
    .map(|v| v.to_ascii_lowercase().replace([' ', '-'], "_"))
    .collect::<Vec<_>>()
    .join(" ");
    ON_CHIP_CANCELLERS
        .iter()
        .any(|w| text.contains(&w.replace([' ', '-'], "_")))
}

/// The channel carrying a known on-chip canceller's processed signal: its first.
pub const ON_CHIP_PROCESSED_CHANNEL: u16 = 0;

/// The device a node belongs to (`device.id`), so a capture node and a sink can be told to
/// be one piece of hardware.
pub fn device_id(props: &BTreeMap<String, String>) -> Option<&str> {
    props.get("device.id").map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_names_a_node_and_maybe_one_channel() {
        assert_eq!(InputSpec::parse(None), InputSpec::default());
        assert_eq!(InputSpec::parse(Some("  ")), InputSpec::default());
        assert_eq!(
            InputSpec::parse(Some("ghost.mic")),
            InputSpec::node("ghost.mic")
        );
        assert_eq!(
            InputSpec::parse(Some("alsa_input.usb-x.iec958-stereo#0")),
            InputSpec {
                node: Some("alsa_input.usb-x.iec958-stereo".into()),
                channel: Some(0)
            }
        );
        assert_eq!(
            InputSpec::parse(Some("#1")),
            InputSpec {
                node: None,
                channel: Some(1)
            }
        );
        // Not a channel: part of the name.
        assert_eq!(
            InputSpec::parse(Some("odd#name")),
            InputSpec::node("odd#name")
        );
        assert_eq!(
            InputSpec::parse(Some("mic#999")),
            InputSpec::node("mic#999")
        );
        for text in ["ghost.mic", "a.b#0", "#2"] {
            let spec = InputSpec::parse(Some(text));
            let shown = spec.to_string().replace("default source", "");
            assert_eq!(InputSpec::parse(Some(&shown)), spec);
        }
        assert!(InputSpec::parse(Some("ghost.mic")).is_canceller());
        assert!(!InputSpec::parse(Some("alsa_input.x#0")).is_canceller());
        assert!(!InputSpec::default().is_canceller());
    }

    fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_array_is_known_to_cancel_on_chip_and_a_webcam_is_not() {
        assert!(cancels_on_chip(&props(&[(
            "node.name",
            "alsa_input.usb-Seeed_Studio_reSpeaker_XVF3800_4-Mic_Array_1-00.iec958-stereo"
        )])));
        assert!(cancels_on_chip(&props(&[(
            "node.description",
            "Jabra SPEAK 510 Mono"
        )])));
        assert!(!cancels_on_chip(&props(&[(
            "node.name",
            "alsa_input.usb-046d_HD_Webcam_C615-00.mono-fallback"
        )])));
        assert!(!cancels_on_chip(&props(&[("node.name", "ghost.mic")])));
    }
}
