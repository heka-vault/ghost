#!/usr/bin/env bash
# ONNX Runtime 1.24.4, the build the Python daemon ran and the one every model is bit-exact
# on (heka-vault/ghost#39, W5): fetched once from Microsoft's release, pinned by SHA-256,
# into Ghost's cache, where `ghost_infer::runtime` finds it for `cargo nextest`, `make dev`
# and `ghostd` outside the bundle. `ort` is built with `load-dynamic` (Cargo.toml), so no
# library is linked at build time and none is downloaded by the crate.
#
#   scripts/onnxruntime.sh          ensure the library; print its absolute path on stdout
#   scripts/onnxruntime.sh stage    also copy it (and LICENSE) under apps/desktop/src-tauri/
#                                   resources/onnxruntime/ for the bundle (scripts/sidecars.sh
#                                   runs this; the folder is not committed)
#   scripts/onnxruntime.sh --help   this
#
# Cache: ${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/onnxruntime/1.24.4/{lib/libonnxruntime.so.1.24.4,
# lib/libonnxruntime.so, LICENSE, VERSION_NUMBER}. Nothing is fetched when the library is
# there. `ORT_DYLIB_PATH` still wins at run time (crates/ghost-infer/src/runtime.rs).
set -euo pipefail

VERSION=1.24.4
ARCHIVE="onnxruntime-linux-x64-$VERSION.tgz"
URL="https://github.com/microsoft/onnxruntime/releases/download/v$VERSION/$ARCHIVE"
SHA256=3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747
SIZE=8155822
LIBRARY="libonnxruntime.so.$VERSION"

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CACHE="${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/onnxruntime/$VERSION"
STAGE="$ROOT/apps/desktop/src-tauri/resources/onnxruntime"

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
  "" | stage) ;;
  -h | --help | help) usage; exit 0 ;;
  *) echo "usage: $0 [stage|--help]" >&2; exit 2 ;;
esac

fetch() {
  local tmp
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/onnxruntime.XXXXXX")
  # Expanded now: the trap runs at exit, after this function's locals are gone.
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  echo "onnxruntime: fetching $URL" >&2
  curl -fsSL --retry 3 -o "$tmp/$ARCHIVE" "$URL"
  local got
  got=$(sha256sum "$tmp/$ARCHIVE" | cut -d' ' -f1)
  if [ "$got" != "$SHA256" ]; then
    echo "onnxruntime: $ARCHIVE does not match the pinned SHA-256" >&2
    echo "  expected $SHA256" >&2
    echo "  got      $got ($(stat -c %s "$tmp/$ARCHIVE") bytes; $SIZE expected)" >&2
    echo "  the archive is not used; check the release and the pin in $0" >&2
    exit 1
  fi
  local top="onnxruntime-linux-x64-$VERSION"
  tar -xzf "$tmp/$ARCHIVE" -C "$tmp" \
    "$top/lib/$LIBRARY" "$top/lib/libonnxruntime.so" "$top/lib/libonnxruntime.so.1" \
    "$top/LICENSE" "$top/VERSION_NUMBER"
  mkdir -p "$CACHE/lib"
  install -m 755 "$tmp/$top/lib/$LIBRARY" "$CACHE/lib/$LIBRARY.part"
  install -m 644 "$tmp/$top/LICENSE" "$CACHE/LICENSE"
  install -m 644 "$tmp/$top/VERSION_NUMBER" "$CACHE/VERSION_NUMBER"
  # The archive's chain: libonnxruntime.so -> .so.1 -> .so.1.24.4.
  ln -sfn "$LIBRARY" "$CACHE/lib/libonnxruntime.so.1"
  ln -sfn "libonnxruntime.so.1" "$CACHE/lib/libonnxruntime.so"
  mv "$CACHE/lib/$LIBRARY.part" "$CACHE/lib/$LIBRARY"
  echo "onnxruntime: $CACHE/lib/$LIBRARY ($(du -h "$CACHE/lib/$LIBRARY" | cut -f1))" >&2
}

if [ ! -f "$CACHE/lib/$LIBRARY" ]; then
  fetch
fi

if [ "${1:-}" = stage ]; then
  mkdir -p "$STAGE"
  install -m 755 "$CACHE/lib/$LIBRARY" "$STAGE/$LIBRARY"
  install -m 644 "$CACHE/LICENSE" "$STAGE/LICENSE"
  echo "onnxruntime: staged $STAGE/$LIBRARY for the bundle" >&2
fi

echo "$CACHE/lib/$LIBRARY"
