#!/usr/bin/env bash
# The rubric's log reader: every turn, wake, echo, tool and latency line the dev log
# gained since the mark, times only (no date, no level), then the mark moves to the end.
#
#   since.sh            print what is new, move the mark
#   since.sh --mark     set the mark at the end of the log without printing
#   since.sh --peek     print what is new, leave the mark
#   LOG=/tmp/other.log since.sh
set -uo pipefail

LOG="${LOG:-/tmp/ghost-dev.log}"
MARK="${MARK:-/tmp/ghost-rubric-mark}"
[[ -r "$LOG" ]] || { echo "no log at $LOG"; exit 1; }

total=$(wc -l < "$LOG")
case "${1:-}" in
  --mark) echo "$total" > "$MARK"; echo "mark at line $total"; exit 0 ;;
esac

from=$(cat "$MARK" 2>/dev/null || echo 0)
tail -n +"$((from + 1))" "$LOG" \
  | grep -av 'CA root certificate' \
  | grep -aE 'turn:|heard|interim|echo:|wake|woken|session |latency:|hang_up|tool |thinking sound cut|audio (in|out):|ducked|paused|resumed|ERROR|panick' \
  | grep -avE 'passive: speech (started|stopped)|trained detector|those phrases only' \
  | sed -E 's/^ghostd +//; s/^[0-9]{4}-[0-9]{2}-[0-9]{2}T//; s/Z +(INFO|DEBUG|WARN) +(\[audio\] )?/ /' \
  | cut -c1-260

[[ "${1:-}" == "--peek" ]] || echo "$total" > "$MARK"
