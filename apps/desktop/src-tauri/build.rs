//! The shell's build script. The bundle's external binaries (`ghostd`, `ghost-mock-github`,
//! `ghost-scripts`, declared in `tauri.conf.json`) are the same workspace's own binaries,
//! and `tauri-build` would copy them from `binaries/` over `target/<profile>/`, where cargo
//! just built them, and fail a plain `cargo build` or `clippy` when `binaries/` is absent.
//! So the build script hides `bundle.externalBin` from `tauri-build` and lets the CLI's
//! bundler, which reads the config on its own, pick them up at `tauri build`.

fn main() {
    let mut merge: serde_json::Value = std::env::var("TAURI_CONFIG")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    merge["bundle"]["externalBin"] = serde_json::Value::Null;
    // Read once by tauri_build::build() below, in this process; nothing else runs here.
    unsafe { std::env::set_var("TAURI_CONFIG", merge.to_string()) };
    tauri_build::build()
}
