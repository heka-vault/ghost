#!/usr/bin/env bash
# SPIKE: what the floating window costs, sampled from /proc and the GPU's RC6 counter.
#
#   measure.sh <label> <seconds> [pid]
#
# Prints CPU % of one core for the window's process tree (the shell plus WebKit's web and
# network processes), for gnome-shell (the compositor) and for Xwayland, and the Intel GPU's
# busy share (1 - RC6 residency) with its mean clock. Without a pid it measures the desktop
# with no floating window (the baseline). Nothing is written anywhere.
set -euo pipefail
label=$1 secs=$2 root=${3:-}
hz=$(getconf CLK_TCK)
gt=$(ls -d /sys/class/drm/card*/gt/gt0 2>/dev/null | head -1)

tree() { # the pid and all its descendants
  local p=$1; echo "$p"
  for c in $(cat /proc/"$p"/task/*/children 2>/dev/null); do tree "$c"; done
}
ticks() { # utime+stime of the given pids
  local s=0
  for p in "$@"; do
    [[ -r /proc/$p/stat ]] || continue
    read -r -a f < <(sed 's/^.*) //' /proc/"$p"/stat)
    s=$((s + f[11] + f[12]))
  done
  echo $s
}
shell=$(pgrep -x gnome-shell | head -1)
xw=$(pgrep -x Xwayland | head -1 || true)
pids=(); [[ -n "$root" ]] && mapfile -t pids < <(tree "$root")

t0=$(date +%s.%N); a0=$(ticks "${pids[@]}"); s0=$(ticks $shell); x0=$(ticks $xw)
r0=$(cat "$gt/rc6_residency_ms"); f=0; n=0
end=$(( $(date +%s) + secs ))
while (( $(date +%s) < end )); do f=$((f + $(cat "$gt/rps_act_freq_mhz"))); n=$((n + 1)); sleep 0.25; done
t1=$(date +%s.%N); a1=$(ticks "${pids[@]}"); s1=$(ticks $shell); x1=$(ticks $xw)
r1=$(cat "$gt/rc6_residency_ms")

awk -v l="$label" -v dt="$(echo "$t1 - $t0" | bc)" -v hz="$hz" \
  -v a=$((a1 - a0)) -v s=$((s1 - s0)) -v x=$((x1 - x0)) -v r=$((r1 - r0)) -v f=$f -v n=$n -v np=${#pids[@]} \
  'BEGIN { printf "%-34s window %5.1f%% (%d procs)  gnome-shell %5.1f%%  Xwayland %4.1f%%  gpu busy %5.1f%% @ %4.0f MHz\n",
     l, 100*a/hz/dt, np, 100*s/hz/dt, 100*x/hz/dt, 100*(1 - r/(dt*1000)), f/n }'
