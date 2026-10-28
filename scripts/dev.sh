#!/usr/bin/env bash
# Ghost from this checkout, for working on it, with its output in this terminal.
#
#   scripts/dev.sh            the daemon and the desktop app          (make dev)
#   scripts/dev.sh daemon     the daemon only                         (make daemon)
#   scripts/dev.sh desktop    the desktop app only                    (make desktop)
#   scripts/dev.sh reload     restart the daemon a running one started (make reload)
#   scripts/dev.sh preflight  stop the service, refuse if a ghostd runs (make start uses it)
#
# The daemon runs the working tree's .venv. Only one Ghost may have the microphone, so
# starting it stops the ghostd user service first (the always-on Ghost, a `make deploy`
# build); `make service-on` starts that again. The desktop app is `tauri dev`: Vite reloads
# the UI on save and a Rust change rebuilds the window. A Python change needs the daemon
# restarted, which `make reload` does from another terminal without touching the desktop.
# If the daemon exits on its own (a crash, `make stop`) it stays down until `make reload`.
# Ctrl+C stops everything this script started.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
UV="${UV:-uv}"
ENV_FILE="${ENV_FILE:-$HOME/.config/ghost/env}"
RUN_DIR="${XDG_RUNTIME_DIR:-/tmp}/ghost"
RELOAD="$RUN_DIR/dev-reload"
MINE="$ROOT/.venv/bin/ghostd run$"  # this checkout's daemon, and no other

if [[ -t 1 ]]; then bold=$'\e[1m' off=$'\e[0m'; else bold='' off=''; fi
say() { echo "${bold}dev:${off} $*"; }

# Prefix each line with the process's name. Ctrl+C is ignored here so the last lines of a
# stopping process still reach the terminal; the filter ends when its input closes.
prefix() {
  trap '' INT
  if [[ -t 1 ]]; then exec sed -u "s/^/"$'\e'"[$2m$1"$'\e'"[0m /"; else exec sed -u "s/^/$1 /"; fi
}

preflight() {
  if systemctl --user is-active --quiet ghostd 2>/dev/null; then
    systemctl --user stop ghostd
    say "stopped the ghostd service so this daemon gets the microphone; make service-on starts it again"
  fi
  local pids
  if pids="$(pgrep -f 'ghostd run$')"; then
    say "a ghostd is already running (pid ${pids//$'\n'/ }); make stop, then try again"
    return 1
  fi
}

render_env() {
  [[ "$ENV_FILE" == "$HOME/.config/ghost/env" ]] || return 0  # a hand-written .env
  if ! infra/systemd/ghost-render-env.sh; then
    [[ -r "$ENV_FILE" ]] || { say "no $ENV_FILE; run make env"; return 1; }
    say "could not refresh the secrets; using $ENV_FILE as it is"
  fi
}

# Each process gets its own session (setsid), so Ctrl+C reaches only this script, which
# then stops them in order: the daemon gets TERM like systemd sends, the desktop's whole
# process group (pnpm, tauri, Vite, the window) gets TERM too.
DAEMON_PID='' DESKTOP_PID='' STOPPING=0

start_daemon() {
  setsid bash -c 'set -a; . "$1"; set +a; exec "$2" run' _ "$ENV_FILE" "$ROOT/.venv/bin/ghostd" \
    </dev/null > >(prefix "ghostd " 36) 2>&1 &
  DAEMON_PID=$!
}

start_desktop() {
  setsid pnpm -s tauri dev </dev/null > >(prefix "desktop" 35) 2>&1 &
  DESKTOP_PID=$!
}

stop_all() {
  STOPPING=1
  [[ -n "$DAEMON_PID" ]] && kill -TERM -- "-$DAEMON_PID" 2>/dev/null
  [[ -n "$DESKTOP_PID" ]] && kill -TERM -- "-$DESKTOP_PID" 2>/dev/null
}

# wait returns early when a trapped signal arrives; keep waiting until the process is gone.
wait_for() { while kill -0 "$1" 2>/dev/null; do wait "$1" 2>/dev/null; done; }

daemon_loop() {
  while :; do
    rm -f "$RELOAD"
    preflight || return 1
    "$UV" sync --all-packages -q || say "uv sync failed; starting with the environment as it is"
    ((STOPPING)) && return 0
    start_daemon
    wait_for "$DAEMON_PID"
    DAEMON_PID=''
    ((STOPPING)) && return 0
    if [[ ! -e "$RELOAD" ]]; then
      say "ghostd stopped. make reload starts it again; Ctrl+C ends this"
      until [[ -e "$RELOAD" ]] || ((STOPPING)); do sleep 1; done
      ((STOPPING)) && return 0
    fi
    say "restarting ghostd"
  done
}

finish() {
  stop_all
  [[ -n "$DAEMON_PID" ]] && wait_for "$DAEMON_PID"
  [[ -n "$DESKTOP_PID" ]] && wait_for "$DESKTOP_PID"
  sleep 0.2  # let the prefix filters print the last lines
}

mode="${1:-all}"
case "$mode" in
  preflight)
    preflight
    ;;
  reload)
    if ! pgrep -f 'scripts/dev.sh( all| daemon)?$' >/dev/null; then
      say "nothing to reload: no make dev or make daemon is running"
      exit 1
    fi
    mkdir -p "$RUN_DIR" && touch "$RELOAD"
    pkill -TERM -f "$MINE" && say "ghostd restarting" || say "ghostd starting"
    ;;
  desktop)
    trap 'stop_all' INT TERM
    start_desktop
    wait_for "$DESKTOP_PID"
    finish
    ;;
  daemon | all)
    preflight || exit 1
    render_env || exit 1
    trap 'stop_all' INT TERM
    [[ "$mode" == all ]] && start_desktop
    daemon_loop
    status=$?
    finish
    exit "$status"
    ;;
  *)
    sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
