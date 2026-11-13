#!/usr/bin/env bash
# A microphone for a meeting that carries Ghost's voice too (docs/floating-room.md, What the
# people on the call hear). Runs for the length of a demo; Ctrl+C (or closing the terminal)
# removes everything it made. Nothing of Ghost's own audio input changes: Ghost still listens
# where it always does, and its voice still plays on your speakers or headphones.
#
#   scripts/demo-audio.sh            (or: make demo-audio)
#   GHOST_DEMO_MIC=<source node.name> scripts/demo-audio.sh   to choose the microphone
#
# What it makes, with PipeWire's pulse modules (pactl):
#   ghost-demo-mix  a sink nobody hears: your microphone and Ghost's voice are mixed into it
#   ghost-demo      "Ghost and your microphone": a source made from that mix. Pick it as the
#                   microphone in the meeting.
# and, while it runs, links Ghost's voice (the `ghost-playback` stream, opened for each
# session) into the mix as well as to wherever it already plays.
set -euo pipefail

MIX=ghost-demo-mix
SRC=ghost-demo
PLAYBACK=ghost-playback

for tool in pactl pw-link; do
  command -v "$tool" >/dev/null || { echo "demo-audio: needs $tool (PipeWire's pulse and tools)" >&2; exit 1; }
done
if pactl list short sinks | cut -f2 | grep -qx "$MIX"; then
  echo "demo-audio: $MIX is already there: another demo-audio is running (Ctrl+C it first)." >&2
  exit 1
fi

# Your microphone: the echo-cancelled one when Ghost's canceller made it, else the default.
mic=${GHOST_DEMO_MIC:-}
if [ -z "$mic" ]; then
  if pactl list short sources | cut -f2 | grep -qx ghost.mic; then mic=ghost.mic; else mic=$(pactl get-default-source); fi
fi

modules=()
cleanup() {
  trap - EXIT INT TERM
  for ((i = ${#modules[@]} - 1; i >= 0; i--)); do pactl unload-module "${modules[$i]}" 2>/dev/null || true; done
  echo
  echo "demo-audio: removed. Switch the meeting's microphone back to your own."
  exit 0
}
trap cleanup EXIT INT TERM

modules+=("$(pactl load-module module-null-sink sink_name=$MIX rate=48000 channels=1 channel_map=mono \
  sink_properties='device.description="Ghost demo mix"')")
modules+=("$(pactl load-module module-remap-source master=$MIX.monitor source_name=$SRC channels=1 channel_map=mono \
  source_properties='device.description="Ghost and your microphone"')")
modules+=("$(pactl load-module module-loopback source="$mic" sink=$MIX latency_msec=30 \
  source_dont_move=true sink_dont_move=true sink_input_properties='media.name="Your microphone into the Ghost demo mix"')")

cat <<EOF
demo-audio: ready.
  In the meeting, choose the microphone "Ghost and your microphone".
  Your microphone ($mic) and Ghost's voice are mixed into it.
  Headphones are best: then the call's voices can't reach Ghost's microphone, and the
  meeting's own echo cancelling has nothing of Ghost's to fight.
  Ctrl+C when the demo is over.
EOF

# Ghost opens its voice for each session: link it into the mix whenever it is there.
linked=""
while true; do
  outs=$(pw-link -o 2>/dev/null | grep "^$PLAYBACK:" || true)
  if [ -n "$outs" ]; then
    ins=$(pw-link -i 2>/dev/null | grep "^$MIX:" || true)
    if [ -n "$ins" ] && ! pw-link -l 2>/dev/null | grep -A3 "^$PLAYBACK:" | grep -q "$MIX:"; then
      # Every channel of Ghost's voice into the mix's one.
      while read -r out; do
        while read -r in; do pw-link "$out" "$in" 2>/dev/null || true; done <<<"$ins"
      done <<<"$outs"
      [ "$linked" = yes ] || echo "demo-audio: Ghost's voice is in the mix."
      linked=yes
    fi
  fi
  sleep 0.25
done
