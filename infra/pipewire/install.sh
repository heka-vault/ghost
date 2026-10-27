#!/usr/bin/env bash
# Install (or remove) Ghost's echo-cancelled microphone.
#
# Renders ghost-echo-cancel.conf.in with this machine's device names into
# ~/.config/ghost/echo-cancel.conf, installs ghost-echo-cancel.service as a
# user unit, starts it, and checks that the "ghost.mic" source exists. The
# module runs in its own PipeWire client process, so PipeWire itself is never
# restarted and other applications keep playing (the one exception is
# --sink-drives, below, which restarts WirePlumber because the operator asked).
#
# Device names are machine-specific and stay out of the repository:
#   GHOST_MIC_NODE     capture node to clean   (default: `pactl get-default-source`)
#   GHOST_SINK_NODE    sink used as reference  (default: `pactl get-default-sink`)
#   GHOST_MIC_CHANNEL  one channel of a multichannel mic to use alone, e.g. FL for
#                      a mic array's processed beam (default: MONO, all channels
#                      mixed down; a re-run keeps the rendered file's choice)
#
# The echo canceller links the mic and the sink's monitor into one PipeWire graph,
# so when they are different devices their two hardware clocks share one driver
# and the follower is resynced every few seconds: crackle in music when the sink
# follows, a few ms skipped in ghost.mic when the mic follows. PipeWire picks the
# capture device by default. --sink-drives makes the sink the driver instead: it
# writes a WirePlumber rule for the sink and restarts WirePlumber, which stops
# playback for a moment (README.md, "Two clocks"). A re-run keeps the rule and
# re-renders it for the current sink; --uninstall removes it.
#
# Usage: install.sh [--uninstall | --dry-run] [--sink-drives]
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
DRIVER_CONF="$HOME/.config/wireplumber/wireplumber.conf.d/51-ghost-sink-driver.conf"

MODE="" SINK_DRIVES=false
for arg in "$@"; do
  case "$arg" in
    --uninstall | --dry-run) MODE="$arg" ;;
    --sink-drives) SINK_DRIVES=true ;;
    *) echo "usage: install.sh [--uninstall | --dry-run] [--sink-drives]" >&2; exit 2 ;;
  esac
done

have_node() { { pactl list short sources; pactl list short sinks; } | awk '{print $2}' | grep -qx "$1"; }

# The PipeWire device a node belongs to; nodes on one device share its clock.
device_of() {
  { pactl list sources; pactl list sinks; } \
    | awk -v n="$1" '$1 == "Name:" { f = ($2 == n) } f && $1 == "device.id" { gsub(/"/, "", $3); print $3; exit }'
}

driver_rule() {
  cat <<RULE
# Rendered by Ghost's infra/pipewire/install.sh --sink-drives; re-run the installer
# rather than editing. The echo canceller puts the mic and this sink on one clock;
# this makes the sink keep time, so music doesn't crackle and the mic is the one
# resynced instead (infra/pipewire/README.md, "Two clocks").
monitor.alsa.rules = [
  {
    matches = [ { node.name = "$1" } ]
    actions = { update-props = { priority.driver = 3000 } }
  }
]
RULE
}

if [[ "$MODE" == "--uninstall" ]]; then
  systemctl --user disable --now "$UNIT" 2>/dev/null || true
  rm -f "$UNIT_DIR/$UNIT" "$CONF" "$LEGACY"
  systemctl --user daemon-reload
  echo "removed $UNIT and $CONF"
  if [[ -e "$DRIVER_CONF" ]]; then
    # Without the echo canceller nothing joins the two clocks, so the rule is
    # harmless until WirePlumber next restarts; don't interrupt playback for it.
    rm -f "$DRIVER_CONF"
    echo "removed $DRIVER_CONF (applies at the next WirePlumber restart)"
  fi
  exit 0
fi

command -v pactl >/dev/null || { echo "install.sh: pactl not found" >&2; exit 1; }
command -v pipewire >/dev/null || { echo "install.sh: pipewire binary not found" >&2; exit 1; }

MIC="${GHOST_MIC_NODE:-$(pactl get-default-source)}"
SINK="${GHOST_SINK_NODE:-$(pactl get-default-sink)}"
CHANNEL="${GHOST_MIC_CHANNEL:-}"

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

