//! Where ONNX Runtime's library is (heka-vault/ghost#39, W10). `ort` loads the library at
//! run time (`load-dynamic` in the workspace's Cargo.toml), and the one Ghost runs is
//! 1.24.4, the build the Python daemon ran: on it every model is bit-exact with the oracle
//! (`tests/parity.rs`), where the crate's own download, 1.31.0, moves the int8 kernels and
//! the vocoder. The library is looked for, in order:
//!
//! 1. `ORT_DYLIB_PATH`, when set: the operator's or a test's own library, taken as given.
//! 2. The bundle's resources beside the running executable: `/usr/bin/ghostd` finds
//!    `/usr/lib/heka-ghost-desktop/onnxruntime/libonnxruntime.so.1.24.4` in the .deb, the
//!    same `usr/` inside the AppImage (`bundle.resources` in `tauri.conf.json`).
//! 3. Ghost's cache, `${GHOST_CACHE_DIR:-~/.cache/ghost}/onnxruntime/1.24.4/lib/`, where
//!    `scripts/onnxruntime.sh` puts it for `cargo nextest`, `make dev` and a daemon run
//!    from the checkout.
//!
//! Otherwise an error that names the script and the variable. `session::environment()`
//! resolves once per process, before the first session, and logs the path it chose.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{InferError, Result};

/// The ONNX Runtime release Ghost runs.
pub const VERSION: &str = "1.24.4";
/// The library's file name in the archive, the cache and the bundle.
pub const LIBRARY: &str = "libonnxruntime.so.1.24.4";
/// The bundle's main binary, which names its resource folder (`/usr/lib/<name>/`).
pub const BUNDLE_NAME: &str = "heka-ghost-desktop";
/// `ort`'s own override, honoured first.
pub const ENV_VAR: &str = "ORT_DYLIB_PATH";

/// The bundle's resource folder for the binaries in `bin_dir`: `<bin_dir>/../lib/
/// heka-ghost-desktop`, the `..` taken lexically. `/usr/bin` gives
/// `/usr/lib/heka-ghost-desktop`, in the .deb and in the AppImage's `usr/` alike. The one
/// rule for everything the bundle carries beside its binaries: the runtime, the model
/// files (`models`) and the build record (`ghostd::build_info`).
pub fn resources_beside(bin_dir: &Path) -> Option<PathBuf> {
    Some(bin_dir.parent()?.join("lib").join(BUNDLE_NAME))
}

/// Ghost's cache: `GHOST_CACHE_DIR` when given, else `~/.cache/ghost`.
pub fn ghost_cache(cache_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match cache_dir {
        Some(dir) => Some(dir.to_path_buf()),
        None => Some(home?.join(".cache").join("ghost")),
    }
}

/// The library's path in the bundle, for the executable at `exe`: `<dir of exe>/../lib/
/// heka-ghost-desktop/onnxruntime/<library>` (`resources_beside`).
pub fn bundle_path(exe: &Path) -> Option<PathBuf> {
    Some(
        resources_beside(exe.parent()?)?
            .join("onnxruntime")
            .join(LIBRARY),
    )
}

/// The library's path in Ghost's cache: `GHOST_CACHE_DIR`, else `~/.cache/ghost`.
pub fn cache_path(cache_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    Some(
        ghost_cache(cache_dir, home)?
            .join("onnxruntime")
            .join(VERSION)
            .join("lib")
            .join(LIBRARY),
    )
}

pub(crate) fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The library to load, from what the process offers: `env` looks a variable up, `exe`
/// is the running executable, `home` the operator's home. Pure over those three.
pub fn resolve(
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = non_empty(env(ENV_VAR)) {
        return Ok(path);
    }
    let bundle = exe.and_then(bundle_path);
    if let Some(path) = &bundle
        && path.is_file()
    {
        return Ok(path.clone());
    }
    let cache_dir = non_empty(env("GHOST_CACHE_DIR"));
    let cache = cache_path(cache_dir.as_deref(), home);
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
    Err(InferError::Runtime(format!(
        "ONNX Runtime {VERSION} ({LIBRARY}) was not found (looked in {looked}); \
         run scripts/onnxruntime.sh to fetch it into Ghost's cache, or set {ENV_VAR} to a library"
    )))
}

