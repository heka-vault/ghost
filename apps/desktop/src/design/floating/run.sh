#!/usr/bin/env bash
# SPIKE: run Ghost's floating surface with mock data (docs/design/floating-2026-11.md).
#
#   apps/desktop/src/design/floating/run.sh            # start it (idle: the ember only)
#   apps/desktop/src/design/floating/run.sh demo       # start it and play the summon sequence
#   apps/desktop/src/design/floating/run.sh send demo  # tell a running one: demo | summon |
#                                                      #   dismiss | state <phase> | mic off|on | yes | no | quit
#
# Environment: GDK_BACKEND=x11 for XWayland; FLOATING_PLACE, FLOATING_MONITOR, FLOATING_IDLE,
# FLOATING_FOCUSABLE as in apps/desktop/src-tauri/examples/floating.rs. It starts no daemon and
# touches no microphone, speaker or Ghost data. Ctrl+C (or `send quit`) closes everything.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../.." && pwd)"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
bin="$repo/target/debug/examples/floating"

if [[ "${1:-}" == "send" ]]; then
  shift
  exec "$bin" send "$@"
fi

(cd "$repo" && cargo build -q -p heka-ghost-desktop --example floating --features floating)

vite_pid=""
if ! curl -sf -o /dev/null http://localhost:1431/src/design/floating/index.html; then
  (cd "$repo/apps/desktop" && exec pnpm exec vite --port 1431 --strictPort --logLevel warn) &
  vite_pid=$!
  for _ in $(seq 50); do curl -sf -o /dev/null http://localhost:1431/src/design/floating/index.html && break; sleep 0.2; done
fi
cleanup() { [[ -n "$vite_pid" ]] && kill "$vite_pid" 2>/dev/null || true; }
trap cleanup EXIT

if [[ "${1:-}" == "demo" ]]; then
  (sleep 2.5 && "$bin" send demo) &
fi
"$bin"
