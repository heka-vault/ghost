#!/usr/bin/env bash
# Run a commit of this checkout as the operator's Ghost service, without root or a .deb.
#
#   scripts/deploy.sh             build HEAD, check it, switch the service to it, restart
#   scripts/deploy.sh rollback    switch back to the build that ran before, restart
#   scripts/deploy.sh off         remove the drop-in: the service runs what its unit says
#   scripts/deploy.sh status      which build the service runs
#   scripts/deploy.sh dropin [commit]   print the drop-in a build runs with (current by default)
#
# A build is the commit's files (git archive, so uncommitted edits are never in it) and the
# daemon compiled from them (`cargo build --release -p ghostd`, ADR 0039) as bin/ghostd, with
# the ONNX Runtime it loads beside it as lib/libonnxruntime.so.<version>
# (scripts/onnxruntime.sh), under ~/.local/share/ghost/runtime/<commit>/. `current` and
# `previous` are links; the user unit's drop-in runs `current`. The build is checked with
# `ghostd check` against the operator's own folder before anything restarts.
#
# The environment is settings.env only: the daemon opens Ghost's vault itself (ADR 0036). A
# build from before the Rust daemon (no bin/ghostd) can't run any more; a rollback to one
# refuses and changes nothing.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUNTIME="${GHOST_RUNTIME_DIR:-$HOME/.local/share/ghost/runtime}"
UNIT=ghostd
DROPIN_DIR="$HOME/.config/systemd/user/$UNIT.service.d"
DROPIN="$DROPIN_DIR/60-runtime.conf"
KEEP=${GHOST_RUNTIME_KEEP:-5}
CARGO_TARGET=${GHOST_CARGO_TARGET_DIR:-$RUNTIME/.cargo-target}  # shared, so a deploy is incremental
SYSTEMCTL=${GHOST_SYSTEMCTL:-systemctl}  # the tests put a fake here
JOURNALCTL=${GHOST_JOURNALCTL:-journalctl}
ORT_LIB=libonnxruntime.so.1.24.4  # what scripts/onnxruntime.sh fetches (option 1 on #39)

say() { echo "deploy: $*" >&2; }

commit_of() { [[ -L "$RUNTIME/$1" ]] && basename "$(readlink "$RUNTIME/$1")" || true; }

# Point a link at a build atomically (a new link renamed over the old one).
point() {
  ln -sfn "$2" "$RUNTIME/.$1.tmp"
  mv -T "$RUNTIME/.$1.tmp" "$RUNTIME/$1"
}

# A build runs when it has the daemon. One from before the Rust daemon (a venv only) can't.
can_run() {
  local dir=$1
  if [[ ! -x "$dir/bin/ghostd" ]]; then
    say "build $(basename "$dir" | cut -c1-12) is from before the Rust daemon (#39): it has no bin/ghostd"
    say "and can't run any more. Stay on this build, or \`make deploy\` a commit."
    return 1
  fi
}

# The drop-in for the build `current` points at: settings.env only, no render step, the
# build's own daemon and the runtime library beside it when the build carries one.
render_dropin() {
  local dir=$1
  local cur="%h/.local/share/ghost/runtime/current"
  [[ -n "${GHOST_RUNTIME_DIR:-}" ]] && cur="$RUNTIME/current"
  echo "# Written by scripts/deploy.sh: the service runs the build in $RUNTIME/current."
  echo "# \`scripts/deploy.sh off\` removes this file."
  echo "[Service]"
  echo "WorkingDirectory=%h"
  cat <<EOF
# ADR 0036: no render step, and no secret in the environment: settings.env only. The
# daemon opens Ghost's vault itself.
ExecStartPre=
EnvironmentFile=
EnvironmentFile=-%h/.config/ghost/settings.env
Environment=GHOST_INFRA_DIR=$cur/src/infra
EOF
  if [[ -e "$dir/lib/$ORT_LIB" ]]; then
    echo "# The ONNX Runtime this build was checked with (ADR 0039)."
    echo "Environment=ORT_DYLIB_PATH=$cur/lib/$ORT_LIB"
  fi
  cat <<EOF
ExecStart=
ExecStart=$cur/bin/ghostd run
ExecStopPost=
ExecStopPost=$cur/bin/ghostd restore-volumes
EOF
}

write_dropin() {
  mkdir -p "$DROPIN_DIR"
  render_dropin "$RUNTIME/current" > "$DROPIN.tmp"
  mv "$DROPIN.tmp" "$DROPIN"
  "$SYSTEMCTL" --user daemon-reload
}