/// `resolve` over this process: its environment, its executable and `HOME`.
pub fn resolve_for_process() -> Result<PathBuf> {
    let exe = std::env::current_exe().ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve(
        |name| std::env::var_os(name),
        exe.as_deref(),
        home.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::*;

    struct Layout {
        _dir: tempfile::TempDir,
        exe: PathBuf,
        bundle: PathBuf,
        home: PathBuf,
        home_cache: PathBuf,
        other_cache_dir: PathBuf,
        other_cache: PathBuf,
    }

    /// A temp tree shaped like an installed bundle and a home: `usr/bin/ghostd`,
    /// `usr/lib/heka-ghost-desktop/onnxruntime/`, `home/.cache/ghost/onnxruntime/1.24.4/lib/`
    /// and a second cache; nothing in it yet.
    fn layout() -> Layout {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exe = root.join("usr/bin/ghostd");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"").unwrap();
        let bundle = root
            .join("usr/lib")
            .join(BUNDLE_NAME)
            .join("onnxruntime")
            .join(LIBRARY);
        let home = root.join("home");
        let home_cache = home
            .join(".cache/ghost/onnxruntime")
            .join(VERSION)
            .join("lib")
            .join(LIBRARY);
        let other_cache_dir = root.join("elsewhere");
        let other_cache = other_cache_dir
            .join("onnxruntime")
            .join(VERSION)
            .join("lib")
            .join(LIBRARY);
        Layout {
            _dir: dir,
            exe,
            bundle,
            home,
            home_cache,
            other_cache_dir,
            other_cache,
        }
    }

    fn put(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"not a library").unwrap();
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
        put(&l.bundle);
        put(&l.home_cache);
        let got = resolve(
            env(&[(ENV_VAR, "/somewhere/else/libonnxruntime.so")]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        assert_eq!(got, PathBuf::from("/somewhere/else/libonnxruntime.so"));
    }

    #[test]
    fn an_empty_variable_is_unset() {
        let l = layout();
        put(&l.home_cache);
        let got = resolve(env(&[(ENV_VAR, "")]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, l.home_cache);
    }

    #[test]
    fn the_bundle_beside_the_executable_wins_over_the_cache() {
        let l = layout();
        put(&l.bundle);
        put(&l.home_cache);
        let got = resolve(env(&[]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, l.bundle);
    }

    #[test]
    fn the_cache_under_home_when_there_is_no_bundle() {
        let l = layout();
        put(&l.home_cache);
        let got = resolve(env(&[]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, l.home_cache);
    }

    #[test]
    fn ghost_cache_dir_moves_the_cache() {
        let l = layout();
        put(&l.home_cache);
        put(&l.other_cache);
        let got = resolve(
            env(&[("GHOST_CACHE_DIR", l.other_cache_dir.to_str().unwrap())]),
            Some(&l.exe),
            Some(&l.home),
        )
        .unwrap();
        assert_eq!(got, l.other_cache);
        // Set but empty is unset: the home cache again.
        let got = resolve(env(&[("GHOST_CACHE_DIR", "")]), Some(&l.exe), Some(&l.home)).unwrap();
        assert_eq!(got, l.home_cache);
    }

    #[test]
    fn nothing_found_names_the_script_and_the_variable() {
        let l = layout();
        let err = resolve(env(&[]), Some(&l.exe), Some(&l.home)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("scripts/onnxruntime.sh"), "{text}");
        assert!(text.contains(ENV_VAR), "{text}");
        assert!(text.contains(VERSION), "{text}");
        assert!(text.contains(&l.bundle.display().to_string()), "{text}");
        assert!(text.contains(&l.home_cache.display().to_string()), "{text}");
    }

    #[test]
    fn no_executable_and_no_home_is_an_error_not_a_panic() {
        let err = resolve(env(&[]), None, None).unwrap_err();
        assert!(matches!(err, InferError::Runtime(_)));
    }

    #[test]
    fn the_paths_are_the_documented_ones() {
        assert_eq!(
            bundle_path(Path::new("/usr/bin/ghostd")).unwrap(),
            PathBuf::from("/usr/lib/heka-ghost-desktop/onnxruntime/libonnxruntime.so.1.24.4")
        );
        assert_eq!(
            bundle_path(Path::new("/tmp/app.AppDir/usr/bin/ghostd")).unwrap(),
            PathBuf::from(
                "/tmp/app.AppDir/usr/lib/heka-ghost-desktop/onnxruntime/libonnxruntime.so.1.24.4"
            )
        );
        assert!(bundle_path(Path::new("ghostd")).is_none());
        assert_eq!(
            resources_beside(Path::new("/usr/bin")).unwrap(),
            PathBuf::from("/usr/lib/heka-ghost-desktop")
        );
        assert!(resources_beside(Path::new("/")).is_none());
        assert_eq!(
            ghost_cache(None, Some(Path::new("/home/op"))).unwrap(),
            PathBuf::from("/home/op/.cache/ghost")
        );
        assert_eq!(
            cache_path(None, Some(Path::new("/home/op"))).unwrap(),
            PathBuf::from("/home/op/.cache/ghost/onnxruntime/1.24.4/lib/libonnxruntime.so.1.24.4")
        );
        assert_eq!(
            cache_path(Some(Path::new("/tmp/c")), Some(Path::new("/home/op"))).unwrap(),
            PathBuf::from("/tmp/c/onnxruntime/1.24.4/lib/libonnxruntime.so.1.24.4")
        );
        assert!(cache_path(None, None).is_none());
    }
}
