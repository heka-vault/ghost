//! The Unix socket the API is served on: `$XDG_RUNTIME_DIR/ghost/api.sock`, mode 0600,
//! inside a 0700 directory. One Ghost per socket: a path another daemon answers on is
//! refused, a stale file nobody answers on is replaced.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use axum::Router;
use tokio::net::{UnixListener, UnixStream};
use tracing::info;

/// `$GHOST_API_SOCKET`, else `$XDG_RUNTIME_DIR/ghost/api.sock` (`/tmp/ghost/api.sock` with
/// no runtime dir), the path the desktop app and the CLI compute the same way.
pub fn default_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GHOST_API_SOCKET") {
        return PathBuf::from(p);
    }
    let base =
        std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    base.join("ghost").join("api.sock")
}

/// Bind `path`, refusing it when a daemon already answers there.
pub async fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            bail!(
                "another Ghost answers on {}; stop it first (make stop, or make service-off)",
                path.display()
            );
        }
        std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting 0600 on {}", path.display()))?;
    Ok(listener)
}

/// Serve `app` on `path` until `shutdown` completes, then remove the socket file.
pub async fn serve(
    path: &Path,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let listener = bind(path).await?;
    info!("api: listening on {}", path.display());
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    let _ = std::fs::remove_file(path);
    result.context("serving the control API")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn the_socket_is_0600_and_a_live_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api.sock");
        let listener = bind(&path).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Someone is listening, so a second daemon must not take the path.
        let err = bind(&path).await.unwrap_err().to_string();
        assert!(err.contains("another Ghost answers"), "{err}");
        drop(listener);
    }

    #[tokio::test]
    async fn a_stale_socket_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api.sock");
        drop(UnixListener::bind(&path).unwrap()); // the file stays, nobody answers
        assert!(path.exists());
        let listener = bind(&path).await.unwrap();
        drop(listener);
    }

    #[test]
    fn the_default_path_follows_the_runtime_dir() {
        // Only the shape is checked: the environment is the process's.
        let p = default_socket_path();
        assert!(p.ends_with("ghost/api.sock") || std::env::var_os("GHOST_API_SOCKET").is_some());
    }
}
