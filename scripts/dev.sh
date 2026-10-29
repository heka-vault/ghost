#!/usr/bin/env bash
# Ghost from this checkout, for working on it, with its output in this terminal.
#
#   scripts/dev.sh            the daemon and the desktop app          (make dev)
#   scripts/dev.sh daemon     the daemon only                         (make daemon)
#   scripts/dev.sh desktop    the desktop app only                    (make desktop)
#   scripts/dev.sh reload     rebuild and restart the daemon a running one started (make reload)
#   scripts/dev.sh preflight  stop the service, refuse if a ghostd runs (make start uses it)
#
# The daemon is the working tree's `cargo build -p ghostd` (ADR 0039; target/debug/ghostd).
# Only one Ghost may have the microphone, so starting it stops the ghostd user service first
# (the headless Ghost, a `make deploy` build); `make service-on` starts that again. The
# desktop app is `tauri dev`: Vite reloads the UI on save and a Rust change rebuilds the
# window. The app adopts the daemon this script runs rather than starting its own (ADR 0038;
# GHOST_SHELL_ADOPT_ONLY). A change to the daemon's crates needs it rebuilt and restarted,
# which `make reload` does from another terminal without touching the desktop. If the daemon
# exits on its own (a crash, `make stop`) it stays down until `make reload`. Ctrl+C stops
# everything this script started.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# Settings only: passwords are in Ghost's vault, which the daemon opens itself (ADR 0036).
ENV_FILE="${ENV_FILE:-$HOME/.config/ghost/settings.env}"
RUN_DIR="${XDG_RUNTIME_DIR:-/tmp}/ghost"
RELOAD="$RUN_DIR/dev-reload"
GHOSTD="$ROOT/target/debug/ghostd"
MINE="$ROOT/target/debug/ghostd run$"  # this checkout's daemon, and no other

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
    say "a ghostd is already running (pid ${pids//$'\n'/ }); make stop, or Quit Ghost in its tray, then try again"
    return 1
  fi
}

# Each process gets its own session (setsid), so Ctrl+C reaches only this script, which
# then stops them in order: the daemon gets TERM like systemd sends, the desktop's whole
# process group (pnpm, tauri, Vite, the window) gets TERM too.
DAEMON_PID='' DESKTOP_PID='' STOPPING=0

# GHOST_LAUNCHER tells the daemon it was started here, so the desktop says make reload.
start_daemon() {
  setsid env GHOST_LAUNCHER=make-dev bash -c 'set -a; [[ -r "$1" ]] && . "$1"; set +a; exec "$2" run' _ "$ENV_FILE" "$GHOSTD" \
    </dev/null > >(prefix "ghostd " 36) 2>&1 &
  DAEMON_PID=$!
}

# The app only adopts the daemon on the socket here; this script is what starts it.
start_desktop() {
  setsid env GHOST_SHELL_ADOPT_ONLY=1 pnpm -s tauri dev </dev/null > >(prefix "desktop" 35) 2>&1 &
  DESKTOP_PID=$!
}

# The daemon compiled from the working tree, and the ONNX Runtime it loads in the cache
# (scripts/onnxruntime.sh fetches it once).
build_daemon() {
  cargo build -q -p ghostd || { say "cargo build -p ghostd failed"; return 1; }
  if [[ -x scripts/onnxruntime.sh ]]; then
    scripts/onnxruntime.sh >/dev/null || { say "scripts/onnxruntime.sh failed"; return 1; }
  fi
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
    build_daemon || return 1
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
    say "rebuilding and restarting ghostd"
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
    pkill -TERM -f "$MINE" && say "ghostd restarting (rebuilt first)" || say "ghostd starting"
    ;;
  desktop)
    trap 'stop_all' INT TERM
    start_desktop
    wait_for "$DESKTOP_PID"
    finish
    ;;
  daemon | all)
    preflight || exit 1
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
