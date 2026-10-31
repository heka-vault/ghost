#!/usr/bin/env bash
# Reads a run of docs/voice-rubric.md out of the dev log, the PipeWire graph and the
# latency records. Read-only; run it after each part, as often as you like.
#
#   scripts/voice-rubric-check.sh [LOG] [LAST]     default /tmp/ghost-dev.log, last 10 turns
#   SINCE=2026-10-31T15:00 scripts/voice-rubric-check.sh   latency records newer than this
#                                                  (default: the last 6 hours)
set -uo pipefail

LOG="${1:-/tmp/ghost-dev.log}"
LAST="${2:-10}"
LATENCY="${GHOST_LATENCY:-$HOME/.local/share/ghost/latency.jsonl}"
SINCE="${SINCE:-$(date -u -d '-6 hours' +%Y-%m-%dT%H:%M)}"

if [[ ! -r "$LOG" ]]; then
  echo "no log at $LOG (GHOST_DEBUG_AUDIO=1 make dev > /tmp/ghost-dev.log 2>&1)"; exit 1
fi

bold=$'\e[1m'; off=$'\e[0m'
h() { echo; echo "${bold}== $* ==${off}"; }
count() { printf '%-50s %s\n' "$1" "$(grep -ac -- "$2" "$LOG")"; }

h "Part A: the audio path"
if command -v pw-top >/dev/null; then
  # FORMAT is the rate a node's audio has; RATE is the quantum it asked for (ghost.mic
  # shows 480/48000 there and still carries 16 kHz audio).
  top=$(pw-top -b -n 2 2>/dev/null)
  echo "$top" | grep -E 'RATE|ghost|alsa_' | tail -8
  bad=$(echo "$top" | grep -E 'ghost|alsa_' | grep -oE '(S16LE|S16|S32LE|F32P|F32) [0-9]+ [0-9]+' | awk '$3 != 16000')
  if [[ -n "$bad" ]]; then
    echo "FAIL: a node on Ghost's path carries audio at another rate: $bad"
  else
    echo "PASS: every node on Ghost's path carries 16000 Hz audio (FORMAT column)"
  fi
  driver=$(echo "$top" | awk '$1 == "R" && /alsa_input/ {print $3 "/" $4; exit}')
  echo "driver quantum/rate: ${driver:-none}   (16 kHz graph: 128/16000; 48 kHz: 256/48000)"
  echo "$top" | grep -E 'ghost.mic$|alsa_output.*reSpeaker|alsa_input.*reSpeaker' | awk '{print "xruns", $9, $NF}' | tail -3
else
  echo "pw-top not found"
fi
echo "allowed rates: $(pw-metadata -n settings 2>/dev/null | sed -n "s/.*clock.allowed-rates' value:'\([^']*\)'.*/\1/p")"
echo "forced rate:   $(pw-metadata -n settings 2>/dev/null | sed -n "s/.*clock.force-rate' value:'\([^']*\)'.*/\1/p")"
echo "daemon input:  $(grep -aoE 'audio in: [^ ]+ @ [0-9]+' "$LOG" | tail -1)"

h "Daemon"
pid=$(pgrep -f 'target/debug/ghostd run$' || true)
if [[ -n "$pid" ]]; then
  echo "ghostd pid $pid, debug audio: $(tr '\0' '\n' < /proc/"$pid"/environ | grep -c '^GHOST_DEBUG_AUDIO=1$')"
else
  echo "FAIL: no dev ghostd running (make dev)"
fi
echo "errors in the log (certificate warning and the desktop's first connects excluded): $(grep -aiE 'error|panick' "$LOG" | grep -vE 'CA root certificate|cannot reach .*api.sock|client error \(Connect\)' | wc -l)"

h "Parts B, C, E, G: turn lines in the log (latest last)"
grep -aE 'turn: (Smart Turn says|answering|went on|reopened|.*sounds unfinished|.*ends a sentence|kept open|dropped|barge-in|closed)|echo: dropped' "$LOG" \
  | sed -E 's/^ghostd +//; s/^[0-9-]+T//; s/Z +(INFO|DEBUG|WARN) +(\[audio\] )?/ /' | cut -c1-200 | tail -40

h "Counts"
count "Smart Turn says done" 'Smart Turn says done'
count "Smart Turn says not done" 'Smart Turn says not done'
count "answering while the close waits (the head start)" 'while the close waits'
count "went on after a pause (B)" 'turn: went on after'
count "reopened, nothing of the answer heard (B)" 'turn: reopened'
count "sounds unfinished, waiting (B2, C2)" 'sounds unfinished'
count "ends a sentence; closing (C4)" 'ends a sentence; closing'
count "full stop overridden by Smart Turn (C)" "but Smart Turn is sure"
count "dropped while Ghost speaks (E2)" 'while Ghost speaks'
count "barge-ins (E1)" 'barge-in on interim'
count "echo warm-up drops after the wake (G1)" "inside the canceller's warm-up"
count "heard nothing in a loud segment (H)" 'heard nothing in a'
count "wake hits under the threshold, checked" 'under 0.60: Parakeet checks it'
count "  of which not confirmed (final)" 'wake: not confirmed$'
echo "what the check read on turned-down hits:"
grep -aoE 'detector hit not confirmed: "[^"]*"' "$LOG" | sed 's/detector hit not confirmed: //' | sort | uniq -c | sort -rn | head -8 | sed 's/^/  /'

h "Part D: hold and speculation per turn (records since $SINCE, last $LAST)"
if [[ -r "$LATENCY" ]]; then
  jq -r --arg since "$SINCE" '
    select(.kind == "voice" and .ts > $since and .hear != null)
    | [ .ts[11:19], .origin,
        ("hold " + ((.hear.hold // 0) | tostring)),
        ("segments " + ((.hear.segments // 0) | tostring)),
        ("speculation " + ((.hear.speculation // "none") | tostring)),
        ("reply " + ((.reply // 0) | tostring)) ]
    | join("  ")' "$LATENCY" | tail -n "$LAST"
  sum() { jq -r --arg since "$SINCE" "select(.kind==\"voice\" and .ts > \$since) | .hear.speculation.$1 // 0" "$LATENCY" | paste -sd+ | bc; }
  kept=$(sum kept); missed=$(sum missed); withdrawn=$(sum withdrawn)
  kept=${kept:-0}; missed=${missed:-0}; withdrawn=${withdrawn:-0}
  echo "kept $kept  missed $missed  withdrawn $withdrawn"
  if [[ "$kept" -gt 0 && "$kept" -ge "$missed" ]]; then
    echo "PASS: the head start is kept on most turns"
  elif [[ "$kept" -eq 0 && "$missed" -eq 0 ]]; then
    echo "no voice turns recorded since $SINCE"
  else
    echo "FAIL: the head start is missed more than kept"
  fi
else
  echo "no latency records at $LATENCY"
fi

echo
echo "Per-stage numbers: make latency LAST=30"