# Same mic as the rendered file and no channel asked for: keep its channel, so a
# re-run from the desktop app doesn't quietly go back to mixing an array down.
if [[ -z "$CHANNEL" && -r "$CONF" ]] \
  && [[ "$(sed -n 's/.*node.target = "\(.*\)"/\1/p' "$CONF" | head -1)" == "$MIC" ]]; then
  CHANNEL=$(sed -n '/ghost.mic.capture/,/}/s/.*audio.position = \[ *\([A-Z0-9]*\) *\]/\1/p' "$CONF" | head -1)
fi
CHANNEL="${CHANNEL:-MONO}"
DONT_REMIX=$([[ "$CHANNEL" == "MONO" ]] && echo false || echo true)

for node in "$MIC" "$SINK"; do
  if ! have_node "$node"; then
    echo "install.sh: no PipeWire node named $node (see: pactl list short sources sinks)" >&2
    exit 1
  fi
done

RENDERED=$(sed -e "s|@MIC@|$MIC|g" -e "s|@SINK@|$SINK|g" -e "s|@CHANNEL@|$CHANNEL|g" \
  -e "s|@DONT_REMIX@|$DONT_REMIX|g" "$TEMPLATE")

TWO_CLOCKS=false
[[ "$(device_of "$MIC")" != "$(device_of "$SINK")" ]] && TWO_CLOCKS=true

# The rule is opt-in; once in place a re-run keeps it, following the current sink.
DRIVER_RULE=""
if $SINK_DRIVES || [[ -e "$DRIVER_CONF" ]]; then
  if [[ "$SINK" != alsa_output.* ]]; then
    echo "install.sh: --sink-drives needs an ALSA sink; $SINK is not one" >&2
    exit 1
  fi
  DRIVER_RULE=$(driver_rule "$SINK")
fi

if [[ "$MODE" == "--dry-run" ]]; then
  echo "would write $CONF and $UNIT_DIR/$UNIT, then start $UNIT:"
  echo "$RENDERED"
  if [[ -n "$DRIVER_RULE" ]]; then
    echo
    echo "would write $DRIVER_CONF$($SINK_DRIVES && echo ", then restart WirePlumber"):"
    echo "$DRIVER_RULE"
  elif $TWO_CLOCKS; then
    echo
    echo "$MIC and $SINK are different devices; see --sink-drives"
  fi
  exit 0
fi

if [[ -n "$DRIVER_RULE" ]]; then
  CHANGED=true
  [[ -r "$DRIVER_CONF" && "$(cat "$DRIVER_CONF")" == "$DRIVER_RULE" ]] && CHANGED=false
  mkdir -p "$(dirname "$DRIVER_CONF")"
  printf '%s\n' "$DRIVER_RULE" > "$DRIVER_CONF"
  if $SINK_DRIVES; then
    # Asked for by name, so restart now; playback stops for a moment. Wait for
    # the devices to come back before the echo canceller is restarted below.
    systemctl --user restart wireplumber
    for _ in $(seq 1 40); do
      have_node "$MIC" && have_node "$SINK" && break
      sleep 0.25
    done
    echo "wrote $DRIVER_CONF and restarted WirePlumber: $SINK keeps time"
  elif $CHANGED; then
    echo "re-rendered $DRIVER_CONF for $SINK; it applies at the next WirePlumber restart" \
      "(systemctl --user restart wireplumber)"
  fi
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
    echo "source $SOURCE_NAME ready (capture $MIC channel $CHANNEL, reference $SINK)"
    echo "set GHOST_INPUT_DEVICE=$SOURCE_NAME for ghostd"
    if $TWO_CLOCKS && [[ -z "$DRIVER_RULE" ]]; then
      echo "note: $MIC and $SINK are different devices, now on one clock; music on" \
        "the speakers may crackle. Re-run with --sink-drives to make the speakers" \
        "keep time (infra/pipewire/README.md, \"Two clocks\")."
    fi
    exit 0
  fi
  sleep 0.25
done
echo "install.sh: $SOURCE_NAME did not appear; check: journalctl --user -u $UNIT -n 50" >&2
exit 1
