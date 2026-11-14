//! Which uploaded source map each of the app's scripts belongs to (ADR 0067): the release
//! build writes `report-chunks.json` into the bundle, one entry per script, `assets/<name>.js`
//! to the chunk id written into that script and its source map (`tools/report_chunks.py`, the
//! same two marks `posthog-cli sourcemap inject` writes). A page's frame in one of those files
//! carries the id, and PostHog resolves it against the map uploaded under that id, so the frame
//! names a TypeScript file and line.
//!
//! The manifest is read from the app's own bundle, never from a page, and is still held to a
//! closed shape: a JSON object of at most [`MAX_ENTRIES`] entries and [`MAX_BYTES`] bytes, each
//! key a script under `assets/`, each value a UUID. Anything else is refused whole, and the
//! frames go as they did before there were ids: minified, with the version.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// The manifest's file in the bundle.
pub const FILE: &str = "report-chunks.json";
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_ENTRIES: usize = 256;

static SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^assets/[A-Za-z0-9_.-]{1,128}\.js$").unwrap());

/// A script's path in the bundle → its chunk id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageChunks(BTreeMap<String, String>);

impl PageChunks {
    /// The manifest as the build wrote it; refused whole when it is not the shape.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("{FILE} is over {MAX_BYTES} bytes"));
        }
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| format!("{FILE} is not JSON"))?;
        let object = value
            .as_object()
            .ok_or_else(|| format!("{FILE} is not an object"))?;
        if object.len() > MAX_ENTRIES {
            return Err(format!("{FILE} has over {MAX_ENTRIES} entries"));
        }
        let mut map = BTreeMap::new();
        for (file, id) in object {
            if !SCRIPT.is_match(file) || file.contains("..") {
                return Err(format!("{FILE}: not a script of the bundle"));
            }
            let id = id
                .as_str()
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .filter(|u| !u.is_nil())
                .ok_or_else(|| format!("{FILE}: a chunk id is not a UUID"))?;
            map.insert(file.clone(), id.hyphenated().to_string());
        }
        Ok(Self(map))
    }

    /// The chunk id of a script, by its path in the bundle (`assets/index-AbC12.js`).
    pub fn get(&self, file: &str) -> Option<&str> {
        self.0.get(file).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1b7a3c6e-2f1d-5e8a-9b0c-4d3e2f1a0b9c";

    #[test]
    fn the_manifest_the_build_writes_is_read() {
        let c = PageChunks::parse(
            format!(
                r#"{{"assets/index-AbC12_-x.js": "{ID}", "assets/floating-Zz9.js": "{}"}}"#,
                ID.to_uppercase()
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get("assets/index-AbC12_-x.js"), Some(ID));
        assert_eq!(
            c.get("assets/floating-Zz9.js"),
            Some(ID),
            "kept lower-case, as the map has it"
        );
        assert_eq!(c.get("assets/other.js"), None);
    }

    #[test]
    fn anything_but_the_shape_is_refused_whole() {
        for bad in [
            "[]".to_string(),
            "not json".to_string(),
            format!(r#"{{"../secret.js": "{ID}"}}"#),
            format!(r#"{{"https://evil.example/x.js": "{ID}"}}"#),
            format!(r#"{{"assets/a b.js": "{ID}"}}"#),
            format!(r#"{{"assets/x.css": "{ID}"}}"#),
            format!(r#"{{"assets/../x.js": "{ID}"}}"#),
            r#"{"assets/x.js": "not-a-uuid"}"#.to_string(),
            r#"{"assets/x.js": 42}"#.to_string(),
            r#"{"assets/x.js": "00000000-0000-0000-0000-000000000000"}"#.to_string(),
        ] {
            assert!(PageChunks::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let many: serde_json::Map<String, serde_json::Value> = (0..=MAX_ENTRIES)
            .map(|i| (format!("assets/c{i}.js"), ID.into()))
            .collect();
        assert!(PageChunks::parse(serde_json::to_string(&many).unwrap().as_bytes()).is_err());
        let huge = format!(
            r#"{{"assets/x.js": "{ID}", "pad": "{}"}}"#,
            "x".repeat(MAX_BYTES)
        );
        assert!(PageChunks::parse(huge.as_bytes()).is_err());
    }

    #[test]
    fn the_build_derives_chunk_ids_as_posthog_cli_does() {
        // posthog-cli 0.20.0's stable_chunk_id: Uuid::new_v5(CHUNK_ID_NAMESPACE, source).
        // tools/tests/test_report_chunks.py asserts the same value for the same script.
        let ns = uuid::Uuid::from_u128(0x0e9b3c7a5d1f42a8b6c4e2d0f8a17593);
        assert_eq!(
            uuid::Uuid::new_v5(&ns, b"code();").to_string(),
            "1bf5417a-ec0f-5e20-9c5f-4f9085e56f72"
        );
    }
}
