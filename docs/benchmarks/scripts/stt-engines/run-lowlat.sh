#!/bin/bash
# The streaming models at their lower-latency settings (each model's menu, read from its
# GGUF): Nemotron's lookahead 480 ms and 80 ms; Unified's chunk/lookahead 560/560, 160/160,
# 80/0 ms. Each under the shared perf lock.
set -u
B=~/.cache/ghost-stt-bench/target/release/stt-engines
S=~/.local/share/ghost/hear/sessions/2026-11-06
SO=${O:-~/.cache/ghost-stt-bench/runs}/stream
M=~/.cache/ghost-stt-bench/models
L="flock /tmp/ghost-perf.lock"
UNI=$M/parakeet-unified-en-0.6b/parakeet-unified-en-0.6b-Q8_0.gguf
NEM=$M/nemotron-speech-streaming-en-0.6b/nemotron-speech-streaming-en-0.6b-Q8_0.gguf
NEM35=$M/nemotron-3.5-asr-streaming-0.6b/nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf
run() { echo "== $*"; $L $B "$@" 2>&1 | grep -vE '^\s+[A-H][0-9]+ ' | tail -3; }
run stream nemotron-en-Q8_0-cpu4-rc6 $NEM cpu 4 $S $SO rc=6
run stream nemotron-en-Q8_0-cpu4-rc1 $NEM cpu 4 $S $SO rc=1
run stream nemotron35-Q8_0-cpu4-rc6 $NEM35 cpu 4 $S $SO rc=6
run stream unified-Q8_0-cpu4-c560r560 $UNI cpu 4 $S $SO lcr=5600,560,560
run stream unified-Q8_0-cpu4-c160r160 $UNI cpu 4 $S $SO lcr=5600,160,160
run stream unified-Q8_0-cpu4-c80r0 $UNI cpu 4 $S $SO lcr=5600,80,0
O2=${O:-~/.cache/ghost-stt-bench/runs}
run interim ghost-interim-fp32-0.3 fp32 0.3 $S $O2/interim
echo lowlat done
