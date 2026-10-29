//! Where the four small model files are (heka-vault/ghost#39, W10): Silero VAD
//! (`silero_vad.onnx`), Smart Turn v3.2 (`smart-turn-v3.2-cpu.onnx`) and the wake front
//! end's two graphs (`melspectrogram.onnx`, `embedding_model.onnx`). The Python wheels
//! shipped them; `scripts/models.sh` fetches them from their upstream releases, pinned by
//! SHA-256 to the wheels' bytes. Each is looked for, in order, as `runtime` looks for ONNX
//! Runtime:
//!
//! 1. `GHOST_MODELS_DIR`, when set: a folder of the operator's or a test's, taken as given.
//! 2. The bundle's resources beside the running executable: `/usr/bin/ghostd` finds
//!    `/usr/lib/heka-ghost-desktop/models/<file>` in the .deb, the same `usr/` inside the
//!    AppImage (`bundle.resources` in `tauri.conf.json`).
//! 3. Ghost's cache, `${GHOST_CACHE_DIR:-~/.cache/ghost}/models/<file>`, where
//!    `scripts/models.sh` puts them for `cargo nextest`, `make dev` and a daemon run from
//!    the checkout.
//!
//! Otherwise an error that names the script and the variable. Parakeet, Kokoro and the
//! operator's trained wake classifiers are not here: they stay in the Hugging Face cache,
//! `~/.cache/ghost/kokoro/` and `~/.cache/ghost/wake/` (docs/releasing.md).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::runtime::{ghost_cache, non_empty, resources_beside};
use crate::{InferError, Result};

/// Silero VAD, as Pipecat ships it (`pipecat/audio/vad/data/`).
pub const SILERO: &str = "silero_vad.onnx";
/// Smart Turn v3.2 for the CPU, as Pipecat ships it (`pipecat/audio/turn/smart_turn/data/`).
pub const SMART_TURN: &str = "smart-turn-v3.2-cpu.onnx";
/// openWakeWord's mel spectrogram, as livekit-wakeword ships it (`livekit/wakeword/resources/`).
pub const MELSPECTROGRAM: &str = "melspectrogram.onnx";
/// openWakeWord's speech embedding, beside it.
pub const EMBEDDING: &str = "embedding_model.onnx";
/// The four, in the order `scripts/models.sh` fetches them.
pub const FILES: [&str; 4] = [SILERO, SMART_TURN, MELSPECTROGRAM, EMBEDDING];
/// The operator's or a test's own folder of them, honoured first.
pub const ENV_VAR: &str = "GHOST_MODELS_DIR";
/// What fetches them into the cache.
pub const SCRIPT: &str = "scripts/models.sh";
/// Their folder under the bundle's resources and under Ghost's cache.
pub const FOLDER: &str = "models";

/// The files' folder in the bundle, for the executable at `exe`: `<dir of exe>/../lib/
/// heka-ghost-desktop/models` (`runtime::resources_beside`).
pub fn bundle_dir(exe: &Path) -> Option<PathBuf> {
    Some(resources_beside(exe.parent()?)?.join(FOLDER))
}

/// The files' folder in Ghost's cache: `GHOST_CACHE_DIR`, else `~/.cache/ghost`, then
/// `models/`.
pub fn cache_dir(cache_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    Some(ghost_cache(cache_dir, home)?.join(FOLDER))
}

/// The file `name` to load, from what the process offers: `env` looks a variable up,
/// `exe` is the running executable, `home` the operator's home. Pure over those three.
pub fn resolve(
    name: &str,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(dir) = non_empty(env(ENV_VAR)) {
        return Ok(dir.join(name));
    }
    let bundle = exe.and_then(bundle_dir).map(|d| d.join(name));
    if let Some(path) = &bundle
        && path.is_file()
    {
        return Ok(path.clone());
    }
    let cache_root = non_empty(env("GHOST_CACHE_DIR"));
    let cache = cache_dir(cache_root.as_deref(), home).map(|d| d.join(name));
    if let Some(path) = &cache
        && path.is_file()
    {
        return Ok(path.clone());
    }
    let looked = [bundle, cache]
        .into_iter()
        .flatten()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(InferError::Model(format!(
        "{name} was not found (looked in {looked}); \
         run {SCRIPT} to fetch it into Ghost's cache, or set {ENV_VAR} to a folder that has it"
    )))
}

