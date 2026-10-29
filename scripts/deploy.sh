#!/usr/bin/env bash
# Run a commit of this checkout as the operator's Ghost service, without root or a .deb.
#
#   scripts/deploy.sh             build HEAD, check it, switch the service to it, restart
#   scripts/deploy.sh rollback    switch back to the build that ran before, restart
#   scripts/deploy.sh off         remove the drop-in: the service runs what its unit says
#   scripts/deploy.sh status      which build the service runs
#   scripts/deploy.sh dropin [commit]   print the drop-in a build runs with (current by default)
#
# A build is the commit's files (git archive, so uncommitted edits are never in it), a venv
# with Ghost installed non-editable, and the Rust daemon (`cargo build --release -p ghostd`,
# ADR 0039) as bin/ghostd, under ~/.local/share/ghost/runtime/<commit>/. `current` and
# `previous` are links; the user unit's drop-in runs `current`, with GHOST_DAEMON_IMPL
# (`python`, the default until W10 of #39 removes it, or `rust`) choosing which daemon the
# service runs; a build records the choice in `daemon-impl`. The build is checked with
# `ghostd check` against the operator's own folder before anything restarts.
#
# Each build records the drop-in it needs (`dropin-form`): `vault`, settings.env only and the
# daemon opens Ghost's vault (ADR 0036), or `render`, a build from before the vault, whose
# passwords come from ~/.config/ghost/env rendered by its own ghost-render-env.sh. A rollback
# writes the form of the build it goes back to (docs/releasing.md).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUNTIME="${GHOST_RUNTIME_DIR:-$HOME/.local/share/ghost/runtime}"
UNIT=ghostd
DROPIN_DIR="$HOME/.config/systemd/user/$UNIT.service.d"
DROPIN="$DROPIN_DIR/60-runtime.conf"
KEEP=${GHOST_RUNTIME_KEEP:-5}
UV=${UV:-uv}
IMPL=${GHOST_DAEMON_IMPL:-}  # which daemon the service runs; a build's record when empty
CARGO_TARGET=${GHOST_CARGO_TARGET_DIR:-$RUNTIME/.cargo-target}  # shared, so a deploy is incremental
SYSTEMCTL=${GHOST_SYSTEMCTL:-systemctl}  # the tests put a fake here
JOURNALCTL=${GHOST_JOURNALCTL:-journalctl}
LEGACY_ENV="$HOME/.config/ghost/env"
# heka-vault's config: a pre-vault build's renderer writes ~/.config/ghost/env from it.
HEKA_ENV="$HOME/.config/heka-vault/ghost.env"

say() { echo "deploy: $*" >&2; }

commit_of() { [[ -L "$RUNTIME/$1" ]] && basename "$(readlink "$RUNTIME/$1")" || true; }

# Point a link at a build atomically (a new link renamed over the old one).
point() {
  ln -sfn "$2" "$RUNTIME/.$1.tmp"
  mv -T "$RUNTIME/.$1.tmp" "$RUNTIME/$1"
}

# The drop-in form a build needs: what it recorded, or for a build with no record, `render`
# when its tree still has ghost-render-env.sh (from before the vault), else `vault`.
form_of() {
  local dir=$1
  if [[ -r "$dir/dropin-form" ]]; then
    tr -d '[:space:]' < "$dir/dropin-form"
  elif [[ -e "$dir/src/infra/systemd/ghost-render-env.sh" ]]; then
    echo render
  else
    echo vault
  fi
}

# Which daemon a build runs: what it recorded, else python (a build from before the Rust
# daemon has no bin/ghostd).
impl_of() {
  local dir=$1
  if [[ -r "$dir/daemon-impl" ]]; then tr -d '[:space:]' < "$dir/daemon-impl"; else echo python; fi
}

# The daemon binary of a build, by its implementation.
ghostd_of() {
  case "$(impl_of "$1")" in
    rust) echo "$1/bin/ghostd" ;;
    *) echo "$1/venv/bin/ghostd" ;;
  esac
}

