#!/usr/bin/env bash
# ONNX Runtime 1.24.4, the build the Python daemon ran and the one every model is bit-exact
# on (heka-vault/ghost#39, W5): fetched once from Microsoft's NuGet package, pinned by
# SHA-256, into Ghost's cache, where `ghost_infer::runtime` finds it for `cargo nextest`,
# `make dev` and `ghostd` outside the bundle. `ort` is built with `load-dynamic`
# (Cargo.toml), so no library is linked at build time and none is downloaded by the crate.
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
LIBRARY="libonnxruntime.so.$VERSION"
# Microsoft's NuGet package. Until 2026-11 this came from the linux-x64 archive of
# github.com/microsoft/onnxruntime's release, which stopped answering (404, the repository
# with it). The package's linux-x64 library is that archive's library byte for byte (the
# shipped 2026.11.4 bundle's copy has the same SHA-256), so the parity it was measured on
# holds; LIBRARY_SHA256 checks it after the package's own pin.
PACKAGE="microsoft.ml.onnxruntime.$VERSION.nupkg"
URL="https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/$VERSION/$PACKAGE"
SHA256=4b978d5065b85e7004b6c6f60ca494bd978fbe6836cbf0a0b52d82b61ab99638
SIZE=125194303
LIBRARY_SHA256=d132535d051344ff5c64c9c200004150559049a81ed330eb4422c1962fb6b7e4

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
  command -v unzip >/dev/null || { echo "onnxruntime: needs unzip (apt install unzip)" >&2; exit 1; }
  local tmp
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/onnxruntime.XXXXXX")
  # Expanded now: the trap runs at exit, after this function's locals are gone.
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  echo "onnxruntime: fetching $URL" >&2
  curl -fsSL --retry 3 -o "$tmp/$PACKAGE" "$URL"
  local got
  got=$(sha256sum "$tmp/$PACKAGE" | cut -d' ' -f1)
  if [ "$got" != "$SHA256" ]; then
    echo "onnxruntime: $PACKAGE does not match the pinned SHA-256" >&2
    echo "  expected $SHA256" >&2
    echo "  got      $got ($(stat -c %s "$tmp/$PACKAGE") bytes; $SIZE expected)" >&2
    echo "  the package is not used; check it and the pin in $0" >&2
    exit 1
  fi
  local native="runtimes/linux-x64/native/libonnxruntime.so"
  unzip -q -o "$tmp/$PACKAGE" "$native" LICENSE -d "$tmp"
  got=$(sha256sum "$tmp/$native" | cut -d' ' -f1)
  if [ "$got" != "$LIBRARY_SHA256" ]; then
    echo "onnxruntime: the package's library is not the one Ghost was measured on" >&2
    echo "  expected $LIBRARY_SHA256" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  mkdir -p "$CACHE/lib"
  install -m 755 "$tmp/$native" "$CACHE/lib/$LIBRARY.part"
  # The package's LICENSE has Windows line ends; the text is the archive's.
  tr -d '\r' <"$tmp/LICENSE" >"$CACHE/LICENSE"
  chmod 644 "$CACHE/LICENSE"
  echo "$VERSION" >"$CACHE/VERSION_NUMBER"
  # The chain the archive had: libonnxruntime.so -> .so.1 (its SONAME) -> .so.1.24.4.
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
