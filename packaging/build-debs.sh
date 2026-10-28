#!/usr/bin/env bash
# Build the three Debian packages. Runs inside the heka-ghost-build image (see
# Dockerfile.build) or on any Ubuntu 26.04 box with the same tools. Output: dist/deb/.
#
#   heka-ghostd          /opt/ghost (bundled Python 3.12 + venv), ghostd shim, user unit,
#                        PipeWire echo-cancel templates. No models: first run downloads them.
#   heka-ghost-desktop   the Tauri app, built by `tauri build --bundles deb`.
#   heka-ghost           meta-package depending on both.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=${VERSION:-$(python3 -c "import tomllib;print(tomllib.load(open('$ROOT/pyproject.toml','rb'))['project']['version'])")}
ARCH=$(dpkg --print-architecture)
OUT="$ROOT/dist/deb"
STAGE="$ROOT/dist/stage"
PY_VERSION=${PY_VERSION:-3.12}
ONLY=${1:-all}   # all | daemon | desktop | meta

mkdir -p "$OUT"
echo "== heka-ghost $VERSION ($ARCH)"

build_daemon() {
  echo "== heka-ghostd: bundling Python $PY_VERSION and the venv under /opt/ghost"
  local root="$STAGE/heka-ghostd"
  rm -rf "$root"; mkdir -p "$root/opt/ghost" "$root/usr/bin" "$root/usr/lib/systemd/user" \
    "$root/usr/share/heka-ghost/pipewire" "$root/usr/share/doc/heka-ghostd" "$root/DEBIAN"

  # A self-contained interpreter: the distro's Python is 3.14 and the audio wheels want 3.12.
  UV_PYTHON_INSTALL_DIR="$root/opt/ghost/python" uv python install "$PY_VERSION" --no-bin --no-registry >/dev/null
  # uv also creates an unversioned alias directory as an absolute symlink into the staging
  # tree; drop it and use the real versioned directory so nothing points at $root.
  find "$root/opt/ghost/python" -maxdepth 1 -type l -delete
  local py
  py=$(realpath "$(ls -d "$root/opt/ghost/python"/cpython-"$PY_VERSION".*/bin/python3 | head -1)")
  # The venv is created at its final path by building through a symlink from /opt/ghost
  # when we can, else made relocatable and its pyvenv.cfg rewritten below.
  # --reinstall-package: uv caches built wheels of workspace members by their pyproject,
  # not their sources, so without this a rebuild could ship stale code.
  local reinstall=()
  for member in ghostd ghost-core ghost-voice ghost-api; do
    reinstall+=(--reinstall-package "$member")
  done
  UV_PROJECT_ENVIRONMENT="$root/opt/ghost/venv" UV_VENV_RELOCATABLE=1 \
    uv sync --frozen --no-dev --no-editable --all-packages "${reinstall[@]}" \
      --python "$py" --project "$ROOT" >/dev/null
  # Point the venv at the bundled interpreter's install location after `dpkg -i`.
  local pyhome
  pyhome=$(dirname "$py" | sed "s#^$root##")
  sed -i "s#^home = .*#home = $pyhome#" "$root/opt/ghost/venv/pyvenv.cfg"
  # The venv's python symlinks point at the staging path; relink to the final one.
  for link in "$root/opt/ghost/venv/bin/python" "$root/opt/ghost/venv/bin/python3" "$root/opt/ghost/venv/bin/python$PY_VERSION"; do
    [[ -L "$link" ]] && ln -sfn "$pyhome/python3" "$link"
  done
  # Console scripts were written with the staging interpreter path; point them at /opt.
  # (Regular files only: the python symlinks already point at the final, not-yet-existing path.)
  for f in "$root/opt/ghost/venv/bin/"*; do
    [[ -f "$f" && ! -L "$f" ]] || continue
    if [[ "$(head -c 2 "$f")" == "#!" ]] && head -1 "$f" | grep -q "^#!$root/"; then
      sed -i "1s#^\#!$root#\#!#" "$f"
    fi
  done
  for f in "$root/opt/ghost/venv/bin/"*; do
    [[ -f "$f" && ! -L "$f" ]] || continue
    if head -1 "$f" | grep -q "^#!$root/"; then echo "$f still references $root" >&2; exit 1; fi
  done
  rm -rf "$root/opt/ghost/venv/.lock" "$root/opt/ghost/venv/CACHEDIR.TAG"
  # Any remaining absolute symlink into the staging tree would be dead after install.
  while IFS= read -r -d "" link; do
    target=$(readlink "$link")
    case "$target" in "$root"/*) ln -sfn "${target#"$root"}" "$link" ;; esac
  done < <(find "$root/opt/ghost" -type l -print0)
  if find "$root/opt/ghost" -type l -lname "$root/*" | grep -q .; then
    echo "staging symlinks still point into $root" >&2; exit 1
  fi
  find "$root/opt/ghost" -name "__pycache__" -type d -prune -exec rm -rf {} +

  install -m 755 "$ROOT/packaging/debian/heka-ghostd/ghostd" "$root/usr/bin/ghostd"
  install -m 644 "$ROOT/packaging/debian/heka-ghostd/ghostd.service" "$root/usr/lib/systemd/user/ghostd.service"
  install -m 755 "$ROOT/infra/systemd/ghost-render-env.sh" "$root/usr/share/heka-ghost/ghost-render-env.sh"
  install -m 755 "$ROOT/infra/pipewire/install.sh" "$root/usr/share/heka-ghost/pipewire/install.sh"
  install -m 644 "$ROOT/infra/pipewire/ghost-echo-cancel.conf.in" "$root/usr/share/heka-ghost/pipewire/"
  install -m 644 "$ROOT/infra/pipewire/ghost-echo-cancel.service" "$root/usr/share/heka-ghost/pipewire/"
  install -m 644 "$ROOT/infra/policy.example.toml" "$root/usr/share/heka-ghost/policy.example.toml"
  install -m 644 "$ROOT/README.md" "$root/usr/share/doc/heka-ghostd/README.md"
  install -m 644 "$ROOT/docs/apps.md" "$root/usr/share/doc/heka-ghostd/apps.md"

  local size
  size=$(du -sk "$root" --exclude=DEBIAN | cut -f1)
  sed -e "s/@VERSION@/$VERSION/g" -e "s/@ARCH@/$ARCH/g" -e "s/@SIZE@/$size/g" \
    "$ROOT/packaging/debian/heka-ghostd/control.in" > "$root/DEBIAN/control"
  install -m 755 "$ROOT/packaging/debian/heka-ghostd/postinst" "$root/DEBIAN/postinst"
  install -m 755 "$ROOT/packaging/debian/heka-ghostd/prerm" "$root/DEBIAN/prerm"
  install -m 755 "$ROOT/packaging/debian/heka-ghostd/postrm" "$root/DEBIAN/postrm"
  fakeroot dpkg-deb --build --root-owner-group -Zxz "$root" "$OUT/heka-ghostd_${VERSION}_${ARCH}.deb"
  echo "   $(du -h "$OUT/heka-ghostd_${VERSION}_${ARCH}.deb" | cut -f1)"
}

build_desktop() {
  echo "== heka-ghost-desktop: tauri build"
  # Clear earlier bundles first: a leftover .deb from a previous build (possibly under
  # Tauri's old product name) would otherwise be copied and overwrite the fresh one.
  local bundle="$ROOT/apps/desktop/src-tauri/target/release/bundle/deb"
  rm -rf "$bundle"
  (cd "$ROOT" && pnpm install --frozen-lockfile --config.confirmModulesPurge=false >/dev/null && pnpm gen >/dev/null \
    && pnpm --filter @ghost/desktop exec tauri build --bundles deb 2>&1 | tail -5)
  rm -f "$OUT"/Ghost_*.deb "$OUT"/ghost_*.deb "$OUT"/heka-ghost-desktop_*.deb
  cp "$bundle"/*.deb "$OUT/"
  # Tauri names it after productName; keep our package naming.
  local target="$OUT/heka-ghost-desktop_${VERSION}_${ARCH}.deb"
  for f in "$OUT"/Ghost_*.deb "$OUT"/ghost_*.deb "$OUT"/heka-ghost-desktop_*.deb; do
    [[ -e "$f" && "$f" != "$target" ]] && mv -f "$f" "$target"
  done
  [[ -e "$target" ]] || { echo "desktop .deb not produced" >&2; exit 1; }
  # Tauri names the package after the product; the meta-package depends on the package
  # name, so rewrite the control field and repack.
  local work="$STAGE/heka-ghost-desktop"
  rm -rf "$work"; dpkg-deb -R "$target" "$work"
  sed -i -e "s/^Package: .*/Package: heka-ghost-desktop/" -e "s/^Version: .*/Version: $VERSION/" "$work/DEBIAN/control"
  grep -q "^Depends: .*heka-ghostd" "$work/DEBIAN/control" || sed -i "s/^Depends: /Depends: heka-ghostd, /" "$work/DEBIAN/control"
  fakeroot dpkg-deb --build --root-owner-group -Zxz "$work" "$target" >/dev/null
  echo "   $(du -h "$OUT/heka-ghost-desktop_${VERSION}_${ARCH}.deb" | cut -f1)"
}

