#!/usr/bin/env bash
# Install (or remove) Ghost's echo-cancelled microphone.
#
# Renders ghost-echo-cancel.conf.in with this machine's device names into
# ~/.config/ghost/echo-cancel.conf, installs ghost-echo-cancel.service as a
# user unit, starts it, and checks that the "ghost.mic" source exists. The
# module runs in its own PipeWire client process, so PipeWire itself is never
# restarted and other applications keep playing.
#
# Device names are machine-specific and stay out of the repository:
#   GHOST_MIC_NODE   capture node to clean   (default: `pactl get-default-source`)
#   GHOST_SINK_NODE  sink used as reference  (default: `pactl get-default-sink`)
#
# Usage: install.sh [--uninstall | --dry-run]
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
TEMPLATE="$HERE/ghost-echo-cancel.conf.in"
UNIT_SRC="$HERE/ghost-echo-cancel.service"
UNIT="ghost-echo-cancel.service"
CONF_DIR="$HOME/.config/ghost"
CONF="$CONF_DIR/echo-cancel.conf"
UNIT_DIR="$HOME/.config/systemd/user"
SOURCE_NAME="ghost.mic"
LEGACY="$HOME/.config/pipewire/pipewire.conf.d/90-ghost-echo-cancel.conf"

have_node() { { pactl list short sources; pactl list short sinks; } | awk '{print $2}' | grep -qx "$1"; }

if [[ "${1:-}" == "--uninstall" ]]; then
  systemctl --user disable --now "$UNIT" 2>/dev/null || true
  rm -f "$UNIT_DIR/$UNIT" "$CONF" "$LEGACY"
  systemctl --user daemon-reload
  echo "removed $UNIT and $CONF"
  exit 0
fi

command -v pactl >/dev/null || { echo "install.sh: pactl not found" >&2; exit 1; }
command -v pipewire >/dev/null || { echo "install.sh: pipewire binary not found" >&2; exit 1; }

MIC="${GHOST_MIC_NODE:-$(pactl get-default-source)}"
SINK="${GHOST_SINK_NODE:-$(pactl get-default-sink)}"

# Re-running: the default source may now be ghost.mic itself. Reuse the real
# capture node from the rendered file instead of nesting.
if [[ "$MIC" == "$SOURCE_NAME" ]]; then
  if [[ -r "$CONF" ]]; then
    MIC=$(sed -n 's/.*node.target = "\(.*\)"/\1/p' "$CONF" | head -1)
  else
    echo "install.sh: default source is $SOURCE_NAME; set GHOST_MIC_NODE to the real mic" >&2
    exit 1
  fi
fi

for node in "$MIC" "$SINK"; do
  if ! have_node "$node"; then
    echo "install.sh: no PipeWire node named $node (see: pactl list short sources sinks)" >&2
    exit 1
  fi
done

RENDERED=$(sed -e "s|@MIC@|$MIC|g" -e "s|@SINK@|$SINK|g" "$TEMPLATE")

if [[ "${1:-}" == "--dry-run" ]]; then
  echo "would write $CONF and $UNIT_DIR/$UNIT, then start $UNIT:"
  echo "$RENDERED"
  exit 0
fi

if [[ -e "$LEGACY" ]]; then
  # Earlier installs loaded the module inside the main daemon; that would give
  # two ghost.mic sources at next login.
  rm -f "$LEGACY"
  echo "removed legacy $LEGACY"
fi

mkdir -p "$CONF_DIR" "$UNIT_DIR"
printf '%s\n' "$RENDERED" > "$CONF"
install -m 644 "$UNIT_SRC" "$UNIT_DIR/$UNIT"
systemctl --user daemon-reload
systemctl --user enable --now "$UNIT" >/dev/null 2>&1
systemctl --user restart "$UNIT"   # pick up a re-rendered conf on re-runs

for _ in $(seq 1 40); do
  if have_node "$SOURCE_NAME"; then
    echo "installed $UNIT (conf $CONF)"
    echo "source $SOURCE_NAME ready (capture $MIC, reference $SINK)"
    echo "set GHOST_INPUT_DEVICE=$SOURCE_NAME for ghostd"
    exit 0
  fi
  sleep 0.25
done
echo "install.sh: $SOURCE_NAME did not appear; check: journalctl --user -u $UNIT -n 50" >&2
exit 1