# A pre-vault build reads its passwords from ~/.config/ghost/env, which the vault's import
# deletes. Its own renderer writes that file again from heka-vault when heka-vault's config is
# still here; without either, refuse instead of starting a Ghost with no passwords.
can_run() {
  local dir=$1
  if [[ "$(impl_of "$dir")" == rust && ! -x "$dir/bin/ghostd" ]]; then
    say "build $(basename "$dir" | cut -c1-12) records the Rust daemon but has no bin/ghostd"
    return 1
  fi
  if [[ "$(form_of "$dir")" == render && ! -e "$LEGACY_ENV" ]]; then
    if [[ -r "$HEKA_ENV" ]]; then
      say "build $(basename "$dir" | cut -c1-12) is from before Ghost's vault: its renderer writes"
      say "~/.config/ghost/env from heka-vault again (${HEKA_ENV/#$HOME/\~}) at its start."
      return 0
    fi
    say "build $(basename "$dir" | cut -c1-12) is from before Ghost's vault: it reads its passwords from"
    say "~/.config/ghost/env, which the import into the vault deleted. It would start with none."
    say "Stay on this build, or put the passwords back in ~/.config/ghost/env by hand first."
    return 1
  fi
}

# The drop-in for the build `current` points at, in the form that build needs and with
# the daemon it records.
render_dropin() {
  local form=$1 impl=${2:-python}
  local cur="%h/.local/share/ghost/runtime/current"
  [[ -n "${GHOST_RUNTIME_DIR:-}" ]] && cur="$RUNTIME/current"
  echo "# Written by scripts/deploy.sh: the service runs the build in $RUNTIME/current."
  echo "# \`scripts/deploy.sh off\` removes this file."
  echo "[Service]"
  echo "WorkingDirectory=%h"
  case "$form" in
    vault) cat <<EOF
# ADR 0036: no render step, and no secret in the environment: settings.env only. The
# daemon opens Ghost's vault itself.
ExecStartPre=
EnvironmentFile=
EnvironmentFile=-%h/.config/ghost/settings.env
EOF
    ;;
    render) cat <<EOF
# A build from before Ghost's vault (ADR 0036): its own renderer writes ~/.config/ghost/env,
# and the daemon reads its passwords from there. Optional (-) so the renderer can run when
# the import deleted it; systemd reads it again for ExecStart.
ExecStartPre=
ExecStartPre=$cur/src/infra/systemd/ghost-render-env.sh
EnvironmentFile=
EnvironmentFile=-%h/.config/ghost/env
EOF
    ;;
    *) say "unknown drop-in form '$form'"; return 1 ;;
  esac
  local ghostd
  case "$impl" in
    rust) ghostd="$cur/bin/ghostd" ;;
    python) ghostd="$cur/venv/bin/ghostd" ;;
    *) say "unknown daemon implementation '$impl'"; return 1 ;;
  esac
  cat <<EOF
Environment=GHOST_INFRA_DIR=$cur/src/infra
# ADR 0039: the Rust daemon (bin/ghostd) or the Python one (venv/bin/ghostd).
Environment=GHOST_DAEMON_IMPL=$impl
ExecStart=
ExecStart=$ghostd run
ExecStopPost=
ExecStopPost=$ghostd restore-volumes
EOF
}

write_dropin() {
  local form impl
  form=$(form_of "$RUNTIME/current")
  impl=$(impl_of "$RUNTIME/current")
  mkdir -p "$DROPIN_DIR"
  render_dropin "$form" "$impl" > "$DROPIN.tmp"
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
  local ghostd
  ghostd=$(ghostd_of "$RUNTIME/current")
  for _ in $(seq 1 90); do
    case "$("$SYSTEMCTL" --user show "$UNIT" -p ActiveState --value)" in
      failed|inactive) say "$UNIT is not running:"; "$JOURNALCTL" --user -u "$UNIT" -n 30 --no-pager; return 1 ;;
    esac
    if "$ghostd" status >/dev/null 2>&1; then
      say "$UNIT is up on $(commit_of current | cut -c1-12) ($(impl_of "$RUNTIME/current") daemon)"
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

# What the daemon reports as its build (ghostd.build, docs/releasing.md): a deploy of this
# commit. In the venv's root, where ghostd looks (sys.prefix).
record_build() {
  printf '{"kind": "deploy", "commit": "%s"}\n' "$2" > "$1/venv/ghost-build.json"
}

# The drop-in form this build needs, for a rollback to it later: this script is the build's
# own (the build is HEAD), and it runs the vault (ADR 0036).
record_form() {
  echo vault > "$1/dropin-form"
}