build_meta() {
  echo "== heka-ghost: meta-package"
  local root="$STAGE/heka-ghost"
  rm -rf "$root"; mkdir -p "$root/DEBIAN"
  sed -e "s/@VERSION@/$VERSION/g" -e "s/@ARCH@/$ARCH/g" \
    "$ROOT/packaging/debian/heka-ghost/control.in" > "$root/DEBIAN/control"
  fakeroot dpkg-deb --build --root-owner-group "$root" "$OUT/heka-ghost_${VERSION}_${ARCH}.deb"
}

# When run as root in the build container, hand what we wrote back to the host user so
# pnpm/uv on the host keep working on the same tree.
restore_ownership() {
  [[ -n "${HOST_UID:-}" && "$(id -u)" == 0 ]] || return 0
  chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" "$OUT" "$STAGE" \
    "$ROOT"/node_modules "$ROOT"/packages/*/node_modules "$ROOT"/apps/*/node_modules \
    "$ROOT"/apps/desktop/dist "$ROOT"/apps/desktop/src-tauri/target "$ROOT"/.pnpm-store 2>/dev/null || true
}
trap restore_ownership EXIT

case "$ONLY" in
  daemon) build_daemon ;;
  desktop) build_desktop ;;
  meta) build_meta ;;
  all) build_daemon; build_desktop; build_meta ;;
  *) echo "usage: build-debs.sh [all|daemon|desktop|meta]" >&2; exit 2 ;;
esac
echo "== done:"; ls -la "$OUT"
