#!/usr/bin/env bash
# The bundle's external binaries (ADR 0038): ghostd, ghost-mock-github and ghost-scripts,
# built from this workspace and copied where Tauri's bundler looks for them,
# apps/desktop/src-tauri/binaries/<name>-<target triple>. `tauri build` runs this first
# (beforeBuildCommand in tauri.conf.json); `make bundle` is `tauri build`.
#
#   scripts/sidecars.sh            release builds (the bundle's)
#   scripts/sidecars.sh dev        debug builds, for trying the bundle layout quickly
#
# The shell's build script hides externalBin from tauri-build, so a plain cargo build or
# clippy never needs these files; only the bundler does.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE=${1:-release}
TRIPLE=${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}
DEST="$ROOT/apps/desktop/src-tauri/binaries"
case "$PROFILE" in
  release) OUT="$ROOT/target/release"; FLAG=--release ;;
  dev) OUT="$ROOT/target/debug"; FLAG= ;;
  *) echo "usage: $0 [release|dev]" >&2; exit 2 ;;
esac
# CI builds against the lock file as it is committed.
LOCKED=${CI:+--locked}

echo "sidecars: cargo build $FLAG $LOCKED -p ghostd -p ghost-core --bins" >&2
# shellcheck disable=SC2086
(cd "$ROOT" && cargo build $FLAG $LOCKED -p ghostd -p ghost-core --bins)
mkdir -p "$DEST"
for name in ghostd ghost-mock-github ghost-scripts; do
  install -m 755 "$OUT/$name" "$DEST/$name-$TRIPLE"
  echo "sidecars: $DEST/$name-$TRIPLE ($(du -h "$OUT/$name" | cut -f1))" >&2
done