# Which daemon the service runs from this build: GHOST_DAEMON_IMPL when given, else what
# the build already records, else python.
record_impl() {
  local dir=$1 impl=${IMPL:-$(impl_of "$dir")}
  case "$impl" in
    python|rust) ;;
    *) say "GHOST_DAEMON_IMPL must be python or rust, not '$impl'"; return 1 ;;
  esac
  echo "$impl" > "$dir/daemon-impl"
}

# The Rust daemon (ADR 0039), built from the archived tree with the workspace's own
# toolchain and linker settings (rust-toolchain.toml, .cargo/config.toml, read from the
# tree), into a target dir shared across builds so only what changed compiles.
build_rust() {
  local dir=$1
  say "building the Rust daemon (cargo build --release -p ghostd)"
  (cd "$dir/src" && CARGO_TARGET_DIR="$CARGO_TARGET" cargo build --release --locked -q -p ghostd) >&2
  mkdir -p "$dir/bin"
  install -m 755 "$CARGO_TARGET/release/ghostd" "$dir/bin/ghostd"
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
    [[ -e "$dir/venv/ghost-build.json" ]] || record_build "$dir" "$sha"
    [[ -e "$dir/dropin-form" ]] || record_form "$dir"
    # A build from before the Rust daemon gets it now when asked for.
    if [[ "${IMPL:-}" == rust && ! -x "$dir/bin/ghostd" ]]; then build_rust "$dir"; fi
    record_impl "$dir" || exit 1
  else
    say "building $(git -C "$ROOT" log -1 --format='%h %s' "$sha")"
    # Built in place: a venv records its own path. `commit` is written last, so a build
    # that stopped halfway has none and is rebuilt.
    rm -rf "$dir"
    mkdir -p "$dir/src"
    git -C "$ROOT" archive "$sha" | tar -x -C "$dir/src"
    # --reinstall-package: uv caches workspace members' wheels by their pyproject, not
    # their sources, so a build without it can ship another commit's code.
    local reinstall=()
    for member in ghostd ghost-core ghost-voice ghost-api; do reinstall+=(--reinstall-package "$member"); done
    UV_PROJECT_ENVIRONMENT="$dir/venv" \
      "$UV" sync --frozen --no-dev --no-editable --all-packages "${reinstall[@]}" \
        --project "$dir/src" --quiet >&2
    record_build "$dir" "$sha"
    record_form "$dir"
    build_rust "$dir"
    record_impl "$dir" || exit 1
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
    "$dir/venv/bin/ghostd" check
  ) > "$dir/check.log" 2>&1 || { say "ghostd check failed; the service was not touched:"; tail -20 "$dir/check.log"; return 1; }
  grep -E '^(version:|config ok|apps:|  app |skills:|folder:|passwords:)' "$dir/check.log" | sed 's/^/  /'
  grep -iE 'no longer read|not read' "$dir/check.log" | sed 's/^/  /' || true
  # The Rust daemon has no `check` yet (it arrives with its packages, #39): its version, at least.
  if [[ "$(impl_of "$dir")" == rust ]]; then
    say "the service runs the Rust daemon: $("$dir/bin/ghostd" version)"
  fi
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
    say "back to $(cut -c1-12 <<<"$prev") (drop-in: $(form_of "$RUNTIME/current"), $(impl_of "$RUNTIME/current") daemon)"
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
      say "$UNIT runs build $(cut -c1-12 <<<"$cur") ($(impl_of "$RUNTIME/current") daemon): $(git -C "$ROOT" log -1 --format='%s (%cr)' "$cur" 2>/dev/null || echo '?')"
      [[ -n "$(commit_of previous)" ]] && say "rollback goes to $(commit_of previous | cut -c1-12)"
    else
      say "no drop-in; $UNIT runs what its unit says"
    fi
    "$SYSTEMCTL" --user is-active "$UNIT" | sed "s/^/deploy: $UNIT is /"
    ;;
  dropin)
    sha=${2:-$(commit_of current)}
    [[ -n "$sha" && -d "$RUNTIME/$sha" ]] || { say "no build ${sha:-current} in $RUNTIME"; exit 1; }
    render_dropin "$(form_of "$RUNTIME/$sha")" "$(impl_of "$RUNTIME/$sha")"
    ;;
  *) echo "usage: $0 [deploy|rollback|off|status|dropin [commit]]" >&2; exit 2 ;;
esac