# `off` hands the service back to its unit file. A unit installed before the vault runs
# infra/systemd/ghost-render-env.sh, which is gone, and needs ~/.config/ghost/env, which the
# import deleted: it would fail to start. Refuse, and say how to install the current unit.
unit_ok() {
  local frag line path prefix bad=()
  frag=$("$SYSTEMCTL" --user show "$UNIT" -p FragmentPath --value 2>/dev/null || true)
  if [[ -z "$frag" || ! -r "$frag" ]]; then
    say "no $UNIT unit file to go back to; \`make service-install\` installs one"
    return 1
  fi
  while IFS= read -r line; do
    case "$line" in
      ExecStartPre=?*)
        path=${line#ExecStartPre=}
        prefix=${path%%[^-@:+!]*}  # systemd's prefixes
        [[ "$prefix" == *-* ]] && continue  # its failure is ignored: the unit still starts
        path=${path#"$prefix"}
        path=${path%% *}
        path=${path//%h/$HOME}
        [[ -x "$path" ]] || bad+=("runs $path, which isn't there")
        ;;
      EnvironmentFile=?*)
        path=${line#EnvironmentFile=}
        [[ "$path" == -* ]] && continue  # optional
        path=${path//%h/$HOME}
        [[ -e "$path" ]] || bad+=("reads $path, which isn't there")
        ;;
    esac
  done < "$frag"
  if (( ${#bad[@]} )); then
    say "the drop-in stays: the $UNIT unit at $frag is from before Ghost's vault and would not start:"
    for b in "${bad[@]}"; do say "  it $b"; done
    say "\`make service-install\` installs the current unit; then run this again."
    return 1
  fi
}

# Restart and wait until the daemon answers on its socket (or the unit gives up).
restart() {
  "$SYSTEMCTL" --user restart "$UNIT"
  local ghostd="$RUNTIME/current/bin/ghostd"
  for _ in $(seq 1 90); do
    case "$("$SYSTEMCTL" --user show "$UNIT" -p ActiveState --value)" in
      failed|inactive) say "$UNIT is not running:"; "$JOURNALCTL" --user -u "$UNIT" -n 30 --no-pager; return 1 ;;
    esac
    if "$ghostd" status >/dev/null 2>&1; then
      say "$UNIT is up on $(commit_of current | cut -c1-12)"
      return 0
    fi
    sleep 2
  done
  say "$UNIT is running but its socket didn't answer within 3 minutes; journalctl --user -u $UNIT"
  return 1
}

prune() {
  local keep_cur keep_prev
  keep_cur=$(commit_of current)
  keep_prev=$(commit_of previous)
  # Newest first; skip the two that are linked.
  ls -1t "$RUNTIME" | grep -E '^[0-9a-f]{40}$' | grep -vx -e "${keep_cur:-none}" -e "${keep_prev:-none}" \
    | tail -n +$((KEEP - 1)) | while read -r old; do rm -rf "${RUNTIME:?}/$old"; done || true
}

# What the daemon reports as its build (ghostd::build_info, docs/releasing.md): a deploy of
# this commit, recorded beside the binary.
record_build() {
  printf '{"kind": "deploy", "commit": "%s"}\n' "$2" > "$1/bin/ghost-build.json"
}

# The daemon (ADR 0039), built from the archived tree with the workspace's own toolchain and
# linker settings (rust-toolchain.toml, .cargo/config.toml, read from the tree), into a
# target dir shared across builds so only what changed compiles.
build_rust() {
  local dir=$1
  say "building the daemon (cargo build --release -p ghostd)"
  (cd "$dir/src" && CARGO_TARGET_DIR="$CARGO_TARGET" cargo build --release --locked -q -p ghostd) >&2
  mkdir -p "$dir/bin"
  install -m 755 "$CARGO_TARGET/release/ghostd" "$dir/bin/ghostd"
}

# The ONNX Runtime the build loads, copied beside it from the cache, so the build is whole on
# its own (the drop-in names it as ORT_DYLIB_PATH).
install_runtime() {
  local dir=$1 lib
  [[ -x "$dir/src/scripts/onnxruntime.sh" ]] || return 0
  lib=$("$dir/src/scripts/onnxruntime.sh") || { say "scripts/onnxruntime.sh failed"; return 1; }
  mkdir -p "$dir/lib"
  install -m 644 "$lib" "$dir/lib/$ORT_LIB"
}

build() {
  local sha dir
  sha=$(git -C "$ROOT" rev-parse HEAD)
  dir="$RUNTIME/$sha"
  if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]]; then
    say "the working tree has uncommitted changes; they are not deployed (building $(git -C "$ROOT" rev-parse --short HEAD))"
  fi
  if [[ -r "$dir/commit" ]]; then
    say "build $(cut -c1-12 <<<"$sha") exists; reusing it"
    [[ -x "$dir/bin/ghostd" ]] || build_rust "$dir"
    [[ -e "$dir/bin/ghost-build.json" ]] || record_build "$dir" "$sha"
    [[ -e "$dir/lib/$ORT_LIB" ]] || install_runtime "$dir"
  else
    say "building $(git -C "$ROOT" log -1 --format='%h %s' "$sha")"
    # `commit` is written last, so a build that stopped halfway has none and is rebuilt.
    rm -rf "$dir"
    mkdir -p "$dir/src"
    git -C "$ROOT" archive "$sha" | tar -x -C "$dir/src"
    build_rust "$dir"
    record_build "$dir" "$sha"
    install_runtime "$dir"
    echo "$sha" > "$dir/commit"
  fi
  echo "$sha"
}

# `ghostd check` from the new build against the operator's real folder, with the service's
# environment. It reads; it starts the apps to list their tools, and stops them.
check() {
  local dir=$1
  say "checking the build against ~/.config/ghost"
  (
    set -a
    [[ -r "$HOME/.config/ghost/settings.env" ]] && . "$HOME/.config/ghost/settings.env"
    set +a
    export GHOST_INFRA_DIR="$dir/src/infra"
    [[ -e "$dir/lib/$ORT_LIB" ]] && export ORT_DYLIB_PATH="$dir/lib/$ORT_LIB"
    "$dir/bin/ghostd" check
  ) > "$dir/check.log" 2>&1 || { say "ghostd check failed; the service was not touched:"; tail -20 "$dir/check.log"; return 1; }
  grep -E '^(version:|config ok|apps:|  app |skills:|folder:|passwords:)' "$dir/check.log" | sed 's/^/  /'
  grep -iE 'no longer read|not read' "$dir/check.log" | sed 's/^/  /' || true
}

cmd=${1:-deploy}
case "$cmd" in
  deploy)
    mkdir -p "$RUNTIME"
    sha=$(build)
    check "$RUNTIME/$sha"
    if [[ "$(commit_of current)" != "$sha" ]]; then
      [[ -n "$(commit_of current)" ]] && point previous "$(commit_of current)"
      point current "$sha"
    fi
    write_dropin
    restart
    prune
    ;;
  rollback)
    prev=$(commit_of previous)
    [[ -n "$prev" && -d "$RUNTIME/$prev" ]] || { say "no previous build to go back to"; exit 1; }
    can_run "$RUNTIME/$prev" || exit 1
    cur=$(commit_of current)
    point current "$prev"
    point previous "$cur"
    write_dropin
    say "back to $(cut -c1-12 <<<"$prev")"
    restart
    ;;
  off)
    unit_ok || exit 1
    rm -f "$DROPIN"
    "$SYSTEMCTL" --user daemon-reload
    say "drop-in removed; $UNIT now runs $("$SYSTEMCTL" --user show "$UNIT" -p ExecStart --value | grep -o 'path=[^ ;]*' | tail -1 | cut -d= -f2)"
    "$SYSTEMCTL" --user restart "$UNIT"
    ;;
  status)
    if [[ -e "$DROPIN" ]]; then
      cur=$(commit_of current)
      say "$UNIT runs build $(cut -c1-12 <<<"$cur"): $(git -C "$ROOT" log -1 --format='%s (%cr)' "$cur" 2>/dev/null || echo '?')"
      [[ -n "$(commit_of previous)" ]] && say "rollback goes to $(commit_of previous | cut -c1-12)"
    else
      say "no drop-in; $UNIT runs what its unit says"
    fi
    "$SYSTEMCTL" --user is-active "$UNIT" | sed "s/^/deploy: $UNIT is /"
    ;;
  dropin)
    sha=${2:-$(commit_of current)}
    [[ -n "$sha" && -d "$RUNTIME/$sha" ]] || { say "no build ${sha:-current} in $RUNTIME"; exit 1; }
    render_dropin "$RUNTIME/$sha"
    ;;
  *) echo "usage: $0 [deploy|rollback|off|status|dropin [commit]]" >&2; exit 2 ;;
esac
