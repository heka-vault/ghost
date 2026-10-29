#!/usr/bin/env bash
# The bundle's external binaries (ADR 0038): ghostd, ghost-mock-github and ghost-scripts,
# built from this workspace and copied where Tauri's bundler looks for them,
# apps/desktop/src-tauri/binaries/<name>-<target triple>; and the bundle's resources under
# apps/desktop/src-tauri/resources/: ONNX Runtime 1.24.4 (scripts/onnxruntime.sh stage), the
# four small model files with their licences (scripts/models.sh stage) and ghost-build.json,
# the record of what this build is (ghostd::build_info, docs/releasing.md). `tauri build`
# runs this first (beforeBuildCommand in tauri.conf.json); `make bundle` is `tauri build`.
#
#   scripts/sidecars.sh            release builds (the bundle's)
#   scripts/sidecars.sh dev        debug builds, for trying the bundle layout quickly
#
# The shell's build script hides externalBin and resources from tauri-build, so a plain
# cargo build or clippy never needs these files; only the bundler does.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE=${1:-release}
TRIPLE=${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}
DEST="$ROOT/apps/desktop/src-tauri/binaries"
RESOURCES="$ROOT/apps/desktop/src-tauri/resources"
# Where cargo puts them: CARGO_TARGET_DIR when set (a shared target dir), else target/.
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
case "$PROFILE" in
  release) OUT="$TARGET/release"; FLAG=--release ;;
  dev) OUT="$TARGET/debug"; FLAG= ;;
  *) echo "usage: $0 [release|dev]" >&2; exit 2 ;;
esac
# CI builds against the lock file as it is committed.
LOCKED=${CI:+--locked}

# The build record the bundle's daemon reads: a release of this commit at the workspace's
# version, under the resources, so /usr/bin/ghostd finds /usr/lib/heka-ghost-desktop/
# ghost-build.json (the AppImage the same under its usr/). Built outside a checkout (a
# tarball), the record has no commit; built from a tree with uncommitted changes to tracked
# files, it says so, as a checkout's daemon does.
record_build() {
  local version commit dirty record
  version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
  [ -n "$version" ] || { echo "sidecars: no version under [workspace.package] in $ROOT/Cargo.toml" >&2; exit 1; }
  record="{\"kind\": \"release\""
  if commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null); then
    dirty=false
    [ -z "$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null)" ] || dirty=true
    record="$record, \"commit\": \"$commit\", \"dirty\": $dirty"
  fi
  record="$record, \"version\": \"$version\"}"
  mkdir -p "$RESOURCES"
  printf '%s\n' "$record" > "$RESOURCES/ghost-build.json"
  echo "sidecars: $RESOURCES/ghost-build.json $record" >&2
}

"$ROOT/scripts/onnxruntime.sh" stage >/dev/null
"$ROOT/scripts/models.sh" stage >/dev/null
record_build

echo "sidecars: cargo build $FLAG $LOCKED -p ghostd -p ghost-core --bins" >&2
# shellcheck disable=SC2086
(cd "$ROOT" && cargo build $FLAG $LOCKED -p ghostd -p ghost-core --bins)
mkdir -p "$DEST"
for name in ghostd ghost-mock-github ghost-scripts; do
  install -m 755 "$OUT/$name" "$DEST/$name-$TRIPLE"
  echo "sidecars: $DEST/$name-$TRIPLE ($(du -h "$OUT/$name" | cut -f1))" >&2
done