/// `resolve` over this process: its environment, its executable and `HOME`.
pub fn resolve_for_process(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve(
        name,
        |var| std::env::var_os(var),
        exe.as_deref(),
        home.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::*;
    use crate::runtime::BUNDLE_NAME;

    struct Layout {
        _dir: tempfile::TempDir,
        exe: PathBuf,
        bundle: PathBuf,
        home: PathBuf,
        home_cache: PathBuf,
        other_cache_dir: PathBuf,
        other_cache: PathBuf,
        own_dir: PathBuf,
    }

    /// A temp tree shaped like an installed bundle and a home: `usr/bin/ghostd`,
    /// `usr/lib/heka-ghost-desktop/models/`, `home/.cache/ghost/models/`, a second cache
    /// and a folder of the operator's own; nothing in them yet.
    fn layout() -> Layout {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exe = root.join("usr/bin/ghostd");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"").unwrap();
        let bundle = root.join("usr/lib").join(BUNDLE_NAME).join(FOLDER);
        let home = root.join("home");
        let home_cache = home.join(".cache/ghost").join(FOLDER);
        let other_cache_dir = root.join("elsewhere");
        let other_cache = other_cache_dir.join(FOLDER);
        let own_dir = root.join("mine");
        Layout {
            _dir: dir,
            exe,
            bundle,
            home,
            home_cache,
            other_cache_dir,
            other_cache,
            own_dir,
        }
    }

    fn put(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, b"not a model").unwrap();
        path
    }

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).map(OsString::from)
    }

    #[test]
    fn the_variable_wins_as_given_even_over_a_bundle_and_a_cache() {
        let l = layout();
        put(&l.bundle, SILERO);
        put(&l.home_cache, SILERO);
        let got = resolve(
            SILERO,
            env(&[(ENV_VAR, l.own_dir.to_str().unwrap())]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        // Taken as given: the folder needn't even exist.
        assert_eq!(got, l.own_dir.join(SILERO));
    }

    #[test]
    fn an_empty_variable_is_unset() {
        let l = layout();
        let cached = put(&l.home_cache, SMART_TURN);
        let got = resolve(
            SMART_TURN,
            env(&[(ENV_VAR, "")]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        assert_eq!(got, cached);
    }

    #[test]
    fn the_bundle_beside_the_executable_wins_over_the_cache() {
        let l = layout();
        let bundled = put(&l.bundle, MELSPECTROGRAM);
        put(&l.home_cache, MELSPECTROGRAM);
        let got = resolve(MELSPECTROGRAM, env(&[]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, bundled);
    }

    #[test]
    fn each_file_is_found_on_its_own() {
        // A bundle with two of the four and a cache with the other two: each resolves
        // where it is, and nothing is assumed from a sibling.
        let l = layout();
        let a = put(&l.bundle, SILERO);
        let b = put(&l.bundle, SMART_TURN);
        let c = put(&l.home_cache, MELSPECTROGRAM);
        let d = put(&l.home_cache, EMBEDDING);
        let got: Vec<PathBuf> = FILES
            .iter()
            .map(|f| resolve(f, env(&[]), Some(&l.exe), Some(&l.home)).unwrap())
            .collect();
        assert_eq!(got, vec![a, b, c, d]);
    }

    #[test]
    fn the_cache_under_home_when_there_is_no_bundle() {
        let l = layout();
        let cached = put(&l.home_cache, EMBEDDING);
        let got = resolve(EMBEDDING, env(&[]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, cached);
    }

    #[test]
    fn ghost_cache_dir_moves_the_cache() {
        let l = layout();
        let in_home = put(&l.home_cache, SILERO);
        let elsewhere = put(&l.other_cache, SILERO);
        let got = resolve(
            SILERO,
            env(&[("GHOST_CACHE_DIR", l.other_cache_dir.to_str().unwrap())]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        assert_eq!(got, elsewhere);
        // Set but empty is unset: the home cache again.
        let got = resolve(
            SILERO,
            env(&[("GHOST_CACHE_DIR", "")]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        assert_eq!(got, in_home);
    }

    #[test]
    fn nothing_found_names_the_script_and_the_variable() {
        let l = layout();
        let err = resolve(SMART_TURN, env(&[]), Some(&l.exe), Some(&l.home)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(SCRIPT), "{text}");
        assert!(text.contains(ENV_VAR), "{text}");
        assert!(text.contains(SMART_TURN), "{text}");
        assert!(
            text.contains(&l.bundle.join(SMART_TURN).display().to_string()),
            "{text}"
        );
        assert!(
            text.contains(&l.home_cache.join(SMART_TURN).display().to_string()),
            "{text}"
        );
    }

    #[test]
    fn no_executable_and_no_home_is_an_error_not_a_panic() {
        let err = resolve(SILERO, env(&[]), None, None).unwrap_err();
        assert!(matches!(err, InferError::Model(_)));
    }

    #[test]
    fn the_paths_are_the_documented_ones() {
        assert_eq!(
            bundle_dir(Path::new("/usr/bin/ghostd")).unwrap(),
            PathBuf::from("/usr/lib/heka-ghost-desktop/models")
        );
        assert_eq!(
            bundle_dir(Path::new("/tmp/app.AppDir/usr/bin/ghostd")).unwrap(),
            PathBuf::from("/tmp/app.AppDir/usr/lib/heka-ghost-desktop/models")
        );
        assert!(bundle_dir(Path::new("ghostd")).is_none());
        assert_eq!(
            cache_dir(None, Some(Path::new("/home/op"))).unwrap(),
            PathBuf::from("/home/op/.cache/ghost/models")
        );
        assert_eq!(
            cache_dir(Some(Path::new("/tmp/c")), Some(Path::new("/home/op"))).unwrap(),
            PathBuf::from("/tmp/c/models")
        );
        assert!(cache_dir(None, None).is_none());
        assert_eq!(
            FILES,
            [
                "silero_vad.onnx",
                "smart-turn-v3.2-cpu.onnx",
                "melspectrogram.onnx",
                "embedding_model.onnx"
            ]
        );
    }
}
