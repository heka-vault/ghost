#!/usr/bin/env bash
# Run a commit of this checkout as the operator's Ghost service, without root or a .deb.
#
#   scripts/deploy.sh             build HEAD, check it, switch the service to it, restart
#   scripts/deploy.sh rollback    switch back to the build that ran before, restart
#   scripts/deploy.sh off         remove the drop-in: the service runs what its unit says
#   scripts/deploy.sh status      which build the service runs
#
# A build is the commit's files (git archive, so uncommitted edits are never in it) and a
# venv with Ghost installed non-editable, under ~/.local/share/ghost/runtime/<commit>/.
# `current` and `previous` are links; the user unit's drop-in runs `current`. The build is
# checked with `ghostd check` against the operator's own folder before anything restarts.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUNTIME="${GHOST_RUNTIME_DIR:-$HOME/.local/share/ghost/runtime}"
UNIT=ghostd
DROPIN_DIR="$HOME/.config/systemd/user/$UNIT.service.d"
DROPIN="$DROPIN_DIR/60-runtime.conf"
KEEP=${GHOST_RUNTIME_KEEP:-5}
UV=${UV:-uv}

say() { echo "deploy: $*" >&2; }

commit_of() { [[ -L "$RUNTIME/$1" ]] && basename "$(readlink "$RUNTIME/$1")" || true; }

# Point a link at a build atomically (a new link renamed over the old one).
point() {
  ln -sfn "$2" "$RUNTIME/.$1.tmp"
  mv -T "$RUNTIME/.$1.tmp" "$RUNTIME/$1"
}

write_dropin() {
  mkdir -p "$DROPIN_DIR"
  local cur="%h/.local/share/ghost/runtime/current"
  [[ -n "${GHOST_RUNTIME_DIR:-}" ]] && cur="$RUNTIME/current"
  cat > "$DROPIN.tmp" <<EOF
# Written by scripts/deploy.sh: the service runs the build in $RUNTIME/current.
# \`scripts/deploy.sh off\` removes this file.
[Service]
WorkingDirectory=%h
# ADR 0036: no render step, and no secret in the environment: settings.env only. The
# daemon opens Ghost's vault itself.
ExecStartPre=
EnvironmentFile=
EnvironmentFile=-%h/.config/ghost/settings.env
Environment=GHOST_INFRA_DIR=$cur/src/infra
ExecStart=
ExecStart=$cur/venv/bin/ghostd run
ExecStopPost=
ExecStopPost=$cur/venv/bin/ghostd restore-volumes
EOF
  mv "$DROPIN.tmp" "$DROPIN"
  systemctl --user daemon-reload
}

# Restart and wait until the daemon answers on its socket (or the unit gives up).
restart() {
  systemctl --user restart "$UNIT"
  local ghostd="$RUNTIME/current/venv/bin/ghostd"
  for _ in $(seq 1 90); do
    case "$(systemctl --user show "$UNIT" -p ActiveState --value)" in
      failed|inactive) say "$UNIT is not running:"; journalctl --user -u "$UNIT" -n 30 --no-pager; return 1 ;;
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

# What the daemon reports as its build (ghostd.build, docs/releasing.md): a deploy of this
# commit. In the venv's root, where ghostd looks (sys.prefix).
record_build() {
  printf '{"kind": "deploy", "commit": "%s"}\n' "$2" > "$1/venv/ghost-build.json"
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
    cur=$(commit_of current)
    point current "$prev"
    point previous "$cur"
    write_dropin
    say "back to $(cut -c1-12 <<<"$prev")"
    restart
    ;;
  off)
    rm -f "$DROPIN"
    systemctl --user daemon-reload
    say "drop-in removed; $UNIT now runs $(systemctl --user show "$UNIT" -p ExecStart --value | grep -o 'path=[^ ;]*' | tail -1 | cut -d= -f2)"
    systemctl --user restart "$UNIT"
    ;;
  status)
    if [[ -e "$DROPIN" ]]; then
      cur=$(commit_of current)
      say "$UNIT runs build $(cut -c1-12 <<<"$cur"): $(git -C "$ROOT" log -1 --format='%s (%cr)' "$cur" 2>/dev/null || echo '?')"
      [[ -n "$(commit_of previous)" ]] && say "rollback goes to $(commit_of previous | cut -c1-12)"
    else
      say "no drop-in; $UNIT runs what its unit says"
    fi
    systemctl --user is-active "$UNIT" | sed "s/^/deploy: $UNIT is /"
    ;;
  *) echo "usage: $0 [deploy|rollback|off|status]" >&2; exit 2 ;;
esac
